# VIZ plugin

Bundled calendar plugin for tracking announced releases from VIZ series pages.

## Behavior

- Accepts a canonical VIZ series URL such as
  `https://www.viz.com/spy-x-family`.
- Also accepts a Shonen Jump chapters URL such as
  `https://www.viz.com/shonenjump/chapters/chainsaw-man` and normalizes it to
  the canonical series URL.
- Finds products marked as preorders on the linked series page.
- Reads each product page for its release date, title and volume label, formats,
  category, creators, ISBN-13, product URL, and cover.
- Returns only releases inside the calendar request's date window.
- Uses stable release IDs based on the VIZ product ID and defaults the market to
  `en-US` when the request does not provide one.

A series with no current preorders returns a successful empty result. An invalid
reference or failed series/product request is returned as a per-reference error,
allowing the server to keep previously stored calendar rows.

## Contract

- Manifest version: `1`
- Plugin version: `0.1.0`
- Capability: `calendar`
- Allowed hosts: `viz.com`, `dw9to29mmj727.cloudfront.net`
- Credentials: none
- Rate limit: one request per second, one request at a time

The CloudFront host is used for VIZ product covers. Browsers receive those covers
through the server's authenticated plugin-image proxy.

## Build and test

From this directory:

```sh
cargo test
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
```

The server's `build.rs` builds the WASM module and stages `viz.wasm`
automatically.
