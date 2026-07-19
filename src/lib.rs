//! Shared server modules and application state.

pub mod intelligence;
pub mod media;
pub mod plugins;
pub mod repo;
pub mod routes;
pub mod scanner;
pub mod server;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use sqlx::SqlitePool;
use tokio::sync::Semaphore;

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Browse-cover hashes keyed by source URL.
#[derive(Default)]
pub struct CoverHashCache {
    inner: Mutex<HashMap<String, u64>>,
}

impl CoverHashCache {
    const CAP: usize = 20_000;

    pub fn insert(&self, url: String, hash: u64) {
        let mut m = self.inner.lock().unwrap();
        if m.len() >= Self::CAP {
            m.clear();
        }
        m.insert(url, hash);
    }

    pub fn get(&self, url: &str) -> Option<u64> {
        self.inner.lock().unwrap().get(url).copied()
    }
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<server::config::Config>,
    /// Concurrent read pool.
    pub read: SqlitePool,
    /// Single-connection write pool.
    pub write: SqlitePool,
    pub page_lists: Arc<media::pages::PageListCache>,
    pub page_thumb_locks: Arc<media::library::KeyedLocks>,
    pub blocking_limiter: Arc<Semaphore>,
    pub scrapers: Arc<plugins::scraper::ScraperRegistry>,
    pub plugin_catalog: Arc<Vec<plugins::wasm_host::BundledPlugin>>,
    pub marketplace: Arc<plugins::marketplace::RepoCache>,
    pub rate_limiter: Arc<plugins::scraper::RateLimiter>,
    pub fetcher: Arc<dyn plugins::scraper::Fetcher>,
    pub similar: Arc<intelligence::recommend::RecommendationCache<i64>>,
    pub for_you: Arc<intelligence::recommend::RecommendationCache<i64>>,
    pub corpus: Arc<intelligence::recommend::CorpusCache>,
    pub entry_corpus: Arc<intelligence::recommend::CorpusCache>,
    pub cover_hashes: Arc<CoverHashCache>,
    pub metrics: Arc<server::metrics::MetricsState>,
    pub search: Arc<intelligence::search::SearchIndex>,
}

impl AppState {
    pub fn clear_recommendation_caches(&self) {
        self.similar.clear();
        self.corpus.clear();
        self.entry_corpus.clear();
        self.for_you.clear();
    }
}
