use codebase_mcp::tools::{
    self, fuzzy_find, project_map, read_file, read_snippets, read_symbol_body, text_search,
};
use serde_json::json;
use std::fs;
use tempfile::tempdir;

#[tokio::test]
async fn test_git_related_tools_are_not_exposed_or_dispatchable() {
    let removed_tools = [
        "git_status",
        "git_diff",
        "git_log",
        "git_blame",
        "get_semantic_diff",
        "markdown_outline",
        "read_markdown_section",
        "replace_markdown_section",
        "find_json_paths",
        "extract_json_schema",
        "sqlite_inspect",
        "diff_two_snippets",
        "history_status",
        "undo_last_change",
        "redo_last_change",
    ];
    let listed_tools = tools::list_tools();

    for removed_tool in removed_tools {
        assert!(
            !listed_tools
                .iter()
                .any(|tool| tool.get("name").and_then(|v| v.as_str()) == Some(removed_tool)),
            "{removed_tool} should not be exposed"
        );

        let result = tools::call_tool(json!({
            "name": removed_tool,
            "arguments": {}
        }))
        .await;
        assert!(result.is_err(), "{removed_tool} should not dispatch");
    }
}

#[tokio::test]
async fn test_content_index_tools_are_exposed_and_report_path_status() {
    let listed_tools = tools::list_tools();
    assert!(
        listed_tools
            .iter()
            .any(|tool| tool.get("name").and_then(|v| v.as_str()) == Some("content_index_status"))
    );
    assert!(
        listed_tools
            .iter()
            .any(|tool| tool.get("name").and_then(|v| v.as_str()) == Some("warm_content_index"))
    );

    let dir = tempdir().unwrap();
    let status_result = tools::call_tool(json!({
        "name": "content_index_status",
        "arguments": { "paths": [dir.path().to_str().unwrap()] }
    }))
    .await
    .unwrap();
    let status_text = status_result
        .get("content")
        .and_then(|v| v.as_array())
        .and_then(|items| items.first())
        .and_then(|item| item.get("text"))
        .and_then(|v| v.as_str())
        .unwrap();
    let status_json: serde_json::Value = serde_json::from_str(status_text).unwrap();
    assert_eq!(status_json.get("total").and_then(|v| v.as_u64()), Some(1));

    let warm_result = tools::call_tool(json!({
        "name": "warm_content_index",
        "arguments": { "paths": [dir.path().to_str().unwrap()], "wait_ms": 0 }
    }))
    .await
    .unwrap();
    assert!(warm_result.get("content").is_some());
}

#[tokio::test]
async fn test_text_search_supports_explicit_modes_and_preserves_raw_line_text() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("sample.rs");
    fs::write(&path, "    TODO\nFIXME\n").unwrap();

    let literal_res = text_search::execute(&json!({
        "query": "TODO|FIXME",
        "paths": [path.to_str().unwrap()],
        "explain_no_results": true
    }))
    .await
    .unwrap();

    assert_eq!(
        literal_res.get("mode").and_then(|v| v.as_str()),
        Some("literal")
    );
    assert_eq!(
        literal_res.get("total_returned").and_then(|v| v.as_u64()),
        Some(0)
    );
    assert_eq!(
        literal_res
            .get("no_results")
            .and_then(|v| v.get("reason"))
            .and_then(|v| v.as_str()),
        Some("no_match_found")
    );

    let regex_res = text_search::execute(&json!({
        "query": "TODO|FIXME",
        "paths": [path.to_str().unwrap()],
        "mode": "regex"
    }))
    .await
    .unwrap();

    assert_eq!(
        regex_res.get("total_returned").and_then(|v| v.as_u64()),
        Some(2)
    );

    let raw_line_res = text_search::execute(&json!({
        "query": "TODO",
        "paths": [path.to_str().unwrap()]
    }))
    .await
    .unwrap();

    let first_match = raw_line_res
        .get("matches")
        .and_then(|v| v.as_array())
        .and_then(|items| items.first())
        .cloned()
        .unwrap();
    assert_eq!(
        first_match.get("snippet").and_then(|v| v.as_str()),
        Some("TODO")
    );
    assert_eq!(
        first_match.get("line_text").and_then(|v| v.as_str()),
        Some("    TODO")
    );
}

#[tokio::test]
async fn test_text_search_applies_excludes_to_explicit_file_paths() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("target.rs");
    fs::write(&path, "needle\n").unwrap();

    let res = text_search::execute(&json!({
        "query": "needle",
        "paths": [path.to_str().unwrap()],
        "excludes": ["target.rs"],
        "explain_no_results": true
    }))
    .await
    .unwrap();

    assert_eq!(res.get("total_returned").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(
        res.get("no_results")
            .and_then(|v| v.get("reason"))
            .and_then(|v| v.as_str()),
        Some("no_candidate_files")
    );
}

#[tokio::test]
async fn test_text_search_reports_fallback_diagnostics_for_regex() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("sample.rs");
    fs::write(&path, "TODO\n").unwrap();

    let res = text_search::execute(&json!({
        "query": "TODO|FIXME",
        "paths": [path.to_str().unwrap()],
        "mode": "regex",
        "explain_no_results": true
    }))
    .await
    .unwrap();

    assert_eq!(
        res.get("search_strategy").and_then(|v| v.as_str()),
        Some("grep_fallback")
    );
    assert!(
        res.get("fallback_reason")
            .and_then(|v| v.as_array())
            .is_some_and(|items| items.iter().any(|item| item == "regex_mode_requires_grep"))
    );
    assert_eq!(
        res.get("verification_engine").and_then(|v| v.as_str()),
        Some("grep_searcher")
    );
}

#[tokio::test]
async fn test_text_search_applies_default_fallback_excludes_but_allows_direct_scope() {
    let dir = tempdir().unwrap();
    let vendor_dir = dir.path().join("third_party");
    fs::create_dir(&vendor_dir).unwrap();
    let vendor_file = vendor_dir.join("lib.rs");
    fs::write(&vendor_file, "needle\n").unwrap();

    let root_res = text_search::execute(&json!({
        "query": "needle",
        "paths": [dir.path().to_str().unwrap()],
        "explain_no_results": true
    }))
    .await
    .unwrap();

    assert_eq!(
        root_res
            .get("default_excludes_applied")
            .and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        root_res.get("total_returned").and_then(|v| v.as_u64()),
        Some(0)
    );

    let direct_res = text_search::execute(&json!({
        "query": "needle",
        "paths": [vendor_dir.to_str().unwrap()]
    }))
    .await
    .unwrap();

    assert_eq!(
        direct_res
            .get("default_excludes_applied")
            .and_then(|v| v.as_bool()),
        Some(false)
    );
    assert_eq!(
        direct_res.get("total_returned").and_then(|v| v.as_u64()),
        Some(1)
    );
}

#[tokio::test]
async fn test_read_file_range_and_snippets_report_truncation_metadata() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("lines.txt");
    fs::write(&path, "alpha\nbeta\ngamma\ndelta\nepsilon").unwrap();

    let read_res = read_file::execute(&json!({
        "path": path.to_str().unwrap(),
        "start_line": 1,
        "end_line": 5,
        "max_lines": 2,
        "include_line_numbers": true
    }))
    .await
    .unwrap();

    assert_eq!(
        read_res.get("content").and_then(|v| v.as_str()),
        Some("1: alpha\n2: beta")
    );
    assert_eq!(
        read_res.get("truncated").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        read_res.get("returned_lines").and_then(|v| v.as_u64()),
        Some(2)
    );
    assert_eq!(
        read_res.get("omitted_lines").and_then(|v| v.as_u64()),
        Some(3)
    );
    assert_eq!(
        read_res.get("next_start_line").and_then(|v| v.as_u64()),
        Some(3)
    );
    assert_eq!(read_res.get("end_line").and_then(|v| v.as_u64()), Some(2));

    let snippets_res = read_snippets::execute(&json!({
        "requests": [{
            "path": path.to_str().unwrap(),
            "start_line": 1,
            "end_line": 5,
            "max_lines": 2
        }]
    }))
    .await
    .unwrap();

    let first_result = snippets_res
        .get("results")
        .and_then(|v| v.as_array())
        .and_then(|items| items.first())
        .cloned()
        .unwrap();
    assert_eq!(
        first_result.get("truncated").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        first_result.get("next_start_line").and_then(|v| v.as_u64()),
        Some(3)
    );
}

#[tokio::test]
async fn test_read_snippets_reports_batch_continuations_and_skipped_requests() {
    let dir = tempdir().unwrap();
    let first = dir.path().join("first.txt");
    let second = dir.path().join("second.txt");
    fs::write(&first, "alpha\nbeta\ngamma\ndelta\n").unwrap();
    fs::write(&second, "epsilon\nzeta\neta\ntheta\n").unwrap();

    let result = read_snippets::execute(&json!({
        "requests": [
            {
                "path": first.to_str().unwrap(),
                "start_line": 1,
                "end_line": 4
            },
            {
                "path": second.to_str().unwrap(),
                "start_line": 1,
                "end_line": 4
            }
        ],
        "max_total_bytes": 12
    }))
    .await
    .unwrap();

    assert_eq!(result.get("has_more").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(
        result
            .get("batch_limits")
            .and_then(|v| v.get("max_total_bytes"))
            .and_then(|v| v.as_u64()),
        Some(12)
    );

    let results = result.get("results").and_then(|v| v.as_array()).unwrap();
    assert_eq!(results.len(), 2);

    assert_eq!(
        results[0].get("status").and_then(|v| v.as_str()),
        Some("success")
    );
    assert_eq!(
        results[0].get("truncated").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        results[0]
            .get("continuation")
            .and_then(|v| v.get("next_start_line"))
            .and_then(|v| v.as_u64()),
        Some(3)
    );

    assert_eq!(
        results[1].get("status").and_then(|v| v.as_str()),
        Some("skipped")
    );
    assert_eq!(
        results[1].get("reason").and_then(|v| v.as_str()),
        Some("batch_total_byte_limit_reached")
    );

    let continuations = result
        .get("continuations")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(continuations.len(), 2);
}

#[tokio::test]
async fn test_read_symbol_body_prefers_ast_and_supports_body_only_mode() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("lib.rs");
    fs::write(
        &path,
        "fn sample() {\n    println!(\"hi\");\n    call();\n}\n\nfn other() {}\n",
    )
    .unwrap();

    let full_res = read_symbol_body::execute(&json!({
        "symbol": "sample",
        "paths": [path.to_str().unwrap()]
    }))
    .await
    .unwrap();

    assert_eq!(
        full_res.get("match_source").and_then(|v| v.as_str()),
        Some("ast")
    );
    assert_eq!(
        full_res.get("confidence").and_then(|v| v.as_str()),
        Some("high")
    );
    assert!(
        full_res
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap()
            .contains("fn sample")
    );

    let body_only_res = read_symbol_body::execute(&json!({
        "symbol": "sample",
        "paths": [path.to_str().unwrap()],
        "include_signature": false
    }))
    .await
    .unwrap();

    let body_only_content = body_only_res
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap();
    assert!(body_only_content.contains("println!"));
    assert!(!body_only_content.contains("fn sample"));
}

#[tokio::test]
async fn test_project_map_and_fuzzy_find_return_polished_fields() {
    let dir = tempdir().unwrap();
    let nested_dir = dir.path().join("src");
    fs::create_dir_all(&nested_dir).unwrap();
    fs::write(nested_dir.join("main.rs"), "fn main() {}\n").unwrap();

    let project_map_res = project_map::execute(&json!({
        "path": dir.path().to_str().unwrap(),
        "show_sizes": false
    }))
    .await
    .unwrap();
    let root_key = project_map_res
        .get("root")
        .and_then(|v| v.as_str())
        .unwrap();

    let root_entries = project_map_res
        .get("tree_representation")
        .and_then(|tree| tree.get(root_key))
        .and_then(|v| v.as_array())
        .unwrap();
    let src_entry = root_entries
        .iter()
        .find(|entry| entry.get("name").and_then(|v| v.as_str()) == Some("src"))
        .unwrap();
    assert!(src_entry.get("size_b").unwrap().is_null());

    let fuzzy_find_res = fuzzy_find::execute(&json!({
        "pattern": "main",
        "paths": [dir.path().to_str().unwrap()],
        "extensions": ["rs"]
    }))
    .await
    .unwrap();

    let first_match = fuzzy_find_res
        .get("results")
        .and_then(|v| v.as_array())
        .and_then(|items| items.first())
        .cloned()
        .unwrap();
    assert_eq!(
        first_match.get("relative_path").and_then(|v| v.as_str()),
        Some("src/main.rs")
    );
    assert!(first_match.get("score").and_then(|v| v.as_i64()).is_some());
}

#[tokio::test]
async fn test_project_map_reports_output_child_truncation() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), "a\n").unwrap();
    fs::write(dir.path().join("b.txt"), "b\n").unwrap();

    let result = project_map::execute(&json!({
        "path": dir.path().to_str().unwrap(),
        "max_children_per_dir": 1
    }))
    .await
    .unwrap();

    assert_eq!(
        result.get("limit_reached").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        result.get("limit_reason").and_then(|v| v.as_str()),
        Some("max_children_per_dir")
    );
    assert_eq!(
        result
            .get("truncated_directory_count")
            .and_then(|v| v.as_u64()),
        Some(1)
    );
}
