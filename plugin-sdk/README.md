# Arcagrad plugin SDK

`arcagrad-plugin-sdk` is the shared Rust crate for arcagrad's WASM plugin contract. The server depends on it for the host-side types, and Rust plugins use the same types when reading requests and returning results.

The crate is a workspace member of the main repository. It is not published to crates.io (`publish = false`), so external plugins should use a Git dependency or a checked-out path.

```toml
[dependencies]
arcagrad-plugin-sdk = {
    git = "https://github.com/KalininG/arcagrad",
    features = ["guest"]
}
```

The JSON exchanged with the host remains the actual ABI. A plugin written in another language does not need this crate as long as it implements the same exports and JSON shapes.

## What the crate contains

| Module | Purpose |
|---|---|
| `manifest` | Plugin metadata, credentials, feeds, rate limits, and declared permissions |
| `contract` | Request and response types for scrape, browse, read, download, identify, calendar, and host HTTP calls |
| `guest` | WASM-only wrappers for `http_fetch` and `get_credential` |
| `repo` | The repository index served by a plugin repository |
| `validation` | The same strict manifest and repository checks used by the server |

The crate re-exports these types at its root, so plugin code normally imports from `arcagrad_plugin_sdk` directly.

## Features

The default feature set is empty.

| Feature | Intended user | Adds |
|---|---|---|
| `guest` | Rust WASM plugins | Extism host functions and typed fetch helpers |
| `validation` | Plugin tests, repository tooling, server | Manifest and repository validation |
| `schema` | Server | `utoipa` schema derives for API-facing types |

`guest` is only compiled for `wasm32`. Keep parsing, matching, and mapping code outside the `wasm32` module so it can be exercised with normal host-target unit tests.

## Version fields

There are three independent version numbers:

- `MANIFEST_VERSION` describes the plugin metadata document.
- `CONTRACT_VERSION` describes the runtime request and response ABI.
- `REPO_VERSION` describes a repository's `index.json` format.

A plugin places `MANIFEST_VERSION` and `CONTRACT_VERSION` in its manifest. A repository places `REPO_VERSION` at the top of its index.

Contract changes should be additive whenever possible. New optional fields need Serde defaults so an older plugin can read a newer request and a newer host can read an older response. Renaming a field, removing one, or changing its meaning requires a contract version bump.

## Manifest

Every plugin exports a `manifest` function returning a JSON-encoded `PluginManifest`. Constructing the Rust type makes the required fields visible at compile time; `validate_manifest` checks values and relationships that the type system cannot express.

```rust
use std::collections::BTreeMap;

use arcagrad_plugin_sdk::{
    PluginManifest, RateLimit, RateRule, CONTRACT_VERSION, MANIFEST_VERSION,
};

fn manifest_doc() -> PluginManifest {
    PluginManifest {
        manifest_version: MANIFEST_VERSION,
        id: "example-books".into(),
        version: "0.1.0".into(),
        author: "your-name".into(),
        icon: None,
        repository: Some("https://github.com/you/example-books".into()),
        name: "Example Books".into(),
        description: "Book metadata from example.test.".into(),
        source: "example-books".into(),
        capabilities: vec!["scrape".into()],
        hosts: vec!["example.test".into()],
        auth: None,
        rate_limit: Some(RateLimit {
            rules: vec![RateRule {
                match_pattern: String::new(),
                requests: 30,
                per_ms: 60_000,
            }],
            max_concurrency: 1,
        }),
        feeds: Vec::new(),
        reference_inputs: BTreeMap::new(),
        item_cache_ttl: 0,
        image_headers: BTreeMap::new(),
        clean_titles: true,
        followable: true,
        reading_mode: "paged".into(),
        nsfw: false,
        contract_version: CONTRACT_VERSION,
    }
}
```

Important fields:

- `id` is the installed plugin identity and must be safe as a filename: ASCII letters, digits, `.`, `_`, and `-`.
- `version` is SemVer.
- `source` is the source identity used for stored credentials and source records. Keep it stable after release.
- `capabilities` controls which operations the host may expose.
- `hosts` restricts `http_fetch` to those hostnames. An empty list permits any
  otherwise safe public hostname.
- `auth` describes write-only credential inputs and which capabilities require them.
- `rate_limit` is enforced by the host rather than by each export.
- `feeds` describes the browse tabs supported by the plugin.
- `reference_inputs` supplies labels and help for opaque source references.
- `image_headers` contains headers the server must send while proxying source images.
- `reading_mode` is `paged` or `vertical`.
- `nsfw` lets the plugin store hide the source unless the operator chooses to show it.

Strict manifests require labels and help text for credential fields. A credential may be optional overall while still being required for a particular capability through `AuthSpec::required_for`.

## Capabilities and exports

The manifest declares capabilities; the WASM artifact supplies the corresponding exports.

| Capability | Export | Input | Output |
|---|---|---|---|
| `scrape` | `search` | `ScrapeHint` | `Vec<Candidate>` |
| `scrape` | `fetch_details` | `Candidate` | `ScrapedMetadata` |
| `download` | `download` | `Candidate` | `DownloadPlan` |
| `browse` | `browse` | `BrowseRequest` | `BrowsePage` |
| `browse` | `fetch_details` | `Candidate` | `ScrapedMetadata` |
| `read` | `pages` | JSON string containing an opaque reference | `BrowsePages` |
| `identify` | `identify` | `IdentifyRequest` | `IdentifyResult` |
| `calendar` | `upcoming` | `CalendarRequest` | `CalendarResponse` |

All plugins also export `manifest`. An optional `icon` export returns WebP bytes for the installed-plugin UI.

References and candidate ids are owned by the plugin. The host stores or returns them but does not parse them. A plugin should accept references it emitted in earlier versions unless it deliberately makes a breaking migration.

## Minimal Rust exports

Extism plugin functions accept and return strings. Deserialize at the boundary, keep the source-specific work in ordinary Rust functions, then serialize the contract type.

```rust
#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::*;
    use arcagrad_plugin_sdk::{guest, Candidate, HttpFetchRequest, ScrapeHint, ScrapedMetadata};
    use extism_pdk::*;

    #[plugin_fn]
    pub fn manifest(_: String) -> FnResult<String> {
        Ok(serde_json::to_string(&manifest_doc())?)
    }

    #[plugin_fn]
    pub fn search(input: String) -> FnResult<String> {
        let hint: ScrapeHint = serde_json::from_str(&input)?;
        let url = format!("https://example.test/search?q={}", hint.title);
        let response = guest::fetch(&HttpFetchRequest::get(url))?;

        if response.status != 200 {
            return Err(Error::msg(format!("source returned {}", response.status)));
        }

        let candidates: Vec<Candidate> = parse_search(&response.body);
        Ok(serde_json::to_string(&candidates)?)
    }

    #[plugin_fn]
    pub fn fetch_details(input: String) -> FnResult<String> {
        let candidate: Candidate = serde_json::from_str(&input)?;
        let metadata: ScrapedMetadata = load_metadata(&candidate)?;
        Ok(serde_json::to_string(&metadata)?)
    }
}
```

The exact Extism error conversion used by a plugin can vary. The bundled plugins under `../plugins/` are the working reference implementations.

## Host calls and sandboxing

Plugins do not open sockets directly. With the `guest` feature they can call:

```rust
let response = guest::fetch(&HttpFetchRequest::get("https://example.test/api"))?;
let credential_json = guest::credentials()?;
```

`guest::fetch` passes the request through the server. The server applies the
manifest's hostname policy, blocks unsafe destinations, enforces the plugin's
rate limits, and performs the network request. `HttpFetchResponse.status == 0`
means the request failed at the network layer. A disallowed URL is a policy
error rather than a normal HTTP response.

`guest::credentials` returns a JSON object containing credentials configured for the plugin's declared source, or `{}` when none exist. The host binds the lookup to the active plugin; changing an argument cannot read another source's credentials.

The guest has no general filesystem or socket access. Files are acquired through a `DownloadPlan`, and source images are served through the host proxy.

## Metadata and tags

`ScrapedMetadata` is used both when metadata is applied to a local item and when the UI previews a remote browse result. `mapped_tags` is the data written into the library. `raw_tags` preserves the source shape on the wire but is not currently persisted.

Mapped tag namespaces are closed:

- `creator`
- `group`
- `parody`
- `character`
- `tag`
- `category`
- `demographic`
- `language`

Use `qualifier` for the subject of a content tag and `role` for a creator's contribution. Use `"none"` when either facet does not apply.

Descriptions and comment bodies may contain source markup. The web client sanitizes them before display, but plugins should still return only the content needed for the feature.

## Validation tests

Enable `validation` in development and validate the same manifest returned by the WASM export.

```toml
[dev-dependencies]
arcagrad-plugin-sdk = {
    git = "https://github.com/KalininG/arcagrad",
    features = ["validation"]
}
```

```rust
#[test]
fn manifest_is_valid() {
    let errors = arcagrad_plugin_sdk::validate_manifest(&manifest_doc());
    assert!(errors.is_empty(), "manifest errors: {errors:?}");
}
```

Keep parsers and mapping functions independent of the WASM module. That allows `cargo test` to run without Extism or network access.

For changes to the SDK itself:

```sh
cargo test -p arcagrad-plugin-sdk --all-features
cargo doc -p arcagrad-plugin-sdk --all-features --no-deps
```

## Building a plugin

A plugin crate normally uses both `cdylib` and `rlib` outputs:

```toml
[lib]
crate-type = ["cdylib", "rlib"]
```

Build the artifact with:

```sh
rustup target add wasm32-unknown-unknown
RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=${HOME}=~" \
    cargo build --release --target wasm32-unknown-unknown
```

Path remapping prevents local absolute paths and usernames from appearing in panic-location strings embedded in a distributed WASM file.

## Repository indexes

`RepoIndex` is the JSON document consumed by the plugin store. Each `RepoEntry` flattens a strict `PluginManifest` beside:

- `artifact_url`, either HTTP(S) or relative to the index URL;
- `artifact_hash`, the artifact's BLAKE3 hash in hex;
- optional `icon_data`, a base64-encoded WebP icon.

Use the repository generator instead of editing the index by hand:

```sh
cargo run --bin arca-plugin -- generate-index \
    --dir ./my-repo \
    --name "My plugins" \
    --out ./my-repo/index.json
```

The server validates the index before listing its contents and verifies the artifact hash during installation.

## Related documentation

- [Plugin authoring guide](../docs/plugins/authoring-plugins.md)
- [Installing and configuring plugins](../docs/plugins/README.md)
