# codeloupe-mcp npm package

Run the `codeloupe-mcp` stdio MCP server through npm/npx without manually downloading a native binary.

`codeloupe-mcp` lets MCP-capable coding agents explore, search, diff, and safely edit large repositories with bounded, structured tool output instead of whole-file dumps.

## Quick Start

```bash
npx -y @ndhkaeru/codeloupe-mcp@latest
```

## Requirements

- Node.js 18 or newer.
- An MCP client that supports local `stdio` servers.
- No Rust toolchain is needed when using the published npm package.

## MCP Client Config

Most clients accept this shape:

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

Codex CLI config (`~/.codex/config.toml`):

```toml
[mcp_servers.codeloupe-mcp]
command = "npx"
args = ["-y", "@ndhkaeru/codeloupe-mcp@latest"]
```

VS Code MCP config:

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

VS Code CLI example:

```bash
code --add-mcp '{"name":"codeloupe-mcp","command":"npx","args":["-y","@ndhkaeru/codeloupe-mcp@latest"]}'
```

## How It Ships

This package is a small JavaScript launcher plus native `codeloupe-mcp` binaries under `native/<platform>-<arch>/`. Release automation downloads those binaries from the matching GitHub Release before publishing to npm.

For local development, set `codeloupe_mcp_BINARY` to an existing binary path or run from the repository after `cargo build --release`.

See the project README for the full tool list, runtime configuration, and release process.