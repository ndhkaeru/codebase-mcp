use crate::common::unix_timestamp_secs;
use crate::indexer::{get_active_runtime_snapshot, get_runtime_snapshots, stale_index_after_secs};
use crate::tools::text_search::search_telemetry;
use crate::version::SERVER_VERSION;
use anyhow::Result;
use serde_json::{Value, json};

lazy_static::lazy_static! {
    pub static ref START_TIME: u64 = unix_timestamp_secs();
}

pub fn schema() -> Value {
    json!({
        "name": "server_health",
        "title": "Check server health",
        "description": "Check server uptime, workspace roots, and indexing health. Use before broad searches in large repos: path_index powers fuzzy/path tools, content_index powers text_search shortlisting and may only cover listed zones.",
        "inputSchema": {
            "type": "object",
            "properties": {}
        }
    })
}

pub async fn execute(_args: &Value) -> Result<Value> {
    let now = unix_timestamp_secs();
    let uptime_secs = now - *START_TIME;

    let runtimes = get_runtime_snapshots();
    let active_runtime = get_active_runtime_snapshot();
    let primary_runtime = active_runtime.as_ref().or_else(|| runtimes.first());
    let total_indexed_entries: usize = runtimes
        .iter()
        .map(|runtime| runtime.indexed_entries_count)
        .sum();
    let primary_workspace_root = primary_runtime.map(|runtime| runtime.workspace_root.clone());
    let primary_workspace_source = primary_runtime.map(|runtime| runtime.workspace_source.clone());
    let primary_index_file = primary_runtime.map(|runtime| runtime.index_file.clone());
    let primary_loaded_from_disk = primary_runtime.map(|runtime| runtime.loaded_from_disk);
    let primary_scan_complete = primary_runtime.map(|runtime| runtime.scan_complete);
    let primary_loaded_entries = primary_runtime.map(|runtime| runtime.last_loaded_entries);
    let primary_last_persisted_entries =
        primary_runtime.map(|runtime| runtime.last_persisted_entries);
    let primary_last_persisted_at = primary_runtime.and_then(|runtime| runtime.last_persisted_at);
    let primary_last_scan_completed_at =
        primary_runtime.and_then(|runtime| runtime.last_scan_completed_at);
    let primary_last_refresh_requested_at =
        primary_runtime.and_then(|runtime| runtime.last_refresh_requested_at);
    let primary_last_request_source =
        primary_runtime.and_then(|runtime| runtime.last_request_source.clone());
    let primary_last_error = primary_runtime.and_then(|runtime| runtime.last_error.clone());
    let primary_index_kind = primary_runtime.map(|runtime| runtime.index_kind);
    let primary_indexed_entries = primary_runtime.map(|runtime| runtime.indexed_entries_count);
    let primary_metadata_index_backend =
        primary_runtime.map(|runtime| runtime.metadata_index_backend);
    let primary_content_index_backend =
        primary_runtime.map(|runtime| runtime.content_index_backend);
    let primary_metadata_index_status =
        primary_runtime.map(|runtime| runtime.metadata_index_status.clone());
    let primary_content_index_status =
        primary_runtime.map(|runtime| runtime.content_index_status.clone());
    let primary_content_index_zones =
        primary_runtime.map(|runtime| runtime.content_index_zones.clone());
    let primary_content_index_partial =
        primary_runtime.map(|runtime| runtime.content_index_partial);
    let primary_indexed_content_files =
        primary_runtime.map(|runtime| runtime.indexed_content_files);
    let primary_indexed_content_bytes =
        primary_runtime.map(|runtime| runtime.indexed_content_bytes);
    let primary_index_storage_dir =
        primary_runtime.map(|runtime| runtime.index_storage_dir.clone());
    let primary_index_map_size_bytes = primary_runtime.map(|runtime| runtime.index_map_size_bytes);
    let primary_index_size_bytes = primary_runtime.map(|runtime| runtime.index_size_bytes);
    let primary_indexed_files_count = primary_runtime.map(|runtime| runtime.indexed_files_count);
    let primary_indexed_dirs_count = primary_runtime.map(|runtime| runtime.indexed_dirs_count);
    let primary_refresh_running = primary_runtime.map(|runtime| runtime.refresh_running);
    let primary_last_refresh_started_at =
        primary_runtime.and_then(|runtime| runtime.last_refresh_started_at);
    let primary_last_refresh_completed_at =
        primary_runtime.and_then(|runtime| runtime.last_refresh_completed_at);
    let index_status = if runtimes.is_empty() {
        "disabled"
    } else if total_indexed_entries > 0 {
        "active"
    } else {
        "idle"
    };

    let mut response = json!({
        "status": "healthy",
        "uptime_seconds": uptime_secs,
        "index_status": index_status,
        "cached_files_count": total_indexed_entries,
        "indexed_entries_count": total_indexed_entries,
        "index_workspace_count": runtimes.len(),
        "version": SERVER_VERSION,
        "transport": "stdio (JSON-RPC)"
    });

    if let Some(object) = response.as_object_mut() {
        object.insert("index_workspaces".to_string(), json!(runtimes));
        object.insert(
            "active_index_workspace_root".to_string(),
            json!(
                active_runtime
                    .as_ref()
                    .map(|runtime| runtime.workspace_root.clone())
            ),
        );
        object.insert(
            "active_index_workspace_source".to_string(),
            json!(
                active_runtime
                    .as_ref()
                    .map(|runtime| runtime.workspace_source.clone())
            ),
        );
        object.insert(
            "index_workspace_root".to_string(),
            json!(primary_workspace_root),
        );
        object.insert(
            "index_workspace_source".to_string(),
            json!(primary_workspace_source),
        );
        object.insert("index_kind".to_string(), json!(primary_index_kind));
        object.insert("index_file".to_string(), json!(primary_index_file));
        object.insert(
            "index_loaded_from_disk".to_string(),
            json!(primary_loaded_from_disk),
        );
        object.insert(
            "index_scan_complete".to_string(),
            json!(primary_scan_complete),
        );
        object.insert(
            "index_loaded_entries".to_string(),
            json!(primary_loaded_entries),
        );
        object.insert(
            "index_indexed_entries".to_string(),
            json!(primary_indexed_entries),
        );
        object.insert(
            "index_last_persisted_entries".to_string(),
            json!(primary_last_persisted_entries),
        );
        object.insert(
            "index_last_persisted_at".to_string(),
            json!(primary_last_persisted_at),
        );
        object.insert(
            "index_last_scan_completed_at".to_string(),
            json!(primary_last_scan_completed_at),
        );
        object.insert(
            "index_last_refresh_requested_at".to_string(),
            json!(primary_last_refresh_requested_at),
        );
        object.insert(
            "index_last_request_source".to_string(),
            json!(primary_last_request_source),
        );
        object.insert(
            "index_last_error".to_string(),
            json!(primary_last_error.clone()),
        );
        object.insert(
            "index_stale_after_seconds".to_string(),
            json!(stale_index_after_secs()),
        );
        object.insert(
            "metadata_index_backend".to_string(),
            json!(primary_metadata_index_backend),
        );
        object.insert(
            "content_index_backend".to_string(),
            json!(primary_content_index_backend),
        );
        object.insert(
            "metadata_index_status".to_string(),
            json!(primary_metadata_index_status),
        );
        object.insert(
            "content_index_status".to_string(),
            json!(primary_content_index_status),
        );
        object.insert(
            "content_index_zones".to_string(),
            json!(primary_content_index_zones),
        );
        object.insert(
            "content_index_partial".to_string(),
            json!(primary_content_index_partial),
        );
        object.insert(
            "indexed_content_files".to_string(),
            json!(primary_indexed_content_files),
        );
        object.insert(
            "indexed_content_bytes".to_string(),
            json!(primary_indexed_content_bytes),
        );
        object.insert(
            "index_storage_dir".to_string(),
            json!(primary_index_storage_dir),
        );
        object.insert(
            "index_map_size_bytes".to_string(),
            json!(primary_index_map_size_bytes),
        );
        object.insert(
            "index_size_bytes".to_string(),
            json!(primary_index_size_bytes),
        );
        object.insert(
            "indexed_files_count".to_string(),
            json!(primary_indexed_files_count),
        );
        object.insert(
            "indexed_dirs_count".to_string(),
            json!(primary_indexed_dirs_count),
        );
        object.insert(
            "index_refresh_running".to_string(),
            json!(primary_refresh_running),
        );
        object.insert(
            "last_refresh_started_at".to_string(),
            json!(primary_last_refresh_started_at),
        );
        object.insert(
            "last_refresh_completed_at".to_string(),
            json!(primary_last_refresh_completed_at),
        );
        object.insert("last_error".to_string(), json!(primary_last_error));
        object.insert(
            "path_index".to_string(),
            json!({
                "backend": primary_metadata_index_backend,
                "status": primary_metadata_index_status,
                "workspace_root": primary_workspace_root,
                "workspace_source": primary_workspace_source,
                "entries": primary_indexed_entries,
                "files": primary_indexed_files_count,
                "dirs": primary_indexed_dirs_count,
                "loaded_from_disk": primary_loaded_from_disk,
                "scan_complete": primary_scan_complete,
                "last_scan_completed_at": primary_last_scan_completed_at,
                "last_persisted_at": primary_last_persisted_at,
                "stale_after_seconds": stale_index_after_secs()
            }),
        );
        object.insert(
            "content_index".to_string(),
            json!({
                "backend": primary_content_index_backend,
                "status": primary_content_index_status,
                "zones_indexed": primary_content_index_zones,
                "partial": primary_content_index_partial,
                "files": primary_indexed_content_files,
                "bytes": primary_indexed_content_bytes,
                "storage_dir": primary_index_storage_dir,
                "index_size_bytes": primary_index_size_bytes,
                "refresh_running": primary_refresh_running,
                "last_refresh_started_at": primary_last_refresh_started_at,
                "last_refresh_completed_at": primary_last_refresh_completed_at,
                "last_error": primary_last_error
            }),
        );
        object.insert(
            "search_guidance".to_string(),
            json!({
                "text_search_best_practices": [
                    "Prefer narrow paths over workspace-root searches in large repositories.",
                    "Use literal mode when possible so Tantivy can shortlist candidate files before exact grep verification.",
                    "If text_search returns warming_zones, retry the same scoped search after the zone finishes warming.",
                    "Set allow_expensive_fallback=true only when a full grep scan is intentional."
                ],
                "diagnostic_fields": [
                    "search_strategy",
                    "fallback_reason",
                    "content_index_used",
                    "content_index_partial",
                    "content_index_zones",
                    "warming_zones"
                ]
            }),
        );
        object.insert("search_telemetry".to_string(), search_telemetry());
    }

    Ok(response)
}
