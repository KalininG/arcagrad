//! Follow subscriptions and discovery state.

use anyhow::Result;
use sqlx::{AssertSqlSafe, SqlitePool};

/// A saved source query and its number of new discoveries.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Follow {
    pub id: i64,
    pub plugin_id: String,
    pub kind: String,
    pub feed: String,
    pub query: String,
    pub created_at: i64,
    pub last_checked_at: Option<i64>,
    pub last_error: Option<String>,
    pub new_count: i64,
}

pub async fn create_follow(
    pool: &SqlitePool,
    plugin_id: &str,
    kind: &str,
    feed: &str,
    query: &str,
) -> Result<(i64, bool)> {
    let res = sqlx::query(
        "INSERT OR IGNORE INTO follows (plugin_id, kind, feed, query, created_at)
         VALUES (?, ?, ?, ?, unixepoch())",
    )
    .bind(plugin_id)
    .bind(kind)
    .bind(feed)
    .bind(query)
    .execute(pool)
    .await?;
    if res.rows_affected() > 0 {
        return Ok((res.last_insert_rowid(), true));
    }
    let id: i64 = sqlx::query_scalar(
        "SELECT id FROM follows WHERE plugin_id = ? AND kind = ? AND feed = ? AND query = ?",
    )
    .bind(plugin_id)
    .bind(kind)
    .bind(feed)
    .bind(query)
    .fetch_one(pool)
    .await?;
    Ok((id, false))
}

pub async fn delete_follow(pool: &SqlitePool, id: i64) -> Result<bool> {
    let res = sqlx::query("DELETE FROM follows WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

const FOLLOW_COLS: &str = "id, plugin_id, kind, feed, query, created_at, last_checked_at, \
     last_error, (SELECT COUNT(*) FROM follow_seen s WHERE s.follow_id = follows.id \
     AND s.state = 'new') AS new_count";

pub async fn list_follows(pool: &SqlitePool) -> Result<Vec<Follow>> {
    Ok(sqlx::query_as(AssertSqlSafe(format!(
        "SELECT {FOLLOW_COLS} FROM follows ORDER BY id"
    )))
    .fetch_all(pool)
    .await?)
}

pub async fn follow_by_id(pool: &SqlitePool, id: i64) -> Result<Option<Follow>> {
    Ok(sqlx::query_as(AssertSqlSafe(format!(
        "SELECT {FOLLOW_COLS} FROM follows WHERE id = ?"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

pub async fn set_follow_checked(pool: &SqlitePool, id: i64, error: Option<&str>) -> Result<()> {
    if error.is_none() {
        sqlx::query(
            "UPDATE follows SET last_checked_at = unixepoch(), last_error = NULL WHERE id = ?",
        )
        .bind(id)
        .execute(pool)
        .await?;
    } else {
        sqlx::query("UPDATE follows SET last_error = ? WHERE id = ?")
            .bind(error)
            .bind(id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

pub async fn follow_seen_references(
    pool: &SqlitePool,
    follow_id: i64,
) -> Result<std::collections::HashSet<String>> {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT reference FROM follow_seen WHERE follow_id = ?")
            .bind(follow_id)
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().map(|(r,)| r).collect())
}

pub async fn insert_follow_seen(
    pool: &SqlitePool,
    follow_id: i64,
    rows: &[(String, &str, Option<String>)],
) -> Result<()> {
    let mut tx = pool.begin().await?;
    for (reference, state, card_json) in rows {
        sqlx::query(
            "INSERT OR IGNORE INTO follow_seen (follow_id, reference, state, card_json, seen_at)
             VALUES (?, ?, ?, ?, unixepoch())",
        )
        .bind(follow_id)
        .bind(reference)
        .bind(state)
        .bind(card_json)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FollowSeenRow {
    pub reference: String,
    pub state: String,
    pub card_json: Option<String>,
    pub seen_at: i64,
}

pub async fn follow_items(pool: &SqlitePool, follow_id: i64) -> Result<Vec<FollowSeenRow>> {
    Ok(sqlx::query_as(
        "SELECT reference, state, card_json, seen_at FROM follow_seen
         WHERE follow_id = ? AND state != 'seen'
         ORDER BY seen_at DESC, reference DESC",
    )
    .bind(follow_id)
    .fetch_all(pool)
    .await?)
}

pub async fn set_follow_item_state(
    pool: &SqlitePool,
    follow_id: i64,
    reference: &str,
    state: &str,
) -> Result<bool> {
    let res = sqlx::query("UPDATE follow_seen SET state = ? WHERE follow_id = ? AND reference = ?")
        .bind(state)
        .bind(follow_id)
        .bind(reference)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn set_follow_new_items_state(
    pool: &SqlitePool,
    follow_id: i64,
    state: &str,
) -> Result<u64> {
    let res = sqlx::query("UPDATE follow_seen SET state = ? WHERE follow_id = ? AND state = 'new'")
        .bind(state)
        .bind(follow_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    #[sqlx::test]
    async fn follows_lifecycle_and_seen_states(pool: SqlitePool) {
        let (id, created) = create_follow(&pool, "openlibrary", "manga", "recent", r#"artist:"x""#)
            .await
            .unwrap();
        assert!(created);
        let (again, created2) =
            create_follow(&pool, "openlibrary", "manga", "recent", r#"artist:"x""#)
                .await
                .unwrap();
        assert!(!created2);
        assert_eq!(id, again, "re-following returns the existing follow");

        insert_follow_seen(
            &pool,
            id,
            &[("101".into(), "seen", None), ("102".into(), "seen", None)],
        )
        .await
        .unwrap();
        insert_follow_seen(
            &pool,
            id,
            &[
                ("103".into(), "new", Some(r#"{"title":"a"}"#.into())),
                ("104".into(), "owned", Some(r#"{"title":"b"}"#.into())),
                ("103".into(), "seen", None),
            ],
        )
        .await
        .unwrap();

        let w = follow_by_id(&pool, id).await.unwrap().unwrap();
        assert_eq!(w.new_count, 1, "only state='new' counts toward the badge");
        let items = follow_items(&pool, id).await.unwrap();
        assert_eq!(items.len(), 2, "baseline rows are not reviewable items");
        assert!(items
            .iter()
            .any(|i| i.reference == "103" && i.state == "new"));

        assert!(set_follow_item_state(&pool, id, "103", "skipped")
            .await
            .unwrap());
        assert_eq!(
            set_follow_new_items_state(&pool, id, "queued")
                .await
                .unwrap(),
            0
        );
        assert_eq!(follow_by_id(&pool, id).await.unwrap().unwrap().new_count, 0);

        set_follow_checked(&pool, id, Some("boom")).await.unwrap();
        assert_eq!(
            follow_by_id(&pool, id)
                .await
                .unwrap()
                .unwrap()
                .last_error
                .as_deref(),
            Some("boom")
        );
        set_follow_checked(&pool, id, None).await.unwrap();
        let w = follow_by_id(&pool, id).await.unwrap().unwrap();
        assert!(w.last_error.is_none() && w.last_checked_at.is_some());

        assert!(delete_follow(&pool, id).await.unwrap());
        let orphans: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM follow_seen")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(orphans, 0, "seen rows cascade with the follow");
    }
}
