use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::tools::ast_support::{
    DEFAULT_AST_FILE_SIZE_LIMIT, collect_symbols, parse_supported_file,
};

pub fn schema() -> Value {
    json!({
        "name": "get_symbols",
        "description": "Extract Tree-sitter AST symbols from Rust, JavaScript/TypeScript, Python, C/C++, Go, Java, C#, PHP, Ruby, Swift, Nix, and Objective-C source files.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": { "type": "string" }
            },
            "required": ["path"]
        }
    })
}

pub async fn execute(args: &Value) -> Result<Value> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing/empty path")?;
    let path = crate::common::resolve_tool_path(path_str);

    if !path.exists() || !path.is_file() {
        return Err(anyhow::anyhow!(
            "File does exist or is not a file: {}",
            path_str
        ));
    }

    let parsed = parse_supported_file(&path, DEFAULT_AST_FILE_SIZE_LIMIT, None)?
        .ok_or_else(|| anyhow::anyhow!("Unsupported extension for get_symbols"))?;
    let symbols = collect_symbols(parsed.tree.root_node(), &parsed.source);

    Ok(json!({
        "file": path_str,
        "language": parsed.language_name,
        "total_symbols": symbols.len(),
        "symbols": symbols
    }))
}
