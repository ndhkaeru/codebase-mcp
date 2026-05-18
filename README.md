# codebase-mcp

![Rust](https://img.shields.io/badge/Rust-stable-black?logo=rust)
![MCP](https://img.shields.io/badge/MCP-stdio-blue)
![License](https://img.shields.io/badge/license-Apache--2.0-green)

`codebase-mcp` is a high-performance, local-first MCP server for codebase exploration and safe filesystem workflows. It runs over `stdio`, exposes 34 tools, and is designed for agents that need precise reads, search, code intelligence, structured edits, and compact responses from real repositories.

## What It Does

- Serves MCP tools over standard JSON-RPC `stdio` framing.
- Reads files and snippets without dumping entire repositories into context.
- Searches code with literal/regex matching, glob filters, and large-repo friendly limits.
- Builds a persistent workspace index with LMDB path metadata and an optional Tantivy content sidecar.
- Provides Tree-sitter based symbols, imports/exports, references, definitions, symbol bodies, and call graphs.
- Supports safe writes with structured errors plus undo/redo history.
- Inspects Markdown, JSON, SQLite databases, archives, diffs, and workspace structure.

## When To Use

Use `codebase-mcp` when your MCP client or coding agent needs to explore, search, and safely edit a local repository without loading large files or full directory trees into context.

It is especially useful for:

- Large repositories where targeted reads and scoped searches matter.
- Agents that need structured filesystem, search, and code-intelligence tools.
- Workflows that benefit from undoable file edits.
- Local-only code analysis without relying on an external indexing service.

## Common Workflows

1. Use `fuzzy_find`, `project_map`, or `workspace_stats` to understand the repository shape.
2. Use `text_search` to locate relevant files, symbols, strings, or patterns.
3. Use `read_file_range`, `read_snippets`, or `file_summary` to inspect focused content.
4. Use `get_symbols`, `find_definition`, `find_references`, or `read_symbol_body` for code navigation.
5. Use `edit_file`, `create_file`, `create_directory`, or `delete_file` to make changes.
6. Use `history_status`, `undo_last_change`, or `redo_last_change` to review or revert supported edits.

## Project Status

- Package: `codebase-mcp`
- Version: `1.2.0`
- License: Apache-2.0
- Runtime: Rust + Tokio
- Transport: MCP `stdio`
- MCP protocol version returned by the server: `2024-11-05`
- Tool count: 34

## Requirements

- Rust stable toolchain
- An MCP-compatible client such as Claude Code, Codex, Cursor, Windsurf, Cline, Roo, or VS Code MCP integrations

## Build And Run

Build a release binary:

```bash
cargo build --release
```

Run directly during development:

```bash
cargo run --release
```

Release binary locations:

- Windows: `target/release/codebase-mcp.exe`
- Linux/macOS: `target/release/codebase-mcp`

## Client Configuration

Use the release binary as a `stdio` MCP server.

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

### Cursor, Windsurf, Cline, Roo

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

### VS Code MCP

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

On Windows, point `command` to the `.exe` file.

## Runtime Configuration

| Variable | Purpose |
| --- | --- |
| `CODEBASE_MCP_LOG` | Log level: `error`, `warn`, `info`, `debug`, or `trace`. Defaults to `info`. |
| `CODEBASE_MCP_LOG_FILE` | Writes logs to a file instead of stderr. |
| `CODEBASE_MCP_WORKSPACE_ROOT` | Preferred workspace root for relative tool paths when client roots are unavailable. |
| `CODEBASE_MCP_WALK_THREADS` | Thread count for directory walking. Defaults to a conservative value up to 4. |
| `CODEBASE_MCP_INDEX_DIR` | Overrides the persistent index cache directory. |
| `CODEBASE_MCP_INDEX_STALE_SECS` | Seconds before completed path metadata is considered stale. |
| `CODEBASE_MCP_INDEX_MAP_SIZE_MB` | LMDB map size in MB. Defaults to `4096`. |
| `CODEBASE_MCP_TANTIVY_ENABLED` | Enables the Tantivy content sidecar. Defaults to `true`. |
| `CODEBASE_MCP_TANTIVY_MAX_FILE_BYTES` | Maximum file size read for Tantivy content indexing. Defaults to `1048576`. |
| `CODEBASE_MCP_TANTIVY_MAX_ZONE_BYTES` | Maximum bytes per warmed content zone. Defaults to `1073741824`. |
| `CODEBASE_MCP_TANTIVY_MAX_WORKSPACE_BYTES` | Maximum bytes per workspace content sidecar. Defaults to `4294967296`. |

Legacy `TURBO_*` aliases are still accepted for backward compatibility where implemented.

## Index Storage

The server keeps one index per canonical workspace root.

- Windows default: `%LOCALAPPDATA%\codebase-mcp\index-v2\<workspace_hash>\`
- Linux/macOS default: `$XDG_CACHE_HOME/codebase-mcp/index-v2/<workspace_hash>/` or `~/.cache/codebase-mcp/index-v2/<workspace_hash>/`
- Custom root: `<CODEBASE_MCP_INDEX_DIR>\index-v2\<workspace_hash>\`
- Tantivy sidecar: `<workspace_index_dir>\tantivy-content\`

LMDB stores path metadata. Tantivy is used as an optional sidecar for warmed content zones. When a query or scope cannot be served from the content index, tools fall back to filesystem scanning where appropriate.

## Tool Catalog

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

### Search And Workspace

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

## Example Tool Calls

Search Rust files with regex:

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

Read multiple focused snippets in one call:

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

Find a symbol body with its signature:

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

Batch related calls in one MCP round trip:

```json
{
  "name": "batch_tool_call",
  "arguments": {
    "calls": [
      { "tool": "file_summary", "args": { "path": "/workspace/README.md" } },
      { "tool": "project_map", "args": { "path": "/workspace", "max_depth": 2 } }
    ]
  }
}
```

## Behavior And Safety

- The server exposes tools only; it does not expose MCP resources, prompts, or templates.
- Tool responses are returned in the standard MCP `content` array shape.
- `batch_tool_call` executes calls sequentially and rejects recursive batch calls.
- Mutating tools record history metadata for undo/redo where supported.
- Write tools return structured success or error payloads instead of free-form text.
- `read_file_range` and search-oriented tools keep output bounded for agent context.
- Tool calls default to a 60 second timeout. Clients may request longer calls with `timeout_seconds`, `timeout_secs`, `timeout_s`, or `timeout_ms`; values are capped at 600 seconds.
- A global rate limiter protects the server from excessive request volume.
- `server_health` reports uptime and indexing status for readiness checks.

## Large Repository Tips

- Scope `paths` to the smallest relevant subtree.
- Prefer concrete path tokens and basenames for `fuzzy_find`.
- Keep `project_map` shallow, for example `max_depth <= 2`.
- Use recursive glob excludes such as `third_party/**`, `node_modules/**`, `target/**`, `dist/**`, and `out/**`.
- Use `read_snippets` after search results instead of reading full files.
- Treat Tree-sitter results as local syntactic intelligence, not as a replacement for full semantic language servers.

## Development

Format, lint, and test:

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test --release
```

Useful local checks:

```bash
cargo test
cargo run --release
```

## Repository Layout

```text
src/
  main.rs              MCP stdio server, JSON-RPC handling, logging, timeouts
  mcp.rs               JSON-RPC request/response types
  common.rs            shared path and environment helpers
  version.rs           package version export
  indexer/             LMDB/Tantivy workspace indexing
  history/             write-history and undo/redo support
  security/            path guarding and rate limiting
  tools/               MCP tool implementations and schemas
tests/                 integration and behavior tests
```

## License

Apache-2.0. See [LICENSE](./LICENSE).
