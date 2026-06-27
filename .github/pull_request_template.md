## Summary

- 

## Risk / Security Impact

- [ ] No filesystem write/delete behavior changed
- [ ] No path resolution, sandboxing, or security guard behavior changed
- [ ] No GitHub Actions, release, dependency, or installer behavior changed
- [ ] No MCP tool schema or server instructions changed
- [ ] If any box above is unchecked, I explained the risk and mitigation below

## Validation

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] `cargo test --release --locked`
- [ ] `cargo audit`
- [ ] `cargo deny check advisories bans licenses sources`

## Notes for Reviewers

- Security-sensitive changes include `.github/**`, `Cargo.*`, `deny.toml`, `src/security/**`, write/delete/edit tools, `src/indexer/**`, and `text_search`/content-index tools.
- For PRs from forks, do not approve workflow or dependency changes unless the diff is fully reviewed.
