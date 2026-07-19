//! Recommendation corpus, candidate, and precomputed-neighbor queries.

use anyhow::Result;
use futures::stream::{StreamExt, TryStreamExt};
use sqlx::{AssertSqlSafe, SqlitePool};
use std::collections::{HashMap, HashSet};

use super::*;
use crate::intelligence::recommend::{self, Corpus};

/// `(tag_id, namespace, value, document_frequency)`.
pub type TagMeta = (i64, String, String, i64);

pub async fn count_all_items(pool: &SqlitePool) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM items")
        .fetch_one(pool)
        .await?)
}

/// Return same-kind items sharing distinctive tags, ordered by overlap.
pub async fn similar_candidates(
    pool: &SqlitePool,
    item_id: i64,
    distinctive: &[i64],
    kind: &str,
    limit: i64,
) -> Result<Vec<i64>> {
    if distinctive.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = vec!["?"; distinctive.len()].join(", ");
    let sql = format!(
        "SELECT it.item_id FROM item_tags it \
         JOIN items c ON c.id = it.item_id \
         WHERE it.tag_id IN ({placeholders}) AND it.item_id != ? AND c.kind = ? \
         GROUP BY it.item_id \
         ORDER BY COUNT(DISTINCT it.tag_id) DESC, it.item_id DESC \
         LIMIT ?"
    );
    let mut q = sqlx::query_scalar::<_, i64>(AssertSqlSafe(sql));
    for &id in distinctive {
        q = q.bind(id);
    }
    q = q.bind(item_id).bind(kind).bind(limit);
    Ok(q.fetch_all(pool).await?)
}

/// Conservative bind limit compatible with older SQLite builds.
const MAX_IN_LIST: usize = 900;

/// Matches the configured read-pool size.
const READ_POOL_CONCURRENCY: usize = 16;

pub async fn tag_vectors_for_items(
    pool: &SqlitePool,
    item_ids: &[i64],
) -> Result<HashMap<i64, Vec<i64>>> {
    let mut map: HashMap<i64, Vec<i64>> = HashMap::new();
    for chunk in item_ids.chunks(MAX_IN_LIST) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "SELECT DISTINCT item_id, tag_id FROM item_tags WHERE item_id IN ({placeholders})"
        );
        let mut q = sqlx::query_as::<_, (i64, i64)>(AssertSqlSafe(sql));
        for &id in chunk {
            q = q.bind(id);
        }
        for (item_id, tag_id) in q.fetch_all(pool).await? {
            map.entry(item_id).or_default().push(tag_id);
        }
    }
    Ok(map)
}

pub async fn all_tag_metas(pool: &SqlitePool) -> Result<Vec<TagMeta>> {
    Ok(sqlx::query_as(
        "SELECT t.id, t.namespace, t.value, COUNT(DISTINCT it.item_id) AS df \
         FROM tags t JOIN item_tags it ON it.tag_id = t.id \
         GROUP BY t.id, t.namespace, t.value",
    )
    .fetch_all(pool)
    .await?)
}

pub async fn item_tag_ids(pool: &SqlitePool, item_id: i64) -> Result<Vec<i64>> {
    Ok(
        sqlx::query_scalar("SELECT DISTINCT tag_id FROM item_tags WHERE item_id = ?")
            .bind(item_id)
            .fetch_all(pool)
            .await?,
    )
}

pub async fn tagged_item_ids(pool: &SqlitePool) -> Result<Vec<i64>> {
    Ok(
        sqlx::query_scalar("SELECT DISTINCT item_id FROM item_tags ORDER BY item_id")
            .fetch_all(pool)
            .await?,
    )
}

pub async fn tagged_series_leaf_ids(pool: &SqlitePool) -> Result<Vec<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT it.item_id FROM item_tags it \
         JOIN items i ON i.id = it.item_id \
         WHERE i.series_id IS NOT NULL ORDER BY it.item_id",
    )
    .fetch_all(pool)
    .await?)
}

pub async fn series_leaf_ids(pool: &SqlitePool, ids: &[i64]) -> Result<HashSet<i64>> {
    let mut out = HashSet::new();
    for chunk in ids.chunks(MAX_IN_LIST) {
        if chunk.is_empty() {
            continue;
        }
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql =
            format!("SELECT id FROM items WHERE series_id IS NOT NULL AND id IN ({placeholders})");
        let mut q = sqlx::query_scalar::<_, i64>(AssertSqlSafe(sql));
        for &id in chunk {
            q = q.bind(id);
        }
        out.extend(q.fetch_all(pool).await?);
    }
    Ok(out)
}

/// Build item-level IDF metadata once for a recommendation generation.
pub async fn build_corpus(pool: &SqlitePool) -> Result<Corpus> {
    let n = count_all_items(pool).await?;
    let metas = all_tag_metas(pool).await?;
    Ok(Corpus::from_metas(&metas, n))
}

/// Generate candidates from distinctive tags, then rerank with the shared corpus.
pub async fn neighbors_of(
    pool: &SqlitePool,
    corpus: &Corpus,
    item_id: i64,
) -> Result<Vec<(i64, f32)>> {
    let tag_ids = item_tag_ids(pool, item_id).await?;
    let distinctive = corpus.distinctive(&tag_ids);
    if distinctive.is_empty() {
        return Ok(Vec::new());
    }
    let Some(kind) = item_kind_by_id(pool, item_id).await? else {
        return Ok(Vec::new());
    };
    let candidates = similar_candidates(
        pool,
        item_id,
        &distinctive,
        &kind,
        crate::intelligence::recommend::CANDIDATE_LIMIT,
    )
    .await?;
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let vectors = tag_vectors_for_items(pool, &candidates).await?;
    Ok(corpus
        .scorer()
        .rerank(&tag_ids, vectors.into_iter().collect()))
}

pub async fn items_kind_by_ids(pool: &SqlitePool, ids: &[i64]) -> Result<HashMap<i64, String>> {
    let mut map = HashMap::new();
    for chunk in ids.chunks(MAX_IN_LIST) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!("SELECT id, kind FROM items WHERE id IN ({placeholders})");
        let mut q = sqlx::query_as::<_, (i64, String)>(AssertSqlSafe(sql));
        for &id in chunk {
            q = q.bind(id);
        }
        map.extend(q.fetch_all(pool).await?);
    }
    Ok(map)
}

/// Batch neighbor generation while sharing candidate tag-vector reads.
pub async fn neighbors_of_batch(
    pool: &SqlitePool,
    corpus: &Corpus,
    ids: &[i64],
) -> Result<Vec<(i64, Vec<(i64, f32)>)>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let own_vectors = tag_vectors_for_items(pool, ids).await?;
    let kinds = items_kind_by_ids(pool, ids).await?;

    let per_item: Vec<(i64, Vec<i64>, Vec<i64>)> = futures::stream::iter(ids.iter().copied())
        .map(|id| {
            let own_vectors = &own_vectors;
            let kinds = &kinds;
            async move {
                let tag_ids = own_vectors.get(&id).cloned().unwrap_or_default();
                let distinctive = corpus.distinctive(&tag_ids);
                let Some(kind) = (if distinctive.is_empty() {
                    None
                } else {
                    kinds.get(&id)
                }) else {
                    return anyhow::Ok((id, tag_ids, Vec::new()));
                };
                let candidates = similar_candidates(
                    pool,
                    id,
                    &distinctive,
                    kind,
                    crate::intelligence::recommend::CANDIDATE_LIMIT,
                )
                .await?;
                anyhow::Ok((id, tag_ids, candidates))
            }
        })
        .buffer_unordered(READ_POOL_CONCURRENCY)
        .try_collect()
        .await?;

    let mut union_candidates: HashSet<i64> = HashSet::new();
    for (_, _, candidates) in &per_item {
        union_candidates.extend(candidates.iter().copied());
    }
    let union_ids: Vec<i64> = union_candidates.into_iter().collect();
    let shared_vectors = tag_vectors_for_items(pool, &union_ids).await?;

    Ok(per_item
        .into_iter()
        .map(|(id, tag_ids, candidates)| {
            if candidates.is_empty() {
                return (id, Vec::new());
            }
            let vectors: Vec<(i64, Vec<i64>)> = candidates
                .iter()
                .filter_map(|c| shared_vectors.get(c).map(|v| (*c, v.clone())))
                .collect();
            (id, corpus.scorer().rerank(&tag_ids, vectors))
        })
        .collect())
}

/// Return sources that must be recomputed when this neighbor changes.
pub async fn neighbor_holders(pool: &SqlitePool, item_id: i64) -> Result<Vec<i64>> {
    Ok(
        sqlx::query_scalar("SELECT DISTINCT item_id FROM item_neighbors WHERE neighbor_id = ?")
            .bind(item_id)
            .fetch_all(pool)
            .await?,
    )
}

pub async fn write_neighbors(
    pool: &SqlitePool,
    item_id: i64,
    neighbors: &[(i64, f32)],
) -> Result<()> {
    write_neighbors_batch(pool, &[(item_id, neighbors.to_vec())]).await
}

/// Replace several neighbor lists in one transaction.
pub async fn write_neighbors_batch(
    pool: &SqlitePool,
    items: &[(i64, Vec<(i64, f32)>)],
) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    for (item_id, neighbors) in items {
        sqlx::query("DELETE FROM item_neighbors WHERE item_id = ?")
            .bind(item_id)
            .execute(&mut *tx)
            .await?;
        for (neighbor_id, score) in neighbors {
            sqlx::query(
                "INSERT INTO item_neighbors (item_id, neighbor_id, score) VALUES (?, ?, ?)",
            )
            .bind(item_id)
            .bind(neighbor_id)
            .bind(*score as f64)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

/// Return favorited or completed items; favorites take precedence when duplicated.
pub async fn liked_items(pool: &SqlitePool, user_id: i64) -> Result<Vec<(i64, bool, i64)>> {
    let favs: Vec<(i64, i64)> =
        sqlx::query_as("SELECT item_id, created_at FROM favorites WHERE user_id = ?")
            .bind(user_id)
            .fetch_all(pool)
            .await?;
    let done: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT rp.item_id, rp.updated_at FROM read_progress rp JOIN items i ON i.id = rp.item_id \
         WHERE rp.user_id = ? \
           AND ((rp.unit = 'page'    AND i.page_count > 0 AND rp.value >= i.page_count - 1) \
             OR (rp.unit = 'percent' AND rp.value >= 0.98))",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    let mut map: HashMap<i64, (bool, i64)> = HashMap::new();
    for (id, ts) in done {
        map.insert(id, (false, ts));
    }
    for (id, ts) in favs {
        map.insert(id, (true, ts));
    }
    Ok(map
        .into_iter()
        .map(|(id, (fav, ts))| (id, fav, ts))
        .collect())
}

pub async fn favorited_series(pool: &SqlitePool, user_id: i64) -> Result<Vec<(i64, i64)>> {
    Ok(
        sqlx::query_as("SELECT series_id, created_at FROM series_favorites WHERE user_id = ?")
            .bind(user_id)
            .fetch_all(pool)
            .await?,
    )
}

pub async fn leaf_series_map(pool: &SqlitePool, item_ids: &[i64]) -> Result<HashMap<i64, i64>> {
    let mut map = HashMap::new();
    for chunk in item_ids.chunks(MAX_IN_LIST) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "SELECT item_id, series_id FROM item_series_leaf WHERE item_id IN ({placeholders})"
        );
        let mut q = sqlx::query_as::<_, (i64, i64)>(AssertSqlSafe(sql));
        for &id in chunk {
            q = q.bind(id);
        }
        for (item_id, series_id) in q.fetch_all(pool).await? {
            map.insert(item_id, series_id);
        }
    }
    Ok(map)
}

pub async fn neighbors_of_entries(pool: &SqlitePool, keys: &[i64]) -> Result<Vec<i64>> {
    let mut out: HashSet<i64> = HashSet::new();
    let item_ids: Vec<i64> = keys.iter().copied().filter(|k| *k > 0).collect();
    let series_ids: Vec<i64> = keys
        .iter()
        .copied()
        .filter(|k| *k < 0)
        .map(|k| -k)
        .collect();
    for (source_type, ids) in [("i", item_ids), ("s", series_ids)] {
        for chunk in ids.chunks(MAX_IN_LIST) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = vec!["?"; chunk.len()].join(", ");
            let sql = format!(
                "SELECT DISTINCT dst_type, dst_id FROM entry_neighbors \
                 WHERE src_type = ? AND src_id IN ({placeholders})"
            );
            let mut q = sqlx::query_as::<_, (String, i64)>(AssertSqlSafe(sql)).bind(source_type);
            for &id in chunk {
                q = q.bind(id);
            }
            for (dt, did) in q.fetch_all(pool).await? {
                out.insert(if dt == "s" { -did } else { did });
            }
        }
    }
    Ok(out.into_iter().collect())
}

/// Rank catalog-entry candidates from confidence- and recency-weighted taste.
pub async fn recommend_for_you(
    pool: &SqlitePool,
    corpus: &Corpus,
    user_id: i64,
    now: i64,
) -> Result<Vec<(i64, f32)>> {
    let liked = liked_items(pool, user_id).await?;
    let fav_series = favorited_series(pool, user_id).await?;
    if liked.is_empty() && fav_series.is_empty() {
        return Ok(Vec::new());
    }

    let liked_item_ids: Vec<i64> = liked.iter().map(|(id, _, _)| *id).collect();
    let leaf_map = leaf_series_map(pool, &liked_item_ids).await?;

    fn note(m: &mut HashMap<i64, (bool, i64)>, key: i64, fav: bool, ts: i64) {
        match m.get_mut(&key) {
            Some(e) => {
                if (fav && !e.0) || (fav == e.0 && ts > e.1) {
                    *e = (fav, ts);
                }
            }
            None => {
                m.insert(key, (fav, ts));
            }
        }
    }
    let mut signal: HashMap<i64, (bool, i64)> = HashMap::new();
    for (id, fav, ts) in &liked {
        let key = leaf_map.get(id).map(|s| -s).unwrap_or(*id);
        note(&mut signal, key, *fav, *ts);
    }
    for (sid, ts) in &fav_series {
        note(&mut signal, -sid, true, *ts);
    }

    let liked_w: Vec<(i64, f32)> = signal
        .iter()
        .map(|(key, (fav, ts))| {
            let confidence = if *fav {
                recommend::FAVORITE_WEIGHT
            } else {
                1.0
            };
            let age_days = (now - ts).max(0) as f32 / 86_400.0;
            let recency = 0.5_f32.powf(age_days / recommend::TASTE_HALF_LIFE_DAYS);
            (*key, confidence * recency)
        })
        .collect();
    let liked_keys: HashSet<i64> = signal.keys().copied().collect();

    let mut candidate_keys: HashSet<i64> =
        neighbors_of_entries(pool, &liked_keys.iter().copied().collect::<Vec<_>>())
            .await?
            .into_iter()
            .collect();
    for k in &liked_keys {
        candidate_keys.remove(k);
    }
    if candidate_keys.is_empty() {
        return Ok(Vec::new());
    }
    let candidates: Vec<i64> = candidate_keys.into_iter().collect();

    let mut all: Vec<i64> = liked_keys.iter().copied().collect();
    all.extend(candidates.iter().copied());
    let vectors = entry_tag_vectors(pool, &all).await?;

    let taste = corpus.scorer().taste_vector(&liked_w, &vectors);
    if taste.is_empty() {
        return Ok(Vec::new());
    }
    let cand_vecs: Vec<(i64, Vec<i64>)> = candidates
        .iter()
        .map(|k| (*k, vectors.get(k).cloned().unwrap_or_default()))
        .collect();
    Ok(corpus.scorer().rank_for_you(&taste, cand_vecs))
}

pub const ITEM_NEIGHBORS_INDEX: &str = "item_neighbors";
pub const ENTRY_NEIGHBORS_INDEX: &str = "entry_neighbors";
const RECOMMENDATION_INDEX_VERSION: i64 = 1;

/// Check the explicit sweep marker; an empty graph can still be complete.
pub async fn recommendation_index_ready(pool: &SqlitePool, name: &str) -> Result<bool> {
    let version: Option<i64> =
        sqlx::query_scalar("SELECT version FROM recommendation_index_state WHERE name = ?")
            .bind(name)
            .fetch_optional(pool)
            .await?;
    Ok(version == Some(RECOMMENDATION_INDEX_VERSION))
}

pub async fn invalidate_recommendation_index(pool: &SqlitePool, name: &str) -> Result<()> {
    sqlx::query("DELETE FROM recommendation_index_state WHERE name = ?")
        .bind(name)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn mark_recommendation_index_ready(pool: &SqlitePool, name: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO recommendation_index_state (name, version, completed_at) \
         VALUES (?, ?, unixepoch()) \
         ON CONFLICT(name) DO UPDATE SET version = excluded.version, \
           completed_at = excluded.completed_at",
    )
    .bind(name)
    .bind(RECOMMENDATION_INDEX_VERSION)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn clear_item_neighbor_sources(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM item_neighbors")
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn clear_item_neighbor_source(pool: &SqlitePool, item_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM item_neighbors WHERE item_id = ?")
        .bind(item_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn read_neighbors(
    pool: &SqlitePool,
    item_id: i64,
    limit: i64,
) -> Result<Vec<(i64, f32)>> {
    let rows: Vec<(i64, f64)> = sqlx::query_as(
        "SELECT neighbor_id, score FROM item_neighbors \
         WHERE item_id = ? ORDER BY score DESC LIMIT ?",
    )
    .bind(item_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id, s)| (id, s as f32)).collect())
}

/// Encode a series as a negative entry key; positive keys are one-shot items.
pub fn entry_key_series(series_id: i64) -> i64 {
    -series_id
}

/// Decode a signed entry key into database type and positive id.
pub fn decode_entry_key(key: i64) -> (&'static str, i64) {
    if key < 0 {
        ("s", -key)
    } else {
        ("i", key)
    }
}

/// Build the unified one-shot and effective-series tag relation.
fn entry_tags() -> String {
    format!(
        "SELECT it.item_id AS ekey, it.tag_id AS tag_id, i.kind AS kind \
           FROM item_tags it JOIN items i ON i.id = it.item_id \
           WHERE i.series_id IS NULL \
         UNION \
         SELECT -s.id AS ekey, et.tag_id AS tag_id, s.kind AS kind \
           FROM series s JOIN ({SERIES_EFFECTIVE_TAGS}) et ON et.series_id = s.id"
    )
}

pub async fn all_entry_tag_metas(pool: &SqlitePool) -> Result<Vec<TagMeta>> {
    let sql = format!(
        "SELECT t.id, t.namespace, t.value, COUNT(DISTINCT e.ekey) AS df \
         FROM tags t JOIN ({}) e ON e.tag_id = t.id \
         GROUP BY t.id, t.namespace, t.value",
        entry_tags()
    );
    Ok(sqlx::query_as(AssertSqlSafe(sql)).fetch_all(pool).await?)
}

pub async fn count_entries(pool: &SqlitePool) -> Result<i64> {
    let oneshots: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM items WHERE series_id IS NULL")
        .fetch_one(pool)
        .await?;
    let series: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM series")
        .fetch_one(pool)
        .await?;
    Ok(oneshots + series)
}

/// Build entry-level IDF metadata, counting a series once per tag.
pub async fn build_entry_corpus(pool: &SqlitePool) -> Result<Corpus> {
    let n = count_entries(pool).await?;
    let metas = all_entry_tag_metas(pool).await?;
    Ok(Corpus::from_metas(&metas, n))
}

pub async fn entry_tag_vectors(pool: &SqlitePool, keys: &[i64]) -> Result<HashMap<i64, Vec<i64>>> {
    let mut map: HashMap<i64, Vec<i64>> = HashMap::new();
    let item_ids: Vec<i64> = keys.iter().copied().filter(|k| *k > 0).collect();
    let series_ids: Vec<i64> = keys
        .iter()
        .copied()
        .filter(|k| *k < 0)
        .map(|k| -k)
        .collect();
    for (id, tags) in tag_vectors_for_items(pool, &item_ids).await? {
        map.insert(id, tags);
    }
    for chunk in series_ids.chunks(MAX_IN_LIST) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "SELECT et.series_id, et.tag_id FROM ({SERIES_EFFECTIVE_TAGS}) et \
             WHERE et.series_id IN ({placeholders})"
        );
        let mut q = sqlx::query_as::<_, (i64, i64)>(AssertSqlSafe(sql));
        for &id in chunk {
            q = q.bind(id);
        }
        for (sid, tag_id) in q.fetch_all(pool).await? {
            map.entry(-sid).or_default().push(tag_id);
        }
    }
    Ok(map)
}

pub async fn entry_kind(pool: &SqlitePool, key: i64) -> Result<Option<String>> {
    if key < 0 {
        Ok(sqlx::query_scalar("SELECT kind FROM series WHERE id = ?")
            .bind(-key)
            .fetch_optional(pool)
            .await?)
    } else {
        item_kind_by_id(pool, key).await
    }
}

/// Return same-kind catalog entries sharing distinctive tags.
pub async fn similar_entry_candidates(
    pool: &SqlitePool,
    target_key: i64,
    distinctive: &[i64],
    kind: &str,
    limit: i64,
) -> Result<Vec<i64>> {
    if distinctive.is_empty() {
        return Ok(Vec::new());
    }
    // A series can accumulate more distinctive tags than one SQL bind list permits.
    let distinctive = &distinctive[..distinctive.len().min(MAX_IN_LIST)];
    let placeholders = vec!["?"; distinctive.len()].join(", ");
    let sql = format!(
        "SELECT e.ekey FROM ({}) e \
         WHERE e.tag_id IN ({placeholders}) AND e.kind = ? AND e.ekey != ? \
         GROUP BY e.ekey \
         ORDER BY COUNT(DISTINCT e.tag_id) DESC, e.ekey DESC \
         LIMIT ?",
        entry_tags()
    );
    let mut q = sqlx::query_scalar::<_, i64>(AssertSqlSafe(sql));
    for &id in distinctive {
        q = q.bind(id);
    }
    q = q.bind(kind).bind(target_key).bind(limit);
    Ok(q.fetch_all(pool).await?)
}

pub async fn entry_neighbors_of(
    pool: &SqlitePool,
    corpus: &Corpus,
    key: i64,
) -> Result<Vec<(i64, f32)>> {
    let own = entry_tag_vectors(pool, &[key]).await?;
    let tag_ids = own.get(&key).cloned().unwrap_or_default();
    let distinctive = corpus.distinctive(&tag_ids);
    if distinctive.is_empty() {
        return Ok(Vec::new());
    }
    let Some(kind) = entry_kind(pool, key).await? else {
        return Ok(Vec::new());
    };
    let candidates = similar_entry_candidates(
        pool,
        key,
        &distinctive,
        &kind,
        crate::intelligence::recommend::CANDIDATE_LIMIT,
    )
    .await?;
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let vectors = entry_tag_vectors(pool, &candidates).await?;
    Ok(corpus
        .scorer()
        .rerank(&tag_ids, vectors.into_iter().collect()))
}

pub async fn entry_kinds(pool: &SqlitePool, keys: &[i64]) -> Result<HashMap<i64, String>> {
    let mut map = HashMap::new();
    let item_ids: Vec<i64> = keys.iter().copied().filter(|k| *k > 0).collect();
    let series_ids: Vec<i64> = keys
        .iter()
        .copied()
        .filter(|k| *k < 0)
        .map(|k| -k)
        .collect();
    map.extend(items_kind_by_ids(pool, &item_ids).await?);
    for chunk in series_ids.chunks(MAX_IN_LIST) {
        if chunk.is_empty() {
            continue;
        }
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!("SELECT id, kind FROM series WHERE id IN ({placeholders})");
        let mut q = sqlx::query_as::<_, (i64, String)>(AssertSqlSafe(sql));
        for &id in chunk {
            q = q.bind(id);
        }
        for (sid, kind) in q.fetch_all(pool).await? {
            map.insert(-sid, kind);
        }
    }
    Ok(map)
}

/// Batch entry-neighbor generation while sharing candidate vector reads.
pub async fn entry_neighbors_of_batch(
    pool: &SqlitePool,
    corpus: &Corpus,
    keys: &[i64],
) -> Result<Vec<(i64, Vec<(i64, f32)>)>> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let own_vectors = entry_tag_vectors(pool, keys).await?;
    let kinds = entry_kinds(pool, keys).await?;

    let per_key: Vec<(i64, Vec<i64>, Vec<i64>)> = futures::stream::iter(keys.iter().copied())
        .map(|key| {
            let own_vectors = &own_vectors;
            let kinds = &kinds;
            async move {
                let tag_ids = own_vectors.get(&key).cloned().unwrap_or_default();
                let distinctive = corpus.distinctive(&tag_ids);
                let Some(kind) = (if distinctive.is_empty() {
                    None
                } else {
                    kinds.get(&key)
                }) else {
                    return anyhow::Ok((key, tag_ids, Vec::new()));
                };
                let candidates = similar_entry_candidates(
                    pool,
                    key,
                    &distinctive,
                    kind,
                    crate::intelligence::recommend::CANDIDATE_LIMIT,
                )
                .await?;
                anyhow::Ok((key, tag_ids, candidates))
            }
        })
        .buffer_unordered(READ_POOL_CONCURRENCY)
        .try_collect()
        .await?;

    let mut union: HashSet<i64> = HashSet::new();
    for (_, _, candidates) in &per_key {
        union.extend(candidates.iter().copied());
    }
    let union_keys: Vec<i64> = union.into_iter().collect();
    let shared_vectors = entry_tag_vectors(pool, &union_keys).await?;

    Ok(per_key
        .into_iter()
        .map(|(key, tag_ids, candidates)| {
            if candidates.is_empty() {
                return (key, Vec::new());
            }
            let vectors: Vec<(i64, Vec<i64>)> = candidates
                .iter()
                .filter_map(|c| shared_vectors.get(c).map(|v| (*c, v.clone())))
                .collect();
            (key, corpus.scorer().rerank(&tag_ids, vectors))
        })
        .collect())
}

pub async fn tagged_entries(pool: &SqlitePool) -> Result<Vec<i64>> {
    let sql = format!("SELECT DISTINCT ekey FROM ({})", entry_tags());
    Ok(sqlx::query_scalar(AssertSqlSafe(sql))
        .fetch_all(pool)
        .await?)
}

pub async fn write_entry_neighbors(
    pool: &SqlitePool,
    key: i64,
    neighbors: &[(i64, f32)],
) -> Result<()> {
    write_entry_neighbors_batch(pool, &[(key, neighbors.to_vec())]).await
}

/// Replace several entry-neighbor lists in one transaction.
pub async fn write_entry_neighbors_batch(
    pool: &SqlitePool,
    entries: &[(i64, Vec<(i64, f32)>)],
) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    for (key, neighbors) in entries {
        let (st, sid) = decode_entry_key(*key);
        sqlx::query("DELETE FROM entry_neighbors WHERE src_type = ? AND src_id = ?")
            .bind(st)
            .bind(sid)
            .execute(&mut *tx)
            .await?;
        for (nkey, score) in neighbors {
            let (dt, did) = decode_entry_key(*nkey);
            sqlx::query(
                "INSERT INTO entry_neighbors (src_type, src_id, dst_type, dst_id, score) \
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(st)
            .bind(sid)
            .bind(dt)
            .bind(did)
            .bind(*score as f64)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

pub async fn read_entry_neighbors(
    pool: &SqlitePool,
    key: i64,
    limit: i64,
) -> Result<Vec<(i64, f32)>> {
    let (st, sid) = decode_entry_key(key);
    let rows: Vec<(String, i64, f64)> = sqlx::query_as(
        "SELECT dst_type, dst_id, score FROM entry_neighbors \
         WHERE src_type = ? AND src_id = ? ORDER BY score DESC LIMIT ?",
    )
    .bind(st)
    .bind(sid)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(t, id, s)| (if t == "s" { -id } else { id }, s as f32))
        .collect())
}

/// Return entry sources that must be recomputed when this destination changes.
pub async fn entry_neighbor_holders(pool: &SqlitePool, key: i64) -> Result<Vec<i64>> {
    let (dt, did) = decode_entry_key(key);
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT DISTINCT src_type, src_id FROM entry_neighbors WHERE dst_type = ? AND dst_id = ?",
    )
    .bind(dt)
    .bind(did)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(st, sid)| if st == "s" { -sid } else { sid })
        .collect())
}

pub async fn clear_entry_neighbor_sources(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM entry_neighbors")
        .execute(pool)
        .await?;
    Ok(())
}

/// Remove both directions for an entry; this mixed-type table has no FK cascade.
pub async fn clear_entry_neighbors(pool: &SqlitePool, kind: &str, id: i64) -> Result<()> {
    sqlx::query(
        "DELETE FROM entry_neighbors \
         WHERE (src_type = ? AND src_id = ?) OR (dst_type = ? AND dst_id = ?)",
    )
    .bind(kind)
    .bind(id)
    .bind(kind)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::test_util::*;
    use sqlx::SqlitePool;

    #[sqlx::test]
    async fn recommendation_readiness_is_explicit_not_row_count(pool: SqlitePool) {
        assert!(
            !recommendation_index_ready(&pool, ITEM_NEIGHBORS_INDEX)
                .await
                .unwrap(),
            "a new index has not completed a sweep"
        );
        mark_recommendation_index_ready(&pool, ITEM_NEIGHBORS_INDEX)
            .await
            .unwrap();
        assert!(
            recommendation_index_ready(&pool, ITEM_NEIGHBORS_INDEX)
                .await
                .unwrap(),
            "an index can be ready while its neighbour table is legitimately empty"
        );
        invalidate_recommendation_index(&pool, ITEM_NEIGHBORS_INDEX)
            .await
            .unwrap();
        assert!(!recommendation_index_ready(&pool, ITEM_NEIGHBORS_INDEX)
            .await
            .unwrap());
    }

    #[sqlx::test]
    async fn liked_items_excludes_zero_page_items(pool: SqlitePool) {
        let user = a_user(&pool).await;
        let good = insert_item(&pool, "good", 1).await;
        let empty: i64 = sqlx::query_scalar(
            "INSERT INTO items (scheme_tag, structural_hash, path, size_bytes, mtime, format, title, page_count, added_at, last_modified_at) \
             VALUES ('zip-structural-v1', 'empty', '/p/e', 1, 1, 'cbz', 't', 0, 1, 0) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        for (id, val) in [(good, 4.0_f64), (empty, 0.0)] {
            sqlx::query(
                "INSERT INTO read_progress (user_id, item_id, unit, value, updated_at) \
                 VALUES (?, ?, 'page', ?, 1)",
            )
            .bind(user)
            .bind(id)
            .bind(val)
            .execute(&pool)
            .await
            .unwrap();
        }
        let liked: Vec<i64> = liked_items(&pool, user)
            .await
            .unwrap()
            .into_iter()
            .map(|(id, _, _)| id)
            .collect();
        assert!(liked.contains(&good), "a finished real item counts");
        assert!(
            !liked.contains(&empty),
            "a 0-page item must not count as finished"
        );
    }

    #[sqlx::test]
    async fn similar_candidates_restricts_to_same_kind(pool: SqlitePool) {
        let mut id = std::collections::HashMap::new();
        for (h, k) in [("m1", "manga"), ("m2", "manga"), ("d1", "comics")] {
            let iid: i64 = sqlx::query_scalar(
                "INSERT INTO items (scheme_tag, structural_hash, path, size_bytes, mtime, format, title, kind, modality, page_count, added_at, last_modified_at) \
                 VALUES ('zip-structural-v1', ?, ?, 1, 1, 'cbz', ?, ?, 'paginated', 5, 1, 0) RETURNING id",
            )
            .bind(h)
            .bind(format!("/p/{h}"))
            .bind(h)
            .bind(k)
            .fetch_one(&pool)
            .await
            .unwrap();
            id.insert(h, iid);
        }
        let tag = get_or_create_tag(&pool, "creator", "x").await.unwrap();
        for h in ["m1", "m2", "d1"] {
            add_item_tag(&pool, id[h], tag, "none", "manual")
                .await
                .unwrap();
        }
        let m1 = id["m1"];
        let m2 = id["m2"];

        let cands = similar_candidates(&pool, m1, &[tag], "manga", 50)
            .await
            .unwrap();
        assert_eq!(cands, vec![m2], "only the same-kind sharer, not cross-kind");
    }

    #[sqlx::test]
    async fn entry_neighbors_are_cross_type_over_effective_tags(pool: SqlitePool) {
        async fn item(pool: &SqlitePool, hash: &str, kind: &str, series: Option<i64>) -> i64 {
            sqlx::query_scalar(
                "INSERT INTO items (scheme_tag, structural_hash, path, size_bytes, mtime, format, title, kind, modality, series_id, page_count, added_at, last_modified_at) \
                 VALUES ('zip-structural-v1', ?, ?, 1, 1, 'cbz', ?, ?, 'paginated', ?, 5, 1, 0) RETURNING id",
            )
            .bind(hash).bind(format!("/p/{hash}")).bind(hash).bind(kind).bind(series)
            .fetch_one(pool).await.unwrap()
        }
        async fn series(pool: &SqlitePool, title: &str) -> i64 {
            sqlx::query_scalar(
                "INSERT INTO series (kind, title, folder_path, added_at) VALUES ('manga', ?, ?, 1) RETURNING id",
            )
            .bind(title).bind(format!("manga/{title}")).fetch_one(pool).await.unwrap()
        }
        async fn leaf(pool: &SqlitePool, item_id: i64, series_id: i64) {
            sqlx::query(
                "INSERT INTO item_series_leaf (item_id, series_id, number_sort) VALUES (?, ?, 1.0)",
            )
            .bind(item_id)
            .bind(series_id)
            .execute(pool)
            .await
            .unwrap();
        }
        async fn tag(pool: &SqlitePool, item_id: i64, value: &str) {
            let t = get_or_create_tag(pool, "tag", value).await.unwrap();
            add_item_tag(pool, item_id, t, "none", "manual")
                .await
                .unwrap();
        }

        let a = series(&pool, "A").await;
        let la1 = item(&pool, "la1", "manga", Some(a)).await;
        leaf(&pool, la1, a).await;
        tag(&pool, la1, "action").await;
        tag(&pool, la1, "demons").await;
        let la2 = item(&pool, "la2", "manga", Some(a)).await;
        leaf(&pool, la2, a).await;
        tag(&pool, la2, "action").await;
        tag(&pool, la2, "school").await;

        let b = series(&pool, "B").await;
        let lb1 = item(&pool, "lb1", "manga", Some(b)).await;
        leaf(&pool, lb1, b).await;
        let action = get_or_create_tag(&pool, "tag", "action").await.unwrap();
        add_series_tag(&pool, b, action, "none", "anilist")
            .await
            .unwrap();

        let o1 = item(&pool, "o1", "manga", None).await;
        tag(&pool, o1, "action").await;
        tag(&pool, o1, "demons").await;
        let o2 = item(&pool, "o2", "manga", None).await;
        tag(&pool, o2, "romance").await;
        let o3 = item(&pool, "o3", "comics", None).await;
        tag(&pool, o3, "action").await;
        tag(&pool, o3, "demons").await;

        let corpus = build_entry_corpus(&pool).await.unwrap();

        let a_key = entry_key_series(a);
        let nb = entry_neighbors_of(&pool, &corpus, a_key).await.unwrap();
        let batched = entry_neighbors_of_batch(&pool, &corpus, &[a_key])
            .await
            .unwrap();
        assert_eq!(batched, vec![(a_key, nb.clone())], "batch == single");
        let keys: Vec<i64> = nb.iter().map(|(k, _)| *k).collect();
        assert!(keys.contains(&o1), "cross-type: a one-shot is a neighbour");
        assert!(
            keys.contains(&entry_key_series(b)),
            "same-kind series neighbour"
        );
        assert!(!keys.contains(&o2), "unrelated one-shot excluded");
        assert!(!keys.contains(&o3), "cross-kind excluded");
        assert_eq!(
            nb[0].0, o1,
            "the stronger (2 shared tags) one-shot ranks first"
        );

        let b_nb = entry_neighbors_of(&pool, &corpus, entry_key_series(b))
            .await
            .unwrap();
        assert!(
            !b_nb.is_empty(),
            "a series with only series-level tags still recommends"
        );

        let o1_nb = entry_neighbors_of(&pool, &corpus, o1).await.unwrap();
        let o1_keys: Vec<i64> = o1_nb.iter().map(|(k, _)| *k).collect();
        assert!(
            o1_keys.contains(&entry_key_series(a)),
            "a one-shot recommends the cross-type series it shares tags with; got {o1_keys:?}"
        );

        write_entry_neighbors(&pool, a_key, &nb).await.unwrap();
        let read = read_entry_neighbors(&pool, a_key, 50).await.unwrap();
        let read_keys: Vec<i64> = read.iter().map(|(k, _)| *k).collect();
        assert_eq!(read_keys, keys, "encoded neighbour keys survive write→read");
    }

    #[sqlx::test]
    async fn for_you_includes_favorited_series_and_cross_type(pool: SqlitePool) {
        async fn item(pool: &SqlitePool, hash: &str, kind: &str, series: Option<i64>) -> i64 {
            sqlx::query_scalar(
                "INSERT INTO items (scheme_tag, structural_hash, path, size_bytes, mtime, format, title, kind, modality, series_id, page_count, added_at, last_modified_at) \
                 VALUES ('zip-structural-v1', ?, ?, 1, 1, 'cbz', ?, ?, 'paginated', ?, 5, 1, 0) RETURNING id",
            )
            .bind(hash).bind(format!("/p/{hash}")).bind(hash).bind(kind).bind(series)
            .fetch_one(pool).await.unwrap()
        }
        async fn tag(pool: &SqlitePool, item_id: i64, value: &str) {
            let t = get_or_create_tag(pool, "tag", value).await.unwrap();
            add_item_tag(pool, item_id, t, "none", "manual")
                .await
                .unwrap();
        }

        let uid: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, role, created_at) VALUES ('u','x','user',0) RETURNING id",
        )
        .fetch_one(&pool).await.unwrap();

        let a: i64 = sqlx::query_scalar(
            "INSERT INTO series (kind, title, folder_path, added_at) VALUES ('manga','A','manga/A',1) RETURNING id",
        )
        .fetch_one(&pool).await.unwrap();
        let la = item(&pool, "la", "manga", Some(a)).await;
        sqlx::query(
            "INSERT INTO item_series_leaf (item_id, series_id, number_sort) VALUES (?, ?, 1.0)",
        )
        .bind(la)
        .bind(a)
        .execute(&pool)
        .await
        .unwrap();
        tag(&pool, la, "action").await;
        tag(&pool, la, "demons").await;

        let o1 = item(&pool, "o1", "manga", None).await;
        tag(&pool, o1, "action").await;
        tag(&pool, o1, "demons").await;
        let o2 = item(&pool, "o2", "manga", None).await;
        tag(&pool, o2, "romance").await;

        let entry_corpus = build_entry_corpus(&pool).await.unwrap();
        let a_key = entry_key_series(a);
        let nb = entry_neighbors_of(&pool, &entry_corpus, a_key)
            .await
            .unwrap();
        write_entry_neighbors(&pool, a_key, &nb).await.unwrap();

        sqlx::query(
            "INSERT INTO series_favorites (user_id, series_id, created_at) VALUES (?, ?, 0)",
        )
        .bind(uid)
        .bind(a)
        .execute(&pool)
        .await
        .unwrap();

        let entry_corpus = build_entry_corpus(&pool).await.unwrap();
        let recs = recommend_for_you(&pool, &entry_corpus, uid, 0)
            .await
            .unwrap();
        let keys: Vec<i64> = recs.iter().map(|(k, _)| *k).collect();
        assert!(
            keys.contains(&o1),
            "a favorited series recommends a cross-type one-shot; got {keys:?}"
        );
        assert!(!keys.contains(&o2), "unrelated one-shot not recommended");
        assert!(
            !keys.contains(&a_key),
            "the liked series isn't recommended back to itself"
        );
    }
}
