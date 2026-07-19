//! Server startup and shutdown.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use arcagrad::media::{pages::PageListCache, thumbnail};
use arcagrad::scanner::watcher;
use arcagrad::server::{auth, config::Config, db, jobs};
use arcagrad::{routes, AppState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::args().any(|a| a == "--healthcheck") {
        let bind = std::env::var("ARCA_BIND").unwrap_or_else(|_| "0.0.0.0:3000".to_string());
        let addr = bind
            .replace("0.0.0.0", "127.0.0.1")
            .replace("[::]", "[::1]");
        let ok = reqwest::Client::new()
            .get(format!("http://{addr}/health"))
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .is_ok_and(|r| r.status().is_success());
        std::process::exit(i32::from(!ok));
    }

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,tower_http=info")),
        )
        .init();

    let config = Arc::new(Config::from_env()?);

    thumbnail::init_vips()?;

    // SQLite WAL requires a local data directory.
    db::guard_data_dir_local(&config.data_dir)?;

    let db = db::connect(&config.data_dir).await?;
    tracing::info!("database ready at {}/arca.db", config.data_dir.display());

    arcagrad::plugins::browse_cache::BrowseCache::new(&config.data_dir)
        .clear()
        .await;

    if auth::count_users(&db.read).await? == 0 {
        tracing::warn!("no users yet — create the admin account via POST /api/auth/setup");
    }

    if let Err(e) = arcagrad::repo::prune_cover_hashes(&db.write).await {
        tracing::warn!("pruning cover hashes failed: {e:#}");
    }

    let http_fetcher = arcagrad::plugins::scraper::HttpFetcher::new();
    let rate_limiter = http_fetcher.limiter();
    let fetcher: Arc<dyn arcagrad::plugins::scraper::Fetcher> = Arc::new(http_fetcher);
    let credentials: Arc<dyn arcagrad::plugins::scraper::Credentials> =
        Arc::new(arcagrad::plugins::scraper::DbCredentials {
            read: db.read.clone(),
            handle: tokio::runtime::Handle::current(),
        });
    let plugin_catalog = Arc::new(arcagrad::plugins::wasm_host::bundled_catalog(
        fetcher.clone(),
        credentials.clone(),
        tokio::runtime::Handle::current(),
    ));
    let scrapers = Arc::new(arcagrad::plugins::scraper::ScraperRegistry::new());
    arcagrad::plugins::plugin_store::boot_load(
        &scrapers,
        &plugin_catalog,
        &db.read,
        &db.write,
        &config.data_dir,
        &rate_limiter,
        fetcher.clone(),
        credentials.clone(),
        tokio::runtime::Handle::current(),
    )
    .await?;
    for s in arcagrad::plugins::wasm_host::load_plugins(
        &config.data_dir.join("plugins"),
        fetcher.clone(),
        credentials.clone(),
        tokio::runtime::Handle::current(),
    ) {
        // Installed plugins take precedence over loose files.
        if scrapers.get(&s.manifest().id).is_none() {
            if let Some(policy) = s.manifest().rate_limit {
                rate_limiter.register(&s.manifest().source, &s.manifest().hosts, policy);
            }
            scrapers.push(s);
        }
    }

    let search = Arc::new(arcagrad::intelligence::search::SearchIndex::open_or_create(
        config.data_dir.join("search"),
    )?);

    let state = AppState {
        config: config.clone(),
        read: db.read,
        write: db.write,
        page_lists: Arc::new(PageListCache::new(4096)),
        page_thumb_locks: Arc::new(arcagrad::media::library::KeyedLocks::default()),
        blocking_limiter: Arc::new(Semaphore::new(config.read_concurrency)),
        scrapers,
        plugin_catalog,
        marketplace: Arc::new(arcagrad::plugins::marketplace::RepoCache::new()),
        rate_limiter: rate_limiter.clone(),
        fetcher,
        similar: Arc::new(arcagrad::intelligence::recommend::RecommendationCache::new(
            512,
        )),
        corpus: Arc::new(arcagrad::intelligence::recommend::CorpusCache::new()),
        entry_corpus: Arc::new(arcagrad::intelligence::recommend::CorpusCache::new()),
        cover_hashes: Arc::new(arcagrad::CoverHashCache::default()),
        metrics: Arc::new(arcagrad::server::metrics::MetricsState::default()),
        for_you: Arc::new(arcagrad::intelligence::recommend::RecommendationCache::new(
            512,
        )),
        search,
    };
    tracing::info!(
        "heavy-read concurrency capped at {}",
        config.read_concurrency
    );

    let cancel = CancellationToken::new();
    jobs::reset_running(&state.write).await?;
    jobs::enqueue(&state.write, "scan", None).await?;
    let workers = jobs::spawn_workers(state.clone(), 2, cancel.clone());
    tracing::info!(
        "workers started; queued initial scan of {}",
        config.content_dir.display()
    );

    watcher::spawn(state.clone(), cancel.clone());

    // Repository failures must not delay startup.
    {
        let state = state.clone();
        tokio::spawn(async move { arcagrad::plugins::marketplace::refresh_all(&state).await });
    }

    if let Err(e) = jobs::schedule_plugin_update_check(&state.write).await {
        tracing::warn!("scheduling plugin update check failed: {e:#}");
    }
    if let Err(e) = jobs::schedule_follow_check(&state.write).await {
        tracing::warn!("scheduling follow check failed: {e:#}");
    }
    if let Err(e) = jobs::schedule_calendar_check(&state.write).await {
        tracing::warn!("scheduling calendar check failed: {e:#}");
    }

    let app = routes::router(state);

    let listener = tokio::net::TcpListener::bind(&config.bind).await?;
    tracing::info!("listening on http://{}", config.bind);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(cancel))
        .await?;

    // Give workers a bounded window to finish.
    tracing::info!("draining background workers… (Ctrl-C again to exit now)");
    tokio::select! {
        _ = workers.wait() => {}
        _ = tokio::time::sleep(Duration::from_secs(10)) => {
            tracing::warn!("background workers did not finish within 10s; exiting anyway");
        }
        _ = wait_for_signal() => {
            tracing::warn!("second signal received; exiting immediately");
        }
    }

    Ok(())
}

/// Wait for SIGINT or SIGTERM.
async fn wait_for_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

async fn shutdown_signal(cancel: CancellationToken) {
    wait_for_signal().await;
    tracing::info!("shutdown signal received, draining");
    cancel.cancel();
}
