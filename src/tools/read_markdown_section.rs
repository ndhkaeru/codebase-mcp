use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::common::insert_object_field;
use crate::tools::markdown_support::{
    SectionSpanMode, load_markdown_file, match_heading, parse_headings, section_end_line,
};
use crate::tools::read_file;

pub fn schema() -> Value {
    json!({
        "name": "read_markdown_section",
        "description": "Read a Markdown section by heading name or heading path.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "heading": { "type": "string" },
                "heading_path": { "type": "array", "items": { "type": "string" } },
                "exact": { "type": "boolean" },
                "include_heading": { "type": "boolean" },
                "include_subsections": { "type": "boolean" },
                "include_line_numbers": { "type": "boolean" },
                "max_lines": { "type": "integer" },
                "max_bytes": { "type": "integer" }
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
    let heading = args.get("heading").and_then(|v| v.as_str());
    let heading_path = args
        .get("heading_path")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(ToString::to_string))
                .collect::<Vec<_>>()
        });

    if heading.is_none() && heading_path.is_none() {
        return Err(anyhow::anyhow!(
            "Either heading or heading_path is required"
        ));
    }

    let exact = args.get("exact").and_then(|v| v.as_bool()).unwrap_or(false);
    let include_heading = args
        .get("include_heading")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let include_subsections = args
        .get("include_subsections")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let path = crate::common::resolve_tool_path(path_str);

    let (content, total_lines) = load_markdown_file(&path)?;
    let headings = parse_headings(&content);
    let heading_index = match_heading(&headings, heading, heading_path.as_ref(), exact)?;
    let selected = &headings[heading_index];
    let section_end = section_end_line(
        &headings,
        heading_index,
        total_lines,
        if include_subsections {
            SectionSpanMode::IncludeSubsections
        } else {
            SectionSpanMode::HeadingBodyOnly
        },
    );
    let start_line = if include_heading {
        selected.line
    } else {
        selected.line + 1
    };

    let mut response = if start_line > section_end {
        json!({
            "content": "",
            "start_line": start_line,
            "end_line": section_end,
            "total_lines": total_lines,
            "encoding": "UTF-8",
            "file_size_bytes": content.len(),
            "truncated": false,
            "omitted_lines": 0,
            "returned_lines": 0
        })
    } else {
        let mut request = json!({
            "path": path.to_string_lossy(),
            "start_line": start_line,
            "end_line": section_end,
            "include_line_numbers": args
                .get("include_line_numbers")
                .cloned()
                .unwrap_or(json!(false))
        });

        if let Some(max_lines) = args.get("max_lines") {
            insert_object_field(&mut request, "max_lines", max_lines.clone());
        }
        if let Some(max_bytes) = args.get("max_bytes") {
            insert_object_field(&mut request, "max_bytes", max_bytes.clone());
        }

        read_file::execute_sync(&request)?
    };

    insert_object_field(
        &mut response,
        "path",
        json!(path.to_string_lossy().to_string()),
    );
    insert_object_field(
        &mut response,
        "selected_heading",
        json!({
            "title": selected.title,
            "level": selected.level,
            "line": selected.line,
            "slug": selected.slug,
            "path": selected.path,
            "path_text": selected.path.join(" > ")
        }),
    );
    insert_object_field(
        &mut response,
        "section",
        json!({
            "include_heading": include_heading,
            "include_subsections": include_subsections,
            "section_start_line": selected.line,
            "section_end_line": section_end
        }),
    );

    if let Some(next_start_line) = response.get("next_start_line").and_then(|v| v.as_u64()) {
        insert_object_field(
            &mut response,
            "continuation",
            json!({
                "reason": "section_truncated",
                "next_start_line": next_start_line,
                "suggested_request": {
                    "path": path.to_string_lossy().to_string(),
                    "start_line": next_start_line,
                    "end_line": section_end,
                    "include_line_numbers": args
                        .get("include_line_numbers")
                        .cloned()
                        .unwrap_or(json!(false))
                }
            }),
        );
    }

    Ok(response)
}
