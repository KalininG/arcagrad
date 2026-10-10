-- Lets the per-kind one-shot count behind page-jump pagination run from the index.
CREATE INDEX idx_items_kind_oneshot ON items (kind, series_id, added_at, id);
