use codeloupe_mcp::tools::{
    compare_symbols, create_directory, get_call_graph, get_symbols, list_exports, list_imports,
    read_symbol_body,
};
use serde_json::json;
use std::fs;
use tempfile::tempdir;

#[tokio::test]
async fn test_create_directory_supports_nested_creation_and_existing_behavior() {
    let dir = tempdir().unwrap();
    let target = dir.path().join("nested").join("leaf");

    let create_res = create_directory::execute(&json!({
        "path": target.to_str().unwrap()
    }))
    .await
    .unwrap();
    assert_eq!(
        create_res.get("success").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        create_res.get("created").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert!(target.exists());

    let existing_res = create_directory::execute(&json!({
        "path": target.to_str().unwrap()
    }))
    .await
    .unwrap();
    assert_eq!(
        existing_res.get("success").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        existing_res.get("created").and_then(|v| v.as_bool()),
        Some(false)
    );

    let fail_existing_res = create_directory::execute(&json!({
        "path": target.to_str().unwrap(),
        "allow_existing": false
    }))
    .await
    .unwrap();
    assert_eq!(
        fail_existing_res.get("success").and_then(|v| v.as_bool()),
        Some(false)
    );
    assert_eq!(
        fail_existing_res.get("error_code").and_then(|v| v.as_str()),
        Some("already_exists")
    );
}

#[tokio::test]
async fn test_list_imports_and_exports_cover_typescript_and_rust() {
    let dir = tempdir().unwrap();
    let tsx_path = dir.path().join("widget.tsx");
    fs::write(
        &tsx_path,
        "import React from \"react\";\nimport type { Foo } from \"./types\";\nexport { Foo } from \"./types\";\nexport const answer = 42;\nexport default function App() { return <div />; }\n",
    )
    .unwrap();

    let rust_path = dir.path().join("mod.rs");
    fs::write(
        &rust_path,
        "use crate::inner::Thing;\npub use crate::inner::PublicThing;\npub struct Model;\npub fn run() {}\n",
    )
    .unwrap();

    let ts_imports = list_imports::execute(&json!({ "path": tsx_path.to_str().unwrap() }))
        .await
        .unwrap();
    assert_eq!(
        ts_imports.get("total_imports").and_then(|v| v.as_u64()),
        Some(2)
    );

    let first_ts_import = ts_imports
        .get("imports")
        .and_then(|v| v.as_array())
        .and_then(|items| items.first())
        .cloned()
        .unwrap();
    assert_eq!(
        first_ts_import.get("source").and_then(|v| v.as_str()),
        Some("react")
    );

    let ts_exports = list_exports::execute(&json!({ "path": tsx_path.to_str().unwrap() }))
        .await
        .unwrap();
    assert_eq!(
        ts_exports.get("total_exports").and_then(|v| v.as_u64()),
        Some(3)
    );
    assert!(
        ts_exports
            .get("exports")
            .and_then(|v| v.as_array())
            .unwrap()
            .iter()
            .any(|item| item.get("kind").and_then(|v| v.as_str()) == Some("reexport"))
    );

    let rust_imports = list_imports::execute(&json!({ "path": rust_path.to_str().unwrap() }))
        .await
        .unwrap();
    assert_eq!(
        rust_imports.get("total_imports").and_then(|v| v.as_u64()),
        Some(2)
    );

    let rust_exports = list_exports::execute(&json!({ "path": rust_path.to_str().unwrap() }))
        .await
        .unwrap();
    assert!(
        rust_exports
            .get("exports")
            .and_then(|v| v.as_array())
            .unwrap()
            .iter()
            .any(|item| item.get("name").and_then(|v| v.as_str()) == Some("run"))
    );
}

#[tokio::test]
async fn test_list_exports_treats_swift_public_set_properties_as_public_api() {
    let dir = tempdir().unwrap();
    let swift_path = dir.path().join("Counter.swift");
    fs::write(
        &swift_path,
        "public struct Counter {\n    public(set) var count: Int\n}\n",
    )
    .unwrap();

    let swift_exports = list_exports::execute(&json!({ "path": swift_path.to_str().unwrap() }))
        .await
        .unwrap();

    assert!(
        swift_exports
            .get("exports")
            .and_then(|value| value.as_array())
            .unwrap()
            .iter()
            .any(|item| item.get("name").and_then(|value| value.as_str()) == Some("count"))
    );
}

#[tokio::test]
async fn test_compare_symbols_returns_unified_diff() {
    let dir = tempdir().unwrap();
    let left_path = dir.path().join("left.rs");
    let right_path = dir.path().join("right.rs");
    fs::write(
        &left_path,
        "fn provider() {\n    step_one();\n    step_two();\n}\n",
    )
    .unwrap();
    fs::write(
        &right_path,
        "fn provider() {\n    step_one();\n    step_three();\n}\n",
    )
    .unwrap();

    let compare_res = compare_symbols::execute(&json!({
        "left": {
            "symbol": "provider",
            "paths": [left_path.to_str().unwrap()]
        },
        "right": {
            "symbol": "provider",
            "paths": [right_path.to_str().unwrap()]
        }
    }))
    .await
    .unwrap();

    assert_eq!(
        compare_res.get("same_content").and_then(|v| v.as_bool()),
        Some(false)
    );
    let compare_diff = compare_res
        .get("unified_diff")
        .and_then(|v| v.as_str())
        .unwrap();
    assert!(compare_diff.contains("step_two"));
    assert!(compare_diff.contains("step_three"));
}

#[tokio::test]
async fn test_ast_tools_support_cpp_symbols_body_and_call_graph() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("widget.cc");
    fs::write(
        &path,
        r#"
namespace demo {
class Widget {
 public:
  void Render();
};

void Widget::Render() {
  HelperCall();
  base::DoThing();
}

int FreeFunction() {
  return ComputeValue();
}
}
"#,
    )
    .unwrap();

    let symbols = get_symbols::execute(&json!({ "path": path.to_str().unwrap() }))
        .await
        .unwrap();
    assert_eq!(
        symbols.get("language").and_then(|v| v.as_str()),
        Some("C++")
    );
    assert_symbol_name(&symbols, "Widget");
    assert_symbol_name(&symbols, "Widget::Render");
    assert_symbol_name(&symbols, "FreeFunction");

    let body = read_symbol_body::execute(&json!({
        "symbol": "Render",
        "file_hint": path.to_str().unwrap(),
        "include_signature": true
    }))
    .await
    .unwrap();
    assert_eq!(
        body.get("match_source").and_then(|v| v.as_str()),
        Some("ast")
    );
    assert!(
        body.get("content")
            .and_then(|v| v.as_str())
            .unwrap()
            .contains("HelperCall")
    );

    let call_graph = get_call_graph::execute(&json!({
        "file_path": path.to_str().unwrap(),
        "symbol": "Render"
    }))
    .await
    .unwrap();
    assert_string_array_contains(&call_graph, "outbound_calls", "HelperCall");
    assert_string_array_contains(&call_graph, "outbound_calls", "base::DoThing");
}

#[tokio::test]
async fn test_ast_tools_support_popular_language_symbols_and_calls() {
    let dir = tempdir().unwrap();

    let go_path = dir.path().join("service.go");
    fs::write(
        &go_path,
        r#"
package main

type Server struct{}

func Start() {
	helperCall()
	fmt.Println("ready")
}

func helperCall() {}
"#,
    )
    .unwrap();
    assert_language_symbols(&go_path, "Go", &["Server", "Start"]).await;
    let go_calls = get_call_graph::execute(&json!({
        "file_path": go_path.to_str().unwrap(),
        "symbol": "Start"
    }))
    .await
    .unwrap();
    assert_string_array_contains(&go_calls, "outbound_calls", "helperCall");

    let java_path = dir.path().join("App.java");
    fs::write(
        &java_path,
        r#"
class App {
  void run() {
    helperCall();
    System.out.println("ready");
  }

  void helperCall() {}
}
"#,
    )
    .unwrap();
    assert_language_symbols(&java_path, "Java", &["App", "run"]).await;
    let java_calls = get_call_graph::execute(&json!({
        "file_path": java_path.to_str().unwrap(),
        "symbol": "run"
    }))
    .await
    .unwrap();
    assert_string_array_contains(&java_calls, "outbound_calls", "helperCall");
    assert_string_array_contains(&java_calls, "outbound_calls", "System.out.println");

    let cs_path = dir.path().join("Worker.cs");
    fs::write(
        &cs_path,
        r#"
namespace Demo {
  class Worker {
    void Run() {
      Helper();
      Console.WriteLine("ready");
    }

    void Helper() {}
  }
}
"#,
    )
    .unwrap();
    assert_language_symbols(&cs_path, "C#", &["Demo", "Worker", "Run"]).await;
    let cs_calls = get_call_graph::execute(&json!({
        "file_path": cs_path.to_str().unwrap(),
        "symbol": "Run"
    }))
    .await
    .unwrap();
    assert_string_array_contains(&cs_calls, "outbound_calls", "Helper");
    assert_string_array_contains(&cs_calls, "outbound_calls", "Console.WriteLine");

    let php_path = dir.path().join("Service.php");
    fs::write(
        &php_path,
        r#"
<?php
class Service {
  function run() {
    helper_call();
    $this->emit();
  }

  function emit() {}
}

function helper_call() {}
"#,
    )
    .unwrap();
    assert_language_symbols(&php_path, "PHP", &["Service", "run", "helper_call"]).await;
    let php_calls = get_call_graph::execute(&json!({
        "file_path": php_path.to_str().unwrap(),
        "symbol": "run"
    }))
    .await
    .unwrap();
    assert_string_array_contains(&php_calls, "outbound_calls", "helper_call");

    let ruby_path = dir.path().join("worker.rb");
    fs::write(
        &ruby_path,
        r#"
module Demo
  class Worker
    def run
      helper_call()
      logger.info("ready")
    end

    def helper_call
    end
  end
end
"#,
    )
    .unwrap();
    assert_language_symbols(
        &ruby_path,
        "Ruby",
        &["Demo", "Worker", "run", "helper_call"],
    )
    .await;
    let ruby_calls = get_call_graph::execute(&json!({
        "file_path": ruby_path.to_str().unwrap(),
        "symbol": "run"
    }))
    .await
    .unwrap();
    assert_string_array_contains(&ruby_calls, "outbound_calls", "helper_call");
}

async fn assert_language_symbols(path: &std::path::Path, language: &str, expected_names: &[&str]) {
    let symbols = get_symbols::execute(&json!({ "path": path.to_str().unwrap() }))
        .await
        .unwrap();
    assert_eq!(
        symbols.get("language").and_then(|v| v.as_str()),
        Some(language)
    );
    for expected_name in expected_names {
        assert_symbol_name(&symbols, expected_name);
    }
}

fn assert_symbol_name(symbols: &serde_json::Value, expected_name: &str) {
    let names = symbols
        .get("symbols")
        .and_then(|v| v.as_array())
        .unwrap()
        .iter()
        .filter_map(|symbol| symbol.get("name").and_then(|v| v.as_str()))
        .collect::<Vec<_>>();
    assert!(
        names.contains(&expected_name),
        "expected symbol {expected_name}, got {names:?}"
    );
}

fn assert_string_array_contains(value: &serde_json::Value, field: &str, expected: &str) {
    let values = value
        .get(field)
        .and_then(|v| v.as_array())
        .unwrap()
        .iter()
        .filter_map(|item| item.as_str())
        .collect::<Vec<_>>();
    assert!(
        values.contains(&expected),
        "expected {field} to contain {expected}, got {values:?}"
    );
}
