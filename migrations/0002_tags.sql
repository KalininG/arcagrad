-- arcagrad schema baseline, part: tags + full-text search.

CREATE TABLE item_tags (
    item_id    INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    tag_id     INTEGER NOT NULL REFERENCES tags(id)  ON DELETE CASCADE,
    -- Optional per-link tag qualifier: which subject a content tag describes.
    qualifier  TEXT NOT NULL DEFAULT 'none',
    -- Creator-role facet for `creator`-namespace tags (writer / illustrator /
    -- artist / ...). Open string (roles evolve), validated in Rust; 'none' =
    -- untracked. Like qualifier it joins the PK, so one person can hold several
    -- roles on one work.
    role       TEXT NOT NULL DEFAULT 'none',
    -- Provenance for filtering / re-import / debugging.
    source     TEXT NOT NULL DEFAULT 'manual',
    PRIMARY KEY (item_id, tag_id, qualifier, role)
);
CREATE VIRTUAL TABLE items_fts USING fts5(
    title,
    tags,
    tokenize = 'unicode61 remove_diacritics 2',
    prefix = '2 3' -- indexed 2- and 3-char prefixes for as-you-type queries
);
CREATE TABLE tags (
    id        INTEGER PRIMARY KEY,
    namespace TEXT NOT NULL
        CHECK (namespace IN
            ('creator', 'group', 'parody', 'character', 'tag', 'category', 'demographic', 'language')),
    value     TEXT NOT NULL,                 -- normalized: trimmed + lowercased
    UNIQUE (namespace, value)
);
CREATE TABLE user_tag_blocklist (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    tag_id  INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, tag_id)
);
CREATE INDEX idx_item_tags_tag ON item_tags (tag_id, item_id);
CREATE INDEX idx_tags_value ON tags (value);
CREATE TRIGGER items_fts_ad AFTER DELETE ON items BEGIN
    DELETE FROM items_fts WHERE rowid = old.id;
END;
CREATE TRIGGER items_fts_ai AFTER INSERT ON items BEGIN
    INSERT INTO items_fts (rowid, title, tags) VALUES (new.id, new.title, '');
END;
CREATE TRIGGER items_fts_au AFTER UPDATE OF title ON items BEGIN
    UPDATE items_fts SET title = new.title WHERE rowid = new.id;
END;
