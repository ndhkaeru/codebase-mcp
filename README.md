## codeloupe-mcp

A Model Context Protocol (MCP) server that gives coding agents fast, local-first access to large codebases. It enables LLMs to inspect files, search text, compare directories, read symbols, and apply structured edits through bounded tool responses instead of dumping whole files or repository trees into context.

### codeloupe-mcp vs shell tools

This package provides an MCP interface into repository exploration and code editing.

If you are using a **coding agent**, shell commands are still useful for concise build/test workflows. `codeloupe-mcp` is most useful when the model needs structured, repeatable, repository-aware code context across a chat session.

- **Shell / CLI**: best for deterministic commands such as `cargo test`, `npm run lint`, project scripts, one-off `rg`, and build logs that are already compact.
- **MCP**: best for agent loops that benefit from tool schemas, workspace-aware path resolution, bounded output, persistent indexes, and precise reads such as symbol bodies, snippets, shallow project maps, and directory diffs.

Use `codeloupe-mcp` when you want an agent to ask for exactly the code context it needs: a line range, a symbol body, a bounded search result, a fuzzy file match, a workspace summary, or a source-directory comparison.

### Key Features

- **Fast and lightweight**. Local Rust server over `stdio`; no external indexing service, no network calls, no telemetry.
- **LLM-friendly output**. Tools return bounded JSON/Markdown designed for agent context windows.
- **Repository-aware indexing**. LMDB path metadata plus optional Tantivy content indexing per workspace.
- **Precise code reads**. Read focused line ranges, snippets, symbol bodies, imports, exports, call graphs, definitions, and references.
- **Scoped search**. Literal and regex search with path scopes, include/exclude globs, fallback diagnostics, and large-repo safeguards.
- **Directory comparison**. Compare two source directories with added/deleted/modified/renamed summaries and bounded diffs.
- **Structured edits**. Create, edit, convert, and delete files with predictable success/error metadata.

### Requirements

- An MCP client that supports local `stdio` servers.
- Node.js 18 or newer when using `npx`.
- No Rust toolchain is needed when using the published npm package or a native release binary.
- Rust stable is required only when building from source.

### Getting started

First, install the `codeloupe-mcp` server with your client.

**Standard config** works in most MCP clients:

```json
{
  "mcpServers": {
    "codeloupe-mcp": {
      "command": "npx",
      "args": ["-y", "@ndhkaeru/codeloupe-mcp@latest"]
    }
  }
}
```

You can also use a native binary from the [GitHub Releases](https://github.com/ndhkaeru/codeloupe-mcp/releases) page:

```json
{
  "mcpServers": {
    "codeloupe-mcp": {
      "command": "/absolute/path/to/codeloupe-mcp",
      "args": []
    }
  }
}
```

On Windows native installs, point `command` to the `.exe` file, for example `C:\path\to\codeloupe-mcp.exe`.

<details>
<summary>Amp</summary>

Add via the Amp VS Code extension settings screen or by updating your settings JSON:

```json
"amp.mcpServers": {
  "codeloupe-mcp": {
    "command": "npx",
    "args": ["-y", "@ndhkaeru/codeloupe-mcp@latest"]
  }
}
```

Amp CLI setup:

```bash
amp mcp add codeloupe-mcp -- npx -y @ndhkaeru/codeloupe-mcp@latest
```

</details>

<details>
<summary>Antigravity</summary>

Add via Antigravity settings or by updating your MCP configuration file:

```json
{
  "mcpServers": {
    "codeloupe-mcp": {
      "command": "npx",
      "args": ["-y", "@ndhkaeru/codeloupe-mcp@latest"]
    }
  }
}
```

</details>

<details>
<summary>Claude Code</summary>

Use the Claude Code CLI:

```bash
claude mcp add codeloupe-mcp -- npx -y @ndhkaeru/codeloupe-mcp@latest
```

Native binary alternative:

```bash
claude mcp add codeloupe-mcp -- /absolute/path/to/codeloupe-mcp
```

</details>

<details>
<summary>Claude Desktop</summary>

Follow the MCP install guide and use the standard config:

```json
{
  "mcpServers": {
    "codeloupe-mcp": {
      "command": "npx",
      "args": ["-y", "@ndhkaeru/codeloupe-mcp@latest"]
    }
  }
}
```

Native Windows binary alternative:

```json
{
  "mcpServers": {
    "codeloupe-mcp": {
      "command": "C:\\path\\to\\codeloupe-mcp.exe",
      "args": []
    }
  }
}
```

</details>

<details>
<summary>Cline</summary>

Follow Cline's MCP server configuration flow and add this to `cline_mcp_settings.json`:

```json
{
  "mcpServers": {
    "codeloupe-mcp": {
      "type": "stdio",
      "command": "npx",
      "timeout": 60,
      "args": ["-y", "@ndhkaeru/codeloupe-mcp@latest"],
      "disabled": false
    }
  }
}
```

</details>

<details>
<summary>Codex</summary>

Use the Codex CLI:

```bash
codex mcp add codeloupe-mcp -- npx -y @ndhkaeru/codeloupe-mcp@latest
```

Alternatively, create or edit `~/.codex/config.toml`:

```toml
[mcp_servers.codeloupe-mcp]
command = "npx"
args = ["-y", "@ndhkaeru/codeloupe-mcp@latest"]
```

</details>

<details>
<summary>Copilot</summary>

Use the Copilot MCP add flow, or add this to your MCP config:

```json
{
  "mcpServers": {
    "codeloupe-mcp": {
      "type": "local",
      "command": "npx",
      "tools": ["*"],
      "args": ["-y", "@ndhkaeru/codeloupe-mcp@latest"]
    }
  }
}
```

</details>

<details>
<summary>Cursor</summary>

Go to `Cursor Settings` -> `MCP` -> `Add new MCP Server`.

Use command type with:

```bash
npx -y @ndhkaeru/codeloupe-mcp@latest
```

Or paste the standard config:

```json
{
  "mcpServers": {
    "codeloupe-mcp": {
      "command": "npx",
      "args": ["-y", "@ndhkaeru/codeloupe-mcp@latest"]
    }
  }
}
```

</details>

<details>
<summary>Factory</summary>

Use the Factory CLI:

```bash
droid mcp add codeloupe-mcp "npx -y @ndhkaeru/codeloupe-mcp@latest"
```

Alternatively, open Factory's MCP UI and paste the standard config.

</details>

<details>
<summary>Gemini CLI</summary>

Use the Gemini CLI MCP configuration file and add:

```json
{
  "mcpServers": {
    "codeloupe-mcp": {
      "command": "npx",
      "args": ["-y", "@ndhkaeru/codeloupe-mcp@latest"]
    }
  }
}
```

</details>

<details>
<summary>Goose</summary>

Go to `Advanced settings` -> `Extensions` -> `Add custom extension`.

Use type `STDIO` and command:

```bash
npx -y @ndhkaeru/codeloupe-mcp@latest
```

</details>

<details>
<summary>Junie</summary>

Use Junie's MCP flow, or add to `.junie/mcp/mcp.json`:

```json
{
  "mcpServers": {
    "codeloupe-mcp": {
      "command": "npx",
      "args": ["-y", "@ndhkaeru/codeloupe-mcp@latest"]
    }
  }
}
```

</details>

<details>
<summary>Kiro</summary>

Follow Kiro's MCP server documentation and add:

```json
{
  "mcpServers": {
    "codeloupe-mcp": {
      "command": "npx",
      "args": ["-y", "@ndhkaeru/codeloupe-mcp@latest"]
    }
  }
}
```

</details>

<details>
<summary>LM Studio</summary>

Go to `Program` in the right sidebar -> `Install` -> `Edit mcp.json`.

Use the standard config:

```json
{
  "mcpServers": {
    "codeloupe-mcp": {
      "command": "npx",
      "args": ["-y", "@ndhkaeru/codeloupe-mcp@latest"]
    }
  }
}
```

</details>

<details>
<summary>opencode</summary>

Add to `~/.config/opencode/opencode.json`:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "codeloupe-mcp": {
      "type": "local",
      "command": ["npx", "-y", "@ndhkaeru/codeloupe-mcp@latest"],
      "enabled": true
    }
  }
}
```

</details>

<details>
<summary>Qodo Gen</summary>

Open Qodo Gen chat panel in VS Code or IntelliJ, choose `Connect more tools` -> `+ Add new MCP`, then paste the standard config.

</details>

<details>
<summary>VS Code</summary>

Use the MCP server configuration:

```json
{
  "servers": {
    "codeloupe-mcp": {
      "type": "stdio",
      "command": "npx",
      "args": ["-y", "@ndhkaeru/codeloupe-mcp@latest"]
    }
  }
}
```

You can also install with the VS Code CLI:

```bash
code --add-mcp '{"name":"codeloupe-mcp","command":"npx","args":["-y","@ndhkaeru/codeloupe-mcp@latest"]}'
```

</details>

<details>
<summary>Warp</summary>

Go to `Settings` -> `AI` -> `Manage MCP Servers` -> `+ Add`, or use the `/add-mcp` slash command and paste the standard config.

</details>

<details>
<summary>Windsurf</summary>

Follow Windsurf's MCP documentation and use the standard config:

```json
{
  "mcpServers": {
    "codeloupe-mcp": {
      "command": "npx",
      "args": ["-y", "@ndhkaeru/codeloupe-mcp@latest"]
    }
  }
}
```

</details>

### Configuration

`codeloupe-mcp` is configured with environment variables in the MCP server process.

| Variable | Description |
|----------|-------------|
| `codeloupe_mcp_INDEX_DIR` | Override the persistent index cache directory. |
| `codeloupe_mcp_TANTIVY_ENABLED` | Enable/disable the Tantivy content sidecar. Defaults to `true`. |
| `codeloupe_mcp_WALK_THREADS` | Filesystem walk parallelism. Values are clamped to `1..=6`. |
| `codeloupe_mcp_BINARY` | npm wrapper override for a custom local binary path. |

Example with a custom index directory:

```json
{
  "mcpServers": {
    "codeloupe-mcp": {
      "command": "npx",
      "args": ["-y", "@ndhkaeru/codeloupe-mcp@latest"],
      "env": {
        "codeloupe_mcp_INDEX_DIR": "/path/to/cache"
      }
    }
  }
}
```

### Tools

Tools are grouped by purpose. Each response uses the standard MCP `content` array shape and keeps output bounded for agent context.

<details>
<summary>Files and edits</summary>

| Tool | Description |
|------|-------------|
| `resolve_path` | Normalize a path and return preflight accessibility metadata. |
| `read_file_range` | Read a file or line range with encoding detection and truncation metadata. |
| `count_file_lines` | Count lines in a text file with encoding and binary detection. |
| `read_snippets` | Read multiple file ranges in one request. |
| `file_summary` | Return file metadata, binary detection, and a short preview. |
| `convert_file_format` | Rewrite a file with normalized encoding and line endings. |
| `create_file` | Create or overwrite a file. |
| `create_directory` | Create a directory with optional parent creation. |
| `edit_file` | Edit a file with replace, append, prepend, or find-replace. |
| `delete_file` | Delete one file with structured success/error metadata. |

</details>

<details>
<summary>Search and workspace</summary>

| Tool | Description |
|------|-------------|
| `text_search` | Literal or regex search with path scopes, include/exclude filters, and fallback diagnostics. |
| `fuzzy_find` | Fuzzy path and file-name search across scoped roots. |
| `project_map` | Bounded tree view with optional size metadata. |
| `workspace_stats` | File, line, language, and size summaries. |
| `compare_directories` | Compare two source directory trees with added/deleted/modified/renamed summaries and bounded diffs. |

</details>

<details>
<summary>Code intelligence</summary>

| Tool | Description |
|------|-------------|
| `get_symbols` | Extract Tree-sitter symbols from a source file. |
| `read_symbol_body` | Read a symbol body with AST-first resolution and fallback heuristics. |
| `find_definition` | Find likely symbol definitions across code files. |
| `find_references` | Find likely symbol references across code files. |
| `list_imports` | List imports for supported languages. |
| `list_exports` | List exports for supported languages. |
| `get_call_graph` | List outbound calls made by a function or symbol. |
| `compare_symbols` | Compare two resolved symbols and return metadata plus a unified diff. |

</details>

<details>
<summary>Data and diagnostics</summary>

| Tool | Description |
|------|-------------|
| `peek_archive` | List archive entries or read one file inside an archive. |
| `server_health` | Report uptime, workspace, and index health. |
| `content_index_status` | Report Tantivy content-index status for scoped files or directories. |
| `warm_content_index` | Warm Tantivy content index zones for repeated searches. |
| `batch_tool_call` | Run a short sequence of tools in one MCP request. |

</details>

### Supported languages

AST-backed tools use Tree-sitter for Rust, C, C++, Go, Java, C#, PHP, Ruby, JavaScript, TypeScript, Python, Swift, and Objective-C.

`list_imports` and `list_exports` currently support Rust, JavaScript/TypeScript, Swift, and Objective-C. File/search/workspace tools work on any text file regardless of language.

### Large repository guidance

- Scope `paths` to the smallest useful subtree.
- Prefer literal `text_search` when possible so Tantivy can shortlist files.
- Use concrete basenames or path tokens with `fuzzy_find`.
- Keep `project_map` shallow, for example `max_depth <= 2`.
- Use recursive excludes such as `third_party/**`, `node_modules/**`, `target/**`, `dist/**`, and `out/**`.
- Use `read_snippets` after search results instead of reading whole files.

### Index storage

The server keeps one index per canonical workspace root.

- Windows: `%LOCALAPPDATA%\codeloupe-mcp\index-v2\<workspace_hash>\`
- Linux/macOS: `$XDG_CACHE_HOME/codeloupe-mcp/index-v2/<workspace_hash>/` or `~/.cache/codeloupe-mcp/index-v2/<workspace_hash>/`
- Custom root: `<codeloupe_mcp_INDEX_DIR>\index-v2\<workspace_hash>\`
- Tantivy sidecar: `<workspace_index_dir>\tantivy-content\`

Workspace roots are dynamic: the server uses client-provided `workspaceFolders`, `roots`, `rootUri`, or `clientInfo.workspaceRoot`; if none are provided, it falls back to the process current directory when that directory looks like a workspace root.

### Shipping

Releases are distributed through two channels:

- **GitHub Releases**: a semver tag such as `v1.0.0` builds native archives/installers for macOS, Linux, and Windows on x64/arm64.
- **npm/npx**: `@ndhkaeru/codeloupe-mcp` publishes a small JavaScript launcher plus native binaries from the matching GitHub Release.

Release checklist:

1. Update versions in `Cargo.toml`, `Cargo.lock`, `packages/npm/package.json`, and `server.json`.
2. Run the checks from `rust.yml`: format, clippy, tests, audit, deny, and typos.
3. Push `main`, then check GitHub Actions for failures.
4. Push a semver tag such as `v1.0.0`.
5. Confirm release and npm publish workflows succeed.
6. Smoke test `npx -y @ndhkaeru/codeloupe-mcp@<version>`.

### Development

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test --release
```

### License

Apache-2.0. See [LICENSE](./LICENSE).