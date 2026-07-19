//! Series grouping and leaf ordering.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use sqlx::{Sqlite, SqlitePool, Transaction};

use super::walk::derive_title;
use super::DEFAULT_KIND;

type Tx<'a> = Transaction<'a, Sqlite>;

/// Find the series folders affected by targeted paths.
pub(super) async fn affected_series_folders(
    pool: &SqlitePool,
    content_dir: &Path,
    path_strs: &[String],
) -> Result<HashSet<String>> {
    let mut folders: HashSet<String> = HashSet::new();
    for p in path_strs {
        if let Some(f) = series_folder_of(content_dir, Path::new(p)) {
            folders.insert(f);
        }
    }
    if !path_strs.is_empty() {
        let placeholders = vec!["?"; path_strs.len()].join(", ");
        let sql = format!(
            "SELECT DISTINCT s.folder_path FROM items i JOIN series s ON s.id = i.series_id \
             WHERE i.path IN ({placeholders}) AND s.folder_path IS NOT NULL"
        );
        let mut q = sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(sql));
        for p in path_strs {
            q = q.bind(p);
        }
        for f in q.fetch_all(pool).await? {
            folders.insert(f);
        }
    }
    Ok(folders)
}

/// Reconcile only watcher-affected series folders.
pub(super) async fn reconcile_series_scoped(
    pool: &SqlitePool,
    content_dir: &Path,
    folders: &HashSet<String>,
    now: i64,
) -> Result<()> {
    let mut tx = pool.begin().await.context("begin scoped series tx")?;
    for folder in folders {
        // Range bounds keep this lookup on the binary path index.
        let prefix = content_dir.join(folder).to_string_lossy().into_owned();
        let lo = format!("{prefix}/");
        let hi = format!("{prefix}0");
        let candidates: Vec<(i64, String, String, Option<f64>)> = sqlx::query_as(
            "SELECT id, path, kind, series_index FROM items WHERE path >= ? AND path < ?",
        )
        .bind(&lo)
        .bind(&hi)
        .fetch_all(&mut *tx)
        .await?;
        let members: Vec<(i64, String, String, Option<f64>)> = candidates
            .into_iter()
            .filter(|(_, path, _, _)| {
                series_folder_of(content_dir, Path::new(path)).as_deref() == Some(folder.as_str())
            })
            .collect();

        let existing_sid: Option<i64> =
            sqlx::query_scalar("SELECT id FROM series WHERE folder_path = ?")
                .bind(folder)
                .fetch_optional(&mut *tx)
                .await?;

        if members.is_empty() {
            if let Some(sid) = existing_sid {
                sqlx::query("UPDATE items SET series_id = NULL WHERE series_id = ?")
                    .bind(sid)
                    .execute(&mut *tx)
                    .await?;
                delete_series_and_neighbors(&mut tx, sid).await?;
            }
            continue;
        }

        let member_tuples: Vec<(i64, String, String)> = members
            .iter()
            .map(|(id, path, kind, _)| (*id, derive_title(Path::new(path)), kind.clone()))
            .collect();
        let member_ids: Vec<i64> = members.iter().map(|(id, _, _, _)| *id).collect();
        let series_index: HashMap<i64, f64> = members
            .iter()
            .filter_map(|(id, _, _, sidx)| sidx.map(|v| (*id, v)))
            .collect();

        let existing_leaf = load_leaf_snapshots(&mut tx, &member_ids).await?;
        let sid = reconcile_one_series(
            &mut tx,
            folder,
            &member_tuples,
            &series_index,
            existing_sid,
            &existing_leaf,
            now,
        )
        .await?;

        let placeholders = vec!["?"; member_ids.len()].join(", ");
        let sql =
            format!("SELECT id FROM items WHERE series_id = ? AND id NOT IN ({placeholders})");
        let mut q = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(sql)).bind(sid);
        for id in &member_ids {
            q = q.bind(id);
        }
        for id in q.fetch_all(&mut *tx).await? {
            detach_item(&mut tx, id).await?;
        }

        refresh_series_sort_creator(&mut tx, sid).await?;
    }
    tx.commit().await.context("commit scoped series tx")?;
    Ok(())
}

/// Return the stable `kind/series` key for a series leaf.
pub(super) fn series_folder_of(content_dir: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(content_dir).ok()?;
    let comps: Vec<_> = rel.components().collect();
    if comps.len() < 3 {
        return None;
    }
    let kind = comps[0].as_os_str().to_str()?;
    let sub = comps[1].as_os_str().to_str()?;
    Some(format!("{kind}/{sub}"))
}

fn series_basename(folder_path: &str) -> String {
    let base = folder_path.rsplit('/').next().unwrap_or(folder_path);
    crate::media::title::clean(base)
}

type LeafSnapshot = (i64, f64, Option<String>, Option<f64>, Option<f64>);

type LeafRowRaw = (i64, i64, f64, Option<String>, Option<f64>, Option<f64>);

struct LeafRow {
    number_sort: f64,
    number_disp: Option<String>,
    volume: Option<f64>,
    chapter: Option<f64>,
}

/// Combine parsed numbering with natural order for unmarked leaves.
fn compute_leaf_numbers(
    members: &[(i64, String, String)],
    series_index: &HashMap<i64, f64>,
) -> Vec<(i64, LeafRow)> {
    let parsed: Vec<Option<crate::media::series::LeafNumber>> = members
        .iter()
        .map(|(id, stem, _)| {
            // EPUB series metadata overrides filename-derived order.
            series_index
                .get(id)
                .map(|&sort| crate::media::series::LeafNumber {
                    sort,
                    display: None,
                    volume: None,
                    chapter: None,
                })
                .or_else(|| crate::media::series::parse_leaf_number(stem))
        })
        .collect();
    let max_parsed = parsed
        .iter()
        .flatten()
        .map(|n| n.sort)
        .fold(f64::NEG_INFINITY, f64::max);
    let base = if max_parsed.is_finite() {
        max_parsed
    } else {
        0.0
    };

    let mut unparsed: Vec<usize> = (0..members.len())
        .filter(|&i| parsed[i].is_none())
        .collect();
    unparsed.sort_by(|&a, &b| crate::media::series::natural_cmp(&members[a].1, &members[b].1));
    let mut appended: HashMap<usize, f64> = HashMap::new();
    for (k, &i) in unparsed.iter().enumerate() {
        appended.insert(i, base + (k as f64) + 1.0);
    }

    members
        .iter()
        .enumerate()
        .map(|(i, (id, _, _))| {
            let row = match &parsed[i] {
                Some(n) => LeafRow {
                    number_sort: n.sort,
                    number_disp: n.display.clone(),
                    volume: n.volume,
                    chapter: n.chapter,
                },
                None => LeafRow {
                    number_sort: appended[&i],
                    number_disp: None,
                    volume: None,
                    chapter: None,
                },
            };
            (*id, row)
        })
        .collect()
}

/// Preserve manual titles while updating scanner-owned fields.
async fn upsert_series(
    tx: &mut Tx<'_>,
    existing_sid: Option<i64>,
    folder: &str,
    title: &str,
    kind: &str,
    now: i64,
) -> Result<i64> {
    Ok(match existing_sid {
        Some(sid) => {
            sqlx::query(
                "UPDATE series SET title = CASE WHEN title_manual = 1 THEN title ELSE ? END, kind = ? \
                 WHERE id = ? AND ((title <> ? AND title_manual = 0) OR kind <> ?)",
            )
            .bind(title)
            .bind(kind)
            .bind(sid)
            .bind(title)
            .bind(kind)
            .execute(&mut **tx)
            .await?;
            sid
        }
        None => {
            sqlx::query_scalar(
                "INSERT INTO series (kind, title, folder_path, added_at) VALUES (?, ?, ?, ?) RETURNING id",
            )
            .bind(kind)
            .bind(title)
            .bind(folder)
            .bind(now)
            .fetch_one(&mut **tx)
            .await?
        }
    })
}

async fn write_leaf(tx: &mut Tx<'_>, item_id: i64, sid: i64, row: &LeafRow) -> Result<()> {
    sqlx::query(
        "INSERT INTO item_series_leaf (item_id, series_id, number_sort, number_disp, volume, chapter) \
         VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT(item_id) DO UPDATE SET \
           series_id = excluded.series_id, number_sort = excluded.number_sort, \
           number_disp = excluded.number_disp, volume = excluded.volume, chapter = excluded.chapter",
    )
    .bind(item_id)
    .bind(sid)
    .bind(row.number_sort)
    .bind(&row.number_disp)
    .bind(row.volume)
    .bind(row.chapter)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn delete_leaf(tx: &mut Tx<'_>, item_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM item_series_leaf WHERE item_id = ?")
        .bind(item_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn null_series_id(tx: &mut Tx<'_>, item_id: i64) -> Result<()> {
    sqlx::query("UPDATE items SET series_id = NULL WHERE id = ?")
        .bind(item_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn detach_item(tx: &mut Tx<'_>, item_id: i64) -> Result<()> {
    null_series_id(tx, item_id).await?;
    delete_leaf(tx, item_id).await
}

/// Remove a series and its non-FK recommendation edges.
async fn delete_series_and_neighbors(tx: &mut Tx<'_>, sid: i64) -> Result<()> {
    sqlx::query("DELETE FROM series WHERE id = ?")
        .bind(sid)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "DELETE FROM entry_neighbors \
         WHERE (src_type = 's' AND src_id = ?) OR (dst_type = 's' AND dst_id = ?)",
    )
    .bind(sid)
    .bind(sid)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn refresh_series_sort_creator(tx: &mut Tx<'_>, sid: i64) -> Result<()> {
    sqlx::query(
        "UPDATE series SET sort_creator = \
             (SELECT MIN(i.sort_creator) FROM items i WHERE i.series_id = ?) WHERE id = ?",
    )
    .bind(sid)
    .bind(sid)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn load_leaf_snapshots(
    tx: &mut Tx<'_>,
    item_ids: &[i64],
) -> Result<HashMap<i64, LeafSnapshot>> {
    if item_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = vec!["?"; item_ids.len()].join(", ");
    let sql = format!(
        "SELECT item_id, series_id, number_sort, number_disp, volume, chapter \
         FROM item_series_leaf WHERE item_id IN ({placeholders})"
    );
    let mut q = sqlx::query_as::<_, LeafRowRaw>(sqlx::AssertSqlSafe(sql));
    for id in item_ids {
        q = q.bind(id);
    }
    Ok(q.fetch_all(&mut **tx)
        .await?
        .into_iter()
        .map(|(iid, sid, ns, nd, v, c)| (iid, (sid, ns, nd, v, c)))
        .collect())
}

/// Reconcile one folder while skipping unchanged leaf rows.
async fn reconcile_one_series(
    tx: &mut Tx<'_>,
    folder: &str,
    members: &[(i64, String, String)],
    series_index: &HashMap<i64, f64>,
    existing_sid: Option<i64>,
    existing_leaf: &HashMap<i64, LeafSnapshot>,
    now: i64,
) -> Result<i64> {
    let kind = members
        .first()
        .map(|m| m.2.as_str())
        .unwrap_or(DEFAULT_KIND);
    let title = series_basename(folder);
    let sid = upsert_series(tx, existing_sid, folder, &title, kind, now).await?;

    for (item_id, row) in compute_leaf_numbers(members, series_index) {
        sqlx::query(
            "UPDATE items SET series_id = ? WHERE id = ? AND (series_id IS NULL OR series_id <> ?)",
        )
        .bind(sid)
        .bind(item_id)
        .bind(sid)
        .execute(&mut **tx)
        .await?;

        let differs = match existing_leaf.get(&item_id) {
            Some((esid, ens, end, ev, ec)) => {
                *esid != sid
                    || *ens != row.number_sort
                    || *end != row.number_disp
                    || *ev != row.volume
                    || *ec != row.chapter
            }
            None => true,
        };
        if differs {
            write_leaf(tx, item_id, sid, &row).await?;
        }
    }
    Ok(sid)
}

/// Reconcile all scanner-managed series and remove orphans.
pub(super) async fn reconcile_series(
    pool: &SqlitePool,
    content_dir: &Path,
    now: i64,
) -> Result<usize> {
    type ItemRow = (i64, String, String, Option<i64>, Option<f64>);
    let rows: Vec<ItemRow> =
        sqlx::query_as("SELECT id, path, kind, series_id, series_index FROM items")
            .fetch_all(pool)
            .await
            .context("load items for series reconcile")?;

    let mut groups: HashMap<String, Vec<(i64, String, String)>> = HashMap::new();
    let mut current_series_id: HashMap<i64, Option<i64>> = HashMap::with_capacity(rows.len());
    let mut series_index: HashMap<i64, f64> = HashMap::new();
    for (id, path, kind, sid, sidx) in &rows {
        current_series_id.insert(*id, *sid);
        if let Some(idx) = sidx {
            series_index.insert(*id, *idx);
        }
        if let Some(folder) = series_folder_of(content_dir, Path::new(path)) {
            let stem = derive_title(Path::new(path));
            groups
                .entry(folder)
                .or_default()
                .push((*id, stem, kind.clone()));
        }
    }

    let existing_series: Vec<(i64, Option<String>)> =
        sqlx::query_as("SELECT id, folder_path FROM series")
            .fetch_all(pool)
            .await?;
    let mut folder_to_sid: HashMap<String, i64> = existing_series
        .iter()
        .filter_map(|(id, fp)| fp.clone().map(|f| (f, *id)))
        .collect();
    let leaf_rows: Vec<LeafRowRaw> = sqlx::query_as(
        "SELECT item_id, series_id, number_sort, number_disp, volume, chapter FROM item_series_leaf",
    )
    .fetch_all(pool)
    .await?;
    let existing_leaf: HashMap<i64, LeafSnapshot> = leaf_rows
        .into_iter()
        .map(|(iid, sid, ns, nd, v, c)| (iid, (sid, ns, nd, v, c)))
        .collect();

    let desired_leaf: HashSet<i64> = groups.values().flatten().map(|(id, _, _)| *id).collect();

    let mut tx = pool.begin().await.context("begin series reconcile tx")?;

    for (folder, members) in &groups {
        let existing_sid = folder_to_sid.get(folder).copied();
        let sid = reconcile_one_series(
            &mut tx,
            folder,
            members,
            &series_index,
            existing_sid,
            &existing_leaf,
            now,
        )
        .await?;
        folder_to_sid.insert(folder.clone(), sid);
    }

    // Detach moved leaves before deleting empty series.
    for item_id in existing_leaf.keys().copied() {
        if !desired_leaf.contains(&item_id) {
            delete_leaf(&mut tx, item_id).await?;
        }
    }
    for (id, sid) in &current_series_id {
        if sid.is_some() && !desired_leaf.contains(id) {
            null_series_id(&mut tx, *id).await?;
        }
    }

    for (folder, sid) in &folder_to_sid {
        if !groups.contains_key(folder) {
            delete_series_and_neighbors(&mut tx, *sid).await?;
        }
    }

    sqlx::query(
        "UPDATE series SET sort_creator = \
             (SELECT MIN(i.sort_creator) FROM items i WHERE i.series_id = series.id)",
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await.context("commit series reconcile tx")?;
    Ok(groups.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::test_util::write_cbz;
    use crate::scanner::{reconcile_paths, scan};

    #[test]
    fn series_index_orders_books_over_filename_and_fallback() {
        let members = vec![
            (10, "KIZUMONOGATARI".to_string(), "books".to_string()),
            (11, "BAKEMONOGATARI Part 1".to_string(), "books".to_string()),
            (12, "NEKOMONOGATARI".to_string(), "books".to_string()),
        ];
        let idx = HashMap::from([(10, 4.0), (11, 1.0), (12, 5.0)]);
        let rows: HashMap<i64, f64> = compute_leaf_numbers(&members, &idx)
            .into_iter()
            .map(|(id, r)| (id, r.number_sort))
            .collect();
        assert_eq!(rows[&11], 1.0, "Bakemonogatari Part 1 first");
        assert_eq!(
            rows[&10], 4.0,
            "Kizumonogatari after Bake, not alphabetical"
        );
        assert_eq!(rows[&12], 5.0);
        assert!(compute_leaf_numbers(&members, &idx)
            .iter()
            .all(|(_, r)| r.number_disp.is_none()));
    }

    #[test]
    fn without_series_index_leaf_numbering_is_unchanged() {
        let members = vec![
            (1, "ChainsawMan v01".to_string(), "manga".to_string()),
            (2, "ChainsawMan v02".to_string(), "manga".to_string()),
            (3, "Extra".to_string(), "manga".to_string()),
        ];
        let empty = HashMap::new();
        let rows: HashMap<i64, f64> = compute_leaf_numbers(&members, &empty)
            .into_iter()
            .map(|(id, r)| (id, r.number_sort))
            .collect();
        assert_eq!(rows[&1], 1.0);
        assert_eq!(rows[&2], 2.0);
        assert_eq!(rows[&3], 3.0, "unmarked leaf appends past the max parsed");
    }

    #[test]
    fn series_folder_of_detects_subfolder_and_oneshot() {
        let root = Path::new("/content");
        assert_eq!(
            series_folder_of(root, Path::new("/content/manga/Attack on Titan/v01.cbz")).as_deref(),
            Some("manga/Attack on Titan")
        );
        assert_eq!(
            series_folder_of(
                root,
                Path::new("/content/manga/One Piece/East Blue/v001.cbz")
            )
            .as_deref(),
            Some("manga/One Piece")
        );
        assert_eq!(
            series_folder_of(root, Path::new("/content/manga/oneshot.cbz")),
            None
        );
        assert_eq!(series_folder_of(root, Path::new("/content/x.cbz")), None);
    }

    #[sqlx::test]
    async fn scan_detects_series_and_orders_leaves(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let s = root.join("manga").join("Attack on Titan");
        std::fs::create_dir_all(&s).unwrap();
        write_cbz(&s.join("Attack on Titan v01 (2012).cbz"), "v1");
        write_cbz(&s.join("Attack on Titan v02 (2012).cbz"), "v2");
        write_cbz(&s.join("Attack on Titan v03 (2013).cbz"), "v3");
        write_cbz(&root.join("manga").join("Loose Oneshot.cbz"), "one");

        let stats = scan(&pool, root).await.unwrap();
        assert_eq!(stats.series, 1);

        let series: Vec<(String, String, Option<String>)> =
            sqlx::query_as("SELECT title, kind, folder_path FROM series")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].0, "Attack on Titan", "title = folder basename");
        assert_eq!(series[0].1, "manga");
        assert_eq!(series[0].2.as_deref(), Some("manga/Attack on Titan"));

        let leaves: Vec<(f64, Option<String>)> = sqlx::query_as(
            "SELECT number_sort, number_disp FROM item_series_leaf ORDER BY number_sort",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            leaves.iter().map(|l| l.0).collect::<Vec<_>>(),
            vec![1.0, 2.0, 3.0],
            "leaves ordered by parsed volume"
        );
        assert_eq!(leaves[0].1.as_deref(), Some("Vol. 1"));

        let in_series: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM items WHERE series_id IS NOT NULL")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(in_series, 3);
        let oneshot_sid: Option<i64> =
            sqlx::query_scalar("SELECT series_id FROM items WHERE title LIKE 'Loose%'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            oneshot_sid, None,
            "the loose file is a first-class one-shot"
        );
        let leaf_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM item_series_leaf")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(leaf_count, 3);

        let s2 = scan(&pool, root).await.unwrap();
        assert_eq!(s2.series, 1);
        assert_eq!(s2.added, 0);
    }

    #[sqlx::test]
    async fn series_title_strips_folder_metadata(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let s = root
            .join("manga")
            .join("River Town (2016-2017) (Digital) (Library Copy)");
        std::fs::create_dir_all(&s).unwrap();
        write_cbz(&s.join("River Town v01.cbz"), "v1");
        write_cbz(&s.join("River Town v02.cbz"), "v2");

        scan(&pool, root).await.unwrap();

        let (title, folder): (String, Option<String>) =
            sqlx::query_as("SELECT title, folder_path FROM series")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(title, "River Town", "series title is the clean name");
        assert_eq!(
            folder.as_deref(),
            Some("manga/River Town (2016-2017) (Digital) (Library Copy)")
        );
    }

    #[sqlx::test]
    async fn nested_arc_subfolder_stays_one_series(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let op = root.join("manga").join("One Piece");
        std::fs::create_dir_all(op.join("East Blue")).unwrap();
        write_cbz(&op.join("East Blue").join("One Piece v001.cbz"), "1");
        write_cbz(&op.join("One Piece v002.cbz"), "2");

        let stats = scan(&pool, root).await.unwrap();
        assert_eq!(stats.series, 1, "arc subfolder does NOT split the series");

        let folder: Option<String> = sqlx::query_scalar("SELECT folder_path FROM series")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(folder.as_deref(), Some("manga/One Piece"));
        let sorts: Vec<f64> =
            sqlx::query_scalar("SELECT number_sort FROM item_series_leaf ORDER BY number_sort")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(sorts, vec![1.0, 2.0]);
    }

    #[sqlx::test]
    async fn positional_fallback_orders_unmarked_leaves(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let s = root.join("manga").join("Weird Series");
        std::fs::create_dir_all(&s).unwrap();
        write_cbz(&s.join("charlie.cbz"), "c");
        write_cbz(&s.join("alpha.cbz"), "a");
        write_cbz(&s.join("bravo.cbz"), "b");

        scan(&pool, root).await.unwrap();

        let leaves: Vec<(String, f64, Option<String>)> = sqlx::query_as(
            "SELECT i.title, l.number_sort, l.number_disp FROM item_series_leaf l \
             JOIN items i ON i.id = l.item_id ORDER BY l.number_sort",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            leaves.iter().map(|l| l.0.as_str()).collect::<Vec<_>>(),
            vec!["alpha", "bravo", "charlie"],
            "natural-sorted, not disk order"
        );
        assert_eq!(
            leaves.iter().map(|l| l.1).collect::<Vec<_>>(),
            vec![1.0, 2.0, 3.0]
        );
        assert!(
            leaves.iter().all(|l| l.2.is_none()),
            "purely positional → no display label"
        );
    }

    #[sqlx::test]
    async fn moving_last_leaf_out_removes_series(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let s = root.join("manga").join("Solo");
        std::fs::create_dir_all(&s).unwrap();
        write_cbz(&s.join("Solo v01.cbz"), "x");

        let stats = scan(&pool, root).await.unwrap();
        assert_eq!(stats.series, 1);
        let id: i64 = sqlx::query_scalar("SELECT id FROM items")
            .fetch_one(&pool)
            .await
            .unwrap();

        std::fs::rename(
            s.join("Solo v01.cbz"),
            root.join("manga").join("Solo v01.cbz"),
        )
        .unwrap();
        let stats = scan(&pool, root).await.unwrap();
        assert_eq!(stats.series, 0);

        let series_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM series")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(series_count, 0, "orphan series removed");
        let sid: Option<i64> = sqlx::query_scalar("SELECT series_id FROM items WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(sid, None, "the moved leaf is now a one-shot");
        let leaf_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM item_series_leaf")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(leaf_count, 0);
    }

    #[sqlx::test]
    async fn targeted_add_folds_new_volume_into_series(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let s = root.join("manga").join("AoT");
        std::fs::create_dir_all(&s).unwrap();
        write_cbz(&s.join("AoT v01.cbz"), "v1");
        scan(&pool, root).await.unwrap();
        let sid: i64 = sqlx::query_scalar("SELECT id FROM series")
            .fetch_one(&pool)
            .await
            .unwrap();

        let v2 = s.join("AoT v02.cbz");
        write_cbz(&v2, "v2");
        reconcile_paths(&pool, root, vec![v2.clone()])
            .await
            .unwrap();

        let series_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM series")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(series_count, 1, "joined the SAME series, not a new one");
        let sorts: Vec<f64> =
            sqlx::query_scalar("SELECT number_sort FROM item_series_leaf ORDER BY number_sort")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(sorts, vec![1.0, 2.0], "both volumes ordered");
        let in_series: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM items WHERE series_id = ?")
            .bind(sid)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(in_series, 2);
    }

    #[sqlx::test]
    async fn targeted_move_out_of_series_orphans_it(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let s = root.join("manga").join("Solo");
        std::fs::create_dir_all(&s).unwrap();
        let old = s.join("Solo v01.cbz");
        write_cbz(&old, "x");
        scan(&pool, root).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items")
            .fetch_one(&pool)
            .await
            .unwrap();

        let new = root.join("manga").join("Solo v01.cbz");
        std::fs::rename(&old, &new).unwrap();
        reconcile_paths(&pool, root, vec![old.clone(), new.clone()])
            .await
            .unwrap();

        let series_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM series")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            series_count, 0,
            "emptied series orphaned via the watcher path"
        );
        let sid: Option<i64> = sqlx::query_scalar("SELECT series_id FROM items WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(sid, None, "the moved leaf is now a one-shot");
        let leaf_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM item_series_leaf")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(leaf_count, 0);
    }
}
