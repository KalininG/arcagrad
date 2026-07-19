//! On-demand admin health metrics.

use std::sync::Mutex;
use std::time::Instant;

use serde::Serialize;
use sqlx::SqlitePool;
use utoipa::ToSchema;

use crate::server::config::Config;
use crate::server::error::AppError;
use crate::AppState;

/// State retained between metric requests.
pub struct MetricsState {
    boot: Instant,
    last_sys: Mutex<Option<SysSample>>,
}

impl Default for MetricsState {
    fn default() -> Self {
        Self {
            boot: Instant::now(),
            last_sys: Mutex::new(None),
        }
    }
}

#[derive(Clone, Copy)]
struct SysSample {
    at: Instant,
    cpu_ticks: u64,
    net_rx: u64,
    net_tx: u64,
    io_read: u64,
    io_write: u64,
}

#[derive(Serialize, ToSchema)]
pub struct Metrics {
    pub generated_at: i64,
    pub version: String,
    pub uptime_secs: u64,
    /// Ordered by severity.
    pub health: Vec<HealthItem>,
    pub library: LibraryMetrics,
    pub jobs: JobsMetrics,
    pub storage: StorageMetrics,
    pub system: SystemMetrics,
}

#[derive(Serialize, ToSchema)]
pub struct HealthItem {
    /// `ok`, `warn`, or `error`.
    pub level: String,
    pub message: String,
}

#[derive(Serialize, ToSchema)]
pub struct LibraryMetrics {
    pub items: i64,
    pub by_kind: Vec<KindCount>,
    pub series: i64,
    pub pages: i64,
    /// Total indexed archive bytes.
    pub bytes: i64,
    pub tags: i64,
    pub untagged: i64,
    pub phash_done: i64,
    pub neighbours_done: i64,
    pub neighbour_eligible: i64,
    pub search_docs: i64,
    pub users: i64,
    pub admins: i64,
    pub api_keys: i64,
    pub sessions: i64,
    pub signup_enabled: bool,
    pub guest_enabled: bool,
    pub hidden_kinds: i64,
}

#[derive(Serialize, ToSchema)]
pub struct KindCount {
    pub kind: String,
    pub count: i64,
    pub bytes: i64,
}

#[derive(Serialize, ToSchema)]
pub struct JobsMetrics {
    pub counts: Vec<JobCount>,
    pub pending: Vec<PendingJob>,
    pub running: Vec<RunningJob>,
    pub failed: Vec<FailedJob>,
    pub last_scan_at: Option<i64>,
    pub last_scan_result: Option<String>,
    pub watcher: bool,
}

#[derive(Serialize, ToSchema)]
pub struct JobCount {
    pub kind: String,
    pub state: String,
    pub count: i64,
}

#[derive(Serialize, ToSchema)]
pub struct PendingJob {
    pub kind: String,
    pub count: i64,
    pub run_after: i64,
}

#[derive(Serialize, ToSchema)]
pub struct RunningJob {
    pub kind: String,
    pub running_secs: i64,
}

#[derive(Serialize, ToSchema)]
pub struct FailedJob {
    pub kind: String,
    pub attempts: i64,
    pub error: Option<String>,
    pub at: i64,
}

#[derive(Serialize, ToSchema)]
pub struct StorageMetrics {
    pub data: Option<DiskSpace>,
    pub content: Option<DiskSpace>,
    /// Whether both directories share a filesystem.
    pub same_fs: bool,
    pub db_bytes: i64,
}

#[derive(Serialize, ToSchema)]
pub struct DiskSpace {
    pub free: u64,
    pub total: u64,
}

/// Linux system metrics. Unsupported or unsampled values are `None`.
#[derive(Serialize, ToSchema)]
pub struct SystemMetrics {
    /// Process CPU since the previous request.
    pub cpu_pct: Option<f64>,
    pub cores: Option<f64>,
    pub mem_rss: Option<u64>,
    pub mem_limit: Option<u64>,
    pub net: NetIo,
    pub disk: DiskIo,
    pub db_pool: DbPool,
}

#[derive(Serialize, ToSchema)]
pub struct NetIo {
    pub rx_total: Option<u64>,
    pub tx_total: Option<u64>,
    pub rx_rate: Option<f64>,
    pub tx_rate: Option<f64>,
}

#[derive(Serialize, ToSchema)]
pub struct DiskIo {
    pub read_total: Option<u64>,
    pub write_total: Option<u64>,
    pub read_rate: Option<f64>,
    pub write_rate: Option<f64>,
}

#[derive(Serialize, ToSchema)]
pub struct DbPool {
    pub in_use: u32,
    pub size: u32,
}

pub async fn collect(state: &AppState) -> Result<Metrics, AppError> {
    let now = crate::now_secs();

    let mut library = gather_library(&state.read, now).await?;
    library.search_docs = state.search.doc_count() as i64;
    let jobs = gather_jobs(&state.read, state.config.watch, now).await?;
    let storage = gather_storage(&state.config);
    let system = gather_system(state);
    let health = derive_health(&library, &jobs, &storage);

    Ok(Metrics {
        generated_at: now,
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_secs: state.metrics.boot.elapsed().as_secs(),
        health,
        library,
        jobs,
        storage,
        system,
    })
}

async fn gather_library(pool: &SqlitePool, now: i64) -> anyhow::Result<LibraryMetrics> {
    let (items, pages, phash_done, bytes): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(page_count),0), COALESCE(SUM(phash IS NOT NULL),0), \
         COALESCE(SUM(size_bytes),0) FROM items",
    )
    .fetch_one(pool)
    .await?;
    let by_kind: Vec<KindCount> = sqlx::query_as::<_, (String, i64, i64)>(
        "SELECT kind, COUNT(*) c, COALESCE(SUM(size_bytes),0) FROM items GROUP BY kind ORDER BY c DESC",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(kind, count, bytes)| KindCount { kind, count, bytes })
    .collect();
    let series: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM series")
        .fetch_one(pool)
        .await?;
    let tags: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tags")
        .fetch_one(pool)
        .await?;
    let untagged: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM items WHERE id NOT IN (SELECT item_id FROM item_tags)",
    )
    .fetch_one(pool)
    .await?;
    let neighbours_done: i64 =
        sqlx::query_scalar("SELECT COUNT(DISTINCT item_id) FROM item_neighbors")
            .fetch_one(pool)
            .await?;
    // A kind needs at least two tagged items to produce neighbours.
    let neighbour_eligible: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(CASE WHEN c >= 2 THEN c ELSE 0 END), 0) FROM ( \
            SELECT COUNT(DISTINCT it.item_id) AS c FROM item_tags it \
            JOIN items i ON i.id = it.item_id GROUP BY i.kind)",
    )
    .fetch_one(pool)
    .await?;
    let (users, admins): (i64, i64) =
        sqlx::query_as("SELECT COUNT(*), COALESCE(SUM(role='admin'),0) FROM users")
            .fetch_one(pool)
            .await?;
    let api_keys: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM api_keys")
        .fetch_one(pool)
        .await?;
    let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE expires_at > ?")
        .bind(now)
        .fetch_one(pool)
        .await?;
    let signup_enabled = crate::server::auth::get_bool_setting(
        pool,
        crate::server::auth::SETTING_SIGNUP_ENABLED,
        false,
    )
    .await?;
    let guest_enabled = crate::server::auth::get_bool_setting(
        pool,
        crate::server::auth::SETTING_GUEST_ENABLED,
        false,
    )
    .await?;
    let hidden_kinds: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM kind_access")
        .fetch_one(pool)
        .await?;

    Ok(LibraryMetrics {
        items,
        by_kind,
        series,
        pages,
        bytes,
        tags,
        untagged,
        phash_done,
        neighbours_done,
        neighbour_eligible,
        search_docs: 0,
        users,
        admins,
        api_keys,
        sessions,
        signup_enabled,
        guest_enabled,
        hidden_kinds,
    })
}

async fn gather_jobs(pool: &SqlitePool, watcher: bool, now: i64) -> anyhow::Result<JobsMetrics> {
    let counts: Vec<JobCount> = sqlx::query_as::<_, (String, String, i64)>(
        "SELECT kind, state, COUNT(*) FROM jobs GROUP BY kind, state",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(kind, state, count)| JobCount { kind, state, count })
    .collect();
    let pending: Vec<PendingJob> = sqlx::query_as::<_, (String, i64, i64)>(
        "SELECT kind, COUNT(*), MIN(run_after) FROM jobs WHERE state = 'pending' GROUP BY kind ORDER BY kind",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(kind, count, run_after)| PendingJob {
        kind,
        count,
        run_after,
    })
    .collect();
    let running: Vec<RunningJob> = sqlx::query_as::<_, (String, i64)>(
        "SELECT kind, updated_at FROM jobs WHERE state = 'running'",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(kind, updated)| RunningJob {
        kind,
        running_secs: (now - updated).max(0),
    })
    .collect();
    let failed: Vec<FailedJob> = sqlx::query_as::<_, (String, i64, Option<String>, i64)>(
        "SELECT kind, attempts, result, updated_at FROM jobs WHERE state = 'failed' ORDER BY updated_at DESC LIMIT 20",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(kind, attempts, result, at)| FailedJob {
        kind,
        attempts,
        error: result.as_deref().and_then(job_error),
        at,
    })
    .collect();
    let last_scan: Option<(i64, Option<String>)> = sqlx::query_as(
        "SELECT updated_at, result FROM jobs WHERE kind = 'scan' AND state = 'done' ORDER BY updated_at DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;

    Ok(JobsMetrics {
        counts,
        pending,
        running,
        failed,
        last_scan_at: last_scan.as_ref().map(|(at, _)| *at),
        last_scan_result: last_scan.and_then(|(_, r)| r),
        watcher,
    })
}

fn job_error(result: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(result)
        .ok()?
        .get("error")?
        .as_str()
        .map(str::to_string)
}

fn gather_storage(config: &Config) -> StorageMetrics {
    let db = config.data_dir.join("arca.db");
    let db_bytes = std::fs::metadata(&db).map(|m| m.len() as i64).unwrap_or(0);
    StorageMetrics {
        data: disk_space(&config.data_dir),
        content: disk_space(&config.content_dir),
        same_fs: same_filesystem(&config.data_dir, &config.content_dir),
        db_bytes,
    }
}

#[cfg(unix)]
fn same_filesystem(a: &std::path::Path, b: &std::path::Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn same_filesystem(_a: &std::path::Path, _b: &std::path::Path) -> bool {
    false
}

fn gather_system(state: &AppState) -> SystemMetrics {
    let cpu_ticks = read_and("/proc/self/stat", parse_cpu_stat);
    let net = read_and("/proc/net/dev", parse_net_dev);
    let io = read_and("/proc/self/io", parse_proc_io);
    let now = Instant::now();

    // Rates use the previous request as their sample.
    let mut cpu_pct = None;
    let (mut rx_rate, mut tx_rate, mut read_rate, mut write_rate) = (None, None, None, None);
    if let (Some(cpu), Some((rx, tx)), Some((rd, wr))) = (cpu_ticks, net, io) {
        let cur = SysSample {
            at: now,
            cpu_ticks: cpu,
            net_rx: rx,
            net_tx: tx,
            io_read: rd,
            io_write: wr,
        };
        let mut last = state.metrics.last_sys.lock().unwrap();
        if let Some(prev) = *last {
            let secs = now.duration_since(prev.at).as_secs_f64();
            if secs > 0.05 {
                let tps = clock_ticks_per_sec();
                cpu_pct = cpu
                    .checked_sub(prev.cpu_ticks)
                    .map(|d| (d as f64 / tps) / secs * 100.0);
                rx_rate = rx.checked_sub(prev.net_rx).map(|d| d as f64 / secs);
                tx_rate = tx.checked_sub(prev.net_tx).map(|d| d as f64 / secs);
                read_rate = rd.checked_sub(prev.io_read).map(|d| d as f64 / secs);
                write_rate = wr.checked_sub(prev.io_write).map(|d| d as f64 / secs);
            }
        }
        *last = Some(cur);
    }

    let (rx_total, tx_total) = net.map_or((None, None), |(rx, tx)| (Some(rx), Some(tx)));
    let (read_total, write_total) = io.map_or((None, None), |(rd, wr)| (Some(rd), Some(wr)));

    SystemMetrics {
        cpu_pct,
        cores: read_cores(),
        mem_rss: read_and("/proc/self/status", parse_vmrss),
        mem_limit: read_mem_limit(),
        net: NetIo {
            rx_total,
            tx_total,
            rx_rate,
            tx_rate,
        },
        disk: DiskIo {
            read_total,
            write_total,
            read_rate,
            write_rate,
        },
        db_pool: DbPool {
            in_use: state
                .read
                .size()
                .saturating_sub(state.read.num_idle() as u32),
            size: state.read.size(),
        },
    }
}

fn derive_health(
    lib: &LibraryMetrics,
    jobs: &JobsMetrics,
    storage: &StorageMetrics,
) -> Vec<HealthItem> {
    let mut out = Vec::new();
    let err = |m: String| HealthItem {
        level: "error".into(),
        message: m,
    };
    let warn = |m: String| HealthItem {
        level: "warn".into(),
        message: m,
    };

    let failed: i64 = jobs
        .counts
        .iter()
        .filter(|c| c.state == "failed")
        .map(|c| c.count)
        .sum();
    if failed > 0 {
        out.push(err(format!("{failed} background job(s) failed")));
    }
    if lib.items > 0 && lib.phash_done < lib.items {
        out.push(warn(format!(
            "{} of {} covers not yet hashed",
            lib.items - lib.phash_done,
            lib.items
        )));
    }
    if lib.neighbour_eligible > 0 && lib.neighbours_done < lib.neighbour_eligible {
        out.push(warn(format!(
            "{} tagged item(s) missing similarity neighbours",
            lib.neighbour_eligible - lib.neighbours_done
        )));
    }
    for d in [("/data", &storage.data), ("/content", &storage.content)] {
        if let Some(sp) = d.1 {
            // Some filesystems report available space above total space.
            let used = sp.total.saturating_sub(sp.free);
            if sp.total > 0 && used as f64 / sp.total as f64 > 0.90 {
                out.push(warn(format!("{} is over 90% full", d.0)));
            }
        }
    }
    if !jobs.watcher {
        out.push(warn("file watcher is disabled".into()));
    }
    if out.is_empty() {
        out.push(HealthItem {
            level: "ok".into(),
            message: "All systems healthy".into(),
        });
    }
    out
}

fn read_and<T>(path: &str, f: impl Fn(&str) -> Option<T>) -> Option<T> {
    std::fs::read_to_string(path).ok().and_then(|s| f(&s))
}

#[cfg(unix)]
fn disk_space(path: &std::path::Path) -> Option<DiskSpace> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    let frsize = s.f_frsize as u64;
    Some(DiskSpace {
        free: s.f_bavail as u64 * frsize,
        total: s.f_blocks as u64 * frsize,
    })
}

#[cfg(not(unix))]
fn disk_space(_path: &std::path::Path) -> Option<DiskSpace> {
    None
}

fn clock_ticks_per_sec() -> f64 {
    #[cfg(unix)]
    {
        let t = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        if t > 0 {
            return t as f64;
        }
    }
    100.0
}

fn read_cores() -> Option<f64> {
    if let Some(c) = read_and("/sys/fs/cgroup/cpu.max", parse_cgroup_cpu_max) {
        return Some(c);
    }
    std::thread::available_parallelism()
        .ok()
        .map(|n| n.get() as f64)
}

fn read_mem_limit() -> Option<u64> {
    if let Some(v) = read_and("/sys/fs/cgroup/memory.max", parse_cgroup_max) {
        return Some(v);
    }
    read_and("/sys/fs/cgroup/memory/memory.limit_in_bytes", |s| {
        let v: u64 = s.trim().parse().ok()?;
        (v < (1u64 << 62)).then_some(v)
    })
}

/// Read process CPU ticks from `/proc/self/stat`.
fn parse_cpu_stat(content: &str) -> Option<u64> {
    let rest = &content[content.rfind(')')? + 1..];
    let toks: Vec<&str> = rest.split_whitespace().collect();
    let utime: u64 = toks.get(11)?.parse().ok()?;
    let stime: u64 = toks.get(12)?.parse().ok()?;
    Some(utime + stime)
}

/// Sum non-loopback network bytes.
fn parse_net_dev(content: &str) -> Option<(u64, u64)> {
    let (mut rx, mut tx) = (0u64, 0u64);
    for line in content.lines().skip(2) {
        let Some((name, data)) = line.split_once(':') else {
            continue;
        };
        if name.trim() == "lo" {
            continue;
        }
        let f: Vec<&str> = data.split_whitespace().collect();
        rx += f.first()?.parse::<u64>().ok()?;
        tx += f.get(8)?.parse::<u64>().ok()?;
    }
    Some((rx, tx))
}

fn parse_proc_io(content: &str) -> Option<(u64, u64)> {
    let (mut r, mut w) = (None, None);
    for line in content.lines() {
        if let Some(v) = line.strip_prefix("read_bytes:") {
            r = v.trim().parse().ok();
        } else if let Some(v) = line.strip_prefix("write_bytes:") {
            w = v.trim().parse().ok();
        }
    }
    Some((r?, w?))
}

fn parse_vmrss(content: &str) -> Option<u64> {
    let line = content.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

fn parse_cgroup_max(content: &str) -> Option<u64> {
    let t = content.trim();
    if t == "max" {
        return None;
    }
    t.parse().ok()
}

fn parse_cgroup_cpu_max(content: &str) -> Option<f64> {
    let mut it = content.split_whitespace();
    let quota = it.next()?;
    let period: f64 = it.next()?.parse().ok()?;
    if quota == "max" || period <= 0.0 {
        return None;
    }
    Some(quota.parse::<f64>().ok()? / period)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    #[sqlx::test]
    async fn neighbour_eligible_excludes_lone_tagged_in_kind(pool: SqlitePool) {
        let item = |hash: &'static str, kind: &'static str, tag: Option<&'static str>| {
            let p = pool.clone();
            async move {
                let id: i64 = sqlx::query_scalar(
                    "INSERT INTO items (scheme_tag, structural_hash, path, size_bytes, mtime, \
                     format, title, kind, page_count, added_at, last_modified_at) \
                     VALUES ('zip-structural-v1', ?, ?, 1, 1, 'cbz', ?, ?, 5, 1, 0) RETURNING id",
                )
                .bind(hash)
                .bind(format!("/p/{hash}"))
                .bind(hash)
                .bind(kind)
                .fetch_one(&p)
                .await
                .unwrap();
                if let Some(t) = tag {
                    let tid: i64 = sqlx::query_scalar(
                        "INSERT INTO tags (namespace, value) VALUES ('tag', ?) \
                         ON CONFLICT(namespace, value) DO UPDATE SET value = value RETURNING id",
                    )
                    .bind(t)
                    .fetch_one(&p)
                    .await
                    .unwrap();
                    sqlx::query("INSERT INTO item_tags (item_id, tag_id) VALUES (?, ?)")
                        .bind(id)
                        .bind(tid)
                        .execute(&p)
                        .await
                        .unwrap();
                }
            }
        };
        item("a1", "a", Some("x")).await;
        item("a2", "a", Some("y")).await;
        item("b1", "b", Some("z")).await;
        item("u1", "a", None).await;

        let lib = gather_library(&pool, 0).await.unwrap();
        assert_eq!(lib.untagged, 1);
        assert_eq!(
            lib.neighbour_eligible, 2,
            "the two tagged 'a' items; the lone tagged 'b' is excluded"
        );
    }

    #[test]
    fn parses_proc_self_stat_cpu_ticks() {
        let s =
            "1234 (ar ca)server) S 1 1234 1234 0 -1 4194560 900 0 0 0 111 222 0 0 20 0 8 0 100 0 0";
        assert_eq!(parse_cpu_stat(s), Some(111 + 222));
    }

    #[test]
    fn parses_net_dev_skipping_loopback() {
        let s = "Inter-|   Receive                    |  Transmit\n\
                 face |bytes    packets errs|bytes\n\
                     lo: 500 5 0 0 0 0 0 0 700 7 0 0 0 0 0 0\n\
                   eth0: 1000 9 0 0 0 0 0 0 2000 9 0 0 0 0 0 0\n";
        assert_eq!(parse_net_dev(s), Some((1000, 2000)));
    }

    #[test]
    fn parses_proc_io() {
        let s =
            "rchar: 9\nwchar: 9\nread_bytes: 4096\nwrite_bytes: 8192\ncancelled_write_bytes: 0\n";
        assert_eq!(parse_proc_io(s), Some((4096, 8192)));
    }

    #[test]
    fn parses_vmrss_to_bytes() {
        assert_eq!(
            parse_vmrss("VmPeak: 100 kB\nVmRSS:   1536 kB\n"),
            Some(1536 * 1024)
        );
    }

    #[test]
    fn parses_cgroup_limits() {
        assert_eq!(parse_cgroup_max("max\n"), None);
        assert_eq!(parse_cgroup_max("4294967296\n"), Some(4_294_967_296));
        assert_eq!(parse_cgroup_cpu_max("400000 100000\n"), Some(4.0));
        assert_eq!(parse_cgroup_cpu_max("max 100000\n"), None);
    }

    #[test]
    fn job_error_extracts_message() {
        assert_eq!(
            job_error(r#"{"error":"HTTP 404"}"#),
            Some("HTTP 404".into())
        );
        assert_eq!(job_error(r#"{"added":4}"#), None);
    }

    #[test]
    fn health_flags_failures_and_gaps() {
        let lib = LibraryMetrics {
            items: 10,
            by_kind: vec![],
            series: 0,
            pages: 0,
            bytes: 0,
            tags: 0,
            untagged: 0,
            phash_done: 7,
            neighbours_done: 10,
            neighbour_eligible: 10,
            search_docs: 10,
            users: 1,
            admins: 1,
            api_keys: 0,
            sessions: 1,
            signup_enabled: false,
            guest_enabled: false,
            hidden_kinds: 0,
        };
        let jobs = JobsMetrics {
            counts: vec![JobCount {
                kind: "scrape".into(),
                state: "failed".into(),
                count: 2,
            }],
            pending: vec![],
            running: vec![],
            failed: vec![],
            last_scan_at: None,
            last_scan_result: None,
            watcher: true,
        };
        let storage = StorageMetrics {
            data: None,
            content: None,
            same_fs: false,
            db_bytes: 0,
        };
        let h = derive_health(&lib, &jobs, &storage);
        assert!(h
            .iter()
            .any(|i| i.level == "error" && i.message.contains("failed")));
        assert!(h
            .iter()
            .any(|i| i.level == "warn" && i.message.contains("covers")));
    }
}
