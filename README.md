<p align="center">
  <img src="client/src-tauri/icons/128x128@2x.png" width="104" alt="Arcagrad" />
</p>

<h1 align="center">Arcagrad</h1>

<p align="center">
  A self-hosted server and reader for your comics, manga, and books.<br />
  Fast at 100k+ items, built in Rust, and shipped as a single Docker image.
</p>

<p align="center">
  <a href="https://arcagrad.com">Website</a> ·
  <a href="https://arcagrad.com/getting-started/introduction/">Docs</a> ·
  <a href="https://arcagrad.com/download/">Download the app</a> ·
  <a href="https://github.com/KalininG/arcagrad/issues">Report a bug</a>
</p>

---

Drop your archives in a folder and Arcagrad turns them into a private, multi-user library — with a built-in reader, full-text search, tags, per-user progress, private recommendations, and plugins that fetch metadata (and browse and download) from external sources.

## Quick start

```bash
docker run -d -p 3000:3000 \
  -v /path/to/your/library:/content \
  -v arcagrad-data:/data \
  ghcr.io/kalining/arcagrad:latest
```

Open **http://localhost:3000** and create your admin account. Organize your library into top-level folders — each becomes a tab. `/content` is your library; **`/data` holds the database and is the only thing you need to back up.**

New to it? Follow the [step-by-step install guide](https://arcagrad.com/getting-started/install/).

## Desktop apps

Native **macOS, Windows, and Linux** apps that connect to your server, with offline reading — or just use the web reader in any browser, no install needed.

<p align="center"><a href="https://arcagrad.com/download/"><strong>Download the desktop app →</strong></a></p>

## What you get

- **One library for everything** — comics, manga, and EPUB books together. Standalone works and multi-volume series are both first-class.
- **Organizes itself** — watches your folders, detects new and moved files, generates covers, and keeps everything searchable, automatically.
- **Real search and tags** — full-text with as-you-type suggestions, namespaced tag filters (`author:`, `source:`, `-exclude`), and per-user blocklists.
- **Discover without leaving the app** — browse an external source's catalog, preview titles, and download them straight into your library through sandboxed plugins.
- **Private recommendations** — "more like this" and a personal shelf, computed locally from your own library. **No telemetry, ever** — Arcagrad makes no connections of its own.
- **Multi-user** — every reader gets their own account, progress, favorites, ratings, and recommendations.

## Documentation

- **[Full documentation →](https://arcagrad.com)** — install, configuration, guides, and administration.
- **Plugins:** [installing and configuration](docs/plugins/README.md) · [authoring your own](docs/plugins/authoring-plugins.md)
- **REST API:** a running server serves interactive docs at **`/api/docs`** (generated from the code, so it can't drift).

## Build from source

libvips is a C dependency, and the web UI is embedded into the binary, so build the frontend first:

```bash
sudo apt-get install -y libvips-dev pkg-config build-essential   # or: brew install vips
cd web && npm install && npm run build && cd ..
python3 scripts/make_fake_content.py                                 # sample archives -> ./content
ARCA_CONTENT_DIR=./content cargo run --release
```

See [CONTRIBUTING.md](CONTRIBUTING.md) before opening a PR.

## License

- **Server** (this repo): [AGPL-3.0](LICENSE) — self-host freely; if you offer a modified Arcagrad as a network service, share your changes.
- **Plugin SDK** ([`plugin-sdk/`](plugin-sdk/)): [MIT](plugin-sdk/LICENSE) — author and distribute plugins under any license; they're not derivative works of the server.
- **Client** ([`client/`](client/)): [MIT](client/LICENSE).

<sub>RAR support uses UnRAR code under its own license (decode-only).</sub>
