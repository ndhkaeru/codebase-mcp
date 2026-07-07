# codebase-mcp

![Rust](https://img.shields.io/badge/Rust-stable-black?logo=rust)
![MCP](https://img.shields.io/badge/MCP-stdio-blue)
![Protocol](https://img.shields.io/badge/MCP%20protocol-2024--11--05-blue)
![License](https://img.shields.io/badge/license-Apache--2.0-green)

> **Let coding agents explore, search, and safely edit large repositories without loading whole files or directory trees into context.**

`codebase-mcp` is a high-performance, local-first [Model Context Protocol](https://modelcontextprotocol.io) server written in Rust. It runs over `stdio` and gives an agent 26 precise tools for targeted reads, scoped search, Tree-sitter code intelligence, and structured edits, so the model spends its context window on answers, not on raw file dumps.

Everything runs on your machine. There is no external indexing service, no network calls, and no telemetry.

---

## Highlights

- **Targeted reads**: read a line range, a few snippets, or a single symbol body instead of an entire file.
- **Scoped search**: literal or regex matching with glob filters and large-repo-friendly limits.
- **Code intelligence**: symbols, definitions, references, imports/exports, and call graphs via Tree-sitter (13 languages).
- **Safe edits**: create, edit, and delete with structured success/error metadata for auditing.
- **Persistent index**: LMDB path metadata with an optional Tantivy content sidecar, kept per workspace.
- **Local-only**: no external services, runs entirely over `stdio`.

---

## Quick Start

### 1. Get the server

**Run with npx:**

```bash
npx -y @ndhkaeru/codebase-mcp@latest
```

**Install with one command (Linux/macOS):**

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/ndhkaeru/codebase-mcp/releases/latest/download/codebase-mcp-installer.sh | sh
```

**Install with one command (Windows, PowerShell):**

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/ndhkaeru/codebase-mcp/releases/latest/download/codebase-mcp-installer.ps1 | iex"
```

**Or download a prebuilt archive** for your platform (Linux, macOS, Windows — x64 and arm64)
from the [Releases page](https://github.com/ndhkaeru/codebase-mcp/releases). Each archive ships
the binary plus `LICENSE`/`README.md`, with a `.sha256` checksum alongside.

**Build from source (all platforms, requires the Rust stable toolchain):**

```bash
cargo build --release
```

The binary is written to:

- Windows: `target/release/codebase-mcp.exe`
- Linux/macOS: `target/release/codebase-mcp`

### 2. Add it to your MCP client

Register the binary as a `stdio` server with one command. Replace the path with your binary location.

**Claude Code**

```bash
claude mcp add codebase-mcp -- /path/to/codebase-mcp
```

**OpenAI Codex CLI**

```bash
codex mcp add codebase-mcp -- /path/to/codebase-mcp
```

**Gemini CLI**

```bash
gemini mcp add codebase-mcp -- /path/to/codebase-mcp
```

On Windows, use the full `.exe` path, for example `C:\path\to\codebase-mcp.exe`.

> Prefer editing a config file, or using Cursor, Windsurf, Cline, Roo, or VS Code?
> See [Client Configuration](#client-configuration) below.

---

## Why codebase-mcp?

Coding agents waste context (and money) when they read more than they need. `codebase-mcp` keeps responses small and on-target:

| Without it | With `codebase-mcp` |
| --- | --- |
| Read a 2,000-line file to inspect one function | `read_symbol_body` returns the function body plus signature only |
| Dump the whole tree to find a file | `fuzzy_find` / `project_map` return a scoped, bounded view |
| `cat` a file and grep it inline | `text_search` with `paths`, `includes`, and `max_results` |
| Multiple round trips to gather context | `batch_tool_call` bundles related calls into one request |

The result: fewer tokens, faster turns, and answers grounded in the real repository.

---

## Tools

26 tools, grouped by purpose. Each returns output in the standard MCP `content` array shape.

### Files and Edits

| Tool | Description |
| --- | --- |
| `resolve_path` | Normalize a path and return preflight accessibility metadata. |
| `read_file_range` | Read a file or line range with encoding detection and truncation metadata. |
| `count_file_lines` | Count lines in a text file with basic encoding and binary detection. |
| `read_snippets` | Read multiple file ranges in one request. |
| `convert_file_format` | Rewrite a file with normalized encoding and line endings. |
| `create_file` | Create or overwrite a file with optional parent creation, encoding, and line endings. |
| `create_directory` | Create a directory with optional parent creation and structured results. |
| `delete_file` | Delete a file with structured success and error metadata. |
| `edit_file` | Edit a file using replace, append, prepend, or find-replace modes. |
| `file_summary` | Return quick file metadata, binary detection, and a short preview. |

### Search and Workspace

| Tool | Description |
| --- | --- |
| `text_search` | Search files or directories using literal or regex matching. |
| `fuzzy_find` | Fuzzy path and file-name search across one or more roots. |
| `project_map` | Build a tree view of a project with optional size metadata. |
| `workspace_stats` | Summarize file, line, and language counts for a workspace path. |

### Code Intelligence

| Tool | Description |
| --- | --- |
| `get_symbols` | Extract Tree-sitter AST symbols from source files. |
| `read_symbol_body` | Read a symbol body with AST-first resolution and heuristic fallback. |
| `find_definition` | Find likely symbol definitions across the project. |
| `find_references` | Find symbol references across the project. |
| `list_imports` | List imports (Rust, JS/TS, Swift, Objective-C). |
| `list_exports` | List exports (Rust, JS/TS, Swift, Objective-C). |
| `get_call_graph` | List outbound calls made from a function or symbol. |
| `compare_symbols` | Compare two resolved symbols and return metadata plus a unified diff. |
| `compare_directories` | Compare two source directories and return AI-friendly added/deleted/modified/renamed summaries with optional bounded diffs. |

Example `compare_directories` input:

```json
{
  "left_path": "./old-version",
  "right_path": "./new-version",
  "summary_only": false,
  "detect_renames": true,
  "rename_similarity_threshold": 0.85,
  "max_diff_bytes": 262144,
  "excludes": ["**/*.lock"]
}
```

The response includes stable JSON fields such as `summary`, `added_files`, `deleted_files`, `renamed_files`, `modified_files`, `top_changed_directories`, `extensions_summary`, `changed_files_by_directory`, `risk_hints`, `binary_files`, `skipped_files`, and `warnings`. Modified text entries include best-effort `affected_symbols`. Use `summary_only: true` for large trees, or `output_format: "markdown"` for a compact human-readable report.

Small response shape example:

```json
{
  "summary": { "added_files": 1, "deleted_files": 0, "renamed_files": 1, "modified_text_files": 1 },
  "top_changed_directories": [{ "name": "src", "count": 2 }],
  "renamed_files": [{ "old_path": "src/old.rs", "new_path": "src/new.rs", "similarity": 0.92 }],
  "modified_files": [{ "path": "src/lib.rs", "affected_symbols": ["run"] }]
}
```

### Data and Diagnostics

| Tool | Description |
| --- | --- |
| `peek_archive` | List archive entries or read a file inside an archive. |
| `server_health` | Check server uptime and indexing health. |
| `batch_tool_call` | Run multiple tools sequentially in one request and flatten combined results. |

---

## Supported Languages

The AST-backed code-intelligence tools (`get_symbols`, `read_symbol_body`, `find_definition`, `find_references`, `get_call_graph`, `compare_symbols`) use Tree-sitter and support these languages:

| Language | Extensions |
| --- | --- |
| Rust | `.rs` |
| C | `.c` |
| C++ | `.cc`, `.cpp`, `.cxx`, `.h`, `.hh`, `.hpp`, `.hxx`, `.inc`, `.inl` |
| Go | `.go` |
| Java | `.java` |
| C# | `.cs` |
| PHP | `.php` |
| Ruby | `.rb` |
| JavaScript | `.js`, `.jsx`, `.mjs`, `.cjs` |
| TypeScript | `.ts`, `.tsx` |
| Python | `.py` |
| Swift | `.swift` |
| Objective-C | `.m`, `.mm` |

> `list_imports` and `list_exports` currently support a subset: Rust, JavaScript/TypeScript, Swift, and Objective-C.
> The file, search, and workspace tools (`text_search`, `fuzzy_find`, `read_file_range`, and others) work on any text file regardless of language. Files larger than 2 MB are skipped by the AST parser.

---

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

Read a symbol body with its signature:

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

---

## Client Configuration

Use the release binary or npm wrapper as a `stdio` MCP server. On Windows binary installs, point `command` to the `.exe` file.

<details>
<summary><strong>npx package</strong></summary>

```json
{
  "mcpServers": {
    "codebase-mcp": {
      "command": "npx",
      "args": ["-y", "@ndhkaeru/codebase-mcp@latest"]
    }
  }
}
```
</details>


<details>
<summary><strong>Claude Desktop</strong></summary>

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
</details>

<details>
<summary><strong>Cursor, Windsurf, Cline, Roo</strong></summary>

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
</details>

<details>
<summary><strong>VS Code MCP</strong></summary>

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
</details>

---

## Runtime Configuration

| Variable | Purpose |
| --- | --- |
| `CODEBASE_MCP_INDEX_DIR` | Overrides the persistent index cache directory. |
| `CODEBASE_MCP_TANTIVY_ENABLED` | Enables the Tantivy content sidecar. Defaults to `true`. |

<details>
<summary><strong>Built-in defaults and workspace handling</strong></summary>

Chromium-sized defaults are built in: path index map size is 8 GB, stale path metadata refreshes after 1 hour, Tantivy reads up to 2 MB per file, warms up to 4 GB per content zone, and caps each workspace content sidecar at 16 GB.

Workspace roots are dynamic: the server indexes client-provided `workspaceFolders`, `roots`, `rootUri`, or `clientInfo.workspaceRoot`; if none are provided, it falls back only to the process current directory when that directory looks like a workspace root. Relative tool paths resolve against the active indexed workspace, so clients should send the workspace for each chat session.
</details>

<details>
<summary><strong>Index storage locations</strong></summary>

The server keeps one index per canonical workspace root.

- Windows default: `%LOCALAPPDATA%\codebase-mcp\index-v2\<workspace_hash>\`
- Linux/macOS default: `$XDG_CACHE_HOME/codebase-mcp/index-v2/<workspace_hash>/` or `~/.cache/codebase-mcp/index-v2/<workspace_hash>/`
- Custom root: `<CODEBASE_MCP_INDEX_DIR>\index-v2\<workspace_hash>\`
- Tantivy sidecar: `<workspace_index_dir>\tantivy-content\`

LMDB stores path metadata. Tantivy is used as an optional sidecar for warmed content zones. When a query or scope cannot be served from the content index, tools fall back to filesystem scanning where appropriate.
</details>

---

## Behavior and Safety

- The server exposes tools only: no MCP resources, prompts, or templates.
- Tool responses are returned in the standard MCP `content` array shape.
- `batch_tool_call` executes calls sequentially and rejects recursive batch calls.
- Mutating tools return structured write metadata for auditing; write tools return structured success or error payloads instead of free-form text.
- `read_file_range` and search-oriented tools keep output bounded for agent context.
- Tool calls default to a 60-second timeout. Clients may request longer calls with `timeout_seconds`, capped at 600 seconds.
- A global rate limiter protects the server from excessive request volume.
- `server_health` reports uptime and indexing status for readiness checks.

---

## Large Repository Tips

- Scope `paths` to the smallest relevant subtree.
- Prefer concrete path tokens and basenames for `fuzzy_find`.
- Keep `project_map` shallow, for example `max_depth <= 2`.
- Use recursive glob excludes such as `third_party/**`, `node_modules/**`, `target/**`, `dist/**`, and `out/**`.
- Use `read_snippets` after search results instead of reading full files.
- Treat Tree-sitter results as local syntactic intelligence, not as a replacement for full semantic language servers.

---

## Shipping

Releases are distributed through two channels:

- **GitHub Releases**: `release.yml` is generated by `cargo-dist`. A semver tag such as `v1.4.1` builds native archives/installers for macOS, Linux, and Windows on x64/arm64.
- **npm/npx**: `npm.yml` publishes `@ndhkaeru/codebase-mcp`. It waits for the matching GitHub Release binaries, bundles them into `packages/npm/native/`, validates `server.json`, and publishes the wrapper when `NPM_TOKEN` is configured.

Release checklist:

1. Update versions in `Cargo.toml`, `Cargo.lock`, `packages/npm/package.json`, and `server.json`.
2. Run the local checks from `rust.yml`: format, clippy, tests, audit, deny, and typos where available.
3. Push `main`, then check GitHub Actions for failures.
4. Push a semver tag such as `v1.4.1`.
5. Confirm `Release` and `Publish npm package` workflows succeed.
6. Optionally dispatch `Publish npm package` with `publish_registry=true` to publish `server.json` to the MCP Registry; use `publish_npm=false` when npm already has that version.
7. Smoke test `npx -y @ndhkaeru/codebase-mcp@<version>`.

For custom local installs, the npm wrapper honors `CODEBASE_MCP_BINARY`.

### MCP Registry Metadata

`server.json` follows the MCP Registry metadata shape used by mature MCP packages. Keep its `name`, `version`, and npm package entry in sync with `packages/npm/package.json`; the npm release workflow validates this before publishing.

### Tool Description Style

Tool descriptions are written for agent routing, not humans only: they state when to use the tool, what to pass first, what limits protect large repositories, and which output format is best for chaining versus review.

---

## Development

Format, lint, and test:

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test --release
```

### Repository layout

```text
src/
  main.rs              MCP stdio server, JSON-RPC handling, logging, timeouts
  mcp.rs               JSON-RPC request/response types
  common.rs            shared path and environment helpers
  version.rs           package version export
  indexer/             LMDB/Tantivy workspace indexing
  history/             write-change metadata support
  security/            path guarding and rate limiting
  tools/               MCP tool implementations and schemas
tests/                 integration and behavior tests
```

---

## Acknowledgements

Built on [Tree-sitter](https://tree-sitter.github.io/tree-sitter/), [Tantivy](https://github.com/quickwit-oss/tantivy), [LMDB](http://www.lmdb.tech/doc/) (via [heed](https://github.com/meilisearch/heed)), [Tokio](https://tokio.rs/), and the [Model Context Protocol](https://modelcontextprotocol.io).

## License

Apache-2.0. See [LICENSE](./LICENSE).


