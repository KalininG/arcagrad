-- arcagrad schema baseline, part: plugins, scraped sources/comments, follows, calendar.

CREATE TABLE calendar_provider_state (
    provider        TEXT PRIMARY KEY,
    source          TEXT NOT NULL,
    last_checked_at INTEGER,
    last_success_at INTEGER,
    last_error      TEXT
);
CREATE TABLE credentials (
    source     TEXT PRIMARY KEY,
    data       TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE follow_seen (
    follow_id INTEGER NOT NULL REFERENCES follows(id) ON DELETE CASCADE,
    reference TEXT NOT NULL,
    state     TEXT NOT NULL DEFAULT 'seen'
              CHECK (state IN ('seen', 'new', 'queued', 'downloaded', 'skipped', 'owned')),
    -- The BrowseItem card JSON captured at discovery, so the review grid renders
    -- without re-hitting the source. NULL for baseline rows (never rendered).
    card_json TEXT,
    seen_at   INTEGER NOT NULL,
    PRIMARY KEY (follow_id, reference)
);
CREATE TABLE follows (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    plugin_id       TEXT NOT NULL,      -- browse plugin (not an FK: plugins live in the registry)
    kind            TEXT NOT NULL,      -- where a download from this follow lands
    feed            TEXT NOT NULL,      -- the plugin feed the query runs against
    query           TEXT NOT NULL,      -- the committed filter ('' = whole feed, refused at the API)
    created_at      INTEGER NOT NULL,
    last_checked_at INTEGER,            -- NULL until the first (baseline) check
    last_error      TEXT,               -- last check failure, shown on the row; NULL = healthy
    UNIQUE (plugin_id, kind, feed, query)
);
CREATE TABLE item_comments (
    item_id     INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    source      TEXT    NOT NULL,        -- which scraper/source (e.g. 'anilist')
    external_id TEXT    NOT NULL,        -- the source's own comment id (stable identity + dedup)
    author      TEXT    NOT NULL,        -- the source-side username (e.g. 'KalininG')
    posted_at   INTEGER,                 -- unix seconds the comment was posted (NULL if unknown)
    score       INTEGER,                 -- the source's signed vote score (e.g. +54; NULL if untracked)
    body        TEXT    NOT NULL,        -- raw comment markup (HTML) so links/formatting survive
    PRIMARY KEY (item_id, source, external_id)
);
CREATE TABLE item_sources (
    item_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    source  TEXT NOT NULL,
    url     TEXT NOT NULL,
    PRIMARY KEY (item_id, source)
);
CREATE TABLE kind_access (
    audience TEXT NOT NULL CHECK (audience IN ('user', 'guest')),
    kind     TEXT NOT NULL,
    PRIMARY KEY (audience, kind)
);
CREATE TABLE plugin_installs (
    plugin_id       TEXT PRIMARY KEY,
    version         TEXT NOT NULL,
    -- blake3 of the installed artifact bytes — the upgrade/refresh check.
    artifact_hash   TEXT NOT NULL,
    installed_at    INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at      INTEGER NOT NULL DEFAULT (unixepoch()),
    -- Set when the installed file fails to load at boot (corrupt/incompatible);
    -- surfaced in the store with a reinstall path. NULL = healthy.
    last_error      TEXT
, origin TEXT NOT NULL DEFAULT 'bundled', repo_url TEXT);
CREATE TABLE plugin_kind_map (
    kind      TEXT NOT NULL,
    plugin_id TEXT NOT NULL,
    -- 1 = also AUTO-run this (enabled) (off by default).
    auto      INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (kind, plugin_id)
);
CREATE TABLE plugin_repos (
    url          TEXT PRIMARY KEY,
    name         TEXT,
    added_at     INTEGER NOT NULL DEFAULT (unixepoch()),
    -- Last successful fetch; NULL until one succeeds.
    last_fetched INTEGER,
    -- Last fetch/parse/validation error; NULL when healthy.
    last_error   TEXT
);
CREATE INDEX idx_follow_seen_state ON follow_seen (follow_id, state);
CREATE INDEX idx_item_comments_item ON item_comments (item_id, posted_at);
