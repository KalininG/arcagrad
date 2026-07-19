//! Persistent background job queue.

use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::intelligence::recommend;
use crate::media::library;
use crate::plugins::scraper;
use crate::{repo, scanner, AppState};

const IDLE_POLL: Duration = Duration::from_secs(1);

const MAX_ATTEMPTS: i64 = 5;

const DOWNLOAD_MAX_ATTEMPTS: i64 = 1;
const SCRAPE_MAX_ATTEMPTS: i64 = 1;

fn max_attempts(kind: &str) -> i64 {
    match kind {
        "scrape" | "scrape_series" => SCRAPE_MAX_ATTEMPTS,
        "download" => DOWNLOAD_MAX_ATTEMPTS,
        _ => MAX_ATTEMPTS,
    }
}

const JOB_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

const SETTLE_DELAY_SECS: i64 = 2;

const MAX_SETTLE_ATTEMPTS: i64 = 30;

#[derive(sqlx::FromRow)]
pub struct Job {
    pub id: i64,
    pub kind: String,
    pub payload: Option<String>,
    pub attempts: i64,
}

fn now() -> i64 {
    crate::now_secs()
}

pub async fn enqueue(pool: &SqlitePool, kind: &str, payload: Option<&str>) -> Result<i64> {
    let n = now();
    let id = sqlx::query(
        "INSERT INTO jobs (kind, payload, state, attempts, created_at, updated_at) \
         VALUES (?, ?, 'pending', 0, ?, ?)",
    )
    .bind(kind)
    .bind(payload)
    .bind(n)
    .bind(n)
    .execute(pool)
    .await?
    .last_insert_rowid();
    Ok(id)
}

const PLUGIN_UPDATE_HOUR: u32 = 3;

fn next_daily_run_in<Tz: chrono::TimeZone>(now: &chrono::DateTime<Tz>, hour: u32) -> i64 {
    let today = now
        .date_naive()
        .and_hms_opt(hour, 0, 0)
        .expect("valid wall-clock hour");
    let next = if now.naive_local() < today {
        today
    } else {
        today + chrono::Duration::days(1)
    };
    match next.and_local_timezone(now.timezone()) {
        chrono::LocalResult::Single(t) => t.timestamp(),
        chrono::LocalResult::Ambiguous(t, _) => t.timestamp(),
        chrono::LocalResult::None => now.timestamp() + 86_400,
    }
}

const FOLLOW_CHECK_HOUR: u32 = 3;
const CALENDAR_CHECK_HOUR: u32 = 3;

async fn schedule_daily(pool: &SqlitePool, kind: &str, hour: u32) -> Result<()> {
    let run_after = next_daily_run_in(&chrono::Local::now(), hour);
    let n = now();
    sqlx::query(
        "INSERT INTO jobs (kind, payload, state, attempts, run_after, created_at, updated_at) \
         SELECT ?, NULL, 'pending', 0, ?, ?, ? \
         WHERE NOT EXISTS (SELECT 1 FROM jobs WHERE kind = ? AND state = 'pending')",
    )
    .bind(kind)
    .bind(run_after)
    .bind(n)
    .bind(n)
    .bind(kind)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn schedule_plugin_update_check(pool: &SqlitePool) -> Result<()> {
    schedule_daily(pool, "plugin_update_check", PLUGIN_UPDATE_HOUR).await
}

pub async fn schedule_follow_check(pool: &SqlitePool) -> Result<()> {
    schedule_daily(pool, "check_follows", FOLLOW_CHECK_HOUR).await
}

pub async fn schedule_calendar_check(pool: &SqlitePool) -> Result<()> {
    schedule_daily(pool, "check_calendar", CALENDAR_CHECK_HOUR).await
}

/// Run derived indexing and metadata work after upload or download.
pub async fn enqueue_ingest_followup(state: &AppState, new_ids: &[i64]) -> Result<()> {
    for &id in new_ids {
        let row: Option<(String, String)> =
            sqlx::query_as("SELECT modality, path FROM items WHERE id = ?")
                .bind(id)
                .fetch_optional(&state.read)
                .await
                .unwrap_or(None);
        if let Some((modality, path)) = row {
            if modality == "reflowable" {
                if let Err(e) = crate::media::library::ingest_epub_metadata(
                    &state.write,
                    id,
                    std::path::Path::new(&path),
                )
                .await
                {
                    tracing::warn!("ingest follow-up: epub enrichment failed for item {id}: {e:#}");
                }
            } else if modality == "paginated" {
                if let Err(e) = crate::media::library::ingest_comicinfo_metadata(
                    &state.write,
                    id,
                    std::path::Path::new(&path),
                )
                .await
                {
                    tracing::warn!(
                        "ingest follow-up: comicinfo enrichment failed for item {id}: {e:#}"
                    );
                }
            }
        }
    }
    if let Err(e) = enqueue_coalesced(&state.write, "thumbnail_sweep", None).await {
        tracing::warn!("ingest follow-up: thumbnail sweep enqueue failed: {e:#}");
    }
    if !new_ids.is_empty() {
        if let Err(e) = enqueue_reindex_search(&state.write, new_ids, &[]).await {
            tracing::warn!("ingest follow-up: search reindex enqueue failed: {e:#}");
        }
    }
    state.clear_recommendation_caches();
    Ok(())
}

/// Queue targeted search upserts and removals.
pub async fn enqueue_reindex_search(
    pool: &SqlitePool,
    items: &[i64],
    remove: &[i64],
) -> Result<Option<i64>> {
    if items.is_empty() && remove.is_empty() {
        return Ok(None);
    }
    let payload = serde_json::json!({ "items": items, "remove": remove }).to_string();
    Ok(Some(enqueue(pool, "reindex_search", Some(&payload)).await?))
}

/// Queue a job unless the same kind is already pending.
/// A running job does not suppress a follow-up.
pub async fn enqueue_coalesced(
    pool: &SqlitePool,
    kind: &str,
    payload: Option<&str>,
) -> Result<Option<i64>> {
    let n = now();
    let res = sqlx::query(
        "INSERT INTO jobs (kind, payload, state, attempts, created_at, updated_at) \
         SELECT ?, ?, 'pending', 0, ?, ? \
         WHERE NOT EXISTS (SELECT 1 FROM jobs WHERE kind = ? AND state = 'pending')",
    )
    .bind(kind)
    .bind(payload)
    .bind(n)
    .bind(n)
    .bind(kind)
    .execute(pool)
    .await?;
    Ok((res.rows_affected() > 0).then(|| res.last_insert_rowid()))
}

pub async fn enqueue_calendar_refresh(pool: &SqlitePool) -> Result<()> {
    let pending: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM jobs WHERE kind = 'check_calendar' AND payload IS NOT NULL \
         AND state IN ('pending','running')",
    )
    .fetch_one(pool)
    .await?;
    if pending.0 == 0 {
        enqueue(pool, "check_calendar", Some("{\"manual\":true}")).await?;
    }
    Ok(())
}

/// Merge paths into the pending targeted scan, if one exists.
pub async fn enqueue_scan_targeted(
    pool: &SqlitePool,
    paths: &[String],
    settle: i64,
    run_after: i64,
) -> Result<Option<i64>> {
    #[derive(serde::Deserialize, Default)]
    struct P {
        #[serde(default)]
        paths: Vec<String>,
        #[serde(default)]
        settle: i64,
    }
    let n = now();
    let mut tx = pool.begin().await?;
    let pending: Option<(i64, Option<String>, i64)> = sqlx::query_as(
        "SELECT id, payload, run_after FROM jobs \
         WHERE kind = 'scan' AND state = 'pending' ORDER BY id LIMIT 1",
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some((id, payload, pending_run_after)) = pending else {
        let payload = serde_json::json!({ "paths": paths, "settle": settle }).to_string();
        let id = sqlx::query(
            "INSERT INTO jobs (kind, payload, state, attempts, run_after, created_at, updated_at) \
             VALUES ('scan', ?, 'pending', 0, ?, ?, ?)",
        )
        .bind(payload)
        .bind(run_after)
        .bind(n)
        .bind(n)
        .execute(&mut *tx)
        .await?
        .last_insert_rowid();
        tx.commit().await?;
        return Ok(Some(id));
    };
    let existing = payload
        .as_deref()
        .map(|p| serde_json::from_str::<P>(p).unwrap_or_default())
        .filter(|p| !p.paths.is_empty());
    let Some(mut existing) = existing else {
        tx.commit().await?;
        return Ok(None);
    };
    for p in paths {
        if !existing.paths.contains(p) {
            existing.paths.push(p.clone());
        }
    }
    let merged = serde_json::json!({
        "paths": existing.paths,
        "settle": existing.settle.min(settle),
    })
    .to_string();
    sqlx::query("UPDATE jobs SET payload = ?, run_after = ?, updated_at = ? WHERE id = ?")
        .bind(merged)
        .bind(pending_run_after.max(run_after))
        .bind(n)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(None)
}

/// Queue a full scan or upgrade a pending targeted scan.
pub async fn enqueue_full_scan(pool: &SqlitePool) -> Result<Option<i64>> {
    let n = now();
    let mut tx = pool.begin().await?;
    let pending: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM jobs WHERE kind = 'scan' AND state = 'pending' ORDER BY id LIMIT 1",
    )
    .fetch_optional(&mut *tx)
    .await?;
    let id = match pending {
        Some(id) => {
            sqlx::query(
                "UPDATE jobs SET payload = NULL, run_after = 0, updated_at = ? WHERE id = ?",
            )
            .bind(n)
            .bind(id)
            .execute(&mut *tx)
            .await?;
            None
        }
        None => {
            let id = sqlx::query(
                "INSERT INTO jobs (kind, payload, state, attempts, run_after, created_at, updated_at) \
                 VALUES ('scan', NULL, 'pending', 0, 0, ?, ?)",
            )
            .bind(n)
            .bind(n)
            .execute(&mut *tx)
            .await?
            .last_insert_rowid();
            Some(id)
        }
    };
    tx.commit().await?;
    Ok(id)
}

/// Requeue jobs interrupted by the previous process.
pub async fn reset_running(pool: &SqlitePool) -> Result<u64> {
    let res =
        sqlx::query("UPDATE jobs SET state = 'pending', updated_at = ? WHERE state = 'running'")
            .bind(now())
            .execute(pool)
            .await?;
    Ok(res.rows_affected())
}

/// Claim one job atomically. Scan jobs never run concurrently.
async fn claim_next(pool: &SqlitePool) -> Result<Option<Job>> {
    let n = now();
    let job: Option<Job> = sqlx::query_as(
        "UPDATE jobs SET state = 'running', attempts = attempts + 1, updated_at = ? \
         WHERE id = (SELECT id FROM jobs WHERE state = 'pending' AND run_after <= ? \
                     AND (kind <> 'scan' \
                          OR NOT EXISTS (SELECT 1 FROM jobs WHERE kind = 'scan' AND state = 'running')) \
                     ORDER BY id LIMIT 1) \
         RETURNING id, kind, payload, attempts",
    )
    .bind(n)
    .bind(n)
    .fetch_optional(pool)
    .await?;
    Ok(job)
}

async fn mark(pool: &SqlitePool, id: i64, state: &str, result: Option<&str>) -> Result<()> {
    sqlx::query("UPDATE jobs SET state = ?, result = ?, updated_at = ? WHERE id = ?")
        .bind(state)
        .bind(result)
        .bind(now())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct JobStatus {
    pub id: i64,
    pub kind: String,
    pub state: String,
    pub result: Option<String>,
}

pub async fn status(pool: &SqlitePool, id: i64) -> Result<Option<JobStatus>> {
    let row: Option<(i64, String, String, Option<String>)> =
        sqlx::query_as("SELECT id, kind, state, result FROM jobs WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(id, kind, state, result)| JobStatus {
        id,
        kind,
        state,
        result,
    }))
}

/// Wait for completion or timeout.
pub async fn wait_for_terminal(
    pool: &SqlitePool,
    id: i64,
    timeout: Duration,
) -> Result<Option<JobStatus>> {
    let start = tokio::time::Instant::now();
    loop {
        match status(pool, id).await? {
            Some(s) if s.state == "done" || s.state == "failed" => return Ok(Some(s)),
            None => return Ok(None),
            Some(_) => {}
        }
        if start.elapsed() >= timeout {
            return Ok(None);
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

async fn requeue(pool: &SqlitePool, id: i64, run_after: i64) -> Result<()> {
    sqlx::query("UPDATE jobs SET state = 'pending', run_after = ?, updated_at = ? WHERE id = ?")
        .bind(run_after)
        .bind(now())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

async fn prune_terminal(pool: &SqlitePool, retention: Duration) -> Result<u64> {
    let cutoff = now() - retention.as_secs() as i64;
    let res = sqlx::query("DELETE FROM jobs WHERE state IN ('done', 'failed') AND updated_at < ?")
        .bind(cutoff)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

#[derive(serde::Deserialize, Default)]
struct RecomputeNeighborsPayload {
    item: Option<i64>,
    #[serde(default)]
    items: Vec<i64>,
}

/// `None` means a full rebuild; malformed payloads remain errors.
fn recompute_neighbor_targets(payload: Option<&str>) -> serde_json::Result<Option<Vec<i64>>> {
    let Some(payload) = payload else {
        return Ok(None);
    };
    let parsed: RecomputeNeighborsPayload = serde_json::from_str(payload)?;
    let mut targets = parsed.items;
    targets.extend(parsed.item);
    targets.sort();
    targets.dedup();
    Ok(Some(targets))
}

pub async fn run_job(
    state: &AppState,
    job: &Job,
    cancel: &CancellationToken,
) -> Result<Option<String>> {
    match job.kind.as_str() {
        "scan" => {
            #[derive(serde::Deserialize)]
            struct ScanPayload {
                #[serde(default)]
                paths: Vec<String>,
                #[serde(default)]
                settle: i64,
            }
            let parsed = job
                .payload
                .as_deref()
                .and_then(|p| serde_json::from_str::<ScanPayload>(p).ok());
            let settle_attempt = parsed.as_ref().map(|s| s.settle).unwrap_or(0);
            let targeted: Vec<String> = parsed.map(|s| s.paths).unwrap_or_default();
            let full_scan = targeted.is_empty();
            let started = std::time::Instant::now();
            // Auto-scrape only items found by a targeted watcher scan.
            let mut auto_scrape_ids: Vec<(i64, String)> = Vec::new();
            let stats = if full_scan {
                scanner::scan(&state.write, &state.config.content_dir).await?
            } else {
                let paths = targeted.into_iter().map(std::path::PathBuf::from).collect();
                match scanner::reconcile_paths(&state.write, &state.config.content_dir, paths).await
                {
                    Ok(s) => {
                        auto_scrape_ids = s.added_ids.clone();
                        s
                    }
                    Err(e) => {
                        tracing::warn!(
                            "targeted reconcile failed ({e:#}); falling back to a full scan"
                        );
                        scanner::scan(&state.write, &state.config.content_dir).await?
                    }
                }
            };
            if let Err(e) = sqlx::query("PRAGMA optimize").execute(&state.write).await {
                tracing::debug!("PRAGMA optimize after scan failed (non-fatal): {e:#}");
            }
            if stats.added + stats.updated + stats.removed + stats.errored > 0 {
                tracing::info!(
                    added = stats.added,
                    updated = stats.updated,
                    removed = stats.removed,
                    errored = stats.errored,
                    deferred = stats.deferred.len(),
                    total = stats.total,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "scan job complete"
                );
            } else {
                tracing::debug!(
                    deferred = stats.deferred.len(),
                    total = stats.total,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "scan job complete (no changes)"
                );
            }
            if !stats.deferred.is_empty() {
                let attempt = settle_attempt + 1;
                if attempt <= MAX_SETTLE_ATTEMPTS {
                    match enqueue_scan_targeted(
                        &state.write,
                        &stats.deferred,
                        attempt,
                        now() + SETTLE_DELAY_SECS,
                    )
                    .await
                    {
                        Ok(Some(id)) => tracing::debug!(
                            "settle follow-up scan {id} queued for {} deferred path(s) (attempt {attempt})",
                            stats.deferred.len()
                        ),
                        Ok(None) => {}
                        Err(e) => tracing::warn!("failed to queue settle follow-up scan: {e:#}"),
                    }
                } else {
                    tracing::warn!(
                        "{} file(s) still incomplete after {MAX_SETTLE_ATTEMPTS} settle checks \
                         (stalled copy?); leaving them for the next change event or full scan: {:?}",
                        stats.deferred.len(),
                        stats.deferred
                    );
                }
            }
            for hash in &stats.removed_thumbs {
                crate::media::thumbnail::remove_item(&state.config.data_dir, hash).await;
            }
            if !stats.added_reflowable.is_empty() || !stats.added_paginated.is_empty() {
                let mut enriched: Vec<i64> = Vec::new();
                for (id, path) in &stats.added_reflowable {
                    match library::ingest_epub_metadata(
                        &state.write,
                        *id,
                        std::path::Path::new(path),
                    )
                    .await
                    {
                        Ok(n) if n > 0 => enriched.push(*id),
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!("epub metadata enrichment failed for item {id}: {e:#}")
                        }
                    }
                }
                for (id, path) in &stats.added_paginated {
                    match library::ingest_comicinfo_metadata(
                        &state.write,
                        *id,
                        std::path::Path::new(path),
                    )
                    .await
                    {
                        Ok(n) if n > 0 => enriched.push(*id),
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!("comicinfo enrichment failed for item {id}: {e:#}")
                        }
                    }
                }
                if !enriched.is_empty() {
                    tracing::info!(
                        count = enriched.len(),
                        "enriched offline metadata (epub opf / comicinfo tags)"
                    );
                    state.clear_recommendation_caches();
                    let payload = serde_json::json!({ "items": enriched }).to_string();
                    if let Err(e) =
                        enqueue(&state.write, "recompute_neighbors", Some(&payload)).await
                    {
                        tracing::warn!("failed to queue recompute after epub enrichment: {e:#}");
                    }
                    let leaf_map = repo::leaf_series_map(&state.read, &enriched).await?;
                    let entries: Vec<i64> = enriched
                        .iter()
                        .map(|id| leaf_map.get(id).map(|sid| -sid).unwrap_or(*id))
                        .collect();
                    let payload = serde_json::json!({ "entries": entries }).to_string();
                    if let Err(e) =
                        enqueue(&state.write, "recompute_entry_neighbors", Some(&payload)).await
                    {
                        tracing::warn!(
                            "failed to queue entry recompute after epub enrichment: {e:#}"
                        );
                    }
                }
            }
            if full_scan {
                match library::repair_epub_descriptions(&state.write).await {
                    Ok(0) => {}
                    Ok(count) => tracing::info!(count, "repaired epub descriptions"),
                    Err(e) => tracing::warn!("epub description repair failed: {e:#}"),
                }
            }
            if stats.added + stats.updated > 0 {
                enqueue(&state.write, "thumbnail_sweep", None).await?;
            }
            if stats.added + stats.updated + stats.removed > 0 {
                state.clear_recommendation_caches();
            } else if stats.moved > 0 {
                state.similar.clear();
                state.entry_corpus.clear();
                state.for_you.clear();
            }
            if !repo::recommendation_index_ready(&state.read, repo::ITEM_NEIGHBORS_INDEX).await? {
                enqueue_coalesced(&state.write, "recompute_neighbors", None).await?;
            }
            if !repo::recommendation_index_ready(&state.read, repo::ENTRY_NEIGHBORS_INDEX).await?
                || stats.added + stats.updated + stats.moved + stats.removed > 0
            {
                enqueue_coalesced(&state.write, "recompute_entry_neighbors", None).await?;
            }
            if !stats.moved_kind_changed.is_empty() || !stats.moved_leaf_status_changed.is_empty() {
                state.similar.clear();
                state.for_you.clear();
                let mut items = stats.moved_kind_changed.clone();
                items.extend(stats.moved_leaf_status_changed.iter().copied());
                items.sort();
                items.dedup();
                let payload = serde_json::json!({ "items": items }).to_string();
                enqueue(&state.write, "recompute_neighbors", Some(&payload)).await?;
            }
            if !auto_scrape_ids.is_empty() {
                let loaded: std::collections::HashSet<String> =
                    state.scrapers.ids().into_iter().collect();
                let mut by_kind: std::collections::HashMap<String, Vec<String>> =
                    std::collections::HashMap::new();
                let mut queued = 0usize;
                for (item_id, kind) in &auto_scrape_ids {
                    let plugins = match by_kind.get(kind) {
                        Some(p) => p,
                        None => {
                            let p = repo::auto_plugins_for_kind(&state.read, kind)
                                .await
                                .unwrap_or_default()
                                .into_iter()
                                .filter(|id| loaded.contains(id))
                                .collect();
                            by_kind.entry(kind.clone()).or_insert(p)
                        }
                    };
                    for source in plugins {
                        let payload =
                            serde_json::json!({ "item_id": item_id, "source": source }).to_string();
                        match enqueue(&state.write, "scrape", Some(&payload)).await {
                            Ok(_) => queued += 1,
                            Err(e) => tracing::warn!(
                                "failed to enqueue auto-scrape (item {item_id}, plugin {source}): {e:#}"
                            ),
                        }
                    }
                }
                if queued > 0 {
                    tracing::info!(queued, "auto-scrape jobs enqueued for watcher-added items");
                }
            }
            if state.search.needs_rebuild() {
                if let Err(e) = enqueue_coalesced(&state.write, "reindex_search", None).await {
                    tracing::warn!("failed to enqueue full search rebuild: {e:#}");
                }
            } else {
                let mut items: Vec<i64> = stats.added_ids.iter().map(|(id, _)| *id).collect();
                items.extend(stats.moved_kind_changed.iter().copied());
                if let Err(e) =
                    enqueue_reindex_search(&state.write, &items, &stats.removed_ids).await
                {
                    tracing::warn!("failed to enqueue search reindex: {e:#}");
                }
            }
            Ok(None)
        }
        "thumbnail_sweep" => {
            let failed = library::sweep_thumbnails(state, cancel).await?;
            if !failed.is_empty() {
                if let Err(e) =
                    enqueue_scan_targeted(&state.write, &failed, 1, now() + SETTLE_DELAY_SECS).await
                {
                    tracing::warn!("failed to queue re-verify scan after sweep failures: {e:#}");
                }
            }
            Ok(None)
        }
        "scrape" => {
            #[derive(serde::Deserialize)]
            struct ScrapePayload {
                item_id: i64,
                source: String,
                #[serde(default)]
                reference: Option<String>,
            }
            let raw = job
                .payload
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("scrape job missing payload"))?;
            let p: ScrapePayload = serde_json::from_str(raw)?;
            let s = state
                .scrapers
                .get(&p.source)
                .ok_or_else(|| anyhow::anyhow!("unknown scraper source '{}'", p.source))?;
            let meta = repo::item_meta(&state.read, p.item_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("scrape target item not found: {}", p.item_id))?;
            let hint = scraper::ScrapeHint {
                title: meta.search_title(),
                display_title: Some(meta.title.clone()),
                author: meta.sort_creator.clone(),
                modality: Some(meta.modality.clone()),
                page_count: meta.page_count,
                reference: p.reference,
            };
            let applied = scraper::run_scrape(
                &state.write,
                s.as_ref(),
                state.fetcher.as_ref(),
                p.item_id,
                &hint,
            )
            .await?;
            tracing::info!(source = %p.source, item_id = p.item_id, applied, "scrape job complete");
            if applied > 0 {
                after_item_tags_changed(state, p.item_id, true).await?;
            }
            Ok(Some(serde_json::json!({ "applied": applied }).to_string()))
        }
        "scrape_series" => {
            #[derive(serde::Deserialize)]
            struct ScrapeSeriesPayload {
                series_id: i64,
                source: String,
                #[serde(default)]
                reference: Option<String>,
            }
            let raw = job
                .payload
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("scrape_series job missing payload"))?;
            let p: ScrapeSeriesPayload = serde_json::from_str(raw)?;
            let s = state
                .scrapers
                .get(&p.source)
                .ok_or_else(|| anyhow::anyhow!("unknown scraper source '{}'", p.source))?;
            let (title, modality) = repo::series_scrape_hint_by_id(&state.read, p.series_id)
                .await?
                .ok_or_else(|| {
                    anyhow::anyhow!("scrape target series not found: {}", p.series_id)
                })?;
            let hint = scraper::ScrapeHint {
                title,
                display_title: None,
                author: None,
                modality,
                page_count: None,
                reference: p.reference,
            };
            let applied = scraper::run_scrape_series(
                &state.write,
                s.as_ref(),
                state.fetcher.as_ref(),
                p.series_id,
                &hint,
            )
            .await?;
            tracing::info!(source = %p.source, series_id = p.series_id, applied, "series scrape complete");
            if applied > 0 {
                enqueue_entry_recompute_for_series(state, p.series_id).await?;
            }
            Ok(Some(serde_json::json!({ "applied": applied }).to_string()))
        }
        "download" => {
            #[derive(serde::Deserialize)]
            struct DownloadPayload {
                source: String,
                reference: String,
                #[serde(default)]
                kind: Option<String>,
            }
            let raw = job
                .payload
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("download job missing payload"))?;
            let p: DownloadPayload = serde_json::from_str(raw)?;
            let s = state
                .scrapers
                .get(&p.source)
                .ok_or_else(|| anyhow::anyhow!("unknown scraper source '{}'", p.source))?;
            let kind = p.kind.as_deref().unwrap_or(scanner::DEFAULT_KIND);
            let outcome = scraper::run_download(
                &state.write,
                &state.read,
                &state.config.content_dir,
                s.as_ref(),
                state.fetcher.as_ref(),
                &p.reference,
                kind,
            )
            .await?;
            tracing::info!(
                source = %p.source, reference = %p.reference, id = outcome.id,
                created = outcome.created, applied = outcome.applied, "download job complete"
            );
            if outcome.created {
                enqueue_ingest_followup(state, &[outcome.id]).await?;
            }
            if outcome.applied > 0 {
                after_item_tags_changed(state, outcome.id, !outcome.created).await?;
            }
            Ok(Some(
                serde_json::json!({
                    "id": outcome.id,
                    "created": outcome.created,
                    "applied": outcome.applied
                })
                .to_string(),
            ))
        }
        "recompute_neighbors" => {
            // An absent payload is the only full-rebuild signal.
            let targets = match recompute_neighbor_targets(job.payload.as_deref()) {
                Ok(targets) => targets,
                Err(e) => {
                    tracing::warn!(
                        "recompute_neighbors: unparseable payload {:?}: {e}",
                        job.payload
                    );
                    return Ok(Some(serde_json::json!({ "items": 0 }).to_string()));
                }
            };
            let started = std::time::Instant::now();
            let corpus = match state.corpus.get() {
                Some(c) => c,
                None => {
                    let c = std::sync::Arc::new(repo::build_corpus(&state.read).await?);
                    state.corpus.set(c.clone());
                    c
                }
            };
            let written = match targets {
                None => {
                    repo::invalidate_recommendation_index(&state.write, repo::ITEM_NEIGHBORS_INDEX)
                        .await?;
                    let w =
                        recompute_all(&state.read, &state.write, &state.similar, &corpus, cancel)
                            .await?;
                    if !cancel.is_cancelled() {
                        repo::mark_recommendation_index_ready(
                            &state.write,
                            repo::ITEM_NEIGHBORS_INDEX,
                        )
                        .await?;
                    }
                    tracing::info!(
                        items = w,
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        "recommendation table built"
                    );
                    w
                }
                Some(targets) => {
                    let mut total = 0;
                    for &item_id in &targets {
                        total += recompute_item(
                            &state.read,
                            &state.write,
                            &state.similar,
                            &corpus,
                            item_id,
                        )
                        .await?;
                    }
                    tracing::info!(
                        targeted = targets.len(),
                        items = total,
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        "recommendations updated"
                    );
                    total
                }
            };
            state.for_you.clear();
            Ok(Some(serde_json::json!({ "items": written }).to_string()))
        }
        "recompute_entry_neighbors" => {
            #[derive(serde::Deserialize, Default)]
            struct Payload {
                #[serde(default)]
                entries: Vec<i64>,
            }
            // Invalid payloads must not become full rebuilds.
            let targets: Option<Vec<i64>> = match job.payload.as_deref() {
                None => None,
                Some(p) => match serde_json::from_str::<Payload>(p) {
                    Ok(pl) => {
                        let mut v = pl.entries;
                        v.sort();
                        v.dedup();
                        Some(v)
                    }
                    Err(e) => {
                        tracing::warn!("recompute_entry_neighbors: unparseable payload {p:?}: {e}");
                        Some(Vec::new())
                    }
                },
            };
            let started = std::time::Instant::now();
            let corpus = match state.entry_corpus.get() {
                Some(c) => c,
                None => {
                    let c = std::sync::Arc::new(repo::build_entry_corpus(&state.read).await?);
                    state.entry_corpus.set(c.clone());
                    c
                }
            };
            let written = match targets {
                None => {
                    repo::invalidate_recommendation_index(
                        &state.write,
                        repo::ENTRY_NEIGHBORS_INDEX,
                    )
                    .await?;
                    let w = recompute_entry_all(&state.read, &state.write, &corpus, cancel).await?;
                    if !cancel.is_cancelled() {
                        repo::mark_recommendation_index_ready(
                            &state.write,
                            repo::ENTRY_NEIGHBORS_INDEX,
                        )
                        .await?;
                    }
                    tracing::info!(
                        entries = w,
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        "entry recommendation table built"
                    );
                    w
                }
                Some(targets) => {
                    let mut total = 0usize;
                    for key in &targets {
                        total += recompute_entry(&state.read, &state.write, &corpus, *key).await?;
                    }
                    tracing::info!(
                        targeted = targets.len(),
                        entries = total,
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        "entry recommendations updated"
                    );
                    total
                }
            };
            if written > 0 {
                state.for_you.clear();
            }
            Ok(Some(serde_json::json!({ "entries": written }).to_string()))
        }
        "reindex_search" => {
            #[derive(serde::Deserialize, Default)]
            struct Payload {
                #[serde(default)]
                items: Vec<i64>,
                #[serde(default)]
                remove: Vec<i64>,
            }
            let payload = job
                .payload
                .as_deref()
                .map(serde_json::from_str::<Payload>)
                .transpose()
                .context("parse reindex_search payload")?;
            let started = std::time::Instant::now();
            match payload {
                None => {
                    let n = state.search.rebuild_from_db(&state.read).await?;
                    tracing::info!(
                        docs = n,
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        "search index rebuilt"
                    );
                    Ok(Some(serde_json::json!({ "rebuilt": n }).to_string()))
                }
                Some(p) => {
                    for &id in &p.items {
                        state.search.reindex_item(&state.read, id).await?;
                    }
                    for &id in &p.remove {
                        state.search.delete_item(id);
                    }
                    if !p.items.is_empty() || !p.remove.is_empty() {
                        let search = state.search.clone();
                        tokio::task::spawn_blocking(move || search.commit())
                            .await
                            .context("join search commit")??;
                    }
                    tracing::debug!(
                        upserts = p.items.len(),
                        removes = p.remove.len(),
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        "search index updated"
                    );
                    Ok(Some(
                        serde_json::json!({ "upserts": p.items.len(), "removes": p.remove.len() })
                            .to_string(),
                    ))
                }
            }
        }
        "plugin_update_check" => {
            crate::plugins::marketplace::refresh_all(state).await;
            schedule_plugin_update_check(&state.write).await?;
            Ok(None)
        }
        "check_follows" => {
            #[derive(serde::Deserialize, Default)]
            struct Payload {
                #[serde(default)]
                follows: Vec<i64>,
            }
            let payload = job
                .payload
                .as_deref()
                .map(serde_json::from_str::<Payload>)
                .transpose()
                .context("parse check_follows payload")?;
            let targeted = payload.as_ref().is_some_and(|p| !p.follows.is_empty());
            let all = repo::list_follows(&state.read).await?;
            let targets: Vec<_> = match &payload {
                Some(p) if !p.follows.is_empty() => all
                    .into_iter()
                    .filter(|w| p.follows.contains(&w.id))
                    .collect(),
                _ => all,
            };
            let (mut checked, mut discovered) = (0usize, 0usize);
            for w in &targets {
                let Some(scraper) = state.scrapers.get(&w.plugin_id) else {
                    continue;
                };
                match check_follow(state, scraper.as_ref(), w).await {
                    Ok(n) => {
                        discovered += n;
                        repo::set_follow_checked(&state.write, w.id, None).await?;
                    }
                    Err(e) => {
                        tracing::warn!(follow = w.id, plugin = %w.plugin_id, "follow check failed: {e:#}");
                        repo::set_follow_checked(&state.write, w.id, Some(&format!("{e:#}")))
                            .await?;
                    }
                }
                checked += 1;
            }
            if !targeted {
                schedule_follow_check(&state.write).await?;
            }
            Ok(Some(
                serde_json::json!({ "checked": checked, "new": discovered }).to_string(),
            ))
        }
        "check_calendar" => {
            let (checked, releases) = crate::plugins::calendar::refresh_all(state).await?;
            schedule_calendar_check(&state.write).await?;
            Ok(Some(
                serde_json::json!({ "checked": checked, "releases": releases }).to_string(),
            ))
        }
        other => anyhow::bail!("unknown job kind '{other}'"),
    }
}

async fn check_follow(
    state: &AppState,
    scraper: &dyn crate::plugins::scraper::MetadataScraper,
    w: &repo::Follow,
) -> Result<usize> {
    const MAX_PAGES: u32 = 3;
    let seen = repo::follow_seen_references(&state.read, w.id).await?;
    let baseline = seen.is_empty();
    let mut rows: Vec<(String, &str, Option<String>)> = Vec::new();
    let mut discovered = 0usize;

    // The first check records a baseline without surfacing old entries.
    let last_page = if baseline { 1 } else { MAX_PAGES };
    for page in 1..=last_page {
        let req = crate::plugins::scraper::BrowseRequest {
            feed: w.feed.clone(),
            query: Some(w.query.clone()).filter(|q| !q.is_empty()),
            range: None,
            page,
        };
        let bp = scraper.browse(&req, &*state.fetcher).await?;
        if bp.items.is_empty() {
            break;
        }
        let urls: Vec<String> = bp
            .items
            .iter()
            .filter_map(|i| i.source_url.clone())
            .collect();
        let owned = repo::item_ids_by_source_urls(&state.read, &urls).await?;
        let mut page_had_known = false;
        for item in &bp.items {
            if seen.contains(&item.reference) {
                page_had_known = true;
                continue;
            }
            if baseline {
                rows.push((item.reference.clone(), "seen", None));
                continue;
            }
            let is_owned = item
                .source_url
                .as_ref()
                .is_some_and(|u| owned.contains_key(u));
            let state_s = if is_owned { "owned" } else { "new" };
            discovered += (state_s == "new") as usize;
            rows.push((
                item.reference.clone(),
                state_s,
                Some(serde_json::to_string(item)?),
            ));
        }
        if page_had_known {
            break;
        }
    }
    repo::insert_follow_seen(&state.write, w.id, &rows).await?;
    Ok(discovered)
}

async fn recompute_all(
    read: &sqlx::SqlitePool,
    write: &sqlx::SqlitePool,
    similar: &recommend::RecommendationCache<i64>,
    corpus: &recommend::Corpus,
    cancel: &CancellationToken,
) -> Result<usize> {
    repo::clear_item_neighbor_sources(write).await?;
    let items = repo::tagged_series_leaf_ids(read).await?;
    tracing::info!(items = items.len(), "building item recommendation table");
    let mut written = 0usize;
    for chunk in items.chunks(SWEEP_CHUNK) {
        if cancel.is_cancelled() {
            break;
        }
        let computed = repo::neighbors_of_batch(read, corpus, chunk).await?;
        repo::write_neighbors_batch(write, &computed).await?;
        written += chunk.len();
        tokio::task::yield_now().await;
    }
    similar.clear();
    Ok(written)
}

const SWEEP_CHUNK: usize = 256;

/// Refresh an item, its current tag sharers, and existing neighbour holders.
async fn recompute_item(
    read: &sqlx::SqlitePool,
    write: &sqlx::SqlitePool,
    similar: &recommend::RecommendationCache<i64>,
    corpus: &recommend::Corpus,
    item_id: i64,
) -> Result<usize> {
    if !repo::item_exists(read, item_id).await? {
        return Ok(0);
    }

    let mut candidates: std::collections::HashSet<i64> = std::collections::HashSet::new();
    candidates.insert(item_id);
    let tag_ids = repo::item_tag_ids(read, item_id).await?;
    let distinctive = corpus.distinctive(&tag_ids);
    if !distinctive.is_empty() {
        if let Some(kind) = repo::item_kind_by_id(read, item_id).await? {
            candidates.extend(
                repo::similar_candidates(
                    read,
                    item_id,
                    &distinctive,
                    &kind,
                    recommend::CANDIDATE_LIMIT,
                )
                .await?,
            );
        }
    }
    candidates.extend(repo::neighbor_holders(read, item_id).await?);

    let ids: Vec<i64> =
        repo::series_leaf_ids(read, &candidates.iter().copied().collect::<Vec<_>>())
            .await?
            .into_iter()
            .collect();
    if !ids.contains(&item_id) {
        repo::clear_item_neighbor_source(write, item_id).await?;
    }

    let computed = repo::neighbors_of_batch(read, corpus, &ids).await?;
    repo::write_neighbors_batch(write, &computed).await?;
    similar.clear();
    Ok(ids.len())
}

async fn recompute_entry_all(
    read: &sqlx::SqlitePool,
    write: &sqlx::SqlitePool,
    corpus: &recommend::Corpus,
    cancel: &CancellationToken,
) -> Result<usize> {
    repo::clear_entry_neighbor_sources(write).await?;
    let keys = repo::tagged_entries(read).await?;
    let mut written = 0usize;
    for chunk in keys.chunks(SWEEP_CHUNK) {
        if cancel.is_cancelled() {
            break;
        }
        let computed = repo::entry_neighbors_of_batch(read, corpus, chunk).await?;
        repo::write_entry_neighbors_batch(write, &computed).await?;
        written += chunk.len();
        tokio::task::yield_now().await;
    }
    Ok(written)
}

async fn recompute_entry(
    read: &sqlx::SqlitePool,
    write: &sqlx::SqlitePool,
    corpus: &recommend::Corpus,
    key: i64,
) -> Result<usize> {
    let mut targets: std::collections::HashSet<i64> = std::collections::HashSet::new();
    targets.insert(key);
    let vecs = repo::entry_tag_vectors(read, &[key]).await?;
    let tag_ids = vecs.get(&key).cloned().unwrap_or_default();
    let distinctive = corpus.distinctive(&tag_ids);
    if !distinctive.is_empty() {
        if let Some(kind) = repo::entry_kind(read, key).await? {
            targets.extend(
                repo::similar_entry_candidates(
                    read,
                    key,
                    &distinctive,
                    &kind,
                    recommend::CANDIDATE_LIMIT,
                )
                .await?,
            );
        }
    }
    targets.extend(repo::entry_neighbor_holders(read, key).await?);

    let ids: Vec<i64> = targets.into_iter().collect();
    let computed = repo::entry_neighbors_of_batch(read, corpus, &ids).await?;
    repo::write_entry_neighbors_batch(write, &computed).await?;
    Ok(ids.len())
}

pub(crate) async fn enqueue_entry_recompute_for_item(state: &AppState, item_id: i64) -> Result<()> {
    let map = repo::leaf_series_map(&state.read, &[item_id]).await?;
    let key = map.get(&item_id).map(|&s| -s).unwrap_or(item_id);
    let payload = serde_json::json!({ "entries": [key] }).to_string();
    enqueue(&state.write, "recompute_entry_neighbors", Some(&payload)).await?;
    Ok(())
}

pub(crate) async fn enqueue_entry_recompute_for_series(
    state: &AppState,
    series_id: i64,
) -> Result<()> {
    state.entry_corpus.clear();
    state.for_you.clear();
    let payload = serde_json::json!({ "entries": [repo::entry_key_series(series_id)] }).to_string();
    enqueue(&state.write, "recompute_entry_neighbors", Some(&payload)).await?;
    Ok(())
}

pub(crate) async fn after_item_tags_changed(
    state: &AppState,
    item_id: i64,
    reindex_fts: bool,
) -> Result<()> {
    state.clear_recommendation_caches();
    let payload = serde_json::json!({ "item": item_id }).to_string();
    enqueue(&state.write, "recompute_neighbors", Some(&payload)).await?;
    enqueue_entry_recompute_for_item(state, item_id).await?;
    if reindex_fts {
        let _ = enqueue_reindex_search(&state.write, &[item_id], &[]).await;
    }
    Ok(())
}

async fn worker_loop(state: AppState, cancel: CancellationToken) {
    loop {
        if cancel.is_cancelled() {
            break;
        }
        match claim_next(&state.write).await {
            Ok(Some(job)) => {
                let (id, kind) = (job.id, job.kind.clone());
                let max = max_attempts(&kind);
                let result = match run_job(&state, &job, &cancel).await {
                    Ok(outcome) => mark(&state.write, id, "done", outcome.as_deref()).await,
                    // Retry transient local work with bounded exponential backoff.
                    Err(e) if job.attempts < max => {
                        let backoff = 2i64.saturating_pow(job.attempts.clamp(1, 6) as u32).min(60);
                        tracing::warn!(
                            "job {id} ({kind}) failed (attempt {}/{max}), retrying in {backoff}s: {e:#}",
                            job.attempts
                        );
                        requeue(&state.write, id, now() + backoff).await
                    }
                    Err(e) => {
                        tracing::error!(
                            "job {id} ({kind}) failed permanently after {} attempts: {e:#}",
                            job.attempts
                        );
                        let err = serde_json::json!({ "error": format!("{e:#}") }).to_string();
                        mark(&state.write, id, "failed", Some(&err)).await
                    }
                };
                if let Err(e) = result {
                    tracing::error!("could not update job {id} state: {e:#}");
                }
                if let Err(e) = prune_terminal(&state.write, JOB_RETENTION).await {
                    tracing::warn!("prune jobs failed: {e:#}");
                }
            }
            Ok(None) => {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tokio::time::sleep(IDLE_POLL) => {}
                }
            }
            Err(e) => {
                tracing::error!("claim job failed: {e:#}");
                tokio::time::sleep(IDLE_POLL).await;
            }
        }
    }
}

/// Start workers that finish their current job after cancellation.
pub fn spawn_workers(state: AppState, n: usize, cancel: CancellationToken) -> TaskTracker {
    let tracker = TaskTracker::new();
    for _ in 0..n {
        tracker.spawn(worker_loop(state.clone(), cancel.clone()));
    }
    tracker.close();
    tracker
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recompute_neighbor_payload_distinguishes_full_targeted_and_invalid() {
        assert_eq!(
            recompute_neighbor_targets(None).unwrap(),
            None,
            "only no payload requests a full rebuild"
        );
        assert_eq!(
            recompute_neighbor_targets(Some(r#"{"item": 3, "items": [2, 3, 1]}"#)).unwrap(),
            Some(vec![1, 2, 3]),
            "target ids are combined, sorted, and deduplicated"
        );
        assert_eq!(
            recompute_neighbor_targets(Some("{}")).unwrap(),
            Some(Vec::new()),
            "an explicit empty payload is a targeted no-op, not a full rebuild"
        );
        assert!(
            recompute_neighbor_targets(Some("not json")).is_err(),
            "malformed input stays distinguishable from no payload"
        );
    }

    #[sqlx::test]
    async fn enqueue_claim_complete_is_fifo(pool: SqlitePool) {
        let a = enqueue(&pool, "scan", None).await.unwrap();
        let b = enqueue(&pool, "thumbnail_sweep", None).await.unwrap();

        let first = claim_next(&pool).await.unwrap().unwrap();
        assert_eq!(first.id, a);
        assert_eq!(first.kind, "scan");

        let second = claim_next(&pool).await.unwrap().unwrap();
        assert_eq!(second.id, b);
        assert!(claim_next(&pool).await.unwrap().is_none());

        mark(&pool, a, "done", None).await.unwrap();
        mark(&pool, b, "done", None).await.unwrap();
        assert!(claim_next(&pool).await.unwrap().is_none());
    }

    #[sqlx::test]
    async fn scan_jobs_are_serialized_other_kinds_are_not(pool: SqlitePool) {
        let s1 = enqueue(&pool, "scan", None).await.unwrap();
        let scrape = enqueue(&pool, "scrape", Some("{}")).await.unwrap();
        let s2 = enqueue(&pool, "scan", None).await.unwrap();

        let first = claim_next(&pool).await.unwrap().unwrap();
        assert_eq!(first.id, s1);
        assert_eq!(first.kind, "scan");

        let second = claim_next(&pool).await.unwrap().unwrap();
        assert_eq!(
            second.id, scrape,
            "a running scan blocks the next scan but not a scrape"
        );
        assert!(
            claim_next(&pool).await.unwrap().is_none(),
            "the second scan stays blocked while the first scan runs"
        );

        mark(&pool, s1, "done", None).await.unwrap();
        let third = claim_next(&pool).await.unwrap().unwrap();
        assert_eq!(third.id, s2, "the blocked scan is now claimable");
    }

    #[test]
    fn scrape_jobs_have_a_lower_retry_budget() {
        assert_eq!(
            max_attempts("scrape"),
            1,
            "scrape tries exactly once, no retry"
        );
        assert_eq!(
            max_attempts("scrape_series"),
            1,
            "scrape_series tries exactly once, no retry"
        );
        assert_eq!(
            max_attempts("download"),
            1,
            "downloads never auto-retry — a failure surfaces immediately"
        );
        assert_eq!(max_attempts("scan"), 5);
        assert_eq!(max_attempts("recompute_neighbors"), 5);
        assert_eq!(max_attempts("thumbnail_sweep"), 5);
    }

    #[sqlx::test]
    async fn status_reports_state_and_result(pool: SqlitePool) {
        let id = enqueue(&pool, "scrape", Some(r#"{"x":1}"#)).await.unwrap();
        let s = status(&pool, id).await.unwrap().unwrap();
        assert_eq!(s.state, "pending");
        assert!(s.result.is_none());

        mark(&pool, id, "done", Some(r#"{"applied":12}"#))
            .await
            .unwrap();
        let s = status(&pool, id).await.unwrap().unwrap();
        assert_eq!(s.state, "done");
        assert_eq!(s.result.as_deref(), Some(r#"{"applied":12}"#));

        assert!(status(&pool, 999_999).await.unwrap().is_none());
    }

    #[sqlx::test]
    async fn wait_for_terminal_resolves_and_times_out(pool: SqlitePool) {
        let id = enqueue(&pool, "scan", None).await.unwrap();

        let timed_out = wait_for_terminal(&pool, id, Duration::from_millis(120))
            .await
            .unwrap();
        assert!(timed_out.is_none());

        let p2 = pool.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(80)).await;
            mark(&p2, id, "done", Some(r#"{"applied":3}"#))
                .await
                .unwrap();
        });
        let s = wait_for_terminal(&pool, id, Duration::from_secs(2))
            .await
            .unwrap()
            .expect("reached terminal before timeout");
        assert_eq!(s.state, "done");
        assert_eq!(s.result.as_deref(), Some(r#"{"applied":3}"#));
    }

    #[sqlx::test]
    async fn targeted_scan_enqueue_merges_instead_of_piling_up(pool: SqlitePool) {
        let n = now();
        let id = enqueue_scan_targeted(&pool, &["/lib/a.cbz".into()], 5, n + 3600)
            .await
            .unwrap()
            .expect("first targeted scan inserts");
        assert!(
            claim_next(&pool).await.unwrap().is_none(),
            "settle scan is not claimable before its run_after"
        );

        assert!(
            enqueue_scan_targeted(&pool, &["/lib/b.cbz".into(), "/lib/a.cbz".into()], 0, 0)
                .await
                .unwrap()
                .is_none()
        );
        let pending: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM jobs WHERE kind = 'scan' AND state = 'pending'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(pending, 1, "merged, not piled up");

        sqlx::query("UPDATE jobs SET run_after = 0 WHERE id = ?")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        let job = claim_next(&pool).await.unwrap().unwrap();
        assert_eq!(job.kind, "scan");
        let payload: serde_json::Value = serde_json::from_str(&job.payload.unwrap()).unwrap();
        assert_eq!(
            payload["paths"],
            serde_json::json!(["/lib/a.cbz", "/lib/b.cbz"])
        );
        assert_eq!(payload["settle"], serde_json::json!(0), "min of 5 and 0");
    }

    #[sqlx::test]
    async fn targeted_scan_enqueue_is_covered_by_a_pending_full_scan(pool: SqlitePool) {
        let full = enqueue(&pool, "scan", None).await.unwrap();
        assert!(enqueue_scan_targeted(&pool, &["/lib/a.cbz".into()], 0, 0)
            .await
            .unwrap()
            .is_none());
        let payload: Option<String> = sqlx::query_scalar("SELECT payload FROM jobs WHERE id = ?")
            .bind(full)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(payload.is_none(), "full scan stays a full scan");
    }

    #[sqlx::test]
    async fn enqueue_full_scan_upgrades_a_pending_targeted_scan(pool: SqlitePool) {
        let targeted = enqueue_scan_targeted(&pool, &["/lib/a.cbz".into()], 0, 0)
            .await
            .unwrap()
            .expect("targeted scan inserts");
        assert!(
            enqueue_full_scan(&pool).await.unwrap().is_none(),
            "upgraded the pending scan in place, not a new row"
        );
        let (count, payload): (i64, Option<String>) = sqlx::query_as(
            "SELECT COUNT(*), MAX(payload) FROM jobs WHERE kind = 'scan' AND state = 'pending'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1, "still exactly one pending scan");
        assert!(
            payload.is_none(),
            "the targeted scan became a FULL scan (payload cleared)"
        );
        let id: i64 = sqlx::query_scalar("SELECT id FROM jobs WHERE kind = 'scan'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(id, targeted);

        assert!(enqueue_full_scan(&pool).await.unwrap().is_none());
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM jobs WHERE kind = 'scan' AND state = 'pending'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
    }

    #[sqlx::test]
    async fn enqueue_coalesced_dedups_pending_scans(pool: SqlitePool) {
        assert!(enqueue_coalesced(&pool, "scan", None)
            .await
            .unwrap()
            .is_some());
        assert!(enqueue_coalesced(&pool, "scan", None)
            .await
            .unwrap()
            .is_none());
        assert!(enqueue_coalesced(&pool, "scan", None)
            .await
            .unwrap()
            .is_none());
        let pending: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM jobs WHERE kind = 'scan' AND state = 'pending'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(pending, 1, "scan signals coalesced into one pending job");

        assert!(enqueue_coalesced(&pool, "thumbnail_sweep", None)
            .await
            .unwrap()
            .is_some());

        let claimed = claim_next(&pool).await.unwrap().unwrap();
        assert_eq!(claimed.kind, "scan");
        assert!(
            enqueue_coalesced(&pool, "scan", None)
                .await
                .unwrap()
                .is_some(),
            "scan running (not pending) → follow-up allowed"
        );
    }

    #[test]
    fn next_daily_run_picks_today_or_tomorrow() {
        use chrono::{FixedOffset, TimeZone};
        let tz = FixedOffset::west_opt(5 * 3600).unwrap();

        let before = tz.with_ymd_and_hms(2026, 7, 11, 1, 30, 0).unwrap();
        let expect_today = tz.with_ymd_and_hms(2026, 7, 11, 3, 0, 0).unwrap();
        assert_eq!(
            next_daily_run_in(&before, 3),
            expect_today.timestamp(),
            "01:30 schedules today's 03:00"
        );

        let at = tz.with_ymd_and_hms(2026, 7, 11, 3, 0, 0).unwrap();
        let after = tz.with_ymd_and_hms(2026, 7, 11, 22, 0, 0).unwrap();
        let expect_tomorrow = tz.with_ymd_and_hms(2026, 7, 12, 3, 0, 0).unwrap();
        assert_eq!(next_daily_run_in(&at, 3), expect_tomorrow.timestamp());
        assert_eq!(next_daily_run_in(&after, 3), expect_tomorrow.timestamp());
    }

    #[sqlx::test]
    async fn plugin_update_check_schedules_once_and_waits(pool: SqlitePool) {
        schedule_plugin_update_check(&pool).await.unwrap();
        schedule_plugin_update_check(&pool).await.unwrap();
        let rows: Vec<(String, i64)> =
            sqlx::query_as("SELECT state, run_after FROM jobs WHERE kind = 'plugin_update_check'")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(rows.len(), 1, "coalesced to one pending occurrence");
        assert!(
            rows[0].1 > now(),
            "scheduled strictly in the future (next 03:00 local)"
        );
        assert!(claim_next(&pool).await.unwrap().is_none());

        sqlx::query("UPDATE jobs SET run_after = 0 WHERE kind = 'plugin_update_check'")
            .execute(&pool)
            .await
            .unwrap();
        let job = claim_next(&pool).await.unwrap().unwrap();
        assert_eq!(job.kind, "plugin_update_check");
        schedule_plugin_update_check(&pool).await.unwrap();
        let pending: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM jobs WHERE kind = 'plugin_update_check' AND state = 'pending'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(pending, 1, "next occurrence queued while current runs");
    }

    #[sqlx::test]
    async fn reset_running_requeues_crashed_jobs(pool: SqlitePool) {
        let id = enqueue(&pool, "scan", None).await.unwrap();
        claim_next(&pool).await.unwrap();
        assert!(claim_next(&pool).await.unwrap().is_none());

        assert_eq!(reset_running(&pool).await.unwrap(), 1);
        let again = claim_next(&pool).await.unwrap().unwrap();
        assert_eq!(again.id, id);
        assert_eq!(again.attempts, 2);
    }

    #[sqlx::test]
    async fn claim_respects_backoff(pool: SqlitePool) {
        let id = enqueue(&pool, "scan", None).await.unwrap();
        requeue(&pool, id, now() + 10_000).await.unwrap();
        assert!(claim_next(&pool).await.unwrap().is_none());
        requeue(&pool, id, 0).await.unwrap();
        assert_eq!(claim_next(&pool).await.unwrap().unwrap().id, id);
    }

    #[sqlx::test]
    async fn prune_drops_old_terminal_rows(pool: SqlitePool) {
        sqlx::query(
            "INSERT INTO jobs (kind, state, attempts, created_at, updated_at, run_after) \
             VALUES ('scan', 'done', 1, 0, 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let fresh = enqueue(&pool, "scan", None).await.unwrap();

        assert_eq!(
            prune_terminal(&pool, Duration::from_secs(60))
                .await
                .unwrap(),
            1
        );
        assert_eq!(claim_next(&pool).await.unwrap().unwrap().id, fresh);
    }

    #[sqlx::test]
    async fn recompute_writes_the_neighbor_table(pool: SqlitePool) {
        let a = mk(&pool, "aaaa1111", 1).await;
        let b = mk(&pool, "bbbb2222", 2).await;
        let c = mk(&pool, "cccc3333", 3).await;
        tag(&pool, a, "creator", "foo").await;
        tag(&pool, a, "tag", "x").await;
        tag(&pool, b, "creator", "foo").await;
        tag(&pool, c, "tag", "lonely").await;

        let cache = recommend::RecommendationCache::new(8);
        let cancel = CancellationToken::new();
        let written = sweep_all(&pool, &cache, &cancel).await.unwrap();
        assert_eq!(written, 3, "all three tagged items swept");

        let a_ids: Vec<i64> = repo::read_neighbors(&pool, a, 50)
            .await
            .unwrap()
            .into_iter()
            .map(|(i, _)| i)
            .collect();
        assert_eq!(a_ids, vec![b], "b is a's only neighbour");

        sweep_all(&pool, &cache, &cancel).await.unwrap();
        assert_eq!(repo::read_neighbors(&pool, a, 50).await.unwrap().len(), 1);
    }

    #[sqlx::test]
    async fn targeted_recompute_updates_item_and_its_sharers(pool: SqlitePool) {
        let a = mk(&pool, "aaaa1111", 1).await;
        let b = mk(&pool, "bbbb2222", 2).await;
        let c = mk(&pool, "cccc3333", 3).await;
        let d = mk(&pool, "dddd4444", 4).await;
        let e = mk(&pool, "eeee5555", 5).await;
        tag(&pool, a, "creator", "foo").await;
        tag(&pool, b, "creator", "foo").await;
        tag(&pool, d, "creator", "bar").await;
        tag(&pool, e, "creator", "bar").await;
        let cache = recommend::RecommendationCache::new(8);
        let cancel = CancellationToken::new();
        sweep_all(&pool, &cache, &cancel).await.unwrap();
        assert!(!repo::read_neighbors(&pool, a, 50)
            .await
            .unwrap()
            .iter()
            .any(|(i, _)| *i == c));

        tag(&pool, c, "creator", "foo").await;
        let touched = sweep_item(&pool, &cache, c).await.unwrap();
        assert!(touched >= 2, "recomputed c plus its sharers (a, b)");

        assert!(!repo::read_neighbors(&pool, c, 50).await.unwrap().is_empty());
        for (who, id) in [("a", a), ("b", b)] {
            assert!(
                repo::read_neighbors(&pool, id, 50)
                    .await
                    .unwrap()
                    .iter()
                    .any(|(i, _)| *i == c),
                "{who}'s list now includes the newly-tagged c"
            );
        }
    }

    #[sqlx::test]
    async fn standalone_is_only_an_item_neighbor_destination(pool: SqlitePool) {
        let leaf = mk(&pool, "leaf", 1).await;
        let standalone: i64 = sqlx::query_scalar(
            "INSERT INTO items (scheme_tag, structural_hash, path, size_bytes, mtime, format, title, kind, added_at, last_modified_at) \
             VALUES ('zip-structural-v1', 'standalone', '/p/standalone', 0, 0, 'cbz', 'Standalone', 'uncategorized', 2, 0) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let filler = mk(&pool, "filler", 3).await;
        tag(&pool, leaf, "creator", "shared").await;
        tag(&pool, standalone, "creator", "shared").await;
        tag(&pool, filler, "creator", "other").await;

        let cache = recommend::RecommendationCache::new(8);
        let cancel = CancellationToken::new();
        sweep_all(&pool, &cache, &cancel).await.unwrap();

        assert!(
            repo::read_neighbors(&pool, leaf, 50)
                .await
                .unwrap()
                .iter()
                .any(|(id, _)| *id == standalone),
            "a leaf may recommend a standalone destination"
        );
        assert!(
            repo::read_neighbors(&pool, standalone, 50)
                .await
                .unwrap()
                .is_empty(),
            "a standalone has no duplicate item-level source row"
        );

        let shared = repo::get_or_create_tag(&pool, "creator", "shared")
            .await
            .unwrap();
        repo::remove_item_tag(&pool, standalone, shared, "none")
            .await
            .unwrap();
        sweep_item(&pool, &cache, standalone).await.unwrap();
        assert!(
            repo::read_neighbors(&pool, leaf, 50)
                .await
                .unwrap()
                .iter()
                .all(|(id, _)| *id != standalone),
            "editing a standalone refreshes leaf holders"
        );
        assert!(
            repo::read_neighbors(&pool, standalone, 50)
                .await
                .unwrap()
                .is_empty(),
            "targeted invalidation must not recreate its source row"
        );
    }

    async fn mk(pool: &SqlitePool, structural_hash: &str, i: i64) -> i64 {
        let sid: i64 = sqlx::query_scalar(
            "INSERT INTO series (kind, title, folder_path, added_at) \
             VALUES ('uncategorized', ?, ?, ?) RETURNING id",
        )
        .bind(format!("Series {i}"))
        .bind(format!("uncategorized/series-{i}"))
        .bind(i)
        .fetch_one(pool)
        .await
        .unwrap();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO items (scheme_tag, structural_hash, path, size_bytes, mtime, format, title, series_id, added_at, last_modified_at) \
             VALUES ('zip-structural-v1', ?, ?, 0, 0, 'cbz', ?, ?, ?, 0) RETURNING id",
        )
        .bind(structural_hash)
        .bind(format!("/p/{i}"))
        .bind(format!("Item {i}"))
        .bind(sid)
        .bind(i)
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO item_series_leaf (item_id, series_id, number_sort, number_disp) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(id)
        .bind(sid)
        .bind(i as f64)
        .bind(i.to_string())
        .execute(pool)
        .await
        .unwrap();
        id
    }

    async fn tag(pool: &SqlitePool, item_id: i64, ns: &str, v: &str) {
        let tid = repo::get_or_create_tag(pool, ns, v).await.unwrap();
        repo::add_item_tag(pool, item_id, tid, "none", "manual")
            .await
            .unwrap();
    }

    async fn seed_clusters(pool: &SqlitePool, clusters: usize, size: usize) -> i64 {
        let mut first = 0i64;
        for cl in 0..clusters {
            let creator = repo::get_or_create_tag(pool, "creator", &format!("creator{cl}"))
                .await
                .unwrap();
            for k in 0..size {
                let idx = (cl * size + k) as i64;
                let hash = format!("{idx:016x}");
                let id = mk(pool, &hash, idx).await;
                if idx == 0 {
                    first = id;
                }
                repo::add_item_tag(pool, id, creator, "none", "manual")
                    .await
                    .unwrap();
            }
        }
        first
    }

    #[sqlx::test]
    async fn targeted_recompute_is_bounded_not_o_n(pool: SqlitePool) {
        let clusters = 8;
        let size = 5;
        let n = clusters * size;
        let first = seed_clusters(&pool, clusters, size).await;

        let cache = recommend::RecommendationCache::new(8);
        let cancel = CancellationToken::new();

        let full = sweep_all(&pool, &cache, &cancel).await.unwrap();
        assert_eq!(full, n, "full sweep is O(N) — touches all items");

        let targeted = sweep_item(&pool, &cache, first).await.unwrap();
        assert_eq!(
            targeted, size,
            "targeted recompute is bounded by the cluster"
        );
        assert!(targeted < full, "targeted ({targeted}) ≪ full ({full})");
    }

    #[sqlx::test]
    #[ignore = "perf: seeds thousands of items; run with `-- --ignored`"]
    async fn targeted_recompute_beats_full_sweep_wall_clock(pool: SqlitePool) {
        let first = seed_clusters(&pool, 100, 10).await;
        let cache = recommend::RecommendationCache::new(8);
        let cancel = CancellationToken::new();

        let t0 = std::time::Instant::now();
        let full = sweep_all(&pool, &cache, &cancel).await.unwrap();
        let full_ms = t0.elapsed().as_secs_f64() * 1000.0;

        let t1 = std::time::Instant::now();
        let targeted = sweep_item(&pool, &cache, first).await.unwrap();
        let targeted_ms = t1.elapsed().as_secs_f64() * 1000.0;

        eprintln!(
            "full sweep: {full} items in {full_ms:.0}ms | targeted: {targeted} items in {targeted_ms:.0}ms ({:.0}x faster)",
            full_ms / targeted_ms.max(0.01)
        );
        assert_eq!(full, 1000);
        assert_eq!(targeted, 10);
        assert!(
            targeted_ms * 5.0 < full_ms,
            "targeted ({targeted_ms:.0}ms) should be ≫5x faster than full ({full_ms:.0}ms)"
        );
    }

    #[sqlx::test]
    async fn targeted_recompute_clears_stale_neighbours_on_tag_removal(pool: SqlitePool) {
        seed_clusters(&pool, 3, 2).await;
        let (x, z) = (iid(&pool, 0).await, iid(&pool, 1).await);
        let cache = recommend::RecommendationCache::new(8);
        let cancel = CancellationToken::new();
        sweep_all(&pool, &cache, &cancel).await.unwrap();
        let z_before = repo::read_neighbors(&pool, z, 50).await.unwrap();
        assert!(
            z_before.iter().any(|(i, _)| *i == x),
            "precondition: Z lists X as a neighbour"
        );

        let creator = repo::get_or_create_tag(&pool, "creator", "creator0")
            .await
            .unwrap();
        repo::remove_item_tag(&pool, x, creator, "none")
            .await
            .unwrap();
        sweep_item(&pool, &cache, x).await.unwrap();

        let z_after = repo::read_neighbors(&pool, z, 50).await.unwrap();
        assert!(
            !z_after.iter().any(|(i, _)| *i == x),
            "Z still points at X after X lost the shared tag (stale neighbour row)"
        );
    }

    async fn iid(pool: &SqlitePool, idx: i64) -> i64 {
        repo::item_by_bucket(pool, "zip-structural-v1", &format!("{idx:016x}"))
            .await
            .unwrap()
            .unwrap()
            .id
    }

    async fn entry_key_for_item(pool: &SqlitePool, item_id: i64) -> i64 {
        let map = repo::leaf_series_map(pool, &[item_id]).await.unwrap();
        map.get(&item_id).map(|sid| -sid).unwrap_or(item_id)
    }

    async fn seed_kinded(pool: &SqlitePool, idx: i64, kind: &str, creator_val: &str) -> i64 {
        let hash = format!("{idx:016x}");
        let sid: i64 = sqlx::query_scalar(
            "INSERT INTO series (kind, title, folder_path, added_at) VALUES (?, ?, ?, ?) RETURNING id",
        )
        .bind(kind)
        .bind(format!("Series {idx}"))
        .bind(format!("{kind}/series-{idx}"))
        .bind(idx)
        .fetch_one(pool).await.unwrap();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO items (scheme_tag, structural_hash, path, size_bytes, mtime, format, kind, title, series_id, added_at, last_modified_at) \
             VALUES ('zip-structural-v1', ?, ?, 0, 0, 'cbz', ?, ?, ?, ?, 0) RETURNING id",
        )
        .bind(&hash).bind(format!("/p/{idx}")).bind(kind).bind(format!("Item {idx}")).bind(sid).bind(idx)
        .fetch_one(pool).await.unwrap();
        sqlx::query(
            "INSERT INTO item_series_leaf (item_id, series_id, number_sort, number_disp) VALUES (?, ?, ?, ?)",
        )
        .bind(id).bind(sid).bind(idx as f64).bind(idx.to_string())
        .execute(pool).await.unwrap();
        let creator = repo::get_or_create_tag(pool, "creator", creator_val)
            .await
            .unwrap();
        repo::add_item_tag(pool, id, creator, "none", "manual")
            .await
            .unwrap();
        id
    }

    async fn seed_filler(pool: &SqlitePool, count: i64) {
        for k in 0..count {
            seed_kinded(pool, 500 + k, "filler", &format!("solo{k}")).await;
        }
    }

    #[sqlx::test]
    async fn neighbours_never_cross_kinds(pool: SqlitePool) {
        seed_filler(&pool, 20).await;
        let a = seed_kinded(&pool, 0, "manga", "shared").await;
        let b = seed_kinded(&pool, 1, "manga", "shared").await;
        let c = seed_kinded(&pool, 2, "comics", "shared").await;

        let cache = recommend::RecommendationCache::new(8);
        let cancel = CancellationToken::new();
        sweep_all(&pool, &cache, &cancel).await.unwrap();

        let a_n = repo::read_neighbors(&pool, a, 50).await.unwrap();
        assert!(
            a_n.iter().any(|(i, _)| *i == b),
            "a (manga) lists same-kind b"
        );
        assert!(
            !a_n.iter().any(|(i, _)| *i == c),
            "a (manga) must NOT list cross-kind c despite the shared tag"
        );

        let c_n = repo::read_neighbors(&pool, c, 50).await.unwrap();
        assert!(
            c_n.is_empty(),
            "c's only tag-sharers are cross-kind → no same-kind neighbour → empty list"
        );
    }

    #[sqlx::test]
    async fn targeted_recompute_isolates_kinds_on_tag_add(pool: SqlitePool) {
        seed_filler(&pool, 20).await;
        let ma = seed_kinded(&pool, 0, "manga", "shared").await;
        let mb = seed_kinded(&pool, 1, "manga", "shared").await;
        let da = seed_kinded(&pool, 2, "comics", "shared").await;
        let db = seed_kinded(&pool, 3, "comics", "shared").await;

        let cache = recommend::RecommendationCache::new(8);
        let cancel = CancellationToken::new();
        sweep_all(&pool, &cache, &cancel).await.unwrap();

        assert!(
            repo::read_neighbors(&pool, ma, 50)
                .await
                .unwrap()
                .iter()
                .any(|(i, _)| *i == mb),
            "precondition: ma↔mb are neighbours"
        );
        let da_before = repo::read_neighbors(&pool, da, 50).await.unwrap();
        let db_before = repo::read_neighbors(&pool, db, 50).await.unwrap();
        assert!(
            da_before.iter().any(|(i, _)| *i == db),
            "precondition: da↔db are neighbours"
        );

        let extra = repo::get_or_create_tag(&pool, "parody", "p").await.unwrap();
        repo::add_item_tag(&pool, ma, extra, "none", "manual")
            .await
            .unwrap();
        let touched = sweep_item(&pool, &cache, ma).await.unwrap();

        assert!(
            touched <= 2,
            "recompute stayed within the manga cluster, touched {touched}"
        );
        assert_eq!(
            repo::read_neighbors(&pool, da, 50).await.unwrap(),
            da_before,
            "da (comics) must be untouched by a manga tag ADD"
        );
        assert_eq!(
            repo::read_neighbors(&pool, db, 50).await.unwrap(),
            db_before,
            "db (comics) must be untouched by a manga tag ADD"
        );
    }

    #[sqlx::test]
    async fn targeted_recompute_isolates_kinds_on_tag_removal(pool: SqlitePool) {
        seed_filler(&pool, 20).await;
        let ma = seed_kinded(&pool, 0, "manga", "shared").await;
        let mb = seed_kinded(&pool, 1, "manga", "shared").await;
        let da = seed_kinded(&pool, 2, "comics", "shared").await;
        let db = seed_kinded(&pool, 3, "comics", "shared").await;

        let cache = recommend::RecommendationCache::new(8);
        let cancel = CancellationToken::new();
        sweep_all(&pool, &cache, &cancel).await.unwrap();
        assert!(
            repo::read_neighbors(&pool, mb, 50)
                .await
                .unwrap()
                .iter()
                .any(|(i, _)| *i == ma),
            "precondition: mb lists ma"
        );
        let da_before = repo::read_neighbors(&pool, da, 50).await.unwrap();
        let db_before = repo::read_neighbors(&pool, db, 50).await.unwrap();

        let creator = repo::get_or_create_tag(&pool, "creator", "shared")
            .await
            .unwrap();
        repo::remove_item_tag(&pool, ma, creator, "none")
            .await
            .unwrap();
        sweep_item(&pool, &cache, ma).await.unwrap();

        assert!(
            !repo::read_neighbors(&pool, mb, 50)
                .await
                .unwrap()
                .iter()
                .any(|(i, _)| *i == ma),
            "mb should drop ma after ma lost the shared tag"
        );
        assert_eq!(
            repo::read_neighbors(&pool, da, 50).await.unwrap(),
            da_before,
            "da (comics) must be untouched by a manga tag REMOVAL"
        );
        assert_eq!(
            repo::read_neighbors(&pool, db, 50).await.unwrap(),
            db_before,
            "db (comics) must be untouched by a manga tag REMOVAL"
        );
    }

    #[sqlx::test]
    async fn for_you_never_crosses_kinds(pool: SqlitePool) {
        seed_filler(&pool, 20).await;
        let ma = seed_kinded(&pool, 0, "manga", "shared").await;
        let mb = seed_kinded(&pool, 1, "manga", "shared").await;
        let da = seed_kinded(&pool, 2, "comics", "shared").await;
        let db = seed_kinded(&pool, 3, "comics", "shared").await;

        add_user_and_fav(&pool, &[ma]).await;

        let cancel = CancellationToken::new();
        sweep_entries_all(&pool, &cancel).await.unwrap();

        let corpus = repo::build_entry_corpus(&pool).await.unwrap();
        let recs = repo::recommend_for_you(&pool, &corpus, 1, now())
            .await
            .expect("recommend_for_you");
        let rec_ids: Vec<i64> = recs.iter().map(|(id, _)| *id).collect();
        let mb_key = entry_key_for_item(&pool, mb).await;
        let da_key = entry_key_for_item(&pool, da).await;
        let db_key = entry_key_for_item(&pool, db).await;

        assert!(
            rec_ids.contains(&mb_key),
            "same-kind mb's series should be recommended"
        );
        assert!(
            !rec_ids.contains(&da_key) && !rec_ids.contains(&db_key),
            "cross-kind comics items must never be recommended, got {rec_ids:?}"
        );
    }

    #[sqlx::test]
    async fn kind_change_recomputes_both_old_and_new_kind(pool: SqlitePool) {
        seed_filler(&pool, 20).await;
        let ma = seed_kinded(&pool, 0, "manga", "shared").await;
        let mb = seed_kinded(&pool, 1, "manga", "shared").await;
        let da = seed_kinded(&pool, 2, "comics", "shared").await;
        let db = seed_kinded(&pool, 3, "comics", "shared").await;

        let cache = recommend::RecommendationCache::new(8);
        let cancel = CancellationToken::new();
        sweep_all(&pool, &cache, &cancel).await.unwrap();

        assert!(
            repo::read_neighbors(&pool, ma, 50)
                .await
                .unwrap()
                .iter()
                .any(|(i, _)| *i == mb),
            "precondition: ma lists mb"
        );
        assert!(
            !repo::read_neighbors(&pool, da, 50)
                .await
                .unwrap()
                .iter()
                .any(|(i, _)| *i == mb),
            "precondition: comics da does NOT list the (still-manga) mb"
        );

        sqlx::query("UPDATE items SET kind = 'comics' WHERE id = ?")
            .bind(mb)
            .execute(&pool)
            .await
            .unwrap();
        sweep_item(&pool, &cache, mb).await.unwrap();

        assert!(
            !repo::read_neighbors(&pool, ma, 50)
                .await
                .unwrap()
                .iter()
                .any(|(i, _)| *i == mb),
            "ma (old kind) must drop mb after mb left manga"
        );
        let mb_n = repo::read_neighbors(&pool, mb, 50).await.unwrap();
        assert!(
            mb_n.iter().any(|(i, _)| *i == da) && mb_n.iter().any(|(i, _)| *i == db),
            "mb (new kind) must list its comics sharers, got {mb_n:?}"
        );
        assert!(
            repo::read_neighbors(&pool, da, 50)
                .await
                .unwrap()
                .iter()
                .any(|(i, _)| *i == mb),
            "da (new kind) must now list mb"
        );
    }

    async fn sweep_all(
        pool: &SqlitePool,
        cache: &recommend::RecommendationCache<i64>,
        cancel: &CancellationToken,
    ) -> Result<usize> {
        let corpus = repo::build_corpus(pool).await?;
        recompute_all(pool, pool, cache, &corpus, cancel).await
    }
    async fn sweep_item(
        pool: &SqlitePool,
        cache: &recommend::RecommendationCache<i64>,
        item_id: i64,
    ) -> Result<usize> {
        let corpus = repo::build_corpus(pool).await?;
        recompute_item(pool, pool, cache, &corpus, item_id).await
    }
    async fn sweep_entries_all(pool: &SqlitePool, cancel: &CancellationToken) -> Result<usize> {
        let corpus = repo::build_entry_corpus(pool).await?;
        recompute_entry_all(pool, pool, &corpus, cancel).await
    }
    async fn add_user_and_fav(pool: &SqlitePool, item_ids: &[i64]) {
        sqlx::query("INSERT INTO users (username, password_hash, role, created_at) VALUES ('u','x','admin',0)")
            .execute(pool).await.unwrap();
        for &iid in item_ids {
            sqlx::query("INSERT INTO favorites (user_id, item_id, created_at) VALUES (1, ?, ?)")
                .bind(iid)
                .bind(now())
                .execute(pool)
                .await
                .unwrap();
        }
    }

    #[sqlx::test]
    async fn targeted_entry_recompute_drops_stale_holder_on_tag_removal(pool: SqlitePool) {
        async fn series(pool: &SqlitePool, title: &str) -> i64 {
            sqlx::query_scalar(
                "INSERT INTO series (kind, title, folder_path, added_at) VALUES ('manga', ?, ?, 1) RETURNING id",
            )
            .bind(title).bind(format!("manga/{title}")).fetch_one(pool).await.unwrap()
        }
        async fn series_tag(pool: &SqlitePool, sid: i64, value: &str) {
            let t = repo::get_or_create_tag(pool, "tag", value).await.unwrap();
            repo::add_series_tag(pool, sid, t, "none", "manual")
                .await
                .unwrap();
        }

        let a = series(&pool, "A").await;
        let b = series(&pool, "B").await;
        for v in ["x", "y"] {
            series_tag(&pool, a, v).await;
            series_tag(&pool, b, v).await;
        }
        for i in 0..3 {
            sqlx::query(
                "INSERT INTO items (scheme_tag, structural_hash, path, size_bytes, mtime, format, title, kind, modality, page_count, added_at, last_modified_at) \
                 VALUES ('zip-structural-v1', ?, ?, 1, 1, 'cbz', 'f', 'manga', 'paginated', 5, 1, 0)",
            )
            .bind(format!("f{i}")).bind(format!("/f/{i}")).execute(&pool).await.unwrap();
        }
        let a_key = repo::entry_key_series(a);
        let b_key = repo::entry_key_series(b);

        let corpus = repo::build_entry_corpus(&pool).await.unwrap();
        recompute_entry(&pool, &pool, &corpus, repo::entry_key_series(a))
            .await
            .unwrap();
        let keys = |v: Vec<(i64, f32)>| v.into_iter().map(|(k, _)| k).collect::<Vec<_>>();
        assert_eq!(
            keys(repo::read_entry_neighbors(&pool, a_key, 50).await.unwrap()),
            vec![b_key],
            "A ~ B initially"
        );
        assert_eq!(
            keys(repo::read_entry_neighbors(&pool, b_key, 50).await.unwrap()),
            vec![a_key],
            "B ~ A initially (the sharer was refreshed too)"
        );

        repo::clear_series_tags_from_source(&pool, a, "manual")
            .await
            .unwrap();
        let corpus = repo::build_entry_corpus(&pool).await.unwrap();
        recompute_entry(&pool, &pool, &corpus, repo::entry_key_series(a))
            .await
            .unwrap();
        assert!(
            repo::read_entry_neighbors(&pool, a_key, 50)
                .await
                .unwrap()
                .is_empty(),
            "A has no neighbours after losing its tags"
        );
        assert!(
            repo::read_entry_neighbors(&pool, b_key, 50)
                .await
                .unwrap()
                .is_empty(),
            "B (a holder) no longer lists the now-empty A"
        );
    }

    #[sqlx::test]
    async fn for_you_surfaces_neighbours_of_favorites(pool: SqlitePool) {
        seed_clusters(&pool, 4, 5).await;
        let favs = [iid(&pool, 0).await, iid(&pool, 1).await];
        add_user_and_fav(&pool, &favs).await;

        let cancel = CancellationToken::new();
        sweep_entries_all(&pool, &cancel).await.unwrap();

        let corpus = repo::build_entry_corpus(&pool).await.unwrap();
        let recs = repo::recommend_for_you(&pool, &corpus, 1, now())
            .await
            .expect("recommend_for_you");
        let rec_ids: Vec<i64> = recs.iter().map(|(id, _)| *id).collect();

        let allowed = [
            entry_key_for_item(&pool, iid(&pool, 2).await).await,
            entry_key_for_item(&pool, iid(&pool, 3).await).await,
            entry_key_for_item(&pool, iid(&pool, 4).await).await,
        ];
        assert!(!rec_ids.is_empty(), "produced recommendations");
        for &r in &rec_ids {
            assert!(
                allowed.contains(&r),
                "rec {r} must be a non-favorited cluster-0 item (no cross-cluster, no favorited)"
            );
        }
    }

    #[sqlx::test]
    #[ignore = "perf: seeds items + recomputes; run with `-- --ignored`"]
    async fn for_you_funnel_is_fast(pool: SqlitePool) {
        seed_clusters(&pool, 100, 10).await;
        let favs = [
            iid(&pool, 0).await,
            iid(&pool, 10).await,
            iid(&pool, 50).await,
            iid(&pool, 100).await,
            iid(&pool, 500).await,
        ];
        add_user_and_fav(&pool, &favs).await;
        let cancel = CancellationToken::new();
        sweep_entries_all(&pool, &cancel).await.unwrap();

        let t = std::time::Instant::now();
        let corpus = repo::build_entry_corpus(&pool).await.unwrap();
        let recs = repo::recommend_for_you(&pool, &corpus, 1, now())
            .await
            .expect("recommend_for_you");
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        eprintln!(
            "for-you funnel: {} recs in {ms:.1}ms (1000 items, 5 favorites)",
            recs.len()
        );
        assert!(!recs.is_empty(), "should produce recommendations");
        assert!(
            ms < 500.0,
            "for-you funnel should be well under 500ms, was {ms:.0}ms"
        );
    }
}
