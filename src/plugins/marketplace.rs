//! Repository-backed plugin catalog and installer.

use std::collections::HashMap;
use std::sync::RwLock;

use anyhow::{anyhow, Context, Result};
use arcagrad_plugin_sdk::{validate_repo_index, RepoEntry, RepoIndex};

use crate::plugins::scraper::{FetchRequest, Fetcher};
use crate::{repo, AppState};

/// Last successfully validated entries for each repository.
#[derive(Default)]
pub struct RepoCache {
    inner: RwLock<HashMap<String, Vec<RepoEntry>>>,
}

impl RepoCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn set(&self, url: &str, entries: Vec<RepoEntry>) {
        self.inner
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(url.to_string(), entries);
    }

    pub fn remove(&self, url: &str) {
        self.inner
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(url);
    }

    pub fn entries(&self) -> Vec<(String, RepoEntry)> {
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .flat_map(|(url, entries)| entries.iter().map(|e| (url.clone(), e.clone())))
            .collect()
    }

    /// Returns the first matching entry and its repository URL.
    pub fn get(&self, id: &str) -> Option<(String, RepoEntry)> {
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find_map(|(url, entries)| {
                entries
                    .iter()
                    .find(|e| e.manifest.id == id)
                    .map(|e| (url.clone(), e.clone()))
            })
    }

    pub fn version_of(&self, id: &str) -> Option<String> {
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .flatten()
            .find(|e| e.manifest.id == id)
            .map(|e| e.manifest.version.clone())
    }
}

/// Fetch and validate a repository while preserving its last-good cache on error.
pub async fn fetch_and_cache(
    cache: &RepoCache,
    write: &sqlx::SqlitePool,
    fetcher: &dyn Fetcher,
    url: &str,
) -> Result<Option<String>> {
    match fetch_index(fetcher, url).await {
        Ok(index) => {
            let name = index.name.clone();
            cache.set(url, index.plugins);
            repo::set_plugin_repo_fetch(write, url, name.as_deref(), None).await?;
            Ok(name)
        }
        Err(e) => {
            let msg = format!("{e:#}");
            repo::set_plugin_repo_fetch(write, url, None, Some(&msg)).await?;
            Err(e)
        }
    }
}

async fn fetch_index(fetcher: &dyn Fetcher, url: &str) -> Result<RepoIndex> {
    let resp = fetcher
        .fetch(FetchRequest::get(url))
        .await
        .with_context(|| format!("fetching repo index {url}"))?;
    if !(200..300).contains(&resp.status) {
        return Err(anyhow!("repo index returned HTTP {}", resp.status));
    }
    let index: RepoIndex =
        serde_json::from_slice(&resp.body).context("repo index is not valid JSON")?;
    let errors = validate_repo_index(&index);
    if !errors.is_empty() {
        return Err(anyhow!("invalid repo index: {}", errors.join("; ")));
    }
    Ok(index)
}

/// Resolve relative artifact paths against the repository index URL.
fn resolve_artifact_url(index_url: &str, artifact_url: &str) -> Result<String> {
    match url::Url::parse(artifact_url) {
        Ok(u) if matches!(u.scheme(), "http" | "https") => Ok(u.to_string()),
        Ok(u) => Err(anyhow!("artifact url scheme not allowed: {}", u.scheme())),
        Err(url::ParseError::RelativeUrlWithoutBase) => {
            let base = url::Url::parse(index_url)
                .with_context(|| format!("repo url is not a valid URL: {index_url}"))?;
            Ok(base
                .join(artifact_url)
                .with_context(|| format!("resolving artifact path {artifact_url}"))?
                .to_string())
        }
        Err(e) => Err(anyhow!("invalid artifact url {artifact_url}: {e}")),
    }
}

/// Compare lenient numeric versions without treating downgrades as updates.
pub fn version_newer(candidate: &str, installed: &str) -> bool {
    let parse = |v: &str| -> Vec<String> { v.split('.').map(str::to_string).collect() };
    let (a, b) = (parse(candidate), parse(installed));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).map(String::as_str), b.get(i).map(String::as_str));
        match (x, y) {
            (Some(x), Some(y)) => {
                let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(nx), Ok(ny)) => nx.cmp(&ny),
                    _ => x.cmp(y),
                };
                match ord {
                    std::cmp::Ordering::Greater => return true,
                    std::cmp::Ordering::Less => return false,
                    std::cmp::Ordering::Equal => {}
                }
            }
            (Some(_), None) => return true,
            (None, _) => return false,
        }
    }
    false
}

/// Build the repository fetcher, optionally allowing operator-configured LAN URLs.
/// Plugin network calls always remain guarded.
fn repo_fetcher(state: &AppState) -> std::sync::Arc<dyn Fetcher> {
    if state.config.allow_private_repos {
        std::sync::Arc::new(crate::plugins::scraper::HttpFetcher::unguarded())
    } else {
        state.fetcher.clone()
    }
}

/// Normalize whitespace and a trailing slash for repository identity.
fn canonical_repo_url(url: &str) -> &str {
    url.trim().trim_end_matches('/')
}

/// Validate and add a repository, rejecting duplicate normalized URLs.
pub async fn add_repo(state: &AppState, url: &str) -> Result<()> {
    let url = canonical_repo_url(url);
    if repo::list_plugin_repos(&state.read)
        .await?
        .iter()
        .any(|r| r.url == url)
    {
        return Err(anyhow!("this repository is already added"));
    }
    let index = fetch_index(repo_fetcher(state).as_ref(), url).await?;
    let name = index.name.clone();
    repo::upsert_plugin_repo(&state.write, url, name.as_deref()).await?;
    state.marketplace.set(url, index.plugins);
    repo::set_plugin_repo_fetch(&state.write, url, name.as_deref(), None).await?;
    tracing::info!(repo = %url, "plugin repo added");
    Ok(())
}

/// Remove a repository and uninstall its plugins while retaining plugin configuration.
pub async fn remove_repo(state: &AppState, url: &str) -> Result<bool> {
    let url = canonical_repo_url(url);
    let existed = repo::delete_plugin_repo(&state.write, url).await?;
    state.marketplace.remove(url);
    if existed {
        for row in repo::list_plugin_installs(&state.read).await? {
            if row.repo_url.as_deref() == Some(url) {
                crate::plugins::plugin_store::uninstall(state, &row.plugin_id).await?;
                tracing::info!(plugin = %row.plugin_id, repo = %url, "uninstalled with its repository");
            }
        }
    }
    Ok(existed)
}

/// Download a community plugin and verify its hash and embedded manifest before install.
pub async fn install_from_repo(state: &AppState, id: &str) -> Result<()> {
    let (repo_url, entry) = state
        .marketplace
        .get(id)
        .ok_or_else(|| anyhow!("unknown plugin '{id}'"))?;

    let artifact_url = resolve_artifact_url(&repo_url, &entry.artifact_url)?;
    let resp = repo_fetcher(state)
        .fetch(FetchRequest::get(&artifact_url))
        .await
        .with_context(|| format!("downloading {artifact_url}"))?;
    if !(200..300).contains(&resp.status) {
        return Err(anyhow!("artifact download returned HTTP {}", resp.status));
    }
    let bytes = resp.body;

    let hash = crate::plugins::wasm_host::artifact_hash(&bytes);
    if hash != entry.artifact_hash {
        return Err(anyhow!(
            "artifact hash mismatch (index pins {}, downloaded {hash}) — the repo's index is stale or the artifact was tampered with",
            entry.artifact_hash
        ));
    }

    let scraper =
        crate::plugins::plugin_store::load_scraper(state, bytes.clone(), "community").await?;
    let manifest = crate::plugins::scraper::MetadataScraper::manifest(&scraper);
    if manifest.id != entry.manifest.id || manifest.version != entry.manifest.version {
        return Err(anyhow!(
            "artifact manifest is {} v{}, index says {} v{}",
            manifest.id,
            manifest.version,
            entry.manifest.id,
            entry.manifest.version
        ));
    }
    for host in &manifest.hosts {
        if !entry.manifest.hosts.contains(host) {
            return Err(anyhow!(
                "artifact declares host '{host}' the index entry didn't"
            ));
        }
    }
    for cap in &manifest.capabilities {
        if !entry.manifest.capabilities.iter().any(|c| c == cap) {
            return Err(anyhow!(
                "artifact declares capability '{cap}' the index entry didn't"
            ));
        }
    }

    crate::plugins::plugin_store::commit_install(
        state,
        scraper,
        &bytes,
        "community",
        Some(&repo_url),
        Some(hash),
    )
    .await
}

/// Refresh all configured repositories, recording individual failures.
pub async fn refresh_all(state: &AppState) {
    let repos = match repo::list_plugin_repos(&state.read).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("listing plugin repos failed: {e:#}");
            return;
        }
    };
    let fetcher = repo_fetcher(state);
    for r in repos {
        if let Err(e) =
            fetch_and_cache(&state.marketplace, &state.write, fetcher.as_ref(), &r.url).await
        {
            tracing::warn!(repo = %r.url, "repo refresh failed: {e:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::scraper::FetchResponse;

    #[test]
    fn canonical_repo_url_trims_whitespace_and_trailing_slash() {
        let base = "https://repo.test/index.json";
        assert_eq!(canonical_repo_url(base), base);
        assert_eq!(canonical_repo_url("  https://repo.test/index.json/ "), base);
        assert_eq!(canonical_repo_url("https://repo.test/index.json/"), base);
    }

    #[test]
    fn version_newer_compares_numerically() {
        assert!(version_newer("0.3.0", "0.2.0"));
        assert!(version_newer("0.10.0", "0.9.1"));
        assert!(version_newer("1.2.1", "1.2"));
        assert!(!version_newer("0.2.0", "0.2.0"));
        assert!(
            !version_newer("0.1.9", "0.2.0"),
            "downgrade is not an update"
        );
        assert!(!version_newer("", "0.2.0"));
        assert!(
            !version_newer("1.2", "1.2.0"),
            "1.2 is not newer than 1.2.0"
        );
        assert!(version_newer("1.0.1", "1.0.0-beta"));
        assert!(!version_newer("weird", "weird"));
    }

    #[test]
    fn resolve_artifact_url_relative_and_absolute() {
        assert_eq!(
            resolve_artifact_url("https://repo.test/r/index.json", "openlibrary.wasm").unwrap(),
            "https://repo.test/r/openlibrary.wasm"
        );
        assert_eq!(
            resolve_artifact_url("https://repo.test/plugins/index.json", "../x.wasm").unwrap(),
            "https://repo.test/x.wasm"
        );
        assert_eq!(
            resolve_artifact_url("https://repo.test/i.json", "https://cdn.test/a.wasm").unwrap(),
            "https://cdn.test/a.wasm"
        );
        assert!(resolve_artifact_url("https://repo.test/i.json", "file:///etc/passwd").is_err());
        assert!(resolve_artifact_url("https://repo.test/i.json", "ftp://x/y.wasm").is_err());
    }

    struct StubFetcher(String);

    #[async_trait::async_trait]
    impl Fetcher for StubFetcher {
        async fn fetch(&self, _req: FetchRequest) -> Result<FetchResponse> {
            Ok(FetchResponse {
                status: 200,
                body: self.0.clone().into_bytes(),
            })
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fetches_validates_caches_and_keeps_last_good() {
        let wasm_dir = tempfile::tempdir().unwrap();
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/plugins/openlibrary/openlibrary.wasm"
        ))
        .expect("built by build.rs on cargo build");
        std::fs::write(wasm_dir.path().join("openlibrary.wasm"), &bytes).unwrap();
        let index = crate::plugins::plugin_index::generate_from_dir(
            wasm_dir.path(),
            Some("https://repo.test/r"),
            Some("Test repo".into()),
            tokio::runtime::Handle::current(),
        )
        .unwrap();
        let json = crate::plugins::plugin_index::to_json(&index).unwrap();

        let data = tempfile::tempdir().unwrap();
        let db = crate::server::db::connect(data.path()).await.unwrap();
        let url = "https://repo.test/r/index.json";
        repo::upsert_plugin_repo(&db.write, url, None)
            .await
            .unwrap();

        let cache = RepoCache::new();
        let name = fetch_and_cache(&cache, &db.write, &StubFetcher(json), url)
            .await
            .unwrap();
        assert_eq!(name.as_deref(), Some("Test repo"));
        let entries = cache.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, url);
        assert_eq!(entries[0].1.manifest.id, "openlibrary");
        let row = &repo::list_plugin_repos(&db.read).await.unwrap()[0];
        assert!(row.last_error.is_none() && row.last_fetched.is_some());

        let err = fetch_and_cache(&cache, &db.write, &StubFetcher("not json".into()), url).await;
        assert!(err.is_err());
        assert_eq!(cache.entries().len(), 1, "last-good kept on failed refresh");
        let row = &repo::list_plugin_repos(&db.read).await.unwrap()[0];
        assert!(row.last_error.is_some());
    }
}
