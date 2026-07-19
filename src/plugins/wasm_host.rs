//! Sandboxed Extism host for scraper plugins.

use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use extism::{host_fn, Function, Manifest, Plugin, UserData, Wasm, PTR};
use serde::Deserialize;
use tokio::runtime::Handle;

use crate::plugins::scraper::{
    AuthSpec, CalendarRequest, CalendarResponse, Candidate, Credentials, FetchRequest, Fetcher,
    HttpFetchRequest, HttpFetchResponse, MetadataScraper, RateLimit, ScrapeHint, ScrapedMetadata,
    ScraperManifest, CONTRACT_VERSION,
};

/// Plugin artifacts embedded in the server binary.
const BUNDLED: &[(&str, &[u8])] = &[
    (
        "anilist",
        include_bytes!("../../plugins/anilist/anilist.wasm"),
    ),
    (
        "comicvine",
        include_bytes!("../../plugins/comicvine/comicvine.wasm"),
    ),
    (
        "openlibrary",
        include_bytes!("../../plugins/openlibrary/openlibrary.wasm"),
    ),
    (
        "gutenberg",
        include_bytes!("../../plugins/gutenberg/gutenberg.wasm"),
    ),
    (
        "marxists",
        include_bytes!("../../plugins/marxists/marxists.wasm"),
    ),
    ("viz", include_bytes!("../../plugins/viz/viz.wasm")),
];

struct FetchCap {
    fetcher: Arc<dyn Fetcher>,
    handle: Handle,
    allowed_hosts: Arc<OnceLock<Vec<String>>>,
}

/// Match a host exactly or as a subdomain. An empty list permits any public host.
pub(crate) fn host_allowed(allowed: &[String], host: &str) -> bool {
    if allowed.is_empty() {
        return true;
    }
    allowed.iter().any(|p| {
        let p = p.trim_start_matches("*.");
        host == p || host.ends_with(&format!(".{p}"))
    })
}

struct CredCap {
    credentials: Arc<dyn Credentials>,
    /// Host-bound source; the guest cannot select another credential namespace.
    bound_source: Arc<OnceLock<String>>,
}

host_fn!(http_fetch(user_data: FetchCap; request: String) -> String {
    let cap = user_data.get()?;
    let cap = cap.lock().unwrap();
    let req: HttpFetchRequest = serde_json::from_str(&request)?;
    let allowed = cap.allowed_hosts.get().map(Vec::as_slice).unwrap_or(&[]);
    let host = reqwest::Url::parse(&req.url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned));
    match host {
        Some(h) if host_allowed(allowed, &h) => {}
        Some(h) => return Err(anyhow!("plugin not allowed to fetch host '{h}'")),
        None => return Err(anyhow!("invalid url")),
    }
    let url = req.url.clone();
    let resp = cap.handle.block_on(cap.fetcher.fetch(FetchRequest {
        method: req.method,
        url: req.url,
        headers: req.headers.into_iter().collect(),
        body: req.body.into_bytes(),
    }));
    let wire = match resp {
        Ok(resp) => HttpFetchResponse {
            status: resp.status,
            body: String::from_utf8_lossy(&resp.body).into_owned(),
        },
        Err(e) => {
            tracing::warn!(
                "plugin fetch of {} failed: {e:#}",
                crate::plugins::scraper::redact_url(&url)
            );
            HttpFetchResponse {
                status: 0,
                body: format!("{e:#}"),
            }
        }
    };
    Ok(serde_json::to_string(&wire)?)
});

host_fn!(get_credential(user_data: CredCap; _source: String) -> String {
    let cap = user_data.get()?;
    let cap = cap.lock().unwrap();
    match cap.bound_source.get() {
        Some(source) => Ok(cap.credentials.get(source)),
        None => Ok(String::new()),
    }
});

/// A scraper backed by a sandboxed `.wasm` plugin.
pub struct WasmScraper {
    manifest: ScraperManifest,
    /// Bytes returned by the optional `icon` export.
    icon: Option<Vec<u8>>,
    plugin: Arc<Mutex<Plugin>>,
}

fn default_capabilities() -> Vec<String> {
    vec!["scrape".to_string()]
}

/// Lenient manifest shape for legacy, versionless plugins.
#[derive(Deserialize)]
struct PluginManifestJson {
    id: String,
    #[serde(default)]
    manifest_version: Option<u32>,
    #[serde(default = "default_plugin_version")]
    version: String,
    #[serde(default = "default_plugin_author")]
    author: String,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    repository: Option<String>,
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    source: Option<String>,
    #[serde(default = "default_capabilities")]
    capabilities: Vec<String>,
    #[serde(default)]
    hosts: Vec<String>,
    #[serde(default)]
    auth: Option<AuthSpec>,
    #[serde(default)]
    rate_limit: Option<RateLimit>,
    #[serde(default)]
    feeds: Vec<crate::plugins::scraper::Feed>,
    #[serde(default)]
    reference_inputs: std::collections::BTreeMap<String, crate::plugins::scraper::ReferenceInput>,
    #[serde(default)]
    item_cache_ttl: u64,
    #[serde(default)]
    image_headers: std::collections::BTreeMap<String, String>,
    #[serde(default = "arcagrad_plugin_sdk::default_true")]
    clean_titles: bool,
    #[serde(default = "arcagrad_plugin_sdk::default_true")]
    followable: bool,
    #[serde(default = "arcagrad_plugin_sdk::default_reading_mode")]
    reading_mode: String,
    #[serde(default)]
    nsfw: bool,
    contract_version: u32,
}

fn default_plugin_version() -> String {
    "0.0.0".to_string()
}

fn default_plugin_author() -> String {
    "Unknown".to_string()
}

/// Map an SDK-validated manifest to the host representation.
fn strict_scraper_manifest(m: arcagrad_plugin_sdk::PluginManifest) -> ScraperManifest {
    ScraperManifest {
        manifest_version: Some(m.manifest_version),
        metadata_status: "strict".to_string(),
        origin: "local".to_string(),
        version: m.version,
        author: m.author,
        icon: m.icon,
        repository: m.repository,
        name: m.name,
        description: Some(m.description),
        source: m.source,
        capabilities: m.capabilities,
        hosts: m.hosts,
        auth: m.auth,
        rate_limit: m.rate_limit,
        feeds: m.feeds,
        reference_inputs: m.reference_inputs,
        item_cache_ttl: m.item_cache_ttl,
        image_headers: m.image_headers,
        clean_titles: m.clean_titles,
        followable: m.followable,
        reading_mode: m.reading_mode,
        nsfw: m.nsfw,
        contract_version: m.contract_version,
        id: m.id,
    }
}

/// Parse a versionless manifest using legacy defaults.
fn legacy_scraper_manifest(manifest_json: &str) -> Result<ScraperManifest> {
    let pm: PluginManifestJson =
        serde_json::from_str(manifest_json).context("plugin manifest is not valid JSON")?;
    Ok(ScraperManifest {
        manifest_version: pm.manifest_version,
        metadata_status: "legacy".to_string(),
        origin: "local".to_string(),
        version: pm.version,
        author: pm.author,
        icon: pm.icon,
        repository: pm.repository,
        name: pm.name.unwrap_or_else(|| pm.id.clone()),
        description: pm.description,
        source: pm.source.unwrap_or_else(|| pm.id.clone()),
        capabilities: pm.capabilities,
        hosts: pm.hosts,
        auth: pm.auth,
        rate_limit: pm.rate_limit,
        feeds: pm.feeds,
        reference_inputs: pm.reference_inputs,
        item_cache_ttl: pm.item_cache_ttl,
        image_headers: pm.image_headers,
        clean_titles: pm.clean_titles,
        followable: pm.followable,
        reading_mode: pm.reading_mode,
        nsfw: pm.nsfw,
        contract_version: pm.contract_version,
        id: pm.id,
    })
}

/// 256 MiB linear-memory ceiling and 10-second guest CPU timeout.
const PLUGIN_MAX_PAGES: u32 = 4096;
const PLUGIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Apply the same resource limits to every plugin instance.
fn sandboxed_manifest(bytes: Vec<u8>) -> Manifest {
    Manifest::new([Wasm::data(bytes)])
        .with_memory_max(PLUGIN_MAX_PAGES)
        .with_timeout(PLUGIN_TIMEOUT)
}

/// Instantiate a plugin with inert capabilities for manifest and icon inspection.
fn inspection_plugin(bytes: &[u8], handle: Handle) -> Result<Plugin> {
    let manifest = sandboxed_manifest(bytes.to_vec());
    let fns = host_functions(
        FetchCap {
            fetcher: Arc::new(crate::plugins::scraper::InertFetcher),
            handle,
            allowed_hosts: Arc::new(OnceLock::new()),
        },
        CredCap {
            credentials: Arc::new(crate::plugins::scraper::NoCredentials),
            bound_source: Arc::new(OnceLock::new()),
        },
    );
    Plugin::new(&manifest, fns, false).map_err(|e| anyhow!("instantiating wasm plugin: {e}"))
}

/// Build the plugin's `http_fetch` and `get_credential` imports.
fn host_functions(fetch: FetchCap, cred: CredCap) -> [Function; 2] {
    [
        Function::new("http_fetch", [PTR], [PTR], UserData::new(fetch), http_fetch),
        Function::new(
            "get_credential",
            [PTR],
            [PTR],
            UserData::new(cred),
            get_credential,
        ),
    ]
}

/// Read a strict manifest for repository index generation.
pub fn read_manifest(bytes: &[u8], handle: Handle) -> Result<arcagrad_plugin_sdk::PluginManifest> {
    let mut plugin = inspection_plugin(bytes, handle)?;
    let json: String = plugin
        .call("manifest", "")
        .map_err(|e| anyhow!("plugin 'manifest' export failed: {e}"))?;
    let inspection = arcagrad_plugin_sdk::inspect_manifest_json(&json);
    inspection.manifest.ok_or_else(|| {
        anyhow!(
            "manifest is not strict v1 (a repo needs strict manifests): {}",
            inspection.errors.join("; ")
        )
    })
}

/// Read an optional non-empty icon export.
pub fn read_icon(bytes: &[u8], handle: Handle) -> Option<Vec<u8>> {
    let mut plugin = inspection_plugin(bytes, handle).ok()?;
    if !plugin.function_exists("icon") {
        return None;
    }
    plugin
        .call::<&str, Vec<u8>>("icon", "")
        .ok()
        .filter(|b| !b.is_empty())
}

impl WasmScraper {
    pub fn from_file(
        path: &Path,
        fetcher: Arc<dyn Fetcher>,
        credentials: Arc<dyn Credentials>,
        handle: Handle,
    ) -> Result<Self> {
        let bytes =
            std::fs::read(path).with_context(|| format!("reading plugin {}", path.display()))?;
        Self::from_bytes(bytes, fetcher, credentials, handle)
    }

    pub fn from_bytes(
        bytes: Vec<u8>,
        fetcher: Arc<dyn Fetcher>,
        credentials: Arc<dyn Credentials>,
        handle: Handle,
    ) -> Result<Self> {
        let manifest = sandboxed_manifest(bytes);
        let allowed_hosts = Arc::new(OnceLock::new());
        let bound_source = Arc::new(OnceLock::new());
        let fns = host_functions(
            FetchCap {
                fetcher,
                handle,
                allowed_hosts: allowed_hosts.clone(),
            },
            CredCap {
                credentials,
                bound_source: bound_source.clone(),
            },
        );
        let mut plugin = Plugin::new(&manifest, fns, false)
            .map_err(|e| anyhow!("instantiating wasm plugin: {e}"))?;

        let manifest_json: String = plugin
            .call("manifest", "")
            .map_err(|e| anyhow!("plugin 'manifest' export failed: {e}"))?;
        let inspection = arcagrad_plugin_sdk::inspect_manifest_json(&manifest_json);
        if !inspection.valid {
            return Err(anyhow!(
                "invalid plugin manifest: {}",
                inspection.errors.join("; ")
            ));
        }
        for w in &inspection.warnings {
            tracing::warn!("plugin manifest: {w}");
        }
        let manifest = match inspection.manifest {
            Some(strict) => strict_scraper_manifest(strict),
            None => legacy_scraper_manifest(&manifest_json)?,
        };
        if manifest.contract_version != CONTRACT_VERSION {
            return Err(anyhow!(
                "plugin '{}' targets contract v{}, host is v{CONTRACT_VERSION}",
                manifest.id,
                manifest.contract_version
            ));
        }
        let _ = allowed_hosts.set(manifest.hosts.clone());
        let _ = bound_source.set(manifest.source.clone());
        let mut manifest = manifest;
        let icon = if plugin.function_exists("icon") {
            match plugin.call::<&str, Vec<u8>>("icon", "") {
                Ok(bytes) if !bytes.is_empty() => {
                    manifest.icon = Some(format!("/api/plugins/{}/icon", manifest.id));
                    Some(bytes)
                }
                Ok(_) => None,
                Err(e) => {
                    tracing::warn!(plugin = %manifest.id, "icon export failed: {e}");
                    None
                }
            }
        } else {
            None
        };
        Ok(Self {
            manifest,
            icon,
            plugin: Arc::new(Mutex::new(plugin)),
        })
    }

    /// Call a plugin export off the async runtime (it's a synchronous wasm call).
    async fn call_export(&self, func: &'static str, input: String) -> Result<String> {
        let plugin = self.plugin.clone();
        tokio::task::spawn_blocking(move || -> Result<String> {
            let mut p = plugin
                .lock()
                .map_err(|_| anyhow!("plugin mutex poisoned"))?;
            let out: String = p
                .call(func, input.as_str())
                .map_err(|e| anyhow!("plugin '{func}' failed: {e}"))?;
            Ok(out)
        })
        .await?
    }
}

#[async_trait]
impl MetadataScraper for WasmScraper {
    fn manifest(&self) -> ScraperManifest {
        self.manifest.clone()
    }

    fn icon_bytes(&self) -> Option<&[u8]> {
        self.icon.as_deref()
    }

    async fn search(&self, hint: &ScrapeHint, _fetch: &dyn Fetcher) -> Result<Vec<Candidate>> {
        let out = self
            .call_export("search", serde_json::to_string(hint)?)
            .await?;
        Ok(serde_json::from_str(&out)?)
    }

    async fn fetch_details(
        &self,
        candidate: &Candidate,
        _fetch: &dyn Fetcher,
    ) -> Result<ScrapedMetadata> {
        let out = self
            .call_export("fetch_details", serde_json::to_string(candidate)?)
            .await?;
        Ok(serde_json::from_str(&out)?)
    }

    async fn download(
        &self,
        candidate: &Candidate,
        _fetch: &dyn Fetcher,
    ) -> Result<crate::plugins::scraper::DownloadPlan> {
        let out = self
            .call_export("download", serde_json::to_string(candidate)?)
            .await?;
        Ok(serde_json::from_str(&out)?)
    }

    async fn browse(
        &self,
        req: &crate::plugins::scraper::BrowseRequest,
        _fetch: &dyn Fetcher,
    ) -> Result<crate::plugins::scraper::BrowsePage> {
        let out = self
            .call_export("browse", serde_json::to_string(req)?)
            .await?;
        Ok(serde_json::from_str(&out)?)
    }

    async fn pages(
        &self,
        reference: &str,
        _fetch: &dyn Fetcher,
    ) -> Result<crate::plugins::scraper::BrowsePages> {
        let out = self
            .call_export("pages", serde_json::to_string(reference)?)
            .await?;
        Ok(serde_json::from_str(&out)?)
    }

    async fn identify(
        &self,
        req: &crate::plugins::scraper::IdentifyRequest,
        _fetch: &dyn Fetcher,
    ) -> Result<crate::plugins::scraper::IdentifyResult> {
        let out = self
            .call_export("identify", serde_json::to_string(req)?)
            .await?;
        Ok(serde_json::from_str(&out)?)
    }

    async fn upcoming(
        &self,
        req: &CalendarRequest,
        _fetch: &dyn Fetcher,
    ) -> Result<CalendarResponse> {
        let out = self
            .call_export("upcoming", serde_json::to_string(req)?)
            .await?;
        Ok(serde_json::from_str(&out)?)
    }
}

/// Warn about optional descriptive metadata that is missing.
fn warn_if_under_described(m: &ScraperManifest) {
    for issue in crate::plugins::scraper::manifest_lint(m) {
        tracing::warn!(plugin = %m.id, "manifest under-described: {issue}");
    }
}

/// An inspected, but not running, bundled artifact.
pub struct BundledPlugin {
    pub manifest: ScraperManifest,
    pub icon: Option<Vec<u8>>,
    /// BLAKE3 artifact hash used for upgrade detection.
    pub artifact_hash: String,
    pub bytes: &'static [u8],
}

/// Canonical BLAKE3 artifact hash.
pub fn artifact_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Inspect bundled artifacts for the plugin catalog.
pub fn bundled_catalog(
    fetcher: Arc<dyn Fetcher>,
    credentials: Arc<dyn Credentials>,
    handle: Handle,
) -> Vec<BundledPlugin> {
    let mut catalog = Vec::new();
    for (name, bytes) in BUNDLED {
        match WasmScraper::from_bytes(
            bytes.to_vec(),
            fetcher.clone(),
            credentials.clone(),
            handle.clone(),
        ) {
            Ok(mut s) => {
                s.manifest.origin = "bundled".to_string();
                warn_if_under_described(&s.manifest);
                catalog.push(BundledPlugin {
                    manifest: s.manifest,
                    icon: s.icon,
                    artifact_hash: artifact_hash(bytes),
                    bytes,
                });
            }
            Err(e) => tracing::error!("bundled plugin '{name}' failed inspection: {e:#}"),
        }
    }
    catalog
}

/// Load an artifact file as a runnable scraper.
pub fn load_artifact(
    path: &Path,
    origin: &str,
    fetcher: Arc<dyn Fetcher>,
    credentials: Arc<dyn Credentials>,
    handle: Handle,
) -> Result<WasmScraper> {
    let mut s = WasmScraper::from_file(path, fetcher, credentials, handle)?;
    s.manifest.origin = origin.to_string();
    Ok(s)
}

/// Load in-memory artifact bytes as a runnable scraper.
pub fn load_artifact_bytes(
    bytes: Vec<u8>,
    origin: &str,
    fetcher: Arc<dyn Fetcher>,
    credentials: Arc<dyn Credentials>,
    handle: Handle,
) -> Result<WasmScraper> {
    let mut s = WasmScraper::from_bytes(bytes, fetcher, credentials, handle)?;
    s.manifest.origin = origin.to_string();
    Ok(s)
}

/// Load valid `*.wasm` plugins from a directory, skipping failures.
pub fn load_plugins(
    dir: &Path,
    fetcher: Arc<dyn Fetcher>,
    credentials: Arc<dyn Credentials>,
    handle: Handle,
) -> Vec<Box<dyn MetadataScraper>> {
    let mut loaded: Vec<Box<dyn MetadataScraper>> = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return loaded,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("wasm") {
            continue;
        }
        match WasmScraper::from_file(&path, fetcher.clone(), credentials.clone(), handle.clone()) {
            Ok(s) => {
                tracing::info!(plugin = %s.manifest.id, path = %path.display(), "loaded scraper plugin");
                warn_if_under_described(&s.manifest);
                loaded.push(Box::new(s));
            }
            Err(e) => tracing::warn!("skipping plugin {}: {e:#}", path.display()),
        }
    }
    loaded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_allowlist_matching() {
        let allowed = vec!["example.com".to_string()];
        assert!(host_allowed(&allowed, "example.com"));
        assert!(host_allowed(&allowed, "i.example.com"));
        assert!(!host_allowed(&allowed, "evil.com"));
        assert!(!host_allowed(&allowed, "example.com.evil.com"));
        assert!(host_allowed(&[], "anything.com"));
        let wild = vec!["*.example.com".to_string()];
        assert!(host_allowed(&wild, "i.example.com"));
        assert!(host_allowed(&wild, "example.com"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bundled_manifests_are_fully_described() {
        let fetcher: Arc<dyn Fetcher> = Arc::new(crate::plugins::scraper::HttpFetcher::new());
        let creds: Arc<dyn Credentials> = Arc::new(crate::plugins::scraper::NoCredentials);
        for (name, bytes) in BUNDLED {
            let scraper = WasmScraper::from_bytes(
                bytes.to_vec(),
                fetcher.clone(),
                creds.clone(),
                Handle::current(),
            )
            .unwrap_or_else(|e| panic!("bundled plugin '{name}' failed to load: {e:#}"));
            let issues = crate::plugins::scraper::manifest_lint(&scraper.manifest());
            assert!(
                issues.is_empty(),
                "bundled plugin '{name}' manifest is under-described: {issues:?}"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bundled_plugins_declare_a_rate_limit() {
        let fetcher: Arc<dyn Fetcher> = Arc::new(crate::plugins::scraper::HttpFetcher::new());
        let creds: Arc<dyn Credentials> = Arc::new(crate::plugins::scraper::NoCredentials);
        for (name, bytes) in BUNDLED {
            let scraper = WasmScraper::from_bytes(
                bytes.to_vec(),
                fetcher.clone(),
                creds.clone(),
                Handle::current(),
            )
            .unwrap_or_else(|e| panic!("bundled plugin '{name}' failed to load: {e:#}"));
            let rl = scraper
                .manifest()
                .rate_limit
                .unwrap_or_else(|| panic!("bundled plugin '{name}' declares no rate_limit"));
            assert!(
                !rl.rules.is_empty(),
                "bundled plugin '{name}' must declare at least one rate-limit rule"
            );
            for r in &rl.rules {
                assert!(
                    r.requests > 0 && r.per_ms > 0,
                    "bundled plugin '{name}' rule '{}' has a zero limit",
                    r.match_pattern
                );
            }
        }
    }

    struct AnilistStub;
    #[async_trait]
    impl Fetcher for AnilistStub {
        async fn fetch(&self, req: FetchRequest) -> Result<crate::plugins::scraper::FetchResponse> {
            assert_eq!(req.method, "POST", "AniList is GraphQL-over-POST only");
            assert_eq!(req.url, "https://graphql.anilist.co");
            assert!(req
                .headers
                .iter()
                .any(|(k, v)| k == "User-Agent" && v.contains("arcagrad")));
            assert!(
                !req.headers.iter().any(|(k, _)| k == "Authorization"),
                "AniList needs no auth"
            );
            let body = String::from_utf8_lossy(&req.body);
            let resp: &[u8] = if body.contains("Page(") {
                br#"{"data":{"Page":{"media":[{"id":34632,
                     "title":{"romaji":"Oyasumi Punpun","english":"Goodnight Punpun"}}]}}}"#
            } else if body.contains("Media(") {
                br#"{"data":{"Media":{
                     "title":{"romaji":"Oyasumi Punpun","english":"Goodnight Punpun"},
                     "description":"Meet Punpun.<br>He wants things.",
                     "genres":["Drama","Psychological"],
                     "tags":[{"name":"Seinen","category":"Demographic","isGeneralSpoiler":false,"rank":92},
                              {"name":"Time Skip","category":"Setting-Time","isGeneralSpoiler":true,"rank":72}],
                     "staff":{"edges":[
                         {"role":"Story & Art","node":{"name":{"full":"Inio Asano"}}},
                         {"role":"Translator (English)","node":{"name":{"full":"JN Productions"}}}]},
                     "siteUrl":"https://anilist.co/manga/34632"}}}"#
            } else {
                panic!("unexpected AniList query body: {body}");
            };
            Ok(crate::plugins::scraper::FetchResponse {
                status: 200,
                body: resp.to_vec(),
            })
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn anilist_plugin_loads_and_scrapes() {
        let bytes = std::fs::read("plugins/anilist/anilist.wasm")
            .expect("built by build.rs on cargo build");
        let fetcher: Arc<dyn Fetcher> = Arc::new(AnilistStub);
        let creds: Arc<dyn Credentials> = Arc::new(crate::plugins::scraper::NoCredentials);
        let scraper = WasmScraper::from_bytes(bytes, fetcher, creds, Handle::current()).unwrap();
        assert_eq!(scraper.manifest().id, "anilist");

        let cands = scraper
            .search(
                &ScrapeHint {
                    title: "Goodnight Punpun".into(),
                    display_title: None,
                    author: None,
                    modality: None,
                    page_count: None,
                    reference: None,
                },
                &AnilistStub,
            )
            .await
            .unwrap();
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].id, "34632");

        let meta = scraper
            .fetch_details(&cands[0], &AnilistStub)
            .await
            .unwrap();
        assert_eq!(meta.title.as_deref(), Some("Goodnight Punpun"));
        assert_eq!(
            meta.language, None,
            "language is deliberately never inferred"
        );
        assert_eq!(
            meta.source_url.as_deref(),
            Some("https://anilist.co/manga/34632"),
            "source_url is ALWAYS the AniList siteUrl"
        );
        assert_eq!(meta.mapped_tags.len(), 4, "got {:?}", meta.mapped_tags);
        assert!(meta
            .mapped_tags
            .iter()
            .any(|t| t.namespace == "tag" && t.value == "Drama"));
        assert!(meta
            .mapped_tags
            .iter()
            .any(|t| t.namespace == "demographic" && t.value == "Seinen"));
        assert!(meta
            .mapped_tags
            .iter()
            .any(|t| t.namespace == "creator" && t.value == "Inio Asano"));
        assert!(
            !meta.mapped_tags.iter().any(|t| t.value == "Time Skip"),
            "spoiler tag must be dropped"
        );
        assert!(
            !meta.mapped_tags.iter().any(|t| t.value == "JN Productions"),
            "translator credit must not become an creator tag"
        );
        assert!(
            meta.comments.is_empty(),
            "AniList has no gallery-style comments"
        );
    }

    struct AnilistNoSearchStub;
    #[async_trait]
    impl Fetcher for AnilistNoSearchStub {
        async fn fetch(&self, req: FetchRequest) -> Result<crate::plugins::scraper::FetchResponse> {
            let body = String::from_utf8_lossy(&req.body);
            assert!(
                !body.contains("Page("),
                "reference path must not hit the search query: {body}"
            );
            assert!(
                body.contains("\"id\":34632"),
                "expected the resolved id in the query variables, got {body}"
            );
            Ok(crate::plugins::scraper::FetchResponse {
                status: 200,
                body: br#"{"data":{"Media":{
                     "title":{"romaji":"By Ref","english":null},
                     "genres":[],"tags":[],"staff":{"edges":[]},
                     "siteUrl":"https://anilist.co/manga/34632"}}}"#
                    .to_vec(),
            })
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn anilist_reference_resolves_without_search() {
        let bytes = std::fs::read("plugins/anilist/anilist.wasm").unwrap();
        let scraper = WasmScraper::from_bytes(
            bytes,
            Arc::new(AnilistNoSearchStub) as Arc<dyn Fetcher>,
            Arc::new(crate::plugins::scraper::NoCredentials),
            Handle::current(),
        )
        .unwrap();

        let cands = scraper
            .search(
                &ScrapeHint {
                    title: "ignored".into(),
                    display_title: None,
                    author: None,
                    modality: None,
                    page_count: None,
                    reference: Some("https://anilist.co/manga/34632/Oyasumi-Punpun".into()),
                },
                &AnilistNoSearchStub,
            )
            .await
            .unwrap();
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].id, "34632");
        assert_eq!(cands[0].score, 1.0, "an exact reference is full-confidence");

        let meta = scraper
            .fetch_details(&cands[0], &AnilistNoSearchStub)
            .await
            .unwrap();
        assert_eq!(meta.title.as_deref(), Some("By Ref"));
    }

    struct GutenbergStub;
    #[async_trait]
    impl Fetcher for GutenbergStub {
        async fn fetch(&self, req: FetchRequest) -> Result<crate::plugins::scraper::FetchResponse> {
            const BOOK: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:dcterms="http://purl.org/dc/terms/">
<title>Pride and Prejudice by Jane Austen</title>
<entry>
<title>Pride and Prejudice</title>
<content type="xhtml"><div xmlns="http://www.w3.org/1999/xhtml">
<p>Summary: A novel of manners. (This is an automatically generated summary.)</p>
<p>Downloads: 74821</p>
</div></content>
<id>urn:gutenberg:1342:2</id>
<author><name>Austen, Jane</name></author>
<category scheme="http://purl.org/dc/terms/LCSH" term="Courtship -- Fiction"/>
<category scheme="http://purl.org/dc/terms/LCSH" term="England -- Fiction"/>
<dcterms:language>en</dcterms:language>
<link type="application/epub+zip" rel="http://opds-spec.org/acquisition" href="https://www.gutenberg.org/ebooks/1342.epub3.images"/>
<link type="image/jpeg" rel="http://opds-spec.org/image" href="https://www.gutenberg.org/cache/epub/1342/pg1342.cover.medium.jpg"/>
</entry>
</feed>"#;
            const LISTING: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
<title>Books</title>
<entry>
<id>https://www.gutenberg.org/ebooks/authors/search.opds/?query=x</id>
<title>Authors</title>
<content type="text">1 author name matches.</content>
</entry>
<entry>
<id>https://www.gutenberg.org/ebooks/1342.opds</id>
<title>Pride and Prejudice</title>
<content type="text">Jane Austen</content>
</entry>
</feed>"#;
            let body = if req.url.contains("/ebooks/1342.opds") {
                BOOK
            } else {
                assert!(
                    req.url
                        .contains("gutenberg.org/ebooks/search.opds/?sort_order=downloads"),
                    "got {}",
                    req.url
                );
                LISTING
            };
            Ok(crate::plugins::scraper::FetchResponse {
                status: 200,
                body: body.as_bytes().to_vec(),
            })
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn gutenberg_plugin_browses_and_plans_an_epub_download() {
        let bytes = std::fs::read("plugins/gutenberg/gutenberg.wasm").unwrap();
        let scraper = WasmScraper::from_bytes(
            bytes,
            Arc::new(GutenbergStub) as Arc<dyn Fetcher>,
            Arc::new(crate::plugins::scraper::NoCredentials),
            Handle::current(),
        )
        .unwrap();

        let m = scraper.manifest();
        assert!(m.capabilities.iter().any(|c| c == "browse"));
        assert!(m.capabilities.iter().any(|c| c == "download"));
        assert!(m.feeds.iter().any(|f| f.id == "popular"));

        let page = scraper
            .browse(
                &crate::plugins::scraper::BrowseRequest {
                    feed: "popular".into(),
                    query: None,
                    range: None,
                    page: 1,
                },
                &GutenbergStub,
            )
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1, "navigation entries must be skipped");
        assert_eq!(page.items[0].reference, "1342");
        assert_eq!(page.items[0].title, "Pride and Prejudice");
        assert_eq!(page.items[0].subtitle.as_deref(), Some("Jane Austen"));
        assert_eq!(
            page.items[0].cover_url,
            "https://www.gutenberg.org/cache/epub/1342/pg1342.cover.medium.jpg"
        );
        assert_eq!(page.num_pages, None);

        let cand = crate::plugins::scraper::Candidate {
            id: "1342".into(),
            title: String::new(),
            score: 1.0,
        };
        let meta = scraper.fetch_details(&cand, &GutenbergStub).await.unwrap();
        assert!(meta
            .mapped_tags
            .iter()
            .any(|t| t.namespace == "creator" && t.value == "Jane Austen"));
        assert!(meta
            .mapped_tags
            .iter()
            .any(|t| t.namespace == "tag" && t.value == "courtship"));
        assert_eq!(meta.language.as_deref(), Some("english"));
        assert!(meta.description.as_deref().unwrap().starts_with("A novel"));
        assert_eq!(meta.favorites, Some(74821));

        let plan = scraper.download(&cand, &GutenbergStub).await.unwrap();
        assert_eq!(
            plan.url,
            "https://www.gutenberg.org/ebooks/1342.epub3.images"
        );
        assert_eq!(plan.filename, "Pride and Prejudice.epub");
        assert_eq!(plan.metadata.title.as_deref(), Some("Pride and Prejudice"));
    }

    struct MarxistsStub;
    #[async_trait]
    impl Fetcher for MarxistsStub {
        async fn fetch(&self, req: FetchRequest) -> Result<crate::plugins::scraper::FetchResponse> {
            assert_eq!(req.url, "https://www.marxists.org/ebooks/index.htm");
            const CATALOG: &str = r#"<html><body><table><tr><td>
<p class="head">V. I. Lenin</p><p class="note">The State and Revolution
<a href="lenin/state-and-revolution.epub">epub</a><br /></p>
</td></tr></table></body></html>"#;
            Ok(crate::plugins::scraper::FetchResponse {
                status: 200,
                body: CATALOG.as_bytes().to_vec(),
            })
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn marxists_plugin_browses_and_plans_an_epub_download() {
        let bytes = std::fs::read("plugins/marxists/marxists.wasm").unwrap();
        let scraper = WasmScraper::from_bytes(
            bytes,
            Arc::new(MarxistsStub) as Arc<dyn Fetcher>,
            Arc::new(crate::plugins::scraper::NoCredentials),
            Handle::current(),
        )
        .unwrap();

        let manifest = scraper.manifest();
        assert!(manifest.capabilities.iter().any(|c| c == "browse"));
        assert!(manifest.capabilities.iter().any(|c| c == "download"));
        assert_eq!(manifest.feeds.len(), 1);
        assert_eq!(manifest.feeds[0].id, "catalog");
        assert!(!manifest.followable);

        let page = scraper
            .browse(
                &crate::plugins::scraper::BrowseRequest {
                    feed: "catalog".into(),
                    query: Some("lenin".into()),
                    range: None,
                    page: 1,
                },
                &MarxistsStub,
            )
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].title, "The State and Revolution");
        assert_eq!(page.items[0].subtitle.as_deref(), Some("V. I. Lenin"));
        assert_eq!(page.items[0].cover_url, "");
        assert_eq!(
            page.items[0].source_url.as_deref(),
            Some("https://www.marxists.org/ebooks/lenin/state-and-revolution.epub")
        );
        assert_eq!(page.num_pages, Some(1));

        let candidate = Candidate {
            id: page.items[0].reference.clone(),
            title: String::new(),
            score: 1.0,
        };
        let metadata = scraper
            .fetch_details(&candidate, &MarxistsStub)
            .await
            .unwrap();
        assert!(metadata
            .mapped_tags
            .iter()
            .any(|tag| tag.namespace == "creator" && tag.value == "V. I. Lenin"));
        assert_eq!(metadata.language.as_deref(), Some("english"));
        assert_eq!(
            metadata.source_url.as_deref(),
            Some("https://www.marxists.org/ebooks/lenin/state-and-revolution.epub")
        );

        let plan = scraper.download(&candidate, &MarxistsStub).await.unwrap();
        assert_eq!(
            plan.url,
            "https://www.marxists.org/ebooks/lenin/state-and-revolution.epub"
        );
        assert_eq!(plan.filename, "The State and Revolution.epub");
    }

    struct KeyCreds;
    impl Credentials for KeyCreds {
        fn get(&self, _source: &str) -> String {
            r#"{"api_key":"secret-key"}"#.to_string()
        }
    }

    struct ComicVineStub;
    #[async_trait]
    impl Fetcher for ComicVineStub {
        async fn fetch(&self, req: FetchRequest) -> Result<crate::plugins::scraper::FetchResponse> {
            assert!(
                req.url.contains("api_key=secret-key"),
                "api_key required: {}",
                req.url
            );
            assert!(req.url.contains("format=json"), "json format: {}", req.url);
            assert!(
                req.url.contains("/?api_key="),
                "endpoint needs a trailing slash before the query: {}",
                req.url
            );
            let body = if req.url.contains("/search") {
                assert!(
                    req.url.contains("resources=volume"),
                    "search volumes: {}",
                    req.url
                );
                assert!(
                    req.url.contains("query=Batman"),
                    "search query: {}",
                    req.url
                );
                br#"{"status_code":1,"error":"OK","results":[
                    {"id":796,"name":"Batman","resource_type":"volume"},
                    {"id":42,"name":"Batman: Year One","resource_type":"volume"}]}"#
                    .to_vec()
            } else if req.url.contains("/volume/4050-796") {
                br#"{"status_code":1,"error":"OK","results":{
                    "name":"Batman","description":"<p>The Dark Knight.</p>","deck":"Bruce Wayne.",
                    "site_detail_url":"https://comicvine.gamespot.com/batman/4050-796/",
                    "publisher":{"id":10,"name":"DC Comics"},
                    "characters":[{"id":1,"name":"Batman","count":"689"}],
                    "people":[
                        {"id":2,"name":"Scott Snyder","count":"50"},
                        {"id":3,"name":"Greg Capullo","count":"40"}],
                    "teams":[{"id":4,"name":"Justice League","count":"20"}],
                    "concepts":[{"id":5,"name":"Time Travel","count":"10"}]}}"#
                    .to_vec()
            } else {
                assert!(
                    req.url.contains("/issue/4000-12345"),
                    "issue endpoint: {}",
                    req.url
                );
                br#"{"status_code":1,"error":"OK","results":{
                    "name":null,"description":"An issue.",
                    "site_detail_url":"https://comicvine.gamespot.com/x/4000-12345/",
                    "volume":{"id":796,"name":"Batman"},
                    "person_credits":[
                        {"name":"Tom King","role":"writer"},
                        {"name":"Some Editor","role":"editor"}],
                    "character_credits":[{"id":1,"name":"Batman"}],
                    "team_credits":[],"concept_credits":[]}}"#
                    .to_vec()
            };
            Ok(crate::plugins::scraper::FetchResponse { status: 200, body })
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn comicvine_plugin_scrapes_volume_and_issue_metadata() {
        let bytes = std::fs::read("plugins/comicvine/comicvine.wasm")
            .expect("built by build.rs on cargo build");
        let fetcher: Arc<dyn Fetcher> = Arc::new(ComicVineStub);
        let creds: Arc<dyn Credentials> = Arc::new(KeyCreds);
        let scraper = WasmScraper::from_bytes(bytes, fetcher, creds, Handle::current()).unwrap();

        let m = scraper.manifest();
        assert!(m.capabilities.iter().any(|c| c == "scrape"));
        assert!(!m.capabilities.iter().any(|c| c == "browse"));
        assert_eq!(m.source, "comicvine");
        assert!(m.auth.is_some(), "declares API-key auth");

        let by_ref = scraper
            .search(
                &ScrapeHint {
                    title: "whatever".into(),
                    display_title: None,
                    author: None,
                    modality: None,
                    page_count: None,
                    reference: Some("https://comicvine.gamespot.com/batman/4050-796/".into()),
                },
                &ComicVineStub,
            )
            .await
            .unwrap();
        assert_eq!(by_ref.len(), 1);
        assert_eq!(by_ref[0].id, "4050-796");

        let hits = scraper
            .search(
                &ScrapeHint {
                    title: "Batman".into(),
                    display_title: None,
                    author: None,
                    modality: None,
                    page_count: None,
                    reference: None,
                },
                &ComicVineStub,
            )
            .await
            .unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, "4050-796");
        assert_eq!(hits[0].title, "Batman");

        let meta = scraper
            .fetch_details(&hits[0], &ComicVineStub)
            .await
            .unwrap();
        assert_eq!(meta.title.as_deref(), Some("Batman"));
        assert_eq!(meta.description.as_deref(), Some("<p>The Dark Knight.</p>"));
        assert_eq!(
            meta.source_url.as_deref(),
            Some("https://comicvine.gamespot.com/batman/4050-796/")
        );
        let has = |ns: &str, v: &str| {
            meta.mapped_tags
                .iter()
                .any(|t| t.namespace == ns && t.value == v)
        };
        assert!(
            has("creator", "Scott Snyder") && has("creator", "Greg Capullo"),
            "volume people → creator"
        );
        assert!(has("character", "Batman"), "character → character");
        assert!(has("character", "Justice League"), "team → character");
        assert!(has("tag", "Time Travel"), "concept → tag");
        assert!(has("group", "DC Comics"), "publisher → group");

        let issue = scraper
            .fetch_details(
                &crate::plugins::scraper::Candidate {
                    id: "4000-12345".into(),
                    title: String::new(),
                    score: 1.0,
                },
                &ComicVineStub,
            )
            .await
            .unwrap();
        assert_eq!(issue.title.as_deref(), Some("Batman"));
        let ihas = |ns: &str, v: &str| {
            issue
                .mapped_tags
                .iter()
                .any(|t| t.namespace == ns && t.value == v)
        };
        assert!(ihas("creator", "Tom King"), "issue creator → creator");
        assert!(
            !issue.mapped_tags.iter().any(|t| t.value == "Some Editor"),
            "issue: pure editor skipped (role-filtered)"
        );
        assert!(ihas("character", "Batman"));
        assert!(
            ihas("group", "DC Comics"),
            "publisher from the volume lookup"
        );
    }

    struct VizCalendarStub;
    #[async_trait]
    impl Fetcher for VizCalendarStub {
        async fn fetch(&self, req: FetchRequest) -> Result<crate::plugins::scraper::FetchResponse> {
            let body = if req.url == "https://www.viz.com/hima-ten" {
                br#"<article><span class="product-tag">Pre-Order</span><a href="/manga-books/manga/hima-ten-volume-2-0/product/8980" class="product-thumb"></a><a href="/manga-books/manga/hima-ten-volume-2-0/product/8980">Hima-Ten!, Vol. 2</a></article>"#.to_vec()
            } else if req.url.ends_with("/product/8980") {
                br#"<meta property="og:title" content="VIZ: See Hima-Ten!, Vol. 2"><meta property="og:image" content="https://dw9to29mmj727.cloudfront.net/products/1974764435.jpg"><a id="buy_paperback_tab">Paperback</a><a id="buy_digital_tab">Digital</a><div class="mar-b-md"><strong>Story and Art by</strong> Genki Ono</div><div class="o_release-date mar-b-md"><strong>Release</strong> September 1, 2026</div><div class="o_isbn13 mar-b-md"><strong>ISBN-13</strong> 978-1-9747-6443-3</div><div><strong>Category</strong> Manga</div>"#.to_vec()
            } else {
                return Ok(crate::plugins::scraper::FetchResponse {
                    status: 404,
                    body: Vec::new(),
                });
            };
            Ok(crate::plugins::scraper::FetchResponse { status: 200, body })
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn viz_calendar_follows_the_authoritative_series_page() {
        let bytes =
            std::fs::read("plugins/viz/viz.wasm").expect("built by build.rs on cargo build");
        let fetcher: Arc<dyn Fetcher> = Arc::new(VizCalendarStub);
        let creds: Arc<dyn Credentials> = Arc::new(crate::plugins::scraper::NoCredentials);
        let scraper = WasmScraper::from_bytes(bytes, fetcher, creds, Handle::current()).unwrap();
        assert!(scraper
            .manifest()
            .capabilities
            .iter()
            .any(|cap| cap == "calendar"));

        let response = scraper
            .upcoming(
                &crate::plugins::scraper::CalendarRequest {
                    window_start: "2026-07-01".into(),
                    window_end: "2026-10-01".into(),
                    references: vec![crate::plugins::scraper::CalendarReference {
                        reference: "https://www.viz.com/hima-ten".into(),
                        title: Some("A deliberately unrelated local title".into()),
                    }],
                    market: Some("en-US".into()),
                },
                &VizCalendarStub,
            )
            .await
            .unwrap();
        assert_eq!(response.results.len(), 1);
        assert_eq!(response.results[0].releases.len(), 1);
        assert_eq!(response.results[0].releases[0].label, "Vol. 2");
        assert_eq!(
            response.results[0].releases[0].formats,
            ["Print", "Digital"]
        );
        assert_eq!(response.results[0].releases[0].release_date, "2026-09-01");
        assert_eq!(
            response.results[0].releases[0].cover_url.as_deref(),
            Some("https://dw9to29mmj727.cloudfront.net/products/1974764435.jpg")
        );
    }
}
