# Authoring plugins

Plugins are standalone WebAssembly modules loaded through Extism. Rust plugins
can share the server's contract types through `arcagrad-plugin-sdk`; other
languages can implement the same JSON ABI directly.

Start with the [SDK reference](../../plugin-sdk/README.md) and one of the
[bundled plugins](../../plugins/). The SDK reference owns the complete request
and response fields. This guide focuses on the choices a plugin author must
make.

## Crate setup

A Rust plugin is a standalone crate, not a member of the Arcagrad workspace.

```toml
[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
arcagrad-plugin-sdk = {
  git = "https://github.com/KalininG/arcagrad",
  features = ["guest"]
}
extism-pdk = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"

[dev-dependencies]
arcagrad-plugin-sdk = {
  git = "https://github.com/KalininG/arcagrad",
  features = ["validation"]
}
```

Keep parsing and mapping functions outside the `wasm32` export module. They can
then run as normal unit tests without loading Extism or making network requests.

## Manifest

Export `manifest` and return a JSON-encoded `PluginManifest`. Constructing the
SDK type catches missing fields at compile time; `validate_manifest` checks
SemVer, IDs, capabilities, hosts, feed relationships, credential descriptions,
and other value rules.

| Field | Purpose |
|---|---|
| `manifest_version` | Metadata schema version; use `MANIFEST_VERSION`. |
| `contract_version` | Runtime ABI version; use `CONTRACT_VERSION`. |
| `id` | Stable install identity and artifact filename stem. |
| `version` | Plugin release in SemVer form. |
| `author` | Maintainer shown in the store. |
| `name`, `description` | Store and selector copy. |
| `icon` | Optional HTTP(S) or root-relative icon URL; an `icon` export can provide bundled WebP bytes instead. |
| `repository` | Optional HTTP(S) source-code URL. |
| `source` | Stable identity for credentials and source records. |
| `capabilities` | Operations implemented by the artifact. |
| `hosts` | Hostnames allowed for plugin HTTP requests and proxied images. |
| `auth` | Credential fields and the capabilities that require them. |
| `rate_limit` | Host-enforced request windows and concurrency. |
| `feeds` | Browse tabs and their query, range, authentication, and cache behavior. |
| `reference_inputs` | UI labels and help for opaque references by capability. |
| `item_cache_ttl` | Cache lifetime for remote item details and page lists. |
| `image_headers` | Headers applied by the server's image proxy. |
| `clean_titles` | Whether the host should clean filename-style titles before matching. |
| `followable` | Whether users can follow searches from this source. |
| `reading_mode` | `paged` or `vertical`. |
| `nsfw` | Visibility flag used to hide the plugin unless the user opts to show it. |

Keep `id` and `source` stable after release. IDs may contain ASCII letters,
digits, `.`, `_`, and `-`. Credentials are shared by plugins that declare the
same `source`, so use a source identifier only when that sharing is intentional.

The manifest is also the permissions screen. Declare every host the plugin or
image proxy needs, the narrowest capabilities it implements, and realistic rate
limits. Do not add hosts for possible future behavior.

## Capabilities

Every plugin exports `manifest`. It may also export `icon`, which returns WebP
bytes. Declared capabilities require these ABI exports:

| Capability | Exports | Purpose |
|---|---|---|
| `scrape` | `search`, `fetch_details` | Match and enrich an existing item or series. |
| `browse` | `browse`, `fetch_details` | List feeds and preview a remote item. |
| `download` | `download` | Return a `DownloadPlan` for server-side ingest. |
| `read` | `pages` | Return ordered remote page URLs. |
| `identify` | `identify` | Match an existing item from its first-page hash and hints. |
| `calendar` | `upcoming` | Return releases for linked series references. |

References and candidate IDs are opaque to the host. Treat values emitted by an
older plugin version as part of your compatibility surface.

### Metadata

`search` receives a `ScrapeHint` and returns ranked `Candidate` values.
`fetch_details` returns `ScrapedMetadata`. Prefer exact references when the user
supplies one; do not silently turn a failed exact reference into an unrelated
title match.

Only `mapped_tags` are written into the library. Use the closed namespaces and
facet rules documented by the SDK. Preserve a canonical `source_url` when the
source provides one so Arcagrad can show provenance and detect an exact library
match.

### Browse, read, and download

Declare browse tabs in `feeds`. `browse` receives the chosen feed, optional
query/range, and a 1-based page number. It returns card-shaped `BrowseItem`
values and an optional total page count.

Return full image URLs. The client sends them through
`/api/plugins/{id}/image`; the host checks the manifest allowlist and applies
`image_headers`. Feed responses use each feed's `cache_ttl`, while item previews
and page lists use `item_cache_ttl`.

A browse item's `reference` should be accepted by `fetch_details`, `pages`, and
`download` where those capabilities are declared. `DownloadPlan` supplies a
direct file URL, safe filename, optional headers, and metadata. The host performs
the download and library ingest.

### Credentials

Describe each field with `name`, `label`, `secret`, `required`, and `help`.
`required_for` must name capabilities declared by the same manifest. Optional
`setup` text can tell the administrator where to obtain the value.

In a Rust guest, `guest::credentials()` returns the active plugin's source-bound
credential object as JSON, or `{}` when nothing is configured. Check required
values at the start of the operation and return a clear error when they are
missing. Never place credentials in URLs, errors, logs, metadata, or cache keys.

## Host networking

Plugins do not open sockets directly. Use `guest::fetch` with an
`HttpFetchRequest`. The host validates the hostname and resolved address, applies
the declared rate limit, and returns an `HttpFetchResponse`.

Handle non-success status codes explicitly. Keep request timeouts and retries in
mind when selecting a rate limit; the host does not reinterpret a source error
as a successful empty result.

## Minimal exports

```rust
#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::*;
    use arcagrad_plugin_sdk::{guest, Candidate, HttpFetchRequest, ScrapeHint};
    use extism_pdk::*;

    #[plugin_fn]
    pub fn manifest(_: String) -> FnResult<String> {
        Ok(serde_json::to_string(&manifest_doc())?)
    }

    #[plugin_fn]
    pub fn search(input: String) -> FnResult<String> {
        let hint: ScrapeHint = serde_json::from_str(&input)?;
        let request = HttpFetchRequest::get(search_url(&hint.title));
        let response = guest::fetch(&request)?;
        let candidates: Vec<Candidate> = parse_search(&response.body);
        Ok(serde_json::to_string(&candidates)?)
    }

    #[plugin_fn]
    pub fn fetch_details(input: String) -> FnResult<String> {
        let candidate: Candidate = serde_json::from_str(&input)?;
        Ok(serde_json::to_string(&load_metadata(&candidate)?)?)
    }
}
```

The bundled plugins show complete manifests, icons, error handling, requests,
and mappings for each supported capability.

## Test and build

Validate the exact manifest returned by the export:

```rust
#[test]
fn manifest_is_valid() {
    let errors = arcagrad_plugin_sdk::validate_manifest(&manifest_doc());
    assert!(errors.is_empty(), "manifest errors: {errors:?}");
}
```

Build and inspect the artifact:

```sh
cargo test
rustup target add wasm32-unknown-unknown
RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=${HOME}=~" \
  cargo build --release --target wasm32-unknown-unknown
strings target/wasm32-unknown-unknown/release/my_plugin.wasm | grep "$USER"
```

The final command should produce no output. Path remapping prevents local paths
and usernames in dependency panic locations from being embedded in a shared
artifact.

Before release, verify that:

- host-target unit tests cover parsing, matching, and mapping;
- the strict manifest validator passes;
- the WASM artifact loads through **Install from file**;
- every requested hostname is declared and no unused hostname remains;
- credential values cannot appear in errors or returned data;
- the version changed when behavior changed;
- the plugin README matches the manifest and current exports.

## Publish a repository

Put one or more `.wasm` files in a directory and generate an index:

```sh
cargo run --bin arca-plugin -- generate-index \
  --dir ./plugin-repo \
  --name "My plugins" \
  --out ./plugin-repo/index.json
```

Serve the directory over HTTP(S). The generator reads each artifact's manifest,
computes its BLAKE3 hash, and emits relative artifact URLs. Do not edit IDs,
versions, permissions, or hashes in the generated index by hand.

The server validates the index at discovery and verifies the artifact again at
installation. See the [operator guide](README.md) for repository behavior.
