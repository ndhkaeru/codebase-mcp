use codebase_mcp::tools::{
    self, compare_directories, fuzzy_find, project_map, read_file, read_snippets, read_symbol_body,
    text_search,
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

#[tokio::test]
async fn test_compare_directories_reports_added_deleted_modified_and_ignored() {
    let dir = tempdir().unwrap();
    let left = dir.path().join("left");
    let right = dir.path().join("right");
    fs::create_dir_all(left.join("src")).unwrap();
    fs::create_dir_all(right.join("src")).unwrap();
    fs::create_dir_all(right.join("target")).unwrap();

    fs::write(
        left.join("src/common.rs"),
        "fn shared() {\n    println!(\"old\");\n}\n",
    )
    .unwrap();
    fs::write(
        right.join("src/common.rs"),
        "fn shared() {\n    println!(\"new\");\n}\n",
    )
    .unwrap();
    fs::write(left.join("src/removed.rs"), "fn removed() {}\n").unwrap();
    fs::write(right.join("src/added.rs"), "fn added() {}\n").unwrap();
    fs::write(right.join("target/generated.rs"), "ignored\n").unwrap();

    let res = compare_directories::execute(&json!({
        "left_path": left.to_str().unwrap(),
        "right_path": right.to_str().unwrap()
    }))
    .await
    .unwrap();

    assert_eq!(
        res.pointer("/summary/added_files").and_then(|v| v.as_u64()),
        Some(1)
    );
    assert_eq!(
        res.pointer("/summary/deleted_files")
            .and_then(|v| v.as_u64()),
        Some(1)
    );
    assert_eq!(
        res.pointer("/summary/modified_text_files")
            .and_then(|v| v.as_u64()),
        Some(1)
    );

    let added = res.get("added_files").and_then(|v| v.as_array()).unwrap();
    assert!(
        added
            .iter()
            .any(|value| value.as_str() == Some("src/added.rs"))
    );
    assert!(
        !added
            .iter()
            .any(|value| value.as_str() == Some("target/generated.rs"))
    );

    let modified = res
        .get("modified_files")
        .and_then(|v| v.as_array())
        .and_then(|items| items.first())
        .unwrap();
    assert_eq!(
        modified.get("path").and_then(|v| v.as_str()),
        Some("src/common.rs")
    );
    assert!(
        modified
            .get("unified_diff")
            .and_then(|v| v.as_str())
            .unwrap()
            .contains("println!(\"new\")")
    );
}

#[tokio::test]
async fn test_compare_directories_is_exposed_and_dispatchable() {
    let listed_tools = tools::list_tools();
    assert!(listed_tools.iter().any(|tool| {
        tool.get("name").and_then(|value| value.as_str()) == Some("compare_directories")
    }));

    let dir = tempdir().unwrap();
    let left = dir.path().join("left");
    let right = dir.path().join("right");
    fs::create_dir_all(&left).unwrap();
    fs::create_dir_all(&right).unwrap();

    let result = tools::call_tool(json!({
        "name": "compare_directories",
        "arguments": {
            "left_path": left.to_str().unwrap(),
            "right_path": right.to_str().unwrap()
        }
    }))
    .await
    .unwrap();
    assert!(result.get("content").is_some());
}

#[tokio::test]
async fn test_compare_directories_summary_truncation_binary_large_and_rename() {
    let dir = tempdir().unwrap();
    let left = dir.path().join("left-rich");
    let right = dir.path().join("right-rich");
    fs::create_dir_all(left.join("src/auth")).unwrap();
    fs::create_dir_all(right.join("src/auth")).unwrap();
    fs::create_dir_all(left.join("docs")).unwrap();
    fs::create_dir_all(right.join("docs")).unwrap();

    fs::write(left.join("docs/old.md"), "same rename content\n").unwrap();
    fs::write(right.join("docs/new.md"), "same rename content\n").unwrap();
    fs::write(
        left.join("src/auth/login.rs"),
        "fn login() {\n    old();\n}\n",
    )
    .unwrap();
    fs::write(
        right.join("src/auth/login.rs"),
        "fn login() {\n    new();\n}\n",
    )
    .unwrap();
    fs::write(left.join("src/blob.bin"), [0x01u8, 0x00, 0x02]).unwrap();
    fs::write(right.join("src/blob.bin"), [0x01u8, 0x00, 0x03]).unwrap();
    fs::write(left.join("src/big.txt"), "a".repeat(64)).unwrap();
    fs::write(right.join("src/big.txt"), "b".repeat(64)).unwrap();

    let res = compare_directories::execute(&json!({
        "left_path": left.to_str().unwrap(),
        "right_path": right.to_str().unwrap(),
        "max_file_size": 32,
        "max_diff_bytes": 12,
        "summary_only": true
    }))
    .await
    .unwrap();

    assert_eq!(
        res.pointer("/summary/renamed_files")
            .and_then(|v| v.as_u64()),
        Some(1)
    );
    assert_eq!(
        res.pointer("/summary/modified_binary_files")
            .and_then(|v| v.as_u64()),
        Some(1)
    );
    assert_eq!(
        res.pointer("/summary/skipped_files")
            .and_then(|v| v.as_u64()),
        Some(1)
    );
    assert_eq!(
        res.pointer("/summary/diff_bytes_returned")
            .and_then(|v| v.as_u64()),
        Some(0)
    );

    let modified = res
        .get("modified_files")
        .and_then(|v| v.as_array())
        .and_then(|items| items.first())
        .unwrap();
    assert!(modified.get("unified_diff").is_none());
    assert_eq!(
        modified.get("risk_category").and_then(|v| v.as_str()),
        Some("auth/security")
    );
    assert!(
        res.get("top_changed_directories")
            .and_then(|v| v.as_array())
            .unwrap()
            .iter()
            .any(|item| item.get("name").and_then(|v| v.as_str()) == Some("src"))
    );
    assert!(
        res.get("extensions_summary")
            .and_then(|v| v.as_array())
            .unwrap()
            .iter()
            .any(|item| item.get("name").and_then(|v| v.as_str()) == Some(".rs"))
    );
    assert!(
        res.get("risk_hints")
            .and_then(|v| v.as_array())
            .unwrap()
            .iter()
            .any(|item| item.get("category").and_then(|v| v.as_str()) == Some("auth/security"))
    );
}

#[tokio::test]
async fn test_compare_directories_markdown_output() {
    let dir = tempdir().unwrap();
    let left = dir.path().join("left-md");
    let right = dir.path().join("right-md");
    fs::create_dir_all(&left).unwrap();
    fs::create_dir_all(&right).unwrap();
    fs::write(right.join("added.rs"), "fn added() {}\n").unwrap();

    let result = tools::call_tool(json!({
        "name": "compare_directories",
        "arguments": {
            "left_path": left.to_str().unwrap(),
            "right_path": right.to_str().unwrap(),
            "output_format": "markdown"
        }
    }))
    .await
    .unwrap();

    let text = result
        .get("content")
        .and_then(|v| v.as_array())
        .and_then(|items| items.first())
        .and_then(|item| item.get("text"))
        .and_then(|v| v.as_str())
        .unwrap();
    assert!(text.contains("# Directory Compare Report"));
    assert!(text.contains("`added.rs`"));
}

#[tokio::test]
async fn test_compare_directories_edge_options() {
    let dir = tempdir().unwrap();
    let left = dir.path().join("left-options");
    let right = dir.path().join("right-options");
    fs::create_dir_all(left.join("src")).unwrap();
    fs::create_dir_all(right.join("src")).unwrap();
    fs::create_dir_all(left.join("docs")).unwrap();
    fs::create_dir_all(right.join("docs")).unwrap();

    fs::write(left.join("docs/a.md"), "same\n").unwrap();
    fs::write(right.join("docs/b.md"), "same\n").unwrap();
    fs::write(left.join("src/main.rs"), "fn main() {\n    old();\n}\n").unwrap();
    fs::write(right.join("src/main.rs"), "fn main() {\n    new();\n}\n").unwrap();
    fs::write(right.join("src/ignored.rs"), "fn ignored() {}\n").unwrap();

    let no_rename = compare_directories::execute(&json!({
        "left_path": left.to_str().unwrap(),
        "right_path": right.to_str().unwrap(),
        "detect_renames": false,
        "include_content_diff": false,
        "excludes": ["src/ignored.rs"]
    }))
    .await
    .unwrap();
    assert_eq!(
        no_rename
            .pointer("/summary/renamed_files")
            .and_then(|v| v.as_u64()),
        Some(0)
    );
    assert_eq!(
        no_rename
            .pointer("/summary/added_files")
            .and_then(|v| v.as_u64()),
        Some(1)
    );
    assert_eq!(
        no_rename
            .pointer("/summary/deleted_files")
            .and_then(|v| v.as_u64()),
        Some(1)
    );
    let modified = no_rename
        .get("modified_files")
        .and_then(|v| v.as_array())
        .and_then(|items| items.first())
        .unwrap();
    assert!(modified.get("unified_diff").is_none());
    assert_eq!(
        modified
            .get("affected_symbols")
            .and_then(|v| v.as_array())
            .and_then(|items| items.first())
            .and_then(|v| v.as_str()),
        Some("main")
    );

    let include_only_docs = compare_directories::execute(&json!({
        "left_path": left.to_str().unwrap(),
        "right_path": right.to_str().unwrap(),
        "includes": ["docs/**"]
    }))
    .await
    .unwrap();
    assert_eq!(
        include_only_docs
            .pointer("/summary/renamed_files")
            .and_then(|v| v.as_u64()),
        Some(1)
    );
    assert_eq!(
        include_only_docs
            .pointer("/summary/modified_text_files")
            .and_then(|v| v.as_u64()),
        Some(0)
    );

    let invalid = compare_directories::execute(&json!({
        "left_path": left.to_str().unwrap(),
        "right_path": right.to_str().unwrap(),
        "output_format": "html"
    }))
    .await;
    assert!(
        invalid
            .unwrap_err()
            .to_string()
            .contains("Unsupported output_format")
    );
}

#[tokio::test]
async fn test_compare_directories_truncates_diff_and_detects_fuzzy_rename() {
    let dir = tempdir().unwrap();
    let left = dir.path().join("left-fuzzy");
    let right = dir.path().join("right-fuzzy");
    fs::create_dir_all(left.join("src")).unwrap();
    fs::create_dir_all(right.join("src")).unwrap();

    fs::write(
        left.join("src/old_name.rs"),
        "fn same() {\n    alpha();\n    beta();\n}\n",
    )
    .unwrap();
    fs::write(
        right.join("src/new_name.rs"),
        "fn same() {\n    alpha();\n    gamma();\n}\n",
    )
    .unwrap();
    fs::write(
        left.join("src/changed.rs"),
        "fn changed() {\n    old_line_one();\n    old_line_two();\n}\n",
    )
    .unwrap();
    fs::write(
        right.join("src/changed.rs"),
        "fn changed() {\n    new_line_one();\n    new_line_two();\n}\n",
    )
    .unwrap();

    let res = compare_directories::execute(&json!({
        "left_path": left.to_str().unwrap(),
        "right_path": right.to_str().unwrap(),
        "rename_similarity_threshold": 0.5,
        "max_diff_bytes": 24
    }))
    .await
    .unwrap();

    assert_eq!(
        res.pointer("/summary/renamed_files")
            .and_then(|v| v.as_u64()),
        Some(1)
    );
    let rename = res
        .get("renamed_files")
        .and_then(|v| v.as_array())
        .and_then(|items| items.first())
        .unwrap();
    assert_eq!(
        rename
            .get("modified_after_rename")
            .and_then(|v| v.as_bool()),
        Some(true)
    );

    let modified = res
        .get("modified_files")
        .and_then(|v| v.as_array())
        .and_then(|items| items.first())
        .unwrap();
    assert_eq!(
        modified.get("diff_truncated").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert!(
        modified
            .get("affected_symbols")
            .and_then(|v| v.as_array())
            .unwrap()
            .iter()
            .any(|value| value.as_str() == Some("changed"))
    );
}
