-- Arcagrad schema baseline, part: core (items, item detail, users/auth, per-user state, jobs).

CREATE TABLE api_keys (
    id         INTEGER PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash TEXT    NOT NULL UNIQUE,   -- BLAKE3 hex of the key
    label      TEXT    NOT NULL,          -- user-supplied name
    created_at INTEGER NOT NULL
, last_used INTEGER);
CREATE TABLE cover_hashes (
    url        TEXT PRIMARY KEY,
    -- u64 dHash stored as i64 (same convention as items.phash).
    hash       INTEGER NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE TABLE favorites (
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    item_id    INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, item_id)
);
CREATE TABLE item_chapters (
    item_id     INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    idx         INTEGER NOT NULL, -- 0-based reading order
    number_sort REAL    NOT NULL, -- the chapter number (0.0 for a leading "Front matter" run)
    number_disp TEXT,             -- 'Ch. 1' / 'Ch. 12.5', or NULL for the front-matter run
    title       TEXT,             -- 'Front matter' for the preamble run; NULL otherwise
    start_page  INTEGER NOT NULL, -- 0-based absolute index of the chapter's first page
    page_count  INTEGER NOT NULL, -- pages the chapter spans
    PRIMARY KEY (item_id, idx)
);
CREATE TABLE item_reading_mode (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    item_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    mode    TEXT NOT NULL,
    PRIMARY KEY (user_id, item_id)
);
CREATE TABLE items (
    -- The ONLY identity: an immutable integer surrogate, never derived.
    -- AUTOINCREMENT so a deleted id is NEVER reused.
    id               INTEGER PRIMARY KEY AUTOINCREMENT,

    -- Content equality, two tiers (both hex blake3).
    --   scheme_tag      classifies the hashing scheme, chosen by MAGIC BYTES (never
    --                   extension). Part of the bucket key, so it doubles as the
    --                   hash version: bump the suffix and old items simply never
    --                   bucket with new ones. 'zip-structural-v1' | 'rar-structural-v1'
    --                   | 'bytes-v1'.
    --   structural_hash the cheap, eager, deterministic bucket key.  NON-UNIQUE by design.
    --   deep_hash       expensive, lazy: computed ONLY to break a multi-item bucket tie 
    scheme_tag       TEXT    NOT NULL,
    structural_hash  TEXT    NOT NULL,
    deep_hash        TEXT,

    -- Multi-type axes. `kind` = the top-level folder name `modality` = how it renders 
    kind              TEXT NOT NULL DEFAULT 'uncategorized',  -- neutral fallback; scanner/upload always set it explicitly
    modality          TEXT NOT NULL DEFAULT 'paginated'
        CHECK (modality IN ('paginated', 'reflowable', 'fixed')),
    modality_override TEXT
        CHECK (modality_override IN ('paginated', 'reflowable', 'fixed')),

    -- The physical file. Kept 1:1 on the item for now.
    path             TEXT    NOT NULL UNIQUE,
    size_bytes       INTEGER NOT NULL,
    mtime            INTEGER NOT NULL,            -- unix seconds

    format           TEXT    NOT NULL,            -- 'cbz' | 'zip' | 'cbr' | 'rar'
    -- `title` is the CLEAN display title
    title            TEXT    NOT NULL,
    raw_title        TEXT,
    page_count       INTEGER,                     -- NULL until computed (deferred I/O)
    description      TEXT,
    -- ISBN-10/13 from an EPUB's dc:identifier (compact digits), NULL otherwise.
    isbn             TEXT,
    -- Denormalized primary creator = the alphabetically-first `creator`-namespace tag
    -- value, for the `?sort=creator` keyset; NULL = no creator (sorts LAST via the
    -- COALESCE(char(1114111)) sentinel).
    sort_creator      TEXT,
    added_at         INTEGER NOT NULL,            -- unix seconds
    last_modified_at INTEGER NOT NULL             -- unix seconds; bumped when the file changes
, series_id INTEGER REFERENCES series(id), reading_direction TEXT
    CHECK (reading_direction IN ('ltr', 'rtl')), spread_mode TEXT
    CHECK (spread_mode IN ('single', 'double')), phash INTEGER, chapters_done INTEGER NOT NULL DEFAULT 0, word_count INTEGER, description_manual INTEGER NOT NULL DEFAULT 0, description_source TEXT, series_index REAL, publisher TEXT);
CREATE TABLE jobs (
    id         INTEGER PRIMARY KEY,
    kind       TEXT    NOT NULL,                    -- 'scan' | 'thumbnail_sweep'
    payload    TEXT,                                -- optional JSON
    state      TEXT    NOT NULL DEFAULT 'pending',  -- pending|running|done|failed
    attempts   INTEGER NOT NULL DEFAULT 0,
    -- Backoff scheduling: a pending job is eligible only once `run_after` has
    -- passed, so a failed job requeues with exponential backoff instead of dying.
    run_after  INTEGER NOT NULL DEFAULT 0,
    -- Terminal outcome: a JSON object on success (e.g. {"applied":12} for a
    -- scrape) or {"error":"..."} on permanent failure; NULL while pending/running
    -- (GET /api/jobs/{id}).
    result     TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE ratings (
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    item_id    INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    -- Half-star granularity: stored 1..10 = 0.5..5.0 stars (display value = stored/2).
    value      INTEGER NOT NULL CHECK (value BETWEEN 1 AND 10),
    updated_at INTEGER NOT NULL,            -- unix seconds
    PRIMARY KEY (user_id, item_id)
);
CREATE TABLE read_progress (
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    item_id    INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    unit       TEXT NOT NULL DEFAULT 'page' CHECK (unit IN ('page', 'percent')),
    value      REAL NOT NULL,
    locator    TEXT,                          -- Readium locator JSON; NULL for images
    updated_at INTEGER NOT NULL,              -- unix seconds
    PRIMARY KEY (user_id, item_id)
);
CREATE TABLE reading_activity (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    day     INTEGER NOT NULL,               -- unix day index (updated_at / 86400), UTC
    pages   INTEGER NOT NULL DEFAULT 0,
    updates INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (user_id, day)
);
CREATE TABLE server_settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE sessions (
    token_hash TEXT    PRIMARY KEY,            -- BLAKE3 hex of the cookie token
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL                -- unix seconds
);
CREATE TABLE users (
    id            INTEGER PRIMARY KEY,
    username      TEXT    NOT NULL UNIQUE,
    password_hash TEXT    NOT NULL,            -- Argon2id PHC string
    role          TEXT    NOT NULL DEFAULT 'user',  -- 'admin' | 'user'
    created_at    INTEGER NOT NULL             -- unix seconds
);
CREATE INDEX idx_api_keys_user ON api_keys (user_id);
CREATE INDEX idx_items_added ON items (added_at, id);
CREATE INDEX idx_items_creator ON items (COALESCE(sort_creator, char(1114111)), id);
CREATE INDEX idx_items_bucket ON items (scheme_tag, structural_hash);
CREATE INDEX idx_items_kind ON items (kind, added_at, id);
CREATE INDEX idx_items_oneshot_added     ON items (series_id, added_at, id);
CREATE INDEX idx_items_oneshot_creator    ON items (series_id, COALESCE(sort_creator, char(1114111)), id);
CREATE INDEX idx_items_oneshot_pagecount ON items (series_id, COALESCE(page_count, -1), id);
CREATE INDEX idx_items_oneshot_title     ON items (series_id, title, id);
CREATE INDEX idx_items_pagecount ON items (COALESCE(page_count, -1), id);
CREATE INDEX idx_items_series ON items (series_id);
CREATE INDEX idx_items_title ON items (title, id);
CREATE INDEX idx_jobs_pending ON jobs (id) WHERE state = 'pending';
CREATE INDEX idx_read_progress_item ON read_progress (item_id, user_id, unit);
CREATE INDEX idx_read_progress_recent ON read_progress (user_id, updated_at);
CREATE INDEX idx_sessions_user ON sessions (user_id);
