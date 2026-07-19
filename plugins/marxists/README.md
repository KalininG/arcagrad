# Marxists Internet Archive plugin

Bundled source plugin for the curated English EPUB catalog at
[marxists.org](https://www.marxists.org/ebooks/index.htm).

## Behavior

- Provides one searchable `Catalog` feed.
- Fetches the catalog page and filters its parsed title and author entries
  locally.
- Returns exact 25-item pages with a known page count.
- Accepts one or more direct `marxists.org` EPUB URLs for download.
- Maps the catalog author to `creator` and sets the language to English.
- Uses the direct EPUB URL as the source URL and downloads the file through the
  server's normal ingest path.

The source catalog does not provide covers or descriptions. Metadata embedded in
the downloaded EPUB is handled later by the server's EPUB ingest.

## Contract

- Manifest version: `1`
- Plugin version: `0.1.0`
- Capabilities: `browse`, `download`
- Feed: `catalog`
- Allowed host: `marxists.org`
- Credentials: none
- Rate limit: one request per second, one request at a time

## Build and test

From this directory:

```sh
cargo test
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
```

The server's `build.rs` builds the WASM module and stages `marxists.wasm`
automatically.
