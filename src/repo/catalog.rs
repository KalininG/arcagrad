//! Mixed item and series catalog queries.

use anyhow::Result;
use serde::Serialize;
use sqlx::{AssertSqlSafe, SqlitePool};
use std::collections::{HashMap, HashSet};
use utoipa::ToSchema;

use super::*;

/// One item listing entry.
#[derive(Serialize, ToSchema)]
pub struct ItemEntry {
    pub id: i64,
    /// Open catalog grouping derived from the top-level folder.
    pub kind: String,
    /// Effective rendering modality after applying any override.
    pub modality: String,
    pub name: String,
    pub page_count: Option<i64>,
    pub added_at: i64,
    /// The viewer's last-read page, zero-based.
    pub progress: Option<i64>,
    pub favorited: bool,
    /// The viewing user's half-star rating (1–10 = 0.5–5.0 stars), omitted when unrated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<i64>,
    /// Structural hash used as the thumbnail `?v=` cache version.
    pub cover_version: String,
    pub tags: Vec<ItemTag>,
    #[serde(skip)]
    pub sort_creator: Option<String>,
}

/// A one-shot item or collapsed series catalog card.
#[derive(Serialize, ToSchema)]
pub struct CatalogEntry {
    /// `"item"` or `"series"`: the discriminator the client routes on.
    #[serde(rename = "type")]
    pub kind_of: String,
    /// Item id (`/api/items/{id}`) or series id (`/api/series/{id}`), per `type`.
    pub id: i64,
    pub kind: String,
    pub modality: String,
    pub name: String,
    pub page_count: Option<i64>,
    pub added_at: i64,
    pub progress: Option<i64>,
    pub favorited: bool,
    /// The viewer's half-star rating (1–10), omitted when unrated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<i64>,
    pub tags: Vec<ItemTag>,
    /// Series-only: number of leaves in the series.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leaf_count: Option<i64>,
    /// Series-only number of leaves the viewer has finished.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_count: Option<i64>,
    /// Series-only item id used for the cover.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_item_id: Option<i64>,
    /// Structural hash used as the cover thumbnail's `?v=` cache version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_version: Option<String>,
    #[serde(skip)]
    pub sort_creator: Option<String>,
}

/// A catalog page. Count and page fields are present only for page-jump mode.
#[derive(Serialize, ToSchema)]
pub struct ListResult {
    pub items: Vec<CatalogEntry>,
    /// Cursor toward newer items (Prev). `null` at the newest end.
    pub prev_cursor: Option<String>,
    /// Cursor toward older items (Next). `null` at the oldest end.
    pub next_cursor: Option<String>,
    /// Total matching items, page-jump mode only. Drives "page X of Y".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
    /// 1-based page actually returned (clamped to the last page), jump mode only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<i64>,
    /// `ceil(total/limit)`, jump mode only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_count: Option<i64>,
}

impl ListResult {
    /// Return an empty page with no cursors.
    pub fn empty() -> Self {
        ListResult {
            items: Vec::new(),
            prev_cursor: None,
            next_cursor: None,
            total: None,
            page: None,
            page_count: None,
        }
    }
}

#[derive(sqlx::FromRow)]
struct ListRow {
    id: i64,
    kind: String,
    modality: String,
    title: String,
    page_count: Option<i64>,
    added_at: i64,
    progress: Option<i64>,
    favorited: bool,
    sort_creator: Option<String>,
    structural_hash: String,
    rating: Option<i64>,
}

/// Catalog navigation with a `(value, type, id)` keyset boundary.
pub enum CatalogSeek {
    First,
    After { value: String, typ: String, id: i64 },
    Before { value: String, typ: String, id: i64 },
    Last,
    Offset(i64),
}

/// Resolve seek direction, boundary, and offset.
fn catalog_seek_plan(seek: &CatalogSeek, sort: Sort) -> (bool, Option<(String, String, i64)>, i64) {
    let fwd = sort.descending;
    match seek {
        CatalogSeek::First => (fwd, None, 0),
        CatalogSeek::After { value, typ, id } => (fwd, Some((value.clone(), typ.clone(), *id)), 0),
        CatalogSeek::Offset(o) => (fwd, None, (*o).max(0)),
        CatalogSeek::Before { value, typ, id } => {
            (!fwd, Some((value.clone(), typ.clone(), *id)), 0)
        }
        CatalogSeek::Last => (!fwd, None, 0),
    }
}

/// Encode the SQL sort value used in a card's cursor.
fn catalog_entry_value(field: SortField, e: &CatalogEntry) -> String {
    match field {
        SortField::AddedAt => e.added_at.to_string(),
        SortField::Title => e.name.clone(),
        SortField::PageCount => e.page_count.unwrap_or(-1).to_string(),
        SortField::Creator => e
            .sort_creator
            .clone()
            .unwrap_or_else(|| CREATOR_SENTINEL.to_string()),
        SortField::Rating => e.rating.unwrap_or(0).to_string(),
    }
}

/// Merge indexed item and series keysets, then hydrate the selected page.
pub async fn list_catalog(
    pool: &SqlitePool,
    user_id: i64,
    limit: i64,
    seek: CatalogSeek,
    sort: Sort,
    filters: &ListFilters,
) -> Result<ListResult> {
    let (descending, boundary, offset) = catalog_seek_plan(&seek, sort);
    let search = filters.search.as_deref();
    let filter = filters.tags.as_ref().filter(|f| !f.tag_ids.is_empty());
    let dir = if descending { "DESC" } else { "ASC" };
    let cmp = if descending { "<" } else { ">" };
    let inner_limit = offset + limit + 1;
    let item_sortval = match sort.field {
        SortField::AddedAt => "a.added_at",
        SortField::Title => "a.title",
        SortField::PageCount => "COALESCE(a.page_count, -1)",
        SortField::Creator => "COALESCE(a.sort_creator, char(1114111))",
        SortField::Rating => "COALESCE(rt.value, 0)",
    };
    // SQLite treats bare integer ORDER BY terms as column ordinals.
    let series_sortval = match sort.field {
        SortField::AddedAt => "s.added_at",
        SortField::Title => "s.title",
        SortField::PageCount => "-1 + 0",
        SortField::Creator => "COALESCE(s.sort_creator, char(1114111))",
        SortField::Rating => "COALESCE(sr.value, 0)",
    };
    let item_rating_join = if sort.field == SortField::Rating {
        " LEFT JOIN ratings rt ON rt.item_id = a.id AND rt.user_id = ?"
    } else {
        ""
    };
    let series_rating_join = if sort.field == SortField::Rating {
        " LEFT JOIN series_ratings sr ON sr.series_id = s.id AND sr.user_id = ?"
    } else {
        ""
    };

    let mut item = format!(
        "SELECT 'item' AS typ, a.id AS id, {item_sortval} AS sortval FROM items a \
         LEFT JOIN read_progress rp ON rp.item_id = a.id AND rp.user_id = ? AND rp.unit = 'page' \
         LEFT JOIN favorites f ON f.item_id = a.id AND f.user_id = ?{item_rating_join} \
         WHERE a.series_id IS NULL"
    );
    if let Some(f) = filter {
        push_tag_filter(&mut item, f);
    }
    if !filters.exclude_tags.is_empty() {
        push_tag_exclude(&mut item, filters.exclude_tags.len());
    }
    if search.is_some() {
        item.push_str(SEARCH_CLAUSE);
    }
    match filters.untagged {
        Some(true) => item.push_str(UNTAGGED_TRUE),
        Some(false) => item.push_str(UNTAGGED_FALSE),
        None => {}
    }
    match filters.favorited {
        Some(true) => item.push_str(" AND f.item_id IS NOT NULL"),
        Some(false) => item.push_str(" AND f.item_id IS NULL"),
        None => {}
    }
    match filters.completed {
        Some(true) => item.push_str(COMPLETED_TRUE),
        Some(false) => item.push_str(COMPLETED_FALSE),
        None => {}
    }
    if filters.kind.is_some() {
        item.push_str(" AND a.kind = ?");
    }
    if !filters.deny_kinds.is_empty() {
        push_kind_deny(&mut item, "a.kind", filters.deny_kinds.len());
    }
    if boundary.is_some() {
        item.push_str(&format!(
            " AND ({item_sortval}, 'item', a.id) {cmp} (?, ?, ?)"
        ));
    }
    item.push_str(&format!(
        " ORDER BY {item_sortval} {dir}, a.id {dir} LIMIT ?"
    ));

    let mut series = format!(
        "SELECT 'series' AS typ, s.id AS id, {series_sortval} AS sortval FROM series s \
         LEFT JOIN series_favorites sf ON sf.series_id = s.id AND sf.user_id = ?{series_rating_join} \
         WHERE 1 = 1"
    );
    if let Some(f) = filter {
        push_series_tag_filter(&mut series, f);
    }
    if !filters.exclude_tags.is_empty() {
        push_series_tag_exclude(&mut series, filters.exclude_tags.len());
    }
    if search.is_some() {
        series.push_str(SERIES_SEARCH_CLAUSE);
    }
    if let Some(u) = filters.untagged {
        series.push_str(&series_untagged_clause(u));
    }
    match filters.favorited {
        Some(true) => series.push_str(" AND sf.series_id IS NOT NULL"),
        Some(false) => series.push_str(" AND sf.series_id IS NULL"),
        None => {}
    }
    if let Some(c) = filters.completed {
        series.push_str(&series_completed_clause(c));
    }
    if filters.kind.is_some() {
        series.push_str(" AND s.kind = ?");
    }
    if !filters.deny_kinds.is_empty() {
        push_kind_deny(&mut series, "s.kind", filters.deny_kinds.len());
    }
    if boundary.is_some() {
        series.push_str(&format!(
            " AND ({series_sortval}, 'series', s.id) {cmp} (?, ?, ?)"
        ));
    }
    series.push_str(&format!(
        " ORDER BY {series_sortval} {dir}, s.id {dir} LIMIT ?"
    ));

    let sql = format!(
        "SELECT typ, id FROM ( SELECT * FROM ({item}) UNION ALL SELECT * FROM ({series}) ) \
         ORDER BY sortval {dir}, typ {dir}, id {dir} LIMIT ? OFFSET ?"
    );

    // Bind in the same order the two generated SQL branches appear.
    let mut q = sqlx::query_as::<_, (String, i64)>(AssertSqlSafe(sql))
        .bind(user_id)
        .bind(user_id);
    if sort.field == SortField::Rating {
        q = q.bind(user_id);
    }
    if let Some(f) = filter {
        for &id in &f.tag_ids {
            q = q.bind(id);
        }
        if f.match_all {
            q = q.bind(f.tag_ids.len() as i64);
        }
    }
    for &id in &filters.exclude_tags {
        q = q.bind(id);
    }
    if let Some(s) = search {
        q = q.bind(s.to_string());
    }
    if let Some(k) = &filters.kind {
        q = q.bind(k.clone());
    }
    for k in &filters.deny_kinds {
        q = q.bind(k.clone());
    }
    if let Some((value, typ, id)) = &boundary {
        if sort.field.is_text() {
            q = q.bind(value.clone());
        } else {
            q = q.bind(value.parse::<i64>().unwrap_or(0));
        }
        q = q.bind(typ.clone()).bind(*id);
    }
    q = q.bind(inner_limit);
    q = q.bind(user_id);
    if sort.field == SortField::Rating {
        q = q.bind(user_id);
    }
    if let Some(f) = filter {
        for &id in &f.tag_ids {
            q = q.bind(id);
        }
        if f.match_all {
            q = q.bind(f.tag_ids.len() as i64);
        }
    }
    for &id in &filters.exclude_tags {
        q = q.bind(id);
    }
    if let Some(s) = search {
        q = q.bind(s.to_string()).bind(s.to_string());
    }
    if filters.completed.is_some() {
        q = q.bind(user_id);
    }
    if let Some(k) = &filters.kind {
        q = q.bind(k.clone());
    }
    for k in &filters.deny_kinds {
        q = q.bind(k.clone());
    }
    if let Some((value, typ, id)) = &boundary {
        if sort.field.is_text() {
            q = q.bind(value.clone());
        } else {
            q = q.bind(value.parse::<i64>().unwrap_or(0));
        }
        q = q.bind(typ.clone()).bind(*id);
    }
    q = q.bind(inner_limit);
    q = q.bind(limit + 1).bind(offset);

    let mut rows: Vec<(String, i64)> = q.fetch_all(pool).await?;
    let has_more = rows.len() as i64 > limit;
    if has_more {
        rows.truncate(limit as usize);
    }
    if matches!(seek, CatalogSeek::Before { .. } | CatalogSeek::Last) {
        rows.reverse();
    }

    let item_ids: Vec<i64> = rows
        .iter()
        .filter(|(t, _)| t == "item")
        .map(|(_, id)| *id)
        .collect();
    let series_ids: Vec<i64> = rows
        .iter()
        .filter(|(t, _)| t == "series")
        .map(|(_, id)| *id)
        .collect();
    let mut item_cards = cards_for_ids(pool, user_id, &item_ids, true).await?;
    let mut series_cards = series_cards_for_ids(pool, user_id, &series_ids, true).await?;
    let items: Vec<CatalogEntry> = rows
        .iter()
        .filter_map(|(typ, id)| {
            if typ == "series" {
                series_cards.remove(id)
            } else {
                item_cards.remove(id).map(item_to_catalog)
            }
        })
        .collect();

    let enc = |e: &CatalogEntry| {
        encode_catalog_cursor(&sort, &catalog_entry_value(sort.field, e), &e.kind_of, e.id)
    };
    let first = items.first().map(enc);
    let last = items.last().map(enc);
    let (prev_cursor, next_cursor) = match seek {
        CatalogSeek::First => (None, has_more.then_some(last).flatten()),
        CatalogSeek::After { .. } => (first, has_more.then_some(last).flatten()),
        CatalogSeek::Offset(o) => (
            (o > 0).then_some(first).flatten(),
            has_more.then_some(last).flatten(),
        ),
        CatalogSeek::Before { .. } => (has_more.then_some(first).flatten(), last),
        CatalogSeek::Last => (has_more.then_some(first).flatten(), None),
    };

    Ok(ListResult {
        items,
        prev_cursor,
        next_cursor,
        total: None,
        page: None,
        page_count: None,
    })
}

/// Filter and hydrate a bounded BM25 result set without changing rank order.
pub async fn relevance_page(
    pool: &SqlitePool,
    user_id: i64,
    ranked_ids: &[i64],
    offset: i64,
    limit: i64,
    filters: &ListFilters,
) -> Result<(Vec<CatalogEntry>, i64)> {
    if ranked_ids.is_empty() {
        return Ok((Vec::new(), 0));
    }
    let placeholders = vec!["?"; ranked_ids.len()].join(", ");
    let mut sql = format!(
        "SELECT a.id FROM items a \
         LEFT JOIN read_progress rp ON rp.item_id = a.id AND rp.user_id = ? AND rp.unit = 'page' \
         LEFT JOIN favorites f ON f.item_id = a.id AND f.user_id = ? \
         WHERE a.id IN ({placeholders})"
    );
    let filter = filters.tags.as_ref().filter(|f| !f.tag_ids.is_empty());
    if let Some(f) = filter {
        push_tag_filter(&mut sql, f);
    }
    if !filters.exclude_tags.is_empty() {
        push_tag_exclude(&mut sql, filters.exclude_tags.len());
    }
    match filters.untagged {
        Some(true) => sql.push_str(UNTAGGED_TRUE),
        Some(false) => sql.push_str(UNTAGGED_FALSE),
        None => {}
    }
    match filters.favorited {
        Some(true) => sql.push_str(" AND f.item_id IS NOT NULL"),
        Some(false) => sql.push_str(" AND f.item_id IS NULL"),
        None => {}
    }
    match filters.completed {
        Some(true) => sql.push_str(COMPLETED_TRUE),
        Some(false) => sql.push_str(COMPLETED_FALSE),
        None => {}
    }
    if filters.kind.is_some() {
        sql.push_str(" AND a.kind = ?");
    }
    if !filters.deny_kinds.is_empty() {
        push_kind_deny(&mut sql, "a.kind", filters.deny_kinds.len());
    }

    let mut q = sqlx::query_scalar::<_, i64>(AssertSqlSafe(sql))
        .bind(user_id)
        .bind(user_id);
    for &id in ranked_ids {
        q = q.bind(id);
    }
    if let Some(f) = filter {
        for &id in &f.tag_ids {
            q = q.bind(id);
        }
        if f.match_all {
            q = q.bind(f.tag_ids.len() as i64);
        }
    }
    for &id in &filters.exclude_tags {
        q = q.bind(id);
    }
    if let Some(k) = &filters.kind {
        q = q.bind(k.clone());
    }
    for k in &filters.deny_kinds {
        q = q.bind(k.clone());
    }
    let passing: std::collections::HashSet<i64> = q.fetch_all(pool).await?.into_iter().collect();

    let filtered: Vec<i64> = ranked_ids
        .iter()
        .copied()
        .filter(|id| passing.contains(id))
        .collect();
    let total = filtered.len() as i64;
    let start = offset.max(0) as usize;
    let page_ids: Vec<i64> = filtered
        .into_iter()
        .skip(start)
        .take(limit.max(0) as usize)
        .collect();

    let mut cards = cards_for_ids(pool, user_id, &page_ids, true).await?;
    let entries: Vec<CatalogEntry> = page_ids
        .iter()
        .filter_map(|id| cards.remove(id).map(item_to_catalog))
        .collect();
    Ok((entries, total))
}

/// Count matching one-shots and collapsed series for page-jump pagination.
pub async fn count_catalog(pool: &SqlitePool, user_id: i64, filters: &ListFilters) -> Result<i64> {
    let search = filters.search.as_deref();
    let filter = filters.tags.as_ref().filter(|f| !f.tag_ids.is_empty());

    let mut item = String::from("SELECT COUNT(*) FROM items a WHERE a.series_id IS NULL");
    if let Some(f) = filter {
        push_tag_filter(&mut item, f);
    }
    if !filters.exclude_tags.is_empty() {
        push_tag_exclude(&mut item, filters.exclude_tags.len());
    }
    if search.is_some() {
        item.push_str(SEARCH_CLAUSE);
    }
    match filters.untagged {
        Some(true) => item.push_str(UNTAGGED_TRUE),
        Some(false) => item.push_str(UNTAGGED_FALSE),
        None => {}
    }
    if filters.kind.is_some() {
        item.push_str(" AND a.kind = ?");
    }
    if !filters.deny_kinds.is_empty() {
        push_kind_deny(&mut item, "a.kind", filters.deny_kinds.len());
    }
    match filters.favorited {
        Some(true) => {
            item.push_str(" AND a.id IN (SELECT item_id FROM favorites WHERE user_id = ?)")
        }
        Some(false) => {
            item.push_str(" AND a.id NOT IN (SELECT item_id FROM favorites WHERE user_id = ?)")
        }
        None => {}
    }
    match filters.completed {
        Some(true) => item.push_str(&format!(" AND a.id IN ({ITEM_COMPLETED_SET})")),
        Some(false) => item.push_str(&format!(" AND a.id NOT IN ({ITEM_COMPLETED_SET})")),
        None => {}
    }

    let mut series = String::from("SELECT COUNT(*) FROM series s WHERE 1 = 1");
    if let Some(f) = filter {
        push_series_tag_filter(&mut series, f);
    }
    if !filters.exclude_tags.is_empty() {
        push_series_tag_exclude(&mut series, filters.exclude_tags.len());
    }
    if search.is_some() {
        series.push_str(SERIES_SEARCH_CLAUSE);
    }
    if let Some(u) = filters.untagged {
        series.push_str(&series_untagged_clause(u));
    }
    if filters.kind.is_some() {
        series.push_str(" AND s.kind = ?");
    }
    if !filters.deny_kinds.is_empty() {
        push_kind_deny(&mut series, "s.kind", filters.deny_kinds.len());
    }
    match filters.favorited {
        Some(true) => series
            .push_str(" AND s.id IN (SELECT series_id FROM series_favorites WHERE user_id = ?)"),
        Some(false) => series.push_str(
            " AND s.id NOT IN (SELECT series_id FROM series_favorites WHERE user_id = ?)",
        ),
        None => {}
    }
    if let Some(c) = filters.completed {
        series.push_str(&series_completed_clause(c));
    }

    let sql = format!("SELECT ({item}) + ({series})");
    let mut q = sqlx::query_scalar::<_, i64>(AssertSqlSafe(sql));
    for branch in 0..2 {
        if let Some(f) = filter {
            for &id in &f.tag_ids {
                q = q.bind(id);
            }
            if f.match_all {
                q = q.bind(f.tag_ids.len() as i64);
            }
        }
        for &id in &filters.exclude_tags {
            q = q.bind(id);
        }
        if let Some(s) = search {
            q = q.bind(s.to_string());
            if branch == 1 {
                q = q.bind(s.to_string());
            }
        }
        if let Some(k) = &filters.kind {
            q = q.bind(k.clone());
        }
        for k in &filters.deny_kinds {
            q = q.bind(k.clone());
        }
        if filters.favorited.is_some() {
            q = q.bind(user_id);
        }
        if filters.completed.is_some() {
            q = q.bind(user_id);
        }
    }
    Ok(q.fetch_one(pool).await?)
}

/// A direct series-title suggestion independent of leaf titles.
pub struct SeriesSuggestion {
    pub id: i64,
    pub title: String,
    pub kind: String,
    /// The first leaf used as the series cover.
    pub cover_item_id: Option<i64>,
    /// Cover thumbnail cache version.
    pub cover_version: Option<String>,
}

/// Search series titles directly and return best BM25 matches first.
pub async fn suggest_series(
    pool: &SqlitePool,
    raw_query: &str,
    kind: Option<&str>,
    deny_kinds: &[String],
    limit: i64,
) -> Result<Vec<SeriesSuggestion>> {
    let Some(m) = fts_query(raw_query) else {
        return Ok(Vec::new());
    };
    let mut sql = format!(
        "SELECT s.id, s.title, s.kind, \
           {SERIES_COVER_ITEM_SUBQ} AS cover_item_id, \
           {SERIES_COVER_VERSION_SUBQ} AS cover_version \
         FROM series_fts JOIN series s ON s.id = series_fts.rowid \
         WHERE series_fts MATCH ?",
    );
    if kind.is_some() {
        sql.push_str(" AND s.kind = ?");
    }
    if !deny_kinds.is_empty() {
        push_kind_deny(&mut sql, "s.kind", deny_kinds.len());
    }
    sql.push_str(" ORDER BY bm25(series_fts) LIMIT ?");
    let mut q =
        sqlx::query_as::<_, (i64, String, String, Option<i64>, Option<String>)>(AssertSqlSafe(sql))
            .bind(m);
    if let Some(k) = kind {
        q = q.bind(k.to_string());
    }
    for k in deny_kinds {
        q = q.bind(k.clone());
    }
    let rows = q.bind(limit).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, title, kind, cover_item_id, cover_version)| SeriesSuggestion {
                id,
                title,
                kind,
                cover_item_id,
                cover_version,
            },
        )
        .collect())
}

/// Map series-leaf item ids to their series ids.
pub async fn series_id_of_items(pool: &SqlitePool, ids: &[i64]) -> Result<HashMap<i64, i64>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = vec!["?"; ids.len()].join(", ");
    let sql = format!(
        "SELECT id, series_id FROM items WHERE id IN ({placeholders}) AND series_id IS NOT NULL"
    );
    let mut q = sqlx::query_as::<_, (i64, i64)>(AssertSqlSafe(sql));
    for &id in ids {
        q = q.bind(id);
    }
    Ok(q.fetch_all(pool).await?.into_iter().collect())
}

pub async fn series_cards_for_ids(
    pool: &SqlitePool,
    user_id: i64,
    series_ids: &[i64],
    with_tags: bool,
) -> Result<HashMap<i64, CatalogEntry>> {
    if series_ids.is_empty() {
        return Ok(HashMap::new());
    }
    #[derive(sqlx::FromRow)]
    struct SeriesCardRow {
        id: i64,
        kind: String,
        title: String,
        added_at: i64,
        sort_creator: Option<String>,
        favorited: bool,
        rating: Option<i64>,
        leaf_count: Option<i64>,
        read_count: Option<i64>,
        cover_item_id: Option<i64>,
        cover_version: Option<String>,
    }
    let placeholders = vec!["?"; series_ids.len()].join(", ");
    let sql = format!(
        "SELECT s.id, s.kind, s.title, s.added_at, s.sort_creator, \
                (sf.series_id IS NOT NULL) AS favorited, \
                sr.value AS rating, \
                (SELECT COUNT(*) FROM item_series_leaf WHERE series_id = s.id) AS leaf_count, \
                {SERIES_READ_COUNT_SUBQ} AS read_count, \
                {SERIES_COVER_ITEM_SUBQ} AS cover_item_id, \
                {SERIES_COVER_VERSION_SUBQ} AS cover_version \
         FROM series s \
         LEFT JOIN series_favorites sf ON sf.series_id = s.id AND sf.user_id = ? \
         LEFT JOIN series_ratings sr ON sr.series_id = s.id AND sr.user_id = ? \
         WHERE s.id IN ({placeholders})"
    );
    let mut q = sqlx::query_as::<_, SeriesCardRow>(AssertSqlSafe(sql))
        .bind(user_id)
        .bind(user_id)
        .bind(user_id);
    for &id in series_ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(pool).await?;
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let mut tags = if with_tags {
        series_tags_for_ids(pool, &ids).await?
    } else {
        HashMap::new()
    };
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                r.id,
                CatalogEntry {
                    kind_of: "series".to_string(),
                    id: r.id,
                    kind: r.kind,
                    modality: "paginated".to_string(),
                    name: r.title,
                    page_count: None,
                    added_at: r.added_at,
                    progress: None,
                    favorited: r.favorited,
                    rating: r.rating,
                    tags: tags.remove(&r.id).unwrap_or_default(),
                    leaf_count: r.leaf_count,
                    read_count: r.read_count,
                    cover_item_id: r.cover_item_id,
                    cover_version: r.cover_version,
                    sort_creator: r.sort_creator,
                },
            )
        })
        .collect())
}

/// Convert an item card to the unified [`CatalogEntry`] (`type="item"`).
fn item_to_catalog(e: ItemEntry) -> CatalogEntry {
    CatalogEntry {
        kind_of: "item".to_string(),
        id: e.id,
        kind: e.kind,
        modality: e.modality,
        name: e.name,
        page_count: e.page_count,
        added_at: e.added_at,
        progress: e.progress,
        favorited: e.favorited,
        rating: e.rating,
        tags: e.tags,
        leaf_count: None,
        read_count: None,
        cover_item_id: None,
        cover_version: Some(e.cover_version),
        sort_creator: e.sort_creator,
    }
}

/// Collapse ranked leaves into series cards while preserving first-match order.
pub async fn collapse_ranked(
    pool: &SqlitePool,
    user_id: i64,
    ranked: &[(i64, f32)],
    kind: Option<&str>,
    limit: i64,
    with_tags: bool,
) -> Result<Vec<(CatalogEntry, f32)>> {
    let ids: Vec<i64> = ranked.iter().map(|(id, _)| *id).collect();
    let mut membership: HashMap<i64, i64> = HashMap::new();
    if !ids.is_empty() {
        let placeholders = vec!["?"; ids.len()].join(", ");
        let sql = format!(
            "SELECT item_id, series_id FROM item_series_leaf WHERE item_id IN ({placeholders})"
        );
        let mut q = sqlx::query_as::<_, (i64, i64)>(AssertSqlSafe(sql));
        for &id in &ids {
            q = q.bind(id);
        }
        for (item_id, series_id) in q.fetch_all(pool).await? {
            membership.insert(item_id, series_id);
        }
    }

    enum Target {
        Item(i64),
        Series(i64),
    }
    let mut seen_series: HashSet<i64> = HashSet::new();
    let mut targets: Vec<(Target, f32)> = Vec::new();
    for (id, score) in ranked {
        match membership.get(id) {
            Some(&sid) => {
                if seen_series.insert(sid) {
                    targets.push((Target::Series(sid), *score));
                }
            }
            None => targets.push((Target::Item(*id), *score)),
        }
    }

    let item_ids: Vec<i64> = targets
        .iter()
        .filter_map(|(t, _)| match t {
            Target::Item(i) => Some(*i),
            _ => None,
        })
        .collect();
    let series_ids: Vec<i64> = targets
        .iter()
        .filter_map(|(t, _)| match t {
            Target::Series(s) => Some(*s),
            _ => None,
        })
        .collect();
    let mut item_cards = cards_for_ids(pool, user_id, &item_ids, with_tags).await?;
    let mut series_cards = series_cards_for_ids(pool, user_id, &series_ids, with_tags).await?;

    let mut out = Vec::new();
    for (t, score) in targets {
        if out.len() as i64 >= limit {
            break;
        }
        let card = match t {
            Target::Item(i) => item_cards.remove(&i).map(item_to_catalog),
            Target::Series(s) => series_cards.remove(&s),
        };
        if let Some(c) = card {
            if kind.is_none_or(|k| c.kind == k) {
                out.push((c, score));
            }
        }
    }
    Ok(out)
}

/// Hydrate ranked entry keys, where positive ids are items and negative ids are series.
pub async fn collapse_entry_ranked(
    pool: &SqlitePool,
    user_id: i64,
    ranked: &[(i64, f32)],
    kind: Option<&str>,
    limit: i64,
    with_tags: bool,
) -> Result<Vec<(CatalogEntry, f32)>> {
    let item_ids: Vec<i64> = ranked.iter().map(|(k, _)| *k).filter(|k| *k > 0).collect();
    let series_ids: Vec<i64> = ranked
        .iter()
        .map(|(k, _)| *k)
        .filter(|k| *k < 0)
        .map(|k| -k)
        .collect();
    let mut item_cards = cards_for_ids(pool, user_id, &item_ids, with_tags).await?;
    let mut series_cards = series_cards_for_ids(pool, user_id, &series_ids, with_tags).await?;
    let mut out = Vec::new();
    for (key, score) in ranked {
        if out.len() as i64 >= limit {
            break;
        }
        let card = if *key < 0 {
            series_cards.remove(&(-*key))
        } else {
            item_cards.remove(key).map(item_to_catalog)
        };
        if let Some(c) = card {
            if kind.is_none_or(|k| c.kind == k) {
                out.push((c, *score));
            }
        }
    }
    Ok(out)
}

/// A library kind and its collapsed catalog-card count.
#[derive(Serialize, ToSchema)]
pub struct KindCount {
    pub kind: String,
    pub count: i64,
}

/// List kinds with one count per one-shot or distinct series.
pub async fn list_kinds(pool: &SqlitePool) -> Result<Vec<KindCount>> {
    Ok(sqlx::query_as::<_, (String, i64)>(
        "SELECT kind, \
                SUM(CASE WHEN series_id IS NULL THEN 1 ELSE 0 END) + COUNT(DISTINCT series_id) AS n \
         FROM items GROUP BY kind ORDER BY kind",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(kind, count)| KindCount { kind, count })
    .collect())
}

/// Hydrate item cards keyed by id. The caller restores its desired order.
pub async fn cards_for_ids(
    pool: &SqlitePool,
    user_id: i64,
    item_ids: &[i64],
    with_tags: bool,
) -> Result<HashMap<i64, ItemEntry>> {
    if item_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = vec!["?"; item_ids.len()].join(", ");
    let sql = format!(
        "SELECT a.id, a.kind, COALESCE(a.modality_override, a.modality) AS modality, a.title, a.page_count, a.added_at, \
                a.sort_creator, a.structural_hash, \
                CAST(rp.value AS INTEGER) AS progress, \
                (f.item_id IS NOT NULL) AS favorited, \
                rt.value AS rating \
         FROM items a \
         LEFT JOIN read_progress rp ON rp.item_id = a.id AND rp.user_id = ? AND rp.unit = 'page' \
         LEFT JOIN favorites f ON f.item_id = a.id AND f.user_id = ? \
         LEFT JOIN ratings rt ON rt.item_id = a.id AND rt.user_id = ? \
         WHERE a.id IN ({placeholders})"
    );
    let mut q = sqlx::query_as::<_, ListRow>(AssertSqlSafe(sql))
        .bind(user_id)
        .bind(user_id)
        .bind(user_id);
    for &id in item_ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(pool).await?;
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let mut tags = if with_tags {
        tags_for_item_ids(pool, &ids).await?
    } else {
        HashMap::new()
    };
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                r.id,
                ItemEntry {
                    tags: tags.remove(&r.id).unwrap_or_default(),
                    id: r.id,
                    kind: r.kind,
                    modality: r.modality,
                    name: r.title,
                    progress: clamp_progress(r.progress, r.page_count),
                    page_count: r.page_count,
                    added_at: r.added_at,
                    favorited: r.favorited,
                    rating: r.rating,
                    cover_version: r.structural_hash,
                    sort_creator: r.sort_creator,
                },
            )
        })
        .collect())
}

/// Clamp stale progress to the item's current page range.
pub fn clamp_progress(progress: Option<i64>, page_count: Option<i64>) -> Option<i64> {
    match (progress, page_count) {
        (Some(p), Some(pc)) => Some(p.clamp(0, (pc - 1).max(0))),
        (Some(p), None) => Some(p.max(0)),
        (None, _) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::test_util::*;

    /// Both catalog branches must use their ordering indexes.
    #[sqlx::test]
    async fn catalog_browse_branches_are_index_served(pool: SqlitePool) {
        async fn plan(pool: &SqlitePool, sql: &str) -> String {
            let rows: Vec<(i64, i64, i64, String)> =
                sqlx::query_as(AssertSqlSafe(format!("EXPLAIN QUERY PLAN {sql}")))
                    .fetch_all(pool)
                    .await
                    .unwrap();
            rows.into_iter()
                .map(|(_, _, _, d)| d)
                .collect::<Vec<_>>()
                .join(" | ")
        }

        for i in 0..200 {
            insert_item(&pool, &format!("h{i}"), i).await;
        }
        for i in 0..20 {
            sqlx::query(
                "INSERT INTO series (kind, title, folder_path, added_at) VALUES ('manga', ?, ?, ?)",
            )
            .bind(format!("s{i}"))
            .bind(format!("manga/s{i}"))
            .bind(i)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query("ANALYZE").execute(&pool).await.unwrap();

        let item = plan(
            &pool,
            "SELECT a.id FROM items a WHERE a.series_id IS NULL \
             ORDER BY a.added_at DESC, a.id DESC LIMIT 26",
        )
        .await;
        assert!(
            item.contains("idx_items_oneshot_added"),
            "item branch must use the partial added index; got: {item}"
        );
        assert!(
            !item.contains("USE TEMP B-TREE"),
            "item branch must not temp-sort; got: {item}"
        );

        let series = plan(
            &pool,
            "SELECT s.id FROM series s ORDER BY s.added_at DESC, s.id DESC LIMIT 26",
        )
        .await;
        assert!(
            series.contains("idx_series_added"),
            "series branch must use its added index; got: {series}"
        );
        assert!(
            !series.contains("USE TEMP B-TREE"),
            "series branch must not temp-sort; got: {series}"
        );
    }

    /// Completion filtering and counting must agree for series and one-shots.
    #[sqlx::test]
    async fn catalog_completed_filter_over_series_and_oneshots(pool: SqlitePool) {
        let uid = a_user(&pool).await;
        let sid: i64 = sqlx::query_scalar(
            "INSERT INTO series (kind, title, folder_path, added_at) VALUES ('manga','S','manga/S',10) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let leaf = |tag: &'static str, sort: f64| {
            let pool = pool.clone();
            async move {
                let id: i64 = sqlx::query_scalar(
                    "INSERT INTO items (scheme_tag, structural_hash, path, size_bytes, mtime, format, title, page_count, added_at, last_modified_at, series_id) \
                     VALUES ('zip-structural-v1', ?, ?, 1, 1, 'cbz', ?, 5, 1, 0, ?) RETURNING id",
                )
                .bind(tag)
                .bind(format!("/p/{tag}"))
                .bind(tag)
                .bind(sid)
                .fetch_one(&pool)
                .await
                .unwrap();
                sqlx::query("INSERT INTO item_series_leaf (item_id, series_id, number_sort) VALUES (?, ?, ?)")
                    .bind(id)
                    .bind(sid)
                    .bind(sort)
                    .execute(&pool)
                    .await
                    .unwrap();
                id
            }
        };
        let l1 = leaf("h1", 1.0).await;
        let l2 = leaf("h2", 2.0).await;
        let one = insert_item(&pool, "one", 20).await;

        let done = |c: bool| ListFilters {
            completed: Some(c),
            ..Default::default()
        };
        let list = |f: ListFilters| {
            let pool = pool.clone();
            async move {
                list_catalog(&pool, uid, 50, CatalogSeek::First, Sort::default(), &f)
                    .await
                    .unwrap()
                    .items
            }
        };

        assert!(list(done(true)).await.is_empty());
        assert_eq!(list(done(false)).await.len(), 2, "series card + one-shot");

        for l in [l1, l2] {
            sqlx::query("INSERT INTO read_progress (user_id, item_id, unit, value, updated_at) VALUES (?, ?, 'page', 4, 0)")
                .bind(uid)
                .bind(l)
                .execute(&pool)
                .await
                .unwrap();
        }
        let c = list(done(true)).await;
        assert_eq!(c.len(), 1);
        assert_eq!((c[0].kind_of.as_str(), c[0].id), ("series", sid));
        let nc = list(done(false)).await;
        assert_eq!(nc.len(), 1);
        assert_eq!((nc[0].kind_of.as_str(), nc[0].id), ("item", one));
        assert_eq!(count_catalog(&pool, uid, &done(true)).await.unwrap(), 1);
        assert_eq!(count_catalog(&pool, uid, &done(false)).await.unwrap(), 1);

        sqlx::query("DELETE FROM read_progress WHERE item_id = ?")
            .bind(l2)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            list(done(true)).await.is_empty(),
            "a partly-read series is not complete"
        );
        assert_eq!(count_catalog(&pool, uid, &done(true)).await.unwrap(), 0);
    }

    /// Every sort must execute across both catalog branches.
    #[sqlx::test]
    async fn catalog_runs_for_every_sort_field(pool: SqlitePool) {
        let uid = a_user(&pool).await;
        insert_item(&pool, "solo", 12).await;
        let sid: i64 = sqlx::query_scalar(
            "INSERT INTO series (kind, title, folder_path, added_at) VALUES ('manga','S','manga/S',10) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let leaf: i64 = sqlx::query_scalar(
            "INSERT INTO items (scheme_tag, structural_hash, path, size_bytes, mtime, format, title, page_count, added_at, last_modified_at, series_id) \
             VALUES ('zip-structural-v1','hh','/p/hh',1,1,'cbz','v1',7,1,0,?) RETURNING id",
        )
        .bind(sid)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO item_series_leaf (item_id, series_id, number_sort) VALUES (?, ?, 1.0)",
        )
        .bind(leaf)
        .bind(sid)
        .execute(&pool)
        .await
        .unwrap();

        for field in [
            SortField::AddedAt,
            SortField::Title,
            SortField::PageCount,
            SortField::Creator,
            SortField::Rating,
        ] {
            for descending in [false, true] {
                let sort = Sort { field, descending };
                let page = list_catalog(
                    &pool,
                    uid,
                    50,
                    CatalogSeek::First,
                    sort,
                    &ListFilters::default(),
                )
                .await
                .unwrap_or_else(|e| {
                    panic!("list_catalog failed for {field:?} descending={descending}: {e}")
                });
                assert_eq!(
                    page.items.len(),
                    2,
                    "one-shot + collapsed series card for sort={field:?}"
                );
            }
        }
    }

    /// Rating keyset pagination keeps unrated entries last without gaps.
    #[sqlx::test]
    async fn rating_sort_orders_by_user_rating_highest_first(pool: SqlitePool) {
        let uid = a_user(&pool).await;
        let a = insert_item(&pool, "a", 10).await;
        let b = insert_item(&pool, "b", 20).await;
        let c = insert_item(&pool, "c", 30).await;
        set_rating(&pool, uid, a, 5).await.unwrap();
        set_rating(&pool, uid, b, 3).await.unwrap();

        let sort = Sort {
            field: SortField::Rating,
            descending: true,
        };
        let page = list_catalog(
            &pool,
            uid,
            50,
            CatalogSeek::First,
            sort,
            &ListFilters::default(),
        )
        .await
        .unwrap();
        let order: Vec<i64> = page.items.iter().map(|e| e.id).collect();
        assert_eq!(order, vec![a, b, c], "5★, 3★, then unrated(0)");
        assert_eq!(page.items[0].rating, Some(5));
        assert_eq!(page.items[1].rating, Some(3));
        assert_eq!(page.items[2].rating, None, "unrated → absent, not 0");

        let p1 = list_catalog(
            &pool,
            uid,
            1,
            CatalogSeek::First,
            sort,
            &ListFilters::default(),
        )
        .await
        .unwrap();
        assert_eq!(p1.items[0].id, a);
        let after = |e: &CatalogEntry| CatalogSeek::After {
            value: catalog_entry_value(SortField::Rating, e),
            typ: e.kind_of.clone(),
            id: e.id,
        };
        let seek2 = after(&p1.items[0]);
        let p2 = list_catalog(&pool, uid, 1, seek2, sort, &ListFilters::default())
            .await
            .unwrap();
        assert_eq!(p2.items[0].id, b, "keyset continues to the 3★");
        let seek3 = after(&p2.items[0]);
        let p3 = list_catalog(&pool, uid, 1, seek3, sort, &ListFilters::default())
            .await
            .unwrap();
        assert_eq!(p3.items[0].id, c, "then the unrated one, no skip/dup");

        clear_rating(&pool, uid, a).await.unwrap();
        set_rating(&pool, uid, c, 4).await.unwrap();
        let page = list_catalog(
            &pool,
            uid,
            50,
            CatalogSeek::First,
            sort,
            &ListFilters::default(),
        )
        .await
        .unwrap();
        let order: Vec<i64> = page.items.iter().map(|e| e.id).collect();
        assert_eq!(order, vec![c, b, a], "now 4★(c), 3★(b), unrated(a)");
        assert_eq!(get_rating(&pool, uid, a).await.unwrap(), None);
    }

    #[sqlx::test]
    async fn tag_filter_and_or_compose_with_keyset(pool: SqlitePool) {
        let uid = a_user(&pool).await;
        let a1 = insert_item(&pool, "a1", 1).await;
        let a2 = insert_item(&pool, "a2", 2).await;
        let a3 = insert_item(&pool, "a3", 3).await;

        let t1 = get_or_create_tag(&pool, "tag", "alpha").await.unwrap();
        let t2 = get_or_create_tag(&pool, "tag", "beta").await.unwrap();
        add_item_tag(&pool, a1, t1, "none", "manual").await.unwrap();
        add_item_tag(&pool, a2, t1, "none", "manual").await.unwrap();
        add_item_tag(&pool, a2, t2, "none", "manual").await.unwrap();
        add_item_tag(&pool, a3, t2, "none", "manual").await.unwrap();

        let f = TagFilter {
            tag_ids: vec![t1, t2],
            match_all: true,
        };
        assert_eq!(
            ids(&list_catalog(
                &pool,
                uid,
                50,
                CatalogSeek::First,
                Sort::default(),
                &ListFilters {
                    tags: Some(f),
                    ..Default::default()
                }
            )
            .await
            .unwrap()),
            vec![a2]
        );

        let f = TagFilter {
            tag_ids: vec![t1, t2],
            match_all: false,
        };
        assert_eq!(
            ids(&list_catalog(
                &pool,
                uid,
                50,
                CatalogSeek::First,
                Sort::default(),
                &ListFilters {
                    tags: Some(f),
                    ..Default::default()
                }
            )
            .await
            .unwrap()),
            vec![a3, a2, a1]
        );

        let f = TagFilter {
            tag_ids: vec![t1, t2],
            match_all: false,
        };
        let lf = ListFilters {
            tags: Some(f),
            ..Default::default()
        };
        let p1 = list_catalog(&pool, uid, 2, CatalogSeek::First, Sort::default(), &lf)
            .await
            .unwrap();
        assert_eq!(ids(&p1), vec![a3, a2]);
        let cursor = decode_cursor(p1.next_cursor.as_ref().unwrap()).unwrap();
        let p2 = list_catalog(
            &pool,
            uid,
            2,
            CatalogSeek::After {
                value: cursor.value,
                typ: cursor.typ.unwrap_or_else(|| "item".to_string()),
                id: cursor.id,
            },
            Sort::default(),
            &lf,
        )
        .await
        .unwrap();
        assert_eq!(ids(&p2), vec![a1]);
        assert!(p2.next_cursor.is_none());
        assert!(p2.prev_cursor.is_some());
    }

    #[sqlx::test]
    async fn untagged_and_completed_filters_compose(pool: SqlitePool) {
        let user = a_user(&pool).await;
        let mut id = std::collections::HashMap::new();
        for (h, a) in [
            ("u_untag", 1),
            ("t_done", 2),
            ("t_reading", 3),
            ("t_unread", 4),
        ] {
            id.insert(h, insert_item(&pool, h, a).await);
        }
        let tag = get_or_create_tag(&pool, "tag", "x").await.unwrap();
        for h in ["t_done", "t_reading", "t_unread"] {
            add_item_tag(&pool, id[h], tag, "none", "manual")
                .await
                .unwrap();
        }
        set_progress(&pool, user, id["t_done"], 4).await.unwrap();
        set_progress(&pool, user, id["t_reading"], 2).await.unwrap();

        let s = Sort::default();

        let f = ListFilters {
            untagged: Some(true),
            ..Default::default()
        };
        let r = list_catalog(&pool, user, 50, CatalogSeek::First, s, &f)
            .await
            .unwrap();
        assert_eq!(ids(&r), vec![id["u_untag"]]);

        let f = ListFilters {
            completed: Some(true),
            ..Default::default()
        };
        let r = list_catalog(&pool, user, 50, CatalogSeek::First, s, &f)
            .await
            .unwrap();
        assert_eq!(ids(&r), vec![id["t_done"]]);

        let f = ListFilters {
            completed: Some(false),
            ..Default::default()
        };
        let r = list_catalog(&pool, user, 50, CatalogSeek::First, s, &f)
            .await
            .unwrap();
        assert_eq!(
            ids(&r),
            vec![id["t_unread"], id["t_reading"], id["u_untag"]]
        );

        let f = ListFilters {
            untagged: Some(true),
            completed: Some(false),
            ..Default::default()
        };
        let r = list_catalog(&pool, user, 50, CatalogSeek::First, s, &f)
            .await
            .unwrap();
        assert_eq!(ids(&r), vec![id["u_untag"]]);

        assert_eq!(
            count_catalog(
                &pool,
                user,
                &ListFilters {
                    completed: Some(false),
                    ..Default::default()
                }
            )
            .await
            .unwrap(),
            3
        );
        assert_eq!(
            count_catalog(
                &pool,
                user,
                &ListFilters {
                    untagged: Some(true),
                    ..Default::default()
                }
            )
            .await
            .unwrap(),
            1
        );
    }

    #[sqlx::test]
    async fn exclude_tags_hide_items_and_series(pool: SqlitePool) {
        let uid = a_user(&pool).await;
        let a = insert_item(&pool, "ex_a", 1).await;
        let b = insert_item(&pool, "ex_b", 2).await;
        let c = insert_item(&pool, "ex_c", 3).await;
        let ntr = get_or_create_tag(&pool, "tag", "mystery").await.unwrap();
        let van = get_or_create_tag(&pool, "tag", "vanilla").await.unwrap();
        add_item_tag(&pool, a, ntr, "none", "manual").await.unwrap();
        add_item_tag(&pool, b, van, "none", "manual").await.unwrap();
        let sid: i64 = sqlx::query_scalar(
            "INSERT INTO series (kind, title, folder_path, added_at) VALUES ('manga','S','manga/S',10) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let mut leaves = Vec::new();
        for (h, sort) in [("ex_l1", 1.0), ("ex_l2", 2.0)] {
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO items (scheme_tag, structural_hash, path, size_bytes, mtime, format, title, page_count, added_at, last_modified_at, series_id) \
                 VALUES ('zip-structural-v1', ?, ?, 1, 1, 'cbz', ?, 5, 1, 0, ?) RETURNING id",
            )
            .bind(h)
            .bind(format!("/p/{h}"))
            .bind(h)
            .bind(sid)
            .fetch_one(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO item_series_leaf (item_id, series_id, number_sort) VALUES (?, ?, ?)",
            )
            .bind(id)
            .bind(sid)
            .bind(sort)
            .execute(&pool)
            .await
            .unwrap();
            leaves.push(id);
        }
        add_item_tag(&pool, leaves[1], ntr, "none", "manual")
            .await
            .unwrap();

        let cards = |f: ListFilters| {
            let pool = pool.clone();
            async move {
                let r = list_catalog(&pool, uid, 50, CatalogSeek::First, Sort::default(), &f)
                    .await
                    .unwrap();
                r.items
                    .iter()
                    .map(|e| (e.kind_of.clone(), e.id))
                    .collect::<Vec<_>>()
            }
        };

        assert_eq!(cards(ListFilters::default()).await.len(), 4);

        let f = ListFilters {
            exclude_tags: vec![ntr],
            ..Default::default()
        };
        let got = cards(f).await;
        assert!(!got.contains(&("item".into(), a)));
        assert!(!got.iter().any(|(t, _)| t == "series"));
        assert!(got.contains(&("item".into(), b)));
        assert!(got.contains(&("item".into(), c)));

        let f = ListFilters {
            tags: Some(TagFilter {
                tag_ids: vec![van],
                match_all: true,
            }),
            exclude_tags: vec![ntr],
            ..Default::default()
        };
        assert_eq!(cards(f).await, vec![("item".to_string(), b)]);

        let f = ListFilters {
            exclude_tags: vec![ntr],
            ..Default::default()
        };
        assert_eq!(count_catalog(&pool, uid, &f).await.unwrap(), 2);

        set_blocklist(&pool, uid, &[ntr, van]).await.unwrap();
        let mut got = blocklist_tag_ids(&pool, uid).await.unwrap();
        got.sort();
        assert_eq!(got, {
            let mut v = vec![ntr, van];
            v.sort();
            v
        });
        set_blocklist(&pool, uid, &[van]).await.unwrap();
        assert_eq!(blocklist_tag_ids(&pool, uid).await.unwrap(), vec![van]);
        let named = blocklist_tags(&pool, uid).await.unwrap();
        assert_eq!(named.len(), 1);
        assert_eq!(named[0].value, "vanilla");
        set_blocklist(&pool, uid, &[]).await.unwrap();
        assert!(blocklist_tag_ids(&pool, uid).await.unwrap().is_empty());
    }

    #[test]
    fn keyset_modes_use_a_cursor_and_never_offset_only_jump_offsets() {
        let s = Sort::default();
        let after = || CatalogSeek::After {
            value: "100".to_string(),
            typ: "item".to_string(),
            id: 5,
        };
        let before = || CatalogSeek::Before {
            value: "100".to_string(),
            typ: "item".to_string(),
            id: 5,
        };
        let boundary = Some(("100".to_string(), "item".to_string(), 5));
        for (seek, label) in [
            (CatalogSeek::First, "First"),
            (after(), "Next/After"),
            (before(), "Prev/Before"),
            (CatalogSeek::Last, "Last"),
        ] {
            assert_eq!(catalog_seek_plan(&seek, s).2, 0, "{label} must not OFFSET");
        }
        assert_eq!(
            catalog_seek_plan(&after(), s).1,
            boundary,
            "Next uses the cursor"
        );
        assert_eq!(
            catalog_seek_plan(&before(), s).1,
            boundary,
            "Prev uses the cursor"
        );
        assert_eq!(catalog_seek_plan(&CatalogSeek::First, s).1, None);
        assert!(
            catalog_seek_plan(&CatalogSeek::First, s).0,
            "First scans in sort order (DESC)"
        );
        assert_eq!(catalog_seek_plan(&CatalogSeek::Last, s).1, None);
        assert!(
            !catalog_seek_plan(&CatalogSeek::Last, s).0,
            "Last scans reversed (ASC, flipped back to newest-first)"
        );
        let (desc, boundary, offset) = catalog_seek_plan(&CatalogSeek::Offset(180), s);
        assert_eq!(offset, 180, "page jump uses OFFSET = (N-1)*limit");
        assert_eq!(boundary, None, "page jump is not cursor-driven");
        assert!(desc, "page jump scans in sort order like the first page");
        let asc = Sort {
            field: SortField::Title,
            descending: false,
        };
        assert!(
            !catalog_seek_plan(&CatalogSeek::First, asc).0,
            "ASC sort: First scans ASC"
        );
        assert!(
            catalog_seek_plan(&CatalogSeek::Last, asc).0,
            "ASC sort: Last scans DESC"
        );
    }

    #[sqlx::test]
    async fn kind_filter_and_facet_ride_idx_items_kind(pool: SqlitePool) {
        insert_item(&pool, "k1", 1).await;
        let plan = |sql: &'static str| {
            let pool = &pool;
            async move {
                sqlx::query_as::<_, (i64, i64, i64, String)>(AssertSqlSafe(format!(
                    "EXPLAIN QUERY PLAN {sql}"
                )))
                .fetch_all(pool)
                .await
                .unwrap()
                .into_iter()
                .map(|r| r.3)
                .collect::<Vec<_>>()
                .join(" | ")
            }
        };

        let listing = plan(
            "SELECT a.id FROM items a WHERE a.kind = 'manga' \
             ORDER BY a.added_at DESC, a.id DESC LIMIT 10",
        )
        .await;
        assert!(
            listing.contains("idx_items_kind") && !listing.contains("TEMP B-TREE"),
            "kind listing must use idx_items_kind, no sort: {listing}"
        );

        let facet = plan("SELECT kind, COUNT(*) FROM items GROUP BY kind ORDER BY kind").await;
        assert!(
            !facet.contains("TEMP B-TREE"),
            "kind facet must group via the index, no sort: {facet}"
        );
    }

    #[sqlx::test]
    async fn sort_keysets_ride_their_indexes(pool: SqlitePool) {
        insert_item(&pool, "s1", 1).await;

        async fn plan(pool: &SqlitePool, sql: &str, a: &str, b: i64) -> String {
            let rows: Vec<(i64, i64, i64, String)> =
                sqlx::query_as(AssertSqlSafe(format!("EXPLAIN QUERY PLAN {sql}")))
                    .bind(a)
                    .bind(b)
                    .fetch_all(pool)
                    .await
                    .unwrap();
            rows.into_iter()
                .map(|r| r.3)
                .collect::<Vec<_>>()
                .join(" | ")
        }

        let title = plan(
            &pool,
            "SELECT a.id FROM items a WHERE (a.title, a.id) > (?, ?) \
             ORDER BY a.title ASC, a.id ASC LIMIT 10",
            "x",
            0,
        )
        .await;
        assert!(
            title.contains("idx_items_title") && !title.contains("TEMP B-TREE"),
            "title keyset must use idx_items_title, no sort: {title}"
        );

        let pages = plan(
            &pool,
            "SELECT a.id FROM items a WHERE (COALESCE(a.page_count,-1), a.id) < (?, ?) \
             ORDER BY COALESCE(a.page_count,-1) DESC, a.id DESC LIMIT 10",
            "0",
            0,
        )
        .await;
        assert!(
            pages.contains("idx_items_pagecount") && !pages.contains("TEMP B-TREE"),
            "page_count keyset must use the COALESCE expression index, no sort: {pages}"
        );

        let creator = plan(
            &pool,
            "SELECT a.id FROM items a WHERE (COALESCE(a.sort_creator, char(1114111)), a.id) > (?, ?) \
             ORDER BY COALESCE(a.sort_creator, char(1114111)) ASC, a.id ASC LIMIT 10",
            "x",
            0,
        )
        .await;
        assert!(
            creator.contains("idx_items_creator") && !creator.contains("TEMP B-TREE"),
            "creator keyset must use the COALESCE expression index, no sort: {creator}"
        );
    }

    #[test]
    fn clamp_progress_bounds() {
        assert_eq!(clamp_progress(Some(99999), Some(20)), Some(19));
        assert_eq!(clamp_progress(Some(5), Some(20)), Some(5));
        assert_eq!(clamp_progress(Some(-3), Some(20)), Some(0));
        assert_eq!(clamp_progress(Some(7), None), Some(7));
        assert_eq!(clamp_progress(None, Some(20)), None);
        assert_eq!(clamp_progress(Some(3), Some(0)), Some(0));
    }
}
