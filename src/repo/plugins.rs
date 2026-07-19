//! Plugin installation, enablement, credentials, and source-owned item data.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use utoipa::ToSchema;

use super::*;

pub async fn plugins_for_kind(pool: &SqlitePool, kind: &str) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT plugin_id FROM plugin_kind_map WHERE kind = ? ORDER BY plugin_id",
    )
    .bind(kind)
    .fetch_all(pool)
    .await?)
}

pub async fn auto_plugins_for_kind(pool: &SqlitePool, kind: &str) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT plugin_id FROM plugin_kind_map WHERE kind = ? AND auto = 1 ORDER BY plugin_id",
    )
    .bind(kind)
    .fetch_all(pool)
    .await?)
}

pub async fn set_plugins_for_kind(
    pool: &SqlitePool,
    kind: &str,
    enabled: &[String],
    auto: &[String],
) -> Result<()> {
    let auto: std::collections::HashSet<&str> = auto.iter().map(String::as_str).collect();
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM plugin_kind_map WHERE kind = ?")
        .bind(kind)
        .execute(&mut *tx)
        .await?;
    for id in enabled {
        sqlx::query("INSERT INTO plugin_kind_map (kind, plugin_id, auto) VALUES (?, ?, ?)")
            .bind(kind)
            .bind(id)
            .bind(auto.contains(id.as_str()) as i64)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PluginInstall {
    pub plugin_id: String,
    pub version: String,
    pub artifact_hash: String,
    /// `bundled`, `community`, or `local`.
    pub origin: String,
    pub installed_at: i64,
    pub updated_at: i64,
    pub last_error: Option<String>,
    /// Repository provenance for community installs.
    pub repo_url: Option<String>,
}

pub async fn list_plugin_installs(pool: &SqlitePool) -> Result<Vec<PluginInstall>> {
    Ok(sqlx::query_as(
        "SELECT plugin_id, version, artifact_hash, origin, installed_at, updated_at, last_error,
                repo_url
         FROM plugin_installs ORDER BY plugin_id",
    )
    .fetch_all(pool)
    .await?)
}

/// Install or refresh a plugin and clear its previous load error.
pub async fn upsert_plugin_install(
    pool: &SqlitePool,
    plugin_id: &str,
    version: &str,
    artifact_hash: &str,
    origin: &str,
    repo_url: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO plugin_installs (plugin_id, version, artifact_hash, origin, repo_url)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(plugin_id) DO UPDATE SET
           version = excluded.version,
           artifact_hash = excluded.artifact_hash,
           origin = excluded.origin,
           repo_url = excluded.repo_url,
           updated_at = unixepoch(),
           last_error = NULL",
    )
    .bind(plugin_id)
    .bind(version)
    .bind(artifact_hash)
    .bind(origin)
    .bind(repo_url)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_plugin_install(pool: &SqlitePool, plugin_id: &str) -> Result<bool> {
    let res = sqlx::query("DELETE FROM plugin_installs WHERE plugin_id = ?")
        .bind(plugin_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn set_plugin_install_error(
    pool: &SqlitePool,
    plugin_id: &str,
    error: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "UPDATE plugin_installs SET last_error = ?, updated_at = unixepoch() WHERE plugin_id = ?",
    )
    .bind(error)
    .bind(plugin_id)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PluginRepo {
    pub url: String,
    pub name: Option<String>,
    pub added_at: i64,
    pub last_fetched: Option<i64>,
    pub last_error: Option<String>,
}

pub async fn list_plugin_repos(pool: &SqlitePool) -> Result<Vec<PluginRepo>> {
    Ok(sqlx::query_as(
        "SELECT url, name, added_at, last_fetched, last_error FROM plugin_repos ORDER BY added_at",
    )
    .fetch_all(pool)
    .await?)
}

pub async fn upsert_plugin_repo(pool: &SqlitePool, url: &str, name: Option<&str>) -> Result<()> {
    sqlx::query(
        "INSERT INTO plugin_repos (url, name) VALUES (?, ?)
         ON CONFLICT(url) DO UPDATE SET name = excluded.name",
    )
    .bind(url)
    .bind(name)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_plugin_repo(pool: &SqlitePool, url: &str) -> Result<bool> {
    let res = sqlx::query("DELETE FROM plugin_repos WHERE url = ?")
        .bind(url)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

/// Record a fetch without erasing the last successful time on error.
pub async fn set_plugin_repo_fetch(
    pool: &SqlitePool,
    url: &str,
    name: Option<&str>,
    error: Option<&str>,
) -> Result<()> {
    if error.is_none() {
        sqlx::query(
            "UPDATE plugin_repos SET last_fetched = unixepoch(), last_error = NULL,
             name = COALESCE(?, name) WHERE url = ?",
        )
        .bind(name)
        .bind(url)
        .execute(pool)
        .await?;
    } else {
        sqlx::query("UPDATE plugin_repos SET last_error = ? WHERE url = ?")
            .bind(error)
            .bind(url)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Persist a proxied cover dHash across server restarts.
pub async fn upsert_cover_hash(pool: &SqlitePool, url: &str, hash: u64) -> Result<()> {
    sqlx::query(
        "INSERT INTO cover_hashes (url, hash) VALUES (?, ?)
         ON CONFLICT(url) DO UPDATE SET hash = excluded.hash, created_at = unixepoch()",
    )
    .bind(url)
    .bind(hash as i64)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn cover_hashes_for(
    pool: &SqlitePool,
    urls: &[String],
) -> Result<std::collections::HashMap<String, u64>> {
    if urls.is_empty() {
        return Ok(Default::default());
    }
    let mut q = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT url, hash FROM cover_hashes WHERE url IN (",
    );
    {
        let mut sep = q.separated(",");
        for u in urls {
            sep.push_bind(u);
        }
    }
    q.push(")");
    let rows: Vec<(String, i64)> = q.build_query_as().fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(u, h)| (u, h as u64)).collect())
}

pub async fn prune_cover_hashes(pool: &SqlitePool) -> Result<u64> {
    let res = sqlx::query("DELETE FROM cover_hashes WHERE created_at < unixepoch() - 2592000")
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

pub async fn get_credential(pool: &SqlitePool, source: &str) -> Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as("SELECT data FROM credentials WHERE source = ?")
        .bind(source)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.0))
}

pub async fn set_credential(pool: &SqlitePool, source: &str, data: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO credentials (source, data, updated_at) VALUES (?, ?, unixepoch()) \
         ON CONFLICT(source) DO UPDATE SET data = excluded.data, updated_at = excluded.updated_at",
    )
    .bind(source)
    .bind(data)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_credential(pool: &SqlitePool, source: &str) -> Result<bool> {
    let res = sqlx::query("DELETE FROM credentials WHERE source = ?")
        .bind(source)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn list_credentials(pool: &SqlitePool) -> Result<Vec<(String, String)>> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT source, data FROM credentials ORDER BY source")
            .fetch_all(pool)
            .await?;
    Ok(rows)
}

#[derive(Serialize, Deserialize, ToSchema, Clone)]
pub struct ItemSource {
    pub source: String,
    pub url: String,
}

/// Upsert a canonical source URL, returning false for an unknown item.
pub async fn set_item_source(
    pool: &SqlitePool,
    item_id: i64,
    source: &str,
    url: &str,
) -> Result<bool> {
    if !item_exists(pool, item_id).await? {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO item_sources (item_id, source, url) VALUES (?, ?, ?) \
         ON CONFLICT(item_id, source) DO UPDATE SET url = excluded.url",
    )
    .bind(item_id)
    .bind(source)
    .bind(url)
    .execute(pool)
    .await?;
    Ok(true)
}

/// Remove one source URL. The caller handles that source's other metadata.
pub async fn delete_item_source(pool: &SqlitePool, item_id: i64, source: &str) -> Result<bool> {
    let res = sqlx::query("DELETE FROM item_sources WHERE item_id = ? AND source = ?")
        .bind(item_id)
        .bind(source)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn item_sources(pool: &SqlitePool, item_id: i64) -> Result<Vec<ItemSource>> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT source, url FROM item_sources WHERE item_id = ? ORDER BY source")
            .bind(item_id)
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(source, url)| ItemSource { source, url })
        .collect())
}

/// Read-only source comment. `body` contains raw markup for client sanitization.
#[derive(Serialize, Deserialize, ToSchema, Debug, Clone, PartialEq)]
pub struct ItemComment {
    pub source: String,
    pub external_id: String,
    pub author: String,
    /// Unix timestamp supplied by the source.
    pub posted_at: Option<i64>,
    /// Signed source vote score.
    pub score: Option<i64>,
    /// Raw source markup; clients must sanitize before rendering.
    pub body: String,
}

/// Replace one source's comment mirror without affecting other sources.
pub async fn replace_item_comments(
    pool: &SqlitePool,
    item_id: i64,
    source: &str,
    comments: &[ItemComment],
) -> Result<bool> {
    if !item_exists(pool, item_id).await? {
        return Ok(false);
    }
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM item_comments WHERE item_id = ? AND source = ?")
        .bind(item_id)
        .bind(source)
        .execute(&mut *tx)
        .await?;
    for c in comments {
        sqlx::query(
            "INSERT INTO item_comments \
             (item_id, source, external_id, author, posted_at, score, body) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(item_id)
        .bind(source)
        .bind(&c.external_id)
        .bind(&c.author)
        .bind(c.posted_at)
        .bind(c.score)
        .bind(&c.body)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(true)
}

type CommentRow = (String, String, String, Option<i64>, Option<i64>, String);

pub async fn item_comments(pool: &SqlitePool, item_id: i64) -> Result<Vec<ItemComment>> {
    let rows: Vec<CommentRow> = sqlx::query_as(
        "SELECT source, external_id, author, posted_at, score, body \
         FROM item_comments \
         WHERE item_id = ? \
         ORDER BY posted_at, source, external_id",
    )
    .bind(item_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(source, external_id, author, posted_at, score, body)| ItemComment {
                source,
                external_id,
                author,
                posted_at,
                score,
                body,
            },
        )
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::test_util::*;

    #[sqlx::test]
    async fn cover_hashes_persist_and_prune(pool: SqlitePool) {
        upsert_cover_hash(&pool, "https://t1.example/cover1.jpg", 0xDEAD_BEEF_u64)
            .await
            .unwrap();
        upsert_cover_hash(&pool, "https://t1.example/cover1.jpg", 0xFEED_FACE_u64)
            .await
            .unwrap();
        let got = cover_hashes_for(
            &pool,
            &[
                "https://t1.example/cover1.jpg".to_string(),
                "https://t1.example/unknown.jpg".to_string(),
            ],
        )
        .await
        .unwrap();
        assert_eq!(
            got.get("https://t1.example/cover1.jpg"),
            Some(&0xFEED_FACE_u64)
        );
        assert_eq!(got.len(), 1, "unknown urls are simply absent");
        assert!(cover_hashes_for(&pool, &[]).await.unwrap().is_empty());

        sqlx::query("UPDATE cover_hashes SET created_at = unixepoch() - 3000000")
            .execute(&pool)
            .await
            .unwrap();
        upsert_cover_hash(&pool, "https://t1.example/fresh.jpg", 1)
            .await
            .unwrap();
        assert_eq!(prune_cover_hashes(&pool).await.unwrap(), 1);
        let left = cover_hashes_for(
            &pool,
            &[
                "https://t1.example/cover1.jpg".to_string(),
                "https://t1.example/fresh.jpg".to_string(),
            ],
        )
        .await
        .unwrap();
        assert_eq!(left.len(), 1);
        assert!(left.contains_key("https://t1.example/fresh.jpg"));
    }

    #[sqlx::test]
    async fn item_comments_replace_and_read(pool: SqlitePool) {
        let good = insert_item(&pool, "good", 1).await;

        repo_replace(
            &pool,
            good,
            "anilist",
            &[comment("anilist", "c1", 54), comment("anilist", "c2", 3)],
        )
        .await;
        let got = item_comments(&pool, good).await.unwrap();
        assert_eq!(got.len(), 2, "both comments stored");
        assert_eq!(
            got[0],
            comment("anilist", "c1", 54),
            "body markup preserved"
        );

        repo_replace(
            &pool,
            good,
            "openlibrary",
            &[comment("openlibrary", "n1", 1)],
        )
        .await;
        assert_eq!(item_comments(&pool, good).await.unwrap().len(), 3);

        repo_replace(&pool, good, "anilist", &[comment("anilist", "c1", 99)]).await;
        let after = item_comments(&pool, good).await.unwrap();
        assert_eq!(
            after.len(),
            2,
            "anilist re-scrape replaced its 2 with 1; openlibrary's kept"
        );
        let eh: Vec<_> = after.iter().filter(|c| c.source == "anilist").collect();
        assert_eq!(eh.len(), 1);
        assert_eq!(eh[0].score, Some(99), "the replaced comment's new score");

        assert!(!replace_item_comments(&pool, 999_999, "anilist", &[])
            .await
            .unwrap());
    }

    #[sqlx::test]
    async fn credentials_crud(pool: SqlitePool) {
        assert!(get_credential(&pool, "openlibrary")
            .await
            .unwrap()
            .is_none());

        set_credential(&pool, "openlibrary", r#"{"api_key":"k1"}"#)
            .await
            .unwrap();
        assert_eq!(
            get_credential(&pool, "openlibrary")
                .await
                .unwrap()
                .as_deref(),
            Some(r#"{"api_key":"k1"}"#)
        );

        set_credential(&pool, "openlibrary", r#"{"api_key":"k2"}"#)
            .await
            .unwrap();
        assert_eq!(
            get_credential(&pool, "openlibrary")
                .await
                .unwrap()
                .as_deref(),
            Some(r#"{"api_key":"k2"}"#)
        );

        let list = list_credentials(&pool).await.unwrap();
        assert_eq!(
            list,
            vec![("openlibrary".to_string(), r#"{"api_key":"k2"}"#.to_string())]
        );

        assert!(delete_credential(&pool, "openlibrary").await.unwrap());
        assert!(!delete_credential(&pool, "openlibrary").await.unwrap());
        assert!(get_credential(&pool, "openlibrary")
            .await
            .unwrap()
            .is_none());
    }

    #[sqlx::test]
    async fn plugin_kind_map_is_per_kind_and_replaces(pool: SqlitePool) {
        assert!(plugins_for_kind(&pool, "manga").await.unwrap().is_empty());

        set_plugins_for_kind(&pool, "manga", &["catalog".into(), "example".into()], &[])
            .await
            .unwrap();
        let mut got = plugins_for_kind(&pool, "manga").await.unwrap();
        got.sort();
        assert_eq!(got, vec!["catalog".to_string(), "example".to_string()]);
        assert!(plugins_for_kind(&pool, "comics").await.unwrap().is_empty());

        set_plugins_for_kind(&pool, "manga", &["catalog".into()], &[])
            .await
            .unwrap();
        assert_eq!(
            plugins_for_kind(&pool, "manga").await.unwrap(),
            vec!["catalog".to_string()]
        );
        set_plugins_for_kind(&pool, "manga", &[], &[])
            .await
            .unwrap();
        assert!(plugins_for_kind(&pool, "manga").await.unwrap().is_empty());
    }

    #[sqlx::test]
    async fn auto_plugins_are_subset_of_enabled(pool: SqlitePool) {
        set_plugins_for_kind(
            &pool,
            "comics",
            &["anilist".into(), "openlibrary".into()],
            &["anilist".into()],
        )
        .await
        .unwrap();

        let mut enabled = plugins_for_kind(&pool, "comics").await.unwrap();
        enabled.sort();
        assert_eq!(
            enabled,
            vec!["anilist".to_string(), "openlibrary".to_string()]
        );

        assert_eq!(
            auto_plugins_for_kind(&pool, "comics").await.unwrap(),
            vec!["anilist".to_string()],
            "only the flagged plugin is auto"
        );

        set_plugins_for_kind(
            &pool,
            "comics",
            &["anilist".into(), "openlibrary".into()],
            &[],
        )
        .await
        .unwrap();
        assert!(auto_plugins_for_kind(&pool, "comics")
            .await
            .unwrap()
            .is_empty());

        set_plugins_for_kind(
            &pool,
            "comics",
            &["anilist".into()],
            &["openlibrary".into()],
        )
        .await
        .unwrap();
        assert!(auto_plugins_for_kind(&pool, "comics")
            .await
            .unwrap()
            .is_empty());
    }
}
