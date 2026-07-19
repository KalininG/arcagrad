//! Shared catalog filters, sorting, and cursor encoding.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

mod catalog;
mod follows;
mod items;
mod plugins;
mod recommend;
mod series;
mod tags;

#[cfg(test)]
pub(crate) mod test_util;

pub use catalog::*;
pub use follows::*;
pub use items::*;
pub use plugins::*;
pub use recommend::*;
pub use series::*;
pub use tags::*;

/// Closed tag namespaces accepted by the database and plugin host.
pub const NAMESPACES: &[&str] = &[
    "creator",
    "group",
    "parody",
    "character",
    "tag",
    "category",
    "demographic",
    "language",
];

pub fn valid_namespace(namespace: &str) -> bool {
    NAMESPACES.contains(&norm(namespace).as_str())
}

fn norm(s: &str) -> String {
    s.trim().to_lowercase()
}

pub struct TagFilter {
    pub tag_ids: Vec<i64>,
    pub match_all: bool,
}

/// Independent filters shared by catalog listing and count queries.
#[derive(Default)]
pub struct ListFilters {
    pub tags: Option<TagFilter>,
    pub exclude_tags: Vec<i64>,
    pub search: Option<String>,
    pub kind: Option<String>,
    pub favorited: Option<bool>,
    pub untagged: Option<bool>,
    pub completed: Option<bool>,
    pub deny_kinds: Vec<String>,
}

fn push_kind_deny(sql: &mut String, col: &str, n: usize) {
    let placeholders = vec!["?"; n].join(", ");
    sql.push_str(&format!(" AND {col} NOT IN ({placeholders})"));
}

const UNTAGGED_TRUE: &str = " AND NOT EXISTS (SELECT 1 FROM item_tags it WHERE it.item_id = a.id)";

const UNTAGGED_FALSE: &str = " AND EXISTS (SELECT 1 FROM item_tags it WHERE it.item_id = a.id)";

const COMPLETED_TRUE: &str = " AND a.page_count > 0 AND rp.value >= a.page_count - 1";

// Explicit NULL arms keep unread items in the unfinished set.
const COMPLETED_FALSE: &str =
    " AND (rp.value IS NULL OR a.page_count IS NULL OR rp.value < a.page_count - 1)";

fn push_tag_filter(sql: &mut String, f: &TagFilter) {
    let placeholders = vec!["?"; f.tag_ids.len()].join(", ");
    if f.match_all {
        sql.push_str(&format!(
            " AND a.id IN (SELECT item_id FROM item_tags WHERE tag_id IN ({placeholders}) \
               GROUP BY item_id HAVING COUNT(DISTINCT tag_id) = ?)"
        ));
    } else {
        sql.push_str(&format!(
            " AND a.id IN (SELECT item_id FROM item_tags WHERE tag_id IN ({placeholders}))"
        ));
    }
}

const SEARCH_CLAUSE: &str = " AND a.id IN (SELECT rowid FROM items_fts WHERE items_fts MATCH ?)";

/// Series-level tags combined with tags from every leaf.
const SERIES_EFFECTIVE_TAGS: &str = "SELECT series_id, tag_id FROM series_tags \
     UNION \
     SELECT l.series_id, it.tag_id FROM item_series_leaf l \
       JOIN item_tags it ON it.item_id = l.item_id";

fn push_series_tag_filter(sql: &mut String, f: &TagFilter) {
    let placeholders = vec!["?"; f.tag_ids.len()].join(", ");
    if f.match_all {
        sql.push_str(&format!(
            " AND s.id IN (SELECT series_id FROM ({SERIES_EFFECTIVE_TAGS}) \
               WHERE tag_id IN ({placeholders}) \
               GROUP BY series_id HAVING COUNT(DISTINCT tag_id) = ?)"
        ));
    } else {
        sql.push_str(&format!(
            " AND s.id IN (SELECT series_id FROM ({SERIES_EFFECTIVE_TAGS}) \
               WHERE tag_id IN ({placeholders}))"
        ));
    }
}

fn push_tag_exclude(sql: &mut String, n: usize) {
    let placeholders = vec!["?"; n].join(", ");
    sql.push_str(&format!(
        " AND a.id NOT IN (SELECT item_id FROM item_tags WHERE tag_id IN ({placeholders}))"
    ));
}

fn push_series_tag_exclude(sql: &mut String, n: usize) {
    let placeholders = vec!["?"; n].join(", ");
    sql.push_str(&format!(
        " AND s.id NOT IN (SELECT series_id FROM ({SERIES_EFFECTIVE_TAGS}) \
           WHERE tag_id IN ({placeholders}))"
    ));
}

/// Matches leaf or series title and therefore requires two query binds.
const SERIES_SEARCH_CLAUSE: &str = " AND (s.id IN (SELECT l.series_id FROM item_series_leaf l \
       WHERE l.item_id IN (SELECT rowid FROM items_fts WHERE items_fts MATCH ?)) \
       OR s.id IN (SELECT rowid FROM series_fts WHERE series_fts MATCH ?))";

fn series_untagged_clause(untagged: bool) -> String {
    let op = if untagged { "NOT IN" } else { "IN" };
    format!(" AND s.id {op} (SELECT series_id FROM ({SERIES_EFFECTIVE_TAGS}))")
}

/// Must match the SQL `char(1114111)` creator-sort sentinel.
const CREATOR_SENTINEL: &str = "\u{10FFFF}";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortField {
    AddedAt,
    Title,
    PageCount,
    Creator,
    /// Viewer rating in half-star units (1–10), with unrated entries last.
    Rating,
}

impl SortField {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "added_at" => Some(Self::AddedAt),
            "title" => Some(Self::Title),
            "page_count" => Some(Self::PageCount),
            "creator" => Some(Self::Creator),
            "rating" => Some(Self::Rating),
            _ => None,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::AddedAt => "added_at",
            Self::Title => "title",
            Self::PageCount => "page_count",
            Self::Creator => "creator",
            Self::Rating => "rating",
        }
    }
    fn is_text(self) -> bool {
        matches!(self, Self::Title | Self::Creator)
    }
    pub fn default_descending(self) -> bool {
        !matches!(self, Self::Title | Self::Creator)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Sort {
    pub field: SortField,
    pub descending: bool,
}

impl Default for Sort {
    fn default() -> Self {
        Sort {
            field: SortField::AddedAt,
            descending: true,
        }
    }
}

impl Sort {
    pub fn signature(self) -> String {
        format!(
            "{}:{}",
            self.field.as_str(),
            if self.descending { "desc" } else { "asc" }
        )
    }
}

/// Decoded keyset cursor. `typ` identifies mixed-catalog entry boundaries.
pub struct Cursor {
    pub sort: String,
    pub value: String,
    pub id: i64,
    pub typ: Option<String>,
}

/// Encode an item keyset boundary as URL-safe JSON.
pub fn encode_cursor(sort: &Sort, value: &str, id: i64) -> String {
    let payload = format!(
        "{{\"s\":{},\"v\":{},\"i\":{id}}}",
        json_str(&sort.signature()),
        json_str(value)
    );
    URL_SAFE_NO_PAD.encode(payload)
}

/// Encode a mixed-catalog keyset boundary including its entry type.
pub fn encode_catalog_cursor(sort: &Sort, value: &str, typ: &str, id: i64) -> String {
    let payload = format!(
        "{{\"s\":{},\"v\":{},\"t\":{},\"i\":{id}}}",
        json_str(&sort.signature()),
        json_str(value),
        json_str(typ)
    );
    URL_SAFE_NO_PAD.encode(payload)
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Decode a cursor, returning none for malformed input.
pub fn decode_cursor(cursor: &str) -> Option<Cursor> {
    let bytes = URL_SAFE_NO_PAD.decode(cursor).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    Some(Cursor {
        sort: v.get("s")?.as_str()?.to_string(),
        value: v.get("v")?.as_str()?.to_string(),
        id: v.get("i")?.as_i64()?,
        typ: v.get("t").and_then(|t| t.as_str()).map(str::to_string),
    })
}

const SERIES_READ_COUNT_SUBQ: &str = "(SELECT COUNT(*) FROM item_series_leaf l \
     JOIN items i ON i.id = l.item_id \
     LEFT JOIN read_progress rp ON rp.item_id = l.item_id AND rp.user_id = ? AND rp.unit = 'page' \
     WHERE l.series_id = s.id AND i.page_count > 0 AND rp.value >= i.page_count - 1)";

const SERIES_COVER_ITEM_SUBQ: &str = "(SELECT l.item_id FROM item_series_leaf l \
     WHERE l.series_id = s.id ORDER BY l.number_sort, l.item_id LIMIT 1)";

const SERIES_COVER_VERSION_SUBQ: &str = "(SELECT i.structural_hash \
     FROM item_series_leaf l JOIN items i ON i.id = l.item_id \
     WHERE l.series_id = s.id ORDER BY l.number_sort, l.item_id LIMIT 1)";

// The NULL-safe predicate counts never-opened leaves as unread.
const SERIES_HAS_UNREAD_LEAF: &str = "SELECT DISTINCT l.series_id FROM item_series_leaf l \
     JOIN items i ON i.id = l.item_id \
     LEFT JOIN read_progress rp ON rp.item_id = l.item_id AND rp.user_id = ? AND rp.unit = 'page' \
     WHERE rp.value IS NULL OR i.page_count IS NULL OR rp.value < i.page_count - 1";

fn series_completed_clause(completed: bool) -> String {
    let op = if completed { "NOT IN" } else { "IN" };
    format!(" AND s.id {op} ({SERIES_HAS_UNREAD_LEAF})")
}

const ITEM_COMPLETED_SET: &str = "SELECT rp.item_id FROM read_progress rp \
     JOIN items i2 ON i2.id = rp.item_id \
     WHERE rp.user_id = ? AND rp.unit = 'page' AND i2.page_count > 0 AND rp.value >= i2.page_count - 1";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_roundtrips() {
        let c = encode_cursor(&Sort::default(), "1700000000", 42);
        let d = decode_cursor(&c).unwrap();
        assert_eq!(d.sort, "added_at:desc");
        assert_eq!(d.value, "1700000000");
        assert_eq!(d.id, 42);

        let title_sort = Sort {
            field: SortField::Title,
            descending: false,
        };
        let c = encode_cursor(&title_sort, "Vol. 2: \"finale\"", 7);
        let d = decode_cursor(&c).unwrap();
        assert_eq!(d.sort, "title:asc");
        assert_eq!(d.value, "Vol. 2: \"finale\"");
        assert_eq!(d.id, 7);
    }

    #[test]
    fn bad_cursor_is_rejected() {
        assert!(decode_cursor("!!!not-base64!!!").is_none());
        assert!(decode_cursor(&URL_SAFE_NO_PAD.encode("not-json")).is_none());
        assert!(decode_cursor(&URL_SAFE_NO_PAD.encode(r#"{"v":"x"}"#)).is_none());
    }
}
