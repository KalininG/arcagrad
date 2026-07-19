//! Tag persistence, provenance, facets, and catalog counts.

use anyhow::{anyhow, Result};
use serde::Serialize;
use sqlx::{AssertSqlSafe, SqlitePool};
use std::collections::HashMap;
use utoipa::ToSchema;

use super::*;

#[derive(Serialize, Clone, PartialEq, Eq, Debug, ToSchema)]
pub struct ItemTag {
    pub namespace: String,
    pub value: String,
    /// Qualifier facet such as `female`, `male`, `mixed`, or `none`.
    pub qualifier: String,
    /// Sources that asserted this collapsed tag.
    pub sources: Vec<String>,
    /// Global catalog-card count, included only on detail surfaces.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<i64>,
    /// Series detail only: whether at least one leaf also carries the tag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leaf: Option<bool>,
}

pub struct ItemTagRaw {
    pub namespace: String,
    pub value: String,
    pub qualifier: String,
    pub source: String,
}

/// Collapse provenance and let specific qualifiers subsume `none`.
fn collapse_tags(rows: Vec<ItemTagRaw>) -> Vec<ItemTag> {
    use std::collections::{BTreeMap, BTreeSet};
    let mut groups: BTreeMap<(String, String), BTreeMap<String, BTreeSet<String>>> =
        BTreeMap::new();
    for r in rows {
        groups
            .entry((r.namespace, r.value))
            .or_default()
            .entry(r.qualifier)
            .or_default()
            .insert(r.source);
    }

    let mut out = Vec::new();
    for ((namespace, value), by_qualifier) in groups {
        let none_sources = by_qualifier.get("none").cloned().unwrap_or_default();
        let specifics: Vec<(&String, &BTreeSet<String>)> =
            by_qualifier.iter().filter(|(g, _)| *g != "none").collect();

        if specifics.is_empty() {
            out.push(ItemTag {
                namespace,
                value,
                qualifier: "none".to_string(),
                sources: none_sources.into_iter().collect(),
                count: None,
                leaf: None,
            });
        } else {
            for (qualifier, sources) in specifics {
                let mut merged: BTreeSet<String> = sources.clone();
                merged.extend(none_sources.iter().cloned());
                out.push(ItemTag {
                    namespace: namespace.clone(),
                    value: value.clone(),
                    qualifier: qualifier.clone(),
                    sources: merged.into_iter().collect(),
                    count: None,
                    leaf: None,
                });
            }
        }
    }
    out
}

#[derive(Serialize, ToSchema)]
pub struct TagCount {
    pub namespace: String,
    pub value: String,
    /// Distinct catalog cards carrying this tag, with series collapsed.
    pub count: i64,
}

/// Convert user text to quoted FTS5 prefix terms, dropping operators.
pub fn fts_query(raw: &str) -> Option<String> {
    let terms: Vec<String> = raw
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\"*"))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

/// Refresh an item's FTS tag bag and creator sort key.
pub async fn reindex_item_tags(pool: &SqlitePool, item_id: i64) -> Result<()> {
    sqlx::query(
        "UPDATE items_fts SET tags = COALESCE( \
             (SELECT group_concat(t.value, ' ') \
              FROM item_tags it JOIN tags t ON t.id = it.tag_id \
              WHERE it.item_id = ?), '') \
         WHERE rowid = ?",
    )
    .bind(item_id)
    .bind(item_id)
    .execute(pool)
    .await?;
    recompute_item_sort_creator(pool, item_id).await?;
    Ok(())
}

/// Refresh an item's creator sort key and its parent series key.
pub async fn recompute_item_sort_creator(pool: &SqlitePool, item_id: i64) -> Result<()> {
    sqlx::query(
        "UPDATE items SET sort_creator = ( \
             SELECT MIN(t.value) FROM item_tags it JOIN tags t ON t.id = it.tag_id \
             WHERE it.item_id = ? AND t.namespace = 'creator') \
         WHERE id = ?",
    )
    .bind(item_id)
    .bind(item_id)
    .execute(pool)
    .await?;
    sqlx::query(
        "UPDATE series SET sort_creator = ( \
             SELECT MIN(i.sort_creator) FROM items i WHERE i.series_id = series.id) \
         WHERE id = (SELECT series_id FROM items WHERE id = ?)",
    )
    .bind(item_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub(crate) async fn tags_for_item_ids(
    pool: &SqlitePool,
    item_ids: &[i64],
) -> Result<HashMap<i64, Vec<ItemTag>>> {
    if item_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = vec!["?"; item_ids.len()].join(", ");
    let sql = format!(
        "SELECT at.item_id, t.namespace, t.value, at.qualifier, at.source \
         FROM item_tags at JOIN tags t ON t.id = at.tag_id \
         WHERE at.item_id IN ({placeholders}) \
         ORDER BY t.namespace, t.value, at.qualifier"
    );
    let mut q = sqlx::query_as::<_, (i64, String, String, String, String)>(AssertSqlSafe(sql));
    for &id in item_ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(pool).await?;

    let mut raw: HashMap<i64, Vec<ItemTagRaw>> = HashMap::new();
    for (item_id, namespace, value, qualifier, source) in rows {
        raw.entry(item_id).or_default().push(ItemTagRaw {
            namespace,
            value,
            qualifier,
            source,
        });
    }
    Ok(raw
        .into_iter()
        .map(|(id, rows)| (id, collapse_tags(rows)))
        .collect())
}

pub async fn tags_for_item(pool: &SqlitePool, item_id: i64) -> Result<Vec<ItemTag>> {
    Ok(tags_for_item_ids(pool, &[item_id])
        .await?
        .remove(&item_id)
        .unwrap_or_default())
}

/// Batch effective series tags from series-level and leaf assertions.
pub(crate) async fn series_tags_for_ids(
    pool: &SqlitePool,
    series_ids: &[i64],
) -> Result<HashMap<i64, Vec<ItemTag>>> {
    if series_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = vec!["?"; series_ids.len()].join(", ");
    let sql = format!(
        "SELECT x.series_id, t.namespace, t.value, x.qualifier, x.source FROM ( \
             SELECT series_id, tag_id, qualifier, source FROM series_tags \
               WHERE series_id IN ({placeholders}) \
             UNION ALL \
             SELECT l.series_id, at.tag_id, at.qualifier, at.source \
             FROM item_series_leaf l JOIN item_tags at ON at.item_id = l.item_id \
             WHERE l.series_id IN ({placeholders}) \
         ) x JOIN tags t ON t.id = x.tag_id \
         ORDER BY t.namespace, t.value, x.qualifier"
    );
    let mut q = sqlx::query_as::<_, (i64, String, String, String, String)>(AssertSqlSafe(sql));
    // The id list appears once in each UNION branch.
    for _ in 0..2 {
        for &id in series_ids {
            q = q.bind(id);
        }
    }
    let mut raw: HashMap<i64, Vec<ItemTagRaw>> = HashMap::new();
    for (series_id, namespace, value, qualifier, source) in q.fetch_all(pool).await? {
        raw.entry(series_id).or_default().push(ItemTagRaw {
            namespace,
            value,
            qualifier,
            source,
        });
    }
    Ok(raw
        .into_iter()
        .map(|(id, rows)| (id, collapse_tags(rows)))
        .collect())
}

pub async fn tag_id(pool: &SqlitePool, namespace: &str, value: &str) -> Result<Option<i64>> {
    Ok(
        sqlx::query_scalar("SELECT id FROM tags WHERE namespace = ? AND value = ?")
            .bind(norm(namespace))
            .bind(norm(value))
            .fetch_optional(pool)
            .await?,
    )
}

#[derive(Serialize, ToSchema)]
pub struct BlockedTag {
    pub namespace: String,
    pub value: String,
}

pub async fn blocklist_tag_ids(pool: &SqlitePool, user_id: i64) -> Result<Vec<i64>> {
    Ok(
        sqlx::query_scalar("SELECT tag_id FROM user_tag_blocklist WHERE user_id = ?")
            .bind(user_id)
            .fetch_all(pool)
            .await?,
    )
}

pub async fn blocklist_tags(pool: &SqlitePool, user_id: i64) -> Result<Vec<BlockedTag>> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT t.namespace, t.value FROM user_tag_blocklist b \
         JOIN tags t ON t.id = b.tag_id WHERE b.user_id = ? \
         ORDER BY t.namespace, t.value",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(namespace, value)| BlockedTag { namespace, value })
        .collect())
}

/// Replace a user's complete tag blocklist atomically.
pub async fn set_blocklist(pool: &SqlitePool, user_id: i64, tag_ids: &[i64]) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM user_tag_blocklist WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    for &id in tag_ids {
        sqlx::query("INSERT OR IGNORE INTO user_tag_blocklist (user_id, tag_id) VALUES (?, ?)")
            .bind(user_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Normalize and validate a tag before creating it idempotently.
pub async fn get_or_create_tag(pool: &SqlitePool, namespace: &str, value: &str) -> Result<i64> {
    let ns = norm(namespace);
    if !valid_namespace(&ns) {
        return Err(anyhow!("invalid namespace: {ns:?}"));
    }
    let v = norm(value);
    if v.is_empty() {
        return Err(anyhow!("tag value must not be empty"));
    }
    sqlx::query("INSERT OR IGNORE INTO tags (namespace, value) VALUES (?, ?)")
        .bind(&ns)
        .bind(&v)
        .execute(pool)
        .await?;
    Ok(
        sqlx::query_scalar("SELECT id FROM tags WHERE namespace = ? AND value = ?")
            .bind(&ns)
            .bind(&v)
            .fetch_one(pool)
            .await?,
    )
}

pub async fn add_item_tag(
    pool: &SqlitePool,
    item_id: i64,
    tag_id: i64,
    qualifier: &str,
    source: &str,
) -> Result<bool> {
    add_item_tag_with_role(pool, item_id, tag_id, qualifier, "none", source).await
}

/// Attach an item tag with an optional creator-role facet.
pub async fn add_item_tag_with_role(
    pool: &SqlitePool,
    item_id: i64,
    tag_id: i64,
    qualifier: &str,
    role: &str,
    source: &str,
) -> Result<bool> {
    if !item_exists(pool, item_id).await? {
        return Ok(false);
    }
    sqlx::query(
        "INSERT OR IGNORE INTO item_tags (item_id, tag_id, qualifier, role, source) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(item_id)
    .bind(tag_id)
    .bind(qualifier)
    .bind(role)
    .bind(source)
    .execute(pool)
    .await?;
    Ok(true)
}

/// Remove only one source's item-tag assertions before replacement.
pub async fn clear_item_tags_from_source(
    pool: &SqlitePool,
    item_id: i64,
    source: &str,
) -> Result<()> {
    sqlx::query("DELETE FROM item_tags WHERE item_id = ? AND source = ?")
        .bind(item_id)
        .bind(source)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn remove_item_tag(
    pool: &SqlitePool,
    item_id: i64,
    tag_id: i64,
    qualifier: &str,
) -> Result<bool> {
    let res =
        sqlx::query("DELETE FROM item_tags WHERE tag_id = ? AND qualifier = ? AND item_id = ?")
            .bind(tag_id)
            .bind(qualifier)
            .bind(item_id)
            .execute(pool)
            .await?;
    Ok(res.rows_affected() > 0)
}

/// Return the exclusive upper bound for an indexed prefix range.
fn prefix_upper_bound(prefix: &str) -> Option<String> {
    let mut chars: Vec<char> = prefix.chars().collect();
    while let Some(last) = chars.pop() {
        if let Some(next) = char::from_u32(last as u32 + 1) {
            chars.push(next);
            return Some(chars.into_iter().collect());
        }
    }
    None
}

/// Count distinct one-shot or collapsed-series cards rather than raw tag links.
const TAG_CARD_COUNT_SUBQUERY: &str = "( \
    SELECT COUNT(DISTINCT card) FROM ( \
        SELECT CASE WHEN i.series_id IS NULL THEN 'i' || i.id ELSE 's' || i.series_id END AS card \
        FROM item_tags it JOIN items i ON i.id = it.item_id WHERE it.tag_id = t.id \
        UNION ALL \
        SELECT 's' || st.series_id FROM series_tags st WHERE st.tag_id = t.id \
    ) \
)";

fn tag_card_count_subquery_where(item_pred: &str, series_pred: &str) -> String {
    format!(
        "( \
        SELECT COUNT(DISTINCT card) FROM ( \
            SELECT CASE WHEN i.series_id IS NULL THEN 'i' || i.id ELSE 's' || i.series_id END AS card \
            FROM item_tags it JOIN items i ON i.id = it.item_id \
            WHERE it.tag_id = t.id AND {item_pred} \
            UNION ALL \
            SELECT 's' || st.series_id FROM series_tags st JOIN series s ON s.id = st.series_id \
            WHERE st.tag_id = t.id AND {series_pred} \
        ) \
    )"
    )
}

fn tag_card_count_subquery_deny(n: usize) -> String {
    let ph = vec!["?"; n].join(", ");
    tag_card_count_subquery_where(
        &format!("i.kind NOT IN ({ph})"),
        &format!("s.kind NOT IN ({ph})"),
    )
}

/// Excludes orphan tag rows from facets and suggestions.
const TAG_USED_PREDICATE: &str = "EXISTS (SELECT 1 FROM item_tags it WHERE it.tag_id = t.id) \
    OR EXISTS (SELECT 1 FROM series_tags st WHERE st.tag_id = t.id)";

fn tag_used_predicate_where(item_pred: &str, series_pred: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM item_tags it JOIN items i ON i.id = it.item_id \
                 WHERE it.tag_id = t.id AND {item_pred}) \
         OR EXISTS (SELECT 1 FROM series_tags st JOIN series s ON s.id = st.series_id \
                 WHERE st.tag_id = t.id AND {series_pred})"
    )
}

fn tag_used_predicate_deny(n: usize) -> String {
    let ph = vec!["?"; n].join(", ");
    tag_used_predicate_where(
        &format!("i.kind NOT IN ({ph})"),
        &format!("s.kind NOT IN ({ph})"),
    )
}

async fn tag_counts_for_tag_ids(
    pool: &SqlitePool,
    tag_ids_sq: &str,
    binds: &[i64],
) -> Result<HashMap<(String, String), i64>> {
    let sql = format!(
        "SELECT t.namespace, t.value, {TAG_CARD_COUNT_SUBQUERY} AS cnt FROM tags t \
         WHERE t.id IN ({tag_ids_sq})"
    );
    let mut q = sqlx::query_as::<_, (String, String, i64)>(AssertSqlSafe(sql));
    for &b in binds {
        q = q.bind(b);
    }
    let rows = q.fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|(namespace, value, count)| ((namespace, value), count))
        .collect())
}

async fn tag_counts_for_item(
    pool: &SqlitePool,
    item_id: i64,
) -> Result<HashMap<(String, String), i64>> {
    tag_counts_for_tag_ids(
        pool,
        "SELECT tag_id FROM item_tags WHERE item_id = ?",
        &[item_id],
    )
    .await
}

pub async fn tags_for_item_with_counts(pool: &SqlitePool, item_id: i64) -> Result<Vec<ItemTag>> {
    let mut tags = tags_for_item(pool, item_id).await?;
    if tags.is_empty() {
        return Ok(tags);
    }
    let counts = tag_counts_for_item(pool, item_id).await?;
    for t in &mut tags {
        t.count = counts.get(&(t.namespace.clone(), t.value.clone())).copied();
    }
    Ok(tags)
}

fn prefix_match_sql(has_upper: bool) -> &'static str {
    if has_upper {
        " AND ((t.value >= ? AND t.value < ?) OR t.value LIKE ? ESCAPE '\\')"
    } else {
        " AND (t.value >= ? OR t.value LIKE ? ESCAPE '\\')"
    }
}

fn like_word_pattern(prefix: &str) -> String {
    let escaped = prefix
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("% {escaped}%")
}

/// List visible, in-use tags with distinct catalog-card counts.
pub async fn tags_with_counts(
    pool: &SqlitePool,
    prefix: Option<&str>,
    limit: Option<i64>,
    deny_kinds: &[String],
) -> Result<Vec<TagCount>> {
    let n = deny_kinds.len();
    let (count_sq, used_pred) = if n == 0 {
        (
            TAG_CARD_COUNT_SUBQUERY.to_string(),
            TAG_USED_PREDICATE.to_string(),
        )
    } else {
        (tag_card_count_subquery_deny(n), tag_used_predicate_deny(n))
    };
    let mut sql =
        format!("SELECT t.namespace, t.value, {count_sq} AS cnt FROM tags t WHERE ({used_pred})");
    let prefix = prefix.filter(|p| !p.is_empty());
    let upper = prefix.and_then(prefix_upper_bound);
    if prefix.is_some() {
        sql.push_str(prefix_match_sql(upper.is_some()));
    }
    sql.push_str(" ORDER BY cnt DESC, t.namespace, t.value");
    if limit.is_some() {
        sql.push_str(" LIMIT ?");
    }

    let mut q = sqlx::query_as::<_, (String, String, i64)>(AssertSqlSafe(sql));
    // Count and usage predicates each contain item and series branches.
    for _ in 0..4 {
        for k in deny_kinds {
            q = q.bind(k.clone());
        }
    }
    if let Some(p) = prefix {
        q = q.bind(p.to_string());
        if let Some(u) = upper {
            q = q.bind(u);
        }
        q = q.bind(like_word_pattern(p));
    }
    if let Some(l) = limit {
        q = q.bind(l);
    }
    let rows = q.fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|(namespace, value, count)| TagCount {
            namespace,
            value,
            count,
        })
        .collect())
}

/// List in-use tags and card counts within one kind.
pub async fn tags_with_counts_for_kind(
    pool: &SqlitePool,
    kind: &str,
    prefix: Option<&str>,
    limit: Option<i64>,
) -> Result<Vec<TagCount>> {
    let count_sq = tag_card_count_subquery_where("i.kind = ?", "s.kind = ?");
    let used_pred = tag_used_predicate_where("i.kind = ?", "s.kind = ?");
    let mut sql =
        format!("SELECT t.namespace, t.value, {count_sq} AS cnt FROM tags t WHERE ({used_pred})");
    let prefix = prefix.filter(|p| !p.is_empty());
    let upper = prefix.and_then(prefix_upper_bound);
    if prefix.is_some() {
        sql.push_str(prefix_match_sql(upper.is_some()));
    }
    sql.push_str(" ORDER BY cnt DESC, t.namespace, t.value");
    if limit.is_some() {
        sql.push_str(" LIMIT ?");
    }
    let mut q = sqlx::query_as::<_, (String, String, i64)>(AssertSqlSafe(sql));
    // Count and usage predicates each contain item and series branches.
    for _ in 0..4 {
        q = q.bind(kind.to_string());
    }
    if let Some(p) = prefix {
        q = q.bind(p.to_string());
        if let Some(u) = upper {
            q = q.bind(u);
        }
        q = q.bind(like_word_pattern(p));
    }
    if let Some(l) = limit {
        q = q.bind(l);
    }
    let rows = q.fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|(namespace, value, count)| TagCount {
            namespace,
            value,
            count,
        })
        .collect())
}

/// Aggregate tags over the viewer's liked catalog cards within a kind.
pub async fn favorite_tags_for_kind(
    pool: &SqlitePool,
    user_id: i64,
    kind: &str,
) -> Result<Vec<TagCount>> {
    let sql = "\
        SELECT t.namespace, t.value, COUNT(DISTINCT lc.card) AS cnt FROM ( \
            SELECT CASE WHEN i.series_id IS NULL THEN 'i' || i.id ELSE 's' || i.series_id END AS card, it.tag_id \
            FROM item_tags it JOIN items i ON i.id = it.item_id \
            WHERE i.kind = ? AND ( \
                i.id IN (SELECT item_id FROM favorites WHERE user_id = ?) \
                OR i.id IN ( \
                    SELECT rp.item_id FROM read_progress rp JOIN items ii ON ii.id = rp.item_id \
                    WHERE rp.user_id = ? AND ( \
                        (rp.unit = 'page' AND ii.page_count > 0 AND rp.value >= ii.page_count - 1) \
                        OR (rp.unit = 'percent' AND rp.value >= 0.98) \
                    ) \
                ) \
                OR i.series_id IN (SELECT series_id FROM series_favorites WHERE user_id = ?) \
            ) \
            UNION ALL \
            SELECT 's' || st.series_id AS card, st.tag_id \
            FROM series_tags st JOIN series s ON s.id = st.series_id \
            WHERE s.kind = ? AND st.series_id IN (SELECT series_id FROM series_favorites WHERE user_id = ?) \
        ) lc JOIN tags t ON t.id = lc.tag_id \
        GROUP BY t.id \
        ORDER BY cnt DESC, t.namespace, t.value";
    let rows = sqlx::query_as::<_, (String, String, i64)>(sql)
        .bind(kind)
        .bind(user_id)
        .bind(user_id)
        .bind(user_id)
        .bind(kind)
        .bind(user_id)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(namespace, value, count)| TagCount {
            namespace,
            value,
            count,
        })
        .collect())
}

pub(crate) async fn tag_counts_for_series(
    pool: &SqlitePool,
    series_id: i64,
) -> Result<HashMap<(String, String), i64>> {
    tag_counts_for_tag_ids(
        pool,
        "SELECT tag_id FROM series_tags WHERE series_id = ? \
         UNION \
         SELECT at.tag_id FROM item_series_leaf l JOIN item_tags at ON at.item_id = l.item_id \
           WHERE l.series_id = ?",
        &[series_id, series_id],
    )
    .await
}

pub async fn add_series_tag(
    pool: &SqlitePool,
    series_id: i64,
    tag_id: i64,
    qualifier: &str,
    source: &str,
) -> Result<bool> {
    add_series_tag_with_role(pool, series_id, tag_id, qualifier, "none", source).await
}

/// Attach a series tag with an optional creator-role facet.
pub async fn add_series_tag_with_role(
    pool: &SqlitePool,
    series_id: i64,
    tag_id: i64,
    qualifier: &str,
    role: &str,
    source: &str,
) -> Result<bool> {
    if !series_exists(pool, series_id).await? {
        return Ok(false);
    }
    sqlx::query(
        "INSERT OR IGNORE INTO series_tags (series_id, tag_id, qualifier, role, source) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(series_id)
    .bind(tag_id)
    .bind(qualifier)
    .bind(role)
    .bind(source)
    .execute(pool)
    .await?;
    Ok(true)
}

/// Remove only one source's series-tag assertions before replacement.
pub async fn clear_series_tags_from_source(
    pool: &SqlitePool,
    series_id: i64,
    source: &str,
) -> Result<()> {
    sqlx::query("DELETE FROM series_tags WHERE series_id = ? AND source = ?")
        .bind(series_id)
        .bind(source)
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
    async fn tags_with_counts_for_kind_scopes_and_prefixes(pool: SqlitePool) {
        let book = insert_item_kind(&pool, "b1", "books").await;
        let dojin = insert_item_kind(&pool, "d1", "comics").await;
        let anatomy = get_or_create_tag(&pool, "tag", "anatomy").await.unwrap();
        let animated = get_or_create_tag(&pool, "tag", "animated").await.unwrap();
        add_item_tag(&pool, book, anatomy, "none", "manual")
            .await
            .unwrap();
        add_item_tag(&pool, dojin, animated, "none", "manual")
            .await
            .unwrap();

        let books = tags_with_counts_for_kind(&pool, "books", Some("an"), Some(10))
            .await
            .unwrap();
        assert_eq!(books.len(), 1);
        assert_eq!(books[0].value, "anatomy");
        assert_eq!(books[0].count, 1);

        let dj = tags_with_counts_for_kind(&pool, "comics", Some("an"), Some(10))
            .await
            .unwrap();
        assert_eq!(
            dj.iter().map(|t| t.value.as_str()).collect::<Vec<_>>(),
            ["animated"]
        );

        assert!(
            tags_with_counts_for_kind(&pool, "books", Some("zzz"), Some(10))
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[sqlx::test]
    async fn tag_prefix_matches_later_words_too(pool: SqlitePool) {
        let item = insert_item_kind(&pool, "m1", "books").await;
        let melville = get_or_create_tag(&pool, "creator", "herman melville")
            .await
            .unwrap();
        add_item_tag(&pool, item, melville, "none", "manual")
            .await
            .unwrap();

        for prefix in ["her", "melv"] {
            let hits = tags_with_counts(&pool, Some(prefix), Some(10), &[])
                .await
                .unwrap();
            assert_eq!(
                hits.iter().map(|t| t.value.as_str()).collect::<Vec<_>>(),
                ["herman melville"],
                "prefix {prefix:?} must match"
            );
            let scoped = tags_with_counts_for_kind(&pool, "books", Some(prefix), Some(10))
                .await
                .unwrap();
            assert_eq!(scoped.len(), 1, "kind-scoped prefix {prefix:?} must match");
        }

        assert!(tags_with_counts(&pool, Some("erman"), Some(10), &[])
            .await
            .unwrap()
            .is_empty());
        assert!(tags_with_counts(&pool, Some("%elv"), Some(10), &[])
            .await
            .unwrap()
            .is_empty());
    }

    #[sqlx::test]
    async fn get_or_create_tag_idempotent_and_normalized(pool: SqlitePool) {
        let a = get_or_create_tag(&pool, "Creator", "Foo Bar")
            .await
            .unwrap();
        let b = get_or_create_tag(&pool, "creator", "  foo bar ")
            .await
            .unwrap();
        assert_eq!(a, b, "case/whitespace variants collapse to one tag");
        assert!(
            get_or_create_tag(&pool, "demographic", "seinen")
                .await
                .is_ok(),
            "a closed-set namespace is accepted"
        );
        assert!(
            get_or_create_tag(&pool, "genre", "drama").await.is_err(),
            "an out-of-set namespace is rejected"
        );
        assert!(
            get_or_create_tag(&pool, "cosplayer", "x").await.is_err(),
            "cosplayer folded into creator — no longer a namespace"
        );
        assert!(
            get_or_create_tag(&pool, "tag", "   ").await.is_err(),
            "empty value rejected"
        );
    }

    #[sqlx::test]
    async fn tags_count_collapses_series(pool: SqlitePool) {
        let one = insert_item(&pool, "one", 1).await;
        let ta = get_or_create_tag(&pool, "tag", "a").await.unwrap();
        add_item_tag(&pool, one, ta, "none", "manual")
            .await
            .unwrap();

        let sid: i64 = sqlx::query_scalar(
            "INSERT INTO series (kind, title, folder_path, added_at) VALUES ('manga','S','manga/S',1) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let leaf = |tag: &'static str| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, i64>(
                    "INSERT INTO items (scheme_tag, structural_hash, path, size_bytes, mtime, format, title, page_count, added_at, last_modified_at, series_id) \
                     VALUES ('zip-structural-v1', ?, ?, 1, 1, 'cbz', ?, 5, 1, 0, ?) RETURNING id",
                )
                .bind(tag).bind(format!("/p/{tag}")).bind(tag).bind(sid)
                .fetch_one(&pool).await.unwrap()
            }
        };
        let l1 = leaf("l1").await;
        let l2 = leaf("l2").await;
        let tg = get_or_create_tag(&pool, "demographic", "seinen")
            .await
            .unwrap();
        add_series_tag(&pool, sid, tg, "none", "anilist")
            .await
            .unwrap();
        let tb = get_or_create_tag(&pool, "tag", "b").await.unwrap();
        add_item_tag(&pool, l1, tb, "none", "manual").await.unwrap();
        add_item_tag(&pool, l2, tb, "none", "manual").await.unwrap();

        let counts = tags_with_counts(&pool, None, None, &[]).await.unwrap();
        let count = |ns: &str, v: &str| {
            counts
                .iter()
                .find(|t| t.namespace == ns && t.value == v)
                .map(|t| t.count)
        };
        assert_eq!(count("tag", "a"), Some(1), "one-shot");
        assert_eq!(
            count("demographic", "seinen"),
            Some(1),
            "series-level tag counts the series once (was 0)"
        );
        assert_eq!(
            count("tag", "b"),
            Some(1),
            "a tag on 2 leaves = 1 series card, not 2"
        );
    }

    #[sqlx::test]
    async fn tags_for_item_with_counts_annotates_popularity(pool: SqlitePool) {
        let a1 = insert_item(&pool, "a1", 1).await;
        let a2 = insert_item(&pool, "a2", 2).await;
        let shared = get_or_create_tag(&pool, "tag", "shared").await.unwrap();
        let solo = get_or_create_tag(&pool, "creator", "solo").await.unwrap();
        add_item_tag(&pool, a1, shared, "none", "manual")
            .await
            .unwrap();
        add_item_tag(&pool, a2, shared, "none", "manual")
            .await
            .unwrap();
        add_item_tag(&pool, a1, solo, "none", "manual")
            .await
            .unwrap();

        let tags = tags_for_item_with_counts(&pool, a1).await.unwrap();
        let count = |ns: &str, v: &str| {
            tags.iter()
                .find(|t| t.namespace == ns && t.value == v)
                .and_then(|t| t.count)
        };
        assert_eq!(count("tag", "shared"), Some(2), "on both items");
        assert_eq!(count("creator", "solo"), Some(1), "on one item");

        let plain = tags_for_item(&pool, a1).await.unwrap();
        assert!(
            plain.iter().all(|t| t.count.is_none()),
            "listing is count-free"
        );
    }

    #[sqlx::test]
    async fn orphan_tags_are_hidden_from_counts(pool: SqlitePool) {
        let item = insert_item(&pool, "a", 1).await;
        let tag = get_or_create_tag(&pool, "language", "chinese")
            .await
            .unwrap();
        add_item_tag(&pool, item, tag, "none", "manual")
            .await
            .unwrap();

        let has = |rows: &[TagCount]| rows.iter().any(|t| t.value == "chinese");
        assert!(
            has(&tags_with_counts(&pool, None, None, &[]).await.unwrap()),
            "shown while linked"
        );
        assert!(
            has(&tags_with_counts(&pool, Some("chi"), None, &[])
                .await
                .unwrap()),
            "typeahead too"
        );

        sqlx::query("DELETE FROM item_tags WHERE item_id = ? AND tag_id = ?")
            .bind(item)
            .bind(tag)
            .execute(&pool)
            .await
            .unwrap();

        assert!(
            !has(&tags_with_counts(&pool, None, None, &[]).await.unwrap()),
            "hidden once orphaned"
        );
        assert!(
            !has(&tags_with_counts(&pool, Some("chi"), None, &[])
                .await
                .unwrap()),
            "typeahead hidden"
        );
    }

    #[test]
    fn fts_query_tokenizes_and_sanitizes() {
        assert_eq!(fts_query("foo bar").as_deref(), Some(r#""foo"* "bar"*"#));
        assert_eq!(
            fts_query("creator:foo book-2").as_deref(),
            Some(r#""creator"* "foo"* "book"* "2"*"#)
        );
        assert_eq!(fts_query("\"*^(").as_deref(), None);
        assert_eq!(fts_query("a\"b").as_deref(), Some(r#""a"* "b"*"#));
        assert_eq!(fts_query(""), None);
        assert_eq!(fts_query("   "), None);
    }

    #[test]
    fn prefix_upper_bound_brackets_the_prefix() {
        assert_eq!(prefix_upper_bound("wad").as_deref(), Some("wae"));
        assert_eq!(prefix_upper_bound("a").as_deref(), Some("b"));
        assert_eq!(prefix_upper_bound("az").as_deref(), Some("a{"));
        assert!("azz" < prefix_upper_bound("az").unwrap().as_str());
        assert_eq!(prefix_upper_bound(""), None);
    }

    #[sqlx::test]
    async fn qualifier_facet_and_counts(pool: SqlitePool) {
        let aid = insert_item(&pool, "g1", 1).await;
        let ntr = get_or_create_tag(&pool, "tag", "mystery").await.unwrap();

        assert!(add_item_tag(&pool, aid, ntr, "female", "anilist")
            .await
            .unwrap());
        assert!(add_item_tag(&pool, aid, ntr, "male", "anilist")
            .await
            .unwrap());
        assert!(add_item_tag(&pool, aid, ntr, "female", "anilist")
            .await
            .unwrap());
        assert_eq!(tags_for_item(&pool, aid).await.unwrap().len(), 2);

        let counts = tags_with_counts(&pool, None, None, &[]).await.unwrap();
        assert_eq!(
            counts.iter().find(|c| c.value == "mystery").unwrap().count,
            1
        );

        assert!(remove_item_tag(&pool, aid, ntr, "male").await.unwrap());
        let tags = tags_for_item(&pool, aid).await.unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].qualifier, "female");

        assert!(add_item_tag(&pool, aid, ntr, "other", "x").await.unwrap());
        assert!(!add_item_tag(&pool, 999_999, ntr, "none", "manual")
            .await
            .unwrap());
    }

    #[test]
    fn collapse_tags_subsumes_none_under_specific_qualifier() {
        let out = collapse_tags(vec![
            raw("tag", "long hair", "female", "anilist"),
            raw("tag", "long hair", "none", "openlibrary"),
        ]);
        assert_eq!(out.len(), 1, "the `none` row is subsumed, not a 2nd tag");
        assert_eq!(out[0].qualifier, "female");
        assert_eq!(out[0].value, "long hair");
        assert_eq!(out[0].sources, vec!["anilist", "openlibrary"]);

        let mut out = collapse_tags(vec![
            raw("tag", "short hair", "female", "anilist"),
            raw("tag", "short hair", "male", "anilist"),
            raw("tag", "short hair", "none", "openlibrary"),
        ]);
        out.sort_by(|a, b| a.qualifier.cmp(&b.qualifier));
        assert_eq!(out.len(), 2);
        assert_eq!(
            (out[0].qualifier.as_str(), out[1].qualifier.as_str()),
            ("female", "male")
        );
        assert!(out
            .iter()
            .all(|t| t.sources == vec!["anilist", "openlibrary"]));

        let out = collapse_tags(vec![
            raw("creator", "foo", "none", "openlibrary"),
            raw("creator", "foo", "none", "anilist"),
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].qualifier, "none");
        assert_eq!(out[0].sources, vec!["anilist", "openlibrary"]);
    }
}
