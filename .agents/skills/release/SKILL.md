---
name: release
description: Prepare a codebase-mcp release: bump versions, validate native release assets, publish npm/npx package, optionally publish MCP Registry metadata, and verify GitHub Actions.
---

# Preparing a Release

Use this checklist when cutting a `codebase-mcp` release. Keep the project npm-first for easy `npx` usage, with native binaries produced by `cargo-dist` and no Docker distribution unless explicitly reintroduced.

## 1. Pick the next version

- Inspect the previous release: `gh release list --repo ndhkaeru/codebase-mcp --limit 5`.
- Review user-visible changes since the previous tag: `git log <previous-tag>..HEAD --oneline`.
- Choose a semver version such as `v1.4.2`.

## 2. Bump versioned files

Update all version fields together:

- `Cargo.toml` package version
- `Cargo.lock` root package version
- `packages/npm/package.json` version
- `server.json` top-level `version`
- `server.json` npm package entry version

Keep `packages/npm/package.json#mcpName` equal to `server.json#name`.

## 3. Validate before push

Run the local equivalents of `.github/workflows/rust.yml` before pushing:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --release --locked
cargo audit
cargo deny check advisories bans licenses sources
typos
```

Validate npm package shape:

```bash
node --check packages/npm/bin/codebase-mcp.js
node --check packages/npm/scripts/prepare-npm-assets.js
cd packages/npm && npm pack --dry-run
```

## 4. Push main and check CI

- Commit with a concise semantic message, e.g. `chore: mark v1.4.2`.
- Push `main`.
- Immediately check GitHub Actions with `gh run list --repo ndhkaeru/codebase-mcp --limit 10`.
- Watch the new Rust run with `gh run watch <run-id> --repo ndhkaeru/codebase-mcp --exit-status` and report failures.

## 5. Tag and publish

- Push the semver tag, e.g. `git tag v1.4.2 && git push origin v1.4.2`.
- Confirm the `Release` workflow uploads all six native binary archives.
- Confirm `Publish npm package` waits for those assets, prepares `packages/npm/native/`, and publishes `@ndhkaeru/codebase-mcp`.
- Smoke test: `npx -y @ndhkaeru/codebase-mcp@1.4.2 --version`.

## 6. Optional MCP Registry publish

After npm succeeds, manually dispatch `Publish npm package` for the same tag with `publish_npm=false` and `publish_registry=true`. This validates `server.json`, authenticates via GitHub OIDC, and runs `mcp-publisher publish` without trying to republish an existing npm version.

## 7. Release notes

Write short GitHub release notes focused on user-visible changes:

- `## What's New` for new tools or workflow improvements
- `## Improvements` for behavior, performance, packaging, or docs
- `## Fixes` for bug fixes and security/dependency updates

Do not mention Docker unless it is explicitly restored.
