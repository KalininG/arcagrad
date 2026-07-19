//! Repository test fixtures.

use sqlx::SqlitePool;

use super::*;

pub(crate) fn meta_with_titles(title: &str, raw_title: Option<&str>) -> ItemMeta {
    ItemMeta {
        id: 1,
        structural_hash: "h".into(),
        title: title.into(),
        raw_title: raw_title.map(Into::into),
        description: None,
        description_manual: false,
        description_source: None,
        page_count: None,
        path: "/p".into(),
        size_bytes: 0,
        kind: "comics".into(),
        modality: "paginated".into(),
        modality_detected: "paginated".into(),
        modality_override: None,
        added_at: 0,
        word_count: None,
        format: "CBZ".into(),
        publisher: None,
        sort_creator: None,
    }
}

/// Insert a default item and return its id.
pub(crate) async fn insert_item(pool: &SqlitePool, hash: &str, added_at: i64) -> i64 {
    sqlx::query_scalar(
            "INSERT INTO items \
             (scheme_tag, structural_hash, path, size_bytes, mtime, format, title, page_count, added_at, last_modified_at) \
             VALUES ('zip-structural-v1', ?, ?, 1, 1, 'cbz', ?, 5, ?, 0) RETURNING id",
        )
        .bind(hash)
        .bind(format!("/p/{hash}"))
        .bind(format!("title-{hash}"))
        .bind(added_at)
        .fetch_one(pool)
        .await
        .unwrap()
}

pub(crate) async fn a_user(pool: &SqlitePool) -> i64 {
    sqlx::query("INSERT INTO users (username, password_hash, role, created_at) VALUES ('u', 'x', 'user', 0)")
            .execute(pool)
            .await
            .unwrap();
    sqlx::query_scalar("SELECT id FROM users")
        .fetch_one(pool)
        .await
        .unwrap()
}

pub(crate) fn ids(r: &ListResult) -> Vec<i64> {
    r.items.iter().map(|a| a.id).collect()
}

pub(crate) async fn insert_item_kind(pool: &SqlitePool, hash: &str, kind: &str) -> i64 {
    sqlx::query_scalar(
            "INSERT INTO items \
             (scheme_tag, structural_hash, path, size_bytes, mtime, format, title, kind, page_count, added_at, last_modified_at) \
             VALUES ('zip-structural-v1', ?, ?, 1, 1, 'cbz', ?, ?, 5, 1, 0) RETURNING id",
        )
        .bind(hash)
        .bind(format!("/p/{hash}"))
        .bind(format!("title-{hash}"))
        .bind(kind)
        .fetch_one(pool)
        .await
        .unwrap()
}

pub(crate) fn comment(source: &str, id: &str, score: i64) -> ItemComment {
    ItemComment {
        source: source.into(),
        external_id: id.into(),
        author: "Fatesifaeve".into(),
        posted_at: Some(1_735_141_500),
        score: Some(score),
        body: "<a href=\"/g/2/\">You are here!</a>".into(),
    }
}

pub(crate) async fn repo_replace(
    pool: &SqlitePool,
    item_id: i64,
    source: &str,
    comments: &[ItemComment],
) {
    assert!(replace_item_comments(pool, item_id, source, comments)
        .await
        .unwrap());
}

pub(crate) fn raw(ns: &str, value: &str, qualifier: &str, source: &str) -> ItemTagRaw {
    ItemTagRaw {
        namespace: ns.into(),
        value: value.into(),
        qualifier: qualifier.into(),
        source: source.into(),
    }
}
