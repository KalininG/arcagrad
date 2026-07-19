//! Item persistence, metadata, and per-user reading state.

use anyhow::{anyhow, Result};
use serde::Serialize;
use sqlx::{AssertSqlSafe, SqlitePool};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use utoipa::ToSchema;

use super::*;

pub async fn item_exists(pool: &SqlitePool, item_id: i64) -> Result<bool> {
    Ok(
        sqlx::query_scalar::<_, i64>("SELECT 1 FROM items WHERE id = ?")
            .bind(item_id)
            .fetch_optional(pool)
            .await?
            .is_some(),
    )
}

pub async fn item_kind_by_id(pool: &SqlitePool, item_id: i64) -> Result<Option<String>> {
    Ok(sqlx::query_scalar("SELECT kind FROM items WHERE id = ?")
        .bind(item_id)
        .fetch_optional(pool)
        .await?)
}

/// Insert a deduplicated upload while preserving its raw and cleaned titles.
#[allow(clippy::too_many_arguments)]
pub async fn create_item(
    pool: &SqlitePool,
    scheme_tag: &str,
    structural_hash: &str,
    path: &str,
    size_bytes: i64,
    mtime: i64,
    format: &str,
    raw_title: &str,
    kind: &str,
    modality: &str,
    now: i64,
) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO items \
         (scheme_tag, structural_hash, deep_hash, path, size_bytes, mtime, format, title, raw_title, kind, modality, page_count, added_at, last_modified_at) \
         VALUES (?, ?, NULL, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, ?) RETURNING id",
    )
    .bind(scheme_tag)
    .bind(structural_hash)
    .bind(path)
    .bind(size_bytes)
    .bind(mtime)
    .bind(format)
    .bind(crate::media::title::clean(raw_title))
    .bind(raw_title)
    .bind(kind)
    .bind(modality)
    .bind(now)
    .bind(now)
    .fetch_one(pool)
    .await?)
}

/// Return the first item in a structural-identity bucket for ingest deduplication.
pub async fn item_by_bucket(
    pool: &SqlitePool,
    scheme_tag: &str,
    structural_hash: &str,
) -> Result<Option<ItemMeta>> {
    let row: Option<ItemMetaRow> = sqlx::query_as(
        "SELECT id, scheme_tag, structural_hash, title, raw_title, description, description_manual, description_source, page_count, path, size_bytes, kind, modality, modality_override, added_at, word_count, publisher, sort_creator \
         FROM items WHERE scheme_tag = ? AND structural_hash = ? LIMIT 1",
    )
    .bind(scheme_tag)
    .bind(structural_hash)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(ItemMeta::from))
}

/// Delete an item row and return its source path. Derived files remain caller-owned.
pub async fn delete_item(pool: &SqlitePool, item_id: i64) -> Result<Option<String>> {
    let path: Option<String> = sqlx::query_scalar("DELETE FROM items WHERE id = ? RETURNING path")
        .bind(item_id)
        .fetch_optional(pool)
        .await?;
    if path.is_some() {
        // Mixed-type entry neighbors have no FK cascade.
        clear_entry_neighbors(pool, "i", item_id).await?;
    }
    Ok(path)
}

pub async fn path_of(pool: &SqlitePool, item_id: i64) -> Result<Option<PathBuf>> {
    let row: Option<(String,)> = sqlx::query_as("SELECT path FROM items WHERE id = ?")
        .bind(item_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|(p,)| PathBuf::from(p)))
}

/// Load item metadata without opening its source file.
pub async fn item_meta(pool: &SqlitePool, item_id: i64) -> Result<Option<ItemMeta>> {
    let row: Option<ItemMetaRow> = sqlx::query_as(
        "SELECT id, scheme_tag, structural_hash, title, raw_title, description, description_manual, description_source, page_count, path, size_bytes, kind, modality, modality_override, added_at, word_count, publisher, sort_creator \
         FROM items WHERE id = ?",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(ItemMeta::from))
}

#[derive(sqlx::FromRow)]
struct ItemMetaRow {
    id: i64,
    scheme_tag: String,
    structural_hash: String,
    title: String,
    raw_title: Option<String>,
    description: Option<String>,
    description_manual: i64,
    description_source: Option<String>,
    page_count: Option<i64>,
    path: String,
    size_bytes: i64,
    kind: String,
    modality: String,
    modality_override: Option<String>,
    added_at: i64,
    word_count: Option<i64>,
    publisher: Option<String>,
    sort_creator: Option<String>,
}

impl From<ItemMetaRow> for ItemMeta {
    fn from(r: ItemMetaRow) -> Self {
        let format = if r.scheme_tag == "epub-structural-v1" {
            "EPUB".to_string()
        } else {
            std::path::Path::new(&r.path)
                .extension()
                .and_then(|ext| ext.to_str())
                .map(str::to_ascii_uppercase)
                .unwrap_or_else(|| "FILE".to_string())
        };
        ItemMeta {
            id: r.id,
            structural_hash: r.structural_hash,
            title: r.title,
            raw_title: r.raw_title,
            description: r.description,
            description_manual: r.description_manual != 0,
            description_source: r.description_source,
            page_count: r.page_count,
            path: r.path,
            size_bytes: r.size_bytes,
            kind: r.kind,
            modality: r
                .modality_override
                .clone()
                .unwrap_or_else(|| r.modality.clone()),
            modality_detected: r.modality,
            modality_override: r.modality_override,
            added_at: r.added_at,
            word_count: r.word_count,
            format,
            publisher: r.publisher,
            sort_creator: r.sort_creator,
        }
    }
}

pub struct ItemMeta {
    pub id: i64,
    pub structural_hash: String,
    pub title: String,
    pub raw_title: Option<String>,
    pub description: Option<String>,
    pub description_manual: bool,
    pub description_source: Option<String>,
    pub page_count: Option<i64>,
    pub path: String,
    pub size_bytes: i64,
    pub kind: String,
    pub modality: String,
    pub modality_detected: String,
    pub modality_override: Option<String>,
    pub added_at: i64,
    pub word_count: Option<i64>,
    pub format: String,
    pub publisher: Option<String>,
    pub sort_creator: Option<String>,
}

impl ItemMeta {
    pub fn search_title(&self) -> String {
        self.raw_title
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.title)
            .to_string()
    }
}

pub async fn set_page_count(pool: &SqlitePool, item_id: i64, count: i64) -> Result<()> {
    sqlx::query("UPDATE items SET page_count = ? WHERE id = ? AND page_count IS NULL")
        .bind(count)
        .bind(item_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Stored page range for one in-archive chapter.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ChapterRow {
    pub number_sort: f64,
    pub number_disp: Option<String>,
    pub title: Option<String>,
    pub start_page: i64,
    pub page_count: i64,
}

/// Replace parsed chapters and mark the item scanned, including an empty result.
pub async fn replace_item_chapters(
    write: &SqlitePool,
    item_id: i64,
    chapters: &[crate::media::chapters::Chapter],
) -> Result<()> {
    let mut tx = write.begin().await?;
    sqlx::query("DELETE FROM item_chapters WHERE item_id = ?")
        .bind(item_id)
        .execute(&mut *tx)
        .await?;
    for (idx, c) in chapters.iter().enumerate() {
        sqlx::query(
            "INSERT INTO item_chapters \
             (item_id, idx, number_sort, number_disp, title, start_page, page_count) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(item_id)
        .bind(idx as i64)
        .bind(c.number_sort)
        .bind(&c.number_disp)
        .bind(&c.title)
        .bind(c.start_page as i64)
        .bind(c.page_count as i64)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("UPDATE items SET chapters_done = 1 WHERE id = ?")
        .bind(item_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Return stored in-archive chapters in reading order.
pub async fn item_chapters(pool: &SqlitePool, item_id: i64) -> Result<Vec<ChapterRow>> {
    let rows = sqlx::query_as::<_, ChapterRow>(
        "SELECT number_sort, number_disp, title, start_page, page_count \
         FROM item_chapters WHERE item_id = ? ORDER BY idx",
    )
    .bind(item_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Whether chapter detection has already run for the current content.
pub async fn chapters_scanned(pool: &SqlitePool, item_id: i64) -> Result<bool> {
    let done: Option<i64> = sqlx::query_scalar("SELECT chapters_done FROM items WHERE id = ?")
        .bind(item_id)
        .fetch_optional(pool)
        .await?;
    Ok(done == Some(1))
}

/// Return the caller's last-read page, or none when unread.
pub async fn get_progress(pool: &SqlitePool, user_id: i64, item_id: i64) -> Result<Option<i64>> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT CAST(value AS INTEGER) FROM read_progress \
         WHERE user_id = ? AND item_id = ? AND unit = 'page'",
    )
    .bind(user_id)
    .bind(item_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(p,)| p))
}

/// Save clamped page progress, returning false for an unknown item.
pub async fn set_progress(
    pool: &SqlitePool,
    user_id: i64,
    item_id: i64,
    page: i64,
) -> Result<bool> {
    let now = crate::now_secs();
    let old_page: Option<f64> = sqlx::query_scalar(
        "SELECT value FROM read_progress WHERE user_id = ? AND item_id = ? AND unit = 'page'",
    )
    .bind(user_id)
    .bind(item_id)
    .fetch_optional(pool)
    .await?;
    let res = sqlx::query(
        "INSERT INTO read_progress (user_id, item_id, unit, value, updated_at) \
         SELECT ?, a.id, 'page', \
                CASE WHEN a.page_count IS NOT NULL AND a.page_count > 0 AND ? > a.page_count - 1 \
                     THEN a.page_count - 1 ELSE ? END, \
                ? \
         FROM items a WHERE a.id = ? \
         ON CONFLICT(user_id, item_id) DO UPDATE SET \
            unit = 'page', value = excluded.value, locator = NULL, updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(page)
    .bind(page)
    .bind(now)
    .bind(item_id)
    .execute(pool)
    .await?;
    let saved = res.rows_affected() > 0;
    if saved {
        let delta = page - old_page.unwrap_or(0.0) as i64;
        if let Err(e) = crate::intelligence::stats::record_activity(pool, user_id, now, delta).await
        {
            tracing::warn!("reading_activity update failed for user {user_id}: {e:#}");
        }
    }
    Ok(saved)
}

/// Return reflowable progression, opaque locator, and update time.
pub async fn get_reflowable_progress(
    pool: &SqlitePool,
    user_id: i64,
    item_id: i64,
) -> Result<Option<(f64, Option<String>, i64)>> {
    let row = sqlx::query_as(
        "SELECT value, locator, updated_at FROM read_progress \
         WHERE user_id = ? AND item_id = ? AND unit = 'percent'",
    )
    .bind(user_id)
    .bind(item_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Save reflowable progression and an opaque locator, returning false if unknown.
pub async fn set_reflowable_progress(
    pool: &SqlitePool,
    user_id: i64,
    item_id: i64,
    value: f64,
    locator: Option<&str>,
) -> Result<bool> {
    let value = value.clamp(0.0, 1.0);
    let now = crate::now_secs();
    let res = sqlx::query(
        "INSERT INTO read_progress (user_id, item_id, unit, value, locator, updated_at) \
         SELECT ?, a.id, 'percent', ?, ?, ? FROM items a WHERE a.id = ? \
         ON CONFLICT(user_id, item_id) DO UPDATE SET \
            unit = 'percent', value = excluded.value, locator = excluded.locator, \
            updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(value)
    .bind(locator)
    .bind(now)
    .bind(item_id)
    .execute(pool)
    .await?;
    let saved = res.rows_affected() > 0;
    if saved {
        if let Err(e) = crate::intelligence::stats::record_activity(pool, user_id, now, 0).await {
            tracing::warn!("reading_activity update failed for user {user_id}: {e:#}");
        }
    }
    Ok(saved)
}

/// Favorite an item idempotently, returning false if it is unknown.
pub async fn add_favorite(pool: &SqlitePool, user_id: i64, item_id: i64) -> Result<bool> {
    if !item_exists(pool, item_id).await? {
        return Ok(false);
    }
    let now = crate::now_secs();
    sqlx::query("INSERT OR IGNORE INTO favorites (user_id, item_id, created_at) VALUES (?, ?, ?)")
        .bind(user_id)
        .bind(item_id)
        .bind(now)
        .execute(pool)
        .await?;
    Ok(true)
}

/// Remove a favorite idempotently, returning false if the item is unknown.
pub async fn remove_favorite(pool: &SqlitePool, user_id: i64, item_id: i64) -> Result<bool> {
    if !item_exists(pool, item_id).await? {
        return Ok(false);
    }
    sqlx::query("DELETE FROM favorites WHERE user_id = ? AND item_id = ?")
        .bind(user_id)
        .bind(item_id)
        .execute(pool)
        .await?;
    Ok(true)
}

pub async fn is_favorited(pool: &SqlitePool, user_id: i64, item_id: i64) -> Result<bool> {
    let hit: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM favorites WHERE user_id = ? AND item_id = ?")
            .bind(user_id)
            .bind(item_id)
            .fetch_optional(pool)
            .await?;
    Ok(hit.is_some())
}

/// Set a half-star rating from 1–10, returning false for an unknown item.
pub async fn set_rating(pool: &SqlitePool, user_id: i64, item_id: i64, value: i64) -> Result<bool> {
    if !item_exists(pool, item_id).await? {
        return Ok(false);
    }
    let now = crate::now_secs();
    sqlx::query(
        "INSERT INTO ratings (user_id, item_id, value, updated_at) VALUES (?, ?, ?, ?) \
         ON CONFLICT(user_id, item_id) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(item_id)
    .bind(value)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(true)
}

/// Clear a rating idempotently, returning false for an unknown item.
pub async fn clear_rating(pool: &SqlitePool, user_id: i64, item_id: i64) -> Result<bool> {
    if !item_exists(pool, item_id).await? {
        return Ok(false);
    }
    sqlx::query("DELETE FROM ratings WHERE user_id = ? AND item_id = ?")
        .bind(user_id)
        .bind(item_id)
        .execute(pool)
        .await?;
    Ok(true)
}

/// Return the viewer's half-star rating from 1–10.
pub async fn get_rating(pool: &SqlitePool, user_id: i64, item_id: i64) -> Result<Option<i64>> {
    Ok(
        sqlx::query_scalar("SELECT value FROM ratings WHERE user_id = ? AND item_id = ?")
            .bind(user_id)
            .bind(item_id)
            .fetch_optional(pool)
            .await?,
    )
}

/// Supported paginated reading modes.
pub const READING_MODES: &[&str] = &["paged", "vertical"];

pub const DEFAULT_READING_MODE: &str = "paged";

/// Set a validated reading mode, returning false for an unknown item.
pub async fn set_reading_mode(
    pool: &SqlitePool,
    user_id: i64,
    item_id: i64,
    mode: &str,
) -> Result<bool> {
    if !READING_MODES.contains(&mode) {
        return Err(anyhow!("invalid reading mode: {mode:?}"));
    }
    if !item_exists(pool, item_id).await? {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO item_reading_mode (user_id, item_id, mode) VALUES (?, ?, ?) \
         ON CONFLICT(user_id, item_id) DO UPDATE SET mode = excluded.mode",
    )
    .bind(user_id)
    .bind(item_id)
    .bind(mode)
    .execute(pool)
    .await?;
    Ok(true)
}

pub async fn clear_reading_mode(pool: &SqlitePool, user_id: i64, item_id: i64) -> Result<bool> {
    if !item_exists(pool, item_id).await? {
        return Ok(false);
    }
    sqlx::query("DELETE FROM item_reading_mode WHERE user_id = ? AND item_id = ?")
        .bind(user_id)
        .bind(item_id)
        .execute(pool)
        .await?;
    Ok(true)
}

pub async fn get_reading_mode(pool: &SqlitePool, user_id: i64, item_id: i64) -> Result<String> {
    let row: Option<String> =
        sqlx::query_scalar("SELECT mode FROM item_reading_mode WHERE user_id = ? AND item_id = ?")
            .bind(user_id)
            .bind(item_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.unwrap_or_else(|| DEFAULT_READING_MODE.to_string()))
}

/// Store a cover dHash using SQLite's signed integer representation.
pub async fn set_phash(pool: &SqlitePool, item_id: i64, phash: u64) -> Result<()> {
    sqlx::query("UPDATE items SET phash = ? WHERE id = ?")
        .bind(phash as i64)
        .bind(item_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Return the cover-hash corpus used for fuzzy ownership matching.
pub async fn phash_corpus(pool: &SqlitePool) -> Result<Vec<(i64, i64, Option<i64>)>> {
    Ok(
        sqlx::query_as("SELECT id, phash, page_count FROM items WHERE phash IS NOT NULL")
            .fetch_all(pool)
            .await?,
    )
}

pub async fn item_ids_by_source_urls(
    pool: &SqlitePool,
    urls: &[String],
) -> Result<HashMap<String, i64>> {
    if urls.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = vec!["?"; urls.len()].join(", ");
    let sql = format!("SELECT url, item_id FROM item_sources WHERE url IN ({placeholders})");
    let mut q = sqlx::query_as::<_, (String, i64)>(AssertSqlSafe(sql));
    for u in urls {
        q = q.bind(u);
    }
    Ok(q.fetch_all(pool).await?.into_iter().collect())
}

pub async fn set_item_description(
    pool: &SqlitePool,
    item_id: i64,
    description: &str,
    source: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "UPDATE items SET description = ?, description_source = ? \
         WHERE id = ? AND description_manual = 0",
    )
    .bind(description)
    .bind(source)
    .bind(item_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn clear_item_description_from_source(
    pool: &SqlitePool,
    item_id: i64,
    source: &str,
) -> Result<bool> {
    let res = sqlx::query(
        "UPDATE items SET description = NULL, description_source = NULL \
         WHERE id = ? AND description_source = ? AND description_manual = 0",
    )
    .bind(item_id)
    .bind(source)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn set_item_title(pool: &SqlitePool, item_id: i64, title: &str) -> Result<bool> {
    let res = sqlx::query("UPDATE items SET title = ? WHERE id = ?")
        .bind(title)
        .bind(item_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn set_item_description_manual(
    pool: &SqlitePool,
    item_id: i64,
    description: Option<&str>,
) -> Result<bool> {
    let res = sqlx::query(
        "UPDATE items SET description = ?, description_manual = ?, description_source = NULL \
         WHERE id = ?",
    )
    .bind(description)
    .bind(description.is_some() as i64)
    .bind(item_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn set_item_modality_override(
    pool: &SqlitePool,
    item_id: i64,
    modality: Option<&str>,
) -> Result<bool> {
    let res = sqlx::query("UPDATE items SET modality_override = ? WHERE id = ?")
        .bind(modality)
        .bind(item_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn set_item_isbn(pool: &SqlitePool, item_id: i64, isbn: Option<&str>) -> Result<()> {
    sqlx::query("UPDATE items SET isbn = ? WHERE id = ?")
        .bind(isbn)
        .bind(item_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_item_publisher(
    pool: &SqlitePool,
    item_id: i64,
    publisher: Option<&str>,
) -> Result<()> {
    sqlx::query("UPDATE items SET publisher = ? WHERE id = ?")
        .bind(publisher)
        .bind(item_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_item_word_count(pool: &SqlitePool, item_id: i64, words: i64) -> Result<()> {
    sqlx::query("UPDATE items SET word_count = ? WHERE id = ?")
        .bind(words)
        .bind(item_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// A one-shot or collapsed series card on a reading shelf.
#[derive(Serialize, ToSchema)]
pub struct ContinueEntry {
    #[serde(rename = "type")]
    pub kind_of: String,
    pub id: i64,
    pub kind: String,
    pub name: String,
    pub page_count: Option<i64>,
    pub progress: Option<i64>,
    /// Effective rendering modality.
    pub modality: String,
    /// Reflowable progression from 0–1; omitted for paginated items.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    pub last_read_at: i64,
    /// Series-only leaf used for the cover.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_item_id: Option<i64>,
    /// Structural hash used as the cover thumbnail's `?v=` cache version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_version: Option<String>,
    /// Series-only leaf to resume.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_leaf_id: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct ContinueRow {
    id: i64,
    kind: String,
    title: String,
    page_count: Option<i64>,
    modality: String,
    unit: String,
    value: f64,
    updated_at: i64,
    series_id: Option<i64>,
    series_title: Option<String>,
    number_disp: Option<String>,
    structural_hash: String,
}

/// Return started but unfinished entries, most recently read first. Reflowable
/// items are considered finished at 98%.
pub async fn continue_reading(
    pool: &SqlitePool,
    user_id: i64,
    kind: Option<&str>,
    limit: i64,
) -> Result<Vec<ContinueEntry>> {
    reading_shelf(pool, user_id, kind, limit, false).await
}

pub async fn recently_finished(
    pool: &SqlitePool,
    user_id: i64,
    kind: Option<&str>,
    limit: i64,
) -> Result<Vec<ContinueEntry>> {
    reading_shelf(pool, user_id, kind, limit, true).await
}

/// Query either side of the derived completion threshold and collapse series leaves.
async fn reading_shelf(
    pool: &SqlitePool,
    user_id: i64,
    kind: Option<&str>,
    limit: i64,
    finished: bool,
) -> Result<Vec<ContinueEntry>> {
    let mut sql = String::from(
        "SELECT i.id, i.kind, i.title, i.page_count, i.modality, rp.unit, rp.value, rp.updated_at, \
                l.series_id, s.title AS series_title, l.number_disp, i.structural_hash \
         FROM read_progress rp \
         JOIN items i ON i.id = rp.item_id \
         LEFT JOIN item_series_leaf l ON l.item_id = i.id \
         LEFT JOIN series s ON s.id = l.series_id \
         WHERE rp.user_id = ?",
    );
    if finished {
        sql.push_str(
            " AND ((rp.unit = 'page'    AND i.page_count > 0 AND rp.value >= i.page_count - 1) \
               OR  (rp.unit = 'percent' AND rp.value >= 0.98))",
        );
    } else {
        sql.push_str(
            " AND NOT (rp.unit = 'page'    AND i.page_count > 0 AND rp.value >= i.page_count - 1) \
              AND NOT (rp.unit = 'percent' AND rp.value >= 0.98)",
        );
    }
    if kind.is_some() {
        sql.push_str(" AND i.kind = ?");
    }
    sql.push_str(" ORDER BY rp.updated_at DESC, rp.item_id DESC LIMIT ?");
    let mut q = sqlx::query_as::<_, ContinueRow>(AssertSqlSafe(sql)).bind(user_id);
    if let Some(k) = kind {
        q = q.bind(k);
    }
    let fetch = (limit * 3).clamp(limit, 200);
    let rows: Vec<ContinueRow> = q.bind(fetch).fetch_all(pool).await?;

    let mut seen_series: HashSet<i64> = HashSet::new();
    let mut out = Vec::new();
    for r in rows {
        if out.len() as i64 >= limit {
            break;
        }
        let progress = (r.unit == "page")
            .then(|| clamp_progress(Some(r.value as i64), r.page_count))
            .flatten();
        let value = (r.unit == "percent").then_some(r.value);
        let series_name = match r.series_title.as_deref() {
            Some(series) if crate::media::series::leaf_belongs_to_series(&r.title, series) => {
                match r.number_disp.as_deref() {
                    Some(nd) if !nd.trim().is_empty() => format!("{series} · {nd}"),
                    _ => r.title.clone(),
                }
            }
            _ => r.title.clone(),
        };
        match r.series_id {
            Some(sid) if seen_series.insert(sid) => out.push(ContinueEntry {
                kind_of: "series".to_string(),
                id: sid,
                kind: r.kind,
                name: series_name,
                page_count: r.page_count,
                progress,
                modality: r.modality.clone(),
                value,
                last_read_at: r.updated_at,
                cover_item_id: Some(r.id),
                cover_version: Some(r.structural_hash),
                resume_leaf_id: Some(r.id),
            }),
            Some(_) => {}
            None => out.push(ContinueEntry {
                kind_of: "item".to_string(),
                id: r.id,
                kind: r.kind,
                name: r.title,
                page_count: r.page_count,
                progress,
                modality: r.modality.clone(),
                value,
                last_read_at: r.updated_at,
                cover_item_id: None,
                cover_version: Some(r.structural_hash),
                resume_leaf_id: None,
            }),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::test_util::*;

    #[test]
    fn search_title_prefers_raw_bracketed_title() {
        let raw = "[Northwind Press (A. Reed)] Xingguang ︱ Starlight [Chinese] {Library Copy}";
        let m = meta_with_titles("Story", Some(raw));
        assert_eq!(m.search_title(), raw);
    }

    #[test]
    fn search_title_falls_back_to_clean_when_raw_absent() {
        assert_eq!(meta_with_titles("Story", None).search_title(), "Story");
        assert_eq!(
            meta_with_titles("Story", Some("   ")).search_title(),
            "Story"
        );
    }

    #[sqlx::test]
    async fn item_meta_selects_sort_creator(pool: SqlitePool) {
        let id = insert_item(&pool, "item-meta-author", 1).await;
        sqlx::query("UPDATE items SET sort_creator = 'cormac mccarthy' WHERE id = ?")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();

        let meta = item_meta(&pool, id).await.unwrap().unwrap();
        assert_eq!(meta.sort_creator.as_deref(), Some("cormac mccarthy"));
    }

    #[sqlx::test]
    async fn phash_and_source_url_match_helpers(pool: SqlitePool) {
        let a = insert_item(&pool, "a", 10).await;
        let b = insert_item(&pool, "b", 20).await;
        set_phash(&pool, a, 0xABCD).await.unwrap();
        set_item_source(&pool, b, "openlibrary", "https://openlibrary.org/g/1/")
            .await
            .unwrap();

        let m = item_ids_by_source_urls(
            &pool,
            &[
                "https://openlibrary.org/g/1/".to_string(),
                "https://openlibrary.org/g/2/".to_string(),
            ],
        )
        .await
        .unwrap();
        assert_eq!(m.get("https://openlibrary.org/g/1/"), Some(&b));
        assert!(!m.contains_key("https://openlibrary.org/g/2/"));

        let corpus = phash_corpus(&pool).await.unwrap();
        assert_eq!(corpus.len(), 1);
        assert_eq!(corpus[0].0, a);
        assert_eq!(corpus[0].1 as u64, 0xABCD);
    }

    #[sqlx::test]
    async fn recently_finished_is_the_continue_complement(pool: SqlitePool) {
        let user = a_user(&pool).await;
        let mut ids = Vec::new();
        for (path, hash) in [("/f/a.cbz", "fa"), ("/f/b.cbz", "fb")] {
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO items (scheme_tag, structural_hash, path, size_bytes, mtime, format, title, page_count, added_at, last_modified_at) \
                 VALUES ('zip-structural-v1', ?, ?, 1, 1, 'cbz', 't', 10, 1, 0) RETURNING id",
            )
            .bind(hash)
            .bind(path)
            .fetch_one(&pool)
            .await
            .unwrap();
            ids.push(id);
        }
        let (done_id, reading_id) = (ids[0], ids[1]);
        for (item, page) in [(done_id, 9_i64), (reading_id, 3)] {
            sqlx::query(
                "INSERT INTO read_progress (user_id, item_id, unit, value, updated_at) \
                 VALUES (?, ?, 'page', ?, 1)",
            )
            .bind(user)
            .bind(item)
            .bind(page)
            .execute(&pool)
            .await
            .unwrap();
        }

        let done = recently_finished(&pool, user, None, 10).await.unwrap();
        assert_eq!(done.len(), 1, "only the finished item");
        assert_eq!(done[0].id, done_id);

        let cont = continue_reading(&pool, user, None, 10).await.unwrap();
        assert_eq!(cont.len(), 1, "the unfinished item stays on continue");
        assert_eq!(cont[0].id, reading_id);
    }

    #[sqlx::test]
    async fn continue_reading_keeps_zero_page_items(pool: SqlitePool) {
        let user = a_user(&pool).await;
        let empty: i64 = sqlx::query_scalar(
            "INSERT INTO items (scheme_tag, structural_hash, path, size_bytes, mtime, format, title, page_count, added_at, last_modified_at) \
             VALUES ('zip-structural-v1', 'empty', '/p/e', 1, 1, 'cbz', 't', 0, 1, 0) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO read_progress (user_id, item_id, unit, value, updated_at) \
             VALUES (?, ?, 'page', 0, 1)",
        )
        .bind(user)
        .bind(empty)
        .execute(&pool)
        .await
        .unwrap();
        let shelf = continue_reading(&pool, user, None, 10).await.unwrap();
        assert!(
            shelf.iter().any(|e| e.id == empty),
            "a 0-page item with progress must stay on the shelf, not read as finished"
        );
    }

    #[sqlx::test]
    async fn delete_item_cascades_every_dependent(pool: SqlitePool) {
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();
        let user = a_user(&pool).await;
        let vid = insert_item(&pool, "victim", 1).await;
        let oid = insert_item(&pool, "other", 2).await;

        let tag = get_or_create_tag(&pool, "tag", "x").await.unwrap();
        add_item_tag(&pool, vid, tag, "none", "manual")
            .await
            .unwrap();
        reindex_item_tags(&pool, vid).await.unwrap();
        set_progress(&pool, user, vid, 3).await.unwrap();
        sqlx::query("INSERT INTO favorites (user_id, item_id, created_at) VALUES (?, ?, 0)")
            .bind(user)
            .bind(vid)
            .execute(&pool)
            .await
            .unwrap();
        set_item_source(&pool, vid, "anilist", "https://anilist.co/g/1/a/")
            .await
            .unwrap();
        replace_item_comments(&pool, vid, "anilist", &[comment("anilist", "c1", 7)])
            .await
            .unwrap();
        write_neighbors(&pool, vid, &[(oid, 1.0)]).await.unwrap();
        write_neighbors(&pool, oid, &[(vid, 1.0)]).await.unwrap();
        write_entry_neighbors(&pool, vid, &[(oid, 1.0)])
            .await
            .unwrap();
        write_entry_neighbors(&pool, oid, &[(vid, 1.0)])
            .await
            .unwrap();

        let path = delete_item(&pool, vid).await.unwrap();
        assert_eq!(path.as_deref(), Some("/p/victim"));

        for (table, col) in [
            ("items", "id"),
            ("item_tags", "item_id"),
            ("read_progress", "item_id"),
            ("favorites", "item_id"),
            ("item_sources", "item_id"),
            ("item_comments", "item_id"),
            ("items_fts", "rowid"),
        ] {
            let sql = format!("SELECT COUNT(*) FROM {table} WHERE {col} = ?");
            let c: i64 = sqlx::query_scalar(AssertSqlSafe(sql))
                .bind(vid)
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(c, 0, "{table} not cascaded");
        }
        let nb: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM item_neighbors WHERE item_id = ? OR neighbor_id = ?",
        )
        .bind(vid)
        .bind(vid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(nb, 0, "item_neighbors not cascaded (both directions)");
        let enb: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM entry_neighbors \
             WHERE (src_type = 'i' AND src_id = ?) OR (dst_type = 'i' AND dst_id = ?)",
        )
        .bind(vid)
        .bind(vid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(enb, 0, "entry_neighbors not cleared (both directions)");

        assert!(
            item_exists(&pool, oid).await.unwrap(),
            "the neighbour survives"
        );

        assert!(delete_item(&pool, 999_999).await.unwrap().is_none());
    }

    #[sqlx::test]
    async fn reading_mode_set_get_clear_and_default(pool: SqlitePool) {
        let user = a_user(&pool).await;
        let id = insert_item(&pool, "rm_item", 1).await;

        assert_eq!(get_reading_mode(&pool, user, id).await.unwrap(), "paged");

        assert!(set_reading_mode(&pool, user, id, "vertical").await.unwrap());
        assert_eq!(get_reading_mode(&pool, user, id).await.unwrap(), "vertical");
        assert!(set_reading_mode(&pool, user, id, "paged").await.unwrap());
        assert_eq!(get_reading_mode(&pool, user, id).await.unwrap(), "paged");

        assert!(clear_reading_mode(&pool, user, id).await.unwrap());
        assert_eq!(get_reading_mode(&pool, user, id).await.unwrap(), "paged");
        assert!(clear_reading_mode(&pool, user, id).await.unwrap());

        assert!(!set_reading_mode(&pool, user, 999_999, "vertical")
            .await
            .unwrap());
        assert!(set_reading_mode(&pool, user, id, "sideways").await.is_err());

        assert!(set_reading_mode(&pool, user, id, "vertical").await.unwrap());
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();
        delete_item(&pool, id).await.unwrap();
        let left: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM item_reading_mode WHERE item_id = ?")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(left, 0);
    }

    #[sqlx::test]
    async fn item_delete_cascades_tags(pool: SqlitePool) {
        let d1 = insert_item(&pool, "d1", 1).await;
        let t = get_or_create_tag(&pool, "tag", "x").await.unwrap();
        add_item_tag(&pool, d1, t, "none", "manual").await.unwrap();
        sqlx::query("DELETE FROM items WHERE id = ?")
            .bind(d1)
            .execute(&pool)
            .await
            .unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM item_tags")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0, "item_tags cascade-deleted with the item");
    }

    #[sqlx::test]
    async fn items_are_standalone_by_default(pool: SqlitePool) {
        let s1 = insert_item(&pool, "s1", 1).await;
        let sid: Option<i64> = sqlx::query_scalar("SELECT series_id FROM items WHERE id = ?")
            .bind(s1)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(sid, None);

        sqlx::query("INSERT INTO series (kind, title, added_at) VALUES ('manga', 'One Piece', 0)")
            .execute(&pool)
            .await
            .unwrap();
        let series_id: i64 = sqlx::query_scalar("SELECT id FROM series")
            .fetch_one(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE items SET series_id = ? WHERE id = ?")
            .bind(series_id)
            .bind(s1)
            .execute(&pool)
            .await
            .unwrap();
        let sid: Option<i64> = sqlx::query_scalar("SELECT series_id FROM items WHERE id = ?")
            .bind(s1)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(sid, Some(series_id));
    }

    #[sqlx::test]
    async fn continue_reading_recent_unfinished_first(pool: SqlitePool) {
        let uid = a_user(&pool).await;
        let mut id = std::collections::HashMap::new();
        for (h, at) in [("a", 1), ("b", 2), ("c", 3), ("d", 4), ("e", 5), ("f", 6)] {
            id.insert(h, insert_item(&pool, h, at).await);
        }
        let put = |item_id: i64, unit: &'static str, value: f64, updated: i64| {
            let p = pool.clone();
            async move {
                sqlx::query(
                    "INSERT INTO read_progress (user_id, item_id, unit, value, updated_at) \
                     VALUES (?, ?, ?, ?, ?)",
                )
                .bind(uid)
                .bind(item_id)
                .bind(unit)
                .bind(value)
                .bind(updated)
                .execute(&p)
                .await
                .unwrap();
            }
        };
        put(id["a"], "page", 2.0, 100).await;
        put(id["b"], "page", 0.0, 200).await;
        put(id["c"], "page", 4.0, 300).await;
        put(id["e"], "percent", 0.5, 400).await;
        put(id["f"], "percent", 0.99, 500).await;

        let shelf = continue_reading(&pool, uid, None, 20).await.unwrap();
        let got: Vec<i64> = shelf.iter().map(|e| e.id).collect();
        assert_eq!(
            got,
            vec![id["e"], id["b"], id["a"]],
            "newest-first; finished (page + percent) and unstarted excluded"
        );
        assert_eq!(shelf[0].progress, None, "percent item has no page progress");
        assert_eq!(
            shelf[1].progress,
            Some(0),
            "just-opened at page 0 still shows"
        );
        assert_eq!(shelf[2].progress, Some(2));

        let two = continue_reading(&pool, uid, None, 2).await.unwrap();
        assert_eq!(
            two.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![id["e"], id["b"]],
            "limit is respected"
        );
    }

    #[sqlx::test]
    async fn continue_reading_names_the_resumed_volume(pool: SqlitePool) {
        let uid = a_user(&pool).await;
        let series = |name: &'static str, folder: &'static str| {
            let p = pool.clone();
            async move {
                sqlx::query_scalar::<_, i64>(
                    "INSERT INTO series (kind, title, folder_path, added_at) \
                     VALUES ('manga', ?, ?, 10) RETURNING id",
                )
                .bind(name)
                .bind(folder)
                .fetch_one(&p)
                .await
                .unwrap()
            }
        };
        let leaf = |title: &'static str,
                    sid: i64,
                    disp: Option<&'static str>,
                    hash: &'static str,
                    at: i64| {
            let p = pool.clone();
            async move {
                let iid: i64 = sqlx::query_scalar(
                        "INSERT INTO items (scheme_tag, structural_hash, path, size_bytes, mtime, \
                         format, title, kind, page_count, added_at, last_modified_at, series_id) \
                         VALUES ('zip-structural-v1', ?, ?, 1, 1, 'cbz', ?, 'manga', 20, ?, 0, ?) RETURNING id",
                    )
                    .bind(hash)
                    .bind(format!("/p/{hash}"))
                    .bind(title)
                    .bind(at)
                    .bind(sid)
                    .fetch_one(&p)
                    .await
                    .unwrap();
                sqlx::query("INSERT INTO item_series_leaf (item_id, series_id, number_sort, number_disp) VALUES (?, ?, 1, ?)")
                        .bind(iid).bind(sid).bind(disp).execute(&p).await.unwrap();
                sqlx::query("INSERT INTO read_progress (user_id, item_id, unit, value, updated_at) VALUES (?, ?, 'page', 3, ?)")
                        .bind(uid).bind(iid).bind(at).execute(&p).await.unwrap();
                iid
            }
        };
        let cm = series("Chainsaw Man", "manga/Chainsaw Man").await;
        let mono = series("Monogatari Series", "manga/Monogatari").await;
        leaf("Chainsaw Man", cm, Some("Vol. 6"), "cm6", 100).await;
        let kizu = leaf("Kizumonogatari", mono, Some("Vol. 1"), "kizu", 200).await;

        let shelf = continue_reading(&pool, uid, None, 20).await.unwrap();
        assert_eq!(shelf.len(), 2, "one card per series");
        assert_eq!(
            shelf[0].name, "Kizumonogatari",
            "a distinct volume title is shown as-is (not the series name)"
        );
        assert_eq!(shelf[0].kind_of, "series");
        assert_eq!(shelf[0].resume_leaf_id, Some(kizu), "resumes the leaf");
        assert_eq!(
            shelf[1].name, "Chainsaw Man · Vol. 6",
            "a same-named volume shows the series + its display number"
        );
    }

    #[sqlx::test]
    async fn set_page_count_is_write_once(pool: SqlitePool) {
        let h: i64 = sqlx::query_scalar(
            "INSERT INTO items \
             (scheme_tag, structural_hash, path, size_bytes, mtime, format, title, page_count, added_at, last_modified_at) \
             VALUES ('zip-structural-v1', 'h', '/p', 1, 1, 'cbz', 't', NULL, 0, 0) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        set_page_count(&pool, h, 10).await.unwrap();
        let pc: Option<i64> = sqlx::query_scalar("SELECT page_count FROM items WHERE id = ?")
            .bind(h)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(pc, Some(10));

        set_page_count(&pool, h, 20).await.unwrap();
        let pc: Option<i64> = sqlx::query_scalar("SELECT page_count FROM items WHERE id = ?")
            .bind(h)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(pc, Some(10), "page_count not rewritten once set");
    }

    #[sqlx::test]
    async fn set_progress_clamps_past_the_end(pool: SqlitePool) {
        let h: i64 = sqlx::query_scalar(
            "INSERT INTO items \
             (scheme_tag, structural_hash, path, size_bytes, mtime, format, title, page_count, added_at, last_modified_at) \
             VALUES ('zip-structural-v1', 'h', '/p', 1, 1, 'cbz', 't', 20, 0, 0) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO users (username, password_hash, role, created_at) VALUES ('u', 'x', 'user', 0)")
            .execute(&pool)
            .await
            .unwrap();
        let uid: i64 = sqlx::query_scalar("SELECT id FROM users")
            .fetch_one(&pool)
            .await
            .unwrap();

        assert!(set_progress(&pool, uid, h, 99_999).await.unwrap());
        let (unit, stored): (String, i64) =
            sqlx::query_as("SELECT unit, CAST(value AS INTEGER) FROM read_progress")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(unit, "page");
        assert_eq!(stored, 19, "stored page clamped to page_count - 1");
    }
}
