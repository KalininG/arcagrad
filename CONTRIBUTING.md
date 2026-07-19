## Before you write code

- **Bugs**: open an issue with the bug form (version, logs, repro).
- **Features / anything beyond ~50 lines**: **open an issue first** and get a
  nod on the approach. Arcagrad has made many decisions that are deliberately settled.
- **Source-specific behavior belongs in a plugin, not the core.** The host is
  deliberately free of site-specific code. If your change is "support site X,"
  write a plugin against [docs/plugins/authoring-plugins.md](docs/plugins/authoring-plugins.md) and publish it yourself — no core PR needed. If there is strong reasoning
  this should be an bundled plugin open an issue first and it will be reviewed.

## Building

libvips is a C dependency, and the SvelteKit SPA is embedded into the binary at
compile time — **frontend first, then cargo**:

```bash
sudo apt-get install -y libvips-dev pkg-config build-essential   # or: brew install vips
cd web && npm install && npm run build && cd ..
python3 scripts/make_fake_content.py                                 # sample archives -> ./content
ARCA_CONTENT_DIR=./content cargo run --release
```

For frontend work, run the Vite dev server against a running backend instead of
rebuilding the embed each time: `cd web && npm run dev` (proxies `/api`).

## Tests & gates

CI requires all of these green — run them locally before pushing:

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test                       # unit + integration
cargo deny check                 # license + advisory audit
```

New behavior needs a test pinning it (unit test for pure logic, or an API
integration test in `tests/api/` driving the real router).

- `cargo test --test plugins -- --ignored` — **live canaries** against real
  vendor APIs for the bundled plugins. Network-dependent by design; never part
  of the PR gate. A red canary means the vendor changed, not that your PR broke.

## The migration rule (please read this one)

**Migrations are append-only. Never edit an existing `migrations/NNNN_*.sql`.**

sqlx stores a checksum of every applied migration and refuses to boot when a
file changed — editing an applied migration bricks every deployed database.
Any schema change is a **new** numbered file, and should be additive 
(`ALTER TABLE ADD COLUMN`, `CREATE TABLE`) so it applies to
a live DB without a drop.

## Other conventions

- **Plugin contract changes are additive-only** (serde defaults on both sides).
  A breaking change bumps `CONTRACT_VERSION` and needs a written rationale —
  it orphans every existing plugin.

## What happens to your PR
CI must pass, and review is required (CODEOWNERS routes everything to the
maintainer). Small focused PRs get reviewed quickly; grab-bag PRs get asked to
split. Squash-merge is the norm, with your PR title as the commit subject.s