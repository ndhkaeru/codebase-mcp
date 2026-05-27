use anyhow::{Context, Result};
use glob::Pattern;
use ignore::{WalkBuilder, WalkState};
use serde_json::{Value, json};
use std::cmp::Reverse;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::task;

use super::path_filters::{apply_walk_overrides, compile_patterns, parse_pattern_strings};
use crate::indexer::indexed_entries_under;

const DEFAULT_MAX_LINE_COUNT_BYTES: u64 = 2 * 1024 * 1024;
const MAX_LINE_COUNT_BYTES_LIMIT: u64 = 20 * 1024 * 1024;

pub fn schema() -> Value {
    json!({
        "name": "workspace_stats",
        "description": "Summarize file, line, and language counts for a workspace path. Use this to estimate repository size and decide whether text_search needs a narrow path/includes scope before content search.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Directory to summarize. In large repos, run on candidate subsystems rather than only workspace root." },
                "max_line_count_bytes": { "type": "integer", "description": "Maximum bytes per file to count lines from; lower values keep scans fast." },
                "includes": { "type": "array", "items": { "type": "string" }, "description": "Optional glob include filters, e.g. **/*.rs." },
                "excludes": { "type": "array", "items": { "type": "string" }, "description": "Optional glob exclude filters for build/generated/vendor areas." }
            },
            "required": ["path"]
        }
    })
}

fn get_lang_from_ext(ext: &str) -> &'static str {
    match ext {
        "rs" => "Rust",
        "js" | "jsx" | "mjs" | "cjs" => "JavaScript",
        "ts" | "tsx" => "TypeScript",
        "py" | "pyi" => "Python",
        "go" => "Go",
        "java" => "Java",
        "kt" | "kts" => "Kotlin",
        "c" | "h" => "C",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" | "inl" | "inc" => "C++",
        "cs" => "C#",
        "swift" => "Swift",
        "nix" => "Nix",
        "dart" => "Dart",
        "rb" => "Ruby",
        "php" => "PHP",
        "html" | "htm" => "HTML",
        "css" | "scss" | "sass" | "less" => "CSS",
        "md" | "rst" => "Markdown",
        "json" => "JSON",
        "yaml" | "yml" => "YAML",
        "toml" => "TOML",
        "xml" => "XML",
        "sh" | "bash" | "zsh" => "Shell",
        "ps1" | "bat" | "cmd" => "PowerShell/Batch",
        "sql" => "SQL",
        "proto" => "Protobuf",
        "gn" | "gni" => "GN",
        "gyp" | "gypi" => "GYP",
        "cmake" | "mk" => "CMake/Make",
        "lua" => "Lua",
        "vue" | "svelte" => "Vue/Svelte",
        "m" | "mm" => "Objective-C",
        _ => "Other",
    }
}

fn count_lines_in_file(path: &Path) -> u64 {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return 0,
    };

    let mut buffer = [0u8; 64 * 1024];
    let mut newline_count = 0u64;
    let mut has_bytes = false;
    let mut ends_with_newline = false;

    loop {
        let read = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(_) => return 0,
        };

        has_bytes = true;
        newline_count += bytecount::count(&buffer[..read], b'\n') as u64;
        ends_with_newline = buffer[read - 1] == b'\n';
    }

    if !has_bytes {
        0
    } else if ends_with_newline {
        newline_count
    } else {
        newline_count + 1
    }
}

pub async fn execute(args: &Value) -> Result<Value> {
    let args_owned = args.clone();
    task::spawn_blocking(move || execute_blocking(args_owned))
        .await
        .context("workspace_stats background task failed to join")?
}

fn execute_blocking(args: Value) -> Result<Value> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing path")?;
    let path = crate::common::resolve_tool_path(path_str);

    if !path.exists() || !path.is_dir() {
        return Err(anyhow::anyhow!(
            "Path is not a valid directory: {}",
            path_str
        ));
    }

    let canonical_path = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    let max_line_count_bytes = parse_u64_arg(
        &args,
        "max_line_count_bytes",
        DEFAULT_MAX_LINE_COUNT_BYTES,
        0,
        MAX_LINE_COUNT_BYTES_LIMIT,
    );
    let include_globs = parse_pattern_strings(args.get("includes"));
    let exclude_globs = parse_pattern_strings(args.get("excludes"));
    let includes = Arc::new(compile_patterns(&include_globs)?);
    let excludes = Arc::new(compile_patterns(&exclude_globs)?);

    if let Some(entries) = indexed_entries_under(&canonical_path) {
        return Ok(build_workspace_stats_from_index(
            path_str,
            &canonical_path,
            entries,
            max_line_count_bytes,
            includes.as_ref(),
            excludes.as_ref(),
        ));
    }

    let walk_threads = crate::common::bounded_walk_threads();
    let mut walker = WalkBuilder::new(&path);
    walker
        .hidden(true)
        .ignore(true)
        .git_ignore(true)
        .git_exclude(true)
        .require_git(false)
        .threads(walk_threads);
    apply_walk_overrides(&mut walker, &canonical_path, &include_globs, &exclude_globs)?;
    let filter_root = canonical_path.clone();
    let filter_excludes = Arc::clone(&excludes);
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
        let rel = relative_path(entry.path(), &filter_root);
        !matches_patterns(entry.path(), &rel, filter_excludes.as_ref())
    });

    let shard_count = walk_threads.max(1);
    let accumulators = Arc::new(
        (0..shard_count)
            .map(|_| Mutex::new(StatsAccumulator::default()))
            .collect::<Vec<_>>(),
    );
    let next_shard = Arc::new(AtomicUsize::new(0));
    let root_for_workers = canonical_path.clone();

    walker.build_parallel().run(|| {
        let includes = Arc::clone(&includes);
        let excludes = Arc::clone(&excludes);
        let accumulators = Arc::clone(&accumulators);
        let shard_index = next_shard.fetch_add(1, Ordering::Relaxed) % shard_count;
        let root = root_for_workers.clone();

        Box::new(move |result| {
            let entry = match result {
                Ok(entry) => entry,
                Err(_) => return WalkState::Continue,
            };

            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                return WalkState::Continue;
            }

            let rel = relative_path(entry.path(), &root);
            if !passes_patterns(entry.path(), &rel, includes.as_ref(), excludes.as_ref()) {
                let mut accumulator = match accumulators[shard_index].lock() {
                    Ok(guard) => guard,
                    Err(_) => return WalkState::Quit,
                };
                accumulator.record_skipped_file();
                return WalkState::Continue;
            }

            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            let ext = entry
                .path()
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            let lang = get_lang_from_ext(ext);
            let should_count = should_count_lines(lang, size, max_line_count_bytes);
            let line_count = if should_count {
                count_lines_in_file(entry.path())
            } else {
                0
            };

            let mut accumulator = match accumulators[shard_index].lock() {
                Ok(guard) => guard,
                Err(_) => return WalkState::Quit,
            };
            accumulator.record_matched_file(
                entry.path().to_string_lossy().to_string(),
                size,
                lang,
                line_count,
                should_count,
            );
            WalkState::Continue
        })
    });

    let mut stats = StatsAccumulator::default();
    for accumulator in accumulators.iter() {
        let mut accumulator = accumulator
            .lock()
            .map_err(|_| anyhow::anyhow!("workspace_stats accumulator is unavailable"))?;
        stats.merge(std::mem::take(&mut *accumulator));
    }

    let mut languages_out = Vec::new();
    for (lang, file_count) in &stats.lang_files {
        languages_out.push(json!({
            "language": lang,
            "files": file_count,
            "lines": stats.lang_lines.get(lang).unwrap_or(&0),
            "size_bytes": stats.lang_size.get(lang).unwrap_or(&0)
        }));
    }

    languages_out.sort_by(|a, b| {
        let count_a = a.get("files").and_then(|v| v.as_u64()).unwrap_or(0);
        let count_b = b.get("files").and_then(|v| v.as_u64()).unwrap_or(0);
        count_b.cmp(&count_a)
    });

    stats.largest_files.sort_by_key(|file| Reverse(file.1));
    stats.largest_files.truncate(10);
    let largest_out: Vec<Value> = stats
        .largest_files
        .iter()
        .map(|(p, s)| json!({ "path": p, "size": s }))
        .collect();

    Ok(json!({
        "path": path_str,
        "canonical_path": canonical_path.to_string_lossy(),
        "total_files": stats.total_files,
        "files_seen": stats.files_seen,
        "files_skipped_by_patterns": stats.files_skipped_by_patterns,
        "total_lines": stats.total_lines,
        "total_size_bytes": stats.total_size,
        "total_size_mb": format!("{:.1}", stats.total_size as f64 / 1_048_576.0),
        "languages_breakdown": languages_out,
        "largest_files": largest_out,
        "line_counted_files": stats.line_counted_files,
        "line_count_skipped_files": stats.line_count_skipped_files,
        "max_line_count_bytes": max_line_count_bytes,
        "line_counts_complete": stats.line_count_skipped_files == 0,
        "search_strategy": "filesystem_walk",
        "metadata_index_used": false,
        "note": if stats.line_count_skipped_files > 0 {
            format!("Line counts skip files over {} bytes or extensions treated as non-text.", max_line_count_bytes)
        } else {
            String::new()
        }
    }))
}

fn build_workspace_stats_from_index(
    path_str: &str,
    canonical_path: &Path,
    entries: Vec<crate::indexer::IndexedPathRecord>,
    max_line_count_bytes: u64,
    includes: &[Pattern],
    excludes: &[Pattern],
) -> Value {
    let mut stats = StatsAccumulator::default();

    for entry in entries {
        if entry.is_dir {
            continue;
        }

        let rel = relative_path(&entry.path, canonical_path);
        if !passes_patterns(&entry.path, &rel, includes, excludes) {
            stats.record_skipped_file();
            continue;
        }

        let lang = get_lang_from_ext(&entry.extension_lower);
        stats.record_matched_file(
            entry.path.to_string_lossy().to_string(),
            entry.size,
            lang,
            0,
            false,
        );
    }

    let mut languages_out = Vec::new();
    for (lang, file_count) in &stats.lang_files {
        languages_out.push(json!({
            "language": lang,
            "files": file_count,
            "lines": stats.lang_lines.get(lang).unwrap_or(&0),
            "size_bytes": stats.lang_size.get(lang).unwrap_or(&0)
        }));
    }

    languages_out.sort_by(|a, b| {
        let count_a = a.get("files").and_then(|v| v.as_u64()).unwrap_or(0);
        let count_b = b.get("files").and_then(|v| v.as_u64()).unwrap_or(0);
        count_b.cmp(&count_a)
    });

    stats.largest_files.sort_by_key(|file| Reverse(file.1));
    stats.largest_files.truncate(10);
    let largest_out: Vec<Value> = stats
        .largest_files
        .iter()
        .map(|(p, s)| json!({ "path": p, "size": s }))
        .collect();

    json!({
        "path": path_str,
        "canonical_path": canonical_path.to_string_lossy(),
        "total_files": stats.total_files,
        "files_seen": stats.files_seen,
        "files_skipped_by_patterns": stats.files_skipped_by_patterns,
        "total_lines": 0,
        "total_size_bytes": stats.total_size,
        "total_size_mb": format!("{:.1}", stats.total_size as f64 / 1_048_576.0),
        "languages_breakdown": languages_out,
        "largest_files": largest_out,
        "line_counted_files": 0,
        "line_count_skipped_files": stats.total_files,
        "max_line_count_bytes": max_line_count_bytes,
        "line_counts_complete": false,
        "search_strategy": "lmdb_metadata",
        "metadata_index_used": true,
        "note": "Line counts are not stored in the phase-1 metadata index; file counts, sizes, and language totals came from LMDB."
    })
}

#[derive(Default)]
struct StatsAccumulator {
    total_files: usize,
    files_seen: usize,
    files_skipped_by_patterns: usize,
    line_counted_files: usize,
    line_count_skipped_files: usize,
    total_size: u64,
    total_lines: u64,
    lang_files: HashMap<&'static str, usize>,
    lang_size: HashMap<&'static str, u64>,
    lang_lines: HashMap<&'static str, u64>,
    largest_files: Vec<(String, u64)>,
}

impl StatsAccumulator {
    fn record_skipped_file(&mut self) {
        self.files_seen += 1;
        self.files_skipped_by_patterns += 1;
    }

    fn record_matched_file(
        &mut self,
        path: String,
        size: u64,
        lang: &'static str,
        line_count: u64,
        line_counted: bool,
    ) {
        self.files_seen += 1;
        self.total_files += 1;
        self.total_size += size;
        self.total_lines += line_count;

        if line_counted {
            self.line_counted_files += 1;
        } else {
            self.line_count_skipped_files += 1;
        }

        *self.lang_files.entry(lang).or_insert(0) += 1;
        *self.lang_size.entry(lang).or_insert(0) += size;
        *self.lang_lines.entry(lang).or_insert(0) += line_count;
        self.push_largest_file(path, size);
    }

    fn merge(&mut self, mut other: Self) {
        self.total_files += other.total_files;
        self.files_seen += other.files_seen;
        self.files_skipped_by_patterns += other.files_skipped_by_patterns;
        self.line_counted_files += other.line_counted_files;
        self.line_count_skipped_files += other.line_count_skipped_files;
        self.total_size += other.total_size;
        self.total_lines += other.total_lines;

        for (lang, count) in other.lang_files {
            *self.lang_files.entry(lang).or_insert(0) += count;
        }
        for (lang, size) in other.lang_size {
            *self.lang_size.entry(lang).or_insert(0) += size;
        }
        for (lang, lines) in other.lang_lines {
            *self.lang_lines.entry(lang).or_insert(0) += lines;
        }

        self.largest_files.append(&mut other.largest_files);
        self.largest_files.sort_by_key(|file| Reverse(file.1));
        self.largest_files.truncate(10);
    }

    fn push_largest_file(&mut self, path: String, size: u64) {
        if self.largest_files.len() < 10
            || size > self.largest_files.last().map(|file| file.1).unwrap_or(0)
        {
            self.largest_files.push((path, size));
            if self.largest_files.len() > 20 {
                self.largest_files.sort_by_key(|file| Reverse(file.1));
                self.largest_files.truncate(10);
            }
        }
    }
}

fn should_count_lines(language: &str, size: u64, max_line_count_bytes: u64) -> bool {
    max_line_count_bytes > 0 && size <= max_line_count_bytes && language != "Other"
}

fn parse_u64_arg(args: &Value, name: &str, default: u64, min: u64, max: u64) -> u64 {
    args.get(name)
        .and_then(|value| value.as_u64())
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

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
