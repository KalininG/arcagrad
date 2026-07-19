//! Series detail, metadata, favorites, and release trackers.

use anyhow::Result;
use serde::Serialize;
use sqlx::{AssertSqlSafe, SqlitePool};
use std::collections::HashSet;

use super::*;

pub struct SeriesDetail {
    pub id: i64,
    pub kind: String,
    pub title: String,
    pub description: Option<String>,
    pub description_manual: bool,
    pub description_source: Option<String>,
    pub added_at: i64,
    pub cover_item_id: Option<i64>,
    pub resume_leaf_id: Option<i64>,
    pub read_count: i64,
    pub favorited: bool,
    pub rating: Option<i64>,
    pub tags: Vec<ItemTag>,
    pub sources: Vec<ItemSource>,
    pub leaves: Vec<SeriesLeaf>,
}

pub struct SeriesLeaf {
    pub item_id: i64,
    pub name: String,
    pub number_disp: Option<String>,
    pub number_sort: f64,
    pub page_count: Option<i64>,
    pub progress: Option<i64>,
    pub modality: String,
    pub value: Option<f64>,
    pub word_count: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct SeriesLeafRow {
    item_id: i64,
    title: String,
    number_disp: Option<String>,
    number_sort: f64,
    page_count: Option<i64>,
    modality: String,
    unit: Option<String>,
    value: Option<f64>,
    word_count: Option<i64>,
}

/// Load a series in leaf order with the viewer's progress and resume target.
/// Reflowable leaves are considered finished at 98%.
pub async fn series_detail(
    pool: &SqlitePool,
    user_id: i64,
    series_id: i64,
) -> Result<Option<SeriesDetail>> {
    #[allow(clippy::type_complexity)]
    let head: Option<(
        i64,
        String,
        String,
        Option<String>,
        bool,
        Option<String>,
        i64,
    )> = sqlx::query_as(
        "SELECT id, kind, title, description, description_manual, description_source, \
                    added_at FROM series WHERE id = ?",
    )
    .bind(series_id)
    .fetch_optional(pool)
    .await?;
    let Some((id, kind, title, description, description_manual, description_source, added_at)) =
        head
    else {
        return Ok(None);
    };
    let rows: Vec<SeriesLeafRow> = sqlx::query_as(
        "SELECT l.item_id, i.title, l.number_disp, l.number_sort, i.page_count, \
                i.modality, rp.unit, rp.value, i.word_count \
         FROM item_series_leaf l \
         JOIN items i ON i.id = l.item_id \
         LEFT JOIN read_progress rp ON rp.item_id = l.item_id AND rp.user_id = ? \
         WHERE l.series_id = ? \
         ORDER BY l.number_sort, l.item_id",
    )
    .bind(user_id)
    .bind(series_id)
    .fetch_all(pool)
    .await?;
    let cover_item_id = rows.first().map(|r| r.item_id);
    let leaves: Vec<SeriesLeaf> = rows
        .into_iter()
        .map(|r| {
            let is_page = r.unit.as_deref() == Some("page");
            let is_pct = r.unit.as_deref() == Some("percent");
            SeriesLeaf {
                progress: is_page
                    .then(|| clamp_progress(Some(r.value.unwrap_or(0.0) as i64), r.page_count))
                    .flatten(),
                value: is_pct.then(|| r.value.unwrap_or(0.0)),
                item_id: r.item_id,
                name: r.title,
                number_disp: r.number_disp,
                number_sort: r.number_sort,
                page_count: r.page_count,
                modality: r.modality,
                word_count: r.word_count,
            }
        })
        .collect();

    let done = |l: &SeriesLeaf| {
        if l.modality == "reflowable" {
            l.value.is_some_and(|v| v >= 0.98)
        } else {
            l.page_count
                .is_some_and(|pc| pc > 0 && l.progress.is_some_and(|p| p >= pc - 1))
        }
    };
    let read_count = leaves.iter().filter(|l| done(l)).count() as i64;
    let resume_leaf_id = leaves
        .iter()
        .find(|l| !done(l))
        .or_else(|| leaves.last())
        .map(|l| l.item_id);
    let mut tags = series_tags_with_counts(pool, series_id).await?;
    let leaf_backed: HashSet<(String, String)> = sqlx::query_as::<_, (String, String)>(
        "SELECT DISTINCT t.namespace, t.value \
         FROM item_series_leaf l \
         JOIN item_tags at ON at.item_id = l.item_id \
         JOIN tags t ON t.id = at.tag_id \
         WHERE l.series_id = ?",
    )
    .bind(series_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect();
    for t in &mut tags {
        t.leaf = Some(leaf_backed.contains(&(t.namespace.clone(), t.value.clone())));
    }
    let favorited = is_series_favorited(pool, user_id, series_id).await?;
    let rating = get_series_rating(pool, user_id, series_id).await?;
    let sources = series_sources(pool, series_id).await?;

    Ok(Some(SeriesDetail {
        id,
        kind,
        title,
        description,
        description_manual,
        description_source,
        added_at,
        cover_item_id,
        resume_leaf_id,
        read_count,
        favorited,
        rating,
        tags,
        sources,
        leaves,
    }))
}

/// Return effective tags from the series and all of its leaves.
pub async fn series_tags(pool: &SqlitePool, series_id: i64) -> Result<Vec<ItemTag>> {
    Ok(series_tags_for_ids(pool, &[series_id])
        .await?
        .remove(&series_id)
        .unwrap_or_default())
}

/// Add global catalog-card counts to a series' effective tags.
pub async fn series_tags_with_counts(pool: &SqlitePool, series_id: i64) -> Result<Vec<ItemTag>> {
    let mut tags = series_tags(pool, series_id).await?;
    if tags.is_empty() {
        return Ok(tags);
    }
    let counts = tag_counts_for_series(pool, series_id).await?;
    for t in &mut tags {
        t.count = counts.get(&(t.namespace.clone(), t.value.clone())).copied();
    }
    Ok(tags)
}

pub struct LeafContext {
    pub series_id: i64,
    pub series_title: String,
    pub number_disp: Option<String>,
    pub prev_leaf_id: Option<i64>,
    pub next_leaf_id: Option<i64>,
}

/// Resolve adjacent leaves using `(number_sort, item_id)` order.
pub async fn series_leaf_context(pool: &SqlitePool, item_id: i64) -> Result<Option<LeafContext>> {
    let this: Option<(i64, f64, Option<String>)> = sqlx::query_as(
        "SELECT series_id, number_sort, number_disp FROM item_series_leaf WHERE item_id = ?",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await?;
    let Some((series_id, number_sort, number_disp)) = this else {
        return Ok(None);
    };
    let series_title: String = sqlx::query_scalar("SELECT title FROM series WHERE id = ?")
        .bind(series_id)
        .fetch_one(pool)
        .await?;
    let prev_leaf_id: Option<i64> = sqlx::query_scalar(
        "SELECT item_id FROM item_series_leaf \
         WHERE series_id = ? AND (number_sort, item_id) < (?, ?) \
         ORDER BY number_sort DESC, item_id DESC LIMIT 1",
    )
    .bind(series_id)
    .bind(number_sort)
    .bind(item_id)
    .fetch_optional(pool)
    .await?;
    let next_leaf_id: Option<i64> = sqlx::query_scalar(
        "SELECT item_id FROM item_series_leaf \
         WHERE series_id = ? AND (number_sort, item_id) > (?, ?) \
         ORDER BY number_sort ASC, item_id ASC LIMIT 1",
    )
    .bind(series_id)
    .bind(number_sort)
    .bind(item_id)
    .fetch_optional(pool)
    .await?;
    Ok(Some(LeafContext {
        series_id,
        series_title,
        number_disp,
        prev_leaf_id,
        next_leaf_id,
    }))
}

pub(crate) async fn series_exists(pool: &SqlitePool, series_id: i64) -> Result<bool> {
    Ok(
        sqlx::query_scalar::<_, i64>("SELECT 1 FROM series WHERE id = ?")
            .bind(series_id)
            .fetch_optional(pool)
            .await?
            .is_some(),
    )
}

pub async fn add_series_favorite(pool: &SqlitePool, user_id: i64, series_id: i64) -> Result<bool> {
    if !series_exists(pool, series_id).await? {
        return Ok(false);
    }
    let now = crate::now_secs();
    sqlx::query(
        "INSERT OR IGNORE INTO series_favorites (user_id, series_id, created_at) VALUES (?, ?, ?)",
    )
    .bind(user_id)
    .bind(series_id)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(true)
}

pub async fn remove_series_favorite(
    pool: &SqlitePool,
    user_id: i64,
    series_id: i64,
) -> Result<bool> {
    if !series_exists(pool, series_id).await? {
        return Ok(false);
    }
    sqlx::query("DELETE FROM series_favorites WHERE user_id = ? AND series_id = ?")
        .bind(user_id)
        .bind(series_id)
        .execute(pool)
        .await?;
    Ok(true)
}

pub async fn is_series_favorited(pool: &SqlitePool, user_id: i64, series_id: i64) -> Result<bool> {
    let hit: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM series_favorites WHERE user_id = ? AND series_id = ?")
            .bind(user_id)
            .bind(series_id)
            .fetch_optional(pool)
            .await?;
    Ok(hit.is_some())
}

pub async fn set_series_rating(
    pool: &SqlitePool,
    user_id: i64,
    series_id: i64,
    value: i64,
) -> Result<bool> {
    if !series_exists(pool, series_id).await? {
        return Ok(false);
    }
    let now = crate::now_secs();
    sqlx::query(
        "INSERT INTO series_ratings (user_id, series_id, value, updated_at) VALUES (?, ?, ?, ?) \
         ON CONFLICT(user_id, series_id) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(series_id)
    .bind(value)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(true)
}

pub async fn clear_series_rating(pool: &SqlitePool, user_id: i64, series_id: i64) -> Result<bool> {
    if !series_exists(pool, series_id).await? {
        return Ok(false);
    }
    sqlx::query("DELETE FROM series_ratings WHERE user_id = ? AND series_id = ?")
        .bind(user_id)
        .bind(series_id)
        .execute(pool)
        .await?;
    Ok(true)
}

pub async fn get_series_rating(
    pool: &SqlitePool,
    user_id: i64,
    series_id: i64,
) -> Result<Option<i64>> {
    Ok(
        sqlx::query_scalar("SELECT value FROM series_ratings WHERE user_id = ? AND series_id = ?")
            .bind(user_id)
            .bind(series_id)
            .fetch_optional(pool)
            .await?,
    )
}

pub async fn series_kind_by_id(pool: &SqlitePool, series_id: i64) -> Result<Option<String>> {
    Ok(sqlx::query_scalar("SELECT kind FROM series WHERE id = ?")
        .bind(series_id)
        .fetch_optional(pool)
        .await?)
}

pub async fn series_scrape_hint_by_id(
    pool: &SqlitePool,
    series_id: i64,
) -> Result<Option<(String, Option<String>)>> {
    Ok(sqlx::query_as(
        "SELECT s.title, (SELECT COALESCE(i.modality_override, i.modality) FROM items i \
         WHERE i.series_id = s.id ORDER BY i.id LIMIT 1) \
         FROM series s WHERE s.id = ?",
    )
    .bind(series_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn set_series_source(
    pool: &SqlitePool,
    series_id: i64,
    source: &str,
    url: &str,
) -> Result<bool> {
    if !series_exists(pool, series_id).await? {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO series_sources (series_id, source, url) VALUES (?, ?, ?) \
         ON CONFLICT(series_id, source) DO UPDATE SET url = excluded.url",
    )
    .bind(series_id)
    .bind(source)
    .bind(url)
    .execute(pool)
    .await?;
    Ok(true)
}

/// Set a source-owned description without overwriting a manual edit.
pub async fn set_series_description(
    pool: &SqlitePool,
    series_id: i64,
    description: &str,
    source: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "UPDATE series SET description = ?, description_source = ? \
         WHERE id = ? AND description_manual = 0",
    )
    .bind(description)
    .bind(source)
    .bind(series_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Clear a description only when the given source owns it.
pub async fn clear_series_description_from_source(
    pool: &SqlitePool,
    series_id: i64,
    source: &str,
) -> Result<bool> {
    let res = sqlx::query(
        "UPDATE series SET description = NULL, description_source = NULL \
         WHERE id = ? AND description_source = ? AND description_manual = 0",
    )
    .bind(series_id)
    .bind(source)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Set a manual title that scanner reconciliation will preserve.
pub async fn set_series_title(pool: &SqlitePool, series_id: i64, title: &str) -> Result<bool> {
    let res = sqlx::query("UPDATE series SET title = ?, title_manual = 1 WHERE id = ?")
        .bind(title)
        .bind(series_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

/// Set a manual description, or clear it to restore scrape ownership.
pub async fn set_series_description_manual(
    pool: &SqlitePool,
    series_id: i64,
    description: Option<&str>,
) -> Result<bool> {
    let res = sqlx::query(
        "UPDATE series SET description = ?, description_manual = ?, description_source = NULL \
         WHERE id = ?",
    )
    .bind(description)
    .bind(description.is_some() as i64)
    .bind(series_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn delete_series_source(pool: &SqlitePool, series_id: i64, source: &str) -> Result<bool> {
    let res = sqlx::query("DELETE FROM series_sources WHERE series_id = ? AND source = ?")
        .bind(series_id)
        .bind(source)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct SeriesTracker {
    pub plugin_id: String,
    pub reference: String,
    pub created_at: i64,
    pub updated_at: i64,
}

pub async fn series_trackers(pool: &SqlitePool, series_id: i64) -> Result<Vec<SeriesTracker>> {
    Ok(sqlx::query_as(
        "SELECT plugin_id, reference, created_at, updated_at FROM series_trackers \
         WHERE series_id = ? ORDER BY plugin_id",
    )
    .bind(series_id)
    .fetch_all(pool)
    .await?)
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct TrackedSeries {
    pub series_id: i64,
    pub title: String,
    pub kind: String,
    pub leaf_count: i64,
    pub cover_item_id: Option<i64>,
    pub cover_version: Option<String>,
    pub plugin_id: String,
    pub reference: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_checked_at: Option<i64>,
    pub last_error: Option<String>,
    pub next_label: Option<String>,
    pub next_release_date: Option<String>,
}

pub async fn tracked_series(pool: &SqlitePool) -> Result<Vec<TrackedSeries>> {
    let sql = format!(
        "SELECT st.series_id, s.title, s.kind, \
                (SELECT COUNT(*) FROM item_series_leaf lc WHERE lc.series_id = s.id) AS leaf_count, \
                {SERIES_COVER_ITEM_SUBQ} AS cover_item_id, \
                {SERIES_COVER_VERSION_SUBQ} AS cover_version, \
                st.plugin_id, st.reference, st.created_at, st.updated_at, \
                cps.last_checked_at, cps.last_error, \
                (SELECT u.label FROM series_upcoming u \
                 WHERE u.series_id = st.series_id AND u.provider = st.plugin_id \
                   AND u.release_date >= date('now', 'localtime') \
                 ORDER BY u.release_date, u.id LIMIT 1) AS next_label, \
                (SELECT u.release_date FROM series_upcoming u \
                 WHERE u.series_id = st.series_id AND u.provider = st.plugin_id \
                   AND u.release_date >= date('now', 'localtime') \
                 ORDER BY u.release_date, u.id LIMIT 1) AS next_release_date \
         FROM series_trackers st \
         JOIN series s ON s.id = st.series_id \
         LEFT JOIN calendar_provider_state cps ON cps.provider = st.plugin_id \
         ORDER BY CASE WHEN cps.last_error IS NOT NULL THEN 0 ELSE 1 END, \
                  s.title COLLATE NOCASE, st.plugin_id"
    );
    Ok(sqlx::query_as(AssertSqlSafe(sql)).fetch_all(pool).await?)
}

/// Replace a tracker and invalidate releases when its reference changes.
pub async fn set_series_tracker(
    pool: &SqlitePool,
    series_id: i64,
    plugin_id: &str,
    reference: &str,
) -> Result<bool> {
    let mut tx = pool.begin().await?;
    let res = sqlx::query(
        "INSERT INTO series_trackers(series_id, plugin_id, reference) \
         SELECT id, ?, ? FROM series WHERE id = ? \
         ON CONFLICT(series_id, plugin_id) DO UPDATE SET \
           reference = excluded.reference, updated_at = unixepoch()",
    )
    .bind(plugin_id)
    .bind(reference)
    .bind(series_id)
    .execute(&mut *tx)
    .await?;
    if res.rows_affected() > 0 {
        sqlx::query("DELETE FROM series_upcoming WHERE series_id = ? AND provider = ?")
            .bind(series_id)
            .bind(plugin_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(res.rows_affected() > 0)
}

/// Remove a tracker and its derived cached releases.
pub async fn delete_series_tracker(
    pool: &SqlitePool,
    series_id: i64,
    plugin_id: &str,
) -> Result<bool> {
    let mut tx = pool.begin().await?;
    let res = sqlx::query("DELETE FROM series_trackers WHERE series_id = ? AND plugin_id = ?")
        .bind(series_id)
        .bind(plugin_id)
        .execute(&mut *tx)
        .await?;
    if res.rows_affected() > 0 {
        sqlx::query("DELETE FROM series_upcoming WHERE series_id = ? AND provider = ?")
            .bind(series_id)
            .bind(plugin_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(res.rows_affected() > 0)
}

pub async fn series_sources(pool: &SqlitePool, series_id: i64) -> Result<Vec<ItemSource>> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT source, url FROM series_sources WHERE series_id = ? ORDER BY source",
    )
    .bind(series_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(source, url)| ItemSource { source, url })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::test_util::*;
    use sqlx::SqlitePool;

    #[sqlx::test]
    async fn series_tags_with_counts_annotates_popularity(pool: SqlitePool) {
        let one = insert_item(&pool, "one", 1).await;
        let shared = get_or_create_tag(&pool, "tag", "shared").await.unwrap();
        add_item_tag(&pool, one, shared, "none", "manual")
            .await
            .unwrap();

        let sid: i64 = sqlx::query_scalar(
            "INSERT INTO series (kind, title, folder_path, added_at) VALUES ('manga','S','manga/S',1) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let leaf: i64 = sqlx::query_scalar(
            "INSERT INTO items (scheme_tag, structural_hash, path, size_bytes, mtime, format, title, page_count, added_at, last_modified_at, series_id) \
             VALUES ('zip-structural-v1','h1','/p/l1',1,1,'cbz','l1',5,1,0,?) RETURNING id",
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
        add_item_tag(&pool, leaf, shared, "none", "manual")
            .await
            .unwrap();
        let dem = get_or_create_tag(&pool, "demographic", "seinen")
            .await
            .unwrap();
        add_series_tag(&pool, sid, dem, "none", "anilist")
            .await
            .unwrap();

        let tags = series_tags_with_counts(&pool, sid).await.unwrap();
        let count = |ns: &str, v: &str| {
            tags.iter()
                .find(|t| t.namespace == ns && t.value == v)
                .and_then(|t| t.count)
        };
        assert_eq!(
            count("tag", "shared"),
            Some(2),
            "leaf tag shared with a one-shot"
        );
        assert_eq!(count("demographic", "seinen"), Some(1), "series-level tag");
    }

    #[sqlx::test]
    async fn series_trackers_are_independent_and_remove_their_calendar_rows(pool: SqlitePool) {
        let series_id: i64 = sqlx::query_scalar(
            "INSERT INTO series (kind, title, folder_path, added_at) \
             VALUES ('manga', 'Akira', 'manga/Akira', 0) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(
            set_series_source(&pool, series_id, "viz", "https://www.viz.com/hima-ten")
                .await
                .unwrap()
        );
        assert!(
            set_series_tracker(&pool, series_id, "viz", "https://www.viz.com/hima-ten")
                .await
                .unwrap()
        );
        sqlx::query(
            "INSERT INTO series_upcoming \
             (series_id, provider, provider_release_id, reference_source, label, \
              release_date, date_precision, date_status, fetched_at) \
             VALUES (?, 'viz', 'viz:8980', 'viz', 'Vol. 2', \
                     '2026-09-01', 'day', 'announced', 0)",
        )
        .bind(series_id)
        .execute(&pool)
        .await
        .unwrap();

        assert!(delete_series_source(&pool, series_id, "viz").await.unwrap());
        let remaining_after_metadata_delete: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM series_upcoming WHERE series_id = ?")
                .bind(series_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(remaining_after_metadata_delete, 1);
        assert_eq!(series_trackers(&pool, series_id).await.unwrap().len(), 1);

        assert!(delete_series_tracker(&pool, series_id, "viz")
            .await
            .unwrap());
        let remaining_after_tracker_delete: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM series_upcoming WHERE series_id = ?")
                .bind(series_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(remaining_after_tracker_delete, 0);
    }
}
