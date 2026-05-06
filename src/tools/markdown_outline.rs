use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::tools::markdown_support::{load_markdown_file, parse_headings, section_end_line};

pub fn schema() -> Value {
    json!({
        "name": "markdown_outline",
        "description": "List Markdown headings with line numbers, hierarchy, and section spans.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "max_depth": { "type": "integer" }
            },
            "required": ["path"]
        }
    })
}

pub async fn execute(args: &Value) -> Result<Value> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing path")?;
    let path = crate::common::resolve_tool_path(path_str);
    let max_depth = args
        .get("max_depth")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);

    let (content, total_lines) = load_markdown_file(&path)?;
    let headings = parse_headings(&content);

    let items = headings
        .iter()
        .enumerate()
        .filter(|(_, heading)| max_depth.is_none_or(|depth| heading.level <= depth))
        .map(|(index, heading)| {
            json!({
                "title": heading.title,
                "level": heading.level,
                "line": heading.line,
                "section_end_line": section_end_line(
                    &headings,
                    index,
                    total_lines,
                    crate::tools::markdown_support::SectionSpanMode::IncludeSubsections
                ),
                "slug": heading.slug,
                "path": heading.path,
                "path_text": heading.path.join(" > ")
            })
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "path": path.to_string_lossy(),
        "total_lines": total_lines,
        "heading_count": headings.len(),
        "headings": items
    }))
}
