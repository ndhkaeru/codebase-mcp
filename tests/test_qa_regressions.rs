use codebase_mcp::tools::{
    batch_tool_call, convert_file_format, file_summary, peek_archive, read_file, read_snippets,
    workspace_stats,
};
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;
use tempfile::tempdir;
use zip::write::SimpleFileOptions;

fn server_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_codebase-mcp").map(PathBuf::from)
        && path.exists()
    {
        return path;
    }

    let exe_name = if cfg!(windows) {
        "codebase-mcp.exe"
    } else {
        "codebase-mcp"
    };
    let current_exe = std::env::current_exe().unwrap();
    let debug_dir = current_exe
        .parent()
        .and_then(|parent| parent.parent())
        .unwrap();
    let candidate = debug_dir.join(exe_name);
    assert!(
        candidate.exists(),
        "could not locate codebase-mcp binary at {}",
        candidate.display()
    );
    candidate
}

fn call_binary_server(current_dir: &Path, initialize_params: Value) -> Value {
    let exe = server_binary();
    let mut command = Command::new(&exe);
    command
        .current_dir(current_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();

    {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(
            stdin,
            "{}",
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":initialize_params})
        )
        .unwrap();
        writeln!(
            stdin,
            "{}",
            json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        )
        .unwrap();
        stdin.flush().unwrap();
        thread::sleep(Duration::from_millis(300));
        writeln!(
            stdin,
            "{}",
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"server_health","arguments":{}}})
        )
        .unwrap();
    }

    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let response = stdout
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with('{'))
        .unwrap();
    let rpc: Value = serde_json::from_str(response).unwrap();
    serde_json::from_str(
        rpc.get("result")
            .and_then(|v| v.get("content"))
            .and_then(|v| v.as_array())
            .and_then(|items| items.first())
            .and_then(|item| item.get("text"))
            .and_then(|v| v.as_str())
            .unwrap(),
    )
    .unwrap()
}

fn call_binary_server_tool_then_health(
    current_dir: &Path,
    initialize_params: Value,
    extra_env: &[(&str, &str)],
    tool_name: &str,
    tool_arguments: Value,
    settle_ms: u64,
) -> (Value, Value) {
    let exe = server_binary();
    let mut command = Command::new(&exe);
    command
        .current_dir(current_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in extra_env {
        command.env(key, value);
    }
    let mut child = command.spawn().unwrap();

    {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(
            stdin,
            "{}",
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":initialize_params})
        )
        .unwrap();
        writeln!(
            stdin,
            "{}",
            json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        )
        .unwrap();
        stdin.flush().unwrap();
        thread::sleep(Duration::from_millis(settle_ms));
        writeln!(
            stdin,
            "{}",
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":tool_name,"arguments":tool_arguments}})
        )
        .unwrap();
        writeln!(
            stdin,
            "{}",
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"server_health","arguments":{}}})
        )
        .unwrap();
    }

    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let responses: Vec<Value> = stdout
        .lines()
        .filter(|line| line.trim_start().starts_with('{'))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();

    let tool_rpc = responses
        .iter()
        .find(|response| response.get("id").and_then(|v| v.as_i64()) == Some(2))
        .cloned()
        .unwrap();
    let health_rpc = responses
        .iter()
        .find(|response| response.get("id").and_then(|v| v.as_i64()) == Some(3))
        .cloned()
        .unwrap();

    (
        decode_tool_rpc_response(&tool_rpc),
        decode_tool_rpc_response(&health_rpc),
    )
}

fn decode_tool_rpc_response(rpc: &Value) -> Value {
    serde_json::from_str(
        rpc.get("result")
            .and_then(|v| v.get("content"))
            .and_then(|v| v.as_array())
            .and_then(|items| items.first())
            .and_then(|item| item.get("text"))
            .and_then(|v| v.as_str())
            .unwrap(),
    )
    .unwrap()
}

fn file_uri_for_test(path: &Path) -> String {
    format!(
        "file:///{}",
        path.to_string_lossy()
            .replace('\\', "/")
            .replace(' ', "%20")
    )
}

#[tokio::test]
async fn test_convert_file_format_writes_real_utf16le() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("convert.txt");
    fs::write(&path, "line-a\nline-b\n").unwrap();

    let result = convert_file_format::execute(&json!({
        "path": path.to_str().unwrap(),
        "target_encoding": "UTF-16LE",
        "target_line_ending": "crlf"
    }))
    .await
    .unwrap();

    assert_eq!(result.get("success").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(
        result.get("target_encoding").and_then(|v| v.as_str()),
        Some("UTF-16LE")
    );

    let bytes = fs::read(&path).unwrap();
    assert!(bytes.starts_with(&[0xFF, 0xFE]));

    let read_back = read_file::execute(&json!({
        "path": path.to_str().unwrap()
    }))
    .await
    .unwrap();
    assert_eq!(
        read_back.get("content").and_then(|v| v.as_str()),
        Some("line-a\r\nline-b\r\n")
    );
}

#[tokio::test]
async fn test_trailing_newline_line_counts_are_not_off_by_one() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("multi.txt");
    fs::write(&path, "first\nsecond\nthird\n").unwrap();

    let read_result = read_file::execute(&json!({
        "path": path.to_str().unwrap()
    }))
    .await
    .unwrap();
    assert_eq!(
        read_result.get("total_lines").and_then(|v| v.as_u64()),
        Some(3)
    );
    assert_eq!(
        read_result.get("returned_lines").and_then(|v| v.as_u64()),
        Some(3)
    );
    assert_eq!(
        read_result.get("content").and_then(|v| v.as_str()),
        Some("first\nsecond\nthird\n")
    );

    let snippets_result = read_snippets::execute(&json!({
        "requests": [{"path": path.to_str().unwrap()}]
    }))
    .await
    .unwrap();
    let snippet = snippets_result
        .get("results")
        .and_then(|v| v.as_array())
        .and_then(|items| items.first())
        .cloned()
        .unwrap();
    assert_eq!(snippet.get("total_lines").and_then(|v| v.as_u64()), Some(3));

    let summary = file_summary::execute(&json!({
        "path": path.to_str().unwrap()
    }))
    .await
    .unwrap();
    assert_eq!(summary.get("lines").and_then(|v| v.as_i64()), Some(3));
}

#[tokio::test]
async fn test_peek_archive_accepts_forward_slash_inner_path() {
    let dir = tempdir().unwrap();
    let archive_path = dir.path().join("sample.zip");
    let file = fs::File::create(&archive_path).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    writer
        .start_file("nested\\inside.txt", SimpleFileOptions::default())
        .unwrap();
    writer.write_all(b"nested archive payload\n").unwrap();
    writer.finish().unwrap();

    let list_result = peek_archive::execute(&json!({
        "archive_path": archive_path.to_str().unwrap()
    }))
    .await
    .unwrap();
    let entries = list_result
        .get("entries")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(
        entries[0].get("name").and_then(|v| v.as_str()),
        Some("nested/inside.txt")
    );

    let extract_result = peek_archive::execute(&json!({
        "archive_path": archive_path.to_str().unwrap(),
        "inner_path": "nested/inside.txt"
    }))
    .await
    .unwrap();
    assert_eq!(
        extract_result.get("content").and_then(|v| v.as_str()),
        Some("nested archive payload\n")
    );
}

#[tokio::test]
async fn test_workspace_stats_reports_total_and_per_language_lines() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("lib.rs"), "fn one() {}\nfn two() {}\n").unwrap();
    fs::write(
        dir.path().join("main.py"),
        "print('a')\nprint('b')\nprint('c')\n",
    )
    .unwrap();

    let result = workspace_stats::execute(&json!({
        "path": dir.path().to_str().unwrap()
    }))
    .await
    .unwrap();

    assert_eq!(result.get("total_lines").and_then(|v| v.as_u64()), Some(5));
    let breakdown = result
        .get("languages_breakdown")
        .and_then(|v| v.as_array())
        .unwrap();
    assert!(breakdown.iter().any(|item| {
        item.get("language").and_then(|v| v.as_str()) == Some("Rust")
            && item.get("lines").and_then(|v| v.as_u64()) == Some(2)
    }));
    assert!(breakdown.iter().any(|item| {
        item.get("language").and_then(|v| v.as_str()) == Some("Python")
            && item.get("lines").and_then(|v| v.as_u64()) == Some(3)
    }));
}

#[tokio::test]
async fn test_batch_tool_call_flattens_inner_tool_payloads() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("sample.txt");
    fs::write(&path, "hello\n").unwrap();

    let result = batch_tool_call::execute(&json!({
        "calls": [{
            "tool": "resolve_path",
            "args": { "path": path.to_str().unwrap() }
        }]
    }))
    .await
    .unwrap();

    let first = result
        .get("results")
        .and_then(|v| v.as_array())
        .and_then(|items| items.first())
        .cloned()
        .unwrap();
    assert_eq!(first.get("status").and_then(|v| v.as_str()), Some("ok"));
    assert!(
        first
            .get("result")
            .and_then(|v| v.get("canonical_path"))
            .is_some()
    );
    assert!(first.get("result").and_then(|v| v.get("content")).is_none());
}

#[test]
fn test_server_health_disables_index_when_process_cwd_is_not_a_workspace() {
    let dir = tempdir().unwrap();
    let result = call_binary_server(dir.path(), json!({}));

    assert_eq!(
        result.get("index_status").and_then(|v| v.as_str()),
        Some("disabled")
    );
    assert!(result.get("index_workspace_source").unwrap().is_null());
    assert!(result.get("index_workspace_root").unwrap().is_null());
}

#[test]
fn test_server_health_can_fallback_to_project_like_process_cwd() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"qa\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    let result = call_binary_server(dir.path(), json!({}));

    let status = result.get("index_status").and_then(|v| v.as_str()).unwrap();
    assert!(matches!(status, "idle" | "active"));
    assert_eq!(
        result
            .get("index_workspace_source")
            .and_then(|v| v.as_str()),
        Some("process_current_dir")
    );
    let root_value = result
        .get("index_workspace_root")
        .and_then(|v| v.as_str())
        .unwrap();
    let actual = Path::new(root_value)
        .canonicalize()
        .unwrap_or_else(|_| Path::new(root_value).to_path_buf());
    assert_eq!(actual, dir.path().canonicalize().unwrap());
}

#[test]
fn test_server_health_uses_client_workspace_root() {
    let current_dir = tempdir().unwrap();
    let workspace_root = tempdir().unwrap();
    let init = json!({
        "workspaceFolders": [
            { "uri": file_uri_for_test(workspace_root.path()) }
        ]
    });
    let result = call_binary_server(current_dir.path(), init);

    assert_eq!(
        result
            .get("index_workspace_source")
            .and_then(|v| v.as_str()),
        Some("client_initialize")
    );
    let root_value = result
        .get("index_workspace_root")
        .and_then(|v| v.as_str())
        .unwrap();
    let expected = workspace_root.path().canonicalize().unwrap();
    let actual = Path::new(root_value)
        .canonicalize()
        .unwrap_or_else(|_| Path::new(root_value).to_path_buf());
    assert_eq!(actual, expected);
}

#[test]
fn test_server_health_reports_multiple_client_workspaces() {
    let current_dir = tempdir().unwrap();
    let workspace_a = tempdir().unwrap();
    let workspace_b = tempdir().unwrap();
    let init = json!({
        "workspaceFolders": [
            { "uri": format!("file:///{}", workspace_a.path().to_string_lossy().replace('\\', "/")) },
            { "uri": format!("file:///{}", workspace_b.path().to_string_lossy().replace('\\', "/")) }
        ]
    });

    let result = call_binary_server(current_dir.path(), init);

    assert_eq!(
        result.get("index_workspace_count").and_then(|v| v.as_u64()),
        Some(2)
    );

    let workspaces = result
        .get("index_workspaces")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(workspaces.len(), 2);
    assert!(workspaces.iter().all(|workspace| {
        workspace.get("workspace_source").and_then(|v| v.as_str()) == Some("client_initialize")
    }));

    let mut roots = workspaces
        .iter()
        .filter_map(|workspace| workspace.get("workspace_root").and_then(|v| v.as_str()))
        .map(|root| {
            Path::new(root)
                .canonicalize()
                .unwrap_or_else(|_| Path::new(root).to_path_buf())
        })
        .collect::<Vec<_>>();
    roots.sort();

    let mut expected = vec![
        workspace_a.path().canonicalize().unwrap(),
        workspace_b.path().canonicalize().unwrap(),
    ];
    expected.sort();

    assert_eq!(roots, expected);
}

#[test]
fn test_tool_call_auto_indexes_workspace_from_request_path() {
    let current_dir = tempdir().unwrap();
    let workspace = tempdir().unwrap();
    fs::write(
        workspace.path().join("Cargo.toml"),
        "[package]\nname = \"qa\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::create_dir_all(workspace.path().join("src")).unwrap();
    let file_path = workspace.path().join("src/lib.rs");
    fs::write(&file_path, "fn sample() {}\n").unwrap();

    let (_tool_result, health) = call_binary_server_tool_then_health(
        current_dir.path(),
        json!({}),
        &[],
        "read_file_range",
        json!({ "path": file_path.to_str().unwrap() }),
        150,
    );

    let active_root = health
        .get("active_index_workspace_root")
        .and_then(|v| v.as_str())
        .unwrap();
    let actual = Path::new(active_root)
        .canonicalize()
        .unwrap_or_else(|_| Path::new(active_root).to_path_buf());
    assert_eq!(actual, workspace.path().canonicalize().unwrap());
    assert_eq!(
        health
            .get("index_last_request_source")
            .and_then(|v| v.as_str()),
        Some("tool_call:read_file_range")
    );
}

#[test]
fn test_tool_call_switches_active_workspace_context() {
    let current_dir = tempdir().unwrap();
    let workspace_a = tempdir().unwrap();
    let workspace_b = tempdir().unwrap();

    fs::write(
        workspace_a.path().join("Cargo.toml"),
        "[package]\nname = \"a\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(
        workspace_b.path().join("Cargo.toml"),
        "[package]\nname = \"b\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::create_dir_all(workspace_b.path().join("src")).unwrap();
    let file_b = workspace_b.path().join("src/lib.rs");
    fs::write(&file_b, "fn current_workspace() {}\n").unwrap();

    let init = json!({
        "workspaceFolders": [
            { "uri": format!("file:///{}", workspace_a.path().to_string_lossy().replace('\\', "/")) }
        ]
    });
    let (_tool_result, health) = call_binary_server_tool_then_health(
        current_dir.path(),
        init,
        &[],
        "read_file_range",
        json!({ "path": file_b.to_str().unwrap() }),
        150,
    );

    let active_root = health
        .get("active_index_workspace_root")
        .and_then(|v| v.as_str())
        .unwrap();
    let actual = Path::new(active_root)
        .canonicalize()
        .unwrap_or_else(|_| Path::new(active_root).to_path_buf());
    assert_eq!(actual, workspace_b.path().canonicalize().unwrap());
    assert_eq!(
        health.get("index_workspace_count").and_then(|v| v.as_u64()),
        Some(2)
    );
}

#[test]
fn test_tool_calls_resolve_relative_paths_against_client_workspace_root_only() {
    let current_dir = tempdir().unwrap();
    let workspace = tempdir().unwrap();
    fs::write(
        workspace.path().join("Cargo.toml"),
        "[package]\nname = \"pathing\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::create_dir_all(workspace.path().join("src")).unwrap();
    fs::write(workspace.path().join("src/lib.rs"), "fn sample() {}\n").unwrap();
    let init = json!({
        "workspaceFolders": [
            { "uri": file_uri_for_test(workspace.path()) }
        ]
    });

    let (read_result, _health) = call_binary_server_tool_then_health(
        current_dir.path(),
        init.clone(),
        &[],
        "read_file_range",
        json!({ "path": "src/lib.rs" }),
        150,
    );
    assert_eq!(
        read_result.get("content").and_then(|v| v.as_str()),
        Some("fn sample() {}\n")
    );

    let (resolve_result, _health) = call_binary_server_tool_then_health(
        current_dir.path(),
        init,
        &[],
        "resolve_path",
        json!({ "path": "src/lib.rs" }),
        150,
    );
    assert_eq!(
        resolve_result
            .get("resolution_basis")
            .and_then(|v| v.as_str()),
        Some("active_index_workspace")
    );
    assert_eq!(
        resolve_result
            .get("repo_root")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .and_then(|path| path.canonicalize().ok()),
        Some(workspace.path().canonicalize().unwrap())
    );
}

#[test]
fn test_tool_calls_resolve_relative_paths_against_client_workspace_root() {
    let current_dir = tempdir().unwrap();
    let workspace_parent = tempdir().unwrap();
    let workspace = workspace_parent.path().join("space root");
    fs::create_dir_all(workspace.join("src")).unwrap();
    fs::write(
        workspace.join("Cargo.toml"),
        "[package]\nname = \"client-pathing\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(workspace.join("src/lib.rs"), "fn client_workspace() {}\n").unwrap();

    let init = json!({
        "workspaceFolders": [
            { "uri": file_uri_for_test(&workspace) }
        ]
    });

    let (read_result, _health) = call_binary_server_tool_then_health(
        current_dir.path(),
        init.clone(),
        &[],
        "read_file_range",
        json!({ "path": "src/lib.rs" }),
        150,
    );
    assert_eq!(
        read_result.get("content").and_then(|v| v.as_str()),
        Some("fn client_workspace() {}\n")
    );

    let (resolve_result, _health) = call_binary_server_tool_then_health(
        current_dir.path(),
        init,
        &[],
        "resolve_path",
        json!({ "path": "src/lib.rs" }),
        150,
    );
    assert_eq!(
        resolve_result
            .get("resolution_basis")
            .and_then(|v| v.as_str()),
        Some("active_index_workspace")
    );
    assert_eq!(
        resolve_result
            .get("repo_root")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .and_then(|path| path.canonicalize().ok()),
        Some(workspace.canonicalize().unwrap())
    );
}
