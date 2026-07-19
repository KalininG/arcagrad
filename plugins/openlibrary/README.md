# Open Library plugin

Bundled metadata plugin for books. It adds subjects and settings from Open
Library without replacing metadata already embedded in an EPUB.

## Behavior

- Searches by display title and primary author when no reference is supplied.
- Requires an exact author match and either an exact title or a title beginning
  with the requested title.
- Accepts Open Library work or edition URLs and IDs, plus ISBN-10 and ISBN-13,
  as optional exact references.
- Resolves edition IDs and ISBNs to their linked work before fetching metadata.
- Normalizes the selected work's subjects, places, and time periods into `tag`
  values.
- Removes broad catalog labels, language labels, call numbers, format labels,
  bracketed qualifiers, and duplicate values.
- Records the canonical Open Library work URL.
- Does not replace the item's title, author, language, description, or cover.

An exact reference that cannot be resolved returns an error instead of falling
back to a possibly unrelated title search.

## Contract

- Manifest version: `1`
- Plugin version: `0.1.0`
- Capabilities: `scrape`
- Allowed host: `openlibrary.org`
- Credentials: none
- Rate limit: one request per second, one request at a time

## Build and test

From this directory:

```sh
cargo test
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
```

The server's `build.rs` builds the WASM module and stages `openlibrary.wasm`
automatically.
