use anyhow::{Context, Result};
use glob::Pattern;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::sinks::UTF8;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder};
use ignore::{WalkBuilder, WalkState};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::task;

use super::path_filters::{apply_walk_overrides, compile_patterns, parse_pattern_strings};
use crate::common::insert_object_field;
use crate::indexer::query_tantivy_content_candidates;

const DEFAULT_MAX_RESULTS: usize = 100;
const MAX_RETURNED_MATCHES: usize = 1_000;
const DEFAULT_MAX_LINE_LENGTH: usize = 240;
const MAX_LINE_LENGTH: usize = 4_000;
const MAX_SEARCH_FILE_BYTES: u64 = 5 * 1024 * 1024;
const DEFAULT_FALLBACK_EXCLUDES: &[&str] = &[
    "out/**",
    "**/out/**",
    "generated/**",
    "**/generated/**",
    ".git/**",
    "**/.git/**",
    "node_modules/**",
    "**/node_modules/**",
    "target/**",
    "**/target/**",
    "third_party/**",
    "**/third_party/**",
];

static TOTAL_TEXT_SEARCHES: AtomicU64 = AtomicU64::new(0);
static TOTAL_GREP_FALLBACKS: AtomicU64 = AtomicU64::new(0);
static TOTAL_REFUSED_LARGE_SCOPE: AtomicU64 = AtomicU64::new(0);
static LAST_SEARCH_DURATION_MS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Serialize, Debug)]
pub struct SearchMatch {
    pub file: String,
    pub line: u64,
    pub snippet: String,
    pub line_text: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub line_truncated: bool,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum SearchMode {
    Literal,
    Regex,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum CaseMode {
    Insensitive,
    Sensitive,
    Smart,
}

#[derive(Debug)]
struct CandidateFile {
    path: PathBuf,
    relative_path: String,
}

#[derive(Default, Debug)]
struct SearchStats {
    paths_received: usize,
    valid_paths: usize,
    invalid_paths: Vec<String>,
    files_considered: usize,
    files_searched: usize,
    files_skipped_large: usize,
}

#[derive(Default)]
struct SharedSearchState {
    matches: Mutex<Vec<SearchMatch>>,
    seen: Mutex<HashSet<String>>,
    files_considered: AtomicUsize,
    files_searched: AtomicUsize,
    search_errors: AtomicUsize,
    files_skipped_large: AtomicUsize,
    stop: AtomicBool,
}

#[derive(Debug)]
struct FallbackPlan {
    allow_grep: bool,
    reason: Option<&'static str>,
}

pub fn schema() -> Value {
    json!({
        "name": "text_search",
        "description": "Search file contents with exact literal/regex verification. For large repos, use narrow paths first (for example src/module), prefer literal queries to use Tantivy candidate shortlisting, and check search_strategy/fallback_reason/warming_zones in the response. Root-wide searches may be planned/refused unless allow_expensive_fallback=true.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query. Interpreted as literal text unless mode is regex." },
                "paths": { "type": "array", "items": { "type": "string" }, "description": "Files or directories to search. Defaults to the active workspace root. In large repositories, provide the narrowest directory or file scope; avoid ['.'] unless allow_expensive_fallback is intentional." },
                "mode": { "type": "string", "enum": ["literal", "regex"], "description": "Search mode. Defaults to literal. Literal queries can use Tantivy to shortlist files before exact grep verification; regex always needs grep verification and should be scoped narrowly." },
                "case_mode": { "type": "string", "enum": ["insensitive", "sensitive", "smart"], "description": "Case handling. smart is case-insensitive unless the query contains uppercase. If case_sensitive is provided, it overrides case_mode for backward compatibility." },
                "case_sensitive": { "type": "boolean", "description": "Legacy override for case matching. When set, true forces sensitive and false forces insensitive, taking precedence over case_mode." },
                "max_results": { "type": "integer", "description": "Maximum matches to return. Defaults to 100; 0 returns no matches." },
                "includes": { "type": "array", "items": { "type": "string" }, "description": "Glob include filters relative to searched roots, e.g. **/*.rs." },
                "excludes": { "type": "array", "items": { "type": "string" }, "description": "Glob exclude filters relative to searched roots. Expensive grep fallback also applies default excludes for build/generated/vendor directories unless the user directly scopes into them." },
                "context_lines": { "type": "integer", "description": "Number of before/after context lines per match. Values are capped at 10." },
                "max_line_length": { "type": "integer", "description": "Maximum displayed characters per matched line before truncation." },
                "explain_no_results": { "type": "boolean", "description": "When true, include diagnostics explaining why no matches were found, including fallback/index context." },
                "allow_expensive_fallback": { "type": "boolean", "description": "Set true to permit root-wide grep fallback in very large indexed workspaces. Default false protects agents from Chromium-scale timeouts; prefer scoping paths first." }
            },
            "required": ["query"]
        }
    })
}

pub async fn execute(args: &Value) -> Result<Value> {
    let args_owned = args.clone();
    task::spawn_blocking(move || execute_blocking(args_owned))
        .await
        .context("text_search background task failed to join")?
}

pub fn search_telemetry() -> Value {
    json!({
        "total_text_searches": TOTAL_TEXT_SEARCHES.load(Ordering::Relaxed),
        "total_grep_fallbacks": TOTAL_GREP_FALLBACKS.load(Ordering::Relaxed),
        "total_refused_large_scope": TOTAL_REFUSED_LARGE_SCOPE.load(Ordering::Relaxed),
        "last_search_duration_ms": LAST_SEARCH_DURATION_MS.load(Ordering::Relaxed)
    })
}

fn execute_blocking(args: Value) -> Result<Value> {
    let started_at = Instant::now();
    TOTAL_TEXT_SEARCHES.fetch_add(1, Ordering::Relaxed);
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim_end();
    if query.is_empty() {
        return Err(anyhow::anyhow!("Query cannot be empty"));
    }

    let input_paths: Vec<PathBuf> = args
        .get("paths")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| p.as_str())
                .map(crate::common::resolve_tool_path)
                .collect()
        })
        .unwrap_or_else(|| vec![crate::common::default_tool_root()]);

    let mode = parse_mode(args.get("mode").and_then(|v| v.as_str()))?;
    let case_mode = parse_case_mode(
        args.get("case_mode").and_then(|v| v.as_str()),
        args.get("case_sensitive").and_then(|v| v.as_bool()),
    )?;
    let case_sensitive_effective = resolve_case_sensitive(case_mode, query);
    let max_results = parse_usize_arg(
        &args,
        "max_results",
        DEFAULT_MAX_RESULTS,
        0,
        MAX_RETURNED_MATCHES,
    );
    let context_lines = args
        .get("context_lines")
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
        .min(10) as usize;
    let max_line_length = parse_usize_arg(
        &args,
        "max_line_length",
        DEFAULT_MAX_LINE_LENGTH,
        40,
        MAX_LINE_LENGTH,
    );
    let explain_no_results = args
        .get("explain_no_results")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let allow_expensive_fallback = args
        .get("allow_expensive_fallback")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let include_globs = parse_pattern_strings(args.get("includes"));
    let user_exclude_globs = parse_pattern_strings(args.get("excludes"));
    let default_exclude_globs = default_fallback_excludes(&input_paths, &user_exclude_globs);
    let mut exclude_globs = user_exclude_globs.clone();
    exclude_globs.extend(default_exclude_globs.iter().cloned());
    let includes = Arc::new(compile_patterns(&include_globs)?);
    let excludes = Arc::new(compile_patterns(&exclude_globs)?);
    let pattern = Arc::new(match mode {
        SearchMode::Literal => regex::escape(query),
        SearchMode::Regex => query.to_string(),
    });
    build_matcher(&pattern, case_sensitive_effective).context("Invalid search query")?;

    let includes_applied = !includes.is_empty();
    let excludes_applied = !excludes.is_empty();
    let default_excludes_applied = !default_exclude_globs.is_empty();
    let shared = Arc::new(SharedSearchState::default());
    let mut stats = SearchStats {
        paths_received: input_paths.len(),
        ..Default::default()
    };
    let mut search_strategy = "grep_fallback";
    let mut content_index_used = false;
    let mut content_index_partial = false;
    let mut content_index_zones = Vec::<String>::new();
    let mut warming_zones = Vec::<String>::new();
    let mut fallback_reasons = Vec::<String>::new();
    let mut candidate_count = 0usize;
    let mut candidate_limit = 0usize;
    let mut candidates_truncated = false;
    let mut run_grep_fallback = true;

    if mode == SearchMode::Literal && max_results > 0 {
        let requested_candidate_limit = max_results.saturating_mul(64).max(256);
        let index_result =
            query_tantivy_content_candidates(&input_paths, query, requested_candidate_limit);
        content_index_used = index_result.content_index_used;
        content_index_partial = index_result.content_index_partial;
        content_index_zones = index_result.zones.clone();
        warming_zones = index_result.warming_zones.clone();
        fallback_reasons.extend(index_result.fallback_reasons.clone());
        candidate_count = index_result.candidate_count;
        candidate_limit = index_result.candidate_limit;
        candidates_truncated = index_result.candidates_truncated;

        if content_index_used {
            let matcher = build_matcher(&pattern, case_sensitive_effective)
                .context("Invalid search query")?;
            let searcher = build_searcher(context_lines);
            search_index_candidate_paths(
                index_result.paths,
                &input_paths,
                includes.as_ref(),
                excludes.as_ref(),
                &matcher,
                &searcher,
                max_results,
                max_line_length,
                Arc::clone(&shared),
                &mut stats,
            );

            if !content_index_partial {
                run_grep_fallback = false;
                search_strategy = "tantivy";
            }
        }
    } else if mode == SearchMode::Regex {
        fallback_reasons.push("regex_mode_requires_grep".to_string());
    } else if max_results == 0 {
        fallback_reasons.push("max_results_zero".to_string());
    }

    if run_grep_fallback {
        if content_index_used {
            search_strategy = "mixed";
        }
        let fallback_plan = plan_grep_fallback(&input_paths, allow_expensive_fallback);
        if !fallback_plan.allow_grep {
            search_strategy = "refused_large_scope";
            TOTAL_REFUSED_LARGE_SCOPE.fetch_add(1, Ordering::Relaxed);
            if let Some(reason) = fallback_plan.reason {
                fallback_reasons.push(reason.to_string());
            }
            record_input_path_validity(&input_paths, &mut stats);
        } else {
            TOTAL_GREP_FALLBACKS.fetch_add(1, Ordering::Relaxed);
            for input_path in &input_paths {
                if max_results > 0
                    && shared.matches.lock().map(|m| m.len()).unwrap_or(0) >= max_results
                {
                    shared.stop.store(true, Ordering::Relaxed);
                    break;
                }

                process_input_path(
                    input_path,
                    &include_globs,
                    &exclude_globs,
                    Arc::clone(&includes),
                    Arc::clone(&excludes),
                    Arc::clone(&pattern),
                    case_sensitive_effective,
                    context_lines,
                    max_results,
                    max_line_length,
                    Arc::clone(&shared),
                    &mut stats,
                )?;
            }
        }
    } else {
        record_input_path_validity(&input_paths, &mut stats);
    }
    fallback_reasons.sort();
    fallback_reasons.dedup();

    let search_errors = shared.search_errors.load(Ordering::Relaxed);
    let duration_ms = started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    LAST_SEARCH_DURATION_MS.store(duration_ms, Ordering::Relaxed);
    let mut matches = match Arc::try_unwrap(shared) {
        Ok(state) => state
            .matches
            .into_inner()
            .map_err(|_| anyhow::anyhow!("text_search result collector is unavailable"))?,
        Err(state) => state
            .matches
            .lock()
            .map_err(|_| anyhow::anyhow!("text_search result collector is unavailable"))?
            .clone(),
    };
    matches.sort_by(|left, right| {
        left.file
            .cmp(&right.file)
            .then_with(|| left.line.cmp(&right.line))
    });
    matches.truncate(max_results);

    let total_returned = matches.len();
    let limit_reached = max_results > 0 && total_returned >= max_results;
    let no_results = if explain_no_results && total_returned == 0 {
        Some(json!({
            "reason": no_results_reason(&stats),
            "paths_received": stats.paths_received,
            "valid_paths": stats.valid_paths,
            "invalid_paths": stats.invalid_paths,
            "files_considered": stats.files_considered,
            "files_searched": stats.files_searched,
            "files_skipped_large": stats.files_skipped_large,
            "includes_applied": includes_applied,
            "excludes_applied": excludes_applied,
            "default_excludes_applied": default_excludes_applied,
            "fallback_reason": fallback_reasons
        }))
    } else {
        None
    };

    let mut response = json!({
        "matches": matches,
        "total_returned": total_returned,
        "limit_reached": limit_reached,
        "limit_reason": if limit_reached { Some("max_results") } else { None },
        "files_considered": stats.files_considered,
        "files_searched": stats.files_searched,
        "files_skipped_large": stats.files_skipped_large,
        "search_errors": search_errors,
        "duration_ms": duration_ms,
        "mode": mode,
        "case_mode": case_mode,
        "max_line_length": max_line_length,
        "engine": engine_for_strategy(search_strategy),
        "search_strategy": search_strategy,
        "candidate_engine": if candidate_limit > 0 { Some("tantivy") } else { None },
        "verification_engine": "grep_searcher",
        "content_index_used": content_index_used,
        "content_index_partial": content_index_partial,
        "content_index_zones": content_index_zones,
        "warming_zones": warming_zones,
        "fallback_reason": fallback_reasons,
        "candidate_count": candidate_count,
        "candidate_limit": candidate_limit,
        "candidates_truncated": candidates_truncated,
        "default_excludes_applied": default_excludes_applied,
        "default_excludes": default_exclude_globs,
        "allow_expensive_fallback": allow_expensive_fallback,
        "suggested_next_query": suggested_next_query(&input_paths, search_strategy)
    });

    if let Some(no_results) = no_results {
        insert_object_field(&mut response, "no_results", no_results);
    }
    let warnings = search_scope_warnings(&input_paths, search_strategy);
    if !warnings.is_empty() {
        insert_object_field(&mut response, "warnings", json!(warnings));
    }

    Ok(response)
}

#[allow(clippy::too_many_arguments)]
fn search_index_candidate_paths(
    paths: Vec<PathBuf>,
    input_paths: &[PathBuf],
    includes: &[Pattern],
    excludes: &[Pattern],
    matcher: &RegexMatcher,
    searcher: &Searcher,
    max_results: usize,
    max_line_length: usize,
    shared: Arc<SharedSearchState>,
    stats: &mut SearchStats,
) {
    if paths.is_empty() {
        return;
    }

    let roots = input_paths
        .iter()
        .map(|path| canonicalize_existing_path(path))
        .collect::<Vec<_>>();

    for path in paths {
        if shared.stop.load(Ordering::Relaxed) {
            break;
        }
        if !path.is_file() {
            continue;
        }

        let canonical_path = canonicalize_existing_path(&path);
        let relative_path = relative_path_for_roots(&canonical_path, &roots);
        search_candidate(
            CandidateFile {
                path: canonical_path,
                relative_path,
            },
            includes,
            excludes,
            matcher,
            searcher,
            max_results,
            max_line_length,
            &shared,
        );
    }

    merge_shared_stats(&shared, stats);
}

#[allow(clippy::too_many_arguments)]
fn process_input_path(
    input_path: &Path,
    include_globs: &[String],
    exclude_globs: &[String],
    includes: Arc<Vec<Pattern>>,
    excludes: Arc<Vec<Pattern>>,
    pattern: Arc<String>,
    case_sensitive: bool,
    context_lines: usize,
    max_results: usize,
    max_line_length: usize,
    shared: Arc<SharedSearchState>,
    stats: &mut SearchStats,
) -> Result<()> {
    if !input_path.exists() {
        stats
            .invalid_paths
            .push(input_path.to_string_lossy().to_string());
        return Ok(());
    }

    let canonical_path = canonicalize_existing_path(input_path);

    if canonical_path.is_file() {
        stats.valid_paths += 1;
        let relative_path = canonical_path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.to_string())
            .unwrap_or_else(|| canonical_path.to_string_lossy().to_string());
        let matcher = build_matcher(&pattern, case_sensitive)?;
        let searcher = build_searcher(context_lines);
        search_candidate(
            CandidateFile {
                path: canonical_path,
                relative_path,
            },
            includes.as_ref(),
            excludes.as_ref(),
            &matcher,
            &searcher,
            max_results,
            max_line_length,
            &shared,
        );
        merge_shared_stats(&shared, stats);
        return Ok(());
    }

    if !canonical_path.is_dir() {
        stats
            .invalid_paths
            .push(canonical_path.to_string_lossy().to_string());
        return Ok(());
    }

    stats.valid_paths += 1;

    let mut walk = WalkBuilder::new(&canonical_path);
    walk.hidden(true)
        .ignore(true)
        .git_ignore(true)
        .git_exclude(true)
        .require_git(false)
        .threads(crate::common::bounded_walk_threads());
    apply_walk_overrides(&mut walk, &canonical_path, include_globs, exclude_globs)?;
    let filter_root = canonical_path.clone();
    let filter_excludes = Arc::clone(&excludes);
    walk.filter_entry(move |entry| {
        if entry.path() == filter_root {
            return true;
        }
        if !entry
            .file_type()
            .is_some_and(|file_type| file_type.is_dir())
        {
            return true;
        }
        let relative_path = entry
            .path()
            .strip_prefix(&filter_root)
            .ok()
            .map(normalize_path)
            .filter(|relative| !relative.is_empty())
            .unwrap_or_else(|| normalize_path(entry.path()));
        let candidate = CandidateFile {
            path: entry.path().to_path_buf(),
            relative_path,
        };
        !matches_excludes(&candidate, filter_excludes.as_ref())
    });

    walk.build_parallel().run(|| {
        let includes = Arc::clone(&includes);
        let excludes = Arc::clone(&excludes);
        let pattern = Arc::clone(&pattern);
        let shared = Arc::clone(&shared);
        let root = canonical_path.clone();
        let matcher = build_matcher(&pattern, case_sensitive).ok();
        let searcher = build_searcher(context_lines);

        Box::new(move |entry| {
            if shared.stop.load(Ordering::Relaxed) {
                return WalkState::Quit;
            }

            let Some(matcher) = matcher.as_ref() else {
                shared.search_errors.fetch_add(1, Ordering::Relaxed);
                shared.stop.store(true, Ordering::Relaxed);
                return WalkState::Quit;
            };

            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    shared.search_errors.fetch_add(1, Ordering::Relaxed);
                    return WalkState::Continue;
                }
            };

            if !entry
                .file_type()
                .is_some_and(|file_type| file_type.is_file())
            {
                return WalkState::Continue;
            }

            let path = entry.path().to_path_buf();
            let relative_path = path
                .strip_prefix(&root)
                .ok()
                .map(normalize_path)
                .filter(|relative| !relative.is_empty())
                .unwrap_or_else(|| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .map(|name| name.to_string())
                        .unwrap_or_else(|| path.to_string_lossy().to_string())
                });

            search_candidate(
                CandidateFile {
                    path,
                    relative_path,
                },
                includes.as_ref(),
                excludes.as_ref(),
                matcher,
                &searcher,
                max_results,
                max_line_length,
                &shared,
            );

            if shared.stop.load(Ordering::Relaxed) {
                WalkState::Quit
            } else {
                WalkState::Continue
            }
        })
    });

    merge_shared_stats(&shared, stats);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn search_candidate(
    candidate: CandidateFile,
    includes: &[Pattern],
    excludes: &[Pattern],
    matcher: &RegexMatcher,
    searcher: &Searcher,
    max_results: usize,
    max_line_length: usize,
    shared: &SharedSearchState,
) {
    if max_results == 0 || shared.stop.load(Ordering::Relaxed) {
        return;
    }

    let path_key = normalize_path(&candidate.path);
    {
        let mut seen = match shared.seen.lock() {
            Ok(guard) => guard,
            Err(_) => {
                shared.search_errors.fetch_add(1, Ordering::Relaxed);
                return;
            }
        };
        if !seen.insert(path_key) {
            return;
        }
    }

    shared.files_considered.fetch_add(1, Ordering::Relaxed);

    if !passes_patterns(&candidate, includes, excludes) {
        return;
    }

    let meta = match candidate.path.metadata() {
        Ok(meta) => meta,
        Err(_) => {
            shared.search_errors.fetch_add(1, Ordering::Relaxed);
            return;
        }
    };

    if meta.len() > MAX_SEARCH_FILE_BYTES {
        shared.files_skipped_large.fetch_add(1, Ordering::Relaxed);
        return;
    }

    shared.files_searched.fetch_add(1, Ordering::Relaxed);
    let mut local_matches = Vec::new();
    let candidate_path = candidate.path.clone();

    let search_result = searcher.clone().search_path(
        matcher,
        &candidate_path,
        UTF8(|line_num, line| {
            let raw_line = line.trim_end_matches(['\r', '\n']);
            let (line_text, line_truncated) = truncate_text(raw_line, max_line_length);
            local_matches.push(SearchMatch {
                file: candidate_path.to_string_lossy().to_string(),
                line: line_num,
                snippet: line_text.trim().to_string(),
                line_text,
                line_truncated,
            });

            Ok(local_matches.len() < max_results)
        }),
    );

    if search_result.is_err() {
        shared.search_errors.fetch_add(1, Ordering::Relaxed);
        return;
    }

    if local_matches.is_empty() {
        return;
    }

    let mut matches = match shared.matches.lock() {
        Ok(guard) => guard,
        Err(_) => {
            shared.search_errors.fetch_add(1, Ordering::Relaxed);
            return;
        }
    };

    for search_match in local_matches {
        if matches.len() >= max_results {
            shared.stop.store(true, Ordering::Relaxed);
            break;
        }
        matches.push(search_match);
    }

    if matches.len() >= max_results {
        shared.stop.store(true, Ordering::Relaxed);
    }
}

fn build_matcher(pattern: &str, case_sensitive: bool) -> Result<RegexMatcher> {
    RegexMatcherBuilder::new()
        .case_insensitive(!case_sensitive)
        .build(pattern)
        .map_err(Into::into)
}

fn build_searcher(context_lines: usize) -> Searcher {
    SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .line_number(true)
        .before_context(context_lines)
        .after_context(context_lines)
        .build()
}

fn merge_shared_stats(shared: &SharedSearchState, stats: &mut SearchStats) {
    stats.files_considered = shared.files_considered.load(Ordering::Relaxed);
    stats.files_searched = shared.files_searched.load(Ordering::Relaxed);
    stats.files_skipped_large = shared.files_skipped_large.load(Ordering::Relaxed);
}

fn parse_mode(raw: Option<&str>) -> Result<SearchMode> {
    match raw.unwrap_or("literal") {
        "literal" => Ok(SearchMode::Literal),
        "regex" => Ok(SearchMode::Regex),
        other => Err(anyhow::anyhow!("Unsupported mode '{}'", other)),
    }
}

fn parse_case_mode(raw: Option<&str>, legacy_case_sensitive: Option<bool>) -> Result<CaseMode> {
    if let Some(mode) = raw {
        return match mode {
            "insensitive" => Ok(CaseMode::Insensitive),
            "sensitive" => Ok(CaseMode::Sensitive),
            "smart" => Ok(CaseMode::Smart),
            other => Err(anyhow::anyhow!("Unsupported case_mode '{}'", other)),
        };
    }

    Ok(match legacy_case_sensitive {
        Some(true) => CaseMode::Sensitive,
        _ => CaseMode::Insensitive,
    })
}

fn resolve_case_sensitive(case_mode: CaseMode, query: &str) -> bool {
    match case_mode {
        CaseMode::Insensitive => false,
        CaseMode::Sensitive => true,
        CaseMode::Smart => query.chars().any(|c| c.is_uppercase()),
    }
}

fn parse_usize_arg(args: &Value, name: &str, default: usize, min: usize, max: usize) -> usize {
    args.get(name)
        .and_then(|value| value.as_u64())
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(default)
        .clamp(min, max)
}

fn passes_patterns(candidate: &CandidateFile, includes: &[Pattern], excludes: &[Pattern]) -> bool {
    let candidates = candidate_match_strings(candidate);

    if !includes.is_empty() && !matches_any_pattern(&candidates, includes) {
        return false;
    }

    if !excludes.is_empty() && matches_any_pattern(&candidates, excludes) {
        return false;
    }

    true
}

fn matches_excludes(candidate: &CandidateFile, excludes: &[Pattern]) -> bool {
    !excludes.is_empty() && matches_any_pattern(&candidate_match_strings(candidate), excludes)
}

fn candidate_match_strings(candidate: &CandidateFile) -> Vec<String> {
    let full_path = normalize_path(&candidate.path);
    let file_name = candidate
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string();

    let mut values = vec![candidate.relative_path.clone(), full_path];
    if !file_name.is_empty() {
        values.push(file_name);
    }
    values
}

fn matches_any_pattern(values: &[String], patterns: &[Pattern]) -> bool {
    patterns
        .iter()
        .any(|pattern| values.iter().any(|value| pattern.matches(value)))
}

fn no_results_reason(stats: &SearchStats) -> &'static str {
    if stats.valid_paths == 0 {
        "no_valid_paths"
    } else if stats.files_searched == 0 {
        "no_candidate_files"
    } else {
        "no_match_found"
    }
}

fn record_input_path_validity(input_paths: &[PathBuf], stats: &mut SearchStats) {
    for input_path in input_paths {
        if input_path.exists() {
            stats.valid_paths += 1;
        } else {
            stats
                .invalid_paths
                .push(input_path.to_string_lossy().to_string());
        }
    }
}

fn search_scope_warnings(input_paths: &[PathBuf], search_strategy: &str) -> Vec<String> {
    if search_strategy == "tantivy" {
        return Vec::new();
    }

    let mut warnings = Vec::new();
    for input_path in input_paths {
        let canonical_path = canonicalize_existing_path(input_path);
        if !canonical_path.is_dir() {
            continue;
        }

        let Some(indexed_root) = crate::indexer::indexed_workspace_root_for_path(&canonical_path)
        else {
            continue;
        };
        if indexed_root == canonical_path {
            warnings.push(format!(
                "Search used {} at indexed workspace root '{}'. For large repos, retry with a narrower paths value (for example a component directory), use a literal query when possible, or set allow_expensive_fallback=true only when a full grep scan is intentional.",
                search_strategy,
                canonical_path.to_string_lossy()
            ));
        }
    }

    warnings.sort();
    warnings.dedup();
    warnings
}

fn engine_for_strategy(search_strategy: &str) -> &'static str {
    match search_strategy {
        "tantivy" => "tantivy+grep_verify",
        "mixed" => "tantivy+grep_fallback",
        "refused_large_scope" => "planner",
        _ => "grep_searcher/ignore",
    }
}

fn plan_grep_fallback(input_paths: &[PathBuf], allow_expensive_fallback: bool) -> FallbackPlan {
    if allow_expensive_fallback {
        return FallbackPlan {
            allow_grep: true,
            reason: None,
        };
    }

    for input_path in input_paths {
        let canonical_path = canonicalize_existing_path(input_path);
        if !canonical_path.is_dir() {
            continue;
        }

        let Some(indexed_root) = crate::indexer::indexed_workspace_root_for_path(&canonical_path)
        else {
            continue;
        };
        if indexed_root == canonical_path {
            return FallbackPlan {
                allow_grep: false,
                reason: Some("large_scope_requires_explicit_fallback"),
            };
        }
    }

    FallbackPlan {
        allow_grep: true,
        reason: None,
    }
}

fn default_fallback_excludes(input_paths: &[PathBuf], user_excludes: &[String]) -> Vec<String> {
    if input_paths
        .iter()
        .any(|path| is_direct_vendor_or_generated_scope(path.as_path()))
    {
        return Vec::new();
    }

    DEFAULT_FALLBACK_EXCLUDES
        .iter()
        .filter(|pattern| !user_excludes.iter().any(|existing| existing == **pattern))
        .map(|pattern| (*pattern).to_string())
        .collect()
}

fn is_direct_vendor_or_generated_scope(path: &Path) -> bool {
    let normalized = normalize_path(&canonicalize_existing_path(path));
    normalized.split('/').any(|part| {
        matches!(
            part,
            "third_party" | "out" | "generated" | "node_modules" | "target"
        )
    })
}

fn suggested_next_query(input_paths: &[PathBuf], search_strategy: &str) -> Option<String> {
    if search_strategy != "refused_large_scope" {
        return None;
    }

    input_paths.first().map(|path| {
        format!(
            "Retry with a narrower paths value under '{}' or set allow_expensive_fallback=true for an intentional full grep scan.",
            path.to_string_lossy()
        )
    })
}

fn relative_path_for_roots(path: &Path, roots: &[PathBuf]) -> String {
    for root in roots {
        let target_root = if root.is_file() {
            root.parent().unwrap_or(root)
        } else {
            root.as_path()
        };
        if let Ok(relative) = path.strip_prefix(target_root) {
            let normalized = normalize_path(relative);
            if !normalized.is_empty() {
                return normalized;
            }
        }
    }

    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

fn truncate_text(raw: &str, max_chars: usize) -> (String, bool) {
    if raw.chars().count() <= max_chars {
        return (raw.to_string(), false);
    }

    let mut truncated = raw.chars().take(max_chars).collect::<String>();
    truncated.push_str("...");
    (truncated, true)
}

fn canonicalize_existing_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
