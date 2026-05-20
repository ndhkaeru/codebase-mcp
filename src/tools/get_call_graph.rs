use anyhow::{Context, Result};
use serde_json::{Value, json};
use tree_sitter::Node;

use crate::tools::ast_support::{
    DEFAULT_AST_FILE_SIZE_LIMIT, call_expression_name, find_named_function_like, is_call_node,
    parse_supported_file,
};

pub fn schema() -> Value {
    json!({
        "name": "get_call_graph",
        "description": "List outbound calls made from a function or symbol using Tree-sitter AST support for Rust, JavaScript/TypeScript, Python, C/C++, Go, Java, C#, PHP, Ruby, Swift, and Objective-C.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "file_path": { "type": "string" },
                "symbol": { "type": "string" }
            },
            "required": ["file_path", "symbol"]
        }
    })
}

pub async fn execute(args: &Value) -> Result<Value> {
    let path_str = args
        .get("file_path")
        .and_then(|v| v.as_str())
        .context("Missing file_path")?;
    let symbol = args
        .get("symbol")
        .and_then(|v| v.as_str())
        .context("Missing symbol")?;

    let path = crate::common::resolve_tool_path(path_str);
    if !path.exists() || !path.is_file() {
        return Err(anyhow::anyhow!("File does not exist: {}", path_str));
    }

    let parsed = parse_supported_file(&path, DEFAULT_AST_FILE_SIZE_LIMIT, None)?
        .ok_or_else(|| anyhow::anyhow!("Unsupported extension for get_call_graph"))?;
    let root = parsed.tree.root_node();
    let function_node = find_named_function_like(root, &parsed.source, symbol)
        .ok_or_else(|| anyhow::anyhow!("Could not find function '{}' in the file", symbol))?;

    let mut outbound = Vec::new();
    find_outbound_calls(function_node, &parsed.source, &mut outbound);
    outbound.sort();
    outbound.dedup();

    Ok(json!({
        "file": path_str,
        "language": parsed.language_name,
        "symbol": symbol,
        "start_line": function_node.start_position().row + 1,
        "end_line": function_node.end_position().row + 1,
        "outbound_calls": outbound,
        "total_calls": outbound.len()
    }))
}

fn find_outbound_calls(node: Node<'_>, source: &[u8], calls: &mut Vec<String>) {
    if is_call_node(node.kind())
        && let Some(text) = call_expression_name(node, source)
        && !text.is_empty()
    {
        calls.push(text);
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        find_outbound_calls(child, source, calls);
    }
}
