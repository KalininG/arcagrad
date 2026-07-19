-- arcagrad schema baseline, part: series (structure, leaves, series metadata, series FTS).

CREATE TABLE item_series_leaf (
    item_id     INTEGER PRIMARY KEY REFERENCES items(id)  ON DELETE CASCADE,
    series_id   INTEGER NOT NULL    REFERENCES series(id) ON DELETE CASCADE,
    number_sort REAL    NOT NULL,   -- the ordering scalar (parsed or positional)
    number_disp TEXT,               -- 'Vol. 3' / 'Ch. 12.5'; NULL when purely positional
    volume      REAL,               -- parsed volume axis (display / ComicInfo reconcile)
    chapter     REAL                -- parsed chapter axis
);
CREATE TABLE series (
    id         INTEGER PRIMARY KEY,
    kind       TEXT NOT NULL,
    title      TEXT    NOT NULL,
    cover_path TEXT,
    added_at   INTEGER NOT NULL,
    description TEXT,
    -- Denormalized primary creator = MIN over the series' leaves' `sort_creator`, for
    -- the `?sort=creator` keyset; NULL = none (sorts LAST, sentinel). Maintained
    -- by reconcile_series (leaf membership) + repo::reindex_item_tags (a leaf's tags);
    sort_creator TEXT
, folder_path TEXT, title_manual INTEGER NOT NULL DEFAULT 0, description_manual INTEGER NOT NULL DEFAULT 0, description_source TEXT);
CREATE TABLE series_favorites (
    user_id    INTEGER NOT NULL REFERENCES users(id)  ON DELETE CASCADE,
    series_id  INTEGER NOT NULL REFERENCES series(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, series_id)
);
-- Per-user series rating, the series-level analogue of `ratings` (per-item). Same
-- half-star scale: stored 1..10 = 0.5..5.0 stars. Absent row = unrated; clearing
-- DELETEs (0 is never stored). Feeds the series card/detail + `?sort=rating`.
CREATE TABLE series_ratings (
    user_id    INTEGER NOT NULL REFERENCES users(id)  ON DELETE CASCADE,
    series_id  INTEGER NOT NULL REFERENCES series(id) ON DELETE CASCADE,
    value      INTEGER NOT NULL CHECK (value BETWEEN 1 AND 10),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, series_id)
);
CREATE VIRTUAL TABLE series_fts USING fts5(
    title,
    tokenize = 'unicode61 remove_diacritics 2',
    prefix = '2 3' -- indexed 2- and 3-char prefixes for as-you-type queries
);
CREATE TABLE series_sources (
    series_id INTEGER NOT NULL REFERENCES series(id) ON DELETE CASCADE,
    source    TEXT NOT NULL,
    url       TEXT NOT NULL,
    PRIMARY KEY (series_id, source)
);
CREATE TABLE series_tags (
    series_id INTEGER NOT NULL REFERENCES series(id) ON DELETE CASCADE,
    tag_id    INTEGER NOT NULL REFERENCES tags(id)   ON DELETE CASCADE,
    -- Tag qualifier + creator role, mirror item_tags (open strings, 'none' default).
    qualifier TEXT NOT NULL DEFAULT 'none',
    role      TEXT NOT NULL DEFAULT 'none',
    source    TEXT NOT NULL DEFAULT 'manual',
    PRIMARY KEY (series_id, tag_id, qualifier, role)
);
CREATE TABLE series_trackers (
    series_id  INTEGER NOT NULL REFERENCES series(id) ON DELETE CASCADE,
    plugin_id  TEXT NOT NULL,
    reference  TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (series_id, plugin_id)
);
CREATE TABLE series_upcoming (
    id                  INTEGER PRIMARY KEY,
    series_id           INTEGER NOT NULL REFERENCES series(id) ON DELETE CASCADE,
    provider            TEXT NOT NULL,
    provider_release_id TEXT NOT NULL,
    reference_source    TEXT NOT NULL,
    label               TEXT NOT NULL,
    title               TEXT,
    release_date        TEXT NOT NULL,
    date_precision      TEXT NOT NULL DEFAULT 'day',
    date_status         TEXT NOT NULL DEFAULT 'announced',
    formats_json        TEXT NOT NULL DEFAULT '[]',
    media_type          TEXT,
    market              TEXT,
    publisher           TEXT,
    creators_json       TEXT NOT NULL DEFAULT '[]',
    isbn                TEXT,
    url                 TEXT,
    cover_url           TEXT,
    fetched_at          INTEGER NOT NULL,
    UNIQUE(series_id, provider, provider_release_id)
);
CREATE INDEX idx_leaf_series ON item_series_leaf (series_id, number_sort, item_id);
CREATE INDEX idx_series_added  ON series (added_at, id);
CREATE INDEX idx_series_creator ON series (COALESCE(sort_creator, char(1114111)), id);
CREATE UNIQUE INDEX idx_series_folder ON series (folder_path);
CREATE INDEX idx_series_tags_tag ON series_tags (tag_id, series_id);
CREATE INDEX idx_series_title  ON series (title, id);
CREATE INDEX idx_series_trackers_plugin
    ON series_trackers(plugin_id, series_id);
CREATE INDEX idx_series_upcoming_date
    ON series_upcoming(release_date, id);
CREATE INDEX idx_series_upcoming_provider
    ON series_upcoming(provider, series_id);
CREATE TRIGGER series_fts_ad AFTER DELETE ON series BEGIN
    DELETE FROM series_fts WHERE rowid = old.id;
END;
CREATE TRIGGER series_fts_ai AFTER INSERT ON series BEGIN
    INSERT INTO series_fts (rowid, title) VALUES (new.id, new.title);
END;
CREATE TRIGGER series_fts_au AFTER UPDATE OF title ON series BEGIN
    UPDATE series_fts SET title = new.title WHERE rowid = new.id;
END;
