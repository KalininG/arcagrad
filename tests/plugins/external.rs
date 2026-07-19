//! Ignored live-service checks for bundled plugins.
//!
//!     cargo test --test plugins -- --ignored

use std::sync::Arc;

use arcagrad::plugins::scraper::{
    Credentials, Fetcher, HttpFetcher, MetadataScraper, NoCredentials, ScrapeHint,
};
use arcagrad::plugins::wasm_host::WasmScraper;
use tokio::runtime::Handle;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: hits graphql.anilist.co; run with `-- --ignored`"]
async fn anilist_live_scrape() {
    let bytes =
        std::fs::read("plugins/anilist/anilist.wasm").expect("built by build.rs on cargo build");
    let fetcher: Arc<dyn Fetcher> = Arc::new(HttpFetcher::new());
    let creds: Arc<dyn Credentials> = Arc::new(NoCredentials);
    let scraper = WasmScraper::from_bytes(bytes, fetcher.clone(), creds, Handle::current())
        .expect("anilist plugin loads");

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
            fetcher.as_ref(),
        )
        .await
        .expect("anilist search request/parse");
    assert!(
        !cands.is_empty(),
        "anilist search returned nothing — API or plugin drift"
    );

    let meta = scraper
        .fetch_details(&cands[0], fetcher.as_ref())
        .await
        .expect("anilist detail fetch/parse");
    assert!(meta.title.is_some(), "detail missing title — shape drift");
    assert!(
        !meta.mapped_tags.is_empty(),
        "detail returned no tags — shape drift"
    );
    assert!(
        meta.source_url
            .as_deref()
            .is_some_and(|u| u.starts_with("https://anilist.co/manga/")),
        "source_url should be the canonical AniList site URL, got {:?}",
        meta.source_url
    );
    assert!(
        meta.comments.is_empty(),
        "AniList has no gallery-style comments"
    );

    let by_ref = scraper
        .search(
            &ScrapeHint {
                title: "ignored".into(),
                display_title: None,
                author: None,
                modality: None,
                page_count: None,
                reference: Some("https://anilist.co/manga/34632/Oyasumi-Punpun".into()),
            },
            fetcher.as_ref(),
        )
        .await
        .expect("anilist reference resolution");
    assert_eq!(by_ref.len(), 1);
    assert_eq!(by_ref[0].id, "34632");
    let ref_meta = scraper
        .fetch_details(&by_ref[0], fetcher.as_ref())
        .await
        .expect("anilist fetch_details by reference");
    assert!(
        ref_meta
            .title
            .as_deref()
            .is_some_and(|t| t.to_lowercase().contains("punpun")),
        "expected a Punpun title, got {:?} — API or id drift",
        ref_meta.title
    );
}
