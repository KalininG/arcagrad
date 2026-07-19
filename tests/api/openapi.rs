//! Drift checks for the generated OpenAPI operations and security schemes.

use std::sync::Arc;

use arcagrad::server::{config::Config, db};
use arcagrad::{routes, AppState};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn app() -> (Router, tempfile::TempDir, tempfile::TempDir) {
    let content = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let db = db::connect(data.path()).await.unwrap();
    let config = Arc::new(Config {
        content_dir: content.path().to_path_buf(),
        data_dir: data.path().to_path_buf(),
        bind: "0.0.0.0:0".into(),
        cookie_secure: false,
        read_concurrency: 8,
        allow_private_repos: false,
        watch: false,
    });
    let router = routes::router(AppState {
        config,
        read: db.read,
        write: db.write,
        page_lists: Arc::new(arcagrad::media::pages::PageListCache::new(64)),
        page_thumb_locks: Arc::new(arcagrad::media::library::KeyedLocks::default()),
        blocking_limiter: Arc::new(tokio::sync::Semaphore::new(8)),
        scrapers: Arc::new(arcagrad::plugins::scraper::ScraperRegistry::new()),
        plugin_catalog: std::sync::Arc::new(Vec::new()),
        marketplace: std::sync::Arc::new(arcagrad::plugins::marketplace::RepoCache::new()),
        rate_limiter: std::sync::Arc::new(arcagrad::plugins::scraper::RateLimiter::default()),
        fetcher: Arc::new(arcagrad::plugins::scraper::HttpFetcher::new()),
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
            arcagrad::intelligence::search::SearchIndex::open_or_create(data.path().join("search"))
                .unwrap(),
        ),
    });
    (router, content, data)
}

#[tokio::test]
async fn openapi_spec_covers_every_route() {
    let (app, _c, _d) = app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "spec endpoint must serve");

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let spec: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    let mut ops: Vec<String> = Vec::new();
    for (path, item) in spec["paths"].as_object().unwrap() {
        for method in ["get", "post", "put", "delete", "patch"] {
            if let Some(operation) = item.get(method) {
                ops.push(format!("{} {path}", method.to_uppercase()));
                assert!(
                    operation["summary"]
                        .as_str()
                        .is_some_and(|summary| !summary.trim().is_empty()),
                    "{method} {path} is missing a summary"
                );
            }
        }
    }
    ops.sort();

    let mut expected: Vec<String> = [
        "GET /health",
        "GET /api/auth/status",
        "POST /api/auth/setup",
        "POST /api/auth/login",
        "POST /api/auth/register",
        "GET /api/settings/auth",
        "PUT /api/settings/auth",
        "GET /api/settings/kind-access",
        "PUT /api/settings/kind-access",
        "POST /api/auth/logout",
        "GET /api/me",
        "PUT /api/auth/password",
        "POST /api/auth/logout-all",
        "GET /api/me/avatar",
        "PUT /api/me/avatar",
        "DELETE /api/me/avatar",
        "GET /api/me/banner",
        "PUT /api/me/banner",
        "DELETE /api/me/banner",
        "GET /api/users",
        "GET /api/users/stats",
        "POST /api/users",
        "DELETE /api/users/{id}",
        "PUT /api/users/{id}/password",
        "GET /api/users/{id}/avatar",
        "GET /api/auth/keys",
        "POST /api/auth/keys",
        "DELETE /api/auth/keys/{id}",
        "GET /api/tags",
        "GET /api/tags/favorites",
        "GET /api/suggest",
        "GET /api/me/tag-blocklist",
        "PUT /api/me/tag-blocklist",
        "GET /api/kinds",
        "GET /api/kinds/{kind}/plugins",
        "PUT /api/kinds/{kind}/plugins",
        "GET /api/plugins",
        "GET /api/plugin-catalog",
        "POST /api/plugin-installs",
        "POST /api/plugin-installs/file",
        "DELETE /api/plugin-installs/{id}",
        "GET /api/plugin-repos",
        "POST /api/plugin-repos",
        "DELETE /api/plugin-repos",
        "POST /api/plugin-repos/refresh",
        "GET /api/follows",
        "POST /api/follows",
        "DELETE /api/follows/{id}",
        "POST /api/follows/check",
        "GET /api/follows/{id}/items",
        "POST /api/follows/{id}/items/state",
        "POST /api/follows/{id}/items/dismiss-all",
        "GET /api/plugins/{id}/browse",
        "GET /api/plugins/{id}/icon",
        "GET /api/plugins/{id}/image",
        "GET /api/plugins/{id}/item",
        "GET /api/plugins/{id}/pages",
        "POST /api/browse/match",
        "GET /api/credentials",
        "PUT /api/credentials/{source}",
        "DELETE /api/credentials/{source}",
        "GET /api/items",
        "POST /api/items",
        "GET /api/items/continue",
        "GET /api/items/finished",
        "GET /api/items/{id}",
        "PUT /api/items/{id}/metadata",
        "DELETE /api/items/{id}/sources/{source}",
        "DELETE /api/items/{id}",
        "GET /api/series/{id}",
        "PUT /api/series/{id}/metadata",
        "DELETE /api/series/{id}/sources/{source}",
        "POST /api/series/{id}/favorite",
        "DELETE /api/series/{id}/favorite",
        "PUT /api/series/{id}/rating",
        "DELETE /api/series/{id}/rating",
        "POST /api/series/{id}/scrape",
        "GET /api/trackers",
        "GET /api/series/{id}/trackers",
        "PUT /api/series/{id}/trackers/{plugin}",
        "DELETE /api/series/{id}/trackers/{plugin}",
        "GET /api/upcoming",
        "POST /api/upcoming/refresh",
        "GET /api/items/{id}/similar",
        "GET /api/series/{id}/similar",
        "GET /api/recommendations",
        "GET /api/me/stats",
        "GET /api/items/{id}/download",
        "GET /api/items/{id}/pages/{n}",
        "GET /api/items/{id}/pages/{n}/thumbnail",
        "GET /api/items/{id}/thumbnail",
        "GET /api/items/{id}/manifest",
        "PUT /api/items/{id}/progress",
        "POST /api/items/{id}/tags",
        "DELETE /api/items/{id}/tags",
        "POST /api/items/{id}/favorite",
        "DELETE /api/items/{id}/favorite",
        "PUT /api/items/{id}/rating",
        "DELETE /api/items/{id}/rating",
        "PUT /api/items/{id}/reading-mode",
        "DELETE /api/items/{id}/reading-mode",
        "POST /api/items/{id}/scrape",
        "POST /api/items/{id}/identify",
        "POST /api/plugins/{id}/download",
        "GET /api/jobs/{id}",
        "POST /api/rescan",
        "GET /api/metrics",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    expected.sort();

    assert_eq!(
        ops, expected,
        "OpenAPI operation set drifted from the router"
    );

    let schemes = spec["components"]["securitySchemes"].as_object().unwrap();
    assert!(
        schemes.contains_key("sessionCookie"),
        "missing sessionCookie"
    );
    assert!(schemes.contains_key("apiKey"), "missing apiKey");

    assert!(spec["components"]["schemas"]["ErrorResponse"].is_object());
    for (path, item) in spec["paths"].as_object().unwrap() {
        for method in ["get", "post", "put", "delete", "patch"] {
            let Some(operation) = item.get(method) else {
                continue;
            };
            for (status, response) in operation["responses"].as_object().unwrap() {
                if status.starts_with('4') || status.starts_with('5') {
                    assert_eq!(
                        response["content"]["application/json"]["schema"]["$ref"],
                        "#/components/schemas/ErrorResponse",
                        "{method} {path} {status} must document ErrorResponse"
                    );
                }
            }
        }
    }

    let item_upload = &spec["paths"]["/api/items"]["post"]["requestBody"]["content"]
        ["multipart/form-data"]["schema"];
    assert_eq!(item_upload["$ref"], "#/components/schemas/ItemUploadForm");
    let plugin_upload = &spec["paths"]["/api/plugin-installs/file"]["post"]["requestBody"]
        ["content"]["multipart/form-data"]["schema"];
    assert_eq!(
        plugin_upload["$ref"],
        "#/components/schemas/PluginFileUploadForm"
    );
    for schema_name in ["ItemUploadForm", "PluginFileUploadForm"] {
        let file = &spec["components"]["schemas"][schema_name]["properties"]["file"];
        assert_eq!(file["type"], "string");
        assert_eq!(file["format"], "binary");
    }
}
