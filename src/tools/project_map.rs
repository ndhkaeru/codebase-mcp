use anyhow::{Context, Result};
use glob::Pattern;
use ignore::WalkBuilder;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tokio::task;

use super::path_filters::{apply_walk_overrides, compile_patterns, parse_pattern_strings};
use crate::indexer::{is_path_index_available, visit_indexed_entries_under};

const DEFAULT_MAX_CHILDREN_PER_DIR: usize = 250;
const MAX_CHILDREN_PER_DIR_LIMIT: usize = 5_000;
const MAX_TOTAL_ENTRIES: usize = 20_000;

pub fn schema() -> Value {
    json!({
        "name": "project_map",
        "title": "Map project tree",
        "description": "Build a bounded tree view of a directory with optional size metadata. Use first for repository orientation and to choose precise text_search scopes; keep depth/children low for large repos.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Directory to map. Prefer a subsystem path over workspace root in large repos." },
                "max_depth": { "type": "integer", "description": "Maximum tree depth; use 1-3 for reconnaissance." },
                "show_sizes": { "type": "boolean", "description": "Include file sizes when useful; omit for compact structure scans." },
                "max_children_per_dir": { "type": "integer", "description": "Per-directory child cap; lower this for very large directories." },
                "includes": { "type": "array", "items": { "type": "string" }, "description": "Optional glob include filters." },
                "excludes": { "type": "array", "items": { "type": "string" }, "description": "Optional glob exclude filters for generated/build/vendor areas." }
            },
            "required": ["path"]
        }
    })
}

pub async fn execute(args: &Value) -> Result<Value> {
    let args_owned = args.clone();
    task::spawn_blocking(move || execute_blocking(args_owned))
        .await
        .context("project_map background task failed to join")?
}

fn execute_blocking(args: Value) -> Result<Value> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .filter(|value| !value.trim().is_empty())
        .context("Missing path")?;
    let path = crate::common::resolve_tool_path(path_str);

    if !path.exists() || !path.is_dir() {
        return Err(anyhow::anyhow!(
            "Path is not a valid directory: {}",
            path_str
        ));
    }

    let max_depth = args.get("max_depth").and_then(|v| v.as_u64()).unwrap_or(3) as usize;
    let show_sizes = args
        .get("show_sizes")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let max_children_per_dir = parse_usize_arg(
        &args,
        "max_children_per_dir",
        DEFAULT_MAX_CHILDREN_PER_DIR,
        1,
        MAX_CHILDREN_PER_DIR_LIMIT,
    );
    let include_globs = parse_pattern_strings(args.get("includes"));
    let exclude_globs = parse_pattern_strings(args.get("excludes"));
    let includes = compile_patterns(&include_globs)?;
    let excludes = compile_patterns(&exclude_globs)?;
    let canonical_path = path.canonicalize().unwrap_or_else(|_| path.clone());

    if is_path_index_available(&canonical_path) {
        return Ok(build_project_map_from_index(
            &path,
            &canonical_path,
            max_depth,
            show_sizes,
            max_children_per_dir,
            &includes,
            &excludes,
        ));
    }

    let mut walker = WalkBuilder::new(&path);
    walker
        .hidden(true)
        .ignore(true)
        .git_ignore(true)
        .git_exclude(true)
        .require_git(false)
        .max_depth(Some(max_depth));
    apply_walk_overrides(&mut walker, &canonical_path, &include_globs, &exclude_globs)?;
    let filter_root = canonical_path.clone();
    let filter_excludes = excludes.clone();
    walker.filter_entry(move |entry| {
        if entry.path() == filter_root {
            return true;
        }
        if !entry
            .file_type()
            .is_some_and(|file_type| file_type.is_dir())
        {
            return true;
        }
        if filter_excludes.is_empty() {
            return true;
        }
        let relative_path = relative_path(entry.path(), &filter_root);
        !matches_patterns(entry.path(), &relative_path, &filter_excludes)
    });

    let mut dir_map: HashMap<String, Vec<Value>> = HashMap::new();
    let mut children_seen: HashMap<String, usize> = HashMap::new();
    let mut truncated_dirs = HashSet::new();
    let mut entries_seen = 0usize;
    let mut entries_returned = 0usize;
    let mut entries_skipped_by_patterns = 0usize;
    let mut total_entries_truncated = false;
    let has_patterns = !includes.is_empty() || !excludes.is_empty();

    for entry in walker.build().flatten() {
        if entry.path() == path {
            continue;
        }

        let is_dir = entry.file_type().is_some_and(|ft| ft.is_dir());
        if has_patterns
            && !passes_patterns(
                entry.path(),
                &relative_path(entry.path(), &canonical_path),
                &includes,
                &excludes,
            )
        {
            entries_skipped_by_patterns += 1;
            continue;
        }

        entries_seen += 1;
        if entries_returned >= MAX_TOTAL_ENTRIES {
            total_entries_truncated = true;
            break;
        }
        let parent_path = entry
            .path()
            .parent()
            .map(normalize_path)
            .unwrap_or_default();

        let child_count = children_seen.entry(parent_path.clone()).or_insert(0);
        *child_count += 1;
        if *child_count > max_children_per_dir {
            truncated_dirs.insert(parent_path);
            continue;
        }

        let size = if show_sizes && !is_dir {
            json!(entry.metadata().map(|m| m.len()).unwrap_or(0))
        } else {
            Value::Null
        };

        let item = json!({
            "name": entry.file_name().to_string_lossy().to_string(),
            "type": if is_dir { "dir" } else { "file" },
            "size_b": size
        });

        dir_map.entry(parent_path).or_default().push(item);
        entries_returned += 1;
    }

    let mut truncated_dirs = truncated_dirs.into_iter().collect::<Vec<_>>();
    let truncated_directory_count = truncated_dirs.len();
    truncated_dirs.sort();
    truncated_dirs.truncate(50);

    Ok(json!({
        "root": normalize_path(&path),
        "canonical_path": normalize_path(&canonical_path),
        "max_depth": max_depth,
        "tree_representation": dir_map,
        "entries_seen": entries_seen,
        "entries_returned": entries_returned,
        "entries_skipped_by_patterns": entries_skipped_by_patterns,
        "max_children_per_dir": max_children_per_dir,
        "limit_reached": truncated_directory_count > 0 || total_entries_truncated,
        "limit_reason": if total_entries_truncated { Some("max_total_entries") } else if truncated_directory_count > 0 { Some("max_children_per_dir") } else { None },
        "max_total_entries": MAX_TOTAL_ENTRIES,
        "truncated_directory_count": truncated_directory_count,
        "truncated_directories": truncated_dirs,
        "search_strategy": "filesystem_walk",
        "metadata_index_used": false,
        "note": "Skipped hidden / git / node_modules inherently to preserve token context."
    }))
}

#[allow(clippy::too_many_arguments)]
fn build_project_map_from_index(
    path: &Path,
    canonical_path: &Path,
    max_depth: usize,
    show_sizes: bool,
    max_children_per_dir: usize,
    includes: &[Pattern],
    excludes: &[Pattern],
) -> Value {
    let mut indexed_entry_count = 0usize;
    let mut warnings = Vec::new();

    let mut dir_map: HashMap<String, Vec<Value>> = HashMap::new();
    let mut children_seen: HashMap<String, usize> = HashMap::new();
    let mut truncated_dirs = HashSet::new();
    let mut entries_seen = 0usize;
    let mut entries_returned = 0usize;
    let mut entries_skipped_by_patterns = 0usize;
    let mut total_entries_truncated = false;

    visit_indexed_entries_under(canonical_path, |entry| {
        indexed_entry_count += 1;
        if entry.path == canonical_path {
            return true;
        }

        let rel = relative_path(&entry.path, canonical_path);
        if rel.is_empty() || relative_depth(&rel) > max_depth {
            return true;
        }

        if !passes_patterns(&entry.path, &rel, includes, excludes) {
            entries_skipped_by_patterns += 1;
            return true;
        }

        entries_seen += 1;
        if entries_returned >= MAX_TOTAL_ENTRIES {
            total_entries_truncated = true;
            return false;
        }
        let parent_path = entry.path.parent().map(normalize_path).unwrap_or_default();

        let child_count = children_seen.entry(parent_path.clone()).or_insert(0);
        *child_count += 1;
        if *child_count > max_children_per_dir {
            truncated_dirs.insert(parent_path);
            return true;
        }

        let size = if show_sizes && !entry.is_dir {
            json!(entry.size)
        } else {
            Value::Null
        };

        let item = json!({
            "name": entry.file_name,
            "type": if entry.is_dir { "dir" } else { "file" },
            "size_b": size
        });

        dir_map.entry(parent_path).or_default().push(item);
        entries_returned += 1;
        true
    });

    if indexed_entry_count >= 200_000 && (max_depth > 2 || max_children_per_dir > 100) {
        warnings.push(
            "Large indexed tree detected; prefer a module path plus max_depth <= 2 and max_children_per_dir <= 100 for Chromium-sized workspaces."
                .to_string(),
        );
    }

    let mut truncated_dirs = truncated_dirs.into_iter().collect::<Vec<_>>();
    let truncated_directory_count = truncated_dirs.len();
    truncated_dirs.sort();
    truncated_dirs.truncate(50);

    json!({
        "root": normalize_path(path),
        "canonical_path": normalize_path(canonical_path),
        "max_depth": max_depth,
        "tree_representation": dir_map,
        "entries_seen": entries_seen,
        "entries_returned": entries_returned,
        "entries_skipped_by_patterns": entries_skipped_by_patterns,
        "max_children_per_dir": max_children_per_dir,
        "limit_reached": truncated_directory_count > 0 || total_entries_truncated,
        "limit_reason": if total_entries_truncated { Some("max_total_entries") } else if truncated_directory_count > 0 { Some("max_children_per_dir") } else { None },
        "max_total_entries": MAX_TOTAL_ENTRIES,
        "truncated_directory_count": truncated_directory_count,
        "truncated_directories": truncated_dirs,
        "search_strategy": "lmdb_metadata",
        "metadata_index_used": true,
        "indexed_entries_available": indexed_entry_count,
        "warnings": warnings,
        "note": "Read from LMDB metadata index; hidden/git ignore rules were applied during index refresh."
    })
}

fn parse_usize_arg(args: &Value, name: &str, default: usize, min: usize, max: usize) -> usize {
    args.get(name)
        .and_then(|value| value.as_u64())
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(default)
        .clamp(min, max)
}

fn passes_patterns(
    path: &Path,
    relative_path: &str,
    includes: &[Pattern],
    excludes: &[Pattern],
) -> bool {
    if !includes.is_empty() && !matches_patterns(path, relative_path, includes) {
        return false;
    }

    if matches_patterns(path, relative_path, excludes) {
        return false;
    }

    true
}

fn matches_patterns(path: &Path, relative_path: &str, patterns: &[Pattern]) -> bool {
    if patterns.is_empty() {
        return false;
    }

    let full_path = normalize_path(path);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let values = [relative_path, full_path.as_str(), file_name];
    patterns
        .iter()
        .any(|pattern| values.iter().any(|value| pattern.matches(value)))
}

fn relative_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .ok()
        .map(normalize_path)
        .filter(|relative| !relative.is_empty())
        .unwrap_or_else(|| normalize_path(path))
}

fn relative_depth(relative_path: &str) -> usize {
    relative_path
        .split('/')
        .filter(|part| !part.is_empty())
        .count()
}

fn normalize_path(path: &Path) -> String {
    crate::common::normalize_display_path(path)
}
