//! Incremental library scanner orchestration.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::SqlitePool;

mod decide;
mod series;
mod walk;
pub mod watcher;

#[cfg(test)]
pub(crate) mod test_util;

pub use decide::changed_paths;
pub(crate) use walk::mtime_secs;

use decide::{
    buckets_by_structural, compute_plan, decide_targeted, probe_paths, rows_by_path, under,
    ExistingRow, Probe, SELECT_ROW,
};
use series::{affected_series_folders, reconcile_series, reconcile_series_scoped};

/// Kind assigned to files directly under the library root.
pub const DEFAULT_KIND: &str = "uncategorized";

/// Summary returned by a scan.
#[derive(Debug, Default, Serialize)]
pub struct ScanStats {
    pub total: usize,
    pub added: usize,
    pub updated: usize,
    pub moved: usize,
    pub removed: usize,
    pub unchanged: usize,
    pub duplicates: usize,
    pub errored: usize,
    pub series: usize,
    #[serde(skip)]
    pub moved_kind_changed: Vec<i64>,
    #[serde(skip)]
    pub moved_leaf_status_changed: Vec<i64>,
    #[serde(skip)]
    pub removed_thumbs: Vec<String>,
    #[serde(skip)]
    pub removed_ids: Vec<i64>,
    #[serde(skip)]
    pub added_ids: Vec<(i64, String)>,
    #[serde(skip)]
    pub added_reflowable: Vec<(i64, String)>,
    #[serde(skip)]
    pub added_paginated: Vec<(i64, String)>,
    #[serde(skip)]
    pub deferred: Vec<String>,
}

enum Mutation {
    Insert {
        scheme_tag: String,
        structural_hash: String,
        deep_hash: Option<String>,
        path: String,
        size: i64,
        mtime: i64,
        format: String,
        title: String,
        raw_title: String,
        kind: String,
        modality: &'static str,
        page_count: Option<i64>,
        chapters: Vec<crate::media::chapters::Chapter>,
        series_index: Option<f64>,
    },
    Repoint {
        id: i64,
        path: String,
        kind: String,
        size: i64,
        mtime: i64,
    },
    Touch {
        id: i64,
        size: i64,
        mtime: i64,
    },
    UpdateContent {
        id: i64,
        scheme_tag: String,
        structural_hash: String,
        size: i64,
        mtime: i64,
        page_count: Option<i64>,
        chapters: Vec<crate::media::chapters::Chapter>,
    },
    SetDeepHash {
        id: i64,
        deep_hash: String,
    },
    Delete {
        id: i64,
    },
}

struct Plan {
    mutations: Vec<Mutation>,
    stats: ScanStats,
}

pub fn is_archive(path: &Path) -> bool {
    crate::media::format::is_supported(path)
}

/// Reconcile the full library and its series.
pub async fn scan(pool: &SqlitePool, content_dir: &Path) -> Result<ScanStats> {
    let mut stats = reconcile(pool, content_dir).await?;
    backfill_epub_fields(pool).await?;
    stats.series = reconcile_series(pool, content_dir, unix_now()).await?;
    Ok(stats)
}

/// Backfill EPUB fields added after initial indexing.
async fn backfill_epub_fields(pool: &SqlitePool) -> Result<()> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, path FROM items WHERE modality = 'reflowable' \
         AND (series_index IS NULL OR publisher IS NULL)",
    )
    .fetch_all(pool)
    .await
    .context("load reflowable items for epub-field backfill")?;
    for (id, path) in rows {
        let p = PathBuf::from(&path);
        let Some(meta) = tokio::task::spawn_blocking(move || crate::media::epub::inspect(&p))
            .await
            .ok()
            .and_then(|r| r.ok())
        else {
            continue;
        };
        if let Some(idx) = meta.series_index {
            sqlx::query("UPDATE items SET series_index = ? WHERE id = ?")
                .bind(idx)
                .bind(id)
                .execute(pool)
                .await
                .context("persist backfilled series_index")?;
        }
        if let Some(publisher) = meta.publisher.as_deref() {
            sqlx::query("UPDATE items SET publisher = ? WHERE id = ?")
                .bind(publisher)
                .bind(id)
                .execute(pool)
                .await
                .context("persist backfilled publisher")?;
        }
    }
    Ok(())
}

/// Plan off-runtime, then apply mutations in one transaction.
async fn reconcile(pool: &SqlitePool, content_dir: &Path) -> Result<ScanStats> {
    let existing: Vec<ExistingRow> = sqlx::query_as(SELECT_ROW)
        .fetch_all(pool)
        .await
        .context("load existing items")?;

    let content_dir = content_dir.to_path_buf();
    let now = unix_now();

    let plan = tokio::task::spawn_blocking(move || compute_plan(&content_dir, existing, now))
        .await
        .context("scan task join")??;

    let mut stats = plan.stats;
    let applied = apply_plan(pool, plan.mutations, now).await?;
    stats.added_ids = applied.added_ids;
    stats.added_reflowable = applied.added_reflowable;
    stats.added_paginated = applied.added_paginated;
    Ok(stats)
}

#[derive(Default)]
struct Applied {
    added_ids: Vec<(i64, String)>,
    added_reflowable: Vec<(i64, String)>,
    added_paginated: Vec<(i64, String)>,
}

async fn insert_item_chapters(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    item_id: i64,
    chapters: &[crate::media::chapters::Chapter],
) -> Result<()> {
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
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn apply_plan(pool: &SqlitePool, mutations: Vec<Mutation>, now: i64) -> Result<Applied> {
    let mut applied = Applied::default();
    if mutations.is_empty() {
        return Ok(applied);
    }
    let Applied {
        added_ids,
        added_reflowable,
        added_paginated,
    } = &mut applied;
    let mut tx = pool.begin().await.context("begin scan tx")?;

    // Deletes free unique paths before inserts and repoints claim them.
    let (deletes, rest): (Vec<_>, Vec<_>) = mutations
        .into_iter()
        .partition(|m| matches!(m, Mutation::Delete { .. }));

    for m in deletes.into_iter().chain(rest) {
        match m {
            Mutation::Insert {
                scheme_tag,
                structural_hash,
                deep_hash,
                path,
                size,
                mtime,
                format,
                title,
                raw_title,
                kind,
                modality,
                page_count,
                chapters,
                series_index,
            } => {
                let chapters_done = page_count.is_some() as i64;
                let id: i64 = sqlx::query_scalar(
                    "INSERT INTO items \
                     (scheme_tag, structural_hash, deep_hash, path, size_bytes, mtime, format, title, raw_title, kind, modality, page_count, chapters_done, series_index, added_at, last_modified_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
                )
                .bind(scheme_tag)
                .bind(structural_hash)
                .bind(deep_hash)
                .bind(&path)
                .bind(size)
                .bind(mtime)
                .bind(format)
                .bind(title)
                .bind(raw_title)
                .bind(&kind)
                .bind(modality)
                .bind(page_count)
                .bind(chapters_done)
                .bind(series_index)
                .bind(now)
                .bind(now)
                .fetch_one(&mut *tx)
                .await?;
                insert_item_chapters(&mut tx, id, &chapters).await?;
                if modality == "reflowable" {
                    added_reflowable.push((id, path));
                } else if modality == "paginated" {
                    added_paginated.push((id, path));
                }
                added_ids.push((id, kind));
            }
            Mutation::Repoint {
                id,
                path,
                kind,
                size,
                mtime,
            } => {
                sqlx::query(
                    "UPDATE items SET path = ?, kind = ?, size_bytes = ?, mtime = ?, \
                     last_modified_at = ? WHERE id = ?",
                )
                .bind(path)
                .bind(kind)
                .bind(size)
                .bind(mtime)
                .bind(now)
                .bind(id)
                .execute(&mut *tx)
                .await?;
            }
            Mutation::Touch { id, size, mtime } => {
                sqlx::query(
                    "UPDATE items SET size_bytes = ?, mtime = ?, last_modified_at = ? WHERE id = ?",
                )
                .bind(size)
                .bind(mtime)
                .bind(now)
                .bind(id)
                .execute(&mut *tx)
                .await?;
            }
            Mutation::UpdateContent {
                id,
                scheme_tag,
                structural_hash,
                size,
                mtime,
                page_count,
                chapters,
            } => {
                let chapters_done = page_count.is_some() as i64;
                sqlx::query(
                    "UPDATE items SET scheme_tag = ?, structural_hash = ?, deep_hash = NULL, \
                     size_bytes = ?, mtime = ?, page_count = ?, chapters_done = ?, \
                     last_modified_at = ? WHERE id = ?",
                )
                .bind(scheme_tag)
                .bind(structural_hash)
                .bind(size)
                .bind(mtime)
                .bind(page_count)
                .bind(chapters_done)
                .bind(now)
                .bind(id)
                .execute(&mut *tx)
                .await?;
                sqlx::query("DELETE FROM item_chapters WHERE item_id = ?")
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                insert_item_chapters(&mut tx, id, &chapters).await?;
            }
            Mutation::SetDeepHash { id, deep_hash } => {
                sqlx::query("UPDATE items SET deep_hash = ? WHERE id = ?")
                    .bind(deep_hash)
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
            }
            Mutation::Delete { id } => {
                sqlx::query("DELETE FROM items WHERE id = ?")
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
            }
        }
    }
    tx.commit().await.context("commit scan tx")?;
    Ok(applied)
}

/// Reconcile watcher-targeted paths.
pub async fn reconcile_paths(
    pool: &SqlitePool,
    content_dir: &Path,
    paths: Vec<PathBuf>,
) -> Result<ScanStats> {
    let flagged = paths.len();
    let mut paths: Vec<PathBuf> = paths
        .into_iter()
        .filter(|p| is_archive(p) && under(content_dir, p))
        .collect();
    paths.sort();
    paths.dedup();
    if paths.is_empty() {
        if flagged > 0 {
            tracing::warn!(
                "targeted reconcile: {flagged} flagged path(s), none under content_dir {} — \
                 skipped (the full/boot scan will still pick them up)",
                content_dir.display()
            );
        }
        return Ok(ScanStats::default());
    }

    let path_strs: Vec<String> = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();

    let by_path = rows_by_path(pool, &path_strs).await?;

    let probes = {
        let (paths, by_path) = (paths.clone(), by_path.clone());
        tokio::task::spawn_blocking(move || probe_paths(&paths, &by_path))
            .await
            .context("probe join")?
    };

    let hashes: Vec<String> = probes
        .iter()
        .filter_map(|pr| match pr {
            Probe::Present {
                structural_hash, ..
            } => Some(structural_hash.clone()),
            _ => None,
        })
        .collect();
    let by_bucket = buckets_by_structural(pool, &hashes).await?;

    let cdir = content_dir.to_path_buf();
    let plan =
        tokio::task::spawn_blocking(move || decide_targeted(&cdir, probes, &by_path, &by_bucket))
            .await
            .context("decide join")?;

    let Plan {
        mutations,
        mut stats,
    } = plan;
    let applied = apply_plan(pool, mutations, unix_now()).await?;
    stats.added_ids = applied.added_ids;
    stats.added_reflowable = applied.added_reflowable;
    stats.added_paginated = applied.added_paginated;

    if stats.added + stats.moved + stats.removed > 0 {
        let folders = affected_series_folders(pool, content_dir, &path_strs).await?;
        if !folders.is_empty() {
            reconcile_series_scoped(pool, content_dir, &folders, unix_now()).await?;
        }
    }
    Ok(stats)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::test_util::{items, write_cbz, write_epub};
    use std::io::Write;

    #[sqlx::test]
    async fn scan_indexes_a_valid_cbr(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("manga")).unwrap();
        let p = root.join("manga").join("v01.cbr");
        std::fs::write(
            &p,
            crate::media::rar::rar4(&[("001.jpg", b"page-one"), ("002.jpg", b"page-two")]),
        )
        .unwrap();

        let stats = reconcile(&pool, root).await.unwrap();
        assert_eq!(stats.added, 1, "a valid cbr is indexed");
        let scheme: String = sqlx::query_scalar("SELECT scheme_tag FROM items")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(scheme, "rar-structural-v2", "indexed under the rar scheme");
    }

    #[sqlx::test]
    async fn scan_indexes_rar_mislabeled_as_cbz(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("manga")).unwrap();
        let p = root.join("manga").join("Vagabond v03.cbz");
        std::fs::write(&p, crate::media::rar::rar4(&[("000.jpg", b"cover")])).unwrap();

        let stats = reconcile(&pool, root).await.unwrap();
        assert_eq!(stats.added, 1, "a rar-in-a-.cbz still indexes");
        let scheme: String = sqlx::query_scalar("SELECT scheme_tag FROM items")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(scheme, "rar-structural-v2", "by magic, not extension");
    }

    #[sqlx::test]
    async fn scan_indexes_an_epub_as_reflowable_with_opf_title(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("books")).unwrap();
        let p = root.join("books").join("[Publisher] whatever-file.epub");
        write_epub(&p, "A Real Book Title", "chapter one");

        let stats = reconcile(&pool, root).await.unwrap();
        assert_eq!(stats.added, 1, "a valid epub is indexed");
        let (scheme, modality, kind, title): (String, String, String, String) =
            sqlx::query_as("SELECT scheme_tag, modality, kind, title FROM items")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            scheme, "epub-structural-v1",
            "own scheme, not the comic zip"
        );
        assert_eq!(
            modality, "reflowable",
            "reader dispatches on this, never kind"
        );
        assert_eq!(kind, "books", "kind is the top-level folder");
        assert_eq!(
            title, "A Real Book Title",
            "OPF title, not the bracketed filename"
        );
    }

    #[sqlx::test]
    async fn scan_eagerly_populates_page_count_and_chapters(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let chaptered = root.join("webtoon.cbz");
        {
            let f = std::fs::File::create(&chaptered).unwrap();
            let mut z = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            let mut put = |name: String| {
                z.start_file(name, opts).unwrap();
                z.write_all(b"img").unwrap();
            };
            for p in 1..=2 {
                put(format!("{p:04}_0000.jpg"));
            }
            for ch in 1..=2 {
                for p in 1..=3 {
                    put(format!("CH{ch:03}_{p:03}.jpg"));
                }
            }
            z.finish().unwrap();
        }
        write_cbz(&root.join("flat.cbz"), "flat");

        reconcile(&pool, root).await.unwrap();

        let (pc, done): (Option<i64>, i64) = sqlx::query_as(
            "SELECT page_count, chapters_done FROM items WHERE path LIKE '%webtoon.cbz'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            pc,
            Some(8),
            "page_count computed at scan, not lazily on open"
        );
        assert_eq!(done, 1, "chapters scanned at scan time");
        let nch: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM item_chapters ic JOIN items i ON i.id = ic.item_id \
             WHERE i.path LIKE '%webtoon.cbz'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(nch, 3, "front matter + 2 chapters persisted at scan");

        let (fpc, fdone): (Option<i64>, i64) = sqlx::query_as(
            "SELECT page_count, chapters_done FROM items WHERE path LIKE '%flat.cbz'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(fpc, Some(1));
        assert_eq!(fdone, 1);
        let fnch: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM item_chapters ic JOIN items i ON i.id = ic.item_id \
             WHERE i.path LIKE '%flat.cbz'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(fnch, 0, "a flat gallery has no chapters");
    }

    #[sqlx::test]
    async fn scan_inserts_then_is_idempotent(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        write_cbz(&dir.path().join("a.cbz"), "a");
        write_cbz(&dir.path().join("b.cbz"), "b");

        let s1 = scan(&pool, dir.path()).await.unwrap();
        assert_eq!(s1.added, 2);

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM items")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 2);

        let s2 = scan(&pool, dir.path()).await.unwrap();
        assert_eq!(s2.unchanged, 2);
        assert_eq!(s2.added, 0);
    }

    #[sqlx::test]
    async fn scan_sets_kind_from_folder_and_modality(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("manga")).unwrap();
        std::fs::create_dir_all(root.join("comics")).unwrap();
        write_cbz(&root.join("manga").join("m.cbz"), "m");
        write_cbz(&root.join("comics").join("d.cbz"), "d");

        scan(&pool, root).await.unwrap();

        let rows: Vec<(String, String, String)> =
            sqlx::query_as("SELECT title, kind, modality FROM items ORDER BY title")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            rows,
            vec![
                ("d".into(), "comics".into(), "paginated".into()),
                ("m".into(), "manga".into(), "paginated".into()),
            ]
        );
    }

    #[sqlx::test]
    async fn scan_stores_clean_title_and_preserves_raw(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("comics")).unwrap();
        let raw = "[Circle (Artist)] Real Title [English] {v2}";
        write_cbz(&dir.path().join("comics").join(format!("{raw}.cbz")), "x");

        scan(&pool, dir.path()).await.unwrap();

        let (title, raw_title): (String, Option<String>) =
            sqlx::query_as("SELECT title, raw_title FROM items")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(title, "Real Title", "served title is the cleaned form");
        assert_eq!(
            raw_title.as_deref(),
            Some(raw),
            "original filename preserved"
        );
    }

    #[sqlx::test]
    async fn scan_rederives_kind_when_an_item_moves_between_folders(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("comics")).unwrap();
        std::fs::create_dir_all(root.join("manga")).unwrap();
        let a = root.join("comics").join("a.cbz");
        write_cbz(&a, "a");
        scan(&pool, root).await.unwrap();

        let (id_before, kind_before): (i64, String) = sqlx::query_as("SELECT id, kind FROM items")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(kind_before, "comics");

        std::fs::rename(&a, root.join("manga").join("a.cbz")).unwrap();
        let s = scan(&pool, root).await.unwrap();
        assert_eq!(s.moved, 1);
        assert_eq!(s.added, 0);
        assert_eq!(s.removed, 0);
        assert_eq!(s.moved_kind_changed, vec![id_before]);

        let (id_after, kind_after): (i64, String) = sqlx::query_as("SELECT id, kind FROM items")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(id_after, id_before, "same row — metadata preserved");
        assert_eq!(kind_after, "manga", "kind re-derived from the new folder");
    }

    #[sqlx::test]
    async fn scan_handles_a_folder_rename_as_a_kind_change(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("manga")).unwrap();
        write_cbz(&root.join("manga").join("a.cbz"), "a");
        write_cbz(&root.join("manga").join("b.cbz"), "b");
        scan(&pool, root).await.unwrap();
        let ids_before: Vec<i64> = sqlx::query_scalar("SELECT id FROM items ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();

        std::fs::rename(root.join("manga"), root.join("comics")).unwrap();
        let s = scan(&pool, root).await.unwrap();
        assert_eq!(s.moved, 2);
        assert_eq!(s.added, 0);
        assert_eq!(s.removed, 0);

        let kinds: Vec<String> = sqlx::query_scalar("SELECT DISTINCT kind FROM items")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(kinds, vec!["comics".to_string()]);
        let ids_after: Vec<i64> = sqlx::query_scalar("SELECT id FROM items ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(ids_after, ids_before, "same rows — metadata preserved");
    }

    #[sqlx::test]
    async fn scan_indexes_a_root_file_in_place_as_uncategorized(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let a = root.join("a.cbz");
        write_cbz(&a, "a");

        let s = scan(&pool, root).await.unwrap();
        assert_eq!(s.added, 1);
        assert_eq!(s.moved, 0, "no move — indexed in place");

        let (path, kind): (String, String) = sqlx::query_as("SELECT path, kind FROM items")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(path, a.to_string_lossy(), "file left exactly where it was");
        assert_eq!(kind, DEFAULT_KIND);
        assert!(a.exists(), "original untouched");
        assert!(
            !root.join(DEFAULT_KIND).exists(),
            "no uncategorized/ folder was created"
        );
    }

    #[sqlx::test]
    async fn scan_preserves_id_across_move(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.cbz");
        write_cbz(&a, "a");
        reconcile(&pool, dir.path()).await.unwrap();

        let (id_before, structural): (i64, String) =
            sqlx::query_as("SELECT id, structural_hash FROM items")
                .fetch_one(&pool)
                .await
                .unwrap();

        let b = dir.path().join("moved.cbz");
        std::fs::rename(&a, &b).unwrap();

        let s = reconcile(&pool, dir.path()).await.unwrap();
        assert_eq!(s.moved, 1);
        assert_eq!(s.added, 0);
        assert_eq!(s.removed, 0);

        let (id_after, path_after, structural_after): (i64, String, String) =
            sqlx::query_as("SELECT id, path, structural_hash FROM items")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(id_before, id_after);
        assert_eq!(structural, structural_after);
        assert_eq!(path_after, b.to_string_lossy());
    }

    #[sqlx::test]
    async fn scan_removes_deleted_file(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.cbz");
        write_cbz(&a, "a");
        reconcile(&pool, dir.path()).await.unwrap();

        std::fs::remove_file(&a).unwrap();
        let s = reconcile(&pool, dir.path()).await.unwrap();
        assert_eq!(s.removed, 1);

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM items")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[sqlx::test]
    async fn full_scan_removes_items_under_a_deleted_folder(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let series = root.join("manga").join("My Series");
        std::fs::create_dir_all(&series).unwrap();
        write_cbz(&series.join("v01.cbz"), "v1");
        write_cbz(&series.join("v02.cbz"), "v2");
        reconcile(&pool, root).await.unwrap();
        assert_eq!(items(&pool).await.len(), 2);

        std::fs::remove_dir_all(&series).unwrap();
        let s = reconcile(&pool, root).await.unwrap();
        assert_eq!(
            s.removed, 2,
            "both items under the deleted folder are removed"
        );
        assert!(items(&pool).await.is_empty());
    }

    #[sqlx::test]
    async fn scan_handles_in_place_content_change(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.cbz");
        write_cbz(&a, "original");
        reconcile(&pool, dir.path()).await.unwrap();

        let (id_before, structural_before): (i64, String) =
            sqlx::query_as("SELECT id, structural_hash FROM items")
                .fetch_one(&pool)
                .await
                .unwrap();

        write_cbz(
            &a,
            "edited — totally different content now, longer than before",
        );

        let stats = reconcile(&pool, dir.path()).await.unwrap();
        assert_eq!(stats.updated, 1);
        assert_eq!(stats.added, 0);
        assert_eq!(stats.removed, 0);

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM items")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "exactly one row, not deleted+reinserted");

        let (id_after, structural_after): (i64, String) =
            sqlx::query_as("SELECT id, structural_hash FROM items")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(id_after, id_before, "row id preserved across in-place edit");
        assert_ne!(
            structural_after, structural_before,
            "structural hash updated"
        );
    }

    #[sqlx::test]
    async fn scan_handles_move_into_a_freed_path(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.cbz");
        let b = dir.path().join("b.cbz");
        write_cbz(&a, "content-Q");
        write_cbz(&b, "content-H");
        reconcile(&pool, dir.path()).await.unwrap();

        std::fs::remove_file(&b).unwrap();
        write_cbz(&a, "content-H");

        reconcile(&pool, dir.path()).await.unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM items")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        let path: String = sqlx::query_scalar("SELECT path FROM items")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(path, a.to_string_lossy());
    }

    #[sqlx::test]
    async fn scan_swap_does_not_crash_or_drop_rows(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.cbz");
        let b = dir.path().join("b.cbz");
        write_cbz(&a, "X");
        write_cbz(&b, "Y");
        reconcile(&pool, dir.path()).await.unwrap();

        write_cbz(&a, "Y");
        write_cbz(&b, "X");
        reconcile(&pool, dir.path()).await.unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM items")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 2, "both items still listed after a swap");
    }

    #[cfg(unix)]
    fn permissions_enforced() -> bool {
        use std::os::unix::fs::PermissionsExt;
        let Ok(dir) = tempfile::tempdir() else {
            return true;
        };
        let probe = dir.path().join("probe");
        if std::fs::write(&probe, b"x").is_err()
            || std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o000)).is_err()
        {
            return true;
        }
        std::fs::read(&probe).is_err()
    }

    #[cfg(unix)]
    #[sqlx::test]
    async fn scan_skips_unreadable_new_file_without_aborting(pool: SqlitePool) {
        use std::os::unix::fs::PermissionsExt;
        if !permissions_enforced() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        write_cbz(&dir.path().join("good.cbz"), "good");
        let bad = dir.path().join("bad.cbz");
        write_cbz(&bad, "bad");
        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o000)).unwrap();

        let stats = reconcile(&pool, dir.path()).await.unwrap();
        assert_eq!(stats.added, 1);
        assert!(stats.errored >= 1);

        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o644)).unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM items")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[cfg(unix)]
    #[sqlx::test]
    async fn scan_keeps_indexed_file_that_became_unreadable(pool: SqlitePool) {
        use std::os::unix::fs::PermissionsExt;
        if !permissions_enforced() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.cbz");
        write_cbz(&f, "original");
        reconcile(&pool, dir.path()).await.unwrap();

        write_cbz(&f, "rewritten with different, longer content");
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o000)).unwrap();

        let stats = reconcile(&pool, dir.path()).await.unwrap();
        assert!(stats.errored >= 1);
        assert_eq!(
            stats.removed, 0,
            "an unreadable-but-present file is not deleted"
        );

        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM items")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "row kept across a transient unreadable scan");
    }

    #[cfg(unix)]
    #[sqlx::test]
    async fn scan_does_not_delete_under_an_unreadable_directory(pool: SqlitePool) {
        use std::os::unix::fs::PermissionsExt;
        if !permissions_enforced() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        write_cbz(&sub.join("a.cbz"), "a");
        write_cbz(&sub.join("b.cbz"), "b");
        reconcile(&pool, dir.path()).await.unwrap();

        let count_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM items")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count_before, 2);

        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o000)).unwrap();
        let stats = reconcile(&pool, dir.path()).await.unwrap();
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(stats.errored >= 1, "the directory error must be detected");
        assert_eq!(
            stats.removed, 0,
            "must not delete from a partial view of disk"
        );

        let count_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM items")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            count_after, 2,
            "subtree preserved through a directory error"
        );
    }
}
