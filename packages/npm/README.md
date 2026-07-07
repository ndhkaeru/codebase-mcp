# codebase-mcp npm wrapper

Run the `codebase-mcp` stdio MCP server through npm/npx:

```bash
npx -y codebase-mcp@latest
```

MCP client config example:

```json
{
  "mcpServers": {
    "codebase-mcp": {
      "command": "npx",
      "args": ["-y", "codebase-mcp@latest"]
    }
  }
}
```

The package expects platform binaries under `native/<platform>-<arch>/`. Release automation fills these from GitHub Release artifacts before publishing. For local development, set `CODEBASE_MCP_BINARY` to an existing binary path or run from the repository after `cargo build --release`.
