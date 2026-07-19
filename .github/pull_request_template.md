## What & why

<!-- Briefly describe the problem and solution. Link the issue when applicable. -->

Closes #

## Checklist

- [ ] `cargo fmt --all --check` and `cargo clippy -- -D warnings` pass
- [ ] `cargo test` passes (and new behavior has a test pinning it)
- [ ] **Migrations are append-only**: no edits to any existing `migrations/NNNN_*.sql` — schema changes are a NEW numbered file (applied migrations are frozen; editing one breaks every deployed DB)
- [ ] If the web UI changed: `cd web && npm run build` succeeds and the change was checked in a browser. UI changes must include screenshots if non trivial.
- [ ] If the plugin contract changed: additive-only (serde defaults both sides), or `CONTRACT_VERSION` bumped with rationale
