# codebase-mcp

![Rust](https://img.shields.io/badge/Rust-stable-black?logo=rust)
![MCP](https://img.shields.io/badge/MCP-stdio-blue)
![License](https://img.shields.io/badge/license-Apache--2.0-green)

`codebase-mcp` is a local-first MCP server for real codebases. It exposes 34 tools for file access, search, Markdown navigation, symbols, diffs, archive browsing, SQLite inspection, and safe write workflows with undo/redo history.

## Highlights

- Local `stdio` MCP server that works with Cursor, Claude Desktop, VS Code, Cline, Roo, and similar clients.
- Fast file and workspace navigation with structured JSON responses.
- AST-aware code intelligence for Rust, Python, JavaScript, TypeScript, and TSX.
- Safe write tools with structured errors, history tracking, undo, and redo.
- Built for large repositories where targeted reads are cheaper than dumping full files into context.

## Quick Start

### Requirements

- Rust stable

### Build

```bash
cargo build --release
```

### Run

```bash
cargo run --release
```

Release binaries are written to:

- Windows: `target/release/codebase-mcp.exe`
- Linux/macOS: `target/release/codebase-mcp`

### Optional Runtime Configuration

- `CODEBASE_MCP_LOG`: log level such as `error`, `warn`, `info`, `debug`, or `trace`
- `CODEBASE_MCP_LOG_FILE`: write logs to a file instead of stderr
- `CODEBASE_MCP_WORKSPACE_ROOT`: preferred workspace root for relative tool paths when the client does not send roots
- `CODEBASE_MCP_INDEX_DIR`: override the persistent path-index cache directory
- `CODEBASE_MCP_INDEX_STALE_SECS`: seconds before a completed path index is considered stale

Legacy `TURBO_*` aliases are still accepted for the same settings.

## Client Configuration

### Claude Desktop

```json
{
  "mcpServers": {
    "codebase-mcp": {
      "command": "C:\\path\\to\\codebase-mcp\\target\\release\\codebase-mcp.exe",
      "args": []
    }
  }
}
```

### Cursor / Windsurf / Cline

```json
{
  "mcpServers": {
    "codebase-mcp": {
      "command": "/absolute/path/to/codebase-mcp/target/release/codebase-mcp",
      "args": []
    }
  }
}
```

### VS Code

```json
{
  "servers": {
    "codebase-mcp": {
      "type": "stdio",
      "command": "/absolute/path/to/codebase-mcp/target/release/codebase-mcp",
      "args": []
    }
  }
}
```

## Tool Surface

### Files And Edits

- `resolve_path`
- `read_file_range`
- `count_file_lines`
- `read_snippets`
- `convert_file_format`
- `create_file`
- `create_directory`
- `delete_file`
- `edit_file`
- `file_summary`
- `history_status`
- `undo_last_change`
- `redo_last_change`

### Search And Workspace Navigation

- `text_search`
- `fuzzy_find`
- `project_map`
- `workspace_stats`
- `markdown_outline`
- `read_markdown_section`
- `find_json_paths`
- `extract_json_schema`

### Code Intelligence

- `get_symbols`
- `read_symbol_body`
- `find_definition`
- `find_references`
- `list_imports`
- `list_exports`
- `get_call_graph`
- `compare_symbols`
- `diff_two_snippets`

### Data And Diagnostics

- `sqlite_inspect`
- `peek_archive`
- `server_health`
- `batch_tool_call`

## Example Calls

### Search Rust files for TODO comments

```json
{
  "name": "text_search",
  "arguments": {
    "query": "TODO|FIXME",
    "paths": ["/workspace"],
    "mode": "regex",
    "includes": ["*.rs"],
    "max_results": 50
  }
}
```

### Read multiple snippets in one request

```json
{
  "name": "read_snippets",
  "arguments": {
    "max_total_bytes": 24000,
    "requests": [
      { "path": "/workspace/src/main.rs", "start_line": 1, "end_line": 80 },
      { "path": "/workspace/src/lib.rs", "start_line": 1, "end_line": 60 }
    ]
  }
}
```

### Jump to a Markdown section by heading

```json
{
  "name": "read_markdown_section",
  "arguments": {
    "path": "/workspace/docs/architecture.md",
    "heading_path": ["Runtime", "Reconnect Flow"],
    "include_subsections": true
  }
}
```

### Read a symbol body directly

```json
{
  "name": "read_symbol_body",
  "arguments": {
    "symbol": "processPayment",
    "paths": ["/workspace/src"],
    "include_signature": true
  }
}
```

### Batch related calls in one round-trip

```json
{
  "name": "batch_tool_call",
  "arguments": {
    "calls": [
      {
        "tool": "file_summary",
        "args": { "path": "/workspace/README.md" }
      },
      {
        "tool": "project_map",
        "args": { "path": "/workspace", "max_depth": 2 }
      }
    ]
  }
}
```

## Behavior And Safety

- The server is tools-only. It does not expose MCP resources, prompts, or templates.
- `batch_tool_call` runs tools sequentially and rejects recursive batch calls.
- Write tools return structured success or error payloads instead of free-form text.
- Mutating tools record history metadata so clients can decide when undo/redo is available.
- `read_file_range` rejects oversized files.
- Tool calls are rate limited and wrapped in a request timeout.
- `server_health` reports uptime and indexing status so clients can reason about readiness.

## Development

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test --release
```

If you want to exercise the server manually, point any MCP client at the local release binary and inspect the available tool catalog from the client UI.

## License

Apache-2.0. See [LICENSE](./LICENSE).
