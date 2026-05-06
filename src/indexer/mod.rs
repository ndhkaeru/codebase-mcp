use dashmap::DashMap;
use heed::types::{SerdeJson, Str};
use heed::{Database, Env, EnvOpenOptions};
use ignore::{WalkBuilder, WalkState};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet, hash_map::DefaultHasher};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{
    Field, IndexRecordOption, STORED, STRING, Schema, TantivyDocument, TextFieldIndexing,
    TextOptions, Value,
};
use tantivy::{Index, ReloadPolicy, Term, doc};
use tracing::{error, info, warn};

use crate::common::{bounded_walk_threads, env_var, env_var_os};

lazy_static::lazy_static! {
    static ref INDEX_RUNTIMES: RwLock<BTreeMap<String, IndexRuntime>> = RwLock::new(BTreeMap::new());
    static ref PATH_INDEXES: DashMap<String, Arc<RwLock<PathIndex>>> = DashMap::new();
    static ref ACTIVE_REFRESHES: DashMap<String, ()> = DashMap::new();
    static ref ACTIVE_CONTENT_REFRESHES: DashMap<String, ()> = DashMap::new();
    static ref ACTIVE_WORKSPACE_KEY: RwLock<Option<String>> = RwLock::new(None);
}

const INDEX_SCHEMA_VERSION: u32 = 1;
const DEFAULT_STALE_INDEX_SECS: u64 = 60 * 60;
const MIN_REFRESH_INTERVAL_SECS: u64 = 30;
const MIN_INDEX_TERM_LEN: usize = 3;
const MAX_SHORTLIST_CANDIDATES: usize = 8_192;
const DEFAULT_INDEX_MAP_SIZE_MB: u64 = 4_096;
const DEFAULT_TANTIVY_MAX_FILE_BYTES: u64 = 1_048_576;
const DEFAULT_TANTIVY_MAX_ZONE_BYTES: u64 = 1_073_741_824;
const DEFAULT_TANTIVY_MAX_WORKSPACE_BYTES: u64 = 4_294_967_296;
const TANTIVY_WRITER_MEMORY_BYTES: usize = 64 * 1024 * 1024;
const ROOT_CHILDREN_KEY: &str = ".";

type MetaDb = Database<Str, SerdeJson<WorkspaceMeta>>;
type PathDb = Database<Str, SerdeJson<IndexedPathEntry>>;
type ChildrenDb = Database<Str, SerdeJson<Vec<String>>>;

#[derive(Debug, Clone)]
struct IndexRuntime {
    workspace_root: PathBuf,
    workspace_source: String,
    storage_dir: PathBuf,
    index_file: PathBuf,
    loaded_from_disk: bool,
    scan_complete: bool,
    refresh_running: bool,
    last_loaded_entries: usize,
    last_persisted_entries: usize,
    last_persisted_at: Option<u64>,
    last_scan_completed_at: Option<u64>,
    last_refresh_requested_at: Option<u64>,
    last_refresh_started_at: Option<u64>,
    last_refresh_completed_at: Option<u64>,
    last_request_source: Option<String>,
    last_error: Option<String>,
    indexed_entries_count: usize,
    indexed_files_count: usize,
    indexed_dirs_count: usize,
    content_index_enabled: bool,
    content_index_status: String,
    content_index_zones: Vec<String>,
    content_index_partial: bool,
    indexed_content_files: usize,
    indexed_content_bytes: u64,
    index_map_size_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct IndexRuntimeSnapshot {
    pub workspace_root: String,
    pub workspace_source: String,
    pub index_file: String,
    pub loaded_from_disk: bool,
    pub scan_complete: bool,
    pub last_loaded_entries: usize,
    pub last_persisted_entries: usize,
    pub last_persisted_at: Option<u64>,
    pub last_scan_completed_at: Option<u64>,
    pub last_refresh_requested_at: Option<u64>,
    pub last_request_source: Option<String>,
    pub last_error: Option<String>,
    pub indexed_entries_count: usize,
    pub cached_files_count: usize,
    pub index_kind: &'static str,
    pub index_status: String,
    pub metadata_index_backend: &'static str,
    pub content_index_backend: &'static str,
    pub metadata_index_status: String,
    pub content_index_status: String,
    pub content_index_zones: Vec<String>,
    pub content_index_partial: bool,
    pub indexed_content_files: usize,
    pub indexed_content_bytes: u64,
    pub index_storage_dir: String,
    pub index_map_size_bytes: u64,
    pub index_size_bytes: u64,
    pub indexed_files_count: usize,
    pub indexed_dirs_count: usize,
    pub refresh_running: bool,
    pub last_refresh_started_at: Option<u64>,
    pub last_refresh_completed_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct WorkspaceMeta {
    schema_version: u32,
    workspace_root: String,
    saved_at: u64,
    scan_complete: bool,
    indexed_entries_count: usize,
    indexed_files_count: usize,
    indexed_dirs_count: usize,
    last_full_scan_at: Option<u64>,
    content_index_enabled: bool,
    content_index_status: String,
    content_index_zones: Vec<String>,
    content_index_partial: bool,
    indexed_content_files: usize,
    indexed_content_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexedPathEntry {
    relative_path: String,
    is_dir: bool,
    size: u64,
    modified_at: u64,
    extension_lower: String,
    parent_relative_path: String,
    indexed_at: u64,
}

#[derive(Debug, Clone)]
struct PathIndexEntry {
    absolute_path: PathBuf,
    relative_path: String,
    relative_path_lower: String,
    file_name_lower: String,
    extension_lower: String,
    is_dir: bool,
    size: u64,
    modified_at: u64,
}

#[derive(Debug, Default)]
struct PathIndex {
    entries: Vec<Option<PathIndexEntry>>,
    path_lookup: HashMap<String, usize>,
    term_postings: HashMap<String, Vec<usize>>,
    live_entries: usize,
}

#[derive(Debug, Clone)]
pub struct PathQueryCandidate {
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub modified_at: u64,
}

#[derive(Debug, Clone)]
pub struct IndexedPathRecord {
    pub path: PathBuf,
    pub relative_path: String,
    pub file_name: String,
    pub extension_lower: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified_at: u64,
}

#[derive(Debug, Clone)]
pub struct ContentCandidateResult {
    pub paths: Vec<PathBuf>,
    pub content_index_used: bool,
    pub content_index_partial: bool,
    pub zones: Vec<String>,
}

struct IndexStore {
    env: Env,
    meta_db: MetaDb,
    path_db: PathDb,
    children_db: ChildrenDb,
    storage_dir: PathBuf,
    tantivy_dir: PathBuf,
}

#[derive(Clone, Copy)]
struct TantivyFields {
    relative_path: Field,
    file_name: Field,
    path_tokens: Field,
    content: Field,
    extension: Field,
    size: Field,
    modified_at: Field,
}

pub fn get_runtime_snapshots() -> Vec<IndexRuntimeSnapshot> {
    match INDEX_RUNTIMES.read() {
        Ok(guard) => guard
            .values()
            .cloned()
            .map(runtime_snapshot_from_state)
            .collect(),
        Err(_) => Vec::new(),
    }
}

pub fn get_active_runtime_snapshot() -> Option<IndexRuntimeSnapshot> {
    let active_key = match ACTIVE_WORKSPACE_KEY.read() {
        Ok(guard) => guard.clone(),
        Err(_) => None,
    }?;

    match INDEX_RUNTIMES.read() {
        Ok(guard) => guard
            .get(&active_key)
            .cloned()
            .map(runtime_snapshot_from_state),
        Err(_) => None,
    }
}

pub fn indexed_workspace_root_for_path(path: &Path) -> Option<PathBuf> {
    let canonical_path = canonicalize_or_original(path.to_path_buf());
    match INDEX_RUNTIMES.read() {
        Ok(guard) => guard
            .values()
            .filter(|state| path_belongs_to_workspace(&state.workspace_root, &canonical_path))
            .max_by_key(|state| state.workspace_root.components().count())
            .map(|state| state.workspace_root.clone()),
        Err(_) => None,
    }
}

pub fn is_path_index_ready(path: &Path) -> bool {
    let canonical_path = canonicalize_or_original(path.to_path_buf());
    match INDEX_RUNTIMES.read() {
        Ok(guard) => guard.values().any(|state| {
            state.scan_complete && path_belongs_to_workspace(&state.workspace_root, &canonical_path)
        }),
        Err(_) => false,
    }
}

pub fn is_path_index_available(path: &Path) -> bool {
    let canonical_path = canonicalize_or_original(path.to_path_buf());
    match INDEX_RUNTIMES.read() {
        Ok(guard) => guard.values().any(|state| {
            (state.scan_complete || state.indexed_entries_count > 0)
                && path_belongs_to_workspace(&state.workspace_root, &canonical_path)
        }),
        Err(_) => false,
    }
}

pub fn query_path_candidates(
    search_root: &Path,
    pattern: &str,
    shortlist_limit: usize,
) -> Option<Vec<PathQueryCandidate>> {
    let canonical_root = canonicalize_or_original(search_root.to_path_buf());
    let (workspace_key, workspace_root) = indexed_workspace_for_path(&canonical_root)?;
    let relative_root = relative_root_prefix(&workspace_root, &canonical_root)?;
    let anchor_terms = query_anchor_terms(pattern);
    if anchor_terms.is_empty() {
        return None;
    }

    let index = PATH_INDEXES.get(&workspace_key)?.value().clone();
    let guard = index.read().ok()?;
    guard.shortlist_candidates(&anchor_terms, relative_root.as_deref(), shortlist_limit)
}

pub fn indexed_entries_under(search_root: &Path) -> Option<Vec<IndexedPathRecord>> {
    let canonical_root = canonicalize_or_original(search_root.to_path_buf());
    let (workspace_key, workspace_root) = indexed_workspace_for_path(&canonical_root)?;
    let relative_root = relative_root_prefix(&workspace_root, &canonical_root)?;
    let index = PATH_INDEXES.get(&workspace_key)?.value().clone();
    let guard = index.read().ok()?;
    Some(guard.records_under(relative_root.as_deref()))
}

pub fn query_tantivy_content_candidates(
    search_paths: &[PathBuf],
    query: &str,
    limit: usize,
) -> ContentCandidateResult {
    if !tantivy_enabled() || !is_tantivy_query_compatible(query) || limit == 0 {
        return ContentCandidateResult {
            paths: Vec::new(),
            content_index_used: false,
            content_index_partial: false,
            zones: Vec::new(),
        };
    }

    let mut out = Vec::new();
    let mut zones = Vec::new();
    let mut used = false;
    let mut partial = false;

    for path in search_paths {
        let canonical_path = canonicalize_or_original(path.clone());
        let Some((workspace_key, workspace_root)) = indexed_workspace_for_path(&canonical_path)
        else {
            continue;
        };
        let zone = content_zone_for_path(&workspace_root, &canonical_path);
        if zone.is_empty() {
            partial = true;
            continue;
        }
        if !content_zone_ready(&workspace_key, &zone) {
            schedule_content_zone_refresh(workspace_root, workspace_key, zone);
            partial = true;
            continue;
        }

        let Ok(paths) =
            search_tantivy_zone(&workspace_root, query, limit.saturating_sub(out.len()))
        else {
            partial = true;
            continue;
        };
        if !paths.is_empty() {
            used = true;
            out.extend(paths);
            zones.push(zone);
        }
        if out.len() >= limit {
            break;
        }
    }

    out.sort();
    out.dedup();
    out.truncate(limit);

    ContentCandidateResult {
        paths: out,
        content_index_used: used,
        content_index_partial: partial,
        zones,
    }
}

pub fn stale_index_after_secs() -> u64 {
    env_var(&["CODEBASE_MCP_INDEX_STALE_SECS", "TURBO_FS_INDEX_STALE_SECS"])
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_STALE_INDEX_SECS)
}

pub fn ensure_workspace_index(workspace_root: PathBuf, workspace_source: String) {
    let workspace_root = canonicalize_or_original(workspace_root);
    if !workspace_root.exists() || !workspace_root.is_dir() {
        return;
    }

    let workspace_key = normalize_path_for_identity(&workspace_root);
    set_active_workspace(&workspace_key);

    let first_seen = ensure_runtime_loaded(&workspace_key, &workspace_root, &workspace_source);
    record_request_source(&workspace_key, &workspace_source);

    let now = current_unix_timestamp();
    let should_refresh = match INDEX_RUNTIMES.read() {
        Ok(guard) => guard.get(&workspace_key).is_none_or(|state| {
            if state.refresh_running {
                return false;
            }
            if !state.scan_complete || state.indexed_entries_count == 0 {
                return true;
            }
            state
                .last_scan_completed_at
                .map(|timestamp| now.saturating_sub(timestamp) >= stale_index_after_secs())
                .unwrap_or(true)
        }),
        Err(_) => first_seen,
    };

    if should_refresh && refresh_interval_elapsed(&workspace_key, now) {
        record_refresh_request(&workspace_key, &workspace_source, now);
        spawn_full_metadata_refresh(workspace_root, workspace_key);
    }
}

fn runtime_snapshot_from_state(state: IndexRuntime) -> IndexRuntimeSnapshot {
    let index_size_bytes = directory_size(&state.storage_dir);
    let metadata_status = if state.refresh_running {
        "refreshing".to_string()
    } else if state.scan_complete {
        "complete".to_string()
    } else if state.indexed_entries_count > 0 {
        "partial".to_string()
    } else if state.last_error.is_some() {
        "error".to_string()
    } else {
        "idle".to_string()
    };

    IndexRuntimeSnapshot {
        workspace_root: state.workspace_root.to_string_lossy().to_string(),
        workspace_source: state.workspace_source,
        index_file: state.index_file.to_string_lossy().to_string(),
        loaded_from_disk: state.loaded_from_disk,
        scan_complete: state.scan_complete,
        last_loaded_entries: state.last_loaded_entries,
        last_persisted_entries: state.last_persisted_entries,
        last_persisted_at: state.last_persisted_at,
        last_scan_completed_at: state.last_scan_completed_at,
        last_refresh_requested_at: state.last_refresh_requested_at,
        last_request_source: state.last_request_source,
        last_error: state.last_error,
        indexed_entries_count: state.indexed_entries_count,
        cached_files_count: state.indexed_entries_count,
        index_kind: "path",
        index_status: if state.indexed_entries_count > 0 {
            "active".to_string()
        } else {
            "idle".to_string()
        },
        metadata_index_backend: "heed_lmdb",
        content_index_backend: if state.content_index_enabled {
            "tantivy"
        } else {
            "disabled"
        },
        metadata_index_status: metadata_status,
        content_index_status: state.content_index_status,
        content_index_zones: state.content_index_zones,
        content_index_partial: state.content_index_partial,
        indexed_content_files: state.indexed_content_files,
        indexed_content_bytes: state.indexed_content_bytes,
        index_storage_dir: state.storage_dir.to_string_lossy().to_string(),
        index_map_size_bytes: state.index_map_size_bytes,
        index_size_bytes,
        indexed_files_count: state.indexed_files_count,
        indexed_dirs_count: state.indexed_dirs_count,
        refresh_running: state.refresh_running,
        last_refresh_started_at: state.last_refresh_started_at,
        last_refresh_completed_at: state.last_refresh_completed_at,
    }
}

fn ensure_runtime_loaded(
    workspace_key: &str,
    workspace_root: &Path,
    workspace_source: &str,
) -> bool {
    if INDEX_RUNTIMES
        .read()
        .ok()
        .is_some_and(|guard| guard.contains_key(workspace_key))
    {
        return false;
    }

    let storage_dir = index_storage_dir_for_workspace(workspace_root);
    let index_file = storage_dir.join("data.mdb");
    let map_size_bytes = index_map_size_bytes();

    PATH_INDEXES.insert(
        workspace_key.to_string(),
        Arc::new(RwLock::new(PathIndex::default())),
    );

    let mut runtime = IndexRuntime {
        workspace_root: workspace_root.to_path_buf(),
        workspace_source: workspace_source.to_string(),
        storage_dir: storage_dir.clone(),
        index_file,
        loaded_from_disk: false,
        scan_complete: false,
        refresh_running: false,
        last_loaded_entries: 0,
        last_persisted_entries: 0,
        last_persisted_at: None,
        last_scan_completed_at: None,
        last_refresh_requested_at: None,
        last_refresh_started_at: None,
        last_refresh_completed_at: None,
        last_request_source: Some(workspace_source.to_string()),
        last_error: None,
        indexed_entries_count: 0,
        indexed_files_count: 0,
        indexed_dirs_count: 0,
        content_index_enabled: tantivy_enabled(),
        content_index_status: if tantivy_enabled() {
            "idle".to_string()
        } else {
            "disabled".to_string()
        },
        content_index_zones: Vec::new(),
        content_index_partial: false,
        indexed_content_files: 0,
        indexed_content_bytes: 0,
        index_map_size_bytes: map_size_bytes,
    };

    match load_existing_index(workspace_key, workspace_root) {
        Ok(Some((meta, index))) => {
            let live_entries = index.live_entries;
            runtime.loaded_from_disk = true;
            runtime.scan_complete = meta.scan_complete;
            runtime.last_loaded_entries = live_entries;
            runtime.last_persisted_entries = live_entries;
            runtime.last_persisted_at = Some(meta.saved_at);
            runtime.last_scan_completed_at = meta.last_full_scan_at;
            runtime.indexed_entries_count = live_entries;
            runtime.indexed_files_count = meta.indexed_files_count;
            runtime.indexed_dirs_count = meta.indexed_dirs_count;
            runtime.content_index_enabled = meta.content_index_enabled;
            runtime.content_index_status = meta.content_index_status;
            runtime.content_index_zones = meta.content_index_zones;
            runtime.content_index_partial = meta.content_index_partial;
            runtime.indexed_content_files = meta.indexed_content_files;
            runtime.indexed_content_bytes = meta.indexed_content_bytes;
            if let Some(slot) = PATH_INDEXES.get(workspace_key)
                && let Ok(mut guard) = slot.value().write()
            {
                *guard = index;
            }
        }
        Ok(None) => {}
        Err(err) => {
            runtime.last_error = Some(err);
        }
    }

    with_runtime_map_write(|state| {
        state.insert(workspace_key.to_string(), runtime);
    });

    true
}

fn load_existing_index(
    workspace_key: &str,
    workspace_root: &Path,
) -> Result<Option<(WorkspaceMeta, PathIndex)>, String> {
    let store = open_store(workspace_root)?;
    let rtxn = store
        .env
        .read_txn()
        .map_err(|e| format!("read_txn_failed: {e}"))?;
    let Some(meta) = store
        .meta_db
        .get(&rtxn, "workspace")
        .map_err(|e| format!("meta_read_failed: {e}"))?
    else {
        return Ok(None);
    };

    if meta.schema_version != INDEX_SCHEMA_VERSION {
        return Err(format!(
            "unsupported_schema_version: got={}, expected={}",
            meta.schema_version, INDEX_SCHEMA_VERSION
        ));
    }
    if meta.workspace_root != normalize_path_for_identity(workspace_root) {
        return Err("workspace_mismatch".to_string());
    }

    let mut entries = Vec::new();
    for item in store
        .path_db
        .iter(&rtxn)
        .map_err(|e| format!("path_iter_failed: {e}"))?
    {
        let (_, entry) = item.map_err(|e| format!("path_decode_failed: {e}"))?;
        entries.push(entry);
    }

    let index = PathIndex::from_entries(workspace_root, entries);
    info!(
        workspace_key,
        entries = index.live_entries,
        "Loaded LMDB metadata index"
    );
    Ok(Some((meta, index)))
}

fn spawn_full_metadata_refresh(workspace_root: PathBuf, workspace_key: String) {
    if ACTIVE_REFRESHES.insert(workspace_key.clone(), ()).is_some() {
        return;
    }

    record_refresh_started(&workspace_key);
    thread::spawn(move || {
        let result = refresh_metadata_index(&workspace_root, &workspace_key);
        match result {
            Ok(summary) => record_refresh_success(&workspace_key, summary),
            Err(err) => {
                record_runtime_error(&workspace_key, err.clone());
                error!(workspace = %workspace_root.display(), error = %err, "Metadata index refresh failed");
            }
        }
        ACTIVE_REFRESHES.remove(&workspace_key);
    });
}

fn refresh_metadata_index(
    workspace_root: &Path,
    workspace_key: &str,
) -> Result<RefreshSummary, String> {
    let store = open_store(workspace_root)?;
    let indexed_at = current_unix_timestamp();
    let entries = collect_workspace_entries(workspace_root, indexed_at)?;
    let mut children: HashMap<String, Vec<String>> = HashMap::new();
    let mut files = 0usize;
    let mut dirs = 0usize;

    for entry in &entries {
        if entry.is_dir {
            dirs += 1;
        } else {
            files += 1;
        }
        if let Some(name) = file_name_from_relative_path(&entry.relative_path) {
            children
                .entry(entry.parent_relative_path.clone())
                .or_default()
                .push(name.to_string());
        }
    }
    for names in children.values_mut() {
        names.sort();
        names.dedup();
    }

    let mut wtxn = store
        .env
        .write_txn()
        .map_err(|e| format!("write_txn_failed: {e}"))?;
    store
        .path_db
        .clear(&mut wtxn)
        .map_err(|e| format!("path_clear_failed: {e}"))?;
    store
        .children_db
        .clear(&mut wtxn)
        .map_err(|e| format!("children_clear_failed: {e}"))?;

    for entry in &entries {
        store
            .path_db
            .put(&mut wtxn, entry.relative_path.as_str(), entry)
            .map_err(|e| format!("path_put_failed: {e}"))?;
    }
    for (parent, names) in &children {
        let parent_key = if parent.is_empty() {
            ROOT_CHILDREN_KEY
        } else {
            parent.as_str()
        };
        store
            .children_db
            .put(&mut wtxn, parent_key, names)
            .map_err(|e| format!("children_put_failed: {e}"))?;
    }

    let meta = WorkspaceMeta {
        schema_version: INDEX_SCHEMA_VERSION,
        workspace_root: normalize_path_for_identity(workspace_root),
        saved_at: indexed_at,
        scan_complete: true,
        indexed_entries_count: entries.len(),
        indexed_files_count: files,
        indexed_dirs_count: dirs,
        last_full_scan_at: Some(indexed_at),
        content_index_enabled: tantivy_enabled(),
        content_index_status: "idle".to_string(),
        content_index_zones: runtime_content_zones(workspace_key),
        content_index_partial: runtime_content_partial(workspace_key),
        indexed_content_files: runtime_indexed_content_files(workspace_key),
        indexed_content_bytes: runtime_indexed_content_bytes(workspace_key),
    };
    store
        .meta_db
        .put(&mut wtxn, "workspace", &meta)
        .map_err(|e| format!("meta_put_failed: {e}"))?;
    wtxn.commit().map_err(|e| format!("commit_failed: {e}"))?;

    write_sidecar_meta(&store.storage_dir, &meta)?;

    let index = PathIndex::from_entries(workspace_root, entries);
    let live_entries = index.live_entries;
    if let Some(slot) = PATH_INDEXES.get(workspace_key)
        && let Ok(mut guard) = slot.value().write()
    {
        *guard = index;
    }

    Ok(RefreshSummary {
        entries: live_entries,
        files,
        dirs,
        completed_at: indexed_at,
    })
}

fn collect_workspace_entries(
    workspace_root: &Path,
    indexed_at: u64,
) -> Result<Vec<IndexedPathEntry>, String> {
    let entries = Arc::new(Mutex::new(Vec::<IndexedPathEntry>::new()));
    let errors = Arc::new(AtomicUsize::new(0));
    let root = workspace_root.to_path_buf();
    let mut walk = WalkBuilder::new(workspace_root);
    walk.hidden(true)
        .ignore(true)
        .git_ignore(true)
        .git_exclude(true)
        .require_git(false)
        .threads(bounded_walk_threads());

    walk.build_parallel().run(|| {
        let entries = Arc::clone(&entries);
        let errors = Arc::clone(&errors);
        let root = root.clone();
        Box::new(move |result| {
            let entry = match result {
                Ok(entry) => entry,
                Err(_) => {
                    errors.fetch_add(1, Ordering::Relaxed);
                    return WalkState::Continue;
                }
            };
            let Some(file_type) = entry.file_type() else {
                return WalkState::Continue;
            };
            if !file_type.is_file() && !file_type.is_dir() {
                return WalkState::Continue;
            }
            let Ok(metadata) = entry.metadata() else {
                errors.fetch_add(1, Ordering::Relaxed);
                return WalkState::Continue;
            };
            let Some(indexed) = indexed_entry_from_metadata(
                &root,
                entry.path(),
                file_type.is_dir(),
                &metadata,
                indexed_at,
            ) else {
                return WalkState::Continue;
            };
            match entries.lock() {
                Ok(mut guard) => guard.push(indexed),
                Err(_) => return WalkState::Quit,
            }
            WalkState::Continue
        })
    });

    if errors.load(Ordering::Relaxed) > 0 {
        warn!(
            workspace = %workspace_root.display(),
            errors = errors.load(Ordering::Relaxed),
            "Metadata refresh skipped some entries"
        );
    }

    let mut entries = Arc::try_unwrap(entries)
        .map_err(|_| "entry_collector_still_shared".to_string())?
        .into_inner()
        .map_err(|_| "entry_collector_poisoned".to_string())?;
    entries.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(entries)
}

fn indexed_entry_from_metadata(
    workspace_root: &Path,
    path: &Path,
    is_dir: bool,
    metadata: &fs::Metadata,
    indexed_at: u64,
) -> Option<IndexedPathEntry> {
    if path == workspace_root {
        return None;
    }
    let relative_path = path
        .strip_prefix(workspace_root)
        .ok()
        .map(normalize_path)
        .filter(|relative| !relative.is_empty())?;
    let parent_relative_path = path
        .parent()
        .and_then(|parent| parent.strip_prefix(workspace_root).ok())
        .map(normalize_path)
        .unwrap_or_default();
    let extension_lower = if is_dir {
        String::new()
    } else {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .unwrap_or_default()
    };
    Some(IndexedPathEntry {
        relative_path,
        is_dir,
        size: if is_dir { 0 } else { metadata.len() },
        modified_at: metadata_modified_secs(metadata),
        extension_lower,
        parent_relative_path,
        indexed_at,
    })
}

fn open_store(workspace_root: &Path) -> Result<IndexStore, String> {
    let storage_dir = index_storage_dir_for_workspace(workspace_root);
    fs::create_dir_all(&storage_dir).map_err(|e| format!("create_index_dir_failed: {e}"))?;
    let env = unsafe {
        EnvOpenOptions::new()
            .map_size(index_map_size_bytes() as usize)
            .max_dbs(16)
            .open(&storage_dir)
            .map_err(|e| format!("lmdb_open_failed: {e}"))?
    };
    let mut wtxn = env
        .write_txn()
        .map_err(|e| format!("write_txn_failed: {e}"))?;
    let meta_db: MetaDb = env
        .create_database(&mut wtxn, Some("workspace_meta"))
        .map_err(|e| format!("meta_db_open_failed: {e}"))?;
    let path_db: PathDb = env
        .create_database(&mut wtxn, Some("path_by_rel"))
        .map_err(|e| format!("path_db_open_failed: {e}"))?;
    let children_db: ChildrenDb = env
        .create_database(&mut wtxn, Some("children_by_parent"))
        .map_err(|e| format!("children_db_open_failed: {e}"))?;
    wtxn.commit()
        .map_err(|e| format!("db_open_commit_failed: {e}"))?;
    let tantivy_dir = storage_dir.join("tantivy-content");
    Ok(IndexStore {
        env,
        meta_db,
        path_db,
        children_db,
        storage_dir,
        tantivy_dir,
    })
}

fn schedule_content_zone_refresh(workspace_root: PathBuf, workspace_key: String, zone: String) {
    if !tantivy_enabled() {
        return;
    }
    let refresh_key = format!("{workspace_key}\n{zone}");
    if ACTIVE_CONTENT_REFRESHES
        .insert(refresh_key.clone(), ())
        .is_some()
    {
        return;
    }
    record_content_status(&workspace_key, "warming", None, false, 0, 0);
    thread::spawn(move || {
        let result = refresh_tantivy_zone(&workspace_root, &workspace_key, &zone);
        match result {
            Ok(summary) => record_content_status(
                &workspace_key,
                "ready",
                Some(zone.clone()),
                summary.partial,
                summary.files,
                summary.bytes,
            ),
            Err(err) => {
                record_runtime_error(&workspace_key, err.clone());
                record_content_status(&workspace_key, "error", None, true, 0, 0);
                error!(workspace = %workspace_root.display(), zone, error = %err, "Tantivy zone refresh failed");
            }
        }
        ACTIVE_CONTENT_REFRESHES.remove(&refresh_key);
    });
}

fn refresh_tantivy_zone(
    workspace_root: &Path,
    workspace_key: &str,
    zone: &str,
) -> Result<ContentRefreshSummary, String> {
    let store = open_store(workspace_root)?;
    let index = open_or_create_tantivy(&store.tantivy_dir)?;
    let fields = tantivy_fields(&index.schema())?;
    let mut writer = index
        .writer_with_num_threads(1, TANTIVY_WRITER_MEMORY_BYTES)
        .map_err(|e| format!("tantivy_writer_failed: {e}"))?;
    let entries = indexed_entries_for_content_zone(workspace_key, zone, workspace_root)?;
    let mut indexed_files = 0usize;
    let mut indexed_bytes = 0u64;
    let mut partial = false;
    let limits = ContentPolicy::from_env();
    let zone_is_workspace_root = zone.is_empty();
    let allow_third_party = zone == "third_party" || zone.starts_with("third_party/");

    for entry in entries {
        if entry.is_dir {
            continue;
        }
        writer.delete_term(Term::from_field_text(
            fields.relative_path,
            &entry.relative_path,
        ));
        if !content_policy_allows(&entry, &limits, allow_third_party) {
            continue;
        }
        if indexed_bytes.saturating_add(entry.size) > limits.max_workspace_bytes
            || indexed_bytes.saturating_add(entry.size) > limits.max_zone_bytes
        {
            partial = true;
            break;
        }

        let content = if zone_is_workspace_root {
            String::new()
        } else {
            read_indexable_content(&entry.path, limits.max_file_bytes)?
        };
        let mut document = TantivyDocument::new();
        document.add_text(fields.relative_path, &entry.relative_path);
        document.add_text(fields.file_name, &entry.file_name);
        document.add_text(
            fields.path_tokens,
            path_tokens_for_tantivy(&entry.relative_path),
        );
        document.add_text(fields.content, &content);
        document.add_text(fields.extension, &entry.extension_lower);
        document.add_u64(fields.size, entry.size);
        document.add_u64(fields.modified_at, entry.modified_at);
        writer
            .add_document(document)
            .map_err(|e| format!("tantivy_add_document_failed: {e}"))?;
        indexed_files += 1;
        if !zone_is_workspace_root {
            indexed_bytes = indexed_bytes.saturating_add(entry.size);
        }
    }

    writer
        .commit()
        .map_err(|e| format!("tantivy_commit_failed: {e}"))?;

    Ok(ContentRefreshSummary {
        files: indexed_files,
        bytes: indexed_bytes,
        partial,
    })
}

fn search_tantivy_zone(
    workspace_root: &Path,
    query: &str,
    limit: usize,
) -> Result<Vec<PathBuf>, String> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let store = open_store(workspace_root)?;
    if !store.tantivy_dir.exists() {
        return Ok(Vec::new());
    }
    let index =
        Index::open_in_dir(&store.tantivy_dir).map_err(|e| format!("tantivy_open_failed: {e}"))?;
    let schema = index.schema();
    let fields = tantivy_fields(&schema)?;
    let reader = index
        .reader_builder()
        .reload_policy(ReloadPolicy::Manual)
        .try_into()
        .map_err(|e| format!("tantivy_reader_failed: {e}"))?;
    let searcher = reader.searcher();
    let parser = QueryParser::for_index(
        &index,
        vec![
            fields.content,
            fields.path_tokens,
            fields.file_name,
            fields.relative_path,
        ],
    );
    let parsed = parser
        .parse_query(query)
        .map_err(|e| format!("tantivy_query_parse_failed: {e}"))?;
    let top_docs = searcher
        .search(&parsed, &TopDocs::with_limit(limit).order_by_score())
        .map_err(|e| format!("tantivy_search_failed: {e}"))?;
    let mut paths = Vec::new();
    for (_, address) in top_docs {
        let doc = searcher
            .doc::<TantivyDocument>(address)
            .map_err(|e| format!("tantivy_doc_fetch_failed: {e}"))?;
        if let Some(relative_path) = doc
            .get_first(fields.relative_path)
            .and_then(|value| value.as_str())
        {
            paths.push(
                workspace_root.join(relative_path.replace('/', std::path::MAIN_SEPARATOR_STR)),
            );
        }
    }
    Ok(paths)
}

fn open_or_create_tantivy(tantivy_dir: &Path) -> Result<Index, String> {
    if tantivy_dir.exists()
        && let Ok(index) = Index::open_in_dir(tantivy_dir)
        && tantivy_fields(&index.schema()).is_ok()
    {
        return Ok(index);
    }

    if tantivy_dir.exists() {
        let _ = fs::remove_dir_all(tantivy_dir);
    }
    fs::create_dir_all(tantivy_dir).map_err(|e| format!("tantivy_dir_create_failed: {e}"))?;
    Index::create_in_dir(tantivy_dir, tantivy_schema())
        .map_err(|e| format!("tantivy_create_failed: {e}"))
}

fn tantivy_schema() -> Schema {
    let mut schema_builder = Schema::builder();
    let text_options = TextOptions::default()
        .set_indexing_options(
            TextFieldIndexing::default().set_index_option(IndexRecordOption::WithFreqsAndPositions),
        )
        .set_stored();
    schema_builder.add_text_field("relative_path", STRING | STORED);
    schema_builder.add_text_field("file_name", STRING | STORED);
    schema_builder.add_text_field("path_tokens", text_options.clone());
    schema_builder.add_text_field("content", text_options);
    schema_builder.add_text_field("extension", STRING | STORED);
    schema_builder.add_u64_field("size", STORED);
    schema_builder.add_u64_field("modified_at", STORED);
    schema_builder.build()
}

fn tantivy_fields(schema: &Schema) -> Result<TantivyFields, String> {
    Ok(TantivyFields {
        relative_path: schema
            .get_field("relative_path")
            .map_err(|_| "tantivy_schema_missing_relative_path".to_string())?,
        file_name: schema
            .get_field("file_name")
            .map_err(|_| "tantivy_schema_missing_file_name".to_string())?,
        path_tokens: schema
            .get_field("path_tokens")
            .map_err(|_| "tantivy_schema_missing_path_tokens".to_string())?,
        content: schema
            .get_field("content")
            .map_err(|_| "tantivy_schema_missing_content".to_string())?,
        extension: schema
            .get_field("extension")
            .map_err(|_| "tantivy_schema_missing_extension".to_string())?,
        size: schema
            .get_field("size")
            .map_err(|_| "tantivy_schema_missing_size".to_string())?,
        modified_at: schema
            .get_field("modified_at")
            .map_err(|_| "tantivy_schema_missing_modified_at".to_string())?,
    })
}

fn indexed_entries_for_content_zone(
    workspace_key: &str,
    zone: &str,
    workspace_root: &Path,
) -> Result<Vec<IndexedPathRecord>, String> {
    let Some(index) = PATH_INDEXES
        .get(workspace_key)
        .map(|entry| entry.value().clone())
    else {
        return Ok(Vec::new());
    };
    let guard = index
        .read()
        .map_err(|_| "path_index_read_failed".to_string())?;
    let relative_root = if zone.is_empty() { None } else { Some(zone) };
    let mut records = guard.records_under(relative_root);
    if records.is_empty() && !zone.is_empty() {
        let zone_path = workspace_root.join(zone.replace('/', std::path::MAIN_SEPARATOR_STR));
        records = fallback_walk_records(workspace_root, &zone_path)?;
    }
    Ok(records)
}

fn fallback_walk_records(
    workspace_root: &Path,
    zone_path: &Path,
) -> Result<Vec<IndexedPathRecord>, String> {
    let mut records = Vec::new();
    for entry in WalkBuilder::new(zone_path)
        .hidden(true)
        .ignore(true)
        .git_ignore(true)
        .git_exclude(true)
        .require_git(false)
        .build()
        .flatten()
    {
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() && !file_type.is_dir() {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if let Some(indexed) = indexed_entry_from_metadata(
            workspace_root,
            entry.path(),
            file_type.is_dir(),
            &metadata,
            current_unix_timestamp(),
        ) {
            records.push(indexed.to_record(workspace_root));
        }
    }
    Ok(records)
}

fn content_policy_allows(
    entry: &IndexedPathRecord,
    limits: &ContentPolicy,
    allow_third_party: bool,
) -> bool {
    if entry.is_dir || entry.size > limits.max_file_bytes {
        return false;
    }
    if !CONTENT_EXTENSIONS.contains(&entry.extension_lower.as_str()) {
        return false;
    }
    let relative = entry.relative_path.replace('\\', "/").to_ascii_lowercase();
    if !allow_third_party && (relative == "third_party" || relative.starts_with("third_party/")) {
        return false;
    }
    if relative.contains("/out/")
        || relative.starts_with("out/")
        || relative.contains("/generated/")
        || relative.contains("/gen/")
    {
        return false;
    }
    true
}

const CONTENT_EXTENSIONS: &[&str] = &[
    "cc", "h", "c", "cpp", "hpp", "rs", "ts", "tsx", "js", "jsx", "py", "gn", "gni", "md", "json",
    "yaml", "yml", "toml",
];

struct ContentPolicy {
    max_file_bytes: u64,
    max_zone_bytes: u64,
    max_workspace_bytes: u64,
}

impl ContentPolicy {
    fn from_env() -> Self {
        Self {
            max_file_bytes: env_u64(
                "CODEBASE_MCP_TANTIVY_MAX_FILE_BYTES",
                DEFAULT_TANTIVY_MAX_FILE_BYTES,
            ),
            max_zone_bytes: env_u64(
                "CODEBASE_MCP_TANTIVY_MAX_ZONE_BYTES",
                DEFAULT_TANTIVY_MAX_ZONE_BYTES,
            ),
            max_workspace_bytes: env_u64(
                "CODEBASE_MCP_TANTIVY_MAX_WORKSPACE_BYTES",
                DEFAULT_TANTIVY_MAX_WORKSPACE_BYTES,
            ),
        }
    }
}

fn read_indexable_content(path: &Path, max_file_bytes: u64) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| format!("content_open_failed: {e}"))?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(max_file_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| format!("content_read_failed: {e}"))?;
    if bytes.len() as u64 > max_file_bytes || bytes.contains(&0) {
        return Ok(String::new());
    }
    Ok(String::from_utf8(bytes).unwrap_or_default())
}

fn path_tokens_for_tantivy(relative_path: &str) -> String {
    relative_path
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_tantivy_query_compatible(query: &str) -> bool {
    !query.trim().is_empty()
        && query.chars().all(|ch| {
            ch.is_ascii_alphanumeric() || ch.is_ascii_whitespace() || "_-./:".contains(ch)
        })
}

fn content_zone_for_path(workspace_root: &Path, path: &Path) -> String {
    let target = if path.is_file() {
        path.parent().unwrap_or(path)
    } else {
        path
    };
    target
        .strip_prefix(workspace_root)
        .ok()
        .map(normalize_path)
        .unwrap_or_default()
}

fn content_zone_ready(workspace_key: &str, zone: &str) -> bool {
    INDEX_RUNTIMES
        .read()
        .ok()
        .and_then(|guard| guard.get(workspace_key).cloned())
        .is_some_and(|state| {
            state.content_index_status == "ready"
                && state
                    .content_index_zones
                    .iter()
                    .any(|existing| existing == zone)
        })
}

#[derive(Debug)]
struct RefreshSummary {
    entries: usize,
    files: usize,
    dirs: usize,
    completed_at: u64,
}

#[derive(Debug)]
struct ContentRefreshSummary {
    files: usize,
    bytes: u64,
    partial: bool,
}

impl PathIndex {
    fn from_entries(workspace_root: &Path, entries: Vec<IndexedPathEntry>) -> Self {
        let mut index = Self::default();
        for entry in entries {
            index.insert_persisted_entry(workspace_root, entry);
        }
        index
    }

    fn insert_persisted_entry(&mut self, workspace_root: &Path, entry: IndexedPathEntry) {
        let normalized_key = entry.relative_path.to_ascii_lowercase();
        if let Some(existing) = self.path_lookup.get(&normalized_key).copied() {
            self.entries[existing] = None;
        }

        let absolute_path = workspace_root.join(
            entry
                .relative_path
                .replace('/', std::path::MAIN_SEPARATOR_STR),
        );
        let file_name_lower = file_name_from_relative_path(&entry.relative_path)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let runtime_entry = PathIndexEntry {
            absolute_path,
            relative_path_lower: entry.relative_path.to_ascii_lowercase(),
            relative_path: entry.relative_path,
            file_name_lower,
            extension_lower: entry.extension_lower,
            is_dir: entry.is_dir,
            size: entry.size,
            modified_at: entry.modified_at,
        };

        let index = self.entries.len();
        self.path_lookup.insert(normalized_key, index);
        for term in index_terms(&runtime_entry) {
            self.term_postings.entry(term).or_default().push(index);
        }
        self.entries.push(Some(runtime_entry));
        self.live_entries += 1;
    }

    fn shortlist_candidates(
        &self,
        anchor_terms: &[String],
        relative_root: Option<&str>,
        limit: usize,
    ) -> Option<Vec<PathQueryCandidate>> {
        if anchor_terms.is_empty() {
            return None;
        }

        let mut workset: Option<HashSet<usize>> = None;
        for term in anchor_terms {
            let postings = self.term_postings.get(term)?;
            let set: HashSet<usize> = postings.iter().copied().collect();
            workset = Some(match workset {
                Some(existing) => existing.intersection(&set).copied().collect(),
                None => set,
            });
        }

        let relative_root_lower = relative_root.map(|root| root.to_ascii_lowercase());
        let mut candidates = Vec::new();
        for index in workset.unwrap_or_default() {
            if candidates.len() >= limit.min(MAX_SHORTLIST_CANDIDATES) {
                break;
            }
            let Some(entry) = self.entries.get(index).and_then(|entry| entry.as_ref()) else {
                continue;
            };
            if !relative_root_matches(&entry.relative_path_lower, relative_root_lower.as_deref()) {
                continue;
            }
            candidates.push(PathQueryCandidate {
                path: entry.absolute_path.clone(),
                is_dir: entry.is_dir,
                size: entry.size,
                modified_at: entry.modified_at,
            });
        }

        Some(candidates)
    }

    fn records_under(&self, relative_root: Option<&str>) -> Vec<IndexedPathRecord> {
        let relative_root_lower = relative_root.map(|root| root.to_ascii_lowercase());
        let mut records = Vec::new();
        for entry in self.entries.iter().filter_map(|entry| entry.as_ref()) {
            if !relative_root_matches(&entry.relative_path_lower, relative_root_lower.as_deref()) {
                continue;
            }
            records.push(IndexedPathRecord {
                path: entry.absolute_path.clone(),
                relative_path: entry.relative_path.clone(),
                file_name: file_name_from_relative_path(&entry.relative_path)
                    .unwrap_or_default()
                    .to_string(),
                extension_lower: entry.extension_lower.clone(),
                is_dir: entry.is_dir,
                size: entry.size,
                modified_at: entry.modified_at,
            });
        }
        records
    }
}

impl IndexedPathEntry {
    fn to_record(&self, workspace_root: &Path) -> IndexedPathRecord {
        IndexedPathRecord {
            path: workspace_root.join(
                self.relative_path
                    .replace('/', std::path::MAIN_SEPARATOR_STR),
            ),
            relative_path: self.relative_path.clone(),
            file_name: file_name_from_relative_path(&self.relative_path)
                .unwrap_or_default()
                .to_string(),
            extension_lower: self.extension_lower.clone(),
            is_dir: self.is_dir,
            size: self.size,
            modified_at: self.modified_at,
        }
    }
}

fn index_terms(entry: &PathIndexEntry) -> Vec<String> {
    let mut terms = entry
        .relative_path_lower
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .chain(
            entry
                .file_name_lower
                .split(|ch: char| !ch.is_ascii_alphanumeric()),
        )
        .filter(|term| term.len() >= MIN_INDEX_TERM_LEN)
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if entry.extension_lower.len() >= MIN_INDEX_TERM_LEN {
        terms.push(entry.extension_lower.clone());
    }
    terms.sort();
    terms.dedup();
    terms
}

fn query_anchor_terms(pattern: &str) -> Vec<String> {
    let mut terms = pattern
        .to_ascii_lowercase()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|term| term.len() >= MIN_INDEX_TERM_LEN)
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    terms.sort();
    terms.dedup();
    terms
}

fn relative_root_matches(relative_path_lower: &str, relative_root_lower: Option<&str>) -> bool {
    let Some(root) = relative_root_lower else {
        return true;
    };
    root.is_empty()
        || relative_path_lower == root
        || relative_path_lower.starts_with(&(root.to_string() + "/"))
}

fn indexed_workspace_for_path(path: &Path) -> Option<(String, PathBuf)> {
    let canonical_path = canonicalize_or_original(path.to_path_buf());
    match INDEX_RUNTIMES.read() {
        Ok(guard) => guard
            .iter()
            .filter(|(_, state)| {
                (state.scan_complete || state.indexed_entries_count > 0)
                    && path_belongs_to_workspace(&state.workspace_root, &canonical_path)
            })
            .max_by_key(|(_, state)| state.workspace_root.components().count())
            .map(|(key, state)| (key.clone(), state.workspace_root.clone())),
        Err(_) => None,
    }
}

fn relative_root_prefix(workspace_root: &Path, search_root: &Path) -> Option<Option<String>> {
    if workspace_root == search_root {
        return Some(None);
    }
    search_root
        .strip_prefix(workspace_root)
        .ok()
        .map(normalize_path)
        .map(Some)
}

fn refresh_interval_elapsed(workspace_key: &str, now: u64) -> bool {
    INDEX_RUNTIMES
        .read()
        .ok()
        .and_then(|guard| guard.get(workspace_key).cloned())
        .and_then(|state| state.last_refresh_requested_at)
        .is_none_or(|timestamp| now.saturating_sub(timestamp) >= MIN_REFRESH_INTERVAL_SECS)
}

fn set_active_workspace(workspace_key: &str) {
    if let Ok(mut guard) = ACTIVE_WORKSPACE_KEY.write() {
        *guard = Some(workspace_key.to_string());
    }
}

fn record_request_source(workspace_key: &str, workspace_source: &str) {
    with_runtime_write(workspace_key, |state| {
        state.workspace_source = workspace_source.to_string();
        state.last_request_source = Some(workspace_source.to_string());
    });
}

fn record_refresh_request(workspace_key: &str, workspace_source: &str, now: u64) {
    with_runtime_write(workspace_key, |state| {
        state.last_refresh_requested_at = Some(now);
        state.last_request_source = Some(workspace_source.to_string());
    });
}

fn record_refresh_started(workspace_key: &str) {
    with_runtime_write(workspace_key, |state| {
        state.refresh_running = true;
        state.last_refresh_started_at = Some(current_unix_timestamp());
        state.last_error = None;
    });
}

fn record_refresh_success(workspace_key: &str, summary: RefreshSummary) {
    with_runtime_write(workspace_key, |state| {
        state.refresh_running = false;
        state.loaded_from_disk = true;
        state.scan_complete = true;
        state.indexed_entries_count = summary.entries;
        state.indexed_files_count = summary.files;
        state.indexed_dirs_count = summary.dirs;
        state.last_persisted_entries = summary.entries;
        state.last_loaded_entries = summary.entries;
        state.last_persisted_at = Some(summary.completed_at);
        state.last_scan_completed_at = Some(summary.completed_at);
        state.last_refresh_completed_at = Some(summary.completed_at);
        state.last_error = None;
    });
}

fn record_runtime_error(workspace_key: &str, err: String) {
    with_runtime_write(workspace_key, |state| {
        state.refresh_running = false;
        state.last_error = Some(err);
    });
}

fn record_content_status(
    workspace_key: &str,
    status: &str,
    zone: Option<String>,
    partial: bool,
    files: usize,
    bytes: u64,
) {
    let mut persist_state = None;
    with_runtime_map_write(|state| {
        if let Some(runtime) = state.get_mut(workspace_key) {
            runtime.content_index_status = status.to_string();
            runtime.content_index_partial = partial;
            let mut new_zone_added = false;
            if let Some(zone) = zone
                && !runtime.content_index_zones.contains(&zone)
            {
                runtime.content_index_zones.push(zone);
                runtime.content_index_zones.sort();
                new_zone_added = true;
            }
            if (files > 0 || bytes > 0) && new_zone_added {
                runtime.indexed_content_files = runtime.indexed_content_files.saturating_add(files);
                runtime.indexed_content_bytes = runtime.indexed_content_bytes.saturating_add(bytes);
            }
            persist_state = Some((
                runtime.workspace_root.clone(),
                runtime.content_index_status.clone(),
                runtime.content_index_zones.clone(),
                runtime.content_index_partial,
                runtime.indexed_content_files,
                runtime.indexed_content_bytes,
            ));
        }
    });

    if let Some((workspace_root, status, zones, partial, files, bytes)) = persist_state
        && let Err(err) =
            persist_content_status(&workspace_root, status, zones, partial, files, bytes)
    {
        warn!(
            workspace = %workspace_root.display(),
            error = %err,
            "Failed to persist Tantivy content index status"
        );
    }
}

fn persist_content_status(
    workspace_root: &Path,
    status: String,
    zones: Vec<String>,
    partial: bool,
    files: usize,
    bytes: u64,
) -> Result<(), String> {
    let store = open_store(workspace_root)?;
    let mut wtxn = store
        .env
        .write_txn()
        .map_err(|e| format!("write_txn_failed: {e}"))?;
    let Some(mut meta) = store
        .meta_db
        .get(&wtxn, "workspace")
        .map_err(|e| format!("meta_read_failed: {e}"))?
    else {
        return Ok(());
    };

    meta.content_index_enabled = tantivy_enabled();
    meta.content_index_status = status;
    meta.content_index_zones = zones;
    meta.content_index_partial = partial;
    meta.indexed_content_files = files;
    meta.indexed_content_bytes = bytes;
    meta.saved_at = current_unix_timestamp();

    store
        .meta_db
        .put(&mut wtxn, "workspace", &meta)
        .map_err(|e| format!("meta_put_failed: {e}"))?;
    wtxn.commit()
        .map_err(|e| format!("content_meta_commit_failed: {e}"))
}

fn with_runtime_write(workspace_key: &str, f: impl FnOnce(&mut IndexRuntime)) {
    with_runtime_map_write(|state| {
        if let Some(runtime) = state.get_mut(workspace_key) {
            f(runtime);
        }
    });
}

fn with_runtime_map_write(f: impl FnOnce(&mut BTreeMap<String, IndexRuntime>)) {
    if let Ok(mut guard) = INDEX_RUNTIMES.write() {
        f(&mut guard);
    }
}

fn runtime_content_zones(workspace_key: &str) -> Vec<String> {
    INDEX_RUNTIMES
        .read()
        .ok()
        .and_then(|guard| {
            guard
                .get(workspace_key)
                .map(|state| state.content_index_zones.clone())
        })
        .unwrap_or_default()
}

fn runtime_content_partial(workspace_key: &str) -> bool {
    INDEX_RUNTIMES
        .read()
        .ok()
        .and_then(|guard| {
            guard
                .get(workspace_key)
                .map(|state| state.content_index_partial)
        })
        .unwrap_or(false)
}

fn runtime_indexed_content_files(workspace_key: &str) -> usize {
    INDEX_RUNTIMES
        .read()
        .ok()
        .and_then(|guard| {
            guard
                .get(workspace_key)
                .map(|state| state.indexed_content_files)
        })
        .unwrap_or(0)
}

fn runtime_indexed_content_bytes(workspace_key: &str) -> u64 {
    INDEX_RUNTIMES
        .read()
        .ok()
        .and_then(|guard| {
            guard
                .get(workspace_key)
                .map(|state| state.indexed_content_bytes)
        })
        .unwrap_or(0)
}

fn index_storage_dir_for_workspace(workspace_root: &Path) -> PathBuf {
    index_storage_root().join(hash_workspace_root(workspace_root))
}

fn index_storage_root() -> PathBuf {
    if let Some(custom_dir) = env_var_os(&["CODEBASE_MCP_INDEX_DIR", "TURBO_FS_INDEX_DIR"]) {
        return PathBuf::from(custom_dir).join("index-v2");
    }
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local_app_data)
            .join("codebase-mcp")
            .join("index-v2");
    }
    if let Some(xdg_cache_home) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(xdg_cache_home)
            .join("codebase-mcp")
            .join("index-v2");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".cache")
            .join("codebase-mcp")
            .join("index-v2");
    }
    std::env::temp_dir().join("codebase-mcp").join("index-v2")
}

fn hash_workspace_root(workspace_root: &Path) -> String {
    let normalized = normalize_path_for_identity(workspace_root);
    let mut hasher = DefaultHasher::new();
    normalized.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn write_sidecar_meta(storage_dir: &Path, meta: &WorkspaceMeta) -> Result<(), String> {
    let payload =
        serde_json::to_vec_pretty(meta).map_err(|e| format!("meta_serialize_failed: {e}"))?;
    fs::write(storage_dir.join("meta.json"), payload).map_err(|e| format!("meta_write_failed: {e}"))
}

fn directory_size(path: &Path) -> u64 {
    let mut total = 0u64;
    let Ok(read_dir) = fs::read_dir(path) else {
        return 0;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        if let Ok(metadata) = entry.metadata() {
            if metadata.is_dir() {
                total = total.saturating_add(directory_size(&path));
            } else {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    total
}

fn canonicalize_or_original(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

fn path_belongs_to_workspace(workspace_root: &Path, path: &Path) -> bool {
    let workspace_key = normalize_path_for_identity(workspace_root);
    let path_key = normalize_path_for_identity(path);
    path_key == workspace_key || path_key.starts_with(&(workspace_key + "/"))
}

fn normalize_path_for_identity(path: &Path) -> String {
    let normalized = normalize_path(path);
    #[cfg(windows)]
    {
        normalized.to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        normalized
    }
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

fn file_name_from_relative_path(relative_path: &str) -> Option<&str> {
    relative_path
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
}

fn metadata_modified_secs(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn current_unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn index_map_size_bytes() -> u64 {
    env_u64("CODEBASE_MCP_INDEX_MAP_SIZE_MB", DEFAULT_INDEX_MAP_SIZE_MB)
        .saturating_mul(1024)
        .saturating_mul(1024)
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn tantivy_enabled() -> bool {
    std::env::var("CODEBASE_MCP_TANTIVY_ENABLED")
        .ok()
        .map(|value| !matches!(value.to_ascii_lowercase().as_str(), "0" | "false" | "off"))
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn workspace_hash_is_stable() {
        let root = PathBuf::from("C:/browser/chromium/src");
        assert_eq!(hash_workspace_root(&root), hash_workspace_root(&root));
    }

    #[test]
    fn children_records_store_names_only() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), "fn main() {}\n").unwrap();
        let key = normalize_path_for_identity(root);
        refresh_metadata_index(root, &key).unwrap();
        let store = open_store(root).unwrap();
        let rtxn = store.env.read_txn().unwrap();
        let children = store.children_db.get(&rtxn, "src").unwrap().unwrap();
        assert_eq!(children, vec!["lib.rs".to_string()]);
    }

    #[test]
    fn lmdb_upsert_load_delete_roundtrip() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("a.rs"), "fn a() {}\n").unwrap();
        let key = normalize_path_for_identity(root);
        let summary = refresh_metadata_index(root, &key).unwrap();
        assert_eq!(summary.files, 1);

        let (_meta, index) = load_existing_index(&key, root).unwrap().unwrap();
        assert_eq!(index.live_entries, 1);

        fs::remove_file(root.join("a.rs")).unwrap();
        let summary = refresh_metadata_index(root, &key).unwrap();
        assert_eq!(summary.files, 0);
        let (_meta, index) = load_existing_index(&key, root).unwrap().unwrap();
        assert_eq!(index.live_entries, 0);
    }

    #[test]
    fn tantivy_schema_can_index_and_delete_path_doc() {
        let dir = tempdir().unwrap();
        let index = open_or_create_tantivy(dir.path()).unwrap();
        let schema = index.schema();
        let fields = tantivy_fields(&schema).unwrap();
        {
            let mut writer: tantivy::IndexWriter<TantivyDocument> = index
                .writer_with_num_threads(1, TANTIVY_WRITER_MEMORY_BYTES)
                .unwrap();
            writer.delete_term(Term::from_field_text(fields.relative_path, "src/lib.rs"));
            writer
                .add_document(doc!(
                    fields.relative_path => "src/lib.rs",
                    fields.file_name => "lib.rs",
                    fields.path_tokens => "src lib rs",
                    fields.content => "fn indexed_symbol() {}",
                    fields.extension => "rs",
                    fields.size => 22u64,
                    fields.modified_at => 1u64
                ))
                .unwrap();
            writer.commit().unwrap();
        }

        {
            let reader: tantivy::IndexReader = index
                .reader_builder()
                .reload_policy(ReloadPolicy::Manual)
                .try_into()
                .unwrap();
            let searcher = reader.searcher();
            let parser = QueryParser::for_index(&index, vec![fields.content]);
            let parsed = parser.parse_query("indexed_symbol").unwrap();
            let top_docs = searcher
                .search(&parsed, &TopDocs::with_limit(10).order_by_score())
                .unwrap();
            assert_eq!(top_docs.len(), 1);
            let doc = searcher.doc::<TantivyDocument>(top_docs[0].1).unwrap();
            assert_eq!(
                doc.get_first(fields.relative_path)
                    .and_then(|value| value.as_str()),
                Some("src/lib.rs")
            );
        }

        {
            let mut writer: tantivy::IndexWriter<TantivyDocument> = index
                .writer_with_num_threads(1, TANTIVY_WRITER_MEMORY_BYTES)
                .unwrap();
            writer.delete_term(Term::from_field_text(fields.relative_path, "src/lib.rs"));
            writer.commit().unwrap();
        }

        {
            let reader: tantivy::IndexReader = index
                .reader_builder()
                .reload_policy(ReloadPolicy::Manual)
                .try_into()
                .unwrap();
            let searcher = reader.searcher();
            let parser = QueryParser::for_index(&index, vec![fields.content]);
            let parsed = parser.parse_query("indexed_symbol").unwrap();
            let top_docs = searcher
                .search(&parsed, &TopDocs::with_limit(10).order_by_score())
                .unwrap();
            assert!(top_docs.is_empty());
        }
    }

    #[test]
    fn content_status_persists_to_lmdb_meta() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("base")).unwrap();
        fs::write(root.join("base/a.rs"), "fn indexed_symbol() {}\n").unwrap();
        let key = normalize_path_for_identity(root);
        refresh_metadata_index(root, &key).unwrap();
        ensure_runtime_loaded(&key, root, "test");

        record_content_status(&key, "ready", Some("base".to_string()), false, 1, 24);

        let (meta, _) = load_existing_index(&key, root).unwrap().unwrap();
        assert_eq!(meta.content_index_status, "ready");
        assert_eq!(meta.content_index_zones, vec!["base".to_string()]);
        assert!(!meta.content_index_partial);
        assert_eq!(meta.indexed_content_files, 1);
        assert_eq!(meta.indexed_content_bytes, 24);
    }

    #[test]
    fn content_policy_respects_extension_size_and_third_party_scope() {
        let policy = ContentPolicy {
            max_file_bytes: 100,
            max_zone_bytes: 1_000,
            max_workspace_bytes: 1_000,
        };
        let mut record = IndexedPathRecord {
            path: PathBuf::from("src/lib.rs"),
            relative_path: "src/lib.rs".to_string(),
            file_name: "lib.rs".to_string(),
            extension_lower: "rs".to_string(),
            is_dir: false,
            size: 50,
            modified_at: 1,
        };

        assert!(content_policy_allows(&record, &policy, false));
        record.size = 101;
        assert!(!content_policy_allows(&record, &policy, false));

        record.size = 50;
        record.extension_lower = "png".to_string();
        assert!(!content_policy_allows(&record, &policy, false));

        record.extension_lower = "cc".to_string();
        record.relative_path = "third_party/lib/a.cc".to_string();
        assert!(!content_policy_allows(&record, &policy, false));
        assert!(content_policy_allows(&record, &policy, true));
    }
}
