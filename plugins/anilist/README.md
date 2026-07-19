# AniList plugin

Bundled metadata plugin for manga, manhwa, manhua, light novels, and one-shots.
It uses AniList's public GraphQL API and does not download media.

## Behavior

- Searches the first ten manga results by title. Reflowable items are restricted
  to AniList's `NOVEL` format.
- Accepts an AniList manga URL or numeric media ID as an optional exact reference.
- Prefers the English title and falls back to the romaji title.
- Maps genres and sufficiently relevant, non-spoiler tags to `tag`.
- Maps tags in a `Demographic*` category to `demographic`.
- Maps story, art, and original-creator staff to `creator`, preserving writer and
  illustrator roles where AniList provides them.
- Cleans the description and records AniList's canonical media URL.
- Does not infer a language from the work's country of origin.

## Contract

- Manifest version: `1`
- Plugin version: `0.1.0`
- Capabilities: `scrape`
- Allowed host: `anilist.co`
- Credentials: none
- Rate limit: 85 requests per minute, one request at a time

## Build and test

From this directory:

```sh
cargo test
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
```

The server's `build.rs` builds the WASM module and stages `anilist.wasm`
automatically. The ignored live API check can be run from the repository root:

```sh
cargo test --test plugins -- --ignored
```
