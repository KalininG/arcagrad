//! Host-side scraper contract and injected capabilities.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, Mutex as StdMutex, RwLock};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use sqlx::SqlitePool;
use tokio::io::AsyncWriteExt;
use tokio::runtime::Handle;

use crate::media::library;
use crate::repo;

/// Capabilities implemented by the current host. The fixed WASM export for
/// `read` remains named `pages` for ABI compatibility.
pub const SUPPORTED_CAPABILITIES: &[&str] = &[
    "scrape", "download", "browse", "read", "identify", "calendar",
];

pub use arcagrad_plugin_sdk::{
    AuthField, AuthSpec, BrowseItem, BrowsePage, BrowsePageInfo, BrowsePages, BrowseRequest,
    CalendarReference, CalendarReferenceError, CalendarRelease, CalendarRequest, CalendarResponse,
    CalendarSeriesResult, Candidate, DownloadPlan, Feed, HttpFetchRequest, HttpFetchResponse,
    IdentifyCandidate, IdentifyRequest, IdentifyResult, MappedTag, RateLimit, RateRule, RawTag,
    ReferenceInput, ScrapeHint, ScrapedChapter, ScrapedComment, ScrapedMetadata, CONTRACT_VERSION,
};
/// HTTP request passed from a plugin to the host. Method defaults to GET.
#[derive(Debug, Clone)]
pub struct FetchRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl FetchRequest {
    pub fn get(url: impl Into<String>) -> Self {
        FetchRequest {
            method: "GET".to_string(),
            url: url.into(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FetchResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Host-injected network capability with centralized policy and rate limiting.
#[async_trait]
pub trait Fetcher: Send + Sync {
    async fn fetch(&self, req: FetchRequest) -> Result<FetchResponse>;

    /// Host-only asset fetch that bypasses vendor throttling but retains SSRF checks.
    async fn fetch_asset(&self, req: FetchRequest) -> Result<(u16, Option<String>, Vec<u8>)> {
        let r = self.fetch(req).await?;
        Ok((r.status, None, r.body))
    }

    /// Stream a successful response to `dest`. The default implementation buffers it.
    async fn fetch_to_file(&self, req: FetchRequest, dest: &Path) -> Result<u16> {
        let resp = self.fetch(req).await?;
        if (200..300).contains(&resp.status) {
            tokio::fs::write(dest, &resp.body)
                .await
                .with_context(|| format!("write {}", dest.display()))?;
        }
        Ok(resp.status)
    }
}

/// Host-injected credential lookup returning source-scoped JSON or `{}`.
pub trait Credentials: Send + Sync {
    fn get(&self, source: &str) -> String;
}

/// Credential provider used where credentials are unavailable.
pub struct NoCredentials;
impl Credentials for NoCredentials {
    fn get(&self, _source: &str) -> String {
        "{}".to_string()
    }
}

/// Inert fetcher used while inspecting manifest and icon exports.
pub struct InertFetcher;
#[async_trait]
impl Fetcher for InertFetcher {
    async fn fetch(&self, _req: FetchRequest) -> Result<FetchResponse> {
        Err(anyhow!(
            "http_fetch is unavailable during plugin inspection"
        ))
    }
}

/// Database-backed source credential provider used from blocking WASM calls.
pub struct DbCredentials {
    pub read: SqlitePool,
    pub handle: Handle,
}
impl Credentials for DbCredentials {
    fn get(&self, source: &str) -> String {
        self.handle
            .block_on(repo::get_credential(&self.read, source))
            .ok()
            .flatten()
            .unwrap_or_else(|| "{}".to_string())
    }
}
/// Parsed plugin manifest used by the host and client selectors.
#[derive(Debug, Clone)]
pub struct ScraperManifest {
    /// Stable plugin identifier.
    pub id: String,
    /// Manifest schema version; `None` denotes the legacy format.
    pub manifest_version: Option<u32>,
    /// Either `strict` or `legacy`.
    pub metadata_status: String,
    /// Host-assigned provenance: `bundled`, `local`, or `community`.
    pub origin: String,
    /// Plugin release version.
    pub version: String,
    /// Plugin author or maintainer.
    pub author: String,
    /// Optional icon URL.
    pub icon: Option<String>,
    /// Optional source repository.
    pub repository: Option<String>,
    /// Display name.
    pub name: String,
    /// Short description for plugin selectors.
    pub description: Option<String>,
    /// Vendor and credential namespace.
    pub source: String,
    /// Operations implemented by the plugin.
    pub capabilities: Vec<String>,
    /// Allowed network hosts. Subdomains match; an empty list permits any public host.
    pub hosts: Vec<String>,
    /// Credential requirements, if any.
    pub auth: Option<AuthSpec>,
    /// Host-enforced vendor rate limits.
    pub rate_limit: Option<RateLimit>,
    /// Browse feeds exposed by the plugin.
    pub feeds: Vec<Feed>,
    /// Capability (`scrape`, `download`, …) to source-specific reference input.
    pub reference_inputs: std::collections::BTreeMap<String, ReferenceInput>,
    /// Browse item cache lifetime in seconds; zero disables caching.
    pub item_cache_ttl: u64,
    /// Headers added by the image proxy, such as `Referer` or `User-Agent`.
    pub image_headers: std::collections::BTreeMap<String, String>,
    /// Apply bracket-style title cleaning to browse results.
    pub clean_titles: bool,
    /// Whether the source supports following searches.
    pub followable: bool,
    /// Source reader mode: `paged` or `vertical`.
    pub reading_mode: String,
    /// Whether the source serves adult content.
    pub nsfw: bool,
    /// Contract version this scraper targets (see [`CONTRACT_VERSION`]).
    pub contract_version: u32,
}

/// Return missing descriptive metadata that does not invalidate the manifest.
pub fn manifest_lint(m: &ScraperManifest) -> Vec<String> {
    let mut issues = Vec::new();
    if m.name.trim().is_empty() {
        issues.push("'name' is empty".to_string());
    }
    if m.capabilities.is_empty() {
        issues.push("declares no capabilities".to_string());
    }
    if let Some(auth) = &m.auth {
        if auth.scheme.trim().is_empty() {
            issues.push("auth 'scheme' is empty".to_string());
        }
        for f in &auth.fields {
            if f.label.is_none() {
                issues.push(format!(
                    "auth field '{}' has no label (bare-string form? use the full AuthField object)",
                    f.name
                ));
            }
            if f.help.is_none() {
                issues.push(format!("auth field '{}' has no help text", f.name));
            }
        }
    }
    issues
}

#[async_trait]
pub trait MetadataScraper: Send + Sync {
    fn manifest(&self) -> ScraperManifest;

    /// Embedded store icon, if exported by the plugin.
    fn icon_bytes(&self) -> Option<&[u8]> {
        None
    }

    /// Find candidate matches for a local item.
    async fn search(&self, hint: &ScrapeHint, fetch: &dyn Fetcher) -> Result<Vec<Candidate>>;
    /// Resolve a chosen candidate to full metadata.
    async fn fetch_details(
        &self,
        candidate: &Candidate,
        fetch: &dyn Fetcher,
    ) -> Result<ScrapedMetadata>;

    /// Resolve a candidate to a download plan.
    async fn download(&self, _candidate: &Candidate, _fetch: &dyn Fetcher) -> Result<DownloadPlan> {
        Err(anyhow!("this scraper does not support download"))
    }

    /// Fetch a page of a browse feed.
    async fn browse(&self, _req: &BrowseRequest, _fetch: &dyn Fetcher) -> Result<BrowsePage> {
        Err(anyhow!("this scraper does not support browse"))
    }

    /// Fetch page image URLs for online reading.
    async fn pages(&self, _reference: &str, _fetch: &dyn Fetcher) -> Result<BrowsePages> {
        Err(anyhow!("this scraper does not support pages"))
    }

    /// Find source candidates using an item's cover.
    async fn identify(
        &self,
        _req: &IdentifyRequest,
        _fetch: &dyn Fetcher,
    ) -> Result<IdentifyResult> {
        Err(anyhow!("this scraper does not support identify"))
    }

    /// Fetch upcoming releases for linked series in one batch. Missing/errored
    /// references preserve their previous rows; successful empty results clear.
    async fn upcoming(
        &self,
        _req: &CalendarRequest,
        _fetch: &dyn Fetcher,
    ) -> Result<CalendarResponse> {
        Err(anyhow!("this scraper does not support calendar"))
    }
}

/// Hot-swappable registry of installed scrapers. Cloned handles survive uninstall.
pub struct ScraperRegistry {
    scrapers: std::sync::RwLock<Vec<Arc<dyn MetadataScraper>>>,
}

impl ScraperRegistry {
    pub fn new() -> Self {
        Self {
            scrapers: std::sync::RwLock::new(Vec::new()),
        }
    }

    pub fn ids(&self) -> Vec<String> {
        self.read().iter().map(|s| s.manifest().id).collect()
    }

    pub fn manifests(&self) -> Vec<ScraperManifest> {
        self.read().iter().map(|s| s.manifest()).collect()
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn MetadataScraper>> {
        self.read()
            .iter()
            .find(|s| s.manifest().id == id)
            .map(Arc::clone)
    }

    /// Register a scraper (boot, or a hot install). Replaces an existing entry
    /// with the same id in place, so a reinstall/upgrade swaps atomically.
    pub fn insert(&self, scraper: Arc<dyn MetadataScraper>) {
        let id = scraper.manifest().id;
        let mut list = self.scrapers.write().unwrap_or_else(|e| e.into_inner());
        if let Some(slot) = list.iter_mut().find(|s| s.manifest().id == id) {
            *slot = scraper;
        } else {
            list.push(scraper);
        }
    }

    pub fn push(&self, scraper: Box<dyn MetadataScraper>) {
        self.insert(Arc::from(scraper));
    }

    /// Drop a scraper from the roster (hot uninstall). In-flight callers keep
    /// their `Arc` until they finish. Returns whether the id was present.
    pub fn remove(&self, id: &str) -> bool {
        let mut list = self.scrapers.write().unwrap_or_else(|e| e.into_inner());
        let before = list.len();
        list.retain(|s| s.manifest().id != id);
        list.len() != before
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Vec<Arc<dyn MetadataScraper>>> {
        self.scrapers.read().unwrap_or_else(|e| e.into_inner())
    }
}

impl Default for ScraperRegistry {
    fn default() -> Self {
        Self::new()
    }
}
/// Validate and resolve one mapped tag.
async fn resolve_mapped_tag(
    write: &SqlitePool,
    source: &str,
    t: &MappedTag,
) -> Result<Option<(i64, String, String)>> {
    let namespace = t.namespace.trim().to_lowercase();
    let qualifier = t.qualifier.trim().to_lowercase();
    let role = t.role.trim().to_lowercase();
    if !repo::valid_namespace(&namespace) {
        tracing::warn!(source, namespace = %t.namespace, "scraper emitted invalid namespace; skipping tag");
        return Ok(None);
    }
    let tag_id = repo::get_or_create_tag(write, &namespace, &t.value).await?;
    Ok(Some((tag_id, qualifier, role)))
}

/// Apply source-owned tags, URL, comments, and description to an item.
pub async fn apply_scraped(
    write: &SqlitePool,
    item_id: i64,
    source: &str,
    meta: &ScrapedMetadata,
) -> Result<usize> {
    if !meta.mapped_tags.is_empty() {
        repo::clear_item_tags_from_source(write, item_id, source).await?;
    }
    let mut applied = 0usize;
    for t in &meta.mapped_tags {
        let Some((tag_id, qualifier, role)) = resolve_mapped_tag(write, source, t).await? else {
            continue;
        };
        if !repo::add_item_tag_with_role(write, item_id, tag_id, &qualifier, &role, source).await? {
            return Err(anyhow!("scrape target item not found: {item_id}"));
        }
        applied += 1;
    }

    if let Some(lang) = meta.language.as_deref().filter(|l| !l.trim().is_empty()) {
        let value = crate::media::epub::normalize_language(lang);
        if !value.is_empty() {
            let tag_id = repo::get_or_create_tag(write, "language", &value).await?;
            if !repo::add_item_tag(write, item_id, tag_id, "none", source).await? {
                return Err(anyhow!("scrape target item not found: {item_id}"));
            }
            applied += 1;
        }
    }

    if let Some(url) = meta.source_url.as_deref().filter(|u| !u.is_empty()) {
        if !repo::set_item_source(write, item_id, source, url).await? {
            return Err(anyhow!("scrape target item not found: {item_id}"));
        }
    }

    if !meta.comments.is_empty() {
        let comments: Vec<repo::ItemComment> = meta
            .comments
            .iter()
            .map(|c| repo::ItemComment {
                source: source.to_string(),
                external_id: c.external_id.clone(),
                author: c.author.clone(),
                posted_at: c.posted_at,
                score: c.score,
                body: c.body.clone(),
            })
            .collect();
        if !repo::replace_item_comments(write, item_id, source, &comments).await? {
            return Err(anyhow!("scrape target item not found: {item_id}"));
        }
    }

    if let Some(desc) = meta.description.as_deref().filter(|d| !d.is_empty()) {
        repo::set_item_description(write, item_id, desc, Some(source)).await?;
    }

    repo::reindex_item_tags(write, item_id).await?;
    Ok(applied)
}

/// Apply source-owned tags, URL, and description to a series.
pub async fn apply_scraped_series(
    write: &SqlitePool,
    series_id: i64,
    source: &str,
    meta: &ScrapedMetadata,
) -> Result<usize> {
    if !meta.mapped_tags.is_empty() {
        repo::clear_series_tags_from_source(write, series_id, source).await?;
    }
    let mut applied = 0usize;
    for t in &meta.mapped_tags {
        let Some((tag_id, qualifier, role)) = resolve_mapped_tag(write, source, t).await? else {
            continue;
        };
        if !repo::add_series_tag_with_role(write, series_id, tag_id, &qualifier, &role, source)
            .await?
        {
            return Err(anyhow!("scrape target series not found: {series_id}"));
        }
        applied += 1;
    }

    if let Some(reference) = meta
        .source_url
        .as_deref()
        .filter(|reference| !reference.is_empty())
    {
        if !repo::set_series_source(write, series_id, source, reference).await? {
            return Err(anyhow!("scrape target series not found: {series_id}"));
        }
    }
    if let Some(desc) = meta.description.as_deref().filter(|d| !d.is_empty()) {
        repo::set_series_description(write, series_id, desc, Some(source)).await?;
    }
    Ok(applied)
}

/// Search, select, fetch, and apply metadata for one item.
pub async fn run_scrape(
    write: &SqlitePool,
    scraper: &dyn MetadataScraper,
    fetch: &dyn Fetcher,
    item_id: i64,
    hint: &ScrapeHint,
) -> Result<usize> {
    let (source, meta) = resolve_scrape(scraper, fetch, hint).await?;
    apply_scraped(write, item_id, &source, &meta).await
}

/// Search, select, fetch, and apply metadata for one series.
pub async fn run_scrape_series(
    write: &SqlitePool,
    scraper: &dyn MetadataScraper,
    fetch: &dyn Fetcher,
    series_id: i64,
    hint: &ScrapeHint,
) -> Result<usize> {
    let (source, meta) = resolve_scrape(scraper, fetch, hint).await?;
    apply_scraped_series(write, series_id, &source, &meta).await
}

/// Resolve the best candidate for a scrape hint.
async fn resolve_scrape(
    scraper: &dyn MetadataScraper,
    fetch: &dyn Fetcher,
    hint: &ScrapeHint,
) -> Result<(String, ScrapedMetadata)> {
    let candidates = scraper.search(hint, fetch).await?;
    let best = candidates
        .into_iter()
        .max_by(|a, b| {
            a.score
                .partial_cmp(&b.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .ok_or_else(|| anyhow!("no candidate match for {:?}", hint.title))?;
    let meta = scraper.fetch_details(&best, fetch).await?;
    Ok((scraper.manifest().id, meta))
}

static DOWNLOAD_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// What a completed download produced.
pub struct DownloadOutcome {
    pub id: i64,
    /// `false` means an identical archive was already in the library (deduped).
    pub created: bool,
    /// In-vocabulary tags applied from the scraped metadata.
    pub applied: usize,
}

/// Download through the guarded host fetcher, ingest, and apply source metadata.
#[allow(clippy::too_many_arguments)]
pub async fn run_download(
    write: &SqlitePool,
    read: &SqlitePool,
    content_dir: &Path,
    scraper: &dyn MetadataScraper,
    fetch: &dyn Fetcher,
    reference: &str,
    kind: &str,
) -> Result<DownloadOutcome> {
    let candidate = Candidate {
        id: reference.to_string(),
        title: String::new(),
        score: 1.0,
    };
    let plan = scraper.download(&candidate, fetch).await?;
    if plan.url.is_empty() {
        return Err(anyhow!("plugin returned no download URL for {reference:?}"));
    }
    let seq = DOWNLOAD_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = content_dir.join(format!(".arca-download-{seq}.tmp"));
    let status = fetch
        .fetch_to_file(
            FetchRequest {
                method: "GET".to_string(),
                url: plan.url.clone(),
                headers: plan.headers.clone(),
                body: Vec::new(),
            },
            &temp,
        )
        .await?;
    if !(200..300).contains(&status) {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(anyhow!(
            "download fetch returned HTTP {} for {}",
            status,
            redact_url(&plan.url)
        ));
    }
    let now = crate::now_secs();
    let source = scraper.manifest().id;
    let result = library::ingest_committed_temp(
        read,
        write,
        content_dir,
        kind,
        &temp,
        Some(&plan.filename),
        now,
    )
    .await
    .map_err(|e| anyhow!("ingest download: {e}"))?;

    let applied = apply_scraped(write, result.id, &source, &plan.metadata).await?;
    Ok(DownloadOutcome {
        id: result.id,
        created: result.created,
        applied,
    })
}
/// Sliding-window request gate allowing bursts up to the declared capacity.
struct RuleGate {
    match_pattern: String,
    recent: tokio::sync::Mutex<std::collections::VecDeque<tokio::time::Instant>>,
    capacity: usize,
    window: Duration,
}

/// Shared vendor concurrency limit and endpoint-specific rate gates.
struct VendorGate {
    sem: Arc<tokio::sync::Semaphore>,
    rules: Vec<RuleGate>,
}

impl VendorGate {
    fn new(policy: &RateLimit) -> Self {
        let rules = policy
            .rules
            .iter()
            .map(|r| RuleGate {
                match_pattern: r.match_pattern.clone(),
                recent: tokio::sync::Mutex::new(std::collections::VecDeque::new()),
                capacity: r.requests.max(1) as usize,
                window: Duration::from_millis(r.per_ms),
            })
            .collect();
        VendorGate {
            sem: Arc::new(tokio::sync::Semaphore::new(policy.concurrency())),
            rules,
        }
    }

    /// Reserve a concurrency permit and any matching rate-limit slot.
    async fn acquire(&self, url: &str) -> RateGuard {
        let permit = self.sem.clone().acquire_owned().await.ok();
        if let Some(rule) = self.rules.iter().find(|r| url.contains(&r.match_pattern)) {
            let start_at = {
                let mut recent = rule.recent.lock().await;
                let now = tokio::time::Instant::now();
                while let Some(&front) = recent.front() {
                    if now.duration_since(front) >= rule.window {
                        recent.pop_front();
                    } else {
                        break;
                    }
                }
                let start = if recent.len() < rule.capacity {
                    now
                } else {
                    let front = *recent.front().expect("full window ⇒ non-empty");
                    (front + rule.window).max(now)
                };
                if recent.len() >= rule.capacity {
                    recent.pop_front();
                }
                recent.push_back(start);
                start
            };
            tokio::time::sleep_until(start_at).await;
        }
        RateGuard { _permit: permit }
    }
}

/// Holds a vendor concurrency permit for the request lifetime.
struct RateGuard {
    _permit: Option<tokio::sync::OwnedSemaphorePermit>,
}

/// Host enforcement for manifest-declared vendor rate limits.
#[derive(Default)]
pub struct RateLimiter {
    /// Host suffix to source routes; the longest match wins.
    routes: RwLock<Vec<(String, String)>>,
    policies: RwLock<HashMap<String, RateLimit>>,
    gates: StdMutex<HashMap<String, Arc<VendorGate>>>,
}

impl RateLimiter {
    /// Register or replace a source's rate-limit policy.
    pub fn register(&self, source: &str, hosts: &[String], policy: RateLimit) {
        self.policies
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(source.to_string(), policy);
        self.gates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(source);
        let mut routes = self.routes.write().unwrap_or_else(|e| e.into_inner());
        for h in hosts {
            let suffix = h.trim().to_ascii_lowercase();
            if suffix.is_empty() {
                continue;
            }
            if !routes.iter().any(|(s, src)| s == &suffix && src == source) {
                routes.push((suffix, source.to_string()));
            }
        }
    }

    fn source_for(&self, req_host: &str) -> Option<String> {
        let host = req_host.to_ascii_lowercase();
        self.routes
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(suffix, _)| host == *suffix || host.ends_with(&format!(".{suffix}")))
            .max_by_key(|(suffix, _)| suffix.len())
            .map(|(_, source)| source.clone())
    }

    /// Acquire the rate-limit guard for a URL, if configured.
    async fn acquire(&self, url: &str) -> RateGuard {
        let source = match host_of(url).and_then(|h| self.source_for(&h)) {
            Some(s) => s,
            None => return RateGuard { _permit: None },
        };
        let gate = {
            let mut gates = self.gates.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(g) = gates.get(&source) {
                g.clone()
            } else {
                let policies = self.policies.read().unwrap_or_else(|e| e.into_inner());
                let policy = policies.get(&source).cloned().unwrap_or(RateLimit {
                    rules: Vec::new(),
                    max_concurrency: 0,
                });
                let g = Arc::new(VendorGate::new(&policy));
                gates.insert(source, g.clone());
                g
            }
        };
        gate.acquire(url).await
    }
}

/// Production fetcher with timeouts, anti-SSRF resolution, and rate limiting.
pub struct HttpFetcher {
    client: reqwest::Client,
    limiter: Arc<RateLimiter>,
    /// False only for explicitly allowed private repository fetches.
    guard: bool,
}

/// Return a request URL's lowercase host.
pub(crate) fn host_of(url: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .ok()?
        .host_str()
        .map(|h| h.to_ascii_lowercase())
}

/// Strip query and fragment data before logging a URL.
pub(crate) fn redact_url(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(mut u) => {
            u.set_query(None);
            u.set_fragment(None);
            u.to_string()
        }
        Err(_) => "<unparseable url>".to_string(),
    }
}

/// Reject private, loopback, link-local, metadata, CGNAT, and equivalent IPv6 addresses.
fn is_blocked_addr(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.octets()[0] == 0
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64) // 100.64/10 CGNAT
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_blocked_addr(IpAddr::V4(v4));
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // fc00::/7 unique-local
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // fe80::/10 link-local
        }
    }
}

/// DNS resolver that removes forbidden addresses before reqwest connects.
struct PublicOnlyResolver;

impl reqwest::dns::Resolve for PublicOnlyResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let host = name.as_str().to_owned();
            let safe: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?
                .filter(|a| !is_blocked_addr(a.ip()))
                .collect();
            if safe.is_empty() {
                return Err("SSRF guard: host resolves only to private/loopback addresses".into());
            }
            Ok(Box::new(safe.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

impl HttpFetcher {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .user_agent(concat!("arcagrad/", env!("CARGO_PKG_VERSION")))
            .timeout(std::time::Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .dns_resolver(Arc::new(PublicOnlyResolver))
            .build()
            .expect("a reqwest client with these static options cannot fail to build");
        Self {
            client,
            limiter: Arc::new(RateLimiter::default()),
            guard: true,
        }
    }

    /// Allow private addresses for operator-configured repositories only.
    /// Never expose this fetcher to plugin code.
    pub fn unguarded() -> Self {
        let client = reqwest::Client::builder()
            .user_agent(concat!("arcagrad/", env!("CARGO_PKG_VERSION")))
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .expect("a reqwest client with these static options cannot fail to build");
        Self {
            client,
            limiter: Arc::new(RateLimiter::default()),
            guard: false,
        }
    }

    pub fn limiter(&self) -> Arc<RateLimiter> {
        self.limiter.clone()
    }
}

impl Default for HttpFetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpFetcher {
    /// Validate an HTTP(S) URL and build a request with SSRF protection.
    fn build(&self, req: &FetchRequest) -> Result<reqwest::RequestBuilder> {
        let url = reqwest::Url::parse(&req.url).map_err(|e| anyhow!("invalid url: {e}"))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(anyhow!("scheme not allowed: {}", url.scheme()));
        }
        if self.guard {
            if let Some(host) = url.host_str() {
                let bare = host.trim_start_matches('[').trim_end_matches(']');
                if let Ok(ip) = bare.parse::<IpAddr>() {
                    if is_blocked_addr(ip) {
                        return Err(anyhow!("blocked address: {ip}"));
                    }
                }
            }
        }
        let method = reqwest::Method::from_bytes(req.method.as_bytes())
            .map_err(|e| anyhow!("bad HTTP method {:?}: {e}", req.method))?;
        let mut builder = self.client.request(method, &req.url);
        for (name, value) in &req.headers {
            builder = builder.header(name, value);
        }
        if !req.body.is_empty() {
            builder = builder.body(req.body.clone());
        }
        Ok(builder)
    }
}

/// Maximum in-memory upstream response body.
const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

/// Maximum streamed archive download size.
const MAX_DOWNLOAD_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// Append a chunk or fail before exceeding the body limit.
fn push_capped(body: &mut Vec<u8>, chunk: &[u8], limit: usize) -> Result<()> {
    if body.len() + chunk.len() > limit {
        return Err(anyhow!(
            "response body exceeded the {limit}-byte cap (possible hostile or runaway upstream)"
        ));
    }
    body.extend_from_slice(chunk);
    Ok(())
}

/// Buffer a response up to a fixed limit without returning truncated data.
async fn read_capped(mut resp: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        push_capped(&mut body, &chunk, limit)?;
    }
    Ok(body)
}

#[async_trait]
impl Fetcher for HttpFetcher {
    async fn fetch(&self, req: FetchRequest) -> Result<FetchResponse> {
        let _guard = self.limiter.acquire(&req.url).await;
        let resp = self.build(&req)?.send().await?;
        let status = resp.status().as_u16();
        let body = read_capped(resp, MAX_RESPONSE_BYTES).await?;
        Ok(FetchResponse { status, body })
    }

    async fn fetch_asset(&self, req: FetchRequest) -> Result<(u16, Option<String>, Vec<u8>)> {
        let resp = self.build(&req)?.send().await?;
        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let body = read_capped(resp, MAX_RESPONSE_BYTES).await?;
        Ok((status, content_type, body))
    }

    /// Stream a successful response to disk, enforcing `MAX_DOWNLOAD_BYTES`.
    async fn fetch_to_file(&self, req: FetchRequest, dest: &Path) -> Result<u16> {
        let _guard = self.limiter.acquire(&req.url).await;
        let mut resp = self.build(&req)?.send().await?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            return Ok(status);
        }
        let mut file = tokio::fs::File::create(dest)
            .await
            .with_context(|| format!("create download temp {}", dest.display()))?;
        let mut written: u64 = 0;
        while let Some(chunk) = resp.chunk().await? {
            written += chunk.len() as u64;
            if written > MAX_DOWNLOAD_BYTES {
                drop(file);
                let _ = tokio::fs::remove_file(dest).await;
                return Err(anyhow!(
                    "download exceeded the {MAX_DOWNLOAD_BYTES}-byte cap (possible runaway or hostile upstream)"
                ));
            }
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        Ok(status)
    }
}

/// Hermetic scraper used by contract and pipeline tests.
pub struct ExampleScraper {
    base: String,
}

impl ExampleScraper {
    pub fn new() -> Self {
        Self {
            base: "https://example.invalid".to_string(),
        }
    }
}

impl Default for ExampleScraper {
    fn default() -> Self {
        Self::new()
    }
}

/// Percent-encode a query value.
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[async_trait]
impl MetadataScraper for ExampleScraper {
    fn manifest(&self) -> ScraperManifest {
        ScraperManifest {
            id: "example".to_string(),
            manifest_version: Some(1),
            metadata_status: "strict".to_string(),
            origin: "bundled".to_string(),
            version: "1.0.0".to_string(),
            author: "arcagrad tests".to_string(),
            icon: None,
            repository: None,
            name: "Example (reference)".to_string(),
            description: Some("Reference scraper — demonstrates the contract.".to_string()),
            source: "example".to_string(),
            capabilities: vec![
                "scrape".to_string(),
                "browse".to_string(),
                "read".to_string(),
            ],
            hosts: vec!["example.test".to_string()],
            auth: None,
            rate_limit: None,
            feeds: vec![Feed {
                id: "popular".into(),
                label: "Popular".into(),
                ranges: vec!["today".into(), "week".into()],
                query: true,
                auth: false,
                cache_ttl: 300,
            }],
            reference_inputs: std::collections::BTreeMap::new(),
            item_cache_ttl: 300,
            image_headers: std::collections::BTreeMap::new(),
            clean_titles: true,
            followable: true,
            reading_mode: "paged".to_string(),
            nsfw: false,
            contract_version: CONTRACT_VERSION,
        }
    }

    async fn search(&self, hint: &ScrapeHint, fetch: &dyn Fetcher) -> Result<Vec<Candidate>> {
        let url = format!("{}/search?q={}", self.base, enc(&hint.title));
        let resp = fetch.fetch(FetchRequest::get(url)).await?;
        if resp.status != 200 {
            return Err(anyhow!("search returned HTTP {}", resp.status));
        }
        #[derive(Deserialize)]
        struct SearchBody {
            results: Vec<Candidate>,
        }
        let body: SearchBody = serde_json::from_slice(&resp.body)?;
        Ok(body.results)
    }

    async fn fetch_details(
        &self,
        candidate: &Candidate,
        fetch: &dyn Fetcher,
    ) -> Result<ScrapedMetadata> {
        let url = format!("{}/g/{}", self.base, enc(&candidate.id));
        let resp = fetch.fetch(FetchRequest::get(url)).await?;
        if resp.status != 200 {
            return Err(anyhow!("details returned HTTP {}", resp.status));
        }
        #[derive(Deserialize)]
        struct DetailBody {
            title: Option<String>,
            language: Option<String>,
            #[serde(default)]
            tags: Vec<MappedTag>,
            #[serde(default)]
            raw: Vec<RawTag>,
        }
        let body: DetailBody = serde_json::from_slice(&resp.body)?;
        Ok(ScrapedMetadata {
            title: body.title,
            language: body.language,
            source_url: Some(format!("{}/g/{}", self.base, candidate.id)),
            mapped_tags: body.tags,
            raw_tags: body.raw,
            ..Default::default()
        })
    }

    async fn download(&self, candidate: &Candidate, _fetch: &dyn Fetcher) -> Result<DownloadPlan> {
        Ok(DownloadPlan {
            url: format!("{}/dl/{}", self.base, candidate.id),
            filename: format!("{}.cbz", candidate.id),
            headers: vec![("X-Test-Auth".into(), "example-cookie".into())],
            metadata: ScrapedMetadata {
                title: Some(format!("Downloaded {}", candidate.id)),
                source_url: Some(format!("{}/g/{}", self.base, candidate.id)),
                mapped_tags: vec![MappedTag {
                    namespace: "creator".into(),
                    value: "Wada Rco".into(),
                    qualifier: "none".into(),
                    role: "none".into(),
                }],
                ..Default::default()
            },
        })
    }

    async fn browse(&self, req: &BrowseRequest, _fetch: &dyn Fetcher) -> Result<BrowsePage> {
        let tag = format!(
            "{}/{}/{}/{}",
            req.feed,
            req.range.as_deref().unwrap_or("-"),
            req.query.as_deref().unwrap_or("-"),
            req.page
        );
        Ok(BrowsePage {
            items: vec![
                BrowseItem {
                    reference: "ex-1".into(),
                    title: format!("Example {tag}"),
                    cover_url: "https://example.test/1.jpg".into(),
                    page_count: Some(12),
                    favorites: Some(99),
                    rating: None,
                    subtitle: None,
                    source_url: Some("https://example.test/g/ex-1".into()),
                },
                BrowseItem {
                    reference: "ex-2".into(),
                    title: "Example two".into(),
                    cover_url: "https://example.test/2.jpg".into(),
                    page_count: None,
                    favorites: None,
                    rating: None,
                    subtitle: None,
                    source_url: None,
                },
            ],
            num_pages: Some(5),
        })
    }

    async fn pages(&self, reference: &str, _fetch: &dyn Fetcher) -> Result<BrowsePages> {
        Ok(BrowsePages {
            pages: (1..=2)
                .map(|n| BrowsePageInfo {
                    number: n,
                    image_url: format!("https://example.test/{reference}/{n}.jpg"),
                    thumb_url: format!("https://example.test/{reference}/{n}t.jpg"),
                    width: 800,
                    height: 1200,
                })
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_capabilities_match_the_sdk_claimable_set() {
        assert_eq!(SUPPORTED_CAPABILITIES, arcagrad_plugin_sdk::CAPABILITIES);
    }

    #[test]
    fn push_capped_accumulates_then_aborts() {
        let mut body = Vec::new();
        push_capped(&mut body, b"hello", 10).unwrap();
        push_capped(&mut body, b"!", 10).unwrap();
        assert_eq!(body, b"hello!");
        let err = push_capped(&mut body, b"world", 10);
        assert!(err.is_err(), "over-limit chunk must error");
        assert_eq!(body, b"hello!", "body unchanged on the rejected chunk");
        let mut exact = Vec::new();
        push_capped(&mut exact, &[0u8; 8], 8).unwrap();
        assert!(push_capped(&mut exact, &[0u8; 1], 8).is_err());
    }

    fn blanket(requests: u32, per_ms: u64, conc: usize) -> RateLimit {
        RateLimit {
            rules: vec![RateRule {
                match_pattern: String::new(),
                requests,
                per_ms,
            }],
            max_concurrency: conc,
        }
    }

    #[test]
    fn rate_limiter_routes_hosts_to_source_by_longest_suffix() {
        let rl = RateLimiter::default();
        rl.register(
            "openlibrary",
            &["openlibrary.org".into()],
            blanket(60, 60000, 1),
        );
        rl.register(
            "special",
            &["cdn.openlibrary.org".into()],
            blanket(60, 60000, 1),
        );

        assert_eq!(
            rl.source_for("openlibrary.org").as_deref(),
            Some("openlibrary")
        );
        assert_eq!(
            rl.source_for("i.openlibrary.org").as_deref(),
            Some("openlibrary")
        );
        assert_eq!(
            rl.source_for("I2.Openlibrary.Org").as_deref(),
            Some("openlibrary")
        );
        assert_eq!(
            rl.source_for("cdn.openlibrary.org").as_deref(),
            Some("special")
        );
        assert_eq!(rl.source_for("notopenlibrary.org"), None);
        assert_eq!(rl.source_for("evil.example"), None);
    }

    #[tokio::test]
    async fn sliding_window_bursts_then_throttles() {
        let rl = RateLimiter::default();
        rl.register(
            "v",
            &["v.test".into()],
            RateLimit {
                rules: vec![RateRule {
                    match_pattern: String::new(),
                    requests: 3,
                    per_ms: 200,
                }],
                max_concurrency: 3,
            },
        );
        let url = "https://v.test/x";

        let t = std::time::Instant::now();
        drop(rl.acquire(url).await);
        drop(rl.acquire(url).await);
        drop(rl.acquire(url).await);
        assert!(
            t.elapsed() < Duration::from_millis(50),
            "3 calls should burst with no wait, got {:?}",
            t.elapsed()
        );

        let t = std::time::Instant::now();
        drop(rl.acquire(url).await);
        assert!(
            t.elapsed() >= Duration::from_millis(180),
            "4th call throttled by the window, got {:?}",
            t.elapsed()
        );
    }

    #[tokio::test]
    async fn per_endpoint_rules_space_independently() {
        let rl = RateLimiter::default();
        rl.register(
            "v",
            &["v.test".into()],
            RateLimit {
                rules: vec![
                    RateRule {
                        match_pattern: "/slow".into(),
                        requests: 1,
                        per_ms: 120,
                    },
                    RateRule {
                        match_pattern: String::new(),
                        requests: 1,
                        per_ms: 60,
                    },
                ],
                max_concurrency: 1,
            },
        );
        let fast = "https://v.test/fast";
        let slow = "https://v.test/slow";

        let t = std::time::Instant::now();
        drop(rl.acquire(fast).await);
        drop(rl.acquire(fast).await);
        assert!(
            t.elapsed() >= Duration::from_millis(55),
            "fast endpoint spaced by its own 60ms rule, got {:?}",
            t.elapsed()
        );

        let t = std::time::Instant::now();
        drop(rl.acquire(slow).await);
        drop(rl.acquire(slow).await);
        assert!(
            t.elapsed() >= Duration::from_millis(110),
            "slow endpoint spaced by its own 120ms rule, got {:?}",
            t.elapsed()
        );

        let t = std::time::Instant::now();
        let _g = rl.acquire("https://other.test/x").await;
        assert!(
            t.elapsed() < Duration::from_millis(40),
            "unknown host not delayed"
        );
    }

    #[test]
    fn manifest_lint_flags_underdescribed_auth() {
        let mk = |auth_json: &str| {
            let auth: AuthSpec = serde_json::from_str(auth_json).unwrap();
            ScraperManifest {
                id: "p".into(),
                manifest_version: None,
                metadata_status: "legacy".into(),
                version: "0.0.0".into(),
                origin: "local".into(),
                author: "Unknown".into(),
                icon: None,
                repository: None,
                name: "P".into(),
                description: None,
                source: "p".into(),
                capabilities: vec!["scrape".into()],
                hosts: vec![],
                auth: Some(auth),
                rate_limit: None,
                feeds: Vec::new(),
                reference_inputs: std::collections::BTreeMap::new(),
                item_cache_ttl: 0,
                image_headers: std::collections::BTreeMap::new(),
                clean_titles: true,
                followable: true,
                reading_mode: "paged".to_string(),
                nsfw: false,
                contract_version: CONTRACT_VERSION,
            }
        };
        let bare = mk(r#"{"scheme":"api_key","fields":["api_key"]}"#);
        assert_eq!(
            manifest_lint(&bare).len(),
            2,
            "bare field flagged for label + help"
        );
        let rich = mk(
            r#"{"scheme":"api_key","fields":[{"name":"api_key","label":"API key","help":"where to find it"}]}"#,
        );
        assert!(manifest_lint(&rich).is_empty(), "fully-described is clean");
    }

    struct StubFetcher;

    fn fake_cbz() -> Vec<u8> {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default();
            z.start_file("001.jpg", opts).unwrap();
            z.write_all(b"dummy-jpeg").unwrap();
            z.finish().unwrap();
        }
        buf
    }

    #[async_trait]
    impl Fetcher for StubFetcher {
        async fn fetch(&self, req: FetchRequest) -> Result<FetchResponse> {
            if req.url.contains("/dl/") {
                return Ok(FetchResponse {
                    status: 200,
                    body: fake_cbz(),
                });
            }
            let body: &[u8] = if req.url.contains("/search") {
                br#"{"results":[{"id":"42","title":"Test Comic","score":0.9}]}"#
            } else if req.url.contains("/g/42") {
                br#"{"title":"Test Comic","language":"english",
                     "tags":[{"namespace":"creator","value":"Wada Rco","qualifier":"none"},
                             {"namespace":"tag","value":"Mystery","qualifier":"female"}],
                     "raw":[{"namespace":"female","value":"mystery"}]}"#
            } else {
                return Ok(FetchResponse {
                    status: 404,
                    body: Vec::new(),
                });
            };
            Ok(FetchResponse {
                status: 200,
                body: body.to_vec(),
            })
        }
    }

    #[tokio::test]
    async fn example_scraper_searches_and_fetches_via_injected_fetcher() {
        let scraper = ExampleScraper::new();
        let fetch = StubFetcher;

        let cands = scraper
            .search(
                &ScrapeHint {
                    title: "test".into(),
                    display_title: None,
                    author: None,
                    modality: None,
                    page_count: Some(20),
                    reference: None,
                },
                &fetch,
            )
            .await
            .unwrap();
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].id, "42");

        let meta = scraper.fetch_details(&cands[0], &fetch).await.unwrap();
        assert_eq!(meta.title.as_deref(), Some("Test Comic"));
        assert_eq!(meta.language.as_deref(), Some("english"));
        assert_eq!(meta.mapped_tags.len(), 2);
        assert_eq!(
            meta.raw_tags,
            vec![RawTag {
                namespace: "female".into(),
                value: "mystery".into(),
            }]
        );
    }

    #[sqlx::test]
    async fn run_scrape_end_to_end_applies_tags(pool: SqlitePool) {
        let hash = "cd".repeat(32);
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO items \
             (scheme_tag, structural_hash, path, size_bytes, mtime, format, title, page_count, added_at, last_modified_at) \
             VALUES ('zip-structural-v1', ?, ?, 1, 1, 'cbz', 'Test Comic', 5, 0, 0) RETURNING id",
        )
        .bind(&hash)
        .bind(format!("/p/{hash}"))
        .fetch_one(&pool)
        .await
        .unwrap();

        let applied = run_scrape(
            &pool,
            &ExampleScraper::new(),
            &StubFetcher,
            id,
            &ScrapeHint {
                title: "Test Comic".into(),
                display_title: None,
                author: None,
                modality: None,
                page_count: Some(20),
                reference: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            applied, 3,
            "search → fetch_details → both in-vocab tags + the scalar language applied"
        );

        let item_tags = repo::tags_for_item(&pool, id).await.unwrap();
        assert!(item_tags.iter().any(|t| t.namespace == "creator"
            && t.value == "wada rco"
            && t.sources == ["example"]));
        assert!(
            item_tags
                .iter()
                .any(|t| t.namespace == "language" && t.value == "english"),
            "the scalar language from fetch_details became a language tag"
        );
    }

    #[sqlx::test]
    async fn run_download_ingests_a_new_item_and_applies_tags(pool: SqlitePool) {
        let content = tempfile::tempdir().unwrap();
        let outcome = run_download(
            &pool,
            &pool,
            content.path(),
            &ExampleScraper::new(),
            &StubFetcher,
            "42",
            "manga",
        )
        .await
        .unwrap();
        assert!(outcome.created, "a new item was created");
        assert_eq!(outcome.applied, 1, "the in-vocab creator tag applied");

        let meta = repo::item_meta(&pool, outcome.id).await.unwrap().unwrap();
        assert_eq!(meta.kind, "manga");
        assert!(content.path().join("manga").join("42.cbz").exists());
        let tags = repo::tags_for_item(&pool, outcome.id).await.unwrap();
        assert!(tags
            .iter()
            .any(|t| t.namespace == "creator" && t.value == "wada rco"));

        let again = run_download(
            &pool,
            &pool,
            content.path(),
            &ExampleScraper::new(),
            &StubFetcher,
            "42",
            "manga",
        )
        .await
        .unwrap();
        assert!(!again.created, "identical re-download is a dedup");
        assert_eq!(again.id, outcome.id);
    }

    #[sqlx::test]
    async fn run_download_forwards_plan_headers_to_the_file_fetch(pool: SqlitePool) {
        struct CapturingFetcher(std::sync::Mutex<Vec<(String, String)>>);
        #[async_trait]
        impl Fetcher for CapturingFetcher {
            async fn fetch(&self, req: FetchRequest) -> Result<FetchResponse> {
                *self.0.lock().unwrap() = req.headers.clone();
                Ok(FetchResponse {
                    status: 200,
                    body: fake_cbz(),
                })
            }
        }
        let content = tempfile::tempdir().unwrap();
        let fetch = CapturingFetcher(std::sync::Mutex::new(Vec::new()));
        run_download(
            &pool,
            &pool,
            content.path(),
            &ExampleScraper::new(),
            &fetch,
            "42",
            "manga",
        )
        .await
        .unwrap();
        let seen = fetch.0.lock().unwrap().clone();
        assert!(
            seen.iter()
                .any(|(k, v)| k == "X-Test-Auth" && v == "example-cookie"),
            "the plan's auth headers reached the file fetch: {seen:?}"
        );
    }

    #[test]
    fn is_blocked_addr_catches_internal_ranges() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "192.168.1.1",
            "172.16.0.1",
            "169.254.169.254",
            "0.0.0.0",
            "100.64.0.1",
            "::1",
            "fe80::1",
            "fc00::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(
                is_blocked_addr(ip.parse().unwrap()),
                "{ip} should be blocked"
            );
        }
        for ip in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] {
            assert!(
                !is_blocked_addr(ip.parse().unwrap()),
                "{ip} should be allowed"
            );
        }
    }

    #[tokio::test]
    async fn http_fetcher_preflight_blocks_ssrf() {
        let f = HttpFetcher::new();
        for url in [
            "http://127.0.0.1:9/",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::1]/",
            "http://10.0.0.1/",
        ] {
            assert!(
                f.fetch(FetchRequest::get(url)).await.is_err(),
                "{url} must be blocked"
            );
        }
        assert!(f
            .fetch(FetchRequest::get("file:///etc/passwd"))
            .await
            .is_err());
    }

    #[test]
    fn unguarded_fetcher_allows_private_ips_but_keeps_scheme_check() {
        let f = HttpFetcher::unguarded();
        for url in ["http://192.168.1.50:8000/index.json", "http://127.0.0.1:9/"] {
            assert!(
                f.build(&FetchRequest::get(url)).is_ok(),
                "{url} should build unguarded"
            );
        }
        assert!(f.build(&FetchRequest::get("file:///etc/passwd")).is_err());
    }

    #[test]
    fn registry_push_and_lookup() {
        let reg = ScraperRegistry::new();
        assert!(reg.ids().is_empty());
        reg.push(Box::new(ExampleScraper::new()));
        assert_eq!(reg.ids(), vec!["example".to_string()]);
        assert!(reg.get("example").is_some());
        assert!(reg.get("nope").is_none());
    }

    #[sqlx::test]
    async fn apply_scraped_lands_in_the_tag_pipeline(pool: SqlitePool) {
        let hash = "ab".repeat(32);
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO items \
             (scheme_tag, structural_hash, path, size_bytes, mtime, format, title, page_count, added_at, last_modified_at) \
             VALUES ('zip-structural-v1', ?, ?, 1, 1, 'cbz', 'orig', 5, 0, 0) RETURNING id",
        )
        .bind(&hash)
        .bind(format!("/p/{hash}"))
        .fetch_one(&pool)
        .await
        .unwrap();

        let meta = ScrapedMetadata {
            title: Some("Test Comic".into()),
            language: Some("english".into()),
            source_url: Some("https://openlibrary.org/works/OL1W".into()),
            description: Some("A <i>gripping</i> read.".into()),
            mapped_tags: vec![
                MappedTag {
                    namespace: "creator".into(),
                    value: "Wada Rco".into(),
                    qualifier: "none".into(),
                    role: "none".into(),
                },
                MappedTag {
                    namespace: "demographic".into(),
                    value: "Seinen".into(),
                    qualifier: "none".into(),
                    role: "none".into(),
                },
                MappedTag {
                    namespace: "genre".into(),
                    value: "x".into(),
                    qualifier: "none".into(),
                    role: "none".into(),
                },
            ],
            raw_tags: vec![RawTag {
                namespace: "female".into(),
                value: "mystery".into(),
            }],
            comments: vec![ScrapedComment {
                external_id: "c1".into(),
                author: "Fatesifaeve".into(),
                posted_at: Some(1_735_141_500),
                score: Some(54),
                body: "<a href=\"/g/2/\">You are here!</a>".into(),
            }],
            ..Default::default()
        };

        let applied = apply_scraped(&pool, id, "example", &meta).await.unwrap();
        assert_eq!(
            applied, 3,
            "creator + demographic + language (from the scalar); the out-of-set `genre` is skipped"
        );

        let desc: Option<String> = sqlx::query_scalar("SELECT description FROM items WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(desc.as_deref(), Some("A <i>gripping</i> read."));

        let comments = repo::item_comments(&pool, id).await.unwrap();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].author, "Fatesifaeve");
        assert_eq!(comments[0].score, Some(54));
        assert_eq!(comments[0].body, "<a href=\"/g/2/\">You are here!</a>");

        let item_tags = repo::tags_for_item(&pool, id).await.unwrap();
        assert!(item_tags.iter().any(|t| t.namespace == "creator"
            && t.value == "wada rco"
            && t.sources == ["example"]));
        assert!(item_tags
            .iter()
            .any(|t| t.namespace == "language" && t.value == "english"));

        let sources = repo::item_sources(&pool, id).await.unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].source, "example");
        assert_eq!(sources[0].url, "https://openlibrary.org/works/OL1W");

        let hits = repo::list_catalog(
            &pool,
            1,
            10,
            repo::CatalogSeek::First,
            repo::Sort::default(),
            &repo::ListFilters {
                search: repo::fts_query("wada"),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(hits.items.len(), 1, "scraped tag is FTS-searchable");
    }

    #[sqlx::test]
    async fn rescrape_replaces_its_source_tags_only(pool: SqlitePool) {
        let hash = "ef".repeat(32);
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO items \
             (scheme_tag, structural_hash, path, size_bytes, mtime, format, title, page_count, added_at, last_modified_at) \
             VALUES ('zip-structural-v1', ?, ?, 1, 1, 'cbz', 'orig', 5, 0, 0) RETURNING id",
        )
        .bind(&hash)
        .bind(format!("/p/{hash}"))
        .fetch_one(&pool)
        .await
        .unwrap();

        let tag = |ns: &str, v: &str| MappedTag {
            namespace: ns.into(),
            value: v.into(),
            qualifier: "none".into(),
            role: "none".into(),
        };
        let scrape = |tags: Vec<MappedTag>| ScrapedMetadata {
            mapped_tags: tags,
            ..Default::default()
        };
        let values = |ts: &[repo::ItemTag]| {
            let mut v: Vec<String> = ts.iter().map(|t| t.value.clone()).collect();
            v.sort();
            v
        };

        let mt = repo::get_or_create_tag(&pool, "creator", "hand drawn")
            .await
            .unwrap();
        repo::add_item_tag(&pool, id, mt, "none", "manual")
            .await
            .unwrap();
        apply_scraped(
            &pool,
            id,
            "s1",
            &scrape(vec![tag("parody", "a"), tag("character", "b")]),
        )
        .await
        .unwrap();
        apply_scraped(&pool, id, "s2", &scrape(vec![tag("tag", "c")]))
            .await
            .unwrap();
        assert_eq!(
            values(&repo::tags_for_item(&pool, id).await.unwrap()),
            vec!["a", "b", "c", "hand drawn"]
        );

        apply_scraped(
            &pool,
            id,
            "s1",
            &scrape(vec![tag("parody", "a"), tag("tag", "d")]),
        )
        .await
        .unwrap();
        assert_eq!(
            values(&repo::tags_for_item(&pool, id).await.unwrap()),
            vec!["a", "c", "d", "hand drawn"],
            "s1's dropped 'b' is gone; 'd' added; s2's 'c' + the manual tag untouched"
        );

        apply_scraped(&pool, id, "s1", &scrape(vec![]))
            .await
            .unwrap();
        assert_eq!(
            values(&repo::tags_for_item(&pool, id).await.unwrap()),
            vec!["a", "c", "d", "hand drawn"],
            "empty scrape is a no-op, not a wipe"
        );
    }

    #[sqlx::test]
    async fn scalar_language_becomes_a_language_tag(pool: SqlitePool) {
        let hash = "ab".repeat(32);
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO items \
             (scheme_tag, structural_hash, path, size_bytes, mtime, format, title, page_count, added_at, last_modified_at) \
             VALUES ('zip-structural-v1', ?, ?, 1, 1, 'cbz', 'orig', 5, 0, 0) RETURNING id",
        )
        .bind(&hash)
        .bind(format!("/p/{hash}"))
        .fetch_one(&pool)
        .await
        .unwrap();

        apply_scraped(
            &pool,
            id,
            "a-publisher",
            &ScrapedMetadata {
                language: Some("en".into()),
                mapped_tags: vec![MappedTag {
                    namespace: "creator".into(),
                    value: "inio asano".into(),
                    qualifier: "none".into(),
                    role: "none".into(),
                }],
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let tags = repo::tags_for_item(&pool, id).await.unwrap();
        let lang = tags.iter().find(|t| t.namespace == "language");
        assert_eq!(
            lang.map(|t| t.value.as_str()),
            Some("english"),
            "scalar language 'en' → a normalized language:english tag"
        );
    }

    #[sqlx::test]
    async fn apply_scraped_series_lands_in_series_tables(pool: SqlitePool) {
        let series_id: i64 = sqlx::query_scalar(
            "INSERT INTO series (kind, title, folder_path, added_at) \
             VALUES ('manga', 'Punpun', 'manga/Punpun', 0) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let hash = "cd".repeat(32);
        let item_id: i64 = sqlx::query_scalar(
            "INSERT INTO items \
             (scheme_tag, structural_hash, path, size_bytes, mtime, format, title, page_count, added_at, last_modified_at, series_id) \
             VALUES ('zip-structural-v1', ?, ?, 1, 1, 'cbz', 'v01', 5, 0, 0, ?) RETURNING id",
        )
        .bind(&hash)
        .bind(format!("/p/{hash}"))
        .bind(series_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO item_series_leaf (item_id, series_id, number_sort) VALUES (?, ?, 1.0)",
        )
        .bind(item_id)
        .bind(series_id)
        .execute(&pool)
        .await
        .unwrap();

        let meta = ScrapedMetadata {
            title: Some("ignored — we never override the title".into()),
            source_url: Some("https://anilist.co/manga/34632".into()),
            description: Some("The best-laid plans of Punpun.".into()),
            mapped_tags: vec![
                MappedTag {
                    namespace: "tag".into(),
                    value: "Drama".into(),
                    qualifier: "none".into(),
                    role: "none".into(),
                },
                MappedTag {
                    namespace: "creator".into(),
                    value: "Inio Asano".into(),
                    qualifier: "none".into(),
                    role: "none".into(),
                },
                MappedTag {
                    namespace: "genre".into(),
                    value: "x".into(),
                    qualifier: "none".into(),
                    role: "none".into(),
                },
            ],
            ..Default::default()
        };

        let applied = apply_scraped_series(&pool, series_id, "anilist", &meta)
            .await
            .unwrap();
        assert_eq!(
            applied, 2,
            "tag + creator applied; out-of-set genre skipped"
        );

        let d = repo::series_detail(&pool, 1, series_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            d.description.as_deref(),
            Some("The best-laid plans of Punpun.")
        );

        let tags = repo::series_tags(&pool, series_id).await.unwrap();
        assert!(tags
            .iter()
            .any(|t| t.namespace == "tag" && t.value == "drama"));
        assert!(tags
            .iter()
            .any(|t| t.namespace == "creator" && t.value == "inio asano"));

        let sources = repo::series_sources(&pool, series_id).await.unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].source, "anilist");
        assert_eq!(sources[0].url, "https://anilist.co/manga/34632");

        assert!(apply_scraped_series(&pool, 999_999, "anilist", &meta)
            .await
            .is_err());
    }
}
