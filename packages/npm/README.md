# codebase-mcp npm package

Run the `codebase-mcp` stdio MCP server through npm/npx without manually downloading a native binary:

```bash
npx -y @ndhkaeru/codebase-mcp@latest
```

MCP client config example:

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

This package is a small JavaScript launcher plus native `codebase-mcp` binaries under `native/<platform>-<arch>/`. Release automation downloads those binaries from the matching GitHub Release before publishing to npm.

For local development, set `CODEBASE_MCP_BINARY` to an existing binary path or run from the repository after `cargo build --release`.
