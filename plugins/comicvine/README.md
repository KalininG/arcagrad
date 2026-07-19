# Comic Vine plugin

Bundled metadata plugin for Western comics. It uses the Comic Vine API and
requires a Comic Vine API key.

## Behavior

- Searches Comic Vine volumes by title when no reference is supplied.
- Accepts Comic Vine volume (`4050`) and issue (`4000`) URLs or handles as exact
  references. A bare numeric ID is treated as a volume ID.
- Maps publishers to `group`, credited creators to `creator`, characters and
  teams to `character`, and concepts to `tag`.
- Excludes credits that are exclusively editorial or translation roles.
- Uses the full description when available and falls back to Comic Vine's short
  deck.
- Records the canonical Comic Vine detail URL.

Long-running volumes can contain very large aggregate lists. Volume metadata is
limited to the most frequently credited 15 characters, 8 creators, 10 concepts,
and 6 teams. Issue metadata uses its direct credits.

## Credentials and contract

Create an API key from [Comic Vine's API page](https://comicvine.gamespot.com/api/),
then enter it in the plugin's `api_key` credential field.

- Manifest version: `1`
- Plugin version: `0.1.0`
- Capabilities: `scrape`
- Allowed host: `comicvine.gamespot.com`
- Required credential: `api_key`
- Rate limit: one request every ten seconds, one request at a time

## Build and test

From this directory:

```sh
cargo test
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
```

The server's `build.rs` builds the WASM module and stages `comicvine.wasm`
automatically.
