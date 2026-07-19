-- arcagrad schema baseline, part: recommendations (neighbour graphs).

CREATE TABLE entry_neighbors (
    src_type TEXT    NOT NULL,   -- 'i' = one-shot item | 's' = series
    src_id   INTEGER NOT NULL,
    dst_type TEXT    NOT NULL,
    dst_id   INTEGER NOT NULL,
    score    REAL    NOT NULL,
    PRIMARY KEY (src_type, src_id, dst_type, dst_id)
) WITHOUT ROWID;
CREATE TABLE item_neighbors (
    item_id     INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    neighbor_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    score       REAL    NOT NULL,
    PRIMARY KEY (item_id, neighbor_id)
) WITHOUT ROWID;
CREATE TABLE recommendation_index_state (
    name         TEXT PRIMARY KEY,
    version      INTEGER NOT NULL,
    completed_at INTEGER NOT NULL
) WITHOUT ROWID;
CREATE INDEX idx_entry_neighbors_src ON entry_neighbors (src_type, src_id, score DESC);
CREATE INDEX idx_neighbors_item ON item_neighbors (item_id, score DESC);
