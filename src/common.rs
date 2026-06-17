use serde_json::{Map, Value};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const WORKSPACE_MARKERS: &[&str] = &[
    ".git",
    "Cargo.toml",
    "package.json",
    "pnpm-workspace.yaml",
    "yarn.lock",
    "package-lock.json",
    "pyproject.toml",
    "requirements.txt",
    "go.mod",
    "pom.xml",
    "build.gradle",
    "settings.gradle",
    "WORKSPACE",
    "WORKSPACE.bazel",
    ".hg",
    ".svn",
];

pub fn ensure_object(value: &mut Value) -> &mut Map<String, Value> {
    if !value.is_object() {
        *value = Value::Object(Map::new());
    }

    match value {
        Value::Object(object) => object,
        _ => unreachable!("value was normalized into an object"),
    }
}

pub fn insert_object_field(target: &mut Value, key: impl Into<String>, value: Value) {
    ensure_object(target).insert(key.into(), value);
}

pub fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn env_var_os(names: &[&str]) -> Option<OsString> {
    names.iter().find_map(std::env::var_os)
}

pub fn path_from_input(raw: &str) -> PathBuf {
    uri_to_path(raw).unwrap_or_else(|| PathBuf::from(raw))
}

pub fn uri_to_path(raw: &str) -> Option<PathBuf> {
    if let Some(path_str) = raw.strip_prefix("file:///") {
        return Some(PathBuf::from(percent_decode_uri_path(path_str)));
    }

    if let Some(path_str) = raw.strip_prefix("file://") {
        let decoded = percent_decode_uri_path(path_str);
        if decoded.starts_with('/') || decoded.starts_with('\\') {
            return Some(PathBuf::from(decoded));
        }

        return Some(PathBuf::from(format!("//{}", decoded)));
    }

    None
}

fn percent_decode_uri_path(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0usize;

    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            decoded.push((high << 4) | low);
            index += 3;
            continue;
        }

        decoded.push(bytes[index]);
        index += 1;
    }

    String::from_utf8(decoded)
        .unwrap_or_else(|err| String::from_utf8_lossy(err.as_bytes()).into_owned())
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }

    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

pub fn canonicalize_if_exists(path: PathBuf) -> PathBuf {
    if path.exists() {
        path.canonicalize().unwrap_or(path)
    } else {
        lexical_normalize(&path)
    }
}

pub fn looks_like_workspace_root(path: &Path) -> bool {
    if !path.exists() || !path.is_dir() {
        return false;
    }

    WORKSPACE_MARKERS
        .iter()
        .any(|marker| path.join(marker).exists())
}

pub fn discover_workspace_root(start: &Path) -> Option<PathBuf> {
    let mut current = if start.is_dir() {
        start.to_path_buf()
    } else {
        start.parent()?.to_path_buf()
    };

    loop {
        if looks_like_workspace_root(&current) {
            return Some(canonicalize_if_exists(current));
        }

        current = current.parent()?.to_path_buf();
    }
}

pub fn preferred_workspace_root() -> Option<(PathBuf, &'static str)> {
    if let Some(active_runtime) = crate::indexer::get_active_runtime_snapshot() {
        let root = canonicalize_if_exists(PathBuf::from(active_runtime.workspace_root));
        if root.exists() && root.is_dir() {
            return Some((root, "active_index_workspace"));
        }
    }

    if let Ok(current_dir) = std::env::current_dir()
        && let Some(root) = discover_workspace_root(&current_dir)
    {
        return Some((root, "workspace_root_discovered"));
    }

    std::env::current_dir()
        .ok()
        .map(|cwd| (canonicalize_if_exists(cwd), "current_dir"))
}

pub fn default_tool_root() -> PathBuf {
    preferred_workspace_root()
        .map(|(root, _)| root)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn resolve_tool_path(raw: &str) -> PathBuf {
    let path = path_from_input(raw);
    if path.is_absolute() {
        return canonicalize_if_exists(path);
    }

    canonicalize_if_exists(resolve_relative_tool_path(&path))
}

fn resolve_relative_tool_path(path: &Path) -> PathBuf {
    let mut roots = Vec::new();

    if let Some((root, _)) = preferred_workspace_root() {
        push_unique_path(&mut roots, root);
    }

    for runtime in crate::indexer::get_runtime_snapshots() {
        let root = canonicalize_if_exists(PathBuf::from(runtime.workspace_root));
        push_unique_path(&mut roots, root.clone());
        if let Some(parent) = root.parent() {
            push_unique_path(&mut roots, parent.to_path_buf());
        }
    }

    if let Ok(current_dir) = std::env::current_dir() {
        if let Some(root) = discover_workspace_root(&current_dir) {
            push_unique_path(&mut roots, root);
        }
        push_unique_path(&mut roots, canonicalize_if_exists(current_dir));
    }

    roots
        .into_iter()
        .map(|root| root.join(path))
        .max_by_key(|candidate| existing_ancestor_depth(candidate))
        .unwrap_or_else(|| PathBuf::from(path))
}

fn push_unique_path(paths: &mut Vec<PathBuf>, candidate: PathBuf) {
    let candidate_key = normalize_path_key(&candidate);
    if paths
        .iter()
        .any(|existing| normalize_path_key(existing) == candidate_key)
    {
        return;
    }

    paths.push(candidate);
}

fn existing_ancestor_depth(path: &Path) -> usize {
    let mut current = Some(path);
    while let Some(candidate) = current {
        if candidate.exists() {
            return candidate.components().count();
        }
        current = candidate.parent();
    }

    0
}

fn normalize_path_key(path: &Path) -> String {
    let normalized = path.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    {
        normalized.to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        normalized
    }
}

pub fn bounded_walk_threads() -> usize {
    std::thread::available_parallelism()
        .map(|parallelism| parallelism.get().clamp(1, 4))
        .unwrap_or(2)
}
