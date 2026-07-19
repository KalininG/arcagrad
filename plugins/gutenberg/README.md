# Project Gutenberg plugin

Bundled source plugin for browsing and downloading public-domain EPUB books from
Project Gutenberg's OPDS catalog.

## Behavior

- Provides searchable `Popular` and `Recent` feeds.
- Displays the catalog title, author, source URL, and a Gutenberg cover URL.
- Accepts Gutenberg book IDs or ebook URLs as download references.
- Reads each book's OPDS record for authors, LCSH subjects, language, summary,
  cover, download count, and acquisition links.
- Maps authors to `creator`, normalized LCSH values to `tag`, and the catalog
  language to `language`.
- Prefers an image-bearing EPUB 3 file, then an image-bearing EPUB, then the
  first EPUB acquisition link.
- Downloads the selected file as an EPUB with the parsed metadata attached.

The search feed uses a 25-item stride but can return extra entries, so browse
results are truncated to 25 items and use open-ended pagination.

## Contract

- Manifest version: `1`
- Plugin version: `0.1.0`
- Capabilities: `browse`, `download`
- Feeds: `popular`, `recent`
- Allowed host: `gutenberg.org`
- Credentials: none
- Rate limit: 60 requests per minute, up to two concurrent requests

## Build and test

From this directory:

```sh
cargo test
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
```

The server's `build.rs` builds the WASM module and stages `gutenberg.wasm`
automatically.
