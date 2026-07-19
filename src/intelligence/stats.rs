//! Per-user reading statistics and daily activity tracking.

use anyhow::Result;
use serde::Serialize;
use sqlx::SqlitePool;
use utoipa::ToSchema;

/// Maximum entries in each top-tags list.
const TOP_N: usize = 12;

/// Seconds per UTC activity bucket.
pub const DAY: i64 = 86_400;

/// Completion predicate shared by the aggregate queries below.
const FINISHED: &str =
    "((rp.unit = 'page' AND i.page_count IS NOT NULL AND rp.value >= i.page_count - 1) \
                       OR (rp.unit = 'percent' AND rp.value >= 0.98))";

// ---- response types ----

#[derive(Serialize, ToSchema, Default)]
pub struct Stats {
    pub totals: Totals,
    /// Read counts by library kind.
    pub by_kind: Vec<KindStat>,
    pub ratings: Ratings,
    pub favorites: Favorites,
    pub series: SeriesStat,
    pub top: TopTags,
    /// Weighted content, demographic, and category tags.
    pub taste: Vec<TagStat>,
    /// Recently finished items, newest first.
    pub recent: Vec<RecentItem>,
    pub activity: Activity,
}

#[derive(Serialize, ToSchema, Default)]
pub struct Totals {
    pub started: i64,
    pub finished: i64,
    pub in_progress: i64,
    pub pages_read: i64,
    pub words_read: i64,
    pub comics_finished: i64,
    pub books_finished: i64,
}

#[derive(Serialize, ToSchema)]
pub struct KindStat {
    pub kind: String,
    pub started: i64,
    pub finished: i64,
}

#[derive(Serialize, ToSchema, Default)]
pub struct Ratings {
    pub count: i64,
    pub average: f64,
    pub distribution: [i64; 10],
}

#[derive(Serialize, ToSchema, Default)]
pub struct Favorites {
    pub items: i64,
    pub series: i64,
}

#[derive(Serialize, ToSchema, Default)]
pub struct SeriesStat {
    pub started: i64,
    pub finished: i64,
    pub items: Vec<SeriesProgress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub longest_completed: Option<SeriesProgress>,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct SeriesProgress {
    pub id: i64,
    pub title: String,
    pub read: i64,
    pub total: i64,
}

#[derive(Serialize, ToSchema)]
pub struct RecentItem {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub modality: String,
    pub cover_version: String,
    pub finished_at: i64,
}

#[derive(Serialize, ToSchema, Default)]
pub struct TopTags {
    pub creators: Vec<TagStat>,
    pub parodies: Vec<TagStat>,
}

#[derive(Serialize, ToSchema)]
pub struct TagStat {
    pub value: String,
    pub count: i64,
}

#[derive(Serialize, ToSchema, Default)]
pub struct Activity {
    /// Active UTC days, oldest first.
    pub days: Vec<DayStat>,
    pub active_days: i64,
    pub current_streak: i64,
    pub longest_streak: i64,
    pub total_pages: i64,
    pub pages_per_active_day: f64,
    /// First and last UTC activity sbuckets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_day: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_day: Option<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct DayStat {
    /// UTC day index (`updated_at / DAY`).
    pub day: i64,
    pub pages: i64,
    pub updates: i64,
}

// ---- activity writes ----

/// Records a progress update in its UTC day bucket.
pub async fn record_activity(
    pool: &SqlitePool,
    user_id: i64,
    now_secs: i64,
    pages_delta: i64,
) -> Result<()> {
    let day = now_secs / DAY;
    let pages = pages_delta.max(0);
    sqlx::query(
        "INSERT INTO reading_activity (user_id, day, pages, updates) VALUES (?, ?, ?, 1) \
         ON CONFLICT(user_id, day) DO UPDATE SET pages = pages + excluded.pages, updates = updates + 1",
    )
    .bind(user_id)
    .bind(day)
    .bind(pages)
    .execute(pool)
    .await?;
    Ok(())
}

// ---- aggregation ----

pub async fn collect(read: &SqlitePool, user_id: i64, today: i64) -> Result<Stats> {
    Ok(Stats {
        totals: totals(read, user_id).await?,
        by_kind: by_kind(read, user_id).await?,
        ratings: ratings(read, user_id).await?,
        favorites: favorites(read, user_id).await?,
        series: series(read, user_id).await?,
        top: top_tags(read, user_id).await?,
        taste: taste(read, user_id).await?,
        recent: recent(read, user_id).await?,
        activity: activity(read, user_id, today).await?,
    })
}

async fn totals(read: &SqlitePool, user_id: i64) -> Result<Totals> {
    let sql = format!(
        "SELECT \
           COUNT(*), \
           COALESCE(SUM(CASE WHEN {FINISHED} THEN 1 ELSE 0 END), 0), \
           CAST(COALESCE(SUM(CASE WHEN rp.unit = 'page' THEN rp.value + 1 ELSE 0 END), 0) AS INTEGER), \
           CAST(COALESCE(SUM(CASE WHEN rp.unit = 'percent' THEN COALESCE(i.word_count, 0) * rp.value ELSE 0 END), 0) AS REAL), \
           COALESCE(SUM(CASE WHEN {FINISHED} AND i.modality = 'reflowable' THEN 1 ELSE 0 END), 0) \
         FROM read_progress rp JOIN items i ON i.id = rp.item_id \
         WHERE rp.user_id = ?"
    );
    let (started, finished, pages_read, words_read, books_finished): (i64, i64, i64, f64, i64) =
        sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(user_id)
            .fetch_one(read)
            .await?;
    Ok(Totals {
        started,
        finished,
        in_progress: started - finished,
        pages_read,
        words_read: words_read as i64,
        comics_finished: finished - books_finished,
        books_finished,
    })
}

async fn by_kind(read: &SqlitePool, user_id: i64) -> Result<Vec<KindStat>> {
    let sql = format!(
        "SELECT i.kind, COUNT(*), COALESCE(SUM(CASE WHEN {FINISHED} THEN 1 ELSE 0 END), 0) \
         FROM read_progress rp JOIN items i ON i.id = rp.item_id \
         WHERE rp.user_id = ? GROUP BY i.kind ORDER BY COUNT(*) DESC, i.kind"
    );
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(user_id)
        .fetch_all(read)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(kind, started, finished)| KindStat {
            kind,
            started,
            finished,
        })
        .collect())
}

async fn ratings(read: &SqlitePool, user_id: i64) -> Result<Ratings> {
    let rows: Vec<(i64, i64)> =
        sqlx::query_as("SELECT value, COUNT(*) FROM ratings WHERE user_id = ? GROUP BY value")
            .bind(user_id)
            .fetch_all(read)
            .await?;
    let mut distribution = [0i64; 10];
    let (mut count, mut sum) = (0i64, 0i64);
    for (value, n) in rows {
        if (1..=10).contains(&value) {
            distribution[(value - 1) as usize] = n;
            count += n;
            sum += value * n;
        }
    }
    let average = if count > 0 {
        sum as f64 / count as f64
    } else {
        0.0
    };
    Ok(Ratings {
        count,
        average,
        distribution,
    })
}

async fn favorites(read: &SqlitePool, user_id: i64) -> Result<Favorites> {
    let items: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM favorites WHERE user_id = ?")
        .bind(user_id)
        .fetch_one(read)
        .await?;
    let series: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM series_favorites WHERE user_id = ?")
        .bind(user_id)
        .fetch_one(read)
        .await?;
    Ok(Favorites { items, series })
}

const SERIES_N: usize = 12;

async fn series(read: &SqlitePool, user_id: i64) -> Result<SeriesStat> {
    let sql = format!(
        "SELECT l.series_id, s.title, \
                (SELECT COUNT(*) FROM item_series_leaf ll WHERE ll.series_id = l.series_id) AS leaves, \
                COUNT(DISTINCT CASE WHEN {FINISHED} THEN rp.item_id END) AS finished_leaves, \
                MAX(rp.updated_at) AS last_read \
         FROM read_progress rp JOIN item_series_leaf l ON l.item_id = rp.item_id \
         JOIN items i ON i.id = rp.item_id \
         JOIN series s ON s.id = l.series_id \
         WHERE rp.user_id = ? GROUP BY l.series_id ORDER BY last_read DESC"
    );
    let rows: Vec<(i64, String, i64, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(user_id)
        .fetch_all(read)
        .await?;
    let started = rows.len() as i64;
    let finished = rows
        .iter()
        .filter(|(_, _, leaves, done, _)| *leaves > 0 && done >= leaves)
        .count() as i64;
    let longest_completed = rows
        .iter()
        .filter(|(_, _, leaves, done, _)| *leaves > 0 && done >= leaves)
        .max_by_key(|(_, _, leaves, _, _)| *leaves)
        .map(|(id, title, leaves, done, _)| SeriesProgress {
            id: *id,
            title: title.clone(),
            read: *done,
            total: *leaves,
        });
    let items = rows
        .iter()
        .take(SERIES_N)
        .map(|(id, title, leaves, done, _)| SeriesProgress {
            id: *id,
            title: title.clone(),
            read: *done,
            total: *leaves,
        })
        .collect();
    Ok(SeriesStat {
        started,
        finished,
        items,
        longest_completed,
    })
}

/// Builds the weighted tag cloud from favorited and finished items.
async fn taste(read: &SqlitePool, user_id: i64) -> Result<Vec<TagStat>> {
    let liked = crate::repo::liked_items(read, user_id).await?;
    if liked.is_empty() {
        return Ok(Vec::new());
    }
    let weight: std::collections::HashMap<i64, f64> = liked
        .iter()
        .map(|(id, fav, _)| {
            (
                *id,
                if *fav {
                    crate::intelligence::recommend::FAVORITE_WEIGHT as f64
                } else {
                    1.0
                },
            )
        })
        .collect();
    let ids: Vec<i64> = weight.keys().copied().collect();
    let placeholders = vec!["?"; ids.len()].join(", ");
    let sql = format!(
        "SELECT it.item_id, t.namespace, t.value FROM item_tags it JOIN tags t ON t.id = it.tag_id \
         WHERE it.item_id IN ({placeholders}) AND t.namespace IN ('tag', 'demographic', 'category')"
    );
    let mut q = sqlx::query_as::<_, (i64, String, String)>(sqlx::AssertSqlSafe(sql));
    for id in &ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(read).await?;
    let mut acc: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    for (item_id, _ns, value) in rows {
        if let Some(w) = weight.get(&item_id) {
            *acc.entry(value).or_default() += w;
        }
    }
    let mut out: Vec<TagStat> = acc
        .into_iter()
        .map(|(value, w)| TagStat {
            value,
            count: w.round() as i64,
        })
        .collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then(a.value.cmp(&b.value)));
    out.truncate(TOP_N);
    Ok(out)
}

#[derive(sqlx::FromRow)]
struct RecentRow {
    id: i64,
    title: String,
    kind: String,
    modality: String,
    structural_hash: String,
    updated_at: i64,
    number_disp: Option<String>,
    series_title: Option<String>,
}

async fn recent(read: &SqlitePool, user_id: i64) -> Result<Vec<RecentItem>> {
    let sql = format!(
        "SELECT i.id, i.title, i.kind, i.modality, i.structural_hash, rp.updated_at, \
                l.number_disp, s.title AS series_title \
         FROM read_progress rp JOIN items i ON i.id = rp.item_id \
         LEFT JOIN item_series_leaf l ON l.item_id = i.id \
         LEFT JOIN series s ON s.id = l.series_id \
         WHERE rp.user_id = ? AND {FINISHED} \
         ORDER BY rp.updated_at DESC LIMIT 12"
    );
    let rows: Vec<RecentRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(user_id)
        .fetch_all(read)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let name = match r.series_title.as_deref() {
                Some(series) if crate::media::series::leaf_belongs_to_series(&r.title, series) => {
                    match r.number_disp.as_deref() {
                        Some(nd) if !nd.trim().is_empty() => format!("{series} · {nd}"),
                        _ => r.title.clone(),
                    }
                }
                _ => r.title.clone(),
            };
            RecentItem {
                id: r.id,
                name,
                kind: r.kind,
                modality: r.modality,
                cover_version: r.structural_hash,
                finished_at: r.updated_at,
            }
        })
        .collect())
}

async fn top_tags(read: &SqlitePool, user_id: i64) -> Result<TopTags> {
    let sql = format!(
        "SELECT t.namespace, t.value, COUNT(DISTINCT rp.item_id) AS n \
         FROM read_progress rp \
         JOIN items i ON i.id = rp.item_id \
         JOIN item_tags it ON it.item_id = rp.item_id \
         JOIN tags t ON t.id = it.tag_id \
         WHERE rp.user_id = ? AND {FINISHED} AND t.namespace IN ('creator', 'parody') \
         GROUP BY t.namespace, t.value \
         ORDER BY t.namespace, n DESC, t.value"
    );
    let rows: Vec<(String, String, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(user_id)
        .fetch_all(read)
        .await?;
    let mut top = TopTags::default();
    for (namespace, value, count) in rows {
        let bucket = match namespace.as_str() {
            "creator" => &mut top.creators,
            "parody" => &mut top.parodies,
            _ => continue,
        };
        if bucket.len() < TOP_N {
            bucket.push(TagStat { value, count });
        }
    }
    Ok(top)
}

async fn activity(read: &SqlitePool, user_id: i64, today: i64) -> Result<Activity> {
    let rows: Vec<(i64, i64, i64)> = sqlx::query_as(
        "SELECT day, pages, updates FROM reading_activity WHERE user_id = ? ORDER BY day",
    )
    .bind(user_id)
    .fetch_all(read)
    .await?;
    Ok(activity_summary(&rows, today))
}

fn activity_summary(rows: &[(i64, i64, i64)], today: i64) -> Activity {
    let days: Vec<DayStat> = rows
        .iter()
        .map(|&(day, pages, updates)| DayStat {
            day,
            pages,
            updates,
        })
        .collect();
    let active_days = days.len() as i64;
    let total_pages: i64 = days.iter().map(|d| d.pages).sum();
    let first_day = days.first().map(|d| d.day);
    let last_day = days.last().map(|d| d.day);

    let mut longest = 0i64;
    let mut run = 0i64;
    let mut prev: Option<i64> = None;
    for d in &days {
        run = match prev {
            Some(p) if d.day == p + 1 => run + 1,
            _ => 1,
        };
        longest = longest.max(run);
        prev = Some(d.day);
    }

    // Yesterday is allowed while the current day is still in progress.
    let mut current = 0i64;
    if let Some(last) = last_day {
        if last >= today - 1 {
            let mut expect = last;
            for d in days.iter().rev() {
                if d.day == expect {
                    current += 1;
                    expect -= 1;
                } else if d.day < expect {
                    break;
                }
            }
        }
    }

    let pages_per_active_day = if active_days > 0 {
        total_pages as f64 / active_days as f64
    } else {
        0.0
    };
    Activity {
        days,
        active_days,
        current_streak: current,
        longest_streak: longest,
        total_pages,
        pages_per_active_day,
        first_day,
        last_day,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_summary_streaks_and_totals() {
        let today = 100;
        let rows = vec![(10, 5, 1), (11, 3, 2), (12, 8, 1), (20, 4, 1), (99, 2, 1)];
        let a = activity_summary(&rows, today);
        assert_eq!(a.active_days, 5);
        assert_eq!(a.total_pages, 22);
        assert_eq!(a.longest_streak, 3, "10-11-12");
        assert_eq!(a.current_streak, 1, "last active day is yesterday");
        assert_eq!(a.first_day, Some(10));
        assert_eq!(a.last_day, Some(99));
        assert!((a.pages_per_active_day - 22.0 / 5.0).abs() < 1e-9);
    }

    #[test]
    fn current_streak_zero_when_stale_and_counts_a_live_run() {
        let stale = activity_summary(&[(10, 1, 1), (11, 1, 1)], 100);
        assert_eq!(stale.current_streak, 0);
        assert_eq!(stale.longest_streak, 2);
        let live = activity_summary(&[(98, 1, 1), (99, 1, 1), (100, 1, 1)], 100);
        assert_eq!(live.current_streak, 3);
    }

    #[test]
    fn empty_activity_is_zeroed() {
        let a = activity_summary(&[], 100);
        assert_eq!(a.active_days, 0);
        assert_eq!(a.current_streak, 0);
        assert_eq!(a.longest_streak, 0);
        assert_eq!(a.first_day, None);
        assert_eq!(a.pages_per_active_day, 0.0);
    }

    async fn item(
        pool: &SqlitePool,
        hash: &str,
        kind: &str,
        modality: &str,
        page_count: Option<i64>,
        word_count: Option<i64>,
    ) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO items (scheme_tag, structural_hash, path, size_bytes, mtime, format, \
             title, kind, modality, page_count, word_count, added_at, last_modified_at) \
             VALUES ('zip-structural-v1', ?, ?, 1, 1, 'cbz', ?, ?, ?, ?, ?, 1, 0) RETURNING id",
        )
        .bind(hash)
        .bind(format!("/p/{hash}"))
        .bind(format!("title-{hash}"))
        .bind(kind)
        .bind(modality)
        .bind(page_count)
        .bind(word_count)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn tag(pool: &SqlitePool, item_id: i64, namespace: &str, value: &str) {
        let tid: i64 = sqlx::query_scalar(
            "INSERT INTO tags (namespace, value) VALUES (?, ?) \
             ON CONFLICT(namespace, value) DO UPDATE SET value = value RETURNING id",
        )
        .bind(namespace)
        .bind(value)
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO item_tags (item_id, tag_id) VALUES (?, ?)")
            .bind(item_id)
            .bind(tid)
            .execute(pool)
            .await
            .unwrap();
    }

    #[sqlx::test]
    async fn collect_aggregates_a_users_reading(pool: SqlitePool) {
        let uid: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, role, created_at) \
             VALUES ('u', 'x', 'user', 0) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let a = item(&pool, "a", "manga", "paginated", Some(10), None).await;
        let b = item(&pool, "b", "manga", "paginated", Some(20), None).await;
        let c = item(&pool, "c", "books", "reflowable", None, Some(1000)).await;
        tag(&pool, a, "creator", "Ken").await;
        tag(&pool, a, "tag", "action").await;
        tag(&pool, b, "creator", "Ken").await;
        tag(&pool, c, "creator", "Rin").await;

        crate::repo::set_progress(&pool, uid, a, 9).await.unwrap();
        crate::repo::set_progress(&pool, uid, b, 5).await.unwrap();
        crate::repo::set_reflowable_progress(&pool, uid, c, 1.0, None)
            .await
            .unwrap();

        sqlx::query(
            "INSERT INTO ratings (user_id, item_id, value, updated_at) VALUES (?, ?, 5, 0)",
        )
        .bind(uid)
        .bind(a)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO favorites (user_id, item_id, created_at) VALUES (?, ?, 0)")
            .bind(uid)
            .bind(a)
            .execute(&pool)
            .await
            .unwrap();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let s = collect(&pool, uid, now / DAY).await.unwrap();

        assert_eq!(s.totals.started, 3);
        assert_eq!(s.totals.finished, 2, "a + c");
        assert_eq!(s.totals.in_progress, 1);
        assert_eq!(s.totals.pages_read, 16, "(9+1) + (5+1)");
        assert_eq!(s.totals.words_read, 1000, "1000 × 1.0");
        assert_eq!(s.totals.comics_finished, 1, "a");
        assert_eq!(s.totals.books_finished, 1, "c");

        assert_eq!(s.ratings.count, 1);
        assert!((s.ratings.average - 5.0).abs() < 1e-9);
        assert_eq!(s.ratings.distribution, [0, 0, 0, 0, 1, 0, 0, 0, 0, 0]);
        assert_eq!(s.favorites.items, 1);

        assert_eq!(
            s.top
                .creators
                .iter()
                .find(|t| t.value == "Ken")
                .unwrap()
                .count,
            1
        );
        assert!(s.top.creators.iter().any(|t| t.value == "Rin"));

        let manga = s.by_kind.iter().find(|k| k.kind == "manga").unwrap();
        assert_eq!((manga.started, manga.finished), (2, 1));

        assert_eq!(s.activity.active_days, 1);
        assert_eq!(s.activity.total_pages, 14);
        assert_eq!(s.activity.current_streak, 1);
        assert_eq!(s.activity.days.len(), 1);
        assert_eq!(s.activity.days[0].updates, 3);

        let action = s.taste.iter().find(|t| t.value == "action").unwrap();
        assert_eq!(action.count, 2, "A is favorited, so 2×");
        assert!(
            !s.taste.iter().any(|t| t.value == "drama"),
            "B in-progress excluded"
        );
        assert!(
            !s.taste.iter().any(|t| t.value == "Ken"),
            "creator not in taste cloud"
        );

        assert_eq!(s.recent.len(), 2);
        let names: Vec<_> = s.recent.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"title-a") && names.contains(&"title-c"));
    }

    #[sqlx::test]
    async fn series_progress_lists_read_total_and_longest_completed(pool: SqlitePool) {
        let uid: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, role, created_at) \
             VALUES ('u', 'x', 'user', 0) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let mk_series = |title: &'static str| {
            let p = pool.clone();
            async move {
                sqlx::query_scalar::<_, i64>(
                    "INSERT INTO series (kind, title, folder_path, added_at) VALUES ('manga', ?, ?, 1) RETURNING id",
                )
                .bind(title).bind(format!("manga/{title}")).fetch_one(&p).await.unwrap()
            }
        };
        let leaf = |sid: i64, hash: &'static str, page: i64, at: i64| {
            let p = pool.clone();
            async move {
                let iid = item(&p, hash, "manga", "paginated", Some(10), None).await;
                sqlx::query("UPDATE items SET series_id = ? WHERE id = ?")
                    .bind(sid)
                    .bind(iid)
                    .execute(&p)
                    .await
                    .unwrap();
                sqlx::query("INSERT INTO item_series_leaf (item_id, series_id, number_sort) VALUES (?, ?, 1)").bind(iid).bind(sid).execute(&p).await.unwrap();
                crate::repo::set_progress(&p, uid, iid, page).await.unwrap();
                let _ = at;
            }
        };
        let foo = mk_series("Foo").await;
        let bar = mk_series("Bar").await;
        leaf(foo, "f1", 9, 1).await;
        leaf(foo, "f2", 9, 2).await;
        leaf(bar, "b1", 9, 3).await;
        leaf(bar, "b2", 3, 4).await;
        leaf(bar, "b3", 0, 5).await;

        let s = collect(&pool, uid, 0).await.unwrap();
        assert_eq!(s.series.started, 2);
        assert_eq!(s.series.finished, 1, "only Foo is complete");
        assert_eq!(s.series.longest_completed.as_ref().unwrap().title, "Foo");
        assert_eq!(s.series.longest_completed.as_ref().unwrap().total, 2);
        let bar_row = s.series.items.iter().find(|p| p.title == "Bar").unwrap();
        assert_eq!((bar_row.read, bar_row.total), (1, 3));
        let foo_row = s.series.items.iter().find(|p| p.title == "Foo").unwrap();
        assert_eq!((foo_row.read, foo_row.total), (2, 2));
    }
}
