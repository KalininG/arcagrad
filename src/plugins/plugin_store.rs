//! Plugin installation, removal, and boot restoration.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use sqlx::SqlitePool;

use crate::plugins::scraper::{Credentials, Fetcher, RateLimiter, ScraperRegistry};
use crate::plugins::wasm_host::{self, BundledPlugin};
use crate::repo;
use crate::AppState;

pub fn managed_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("plugins").join("managed")
}

/// Resolve a validated plugin id to its managed artifact path.
pub fn managed_path(data_dir: &Path, id: &str) -> Result<PathBuf> {
    if !arcagrad_plugin_sdk::is_valid_plugin_id(id) {
        return Err(anyhow!("invalid plugin id"));
    }
    Ok(managed_dir(data_dir).join(format!("{id}.wasm")))
}

/// Write through a sibling temporary file to avoid partial artifacts.
fn write_artifact(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("artifact path has no parent")?;
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension("wasm.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Install or refresh a bundled plugin.
pub async fn install(state: &AppState, id: &str) -> Result<()> {
    let bytes = state
        .plugin_catalog
        .iter()
        .find(|p| p.manifest.id == id)
        .ok_or_else(|| anyhow!("unknown plugin '{id}'"))?
        .bytes
        .to_vec();
    let scraper = load_scraper(state, bytes.clone(), "bundled").await?;
    commit_install(state, scraper, &bytes, "bundled", None, None).await
}

/// Install an uploaded artifact, rejecting ids reserved by bundled plugins.
pub async fn install_from_file(state: &AppState, bytes: Vec<u8>) -> Result<String> {
    let scraper = load_scraper(state, bytes.clone(), "local").await?;
    let id = crate::plugins::scraper::MetadataScraper::manifest(&scraper).id;
    if state.plugin_catalog.iter().any(|p| p.manifest.id == id) {
        return Err(anyhow!(
            "'{id}' is a bundled plugin — install it from the store shelf instead"
        ));
    }
    commit_install(state, scraper, &bytes, "local", None, None).await?;
    Ok(id)
}

/// Validate and instantiate artifact bytes off the async runtime.
pub async fn load_scraper(
    state: &AppState,
    bytes: Vec<u8>,
    origin: &str,
) -> Result<crate::plugins::wasm_host::WasmScraper> {
    let origin = origin.to_string();
    let fetcher = state.fetcher.clone();
    let handle = tokio::runtime::Handle::current();
    let credentials: Arc<dyn Credentials> = Arc::new(crate::plugins::scraper::DbCredentials {
        read: state.read.clone(),
        handle: handle.clone(),
    });
    tokio::task::spawn_blocking(move || {
        wasm_host::load_artifact_bytes(bytes, &origin, fetcher, credentials, handle)
    })
    .await?
}

/// Persist and hot-swap a verified scraper, replacing an existing install by id.
pub async fn commit_install(
    state: &AppState,
    scraper: crate::plugins::wasm_host::WasmScraper,
    bytes: &[u8],
    origin: &str,
    repo_url: Option<&str>,
    artifact_hash: Option<String>,
) -> Result<()> {
    let manifest = crate::plugins::scraper::MetadataScraper::manifest(&scraper);
    let id = manifest.id.clone();
    let path = managed_path(&state.config.data_dir, &id)?;
    let artifact_hash = artifact_hash.unwrap_or_else(|| wasm_host::artifact_hash(bytes));

    write_artifact(&path, bytes)?;
    register_rate_limit(&state.rate_limiter, &manifest);
    state.scrapers.insert(Arc::new(scraper));
    repo::upsert_plugin_install(
        &state.write,
        &id,
        &manifest.version,
        &artifact_hash,
        origin,
        repo_url,
    )
    .await?;
    tracing::info!(plugin = %id, origin, "plugin installed");
    Ok(())
}

/// Uninstall a plugin without deleting its credentials or kind settings.
pub async fn uninstall(state: &AppState, id: &str) -> Result<bool> {
    if !repo::delete_plugin_install(&state.write, id).await? {
        return Ok(false);
    }
    state.scrapers.remove(id);
    if let Ok(path) = managed_path(&state.config.data_dir, id) {
        let _ = std::fs::remove_file(path);
    }
    tracing::info!(plugin = %id, "plugin uninstalled");
    Ok(true)
}

fn register_rate_limit(limiter: &RateLimiter, m: &crate::plugins::scraper::ScraperManifest) {
    if let Some(policy) = m.rate_limit.clone() {
        limiter.register(&m.source, &m.hosts, policy);
    }
}

/// Restore installed plugins at boot, refreshing and repairing bundled artifacts.
#[allow(clippy::too_many_arguments)]
pub async fn boot_load(
    registry: &ScraperRegistry,
    catalog: &[BundledPlugin],
    read: &SqlitePool,
    write: &SqlitePool,
    data_dir: &Path,
    limiter: &RateLimiter,
    fetcher: Arc<dyn Fetcher>,
    credentials: Arc<dyn Credentials>,
    handle: tokio::runtime::Handle,
) -> Result<()> {
    for row in repo::list_plugin_installs(read).await? {
        let id = row.plugin_id.clone();
        let path = match managed_path(data_dir, &id) {
            Ok(p) => p,
            Err(e) => {
                repo::set_plugin_install_error(write, &id, Some(&format!("{e:#}"))).await?;
                continue;
            }
        };
        let entry = catalog.iter().find(|p| p.manifest.id == id);
        if let Some(entry) = entry {
            if entry.artifact_hash != row.artifact_hash || !path.exists() {
                if let Err(e) = write_artifact(&path, entry.bytes) {
                    repo::set_plugin_install_error(write, &id, Some(&format!("{e:#}"))).await?;
                    continue;
                }
                repo::upsert_plugin_install(
                    write,
                    &id,
                    &entry.manifest.version,
                    &entry.artifact_hash,
                    "bundled",
                    None,
                )
                .await?;
                tracing::info!(plugin = %id, version = %entry.manifest.version, "refreshed installed plugin from binary");
            }
        }
        let origin = if entry.is_some() {
            "bundled"
        } else {
            row.origin.as_str()
        };
        match wasm_host::load_artifact(
            &path,
            origin,
            fetcher.clone(),
            credentials.clone(),
            handle.clone(),
        ) {
            Ok(s) => {
                register_rate_limit(
                    limiter,
                    &crate::plugins::scraper::MetadataScraper::manifest(&s),
                );
                registry.insert(Arc::new(s));
                if row.last_error.is_some() {
                    repo::set_plugin_install_error(write, &id, None).await?;
                }
                tracing::info!(plugin = %id, "loaded installed plugin");
            }
            Err(e) => {
                tracing::error!(plugin = %id, "installed plugin failed to load: {e:#}");
                repo::set_plugin_install_error(write, &id, Some(&format!("{e:#}"))).await?;
            }
        }
    }
    Ok(())
}
