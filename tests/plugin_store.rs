//! Plugin installation and boot restoration tests.

use std::sync::Arc;

use arcagrad::plugins::scraper::{
    Credentials, DbCredentials, Fetcher, HttpFetcher, MetadataScraper, RateLimiter, ScraperRegistry,
};
use arcagrad::plugins::wasm_host::{artifact_hash, BundledPlugin};
use arcagrad::plugins::{plugin_store, wasm_host};
use arcagrad::repo;

fn openlibrary_bytes() -> &'static [u8] {
    Box::leak(
        std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/plugins/openlibrary/openlibrary.wasm"
        ))
        .unwrap()
        .into_boxed_slice(),
    )
}

#[tokio::test]
async fn boot_refreshes_upgraded_install_and_flags_broken_ones() {
    let data = tempfile::tempdir().unwrap();
    let db = arcagrad::server::db::connect(data.path()).await.unwrap();
    let fetcher: Arc<dyn Fetcher> = Arc::new(HttpFetcher::new());
    let credentials: Arc<dyn Credentials> = Arc::new(DbCredentials {
        read: db.read.clone(),
        handle: tokio::runtime::Handle::current(),
    });
    let handle = tokio::runtime::Handle::current();

    let wasm = openlibrary_bytes();
    let inspected = wasm_host::load_artifact_bytes(
        wasm.to_vec(),
        "bundled",
        fetcher.clone(),
        credentials.clone(),
        handle.clone(),
    )
    .unwrap();
    let catalog = vec![BundledPlugin {
        manifest: inspected.manifest(),
        icon: None,
        artifact_hash: artifact_hash(wasm),
        bytes: wasm,
    }];

    let managed = plugin_store::managed_path(data.path(), "openlibrary").unwrap();
    std::fs::create_dir_all(managed.parent().unwrap()).unwrap();
    std::fs::write(&managed, b"old artifact bytes").unwrap();
    repo::upsert_plugin_install(
        &db.write,
        "openlibrary",
        "0.1.0",
        "stale-hash",
        "bundled",
        None,
    )
    .await
    .unwrap();
    let broken = plugin_store::managed_path(data.path(), "broken").unwrap();
    std::fs::write(&broken, b"not a wasm file").unwrap();
    repo::upsert_plugin_install(&db.write, "broken", "1.0.0", "whatever", "local", None)
        .await
        .unwrap();

    let registry = ScraperRegistry::new();
    let limiter = RateLimiter::default();
    plugin_store::boot_load(
        &registry,
        &catalog,
        &db.read,
        &db.write,
        data.path(),
        &limiter,
        fetcher,
        credentials,
        handle,
    )
    .await
    .unwrap();

    assert_eq!(std::fs::read(&managed).unwrap(), wasm, "artifact refreshed");
    let rows = repo::list_plugin_installs(&db.read).await.unwrap();
    let nh = rows.iter().find(|r| r.plugin_id == "openlibrary").unwrap();
    assert_eq!(nh.artifact_hash, artifact_hash(wasm));
    assert_eq!(nh.version, inspected.manifest().version);
    assert!(nh.last_error.is_none());
    assert!(registry.get("openlibrary").is_some(), "hot-loaded at boot");

    let br = rows.iter().find(|r| r.plugin_id == "broken").unwrap();
    assert!(br.last_error.is_some(), "load failure recorded");
    assert!(registry.get("broken").is_none());

    assert_eq!(registry.ids(), vec!["openlibrary".to_string()]);
}

mod community {
    use super::*;
    use arcagrad::plugins::scraper::{FetchRequest, FetchResponse};
    use std::collections::HashMap;

    struct RepoStub(HashMap<String, Vec<u8>>);

    #[async_trait::async_trait]
    impl Fetcher for RepoStub {
        async fn fetch(&self, req: FetchRequest) -> anyhow::Result<FetchResponse> {
            match self.0.get(&req.url) {
                Some(body) => Ok(FetchResponse {
                    status: 200,
                    body: body.clone(),
                }),
                None => Ok(FetchResponse {
                    status: 404,
                    body: Vec::new(),
                }),
            }
        }
    }

    fn app_state(
        data: &tempfile::TempDir,
        db: &arcagrad::server::db::Pools,
        fetcher: Arc<dyn Fetcher>,
    ) -> arcagrad::AppState {
        arcagrad::AppState {
            config: Arc::new(arcagrad::server::config::Config {
                content_dir: data.path().join("content"),
                data_dir: data.path().to_path_buf(),
                bind: "0.0.0.0:0".into(),
                cookie_secure: false,
                read_concurrency: 4,
                allow_private_repos: false,
                watch: false,
            }),
            read: db.read.clone(),
            write: db.write.clone(),
            page_lists: Arc::new(arcagrad::media::pages::PageListCache::new(8)),
            page_thumb_locks: Arc::new(arcagrad::media::library::KeyedLocks::default()),
            blocking_limiter: Arc::new(tokio::sync::Semaphore::new(4)),
            scrapers: Arc::new(ScraperRegistry::new()),
            plugin_catalog: Arc::new(Vec::new()),
            marketplace: Arc::new(arcagrad::plugins::marketplace::RepoCache::new()),
            rate_limiter: Arc::new(RateLimiter::default()),
            fetcher,
            similar: Arc::new(arcagrad::intelligence::recommend::RecommendationCache::new(
                8,
            )),
            for_you: Arc::new(arcagrad::intelligence::recommend::RecommendationCache::new(
                8,
            )),
            corpus: Arc::new(arcagrad::intelligence::recommend::CorpusCache::new()),
            entry_corpus: Arc::new(arcagrad::intelligence::recommend::CorpusCache::new()),
            cover_hashes: Arc::new(arcagrad::CoverHashCache::default()),
            metrics: Arc::new(arcagrad::server::metrics::MetricsState::default()),
            search: Arc::new(
                arcagrad::intelligence::search::SearchIndex::open_or_create(
                    data.path().join("search"),
                )
                .unwrap(),
            ),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn installs_from_repo_and_refuses_tampered_artifact() {
        let wasm = openlibrary_bytes();
        let wasm_dir = tempfile::tempdir().unwrap();
        std::fs::write(wasm_dir.path().join("openlibrary.wasm"), wasm).unwrap();
        let index = arcagrad::plugins::plugin_index::generate_from_dir(
            wasm_dir.path(),
            None,
            Some("Test repo".into()),
            tokio::runtime::Handle::current(),
        )
        .unwrap();
        assert_eq!(index.plugins[0].artifact_url, "openlibrary.wasm");
        let index_json = arcagrad::plugins::plugin_index::to_json(&index).unwrap();

        let data = tempfile::tempdir().unwrap();
        let db = arcagrad::server::db::connect(data.path()).await.unwrap();
        let index_url = "https://repo.test/r/index.json";
        let stub: Arc<dyn Fetcher> = Arc::new(RepoStub(HashMap::from([
            (index_url.to_string(), index_json.clone().into_bytes()),
            (
                "https://repo.test/r/openlibrary.wasm".to_string(),
                wasm.to_vec(),
            ),
        ])));
        let state = app_state(&data, &db, stub);

        arcagrad::plugins::marketplace::add_repo(&state, index_url)
            .await
            .unwrap();
        let dup = arcagrad::plugins::marketplace::add_repo(&state, index_url).await;
        assert!(format!("{:#}", dup.unwrap_err()).contains("already added"));
        let dup2 =
            arcagrad::plugins::marketplace::add_repo(&state, &format!("  {index_url}/ ")).await;
        assert!(format!("{:#}", dup2.unwrap_err()).contains("already added"));
        arcagrad::plugins::marketplace::install_from_repo(&state, "openlibrary")
            .await
            .unwrap();
        let m = state.scrapers.get("openlibrary").unwrap().manifest();
        assert_eq!(m.origin, "community", "host-stamped provenance");
        assert!(plugin_store::managed_path(data.path(), "openlibrary")
            .unwrap()
            .exists());
        let rows = repo::list_plugin_installs(&db.read).await.unwrap();
        assert_eq!(rows[0].origin, "community");
        assert_eq!(
            rows[0].repo_url.as_deref(),
            Some(index_url),
            "source repo persisted for provenance + updates"
        );

        assert!(
            arcagrad::plugins::marketplace::remove_repo(&state, index_url)
                .await
                .unwrap()
        );
        assert!(state.scrapers.get("openlibrary").is_none());
        assert!(!plugin_store::managed_path(data.path(), "openlibrary")
            .unwrap()
            .exists());
        assert!(repo::list_plugin_installs(&db.read)
            .await
            .unwrap()
            .is_empty());

        arcagrad::plugins::marketplace::add_repo(&state, index_url)
            .await
            .unwrap();
        arcagrad::plugins::marketplace::install_from_repo(&state, "openlibrary")
            .await
            .unwrap();

        let evil: Arc<dyn Fetcher> = Arc::new(RepoStub(HashMap::from([
            (index_url.to_string(), index_json.into_bytes()),
            (
                "https://repo.test/r/openlibrary.wasm".to_string(),
                b"malicious bytes".to_vec(),
            ),
        ])));
        let state2 = {
            let data2 = tempfile::tempdir().unwrap();
            let db2 = arcagrad::server::db::connect(data2.path()).await.unwrap();
            let s = app_state(&data2, &db2, evil);
            arcagrad::plugins::marketplace::add_repo(&s, index_url)
                .await
                .unwrap();
            let err = arcagrad::plugins::marketplace::install_from_repo(&s, "openlibrary")
                .await
                .unwrap_err();
            assert!(format!("{err:#}").contains("hash mismatch"));
            assert!(s.scrapers.get("openlibrary").is_none(), "nothing installed");
            assert!(repo::list_plugin_installs(&db2.read)
                .await
                .unwrap()
                .is_empty());
            (data2, db2)
        };
        drop(state2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn install_from_file_local_reserved_and_invalid() {
        let data = tempfile::tempdir().unwrap();
        let db = arcagrad::server::db::connect(data.path()).await.unwrap();
        let fetcher: Arc<dyn Fetcher> = Arc::new(HttpFetcher::new());
        let mut state = app_state(&data, &db, fetcher.clone());
        let creds: Arc<dyn Credentials> = Arc::new(arcagrad::plugins::scraper::NoCredentials);
        state.plugin_catalog = Arc::new(wasm_host::bundled_catalog(
            fetcher,
            creds,
            tokio::runtime::Handle::current(),
        ));

        let reserved = plugin_store::install_from_file(&state, openlibrary_bytes().to_vec()).await;
        assert!(
            format!("{:#}", reserved.unwrap_err()).contains("bundled"),
            "a bundled id must be reserved"
        );
        assert!(state.scrapers.get("openlibrary").is_none());

        let junk = plugin_store::install_from_file(&state, b"not a wasm module".to_vec()).await;
        assert!(junk.is_err(), "invalid artifact must be refused");
        assert!(repo::list_plugin_installs(&db.read)
            .await
            .unwrap()
            .is_empty());
    }
}
