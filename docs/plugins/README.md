# Plugins

Arcagrad plugins are sandboxed WebAssembly modules that add source-specific
metadata, catalog browsing, downloads, remote reading, identification, or
release calendars. The server owns storage, authentication, jobs, networking,
rate limits, and library writes.

This directory has two guides:

- This page covers installation and administration.
- [Authoring plugins](authoring-plugins.md) covers the SDK and publishing flow.

The generated API documentation at `/api/docs` is the source of truth for HTTP
request and response shapes. The [plugin SDK README](../../plugin-sdk/README.md)
is the source of truth for the WASM contract and Rust types.

## Bundled plugins

Bundled plugins are embedded in the server but are not installed or enabled by
default.

| Plugin | Capabilities | Credential |
|---|---|---|
| AniList | metadata | none |
| Comic Vine | metadata | API key |
| Open Library | metadata | none |
| Project Gutenberg | browse and download | none |
| Marxists Internet Archive | browse and download | none |
| VIZ | release calendar | none |

Each bundled plugin has a README beside its source under [`plugins/`](../../plugins/).
Those files document its exact references, mappings, hosts, and rate limit.

## Install and enable

Open **Plugins** as an administrator. The store has three views:

- **Installed** shows running plugins and available updates.
- **Discover** shows bundled and repository plugins that can be installed.
- **Repositories** manages external plugin indexes.

Installation hot-loads a plugin without restarting the server. Uninstalling it
keeps its credentials and per-kind settings so they are available after a
reinstall.

An installed plugin is still disabled for every library kind. Open its detail
page and enable it for the kinds where it should run. **Auto** is a subset of
enabled plugins and runs metadata scraping when the file watcher adds a new item
of that kind.

Kinds are user-defined top-level folder names. Plugins therefore do not declare
which kind names they support; the administrator owns that mapping.

The store hides entries whose manifest has `nsfw: true` until the user enables
the **Show NSFW sources** option. This is a visibility preference, not an
additional sandbox permission.

## Credentials

A plugin manifest describes the fields its configuration form needs. Values are
stored under the manifest's `source`, so plugins with the same source share the
same credential record.

Credential values are available only to administrators when saving them and to
the active plugin through the host call. The API lists configured field names
but never returns saved values. Values are stored in the server database without
application-level encryption, so protect and back up the data volume as a secret
store.

Only install a third-party plugin if you trust it with credentials stored for
its declared source.

## Plugin origins

The server assigns one of three origins; a plugin cannot choose its own:

- `bundled`: embedded in the Arcagrad binary and installed from Discover.
- `local`: uploaded through **Install from file** or loaded as a loose `.wasm`
  from `<data>/plugins/` at startup.
- `community`: installed from a configured repository.

Bundled plugin IDs are reserved. Managed installs take precedence over loose
files with the same ID.

## Repositories and updates

A repository is an HTTP(S) URL serving an Arcagrad plugin `index.json`. The
server validates the index, verifies each artifact's BLAKE3 hash, loads the
artifact's own manifest, and checks that it does not declare a different ID,
version, extra capability, or extra host.

Repository indexes refresh at startup, daily at 03:00 server-local time, and
when an administrator chooses **Check for updates**. Updates remain explicit;
the store shows the newer version and the administrator installs it.

Removing a repository also uninstalls plugins installed from it. Credentials
and per-kind settings remain.

Repository and artifact URLs must resolve to public addresses by default. Set
`ARCA_ALLOW_PRIVATE_REPOS=1` only when an administrator needs to use a repository
on a trusted private network. This exception applies to repository traffic, not
to plugin HTTP requests.

Create an index with the repository tool:

```sh
cargo run --bin arca-plugin -- generate-index \
  --dir ./plugin-repo \
  --name "My plugins" \
  --out ./plugin-repo/index.json
```

Artifact URLs in the index may be relative to the index URL, which makes the
directory suitable for any static file server.

## Security model

A plugin has no general filesystem or socket access. It asks the host to perform
HTTP requests, and the host applies:

- the manifest's hostname list (an empty list permits any otherwise safe public
  destination);
- private, loopback, link-local, and metadata-address blocking;
- the manifest's rate and concurrency limits;
- the active plugin's source-bound credential lookup.

Downloads are returned as plans and pass through the server's normal ingest and
deduplication path. Remote images are fetched through the authenticated image
proxy and must also match the plugin's host allowlist.

The sandbox limits access, but it does not establish that a plugin behaves as
advertised. Review the source and manifest before installing code from outside
the bundled set.

## API map

Most users should use the web UI. Client developers should discover plugins and
feeds from the server rather than hardcoding IDs.

| Area | Main endpoints |
|---|---|
| Running plugins | `GET /api/plugins` |
| Store | `GET /api/plugin-catalog`, `/api/plugin-installs*` |
| Repositories | `/api/plugin-repos*` |
| Per-kind settings | `GET/PUT /api/kinds/{kind}/plugins` |
| Credentials | `GET/PUT/DELETE /api/credentials*` |
| Metadata | `POST /api/items/{id}/scrape`, `POST /api/series/{id}/scrape` |
| Browse and preview | `/api/plugins/{id}/browse`, `/item`, `/image`, `/pages` |
| Download | `POST /api/plugins/{id}/download` |
| Identification | `POST /api/items/{id}/identify` |

Authentication, request parameters, response types, and status codes are kept in
the generated OpenAPI documentation at `/api/docs` and `/api/openapi.json`.
