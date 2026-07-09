use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};
use tokio::task;

use crate::indexer::{content_status_for_paths, warm_content_index_paths};

const DEFAULT_POLL_MS: u64 = 250;
const MAX_WAIT_MS: u64 = 30_000;

pub fn schema() -> Value {
    json!({
        "name": "warm_content_index",
        "title": "Warm content index",
        "description": "Request Tantivy content-index warming for specific scoped paths. Use before repeated literal text_search in large repositories; pass subsystem directories, then inspect statuses/warming_zones and retry when ready.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "paths": { "type": "array", "items": { "type": "string" }, "description": "Files or directories whose content zones should be warmed. Avoid workspace root; choose the narrowest subsystem path." },
                "wait_ms": { "type": "integer", "description": "Optional time to wait for warming to complete, capped at 30000 ms. Omit or set 0 to schedule asynchronously." },
                "force": { "type": "boolean", "description": "When true, schedule refresh even if the zone already appears ready." }
            },
            "required": ["paths"]
        }
    })
}

pub async fn execute(args: &Value) -> Result<Value> {
    let args_owned = args.clone();
    task::spawn_blocking(move || execute_blocking(args_owned))
        .await
        .context("warm_content_index background task failed to join")?
}

fn execute_blocking(args: Value) -> Result<Value> {
    let paths = parse_paths(&args)?;
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let wait_ms = args
        .get("wait_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
        .min(MAX_WAIT_MS);

    let initial_statuses = warm_content_index_paths(&paths, force);
    let mut final_statuses = initial_statuses.clone();
    if wait_ms > 0 {
        let deadline = Instant::now() + Duration::from_millis(wait_ms);
        while Instant::now() < deadline {
            final_statuses = content_status_for_paths(&paths);
            if final_statuses
                .iter()
                .all(|status| status.ready || !status.warming)
            {
                break;
            }
            thread::sleep(Duration::from_millis(DEFAULT_POLL_MS));
        }
        final_statuses = content_status_for_paths(&paths);
    }

    let requested_zones = zones_from_statuses(&initial_statuses);
    let ready_zones = final_statuses
        .iter()
        .filter(|status| status.ready)
        .filter_map(|status| status.zone.clone())
        .collect::<Vec<_>>();
    let warming_zones = final_statuses
        .iter()
        .filter(|status| status.warming)
        .filter_map(|status| status.zone.clone())
        .collect::<Vec<_>>();

    Ok(json!({
        "requested_zones": requested_zones,
        "ready_zones": ready_zones,
        "warming_zones": warming_zones,
        "statuses": final_statuses,
        "wait_ms": wait_ms,
        "force": force
    }))
}

fn parse_paths(args: &Value) -> Result<Vec<PathBuf>> {
    let paths = args
        .get("paths")
        .and_then(|v| v.as_array())
        .context("Missing paths")?;
    let paths = paths
        .iter()
        .filter_map(|path| path.as_str())
        .map(crate::common::resolve_tool_path)
        .collect::<Vec<_>>();
    if paths.is_empty() {
        return Err(anyhow::anyhow!("paths must contain at least one path"));
    }
    Ok(paths)
}

fn zones_from_statuses(statuses: &[crate::indexer::ContentZoneStatus]) -> Vec<String> {
    let mut zones = statuses
        .iter()
        .filter_map(|status| status.zone.clone())
        .collect::<Vec<_>>();
    zones.sort();
    zones.dedup();
    zones
}
