use anyhow::{Context, Result};
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use glob::Pattern;
use ignore::WalkBuilder;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;
use tokio::task;

use super::path_filters::apply_walk_overrides;

const MAX_FUZZY_RESULTS: usize = 500;

pub fn schema() -> Value {
    json!({
        "name": "fuzzy_find",
        "description": "Perform fast fuzzy path and file-name search using the metadata/path index when available. Use this before text_search to discover precise directories/files in large repositories; prefer concrete basename/path tokens, target_type/extensions filters, and scoped paths over broad natural-language patterns.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Fuzzy path/name pattern. For best results in large repos, use distinctive path tokens such as browser net url_loader rather than a prose query." },
                "paths": { "type": "array", "items": { "type": "string" }, "description": "Search roots or files. Defaults to the active workspace; scope this when you already know a subsystem." },
                "target_type": { "type": "string", "enum": ["file", "dir", "any"], "description": "Limit matches to files, directories, or both. Use dir to find a good text_search scope." },
                "extensions": { "type": "array", "items": { "type": "string" }, "description": "Optional file extensions without dots, e.g. rs or cc." },
                "max_depth": { "type": "integer", "description": "Optional traversal depth for filesystem fallback." },
                "max_results": { "type": "integer", "description": "Maximum ranked matches to return; defaults are capped to keep responses usable." }
            },
            "required": ["pattern"]
        }
    })
}

#[derive(Clone, Debug)]
struct RankedMatch {
    path: String,
    relative_path: String,
    score: i64,
    entry_type: &'static str,
    size: u64,
    modified_at: u64,
}

#[derive(Default, Debug)]
struct FuzzyStats {
    entries_scanned: usize,
    indexed_roots_used: usize,
    partial_index_roots_used: usize,
    filesystem_roots_walked: usize,
    broad_query_roots_skipped: usize,
    warnings: Vec<String>,
}

pub async fn execute(args: &Value) -> Result<Value> {
    let args_owned = args.clone();
    task::spawn_blocking(move || execute_blocking(args_owned))
        .await
        .context("fuzzy_find background task failed to join")?
}

fn execute_blocking(args: Value) -> Result<Value> {
    let pattern = args
        .get("pattern")
        .and_then(|v| v.as_str())
        .context("Missing/empty pattern")?;
    let paths: Vec<PathBuf> = args
        .get("paths")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| p.as_str())
                .map(crate::common::resolve_tool_path)
                .collect()
        })
        .unwrap_or_else(|| vec![crate::common::default_tool_root()]);

    let target_type = args
        .get("target_type")
        .and_then(|v| v.as_str())
        .unwrap_or("any");
    let max_results = parse_usize_arg(&args, "max_results", 50, 0, MAX_FUZZY_RESULTS);
    let max_depth = args.get("max_depth").and_then(|v| v.as_u64());
    let extensions: Vec<String> = args
        .get("extensions")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| {
                    p.as_str().and_then(|s| {
                        let extension = s.trim().trim_start_matches('.').to_ascii_lowercase();
                        (!extension.is_empty()).then_some(extension)
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let search_roots: Vec<PathBuf> = paths
        .iter()
        .map(|path| path.canonicalize().unwrap_or_else(|_| path.clone()))
        .collect();
    if let Some(glob_pattern) = compile_glob_pattern(pattern) {
        return execute_glob_find(
            pattern,
            &glob_pattern,
            &search_roots,
            target_type,
            &extensions,
            max_depth,
            max_results,
        );
    }

    let matcher = SkimMatcherV2::default();
    let filter_terms = pattern_filter_terms(pattern);
    let mut ranked = Vec::new();
    let mut seen_paths = HashSet::new();
    let mut stats = FuzzyStats::default();

    for root in &search_roots {
        process_search_root(
            root,
            &search_roots,
            pattern,
            target_type,
            &extensions,
            &filter_terms,
            max_depth,
            max_results,
            &matcher,
            &mut ranked,
            &mut seen_paths,
            &mut stats,
        )?;
    }

    ranked.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.relative_path.cmp(&right.relative_path))
            .then_with(|| left.path.cmp(&right.path))
    });

    let results: Vec<Value> = ranked
        .into_iter()
        .take(max_results)
        .map(|item| {
            json!({
                "path": item.path,
                "relative_path": item.relative_path,
                "score": item.score,
                "type": item.entry_type,
                "size": item.size,
                "modified_at": item.modified_at
            })
        })
        .collect();

    let search_strategy = match (
        stats.indexed_roots_used > 0,
        stats.filesystem_roots_walked > 0,
    ) {
        (true, true) => "mixed",
        (true, false) => "index",
        (false, true) => "filesystem_walk",
        (false, false) => "none",
    };
    let limit_reached = max_results > 0 && results.len() >= max_results;

    Ok(json!({
        "results": results,
        "total_returned": results.len(),
        "limit_reached": limit_reached,
        "limit_reason": if limit_reached { Some("max_results") } else { None },
        "search_strategy": search_strategy,
        "entries_scanned": stats.entries_scanned,
        "indexed_roots_used": stats.indexed_roots_used,
        "partial_index_roots_used": stats.partial_index_roots_used,
        "filesystem_roots_walked": stats.filesystem_roots_walked,
        "broad_query_roots_skipped": stats.broad_query_roots_skipped,
        "warnings": stats.warnings,
        "index_complete": stats.partial_index_roots_used == 0
    }))
}

fn execute_glob_find(
    pattern: &str,
    glob_pattern: &Pattern,
    search_roots: &[PathBuf],
    target_type: &str,
    extensions: &[String],
    max_depth: Option<u64>,
    max_results: usize,
) -> Result<Value> {
    let mut ranked = Vec::new();
    let mut seen_paths = HashSet::new();
    let mut stats = FuzzyStats::default();

    for root in search_roots {
        process_glob_root(
            root,
            search_roots,
            glob_pattern,
            target_type,
            extensions,
            max_depth,
            max_results,
            &mut ranked,
            &mut seen_paths,
            &mut stats,
        )?;
    }

    ranked.sort_by(compare_ranked_match);
    let results: Vec<Value> = ranked
        .into_iter()
        .take(max_results)
        .map(|item| {
            json!({
                "path": item.path,
                "relative_path": item.relative_path,
                "score": item.score,
                "type": item.entry_type,
                "size": item.size,
                "modified_at": item.modified_at
            })
        })
        .collect();
    let search_strategy = match (
        stats.indexed_roots_used > 0,
        stats.filesystem_roots_walked > 0,
    ) {
        (true, true) => "mixed",
        (true, false) => "index",
        (false, true) => "filesystem_walk",
        (false, false) => "none",
    };
    let limit_reached = max_results > 0 && results.len() >= max_results;

    Ok(json!({
        "results": results,
        "total_returned": results.len(),
        "limit_reached": limit_reached,
        "limit_reason": if limit_reached { Some("max_results") } else { None },
        "search_strategy": search_strategy,
        "pattern_kind": "glob",
        "entries_scanned": stats.entries_scanned,
        "indexed_roots_used": stats.indexed_roots_used,
        "partial_index_roots_used": stats.partial_index_roots_used,
        "filesystem_roots_walked": stats.filesystem_roots_walked,
        "broad_query_roots_skipped": stats.broad_query_roots_skipped,
        "warnings": stats.warnings,
        "index_complete": stats.partial_index_roots_used == 0,
        "note": format!("Pattern '{}' was treated as a glob; match is applied to relative path and file name.", pattern)
    }))
}

#[allow(clippy::too_many_arguments)]
fn process_glob_root(
    root: &Path,
    all_roots: &[PathBuf],
    glob_pattern: &Pattern,
    target_type: &str,
    extensions: &[String],
    max_depth: Option<u64>,
    max_results: usize,
    ranked: &mut Vec<RankedMatch>,
    seen_paths: &mut HashSet<String>,
    stats: &mut FuzzyStats,
) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }

    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let index_ready = crate::indexer::is_path_index_ready(&canonical_root);
    let has_index = crate::indexer::is_path_index_available(&canonical_root);
    let has_glob_anchors = !glob_anchor_terms(glob_pattern.as_str()).is_empty();
    if has_index
        && let Some(candidates) = crate::indexer::query_path_candidates(
            &canonical_root,
            glob_pattern.as_str(),
            indexed_shortlist_limit(max_results),
        )
    {
        stats.indexed_roots_used += 1;
        if !index_ready {
            stats.partial_index_roots_used += 1;
        }
        let ranked_before = ranked.len();
        for candidate in candidates {
            stats.entries_scanned = stats.entries_scanned.saturating_add(1);
            consider_glob_record(
                &candidate.path,
                candidate.is_dir,
                all_roots,
                glob_pattern,
                target_type,
                extensions,
                max_depth,
                max_results,
                ranked,
                seen_paths,
                Some((candidate.size, candidate.modified_at)),
            );
        }
        if ranked.len() > ranked_before || (index_ready && has_glob_anchors) {
            return Ok(());
        }
    } else if index_ready && has_glob_anchors {
        stats.indexed_roots_used += 1;
        return Ok(());
    }

    if let Some(entries) = crate::indexer::indexed_entries_under(&canonical_root) {
        stats.indexed_roots_used += 1;
        if !index_ready {
            stats.partial_index_roots_used += 1;
        }
        for entry in entries {
            stats.entries_scanned = stats.entries_scanned.saturating_add(1);
            consider_glob_record(
                &entry.path,
                entry.is_dir,
                all_roots,
                glob_pattern,
                target_type,
                extensions,
                max_depth,
                max_results,
                ranked,
                seen_paths,
                Some((entry.size, entry.modified_at)),
            );
        }
        return Ok(());
    }

    let mut walk = WalkBuilder::new(&canonical_root);
    walk.hidden(true)
        .ignore(true)
        .git_ignore(true)
        .git_exclude(true)
        .require_git(false)
        .threads(crate::common::bounded_walk_threads());
    if let Some(depth) = max_depth {
        walk.max_depth(Some(depth as usize));
    }
    let extension_globs = extension_override_globs(target_type, extensions);
    if !extension_globs.is_empty() {
        apply_walk_overrides(&mut walk, &canonical_root, &extension_globs, &[])?;
    }

    stats.filesystem_roots_walked += 1;
    for entry in walk.build().flatten() {
        stats.entries_scanned = stats.entries_scanned.saturating_add(1);
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        consider_glob_record(
            entry.path(),
            file_type.is_dir(),
            all_roots,
            glob_pattern,
            target_type,
            extensions,
            max_depth,
            max_results,
            ranked,
            seen_paths,
            None,
        );
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn consider_glob_record(
    path: &Path,
    is_dir: bool,
    roots: &[PathBuf],
    glob_pattern: &Pattern,
    target_type: &str,
    extensions: &[String],
    max_depth: Option<u64>,
    max_results: usize,
    ranked: &mut Vec<RankedMatch>,
    seen_paths: &mut HashSet<String>,
    cached_meta: Option<(u64, u64)>,
) {
    if target_type == "file" && is_dir {
        return;
    }
    if target_type == "dir" && !is_dir {
        return;
    }
    if !extensions.is_empty() && !is_dir {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();
        if !extensions.contains(&ext) {
            return;
        }
    }

    let relative_path = compute_relative_path(path, roots);
    if !within_max_depth(&relative_path, max_depth) {
        return;
    }

    let relative_lower = relative_path.to_ascii_lowercase();
    let file_name_lower = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !glob_pattern.matches(&relative_lower) && !glob_pattern.matches(&file_name_lower) {
        return;
    }

    let normalized_path = normalize_path(path);
    if !seen_paths.insert(normalized_path.clone()) {
        return;
    }
    let (size, modified_at) = cached_meta.unwrap_or_else(|| read_entry_metadata(path));
    let depth_penalty = relative_path.split('/').count() as i64 * 10;
    let length_penalty = relative_path.len().min(9_000) as i64;
    push_ranked_match(
        ranked,
        RankedMatch {
            path: normalized_path,
            relative_path,
            score: 10_000 - depth_penalty - length_penalty,
            entry_type: if is_dir { "dir" } else { "file" },
            size,
            modified_at,
        },
        max_results,
    );
}

#[allow(clippy::too_many_arguments)]
fn process_search_root(
    root: &Path,
    all_roots: &[PathBuf],
    pattern: &str,
    target_type: &str,
    extensions: &[String],
    filter_terms: &[String],
    max_depth: Option<u64>,
    max_results: usize,
    matcher: &SkimMatcherV2,
    ranked: &mut Vec<RankedMatch>,
    seen_paths: &mut HashSet<String>,
    stats: &mut FuzzyStats,
) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }

    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

    if canonical_root.is_file() {
        consider_candidate(
            &canonical_root,
            false,
            all_roots,
            pattern,
            target_type,
            extensions,
            filter_terms,
            None,
            max_results,
            matcher,
            ranked,
            seen_paths,
            None,
        );
        return Ok(());
    }

    if !canonical_root.is_dir() {
        return Ok(());
    }

    let index_complete = crate::indexer::is_path_index_ready(&canonical_root);
    if crate::indexer::is_path_index_available(&canonical_root)
        && let Some(candidates) = crate::indexer::query_path_candidates(
            &canonical_root,
            pattern,
            indexed_shortlist_limit(max_results),
        )
    {
        let ranked_before_index = ranked.len();
        stats.indexed_roots_used += 1;
        if !index_complete {
            stats.partial_index_roots_used += 1;
        }

        if target_type != "file" {
            consider_candidate(
                &canonical_root,
                true,
                all_roots,
                pattern,
                target_type,
                extensions,
                filter_terms,
                max_depth,
                max_results,
                matcher,
                ranked,
                seen_paths,
                None,
            );
        }

        for candidate in candidates {
            consider_indexed_candidate(
                &candidate,
                all_roots,
                pattern,
                target_type,
                extensions,
                filter_terms,
                max_depth,
                max_results,
                matcher,
                ranked,
                seen_paths,
            );
        }

        if ranked.len() > ranked_before_index {
            return Ok(());
        }

        if index_complete
            && should_skip_broad_filesystem_fallback(pattern, filter_terms, extensions, max_depth)
        {
            stats.broad_query_roots_skipped += 1;
            stats.warnings.push(format!(
                "Skipped filesystem fallback for broad fuzzy pattern '{}' under indexed root '{}'; use a concrete basename/path token, extensions, max_depth, or a narrower path.",
                pattern,
                canonical_root.to_string_lossy()
            ));
            return Ok(());
        }
    }

    let mut walk = WalkBuilder::new(&canonical_root);
    walk.hidden(true)
        .ignore(true)
        .git_ignore(true)
        .git_exclude(true)
        .require_git(false)
        .threads(crate::common::bounded_walk_threads());
    if let Some(depth) = max_depth {
        walk.max_depth(Some(depth as usize));
    }
    let extension_globs = extension_override_globs(target_type, extensions);
    if !extension_globs.is_empty() {
        apply_walk_overrides(&mut walk, &canonical_root, &extension_globs, &[])?;
    }

    stats.filesystem_roots_walked += 1;
    let local_ranked = Arc::new(Mutex::new(Vec::<RankedMatch>::new()));
    let local_seen = Arc::new(Mutex::new(HashSet::<String>::new()));
    let entries_scanned = Arc::new(AtomicUsize::new(0));
    let closure_roots = all_roots.to_vec();
    let closure_pattern = pattern.to_string();
    let closure_target_type = target_type.to_string();
    let closure_extensions = extensions.to_vec();
    let closure_filter_terms = filter_terms.to_vec();
    let closure_max_depth = max_depth;
    let closure_max_results = max_results;

    walk.build_parallel().run(|| {
        let local_ranked = Arc::clone(&local_ranked);
        let local_seen = Arc::clone(&local_seen);
        let entries_scanned = Arc::clone(&entries_scanned);
        let roots = closure_roots.clone();
        let pattern = closure_pattern.clone();
        let target_type = closure_target_type.clone();
        let extensions = closure_extensions.clone();
        let filter_terms = closure_filter_terms.clone();
        let matcher = SkimMatcherV2::default();

        Box::new(move |entry| {
            entries_scanned.fetch_add(1, Ordering::Relaxed);
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => return ignore::WalkState::Continue,
            };

            let Some(file_type) = entry.file_type() else {
                return ignore::WalkState::Continue;
            };

            if target_type == "file" && !file_type.is_file() {
                return ignore::WalkState::Continue;
            }
            if target_type == "dir" && !file_type.is_dir() {
                return ignore::WalkState::Continue;
            }

            let Some(candidate) = score_candidate(
                entry.path(),
                file_type.is_dir(),
                &roots,
                &pattern,
                &target_type,
                &extensions,
                &filter_terms,
                closure_max_depth,
                &matcher,
                None,
            ) else {
                return ignore::WalkState::Continue;
            };

            {
                let mut seen = match local_seen.lock() {
                    Ok(guard) => guard,
                    Err(_) => return ignore::WalkState::Quit,
                };
                if !seen.insert(candidate.path.clone()) {
                    return ignore::WalkState::Continue;
                }
            }

            let mut ranked = match local_ranked.lock() {
                Ok(guard) => guard,
                Err(_) => return ignore::WalkState::Quit,
            };
            push_ranked_match(&mut ranked, candidate, closure_max_results);
            ignore::WalkState::Continue
        })
    });

    stats.entries_scanned = stats
        .entries_scanned
        .saturating_add(entries_scanned.load(Ordering::Relaxed));

    let local_ranked = match Arc::try_unwrap(local_ranked) {
        Ok(mutex) => mutex
            .into_inner()
            .map_err(|_| anyhow::anyhow!("fuzzy_find ranked collector is unavailable"))?,
        Err(shared) => shared
            .lock()
            .map_err(|_| anyhow::anyhow!("fuzzy_find ranked collector is unavailable"))?
            .clone(),
    };
    for candidate in local_ranked {
        if !seen_paths.insert(candidate.path.clone()) {
            continue;
        }
        push_ranked_match(ranked, candidate, max_results);
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn consider_candidate(
    path: &Path,
    is_dir: bool,
    roots: &[PathBuf],
    pattern: &str,
    target_type: &str,
    extensions: &[String],
    filter_terms: &[String],
    max_depth: Option<u64>,
    max_results: usize,
    matcher: &SkimMatcherV2,
    ranked: &mut Vec<RankedMatch>,
    seen_paths: &mut HashSet<String>,
    cached_meta: Option<(u64, u64)>,
) {
    let Some(candidate) = score_candidate(
        path,
        is_dir,
        roots,
        pattern,
        target_type,
        extensions,
        filter_terms,
        max_depth,
        matcher,
        cached_meta,
    ) else {
        return;
    };
    if !seen_paths.insert(candidate.path.clone()) {
        return;
    }

    push_ranked_match(ranked, candidate, max_results);
}

#[allow(clippy::too_many_arguments)]
fn consider_indexed_candidate(
    candidate: &crate::indexer::PathQueryCandidate,
    roots: &[PathBuf],
    pattern: &str,
    target_type: &str,
    extensions: &[String],
    filter_terms: &[String],
    max_depth: Option<u64>,
    max_results: usize,
    matcher: &SkimMatcherV2,
    ranked: &mut Vec<RankedMatch>,
    seen_paths: &mut HashSet<String>,
) {
    let Some(candidate) = score_candidate_from_parts(
        &candidate.path,
        candidate.is_dir,
        compute_relative_path(&candidate.path, roots),
        pattern,
        target_type,
        extensions,
        filter_terms,
        max_depth,
        matcher,
        Some((candidate.size, candidate.modified_at)),
    ) else {
        return;
    };
    if !seen_paths.insert(candidate.path.clone()) {
        return;
    }

    push_ranked_match(ranked, candidate, max_results);
}

fn push_ranked_match(ranked: &mut Vec<RankedMatch>, candidate: RankedMatch, max_results: usize) {
    if max_results == 0 {
        return;
    }

    if ranked.len() >= max_results
        && ranked
            .last()
            .is_some_and(|worst| compare_ranked_match(&candidate, worst).is_lt())
    {
        return;
    }

    ranked.push(candidate);
    ranked.sort_by(compare_ranked_match);
    if ranked.len() > max_results {
        ranked.truncate(max_results);
    }
}

#[allow(clippy::too_many_arguments)]
fn score_candidate(
    path: &Path,
    is_dir: bool,
    roots: &[PathBuf],
    pattern: &str,
    target_type: &str,
    extensions: &[String],
    filter_terms: &[String],
    max_depth: Option<u64>,
    matcher: &SkimMatcherV2,
    cached_meta: Option<(u64, u64)>,
) -> Option<RankedMatch> {
    let relative_path = compute_relative_path(path, roots);
    score_candidate_from_parts(
        path,
        is_dir,
        relative_path,
        pattern,
        target_type,
        extensions,
        filter_terms,
        max_depth,
        matcher,
        cached_meta,
    )
}

#[allow(clippy::too_many_arguments)]
fn score_candidate_from_parts(
    path: &Path,
    is_dir: bool,
    relative_path: String,
    pattern: &str,
    target_type: &str,
    extensions: &[String],
    filter_terms: &[String],
    max_depth: Option<u64>,
    matcher: &SkimMatcherV2,
    cached_meta: Option<(u64, u64)>,
) -> Option<RankedMatch> {
    if target_type == "file" && is_dir {
        return None;
    }
    if target_type == "dir" && !is_dir {
        return None;
    }

    if !extensions.is_empty() && !is_dir {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();
        if !extensions.contains(&ext) {
            return None;
        }
    }

    if max_depth.is_some() && !within_max_depth(&relative_path, max_depth) {
        return None;
    }

    if !filter_terms.is_empty() {
        let filter_target = relative_path.to_ascii_lowercase();
        if !filter_terms.iter().all(|term| filter_target.contains(term)) {
            return None;
        }
    }

    let normalized_path = normalize_path(path);

    let score_target = if relative_path.is_empty() {
        normalized_path.as_str()
    } else {
        relative_path.as_str()
    };
    let score = matcher.fuzzy_match(score_target, pattern).or_else(|| {
        (!filter_terms.is_empty()).then(|| term_containment_score(score_target, filter_terms))
    })?;
    if score <= 0 {
        return None;
    }

    let (size, modified_at) = cached_meta.unwrap_or_else(|| read_entry_metadata(path));

    Some(RankedMatch {
        path: normalized_path,
        relative_path,
        score,
        entry_type: if is_dir { "dir" } else { "file" },
        size,
        modified_at,
    })
}

fn read_entry_metadata(path: &Path) -> (u64, u64) {
    std::fs::metadata(path)
        .map(|meta| {
            let modified_at = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs())
                .unwrap_or(0);
            (meta.len(), modified_at)
        })
        .unwrap_or((0, 0))
}

fn within_max_depth(relative_path: &str, max_depth: Option<u64>) -> bool {
    let Some(max_depth) = max_depth else {
        return true;
    };
    if relative_path.is_empty() {
        return true;
    }

    relative_path.split('/').count() as u64 <= max_depth
}

fn term_containment_score(score_target: &str, filter_terms: &[String]) -> i64 {
    let score_target_lower = score_target.to_ascii_lowercase();
    let position_penalty = filter_terms
        .iter()
        .filter_map(|term| score_target_lower.find(term))
        .sum::<usize>() as i64;
    let length_penalty = score_target_lower.len().min(2_000) as i64;
    2_000 - length_penalty - position_penalty
}

fn compute_relative_path(path: &Path, roots: &[PathBuf]) -> String {
    for root in roots {
        if root.is_dir() {
            if let Ok(stripped) = path.strip_prefix(root) {
                return normalize_path(stripped);
            }
        } else if root.is_file() && root == path {
            return path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string();
        }
    }

    normalize_path(path)
}

fn compare_ranked_match(left: &RankedMatch, right: &RankedMatch) -> std::cmp::Ordering {
    right
        .score
        .cmp(&left.score)
        .then_with(|| left.relative_path.cmp(&right.relative_path))
        .then_with(|| left.path.cmp(&right.path))
}

fn indexed_shortlist_limit(max_results: usize) -> usize {
    max_results.saturating_mul(64).clamp(256, 4096)
}

fn compile_glob_pattern(pattern: &str) -> Option<Pattern> {
    if !pattern.chars().any(|ch| matches!(ch, '*' | '?' | '[')) {
        return None;
    }
    let normalized = pattern.replace('\\', "/").to_ascii_lowercase();
    Pattern::new(&normalized).ok()
}

fn glob_anchor_terms(pattern: &str) -> Vec<String> {
    let mut terms = pattern
        .to_ascii_lowercase()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|term| term.len() >= 3)
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    terms.sort();
    terms.dedup();
    terms
}

fn pattern_filter_terms(pattern: &str) -> Vec<String> {
    let normalized = pattern.to_ascii_lowercase();
    let mut terms = normalized
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|term| term.len() >= 2)
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    if terms.len() < 2 {
        return Vec::new();
    }

    terms.sort();
    terms.dedup();
    terms
}

fn should_skip_broad_filesystem_fallback(
    pattern: &str,
    filter_terms: &[String],
    extensions: &[String],
    max_depth: Option<u64>,
) -> bool {
    pattern.split_whitespace().count() >= 2
        && filter_terms.len() >= 2
        && !pattern.contains('/')
        && !pattern.contains('\\')
        && !pattern.contains('.')
        && extensions.is_empty()
        && max_depth.is_none()
}

fn extension_override_globs(target_type: &str, extensions: &[String]) -> Vec<String> {
    if target_type != "file" {
        return Vec::new();
    }

    extensions
        .iter()
        .filter(|extension| !extension.is_empty())
        .map(|extension| format!("*.{}", extension))
        .collect()
}

fn parse_usize_arg(args: &Value, name: &str, default: usize, min: usize, max: usize) -> usize {
    args.get(name)
        .and_then(|value| value.as_u64())
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(default)
        .clamp(min, max)
}

fn normalize_path(path: &Path) -> String {
    let raw = path.to_string_lossy();

    if let Some(stripped) = raw.strip_prefix("\\\\?\\UNC\\") {
        format!("//{}", stripped.replace('\\', "/"))
    } else if let Some(stripped) = raw.strip_prefix("\\\\?\\") {
        stripped.replace('\\', "/")
    } else {
        raw.replace('\\', "/")
    }
}
