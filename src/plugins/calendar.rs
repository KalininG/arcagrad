//! Upcoming-release calendar host. Publisher knowledge stays in plugins; this
//! module batches explicit `series_trackers`, validates results, and owns durable
//! replace-per-reference semantics.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use chrono::{Duration, Local, NaiveDate};
use serde::Serialize;
use sqlx::SqlitePool;
use utoipa::ToSchema;

use crate::plugins::scraper::{CalendarReference, CalendarRelease, CalendarRequest};
use crate::AppState;

const WINDOW_PAST_DAYS: i64 = 7;
const WINDOW_FUTURE_DAYS: i64 = 180;

fn now() -> i64 {
    crate::now_secs()
}

fn valid_release(release: &CalendarRelease, start: NaiveDate, end: NaiveDate) -> bool {
    !release.release_id.trim().is_empty()
        && !release.label.trim().is_empty()
        && NaiveDate::parse_from_str(&release.release_date, "%Y-%m-%d")
            .is_ok_and(|date| date >= start && date <= end)
}

/// Refresh installed calendar plugins while preserving rows for failed sources.
pub async fn refresh_all(state: &AppState) -> Result<(usize, usize)> {
    let today = Local::now().date_naive();
    let start = today - Duration::days(WINDOW_PAST_DAYS);
    let end = today + Duration::days(WINDOW_FUTURE_DAYS);
    let mut checked = 0usize;
    let mut stored = 0usize;

    for manifest in state.scrapers.manifests() {
        if !manifest.capabilities.iter().any(|c| c == "calendar") {
            continue;
        }
        let Some(plugin) = state.scrapers.get(&manifest.id) else {
            continue;
        };
        sqlx::query(
            "DELETE FROM series_upcoming WHERE provider = ? \
             AND NOT EXISTS (SELECT 1 FROM series_trackers st \
                 WHERE st.series_id = series_upcoming.series_id \
                   AND st.plugin_id = series_upcoming.provider)",
        )
        .bind(&manifest.id)
        .execute(&state.write)
        .await?;
        let links: Vec<(i64, String, String)> = sqlx::query_as(
            "SELECT st.series_id, st.reference, s.title FROM series_trackers st \
             JOIN series s ON s.id = st.series_id WHERE st.plugin_id = ? ORDER BY st.series_id",
        )
        .bind(&manifest.id)
        .fetch_all(&state.read)
        .await?;
        if links.is_empty() {
            continue;
        }

        let mut by_reference: HashMap<String, Vec<i64>> = HashMap::new();
        let refs: Vec<CalendarReference> = links
            .into_iter()
            .map(|(series_id, reference, title)| {
                by_reference
                    .entry(reference.clone())
                    .or_default()
                    .push(series_id);
                CalendarReference {
                    reference,
                    title: Some(title),
                }
            })
            .collect();
        let request = CalendarRequest {
            window_start: start.format("%Y-%m-%d").to_string(),
            window_end: end.format("%Y-%m-%d").to_string(),
            references: refs,
            market: Some("en-US".to_string()),
        };
        let response = match plugin.upcoming(&request, state.fetcher.as_ref()).await {
            Ok(response) => response,
            Err(error) => {
                let message = format!("{error:#}");
                tracing::warn!(plugin = %manifest.id, "calendar refresh failed: {error:#}");
                set_provider_state(&state.write, &manifest.id, &manifest.source, Some(&message))
                    .await?;
                continue;
            }
        };
        for e in &response.errors {
            tracing::warn!(plugin = %manifest.id, reference = %e.reference, "calendar reference error: {}", e.message);
        }
        let errored: HashSet<_> = response
            .errors
            .iter()
            .map(|e| e.reference.as_str())
            .collect();
        for result in response.results {
            if errored.contains(result.reference.as_str()) {
                continue;
            }
            let Some(series_ids) = by_reference.get(&result.reference) else {
                tracing::warn!(plugin = %manifest.id, reference = %result.reference, "calendar plugin returned an unknown reference");
                continue;
            };
            let releases: Vec<_> = result
                .releases
                .into_iter()
                .filter(|release| valid_release(release, start, end))
                .collect();
            for &series_id in series_ids {
                replace_reference(
                    &state.write,
                    series_id,
                    &manifest.id,
                    &manifest.source,
                    &releases,
                )
                .await?;
                stored += releases.len();
            }
            checked += series_ids.len();
        }
        set_provider_state(&state.write, &manifest.id, &manifest.source, None).await?;
    }
    Ok((checked, stored))
}

async fn replace_reference(
    pool: &SqlitePool,
    series_id: i64,
    provider: &str,
    source: &str,
    releases: &[CalendarRelease],
) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM series_upcoming WHERE series_id = ? AND provider = ?")
        .bind(series_id)
        .bind(provider)
        .execute(&mut *tx)
        .await?;
    let fetched_at = now();
    for release in releases {
        sqlx::query(
            "INSERT INTO series_upcoming \
             (series_id, provider, provider_release_id, reference_source, label, title, \
              release_date, date_precision, date_status, formats_json, media_type, market, \
              publisher, creators_json, isbn, url, cover_url, fetched_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(series_id)
        .bind(provider)
        .bind(&release.release_id)
        .bind(source)
        .bind(&release.label)
        .bind(&release.title)
        .bind(&release.release_date)
        .bind(&release.date_precision)
        .bind(&release.date_status)
        .bind(serde_json::to_string(&release.formats)?)
        .bind(&release.media_type)
        .bind(&release.market)
        .bind(&release.publisher)
        .bind(serde_json::to_string(&release.creators)?)
        .bind(&release.isbn)
        .bind(&release.url)
        .bind(&release.cover_url)
        .bind(fetched_at)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn set_provider_state(
    pool: &SqlitePool,
    provider: &str,
    source: &str,
    error: Option<&str>,
) -> Result<()> {
    let checked = now();
    sqlx::query(
        "INSERT INTO calendar_provider_state \
         (provider, source, last_checked_at, last_success_at, last_error) \
         VALUES (?, ?, ?, CASE WHEN ? IS NULL THEN ? END, ?) \
         ON CONFLICT(provider) DO UPDATE SET source = excluded.source, \
           last_checked_at = excluded.last_checked_at, \
           last_success_at = CASE WHEN excluded.last_error IS NULL THEN excluded.last_checked_at \
                                  ELSE calendar_provider_state.last_success_at END, \
           last_error = excluded.last_error",
    )
    .bind(provider)
    .bind(source)
    .bind(checked)
    .bind(error)
    .bind(checked)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UpcomingSource {
    pub id: String,
    pub name: String,
    pub source: String,
    pub linked_series: i64,
    pub status: String,
    pub last_checked_at: Option<i64>,
    pub last_success_at: Option<i64>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UpcomingRelease {
    pub id: String,
    pub series_id: i64,
    pub source: String,
    pub title: String,
    pub label: String,
    pub creators: Vec<String>,
    pub publisher: Option<String>,
    pub kind: String,
    pub formats: Vec<String>,
    pub market: Option<String>,
    pub date: String,
    pub date_precision: String,
    pub date_status: String,
    pub isbn: Option<String>,
    pub url: Option<String>,
    pub cover_url: Option<String>,
    pub cover_item_id: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UpcomingResponse {
    pub window_days: i64,
    pub generated_at: i64,
    pub next_refresh_at: Option<i64>,
    pub sources: Vec<UpcomingSource>,
    pub releases: Vec<UpcomingRelease>,
}

#[derive(sqlx::FromRow)]
struct UpcomingRow {
    row_id: i64,
    series_id: i64,
    provider: String,
    title: String,
    label: String,
    publisher: Option<String>,
    kind: String,
    formats: String,
    market: Option<String>,
    date: String,
    isbn: Option<String>,
    url: Option<String>,
    precision: String,
    status: String,
    creators: String,
    cover_url: Option<String>,
    cover_item_id: Option<i64>,
}

pub async fn list(state: &AppState) -> Result<UpcomingResponse> {
    let today = Local::now().date_naive();
    let start = (today - Duration::days(WINDOW_PAST_DAYS))
        .format("%Y-%m-%d")
        .to_string();
    let end = (today + Duration::days(WINDOW_FUTURE_DAYS))
        .format("%Y-%m-%d")
        .to_string();
    let mut sources = Vec::new();
    for manifest in state.scrapers.manifests() {
        if !manifest.capabilities.iter().any(|c| c == "calendar") {
            continue;
        }
        let linked_series: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM series_trackers WHERE plugin_id = ?")
                .bind(&manifest.id)
                .fetch_one(&state.read)
                .await?;
        let status: Option<(Option<i64>, Option<i64>, Option<String>)> = sqlx::query_as(
            "SELECT last_checked_at, last_success_at, last_error \
             FROM calendar_provider_state WHERE provider = ?",
        )
        .bind(&manifest.id)
        .fetch_optional(&state.read)
        .await?;
        let (last_checked_at, last_success_at, error) = status.unwrap_or((None, None, None));
        sources.push(UpcomingSource {
            id: manifest.id,
            name: manifest.name,
            source: manifest.source,
            linked_series: linked_series.0,
            status: if linked_series.0 == 0 {
                "inactive"
            } else if error.is_some() {
                "error"
            } else if last_checked_at.is_some() {
                "updated"
            } else {
                "pending"
            }
            .to_string(),
            last_checked_at,
            last_success_at,
            error,
        });
    }

    let rows: Vec<UpcomingRow> = sqlx::query_as(
        "SELECT u.id AS row_id, u.series_id, u.provider, \
                COALESCE(NULLIF(u.title, ''), s.title) AS title, u.label, u.publisher, \
                COALESCE(u.media_type, s.kind) AS kind, u.formats_json AS formats, \
                u.market, u.release_date AS date, u.isbn, u.url, \
                u.date_precision AS precision, u.date_status AS status, \
                u.creators_json AS creators, u.cover_url, \
                (SELECT l.item_id FROM item_series_leaf l WHERE l.series_id = s.id \
                 ORDER BY l.number_sort, l.item_id LIMIT 1) AS cover_item_id \
         FROM series_upcoming u JOIN series s ON s.id = u.series_id \
         WHERE u.release_date BETWEEN ? AND ? \
           AND EXISTS (SELECT 1 FROM series_trackers st \
               WHERE st.series_id = u.series_id AND st.plugin_id = u.provider) \
         ORDER BY u.release_date, u.id",
    )
    .bind(start)
    .bind(end)
    .fetch_all(&state.read)
    .await
    .context("list upcoming releases")?;
    let series_ids: Vec<i64> = rows.iter().map(|row| row.series_id).collect();
    let local_tags = crate::repo::series_tags_for_ids(&state.read, &series_ids).await?;
    let releases = rows
        .into_iter()
        .map(|row| {
            let UpcomingRow {
                row_id,
                series_id,
                provider,
                title,
                label,
                publisher,
                kind,
                formats,
                market,
                date,
                isbn,
                url,
                precision,
                status,
                creators,
                cover_url,
                cover_item_id,
            } = row;
            let plugin_creators: Vec<String> = serde_json::from_str(&creators).unwrap_or_default();
            let creators = if plugin_creators.is_empty() {
                local_tags
                    .get(&series_id)
                    .into_iter()
                    .flatten()
                    .filter(|tag| tag.namespace == "creator")
                    .map(|tag| tag.value.clone())
                    .collect()
            } else {
                plugin_creators
            };
            UpcomingRelease {
                id: format!("{provider}:{row_id}"),
                series_id,
                source: provider,
                title,
                label,
                creators,
                publisher,
                kind,
                formats: serde_json::from_str(&formats).unwrap_or_default(),
                market,
                date,
                date_precision: precision,
                date_status: status,
                isbn,
                url,
                cover_url,
                cover_item_id,
            }
        })
        .collect();
    let next_refresh_at = sqlx::query_as::<_, (i64,)>(
        "SELECT run_after FROM jobs WHERE kind = 'check_calendar' AND state = 'pending' \
           AND run_after > 0 \
         ORDER BY run_after LIMIT 1",
    )
    .fetch_optional(&state.read)
    .await?
    .map(|row| row.0);
    Ok(UpcomingResponse {
        window_days: WINDOW_FUTURE_DAYS,
        generated_at: now(),
        next_refresh_at,
        sources,
        releases,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::scraper::CalendarRelease;

    fn rel(release_id: &str, date: &str) -> CalendarRelease {
        CalendarRelease {
            release_id: release_id.into(),
            label: format!("Vol. {release_id}"),
            title: None,
            release_date: date.into(),
            date_precision: "day".into(),
            date_status: "announced".into(),
            formats: vec![],
            media_type: None,
            market: None,
            publisher: None,
            creators: vec![],
            isbn: None,
            url: None,
            cover_url: None,
        }
    }

    #[test]
    fn valid_release_enforces_window_and_nonempty() {
        let start = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let end = NaiveDate::from_ymd_opt(2026, 12, 31).unwrap();
        assert!(valid_release(&rel("a", "2026-06-15"), start, end));
        assert!(!valid_release(&rel("a", "2025-12-31"), start, end));
        assert!(!valid_release(&rel("a", "2027-01-01"), start, end));
        assert!(!valid_release(&rel("a", "soon"), start, end));
        let mut no_id = rel("", "2026-06-15");
        no_id.label = "Vol. 1".into();
        assert!(!valid_release(&no_id, start, end));
        let mut blank_label = rel("x", "2026-06-15");
        blank_label.label = "   ".into();
        assert!(!valid_release(&blank_label, start, end));
    }

    #[sqlx::test]
    async fn replace_reference_replaces_atomically(pool: SqlitePool) {
        let sid: i64 = sqlx::query_scalar(
            "INSERT INTO series (kind, title, folder_path, added_at) \
             VALUES ('manga','S','manga/S',0) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        replace_reference(
            &pool,
            sid,
            "viz",
            "viz",
            &[rel("r1", "2026-06-01"), rel("r2", "2026-07-01")],
        )
        .await
        .unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM series_upcoming WHERE series_id = ?")
            .bind(sid)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 2);

        replace_reference(&pool, sid, "viz", "viz", &[rel("r1", "2026-06-02")])
            .await
            .unwrap();
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT provider_release_id, release_date FROM series_upcoming WHERE series_id = ?",
        )
        .bind(sid)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            rows,
            vec![("r1".to_string(), "2026-06-02".to_string())],
            "old rows replaced, r2 dropped, r1's date corrected"
        );
    }

    #[sqlx::test]
    async fn provider_state_tracks_success_then_preserves_it_through_a_failure(pool: SqlitePool) {
        set_provider_state(&pool, "viz", "viz", None).await.unwrap();
        let (checked, success, err): (Option<i64>, Option<i64>, Option<String>) = sqlx::query_as(
            "SELECT last_checked_at, last_success_at, last_error \
             FROM calendar_provider_state WHERE provider = 'viz'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(checked.is_some() && success.is_some() && err.is_none());

        set_provider_state(&pool, "viz", "viz", Some("boom"))
            .await
            .unwrap();
        let (_c, success2, err2): (Option<i64>, Option<i64>, Option<String>) = sqlx::query_as(
            "SELECT last_checked_at, last_success_at, last_error \
             FROM calendar_provider_state WHERE provider = 'viz'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(err2.as_deref(), Some("boom"));
        assert_eq!(
            success2, success,
            "prior success time preserved through a failure"
        );
    }
}
