//! API integration tests over temporary storage and the real router.

use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use arcagrad::server::{config::Config, db};
use arcagrad::{routes, scanner, AppState};
use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tower::ServiceExt;

const CREDS: &str = r#"{"username":"admin","password":"password123"}"#;

fn write_cbz(path: &Path, content: &str) {
    let f = std::fs::File::create(path).unwrap();
    let mut z = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default();
    for p in 1..=3 {
        z.start_file(format!("{p:03}.jpg"), opts).unwrap();
        z.write_all(format!("dummy-jpeg-{content}-{p}").as_bytes())
            .unwrap();
    }
    z.finish().unwrap();
}

fn write_epub(path: &Path) {
    let f = std::fs::File::create(path).unwrap();
    let mut z = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default();
    let mut put = |name: &str, data: &[u8]| {
        z.start_file(name, opts).unwrap();
        z.write_all(data).unwrap();
    };
    put(
        "META-INF/container.xml",
        br#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
    );
    put(
        "OEBPS/content.opf",
        br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>A Real Book</dc:title><dc:creator>Jane Author</dc:creator><dc:language>en</dc:language></metadata><manifest><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="cov" href="cover.jpg" media-type="image/jpeg" properties="cover-image"/><item id="c1" href="text/ch1.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="text/ch2.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/><itemref idref="c2"/></spine></package>"#,
    );
    put(
        "OEBPS/nav.xhtml",
        br#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><body><nav epub:type="toc"><ol><li><a href="text/ch1.xhtml">Chapter One</a></li><li><a href="text/ch2.xhtml">Chapter Two</a></li></ol></nav></body></html>"#,
    );
    put("OEBPS/cover.jpg", b"\xff\xd8\xffcover");
    put(
        "OEBPS/text/ch1.xhtml",
        b"<html><body>Chapter One</body></html>",
    );
    put(
        "OEBPS/text/ch2.xhtml",
        b"<html><body>Chapter Two</body></html>",
    );
    z.finish().unwrap();
}

fn write_chaptered_cbz(path: &Path, preamble: usize, chapters: usize, pages_each: usize) {
    let f = std::fs::File::create(path).unwrap();
    let mut z = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default();
    let mut put = |name: String, seed: String| {
        z.start_file(name, opts).unwrap();
        z.write_all(format!("img-{seed}").as_bytes()).unwrap();
    };
    for p in 1..=preamble {
        put(format!("{p:04}_0000.jpg"), format!("pre{p}"));
    }
    for ch in 1..=chapters {
        for p in 1..=pages_each {
            put(format!("CH{ch:03}_{p:03}.jpg"), format!("c{ch}p{p}"));
        }
    }
    z.finish().unwrap();
}

async fn build_app(n: usize) -> (Router, tempfile::TempDir, tempfile::TempDir) {
    let (state, content, data) = build_state(n).await;
    (routes::router(state), content, data)
}

async fn build_state(n: usize) -> (AppState, tempfile::TempDir, tempfile::TempDir) {
    let content = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    for i in 0..n {
        write_cbz(
            &content.path().join(format!("book-{i}.cbz")),
            &format!("c{i}"),
        );
    }

    let db = db::connect(data.path()).await.unwrap();
    scanner::scan(&db.write, content.path()).await.unwrap();

    let config = Arc::new(Config {
        content_dir: content.path().to_path_buf(),
        data_dir: data.path().to_path_buf(),
        bind: "0.0.0.0:0".into(),
        cookie_secure: false,
        read_concurrency: 8,
        allow_private_repos: false,
        watch: false,
    });
    let search = Arc::new(
        arcagrad::intelligence::search::SearchIndex::open_or_create(data.path().join("search"))
            .unwrap(),
    );
    search.rebuild_from_db(&db.read).await.unwrap();
    let state = AppState {
        config,
        read: db.read,
        write: db.write,
        page_lists: Arc::new(arcagrad::media::pages::PageListCache::new(64)),
        page_thumb_locks: Arc::new(arcagrad::media::library::KeyedLocks::default()),
        blocking_limiter: Arc::new(tokio::sync::Semaphore::new(8)),
        scrapers: Arc::new({
            let r = arcagrad::plugins::scraper::ScraperRegistry::new();
            r.push(Box::new(arcagrad::plugins::scraper::ExampleScraper::new()));
            r
        }),
        plugin_catalog: std::sync::Arc::new(Vec::new()),
        marketplace: std::sync::Arc::new(arcagrad::plugins::marketplace::RepoCache::new()),
        rate_limiter: std::sync::Arc::new(arcagrad::plugins::scraper::RateLimiter::default()),
        fetcher: Arc::new(arcagrad::plugins::scraper::HttpFetcher::new()),
        similar: Arc::new(arcagrad::intelligence::recommend::RecommendationCache::new(
            64,
        )),
        for_you: Arc::new(arcagrad::intelligence::recommend::RecommendationCache::new(
            64,
        )),
        corpus: Arc::new(arcagrad::intelligence::recommend::CorpusCache::new()),
        entry_corpus: Arc::new(arcagrad::intelligence::recommend::CorpusCache::new()),
        cover_hashes: Arc::new(arcagrad::CoverHashCache::default()),
        metrics: Arc::new(arcagrad::server::metrics::MetricsState::default()),
        search,
    };
    (state, content, data)
}

async fn drain_jobs(state: &AppState) {
    let cancel = tokio_util::sync::CancellationToken::new();
    loop {
        let row: Option<(i64, String, Option<String>)> = sqlx::query_as(
            "SELECT id, kind, payload FROM jobs \
             WHERE state = 'pending' AND run_after <= unixepoch() ORDER BY id LIMIT 1",
        )
        .fetch_optional(&state.read)
        .await
        .unwrap();
        let Some((id, kind, payload)) = row else {
            break;
        };
        let job = arcagrad::server::jobs::Job {
            id,
            kind,
            payload,
            attempts: 1,
        };
        arcagrad::server::jobs::run_job(state, &job, &cancel)
            .await
            .unwrap();
        sqlx::query("UPDATE jobs SET state = 'done' WHERE id = ?")
            .bind(id)
            .execute(&state.write)
            .await
            .unwrap();
    }
}

async fn app_with_auth(n: usize) -> (Router, String, tempfile::TempDir, tempfile::TempDir) {
    let (app, c, d) = build_app(n).await;
    let resp = send(&app, "POST", "/api/auth/setup", None, Some(CREDS)).await;
    assert_eq!(resp.status(), StatusCode::OK, "admin setup failed");
    let cookie = cookie_of(&resp);
    (app, cookie, c, d)
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    json_body: Option<&str>,
) -> Response<Body> {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(c) = cookie {
        b = b.header("cookie", c);
    }
    let body = match json_body {
        Some(j) => {
            b = b.header("content-type", "application/json");
            Body::from(j.to_string())
        }
        None => Body::empty(),
    };
    app.clone().oneshot(b.body(body).unwrap()).await.unwrap()
}

async fn send_bearer(
    app: &Router,
    method: &str,
    uri: &str,
    key: &str,
    body: Option<&str>,
) -> Response<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {key}"));
    let body = match body {
        Some(j) => {
            b = b.header("content-type", "application/json");
            Body::from(j.to_string())
        }
        None => Body::empty(),
    };
    app.clone().oneshot(b.body(body).unwrap()).await.unwrap()
}

fn cookie_of(resp: &Response<Body>) -> String {
    resp.headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

async fn json_of(resp: Response<Body>) -> serde_json::Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn suggest_blends_tags_and_titles() {
    let (app, cookie, _c, _d) = app_with_auth(3).await;

    let r = send(&app, "GET", "/api/suggest?q=book", None, None).await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    let r = send(&app, "GET", "/api/suggest?q=book", Some(&cookie), None).await;
    assert_eq!(r.status(), StatusCode::OK);
    let body = json_of(r).await;
    let results = body["results"].as_array().unwrap();
    let t = results
        .iter()
        .find(|s| s["type"] == "title")
        .unwrap_or_else(|| panic!("expected a title suggestion for 'book', got {results:?}"));
    assert!(t["id"].is_number());
    assert!(t["cover_version"].is_string());

    let r = send(&app, "GET", "/api/suggest?q=", Some(&cookie), None).await;
    assert!(json_of(r).await["results"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn stats_requires_auth_and_returns_shape() {
    let (app, cookie, _c, _d) = app_with_auth(2).await;
    let r = send(&app, "GET", "/api/me/stats", None, None).await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    let r = send(&app, "GET", "/api/me/stats", Some(&cookie), None).await;
    let body = json_of(r).await;
    for k in [
        "totals",
        "by_kind",
        "ratings",
        "favorites",
        "series",
        "top",
        "activity",
    ] {
        assert!(body.get(k).is_some(), "stats missing '{k}': {body}");
    }
    assert!(body["totals"]["started"].is_number());
    assert!(body["activity"]["days"].is_array());
    assert!(body["ratings"]["distribution"].is_array());
}

#[tokio::test]
async fn relevance_sort_ranks_by_query() {
    let (app, cookie, _c, _d) = app_with_auth(5).await;

    let r = send(
        &app,
        "GET",
        "/api/items?sort=relevance",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);

    let r = send(
        &app,
        "GET",
        "/api/items?sort=relevance&q=book-2",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);
    let body = json_of(r).await;
    let items = body["items"].as_array().unwrap();
    assert!(!items.is_empty(), "relevance search returns matches");
    assert_eq!(items[0]["type"], "item", "relevance results are flat items");
    assert_eq!(
        items[0]["name"], "book-2",
        "the best BM25 match ranks first, got {items:?}"
    );
    assert!(body["total"].as_i64().unwrap() >= 1);
    assert!(body["page"].as_i64().is_some());
    assert!(body["next_cursor"].is_null());
}

fn cbz_bytes(tag: &str) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default();
        z.start_file("001.jpg", opts).unwrap();
        z.write_all(format!("dummy-jpeg-{tag}").as_bytes()).unwrap();
        z.finish().unwrap();
    }
    buf
}

const BOUNDARY: &str = "ARCATESTBOUNDARY";

fn multipart_body(parts: &[(&str, Option<&str>, &[u8])]) -> (String, Vec<u8>) {
    let mut body = Vec::new();
    for (name, filename, content) in parts {
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        match filename {
            Some(fname) => body.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"{name}\"; filename=\"{fname}\"\r\n\
                     Content-Type: application/octet-stream\r\n\r\n"
                )
                .as_bytes(),
            ),
            None => body.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
            ),
        }
        body.extend_from_slice(content);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={BOUNDARY}"), body)
}

async fn send_raw(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    content_type: &str,
    body: Vec<u8>,
) -> Response<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", content_type);
    if let Some(c) = cookie {
        b = b.header("cookie", c);
    }
    app.clone()
        .oneshot(b.body(Body::from(body)).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn health_is_public() {
    let (app, _c, _d) = build_app(0).await;
    let resp = send(&app, "GET", "/health", None, None).await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn full_auth_lifecycle() {
    let (app, _c, _d) = build_app(1).await;

    let s = json_of(send(&app, "GET", "/api/auth/status", None, None).await).await;
    assert_eq!(s["setup_required"], true);
    assert_eq!(s["authenticated"], false);

    let resp = send(&app, "POST", "/api/auth/setup", None, Some(CREDS)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = cookie_of(&resp);

    let me = json_of(send(&app, "GET", "/api/me", Some(&cookie), None).await).await;
    assert_eq!(me["username"], "admin");
    assert_eq!(me["role"], "admin");

    let s = json_of(send(&app, "GET", "/api/auth/status", Some(&cookie), None).await).await;
    assert_eq!(s["authenticated"], true);
    assert_eq!(s["user"]["username"], "admin");
    assert_eq!(s["user"]["role"], "admin");
    assert!(
        s["user"]["created_at"].is_i64(),
        "status.user carries /me fields"
    );

    let resp = send(&app, "POST", "/api/auth/setup", None, Some(CREDS)).await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    let resp = send(&app, "POST", "/api/auth/logout", Some(&cookie), None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = send(&app, "GET", "/api/me", Some(&cookie), None).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let resp = send(&app, "POST", "/api/auth/login", None, Some(CREDS)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie2 = cookie_of(&resp);
    let resp = send(&app, "GET", "/api/me", Some(&cookie2), None).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let bad = r#"{"username":"admin","password":"wrongpass"}"#;
    let resp = send(&app, "POST", "/api/auth/login", None, Some(bad)).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn listing_sorts_by_field_and_paginates_within_a_sort() {
    let (app, cookie, _c, _d) = app_with_auth(5).await;
    let titles = |json: &serde_json::Value| -> Vec<String> {
        json["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["name"].as_str().unwrap().to_string())
            .collect()
    };

    let asc = json_of(send(&app, "GET", "/api/items?sort=title", Some(&cookie), None).await).await;
    let want = titles(&asc);
    let mut sorted = want.clone();
    sorted.sort();
    assert_eq!(want, sorted, "sort=title orders A→Z");

    let desc = json_of(
        send(
            &app,
            "GET",
            "/api/items?sort=title&order=desc",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    let mut rev = want.clone();
    rev.reverse();
    assert_eq!(titles(&desc), rev, "order=desc reverses the title sort");

    let def = json_of(send(&app, "GET", "/api/items", Some(&cookie), None).await).await;
    let oldest = json_of(
        send(
            &app,
            "GET",
            "/api/items?sort=added_at&order=asc",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    let mut def_rev = titles(&def);
    def_rev.reverse();
    assert_eq!(
        titles(&oldest),
        def_rev,
        "oldest-first reverses newest-first"
    );

    let p1 = json_of(
        send(
            &app,
            "GET",
            "/api/items?sort=title&limit=2",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    let cur = p1["next_cursor"].as_str().unwrap().to_string();
    let p2 = json_of(
        send(
            &app,
            "GET",
            &format!("/api/items?sort=title&limit=2&cursor={cur}"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    let mut walked = titles(&p1);
    walked.extend(titles(&p2));
    assert_eq!(walked, vec!["book-0", "book-1", "book-2", "book-3"]);

    let mismatch = send(
        &app,
        "GET",
        &format!("/api/items?cursor={cur}"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(mismatch.status(), StatusCode::BAD_REQUEST);

    assert_eq!(
        send(&app, "GET", "/api/items?sort=bogus", Some(&cookie), None)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send(
            &app,
            "GET",
            "/api/items?sort=title&order=sideways",
            Some(&cookie),
            None
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn listing_sorts_by_artist_tag() {
    let (app, cookie, _c, _d) = app_with_auth(3).await;
    let names = |json: &serde_json::Value| -> Vec<String> {
        json["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["name"].as_str().unwrap().to_string())
            .collect()
    };
    let list = json_of(send(&app, "GET", "/api/items", Some(&cookie), None).await).await;
    let id_of = |name: &str| -> i64 {
        list["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["name"] == name)
            .unwrap()["id"]
            .as_i64()
            .unwrap()
    };

    for (name, artist) in [("book-0", "zebra artist"), ("book-1", "apple artist")] {
        let body = format!(r#"{{"namespace":"creator","value":"{artist}"}}"#);
        let r = send(
            &app,
            "POST",
            &format!("/api/items/{}/tags", id_of(name)),
            Some(&cookie),
            Some(&body),
        )
        .await;
        assert!(r.status().is_success(), "add artist tag");
    }

    let asc =
        json_of(send(&app, "GET", "/api/items?sort=creator", Some(&cookie), None).await).await;
    assert_eq!(
        names(&asc),
        vec![
            "book-1".to_string(),
            "book-0".to_string(),
            "book-2".to_string()
        ],
        "sort=creator orders by artist, no-artist last"
    );

    let desc = json_of(
        send(
            &app,
            "GET",
            "/api/items?sort=creator&order=desc",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(
        names(&desc),
        vec![
            "book-2".to_string(),
            "book-0".to_string(),
            "book-1".to_string()
        ],
        "order=desc reverses the artist sort"
    );
}

#[tokio::test]
async fn api_key_bearer_auth() {
    let (app, cookie, _c, _d) = app_with_auth(2).await;

    let resp = send(
        &app,
        "POST",
        "/api/auth/keys",
        Some(&cookie),
        Some(r#"{"label":"my-script"}"#),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let j = json_of(resp).await;
    let key = j["key"].as_str().unwrap().to_string();
    assert!(key.starts_with("arca_"), "keys are prefixed");
    let key_id = j["id"].as_i64().unwrap();

    assert_eq!(
        send_bearer(&app, "GET", "/api/items", &key, None)
            .await
            .status(),
        StatusCode::OK
    );
    let me = json_of(send_bearer(&app, "GET", "/api/me", &key, None).await).await;
    assert_eq!(me["username"], "admin");

    assert_eq!(
        send_bearer(&app, "GET", "/api/items", "arca_bogus", None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    assert_eq!(
        send_bearer(
            &app,
            "POST",
            "/api/auth/keys",
            &key,
            Some(r#"{"label":"sneaky"}"#)
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED,
        "clients must not create keys"
    );
    assert_eq!(
        send_bearer(&app, "GET", "/api/auth/keys", &key, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED,
        "clients must not list keys"
    );
    assert_eq!(
        send_bearer(&app, "DELETE", "/api/auth/keys/1", &key, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED,
        "clients must not revoke keys"
    );

    let keys = json_of(send(&app, "GET", "/api/auth/keys", Some(&cookie), None).await).await;
    let arr = keys.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["label"], "my-script");
    assert!(arr[0].get("key").is_none(), "secret is never listed");

    let resp = send(
        &app,
        "DELETE",
        &format!("/api/auth/keys/{key_id}"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        send_bearer(&app, "GET", "/api/items", &key, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn protected_routes_require_auth() {
    let (app, _cookie, _c, _d) = app_with_auth(2).await;
    let resp = send(&app, "GET", "/api/items", None, None).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn lists_indexed_items() {
    let (app, cookie, _c, _d) = app_with_auth(3).await;
    let resp = send(&app, "GET", "/api/items", Some(&cookie), None).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let json = json_of(resp).await;
    let arr = json["items"].as_array().unwrap();
    assert_eq!(arr.len(), 3);
    assert!(arr[0].get("id").is_some());
    assert!(arr[0].get("name").is_some());
    assert_eq!(arr[0]["kind"], "uncategorized");
    assert!(json["next_cursor"].is_null());

    let id = arr[0]["id"].as_i64().unwrap();
    let detail = json_of(
        send(
            &app,
            "GET",
            &format!("/api/items/{id}"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(detail["kind"], "uncategorized");
    assert_eq!(detail["modality"], "paginated");
}

#[tokio::test]
async fn paginates_with_keyset_cursor() {
    let (app, cookie, _c, _d) = app_with_auth(5).await;

    let mut seen = std::collections::HashSet::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;

    loop {
        let uri = match &cursor {
            Some(c) => format!("/api/items?limit=2&cursor={c}"),
            None => "/api/items?limit=2".to_string(),
        };
        let resp = send(&app, "GET", &uri, Some(&cookie), None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let json = json_of(resp).await;

        let arr = json["items"].as_array().unwrap();
        assert!(arr.len() <= 2);
        for a in arr {
            assert!(
                seen.insert(a["id"].as_i64().unwrap()),
                "duplicate across pages"
            );
        }
        pages += 1;
        assert!(pages <= 5, "pagination did not terminate");

        match json["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_string()),
            None => break,
        }
    }

    assert_eq!(seen.len(), 5);
    assert_eq!(pages, 3);
}

fn item_ids(j: &serde_json::Value) -> Vec<i64> {
    j["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_i64().unwrap())
        .collect()
}

#[tokio::test]
async fn nav_params_are_mutually_exclusive() {
    let (app, cookie, _c, _d) = app_with_auth(3).await;
    for q in [
        "cursor=abc&page=2",
        "last=true&before=abc",
        "page=1&last=true",
    ] {
        let resp = send(&app, "GET", &format!("/api/items?{q}"), Some(&cookie), None).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "ambiguous nav: {q}");
    }
}

#[tokio::test]
async fn page_jump_reports_total_and_clamps() {
    let (app, cookie, _c, _d) = app_with_auth(5).await;

    let j = json_of(
        send(
            &app,
            "GET",
            "/api/items?limit=2&page=1",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(j["total"], 5);
    assert_eq!(j["page"], 1);
    assert_eq!(j["page_count"], 3);
    assert_eq!(j["items"].as_array().unwrap().len(), 2);
    assert!(j["prev_cursor"].is_null(), "page 1 has no prev");
    assert!(
        j["next_cursor"].is_string(),
        "page 1 offers next (keyset spine works)"
    );

    let j = json_of(
        send(
            &app,
            "GET",
            "/api/items?limit=2&page=3",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(j["page"], 3);
    assert_eq!(j["items"].as_array().unwrap().len(), 1);

    let j = json_of(
        send(
            &app,
            "GET",
            "/api/items?limit=2&page=99",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(j["page"], 3);
    assert_eq!(j["page_count"], 3);
    assert_eq!(j["items"].as_array().unwrap().len(), 1);

    let resp = send(
        &app,
        "GET",
        "/api/items?limit=2&page=0",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let j = json_of(send(&app, "GET", "/api/items?limit=2", Some(&cookie), None).await).await;
    assert!(j.get("total").is_none(), "keyset response omits total");
    assert!(j.get("page").is_none(), "keyset response omits page");
}

#[tokio::test]
async fn prev_walks_back_to_the_first_page() {
    let (app, cookie, _c, _d) = app_with_auth(5).await;

    let r1 = json_of(send(&app, "GET", "/api/items?limit=2", Some(&cookie), None).await).await;
    assert!(r1["prev_cursor"].is_null(), "first page has no prev");
    let c1 = r1["next_cursor"].as_str().unwrap().to_string();

    let r2 = json_of(
        send(
            &app,
            "GET",
            &format!("/api/items?limit=2&cursor={c1}"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    let p2 = r2["prev_cursor"].as_str().unwrap().to_string();
    assert_ne!(
        item_ids(&r1),
        item_ids(&r2),
        "second page differs from first"
    );

    let back = json_of(
        send(
            &app,
            "GET",
            &format!("/api/items?limit=2&before={p2}"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(
        item_ids(&back),
        item_ids(&r1),
        "Prev returns the first page intact"
    );
    assert!(back["prev_cursor"].is_null(), "back at the start");
    assert!(back["next_cursor"].is_string(), "and can go forward again");
}

#[tokio::test]
async fn scrape_endpoint_enqueues_a_job() {
    let (app, cookie, _c, _d) = app_with_auth(1).await;
    let list = json_of(send(&app, "GET", "/api/items?limit=1", Some(&cookie), None).await).await;
    let id = list["items"][0]["id"].as_i64().unwrap().to_string();
    let kind = list["items"][0]["kind"].as_str().unwrap().to_string();

    let off = send(
        &app,
        "POST",
        &format!("/api/items/{id}/scrape?plugin=example"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(
        off.status(),
        StatusCode::FORBIDDEN,
        "plugins are off by default per kind"
    );

    let put = send(
        &app,
        "PUT",
        &format!("/api/kinds/{kind}/plugins"),
        Some(&cookie),
        Some(r#"{"plugin_ids":["example"]}"#),
    )
    .await;
    assert_eq!(put.status(), StatusCode::OK);

    let resp = send(
        &app,
        "POST",
        &format!("/api/items/{id}/scrape?plugin=example"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let j = json_of(resp).await;
    assert_eq!(j["queued"], true);
    assert_eq!(j["plugin"], "example");
    let job_id = j["job_id"].as_i64().unwrap();
    assert!(job_id > 0);
    assert!(j.get("state").is_none());

    let st = json_of(
        send(
            &app,
            "GET",
            &format!("/api/jobs/{job_id}"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(st["kind"], "scrape");
    assert_eq!(st["state"], "pending");
    assert_eq!(
        send(&app, "GET", "/api/jobs/99999999", Some(&cookie), None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &app,
            "GET",
            "/api/jobs/99999999?wait=true",
            Some(&cookie),
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    assert_eq!(
        send(
            &app,
            "POST",
            &format!("/api/items/{id}/scrape"),
            Some(&cookie),
            None
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let bad = send(
        &app,
        "POST",
        &format!("/api/items/{id}/scrape?plugin=nope"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    let missing = send(
        &app,
        "POST",
        "/api/items/0000/scrape?plugin=example",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn job_status_longpoll_returns_a_terminal_job() {
    let (state, _c, _d) = build_state(1).await;
    let app = routes::router(state.clone());
    let cookie = cookie_of(&send(&app, "POST", "/api/auth/setup", None, Some(CREDS)).await);

    let job_id = arcagrad::server::jobs::enqueue(&state.write, "scan", None)
        .await
        .unwrap();
    drain_jobs(&state).await;

    let resp = send(
        &app,
        "GET",
        &format!("/api/jobs/{job_id}?wait=true"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let j = json_of(resp).await;
    assert_eq!(
        j["state"], "done",
        "long-poll returns the terminal state: {j}"
    );
}

#[tokio::test]
async fn credentials_admin_crud_hides_values() {
    let (app, cookie, _c, _d) = app_with_auth(0).await;

    let empty = json_of(send(&app, "GET", "/api/credentials", Some(&cookie), None).await).await;
    assert_eq!(empty.as_array().unwrap().len(), 0);

    let bad = send(
        &app,
        "PUT",
        "/api/credentials/openlibrary",
        Some(&cookie),
        Some(r#"{"data":{}}"#),
    )
    .await;
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);

    let set = send(
        &app,
        "PUT",
        "/api/credentials/openlibrary",
        Some(&cookie),
        Some(r#"{"data":{"api_key":"super-secret"}}"#),
    )
    .await;
    assert_eq!(set.status(), StatusCode::OK);

    let list = json_of(send(&app, "GET", "/api/credentials", Some(&cookie), None).await).await;
    assert_eq!(list[0]["source"], "openlibrary");
    assert_eq!(list[0]["fields"][0], "api_key");
    assert!(
        !list.to_string().contains("super-secret"),
        "secret value must never be returned by the API"
    );

    assert_eq!(
        send(
            &app,
            "DELETE",
            "/api/credentials/openlibrary",
            Some(&cookie),
            None
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        send(
            &app,
            "DELETE",
            "/api/credentials/openlibrary",
            Some(&cookie),
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn plugin_install_lifecycle() {
    use arcagrad::plugins::scraper::MetadataScraper;
    let content = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let db = arcagrad::server::db::connect(data.path()).await.unwrap();
    let config = Arc::new(arcagrad::server::config::Config {
        content_dir: content.path().to_path_buf(),
        data_dir: data.path().to_path_buf(),
        bind: "0.0.0.0:0".into(),
        cookie_secure: false,
        read_concurrency: 8,
        allow_private_repos: false,
        watch: false,
    });
    let wasm: &'static [u8] = Box::leak(
        std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/plugins/openlibrary/openlibrary.wasm"
        ))
        .unwrap()
        .into_boxed_slice(),
    );
    let fetcher: Arc<dyn arcagrad::plugins::scraper::Fetcher> =
        Arc::new(arcagrad::plugins::scraper::HttpFetcher::new());
    let inspected = arcagrad::plugins::wasm_host::load_artifact_bytes(
        wasm.to_vec(),
        "bundled",
        fetcher.clone(),
        Arc::new(arcagrad::plugins::scraper::DbCredentials {
            read: db.read.clone(),
            handle: tokio::runtime::Handle::current(),
        }),
        tokio::runtime::Handle::current(),
    )
    .unwrap();
    let catalog = vec![arcagrad::plugins::wasm_host::BundledPlugin {
        manifest: inspected.manifest(),
        icon: inspected.icon_bytes().map(<[u8]>::to_vec),
        artifact_hash: arcagrad::plugins::wasm_host::artifact_hash(wasm),
        bytes: wasm,
    }];
    let app = routes::router(AppState {
        config,
        read: db.read.clone(),
        write: db.write.clone(),
        page_lists: Arc::new(arcagrad::media::pages::PageListCache::new(64)),
        page_thumb_locks: Arc::new(arcagrad::media::library::KeyedLocks::default()),
        blocking_limiter: Arc::new(tokio::sync::Semaphore::new(8)),
        scrapers: Arc::new(arcagrad::plugins::scraper::ScraperRegistry::new()),
        plugin_catalog: Arc::new(catalog),
        marketplace: std::sync::Arc::new(arcagrad::plugins::marketplace::RepoCache::new()),
        rate_limiter: Arc::new(arcagrad::plugins::scraper::RateLimiter::default()),
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
            arcagrad::intelligence::search::SearchIndex::open_or_create(data.path().join("search"))
                .unwrap(),
        ),
    });
    let resp = send(&app, "POST", "/api/auth/setup", None, Some(CREDS)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = cookie_of(&resp);

    let cat = json_of(send(&app, "GET", "/api/plugin-catalog", Some(&cookie), None).await).await;
    assert_eq!(cat[0]["plugin"]["id"], "openlibrary");
    assert_eq!(cat[0]["installed"], false);
    let running = json_of(send(&app, "GET", "/api/plugins", Some(&cookie), None).await).await;
    assert!(running.as_array().unwrap().is_empty(), "shelf must not run");
    let icon = send(
        &app,
        "GET",
        "/api/plugins/openlibrary/icon",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(icon.status(), StatusCode::OK);

    let resp = send(
        &app,
        "POST",
        "/api/plugin-installs",
        Some(&cookie),
        Some(r#"{"id":"openlibrary"}"#),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let running = json_of(send(&app, "GET", "/api/plugins", Some(&cookie), None).await).await;
    assert_eq!(running[0]["id"], "openlibrary");
    assert_eq!(running[0]["origin"], "bundled");
    let managed = data.path().join("plugins/managed/openlibrary.wasm");
    assert!(managed.exists(), "artifact copied to the managed dir");
    let cat = json_of(send(&app, "GET", "/api/plugin-catalog", Some(&cookie), None).await).await;
    assert_eq!(cat[0]["installed"], true);

    let resp = send(
        &app,
        "POST",
        "/api/plugin-installs",
        Some(&cookie),
        Some(r#"{"id":"nope"}"#),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let resp = send(
        &app,
        "DELETE",
        "/api/plugin-installs/nope",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let resp = send(&app, "GET", "/api/plugin-catalog", None, None).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let resp = send(
        &app,
        "DELETE",
        "/api/plugin-installs/openlibrary",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let running = json_of(send(&app, "GET", "/api/plugins", Some(&cookie), None).await).await;
    assert!(running.as_array().unwrap().is_empty());
    assert!(!managed.exists(), "managed artifact removed");
    let cat = json_of(send(&app, "GET", "/api/plugin-catalog", Some(&cookie), None).await).await;
    assert_eq!(cat[0]["installed"], false);
}

#[tokio::test]
async fn plugins_discovery_lists_and_filters() {
    let (app, cookie, _c, _d) = app_with_auth(1).await;

    let all = json_of(send(&app, "GET", "/api/plugins", Some(&cookie), None).await).await;
    let ids: Vec<&str> = all
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"example"));

    let scrape = json_of(
        send(
            &app,
            "GET",
            "/api/plugins?capability=scrape",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(scrape.as_array().unwrap().len(), 1);
    let dl = json_of(
        send(
            &app,
            "GET",
            "/api/plugins?capability=download",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(dl.as_array().unwrap().len(), 0);

    let with_kind =
        json_of(send(&app, "GET", "/api/plugins?kind=manga", Some(&cookie), None).await).await;
    assert_eq!(
        with_kind.as_array().unwrap().len(),
        1,
        "kind does not filter plugins"
    );
}

#[tokio::test]
async fn plugin_browse_feed_and_gate() {
    let (app, cookie, _c, _d) = app_with_auth(1).await;

    let all = json_of(send(&app, "GET", "/api/plugins", Some(&cookie), None).await).await;
    let example = all
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "example")
        .unwrap();
    let feeds = example["feeds"].as_array().unwrap();
    assert_eq!(feeds.len(), 1);
    assert_eq!(feeds[0]["id"], "popular");
    assert_eq!(feeds[0]["ranges"].as_array().unwrap().len(), 2);

    let page = json_of(
        send(
            &app,
            "GET",
            "/api/plugins/example/browse?feed=popular&range=week&query=fantasy&page=2",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(page["num_pages"], 5);
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["title"], "Example popular/week/fantasy/2");
    assert_eq!(items[0]["reference"], "ex-1");
    assert_eq!(items[0]["cover_url"], "https://example.test/1.jpg");
    assert_eq!(items[0]["favorites"], 99);

    let unknown = send(
        &app,
        "GET",
        "/api/plugins/nope/browse?feed=popular",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::BAD_REQUEST);

    let unauth = send(
        &app,
        "GET",
        "/api/plugins/example/browse?feed=popular",
        None,
        None,
    )
    .await;
    assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn plugin_image_proxy_gates() {
    let (app, cookie, _c, _d) = app_with_auth(1).await;

    let blocked = send(
        &app,
        "GET",
        "/api/plugins/example/image?url=https://evil.com/x.jpg",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(blocked.status(), StatusCode::FORBIDDEN);

    let bad = send(
        &app,
        "GET",
        "/api/plugins/example/image?url=notaurl",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);

    let unknown = send(
        &app,
        "GET",
        "/api/plugins/nope/image?url=https://example.test/x.jpg",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::BAD_REQUEST);
    let unauth = send(
        &app,
        "GET",
        "/api/plugins/example/image?url=https://example.test/x.jpg",
        None,
        None,
    )
    .await;
    assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn plugin_item_detail_gates() {
    let (app, cookie, _c, _d) = app_with_auth(1).await;

    let empty = send(
        &app,
        "GET",
        "/api/plugins/example/item?ref=",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(empty.status(), StatusCode::BAD_REQUEST);

    let unknown = send(
        &app,
        "GET",
        "/api/plugins/nope/item?ref=1",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::BAD_REQUEST);

    let unauth = send(&app, "GET", "/api/plugins/example/item?ref=1", None, None).await;
    assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn plugin_pages_list_and_gates() {
    let (app, cookie, _c, _d) = app_with_auth(1).await;

    let out = json_of(
        send(
            &app,
            "GET",
            "/api/plugins/example/pages?ref=g42",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    let pages = out["pages"].as_array().unwrap();
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0]["number"], 1);
    assert_eq!(pages[0]["image_url"], "https://example.test/g42/1.jpg");
    assert_eq!(pages[0]["thumb_url"], "https://example.test/g42/1t.jpg");
    assert_eq!(pages[0]["width"], 800);

    assert_eq!(
        send(
            &app,
            "GET",
            "/api/plugins/example/pages?ref=",
            Some(&cookie),
            None
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send(
            &app,
            "GET",
            "/api/plugins/nope/pages?ref=1",
            Some(&cookie),
            None
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send(&app, "GET", "/api/plugins/example/pages?ref=1", None, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn plugin_browse_is_cached_to_disk() {
    let (app, cookie, _c, data) = app_with_auth(1).await;
    let path = "/api/plugins/example/browse?feed=popular&range=today&page=1";

    let r1 = send(&app, "GET", path, Some(&cookie), None).await;
    assert_eq!(r1.status(), StatusCode::OK);
    assert_eq!(
        r1.headers().get("cache-control").unwrap(),
        "private, max-age=300"
    );
    let b1 = r1.into_body().collect().await.unwrap().to_bytes();

    fn count_json(dir: &std::path::Path) -> usize {
        let mut n = 0;
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    n += count_json(&p);
                } else if p.extension().is_some_and(|x| x == "json") {
                    n += 1;
                }
            }
        }
        n
    }
    let cache_dir = data.path().join("cache").join("browse");
    assert!(
        count_json(&cache_dir) >= 1,
        "browse response cached to disk"
    );

    let r2 = send(&app, "GET", path, Some(&cookie), None).await;
    let b2 = r2.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(b1, b2);
}

#[tokio::test]
async fn kind_plugin_enablement_toggles_per_kind() {
    let (app, cookie, _c, _d) = app_with_auth(1).await;
    let find = |v: &serde_json::Value, id: &str| -> serde_json::Value {
        v.as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .cloned()
            .unwrap_or(serde_json::Value::Null)
    };

    let before = json_of(
        send(
            &app,
            "GET",
            "/api/kinds/uncategorized/plugins",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    let ex = find(&before, "example");
    assert_eq!(ex["enabled"], false, "off by default");
    assert!(
        ex["description"].as_str().is_some(),
        "description surfaced for the UI"
    );

    let saved = json_of(
        send(
            &app,
            "PUT",
            "/api/kinds/uncategorized/plugins",
            Some(&cookie),
            Some(r#"{"plugin_ids":["example"]}"#),
        )
        .await,
    )
    .await;
    assert_eq!(find(&saved, "example")["enabled"], true);

    let after = json_of(
        send(
            &app,
            "GET",
            "/api/kinds/uncategorized/plugins",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(find(&after, "example")["enabled"], true);
    let other =
        json_of(send(&app, "GET", "/api/kinds/manga/plugins", Some(&cookie), None).await).await;
    assert_eq!(
        find(&other, "example")["enabled"],
        false,
        "enabling for one kind must not leak into another"
    );

    let bad = send(
        &app,
        "PUT",
        "/api/kinds/uncategorized/plugins",
        Some(&cookie),
        Some(r#"{"plugin_ids":["ghost"]}"#),
    )
    .await;
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);

    let cleared = json_of(
        send(
            &app,
            "PUT",
            "/api/kinds/uncategorized/plugins",
            Some(&cookie),
            Some(r#"{"plugin_ids":[]}"#),
        )
        .await,
    )
    .await;
    assert_eq!(find(&cleared, "example")["enabled"], false);

    let auto_on = json_of(
        send(
            &app,
            "PUT",
            "/api/kinds/uncategorized/plugins",
            Some(&cookie),
            Some(r#"{"plugin_ids":["example"],"auto":["example"]}"#),
        )
        .await,
    )
    .await;
    let ex = find(&auto_on, "example");
    assert_eq!(ex["enabled"], true);
    assert_eq!(ex["auto"], true, "auto flag surfaced");

    let auto_off = json_of(
        send(
            &app,
            "PUT",
            "/api/kinds/uncategorized/plugins",
            Some(&cookie),
            Some(r#"{"plugin_ids":["example"]}"#),
        )
        .await,
    )
    .await;
    assert_eq!(find(&auto_off, "example")["enabled"], true);
    assert_eq!(
        find(&auto_off, "example")["auto"],
        false,
        "auto defaults off when omitted"
    );

    let bad_auto = send(
        &app,
        "PUT",
        "/api/kinds/uncategorized/plugins",
        Some(&cookie),
        Some(r#"{"plugin_ids":[],"auto":["example"]}"#),
    )
    .await;
    assert_eq!(bad_auto.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn offset_page_matches_keyset_walk() {
    let (app, cookie, _c, _d) = app_with_auth(5).await;

    let jump = json_of(
        send(
            &app,
            "GET",
            "/api/items?limit=2&page=2",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;

    let p1 = json_of(send(&app, "GET", "/api/items?limit=2", Some(&cookie), None).await).await;
    let c1 = p1["next_cursor"].as_str().unwrap();
    let walk = json_of(
        send(
            &app,
            "GET",
            &format!("/api/items?limit=2&cursor={c1}"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;

    assert_eq!(
        item_ids(&jump),
        item_ids(&walk),
        "offset page == keyset walk"
    );
}

#[tokio::test]
async fn last_jumps_to_the_oldest_page() {
    let (app, cookie, _c, _d) = app_with_auth(5).await;

    let mut all = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let uri = match &cursor {
            Some(c) => format!("/api/items?limit=2&cursor={c}"),
            None => "/api/items?limit=2".to_string(),
        };
        let j = json_of(send(&app, "GET", &uri, Some(&cookie), None).await).await;
        all.extend(item_ids(&j));
        match j["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_string()),
            None => break,
        }
    }
    assert_eq!(all.len(), 5);

    let last = json_of(
        send(
            &app,
            "GET",
            "/api/items?limit=2&last=true",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert!(last["next_cursor"].is_null(), "oldest page has no next");
    assert!(last["prev_cursor"].is_string(), "oldest page has a prev");
    assert_eq!(item_ids(&last), all[all.len() - 2..].to_vec());
}

#[tokio::test]
async fn search_filters_by_title_and_reindexed_tags() {
    let (app, cookie, _c, _d) = app_with_auth(3).await;

    let j = json_of(send(&app, "GET", "/api/items?q=book", Some(&cookie), None).await).await;
    assert_eq!(j["items"].as_array().unwrap().len(), 3);

    let j = json_of(send(&app, "GET", "/api/items?q=zzznomatch", Some(&cookie), None).await).await;
    assert!(j["items"].as_array().unwrap().is_empty());

    let all = json_of(send(&app, "GET", "/api/items", Some(&cookie), None).await).await;
    let target = all["items"][0]["id"].as_i64().unwrap().to_string();
    let resp = send(
        &app,
        "POST",
        &format!("/api/items/{target}/tags"),
        Some(&cookie),
        Some(r#"{"namespace":"tag","value":"mystery"}"#),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let j = json_of(send(&app, "GET", "/api/items?q=mystery", Some(&cookie), None).await).await;
    assert_eq!(
        item_ids(&j),
        vec![target.parse::<i64>().unwrap()],
        "search by tag value"
    );

    let j = json_of(
        send(
            &app,
            "GET",
            "/api/items?q=book&tags=tag:mystery",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(item_ids(&j), vec![target.parse::<i64>().unwrap()]);

    let j = json_of(
        send(
            &app,
            "GET",
            "/api/items?q=mystery&page=1",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(j["total"], 1);
    assert_eq!(j["page_count"], 1);
}

#[tokio::test]
async fn search_query_is_sanitized_never_500s() {
    let (app, cookie, _c, _d) = app_with_auth(2).await;
    for q in [
        "%22%2A%5E%28",
        "a%22b",
        "foo%3A%3Abar",
        "%2A%2A%2A",
        "-%20OR",
    ] {
        let resp = send(
            &app,
            "GET",
            &format!("/api/items?q={q}"),
            Some(&cookie),
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK, "q={q} must not error");
    }
}

#[tokio::test]
async fn bad_cursor_is_400() {
    let (app, cookie, _c, _d) = app_with_auth(1).await;
    let resp = send(
        &app,
        "GET",
        "/api/items?cursor=%21%21%21",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn tag_routes_crud_and_filter() {
    let (app, cookie, _c, _d) = app_with_auth(3).await;

    let listing = json_of(send(&app, "GET", "/api/items", Some(&cookie), None).await).await;
    let arr = listing["items"].as_array().unwrap();
    let id0 = arr[0]["id"].as_i64().unwrap().to_string();
    let id1 = arr[1]["id"].as_i64().unwrap().to_string();

    for body in [
        r#"{"namespace":"creator","value":"Rumiko Takahashi"}"#,
        r#"{"namespace":"tag","value":"mystery","qualifier":"female"}"#,
    ] {
        let resp = send(
            &app,
            "POST",
            &format!("/api/items/{id0}/tags"),
            Some(&cookie),
            Some(body),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    let detail = json_of(
        send(
            &app,
            "GET",
            &format!("/api/items/{id0}"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    let tags = detail["tags"].as_array().unwrap();
    assert_eq!(tags.len(), 2);
    assert!(tags
        .iter()
        .any(|t| t["namespace"] == "creator" && t["value"] == "rumiko takahashi"));
    assert!(tags
        .iter()
        .any(|t| t["value"] == "mystery" && t["qualifier"] == "female"));

    let counts = json_of(send(&app, "GET", "/api/tags", Some(&cookie), None).await).await;
    assert!(counts
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["value"] == "mystery" && c["count"] == 1));

    let f = json_of(
        send(
            &app,
            "GET",
            "/api/items?tags=tag:mystery",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    let ids: Vec<String> = f["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_i64().unwrap().to_string())
        .collect();
    assert_eq!(ids, vec![id0.clone()]);

    let f = json_of(
        send(
            &app,
            "GET",
            "/api/items?tags=tag:mystery,tag:nope&match=all",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert!(f["items"].as_array().unwrap().is_empty());

    send(
        &app,
        "POST",
        &format!("/api/items/{id1}/tags"),
        Some(&cookie),
        Some(r#"{"namespace":"tag","value":"mystery"}"#),
    )
    .await;
    let f = json_of(
        send(
            &app,
            "GET",
            "/api/items?tags=tag:mystery",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(f["items"].as_array().unwrap().len(), 2);

    let resp = send(
        &app,
        "DELETE",
        &format!("/api/items/{id0}/tags"),
        Some(&cookie),
        Some(r#"{"namespace":"tag","value":"mystery","qualifier":"female"}"#),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let f = json_of(
        send(
            &app,
            "GET",
            "/api/items?tags=tag:mystery",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    let ids: Vec<String> = f["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_i64().unwrap().to_string())
        .collect();
    assert_eq!(ids, vec![id1.clone()]);

    let good_ns = send(
        &app,
        "POST",
        &format!("/api/items/{id0}/tags"),
        Some(&cookie),
        Some(r#"{"namespace":"demographic","value":"seinen"}"#),
    )
    .await;
    assert_eq!(good_ns.status(), StatusCode::OK);

    let bad_ns = send(
        &app,
        "POST",
        &format!("/api/items/{id0}/tags"),
        Some(&cookie),
        Some(r#"{"namespace":"genre","value":"drama"}"#),
    )
    .await;
    assert_eq!(bad_ns.status(), StatusCode::BAD_REQUEST);

    let unauth = send(
        &app,
        "POST",
        &format!("/api/items/{id0}/tags"),
        None,
        Some(r#"{"namespace":"tag","value":"x"}"#),
    )
    .await;
    assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);

    let missing = send(
        &app,
        "POST",
        "/api/items/999999/tags",
        Some(&cookie),
        Some(r#"{"namespace":"tag","value":"x"}"#),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn password_change_and_logout_all() {
    let (app, cookie, _c, _d) = app_with_auth(1).await;

    let m = json_of(send(&app, "GET", "/api/me", Some(&cookie), None).await).await;
    assert!(m["created_at"].as_i64().is_some());
    assert!(m["avatar_version"].is_null());

    let other = cookie_of(&send(&app, "POST", "/api/auth/login", None, Some(CREDS)).await);
    assert_eq!(
        send(&app, "GET", "/api/me", Some(&other), None)
            .await
            .status(),
        StatusCode::OK
    );

    let r = send(
        &app,
        "PUT",
        "/api/auth/password",
        Some(&cookie),
        Some(r#"{"current":"wrong","new":"longenough1"}"#),
    )
    .await;
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    let r = send(
        &app,
        "PUT",
        "/api/auth/password",
        Some(&cookie),
        Some(r#"{"current":"password123","new":"short"}"#),
    )
    .await;
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);

    let r = send(
        &app,
        "PUT",
        "/api/auth/password",
        Some(&cookie),
        Some(r#"{"current":"password123","new":"n3w-password"}"#),
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(
        send(&app, "GET", "/api/me", Some(&cookie), None)
            .await
            .status(),
        StatusCode::OK,
        "calling session stays signed in"
    );
    assert_eq!(
        send(&app, "GET", "/api/me", Some(&other), None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED,
        "other session was ended"
    );
    assert_eq!(
        send(&app, "POST", "/api/auth/login", None, Some(CREDS))
            .await
            .status(),
        StatusCode::UNAUTHORIZED,
        "old password no longer logs in"
    );
    let relogin = send(
        &app,
        "POST",
        "/api/auth/login",
        None,
        Some(r#"{"username":"admin","password":"n3w-password"}"#),
    )
    .await;
    assert_eq!(relogin.status(), StatusCode::OK);

    let key = json_of(
        send(
            &app,
            "POST",
            "/api/auth/keys",
            Some(&cookie),
            Some(r#"{"label":"survivor"}"#),
        )
        .await,
    )
    .await["key"]
        .as_str()
        .unwrap()
        .to_string();
    let r = send(&app, "POST", "/api/auth/logout-all", Some(&cookie), None).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(
        send(&app, "GET", "/api/me", Some(&cookie), None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED,
        "caller's session ended too"
    );
    assert_eq!(
        send_bearer(&app, "GET", "/api/me", &key, None)
            .await
            .status(),
        StatusCode::OK,
        "API keys stay valid"
    );

    let r = send_bearer(
        &app,
        "PUT",
        "/api/auth/password",
        &key,
        Some(r#"{"current":"n3w-password","new":"whatever123"}"#),
    )
    .await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn rescan_requires_admin() {
    let (app, admin, _c, _d) = app_with_auth(1).await;
    send(
        &app,
        "POST",
        "/api/users",
        Some(&admin),
        Some(r#"{"username":"reader","password":"password456","role":"user"}"#),
    )
    .await;
    let resp = send(
        &app,
        "POST",
        "/api/auth/login",
        None,
        Some(r#"{"username":"reader","password":"password456"}"#),
    )
    .await;
    let user = cookie_of(&resp);

    assert_eq!(
        send(&app, "POST", "/api/rescan", Some(&user), None)
            .await
            .status(),
        StatusCode::FORBIDDEN,
        "a non-admin must not trigger a full scan"
    );
    assert_eq!(
        send(&app, "POST", "/api/rescan", Some(&admin), None)
            .await
            .status(),
        StatusCode::OK
    );

    assert_eq!(
        send(&app, "GET", "/api/jobs/1", Some(&user), None)
            .await
            .status(),
        StatusCode::FORBIDDEN,
        "a non-admin must not poll job status"
    );
}

#[tokio::test]
async fn signup_toggle_and_register_flow() {
    let (app, admin, _c, _d) = app_with_auth(1).await;

    let s = json_of(send(&app, "GET", "/api/auth/status", None, None).await).await;
    assert_eq!(s["signup_enabled"], false);
    assert_eq!(s["guest_enabled"], false);
    assert_eq!(
        send(
            &app,
            "POST",
            "/api/auth/register",
            None,
            Some(r#"{"username":"newbie","password":"password456"}"#),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );

    assert_eq!(
        send(&app, "GET", "/api/settings/auth", None, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let r = send(
        &app,
        "PUT",
        "/api/settings/auth",
        Some(&admin),
        Some(r#"{"signup_enabled":true,"guest_enabled":false}"#),
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);
    let s = json_of(send(&app, "GET", "/api/auth/status", None, None).await).await;
    assert_eq!(s["signup_enabled"], true);

    let resp = send(
        &app,
        "POST",
        "/api/auth/register",
        None,
        Some(r#"{"username":"newbie","password":"password456"}"#),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = cookie_of(&resp);
    let me = json_of(send(&app, "GET", "/api/me", Some(&cookie), None).await).await;
    assert_eq!(me["username"], "newbie");
    assert_eq!(me["role"], "user");
    assert_eq!(
        send(
            &app,
            "POST",
            "/api/auth/register",
            None,
            Some(r#"{"username":"newbie","password":"password456"}"#),
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );

    send(
        &app,
        "PUT",
        "/api/settings/auth",
        Some(&admin),
        Some(r#"{"signup_enabled":false,"guest_enabled":false}"#),
    )
    .await;
    assert_eq!(
        send(
            &app,
            "POST",
            "/api/auth/register",
            None,
            Some(r#"{"username":"another","password":"password456"}"#),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn guest_browsing_toggle_and_scope() {
    let (app, admin, _c, _d) = app_with_auth(2).await;

    assert_eq!(
        send(&app, "GET", "/api/items", None, None).await.status(),
        StatusCode::UNAUTHORIZED
    );

    let r = send(
        &app,
        "PUT",
        "/api/settings/auth",
        Some(&admin),
        Some(r#"{"signup_enabled":false,"guest_enabled":true}"#),
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);
    let s = json_of(send(&app, "GET", "/api/auth/status", None, None).await).await;
    assert_eq!(s["guest_enabled"], true);

    let list = json_of(send(&app, "GET", "/api/items", None, None).await).await;
    let entries = list["items"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "guest sees the library listing");
    let id = entries[0]["id"].as_i64().unwrap();
    let detail = json_of(send(&app, "GET", &format!("/api/items/{id}"), None, None).await).await;
    assert_eq!(detail["favorited"], false);
    assert!(detail["progress"].is_null());
    for uri in [
        "/api/kinds".to_string(),
        "/api/tags".to_string(),
        "/api/suggest?q=a".to_string(),
        format!("/api/items/{id}/similar"),
    ] {
        assert_eq!(
            send(&app, "GET", &uri, None, None).await.status(),
            StatusCode::OK,
            "guest-readable: {uri}"
        );
    }

    for (method, uri, body) in [
        ("GET", "/api/items/continue".to_string(), None),
        ("GET", "/api/recommendations".to_string(), None),
        ("GET", "/api/tags/favorites".to_string(), None),
        ("GET", "/api/me".to_string(), None),
        ("POST", format!("/api/items/{id}/favorite"), None),
        (
            "PUT",
            format!("/api/items/{id}/rating"),
            Some(r#"{"value":5}"#),
        ),
        (
            "PUT",
            format!("/api/items/{id}/progress"),
            Some(r#"{"page":1}"#),
        ),
        (
            "PUT",
            format!("/api/items/{id}/reading-mode"),
            Some(r#"{"mode":"vertical"}"#),
        ),
        ("GET", "/api/me/tag-blocklist".to_string(), None),
        ("GET", "/api/users".to_string(), None),
        ("POST", "/api/rescan".to_string(), None),
    ] {
        assert_eq!(
            send(&app, method, &uri, None, body).await.status(),
            StatusCode::UNAUTHORIZED,
            "guest must not reach {method} {uri}"
        );
    }

    assert_eq!(
        send_bearer(&app, "GET", "/api/items", "arca_bogus", None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    send(
        &app,
        "PUT",
        "/api/settings/auth",
        Some(&admin),
        Some(r#"{"signup_enabled":false,"guest_enabled":false}"#),
    )
    .await;
    assert_eq!(
        send(&app, "GET", "/api/items", None, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn download_streams_the_original_archive() {
    let (app, cookie, content, _d) = app_with_auth(1).await;

    assert_eq!(
        send(&app, "GET", "/api/items/1/download", None, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let list = json_of(send(&app, "GET", "/api/items", Some(&cookie), None).await).await;
    let id = list["items"][0]["id"].as_i64().unwrap();
    let resp = send(
        &app,
        "GET",
        &format!("/api/items/{id}/download"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let cd = resp
        .headers()
        .get("content-disposition")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(cd.starts_with("attachment"), "attachment disposition: {cd}");
    assert!(cd.contains("book-0.cbz"), "original filename in: {cd}");
    let len: usize = resp
        .headers()
        .get("content-length")
        .unwrap()
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let disk = std::fs::read(content.path().join("book-0.cbz")).unwrap();
    assert_eq!(bytes.len(), len);
    assert_eq!(&bytes[..], &disk[..]);

    assert_eq!(
        send(
            &app,
            "GET",
            "/api/items/999999/download",
            Some(&cookie),
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn kind_visibility_hides_kinds_per_audience() {
    let (app, admin, _c, _d) = app_with_auth(1).await;
    let (ct, body) = multipart_body(&[
        ("kind", None, b"manga".as_slice()),
        ("file", Some("Secret Manga.cbz"), &cbz_bytes("kv-secret")),
    ]);
    let resp = send_raw(&app, "POST", "/api/items", Some(&admin), &ct, body).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let manga_id = json_of(resp).await["id"].as_i64().unwrap();

    send(
        &app,
        "POST",
        "/api/users",
        Some(&admin),
        Some(r#"{"username":"reader","password":"password456","role":"user"}"#),
    )
    .await;
    let resp = send(
        &app,
        "POST",
        "/api/auth/login",
        None,
        Some(r#"{"username":"reader","password":"password456"}"#),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let user = cookie_of(&resp);
    send(
        &app,
        "PUT",
        "/api/settings/auth",
        Some(&admin),
        Some(r#"{"signup_enabled":false,"guest_enabled":true}"#),
    )
    .await;

    let resp = send(
        &app,
        "PUT",
        "/api/settings/kind-access",
        Some(&admin),
        Some(r#"{"user":["manga"],"guest":[]}"#),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        send(
            &app,
            "PUT",
            "/api/settings/kind-access",
            Some(&user),
            Some(r#"{"user":[],"guest":[]}"#),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );

    let kinds = json_of(send(&app, "GET", "/api/kinds", Some(&user), None).await).await;
    assert!(
        !kinds
            .as_array()
            .unwrap()
            .iter()
            .any(|k| k["kind"] == "manga"),
        "hidden kind must not be listed for users"
    );
    let list = json_of(send(&app, "GET", "/api/items", Some(&user), None).await).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert_eq!(list["items"][0]["kind"], "uncategorized");
    let jump = json_of(send(&app, "GET", "/api/items?page=1", Some(&user), None).await).await;
    assert_eq!(jump["total"], 1);
    for uri in [
        format!("/api/items/{manga_id}"),
        format!("/api/items/{manga_id}/thumbnail"),
        format!("/api/items/{manga_id}/pages/0"),
        format!("/api/items/{manga_id}/similar"),
    ] {
        assert_eq!(
            send(&app, "GET", &uri, Some(&user), None).await.status(),
            StatusCode::NOT_FOUND,
            "hidden for user: {uri}"
        );
    }
    assert_eq!(
        send(
            &app,
            "GET",
            &format!("/api/items/{manga_id}/thumbnail?v=abcdef0123456789abcdef0123456789"),
            Some(&user),
            None,
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    let r = send(
        &app,
        "POST",
        &format!("/api/items/{manga_id}/tags"),
        Some(&admin),
        Some(r#"{"namespace":"creator","value":"hiddenartist"}"#),
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);
    let tags = json_of(send(&app, "GET", "/api/tags", Some(&user), None).await).await;
    assert!(
        !tags
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["value"] == "hiddenartist"),
        "hidden-kind-only tag must not be listed for users"
    );
    let blended = json_of(
        send(
            &app,
            "GET",
            "/api/suggest?q=hiddenartist",
            Some(&user),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(blended["results"].as_array().unwrap().len(), 0);
    for uri in ["/api/tags?kind=manga", "/api/tags/favorites?kind=manga"] {
        assert_eq!(
            send(&app, "GET", uri, Some(&user), None).await.status(),
            StatusCode::NOT_FOUND,
            "kind-scoped tag surface: {uri}"
        );
    }
    let tags = json_of(send(&app, "GET", "/api/tags", Some(&admin), None).await).await;
    assert!(tags
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["value"] == "hiddenartist"));

    send(
        &app,
        "PUT",
        "/api/settings/kind-access",
        Some(&admin),
        Some(r#"{"user":["uncategorized","manga"],"guest":[]}"#),
    )
    .await;
    let blended = json_of(send(&app, "GET", "/api/suggest?q=book", Some(&admin), None).await).await;
    assert!(
        blended["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["type"] == "title"),
        "sanity: the seeded title is in the index for the admin"
    );
    let blended = json_of(send(&app, "GET", "/api/suggest?q=book", Some(&user), None).await).await;
    assert!(
        !blended["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["type"] == "title"),
        "hidden-kind titles must not surface in the blended suggest"
    );
    send(
        &app,
        "PUT",
        "/api/settings/kind-access",
        Some(&admin),
        Some(r#"{"user":["manga"],"guest":[]}"#),
    )
    .await;

    let kinds = json_of(send(&app, "GET", "/api/kinds", Some(&admin), None).await).await;
    assert!(kinds
        .as_array()
        .unwrap()
        .iter()
        .any(|k| k["kind"] == "manga"));
    assert_eq!(
        send(
            &app,
            "GET",
            &format!("/api/items/{manga_id}"),
            Some(&admin),
            None
        )
        .await
        .status(),
        StatusCode::OK
    );
    let kinds = json_of(send(&app, "GET", "/api/kinds", None, None).await).await;
    assert!(
        kinds
            .as_array()
            .unwrap()
            .iter()
            .any(|k| k["kind"] == "manga"),
        "guest audience is independent — manga stays visible to guests"
    );

    send(
        &app,
        "PUT",
        "/api/settings/kind-access",
        Some(&admin),
        Some(r#"{"user":[],"guest":["manga"]}"#),
    )
    .await;
    let kinds = json_of(send(&app, "GET", "/api/kinds", None, None).await).await;
    assert!(!kinds
        .as_array()
        .unwrap()
        .iter()
        .any(|k| k["kind"] == "manga"));
    assert_eq!(
        send(&app, "GET", &format!("/api/items/{manga_id}"), None, None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &app,
            "GET",
            &format!("/api/items/{manga_id}"),
            Some(&user),
            None
        )
        .await
        .status(),
        StatusCode::OK,
        "user side was cleared — visible again"
    );
    let acc =
        json_of(send(&app, "GET", "/api/settings/kind-access", Some(&admin), None).await).await;
    assert_eq!(acc["user"].as_array().unwrap().len(), 0);
    assert_eq!(acc["guest"][0], "manga");
}

#[tokio::test]
async fn login_brute_force_throttles_per_username() {
    let (app, _admin, _c, _d) = app_with_auth(1).await;
    let body = r#"{"username":"bruteforce-target","password":"wrong-pass"}"#;
    let mut last = StatusCode::OK;
    for _ in 0..12 {
        last = send(&app, "POST", "/api/auth/login", None, Some(body))
            .await
            .status();
        if last == StatusCode::TOO_MANY_REQUESTS {
            break;
        }
        assert_eq!(last, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(
        last,
        StatusCode::TOO_MANY_REQUESTS,
        "sustained failures must trip the throttle"
    );
    assert_eq!(
        send(&app, "POST", "/api/auth/login", None, Some(CREDS))
            .await
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn user_management_crud_and_guards() {
    let (app, admin, _c, _d) = app_with_auth(1).await;

    let list = json_of(send(&app, "GET", "/api/users", Some(&admin), None).await).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    let admin_id = list[0]["id"].as_i64().unwrap();

    let created = json_of(
        send(
            &app,
            "POST",
            "/api/users",
            Some(&admin),
            Some(r#"{"username":"alice","password":"password456"}"#),
        )
        .await,
    )
    .await;
    assert_eq!(created["role"], "user");
    let alice_id = created["id"].as_i64().unwrap();
    assert_eq!(
        send(
            &app,
            "POST",
            "/api/users",
            Some(&admin),
            Some(r#"{"username":"alice","password":"password456"}"#),
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        send(
            &app,
            "POST",
            "/api/users",
            Some(&admin),
            Some(r#"{"username":"bob","password":"password456","role":"root"}"#),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );

    let alice = cookie_of(
        &send(
            &app,
            "POST",
            "/api/auth/login",
            None,
            Some(r#"{"username":"alice","password":"password456"}"#),
        )
        .await,
    );
    assert_eq!(
        send(&app, "GET", "/api/users", Some(&alice), None)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    assert_eq!(
        send(
            &app,
            "DELETE",
            &format!("/api/users/{admin_id}"),
            Some(&admin),
            None
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST,
        "self-delete refused"
    );
    send(
        &app,
        "POST",
        "/api/users",
        Some(&admin),
        Some(r#"{"username":"root2","password":"password456","role":"admin"}"#),
    )
    .await;
    let list = json_of(send(&app, "GET", "/api/users", Some(&admin), None).await).await;
    let root2_id = list
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["username"] == "root2")
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    assert_eq!(
        send(
            &app,
            "DELETE",
            &format!("/api/users/{root2_id}"),
            Some(&admin),
            None
        )
        .await
        .status(),
        StatusCode::OK
    );

    assert_eq!(
        send(
            &app,
            "PUT",
            &format!("/api/users/{alice_id}/password"),
            Some(&admin),
            Some(r#"{"new":"resetpass99"}"#),
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        send(&app, "GET", "/api/me", Some(&alice), None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED,
        "reset ends the user's sessions"
    );
    assert_eq!(
        send(
            &app,
            "POST",
            "/api/auth/login",
            None,
            Some(r#"{"username":"alice","password":"resetpass99"}"#),
        )
        .await
        .status(),
        StatusCode::OK
    );

    assert_eq!(
        send(
            &app,
            "DELETE",
            &format!("/api/users/{alice_id}"),
            Some(&admin),
            None
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        send(
            &app,
            "POST",
            "/api/auth/login",
            None,
            Some(r#"{"username":"alice","password":"resetpass99"}"#),
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let list = json_of(send(&app, "GET", "/api/users", Some(&admin), None).await).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn api_key_last_used_stamps_on_bearer_auth() {
    let (app, cookie, _c, _d) = app_with_auth(1).await;
    let key = json_of(
        send(
            &app,
            "POST",
            "/api/auth/keys",
            Some(&cookie),
            Some(r#"{"label":"stamp-me"}"#),
        )
        .await,
    )
    .await["key"]
        .as_str()
        .unwrap()
        .to_string();

    let list = json_of(send(&app, "GET", "/api/auth/keys", Some(&cookie), None).await).await;
    assert!(list[0]["last_used"].is_null());

    assert_eq!(
        send_bearer(&app, "GET", "/api/me", &key, None)
            .await
            .status(),
        StatusCode::OK
    );
    let mut stamped = false;
    for _ in 0..40 {
        let list = json_of(send(&app, "GET", "/api/auth/keys", Some(&cookie), None).await).await;
        if list[0]["last_used"].as_i64().is_some() {
            stamped = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(stamped, "last_used never landed after a bearer request");
}

#[tokio::test]
async fn avatar_gates_without_vips() {
    let (app, cookie, _c, _d) = app_with_auth(1).await;

    assert_eq!(
        send(&app, "GET", "/api/me/avatar", Some(&cookie), None)
            .await
            .status(),
        StatusCode::NOT_FOUND,
        "no avatar set yet"
    );
    assert_eq!(
        send(&app, "DELETE", "/api/me/avatar", Some(&cookie), None)
            .await
            .status(),
        StatusCode::OK,
        "delete is idempotent"
    );
    assert_eq!(
        send(&app, "GET", "/api/me/avatar", None, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&app, "PUT", "/api/me/avatar", None, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn reading_mode_via_api() {
    let (app, cookie, _c, _d) = app_with_auth(1).await;
    let listing = json_of(send(&app, "GET", "/api/items", Some(&cookie), None).await).await;
    let id = listing["items"][0]["id"].as_i64().unwrap();

    let detail = json_of(
        send(
            &app,
            "GET",
            &format!("/api/items/{id}"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(detail["reading_mode"], "paged");

    let r = send(
        &app,
        "PUT",
        &format!("/api/items/{id}/reading-mode"),
        Some(&cookie),
        Some(r#"{"mode":"vertical"}"#),
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);
    let detail = json_of(
        send(
            &app,
            "GET",
            &format!("/api/items/{id}"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(detail["reading_mode"], "vertical");

    let r = send(
        &app,
        "PUT",
        &format!("/api/items/{id}/reading-mode"),
        Some(&cookie),
        Some(r#"{"mode":"sideways"}"#),
    )
    .await;
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    let r = send(
        &app,
        "PUT",
        "/api/items/999999/reading-mode",
        Some(&cookie),
        Some(r#"{"mode":"vertical"}"#),
    )
    .await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    let r = send(
        &app,
        "PUT",
        &format!("/api/items/{id}/reading-mode"),
        None,
        Some(r#"{"mode":"vertical"}"#),
    )
    .await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    let r = send(
        &app,
        "DELETE",
        &format!("/api/items/{id}/reading-mode"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);
    let detail = json_of(
        send(
            &app,
            "GET",
            &format!("/api/items/{id}"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(detail["reading_mode"], "paged");
}

#[tokio::test]
async fn exclude_tags_and_blocklist() {
    let (app, cookie, _c, _d) = app_with_auth(3).await;

    let listing = json_of(send(&app, "GET", "/api/items", Some(&cookie), None).await).await;
    let arr = listing["items"].as_array().unwrap();
    let id0 = arr[0]["id"].as_i64().unwrap();
    let id1 = arr[1]["id"].as_i64().unwrap();

    for (id, body) in [
        (id0, r#"{"namespace":"tag","value":"mystery"}"#),
        (id1, r#"{"namespace":"tag","value":"vanilla"}"#),
    ] {
        let resp = send(
            &app,
            "POST",
            &format!("/api/items/{id}/tags"),
            Some(&cookie),
            Some(body),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }
    let ids_of = |v: serde_json::Value| -> Vec<i64> {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["id"].as_i64().unwrap())
            .collect()
    };

    let f = json_of(
        send(
            &app,
            "GET",
            "/api/items?tags=-tag:mystery",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    let got = ids_of(f);
    assert!(!got.contains(&id0));
    assert_eq!(got.len(), 2);

    let f = json_of(
        send(
            &app,
            "GET",
            "/api/items?tags=tag:vanilla&exclude=tag:mystery",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(ids_of(f), vec![id1]);

    let f = json_of(
        send(
            &app,
            "GET",
            "/api/items?exclude=tag:doesnotexist",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(ids_of(f).len(), 3);

    let f = json_of(
        send(
            &app,
            "GET",
            "/api/items?exclude=tag:mystery&page=1",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(f["total"].as_i64(), Some(2));

    let bl = json_of(send(&app, "GET", "/api/me/tag-blocklist", Some(&cookie), None).await).await;
    assert!(bl.as_array().unwrap().is_empty());

    let r = send(
        &app,
        "PUT",
        "/api/me/tag-blocklist",
        Some(&cookie),
        Some(r#"{"tags":["tag:mystery"]}"#),
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);
    let bl = json_of(send(&app, "GET", "/api/me/tag-blocklist", Some(&cookie), None).await).await;
    assert_eq!(bl[0]["namespace"], "tag");
    assert_eq!(bl[0]["value"], "mystery");

    let f = json_of(send(&app, "GET", "/api/items", Some(&cookie), None).await).await;
    let got = ids_of(f);
    assert!(!got.contains(&id0));
    assert_eq!(got.len(), 2);

    let f = json_of(
        send(
            &app,
            "GET",
            "/api/items?tags=tag:mystery",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(ids_of(f), vec![id0]);

    let r = send(
        &app,
        "PUT",
        "/api/me/tag-blocklist",
        Some(&cookie),
        Some(r#"{"tags":["tag:doesnotexist"]}"#),
    )
    .await;
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    let r = send(
        &app,
        "PUT",
        "/api/me/tag-blocklist",
        Some(&cookie),
        Some(r#"{"tags":["notag"]}"#),
    )
    .await;
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    let r = send(&app, "GET", "/api/me/tag-blocklist", None, None).await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    let r = send(
        &app,
        "PUT",
        "/api/me/tag-blocklist",
        Some(&cookie),
        Some(r#"{"tags":[]}"#),
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);
    let f = json_of(send(&app, "GET", "/api/items", Some(&cookie), None).await).await;
    assert_eq!(ids_of(f).len(), 3);
}

#[tokio::test]
async fn favorite_toggle_and_flag() {
    let (app, cookie, _c, _d) = app_with_auth(2).await;
    let listing = json_of(send(&app, "GET", "/api/items", Some(&cookie), None).await).await;
    let id = listing["items"][0]["id"].as_i64().unwrap().to_string();
    assert_eq!(
        listing["items"][0]["favorited"], false,
        "unfavorited initially"
    );

    let r = send(
        &app,
        "POST",
        &format!("/api/items/{id}/favorite"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);
    let detail = json_of(
        send(
            &app,
            "GET",
            &format!("/api/items/{id}"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(detail["favorited"], true);
    let listing = json_of(send(&app, "GET", "/api/items", Some(&cookie), None).await).await;
    let entry = listing["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"].as_i64().map(|n| n.to_string()).as_deref() == Some(id.as_str()))
        .unwrap();
    assert_eq!(entry["favorited"], true);

    assert_eq!(
        send(
            &app,
            "POST",
            &format!("/api/items/{id}/favorite"),
            Some(&cookie),
            None
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        send(
            &app,
            "DELETE",
            &format!("/api/items/{id}/favorite"),
            Some(&cookie),
            None
        )
        .await
        .status(),
        StatusCode::OK
    );
    let detail = json_of(
        send(
            &app,
            "GET",
            &format!("/api/items/{id}"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(detail["favorited"], false);

    assert_eq!(
        send(
            &app,
            "POST",
            "/api/items/999999/favorite",
            Some(&cookie),
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("/api/items/{id}/favorite"),
            None,
            None
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn rating_set_clear_sort_and_validation() {
    let (app, cookie, _c, _d) = app_with_auth(3).await;
    let listing = json_of(send(&app, "GET", "/api/items", Some(&cookie), None).await).await;
    let ids: Vec<String> = listing["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_i64().unwrap().to_string())
        .collect();
    let detail = json_of(
        send(
            &app,
            "GET",
            &format!("/api/items/{}", ids[0]),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert!(detail["rating"].is_null(), "unrated → null");

    let rate = |id: &str, v: i32| {
        let app = app.clone();
        let cookie = cookie.clone();
        let path = format!("/api/items/{id}/rating");
        let body = format!("{{\"value\":{v}}}");
        async move { send(&app, "PUT", &path, Some(&cookie), Some(&body)).await }
    };
    assert_eq!(rate(&ids[0], 5).await.status(), StatusCode::OK);
    assert_eq!(rate(&ids[1], 3).await.status(), StatusCode::OK);

    let detail = json_of(
        send(
            &app,
            "GET",
            &format!("/api/items/{}", ids[0]),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(detail["rating"].as_i64(), Some(5));

    let sorted =
        json_of(send(&app, "GET", "/api/items?sort=rating", Some(&cookie), None).await).await;
    let order: Vec<String> = sorted["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_i64().unwrap().to_string())
        .collect();
    assert_eq!(order[0], ids[0], "5★ first");
    assert_eq!(order[1], ids[1], "then 3★");
    assert_eq!(sorted["items"][0]["rating"].as_i64(), Some(5));
    assert!(sorted["items"][2]["rating"].is_null() || sorted["items"][2].get("rating").is_none());

    for bad in ["0", "11"] {
        let body = format!("{{\"value\":{bad}}}");
        assert_eq!(
            send(
                &app,
                "PUT",
                &format!("/api/items/{}/rating", ids[0]),
                Some(&cookie),
                Some(&body)
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST,
            "rating {bad} rejected"
        );
    }

    assert_eq!(
        send(
            &app,
            "DELETE",
            &format!("/api/items/{}/rating", ids[0]),
            Some(&cookie),
            None
        )
        .await
        .status(),
        StatusCode::OK
    );
    let detail = json_of(
        send(
            &app,
            "GET",
            &format!("/api/items/{}", ids[0]),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert!(detail["rating"].is_null(), "cleared → null");

    assert_eq!(
        send(
            &app,
            "PUT",
            "/api/items/999999/rating",
            Some(&cookie),
            Some(r#"{"value":3}"#)
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &app,
            "PUT",
            &format!("/api/items/{}/rating", ids[0]),
            None,
            Some(r#"{"value":3}"#)
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn continue_shelf_route_and_auth() {
    let (app, cookie, _c, _d) = app_with_auth(2).await;

    assert_eq!(
        send(&app, "GET", "/api/items/continue", None, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let j = json_of(send(&app, "GET", "/api/items/continue", Some(&cookie), None).await).await;
    assert!(j["items"].as_array().unwrap().is_empty());

    let listing = json_of(send(&app, "GET", "/api/items", Some(&cookie), None).await).await;
    let id = listing["items"][0]["id"].as_i64().unwrap().to_string();
    let r = send(
        &app,
        "PUT",
        &format!("/api/items/{id}/progress"),
        Some(&cookie),
        Some(r#"{"page":0}"#),
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);

    let j = json_of(send(&app, "GET", "/api/items/continue", Some(&cookie), None).await).await;
    let arr = j["items"].as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(
        arr[0]["id"].as_i64().map(|n| n.to_string()).as_deref(),
        Some(id.as_str())
    );
    assert_eq!(arr[0]["kind"], "uncategorized");
    assert_eq!(arr[0]["progress"].as_i64(), Some(0));
    assert!(arr[0].get("last_read_at").is_some());

    let same = json_of(
        send(
            &app,
            "GET",
            "/api/items/continue?kind=uncategorized",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(same["items"].as_array().unwrap().len(), 1);
    let other = json_of(
        send(
            &app,
            "GET",
            "/api/items/continue?kind=manga",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert!(
        other["items"].as_array().unwrap().is_empty(),
        "kind filter excludes other kinds"
    );
}

#[tokio::test]
async fn page_thumb_route_auth_and_bounds() {
    let (app, cookie, _c, _d) = app_with_auth(1).await;
    let listing = json_of(send(&app, "GET", "/api/items", Some(&cookie), None).await).await;
    let id = listing["items"][0]["id"].as_i64().unwrap().to_string();

    assert_eq!(
        send(
            &app,
            "GET",
            &format!("/api/items/{id}/pages/0/thumbnail"),
            None,
            None
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &app,
            "GET",
            &format!("/api/items/{id}/pages/9999/thumbnail"),
            Some(&cookie),
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &app,
            "GET",
            "/api/items/999999/pages/0/thumbnail",
            Some(&cookie),
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &app,
            "GET",
            "/api/items/x/pages/0/thumbnail",
            Some(&cookie),
            None
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn serves_a_page_and_404s_appropriately() {
    let (app, cookie, _c, _d) = app_with_auth(1).await;

    let json = json_of(send(&app, "GET", "/api/items", Some(&cookie), None).await).await;
    let id = json["items"][0]["id"].as_i64().unwrap().to_string();

    let resp = send(
        &app,
        "GET",
        &format!("/api/items/{id}/pages/0"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers().get("content-type").unwrap(), "image/jpeg");

    let resp = send(
        &app,
        "GET",
        &format!("/api/items/{id}/pages/999"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let resp = send(
        &app,
        "GET",
        "/api/items/999999/pages/0",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let json = json_of(send(&app, "GET", "/api/items", Some(&cookie), None).await).await;
    assert_eq!(json["items"][0]["page_count"].as_i64(), Some(3));
}

#[tokio::test]
async fn image_responses_revalidate_via_etag() {
    let (app, cookie, _c, _d) = app_with_auth(1).await;
    let json = json_of(send(&app, "GET", "/api/items?limit=1", Some(&cookie), None).await).await;
    let id = json["items"][0]["id"].as_i64().unwrap();
    let uri = format!("/api/items/{id}/pages/0");

    let r1 = send(&app, "GET", &uri, Some(&cookie), None).await;
    assert_eq!(r1.status(), StatusCode::OK);
    assert_eq!(
        r1.headers().get("cache-control").unwrap(),
        "private, no-cache"
    );
    let etag = r1
        .headers()
        .get("etag")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        etag.starts_with('"') && etag.ends_with('"'),
        "quoted: {etag}"
    );

    let cond = |inm: &str| {
        Request::builder()
            .method("GET")
            .uri(&uri)
            .header("cookie", &cookie)
            .header("if-none-match", inm)
            .body(Body::empty())
            .unwrap()
    };
    let r2 = app.clone().oneshot(cond(&etag)).await.unwrap();
    assert_eq!(r2.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(r2.headers().get("etag").unwrap().to_str().unwrap(), etag);

    let r3 = app.clone().oneshot(cond("\"stale:0\"")).await.unwrap();
    assert_eq!(r3.status(), StatusCode::OK);
}

#[tokio::test]
async fn read_progress_roundtrips_and_is_listed() {
    let (app, cookie, _c, _d) = app_with_auth(1).await;
    let json = json_of(send(&app, "GET", "/api/items", Some(&cookie), None).await).await;
    let id = json["items"][0]["id"].as_i64().unwrap().to_string();
    assert!(json["items"][0]["progress"].is_null(), "unread initially");

    let resp = send(
        &app,
        "PUT",
        &format!("/api/items/{id}/progress"),
        Some(&cookie),
        Some(r#"{"page":0}"#),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let d = json_of(
        send(
            &app,
            "GET",
            &format!("/api/items/{id}"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(d["progress"].as_i64(), Some(0));
    assert!(d["added_at"].is_i64(), "detail carries added_at");
    let json = json_of(send(&app, "GET", "/api/items", Some(&cookie), None).await).await;
    assert_eq!(json["items"][0]["progress"].as_i64(), Some(0));
    assert!(
        json["items"][0]["added_at"].is_i64(),
        "listing carries added_at"
    );

    let resp = send(
        &app,
        "PUT",
        &format!("/api/items/{id}/progress"),
        Some(&cookie),
        Some(r#"{"page":99999}"#),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let d = json_of(
        send(
            &app,
            "GET",
            &format!("/api/items/{id}"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(
        d["progress"].as_i64(),
        Some(2),
        "out-of-range progress clamped"
    );

    let resp = send(
        &app,
        "PUT",
        "/api/items/999999/progress",
        Some(&cookie),
        Some(r#"{"page":0}"#),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let resp = send(
        &app,
        "PUT",
        &format!("/api/items/{id}/progress"),
        Some(&cookie),
        Some(r#"{"page":-1}"#),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let resp = send(
        &app,
        "PUT",
        &format!("/api/items/{id}/progress"),
        None,
        Some(r#"{"page":0}"#),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn favorites_filter_composes_with_listing() {
    let (app, cookie, _c, _d) = app_with_auth(4).await;
    let all = json_of(send(&app, "GET", "/api/items", Some(&cookie), None).await).await;
    let ids: Vec<String> = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_i64().unwrap().to_string())
        .collect();
    assert_eq!(ids.len(), 4);

    for id in [&ids[0], &ids[2]] {
        let r = send(
            &app,
            "POST",
            &format!("/api/items/{id}/favorite"),
            Some(&cookie),
            None,
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
    }

    let fav = json_of(
        send(
            &app,
            "GET",
            "/api/items?favorited=true",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    let fav_items = fav["items"].as_array().unwrap();
    assert_eq!(fav_items.len(), 2);
    assert!(fav_items.iter().all(|i| i["favorited"] == true));
    let fav_ids: Vec<String> = fav_items
        .iter()
        .map(|i| i["id"].as_i64().unwrap().to_string())
        .collect();
    assert!(fav_ids.contains(&ids[0]) && fav_ids.contains(&ids[2]));

    let sorted = json_of(
        send(
            &app,
            "GET",
            "/api/items?favorited=true&sort=title",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    let names: Vec<String> = sorted["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap().to_string())
        .collect();
    let mut want = names.clone();
    want.sort();
    assert_eq!(names, want);

    let page = json_of(
        send(
            &app,
            "GET",
            "/api/items?favorited=true&page=1",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(
        page["total"].as_i64(),
        Some(2),
        "page-jump count is favorites-scoped"
    );

    let non = json_of(
        send(
            &app,
            "GET",
            "/api/items?favorited=false",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    let non_items = non["items"].as_array().unwrap();
    assert_eq!(non_items.len(), 2);
    assert!(non_items.iter().all(|i| i["favorited"] == false));
}

#[tokio::test]
async fn untagged_and_completed_filters_via_api() {
    let (app, cookie, _c, _d) = app_with_auth(3).await;
    let all = json_of(send(&app, "GET", "/api/items", Some(&cookie), None).await).await;
    let ids: Vec<String> = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_i64().unwrap().to_string())
        .collect();
    assert_eq!(ids.len(), 3);

    let count = |v: &serde_json::Value| v["items"].as_array().unwrap().len();

    let untagged =
        json_of(send(&app, "GET", "/api/items?untagged=true", Some(&cookie), None).await).await;
    assert_eq!(count(&untagged), 3);

    let r = send(
        &app,
        "POST",
        &format!("/api/items/{}/tags", ids[0]),
        Some(&cookie),
        Some(r#"{"namespace":"tag","value":"x"}"#),
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);
    let untagged =
        json_of(send(&app, "GET", "/api/items?untagged=true", Some(&cookie), None).await).await;
    assert_eq!(count(&untagged), 2, "the tagged item is excluded");
    let tagged = json_of(
        send(
            &app,
            "GET",
            "/api/items?untagged=false",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    let tagged_ids: Vec<String> = tagged["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_i64().unwrap().to_string())
        .collect();
    assert_eq!(tagged_ids, vec![ids[0].clone()]);

    let done = json_of(
        send(
            &app,
            "GET",
            "/api/items?completed=true",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(count(&done), 0);
    let not_done = json_of(
        send(
            &app,
            "GET",
            "/api/items?completed=false",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(count(&not_done), 3);

    let combo = json_of(
        send(
            &app,
            "GET",
            "/api/items?untagged=true&completed=false",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(count(&combo), 2);
}

#[tokio::test]
async fn delete_item_removes_file_and_cascades_via_api() {
    let (app, cookie, content, data) = app_with_auth(2).await;
    let file0 = content.path().join("book-0.cbz");
    assert!(file0.exists(), "precondition: the source file is on disk");

    let all = json_of(send(&app, "GET", "/api/items", Some(&cookie), None).await).await;
    let id0 = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == "book-0")
        .unwrap()["id"]
        .as_i64()
        .unwrap()
        .to_string();

    let structural = match arcagrad::media::identity::identify(&file0).unwrap() {
        arcagrad::media::identity::Identity::Ready {
            structural_hash, ..
        } => structural_hash,
        _ => panic!("book-0 must be a readable archive"),
    };
    let cover = arcagrad::media::thumbnail::cache_path(data.path(), &structural);
    let page_thumb = arcagrad::media::thumbnail::page_cache_path(data.path(), &structural, 0);
    std::fs::create_dir_all(cover.parent().unwrap()).unwrap();
    std::fs::write(&cover, b"fake-cover").unwrap();
    std::fs::create_dir_all(page_thumb.parent().unwrap()).unwrap();
    std::fs::write(&page_thumb, b"fake-page-thumb").unwrap();

    send(
        &app,
        "POST",
        &format!("/api/items/{id0}/tags"),
        Some(&cookie),
        Some(r#"{"namespace":"tag","value":"x"}"#),
    )
    .await;
    send(
        &app,
        "POST",
        &format!("/api/items/{id0}/favorite"),
        Some(&cookie),
        None,
    )
    .await;

    let r = send(
        &app,
        "DELETE",
        &format!("/api/items/{id0}"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);

    assert!(!file0.exists(), "the source file was actually removed");
    assert!(!cover.exists(), "cover thumbnail removed");
    assert!(!page_thumb.exists(), "page thumbnail removed");
    assert!(
        !page_thumb.parent().unwrap().exists(),
        "per-item page-thumbnail directory removed"
    );
    let after = send(
        &app,
        "GET",
        &format!("/api/items/{id0}"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(after.status(), StatusCode::NOT_FOUND);
    let list = json_of(send(&app, "GET", "/api/items", Some(&cookie), None).await).await;
    let names: Vec<&str> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["book-1"]);
    let fav = json_of(
        send(
            &app,
            "GET",
            "/api/items?favorited=true",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(
        fav["items"].as_array().unwrap().len(),
        0,
        "favorite cascaded"
    );
}

#[tokio::test]
async fn delete_unknown_id_404s_and_deletes_nothing() {
    let (app, cookie, content, _d) = app_with_auth(2).await;

    for bogus in ["999999", "888888", "0"] {
        let r = send(
            &app,
            "DELETE",
            &format!("/api/items/{bogus}"),
            Some(&cookie),
            None,
        )
        .await;
        assert_eq!(
            r.status(),
            StatusCode::NOT_FOUND,
            "DELETE /api/items/{bogus} should 404"
        );
    }

    let list = json_of(send(&app, "GET", "/api/items", Some(&cookie), None).await).await;
    assert_eq!(
        list["items"].as_array().unwrap().len(),
        2,
        "no items deleted"
    );
    assert!(content.path().join("book-0.cbz").exists());
    assert!(content.path().join("book-1.cbz").exists());
}

#[tokio::test]
async fn upload_creates_item_with_declared_kind_and_dedups() {
    let (app, cookie, content, _d) = app_with_auth(0).await;
    let cbz = cbz_bytes("u1");

    let (ct, body) = multipart_body(&[
        ("kind", None, b"manga".as_slice()),
        ("file", Some("My Comic.cbz"), &cbz),
    ]);
    let resp = send_raw(&app, "POST", "/api/items", Some(&cookie), &ct, body).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let j = json_of(resp).await;
    assert_eq!(j["kind"], "manga");
    assert_eq!(j["created"], true);
    assert_eq!(j["title"], "My Comic");
    let id = j["id"].as_i64().unwrap().to_string();

    assert!(
        content.path().join("manga").join("My Comic.cbz").exists(),
        "file routed into the kind folder"
    );
    let detail = json_of(
        send(
            &app,
            "GET",
            &format!("/api/items/{id}"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(detail["kind"], "manga");
    let list = json_of(send(&app, "GET", "/api/items", Some(&cookie), None).await).await;
    let card = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"].as_i64().map(|n| n.to_string()).as_deref() == Some(id.as_str()))
        .unwrap();
    assert_eq!(card["kind"], "manga");
    assert_eq!(card["modality"], "paginated");

    let (ct2, body2) = multipart_body(&[("file", Some("again.cbz"), &cbz)]);
    let resp2 = send_raw(&app, "POST", "/api/items", Some(&cookie), &ct2, body2).await;
    assert_eq!(resp2.status(), StatusCode::OK);
    let j2 = json_of(resp2).await;
    assert_eq!(j2["created"], false);
    assert_eq!(j2["id"].as_i64().unwrap().to_string(), id);
    assert!(
        !content.path().join("manga").join("again.cbz").exists(),
        "dedup wrote no second file"
    );

    let (ct3, body3) = multipart_body(&[("file", Some("plain.cbz"), &cbz_bytes("u2"))]);
    let j3 = json_of(send_raw(&app, "POST", "/api/items", Some(&cookie), &ct3, body3).await).await;
    assert_eq!(j3["kind"], "uncategorized");
    assert!(content
        .path()
        .join("uncategorized")
        .join("plain.cbz")
        .exists());

    let (ct4, body4) = multipart_body(&[("file", Some("bad.cbz"), b"not a zip".as_slice())]);
    let resp4 = send_raw(&app, "POST", "/api/items", Some(&cookie), &ct4, body4).await;
    assert_eq!(resp4.status(), StatusCode::BAD_REQUEST);

    let (ct5, body5) = multipart_body(&[
        ("kind", None, "Western Comics".as_bytes()),
        ("file", Some("wc.cbz"), &cbz_bytes("u3")),
    ]);
    let j5 = json_of(send_raw(&app, "POST", "/api/items", Some(&cookie), &ct5, body5).await).await;
    assert_eq!(j5["kind"], "Western Comics");
    assert!(content
        .path()
        .join("Western Comics")
        .join("wc.cbz")
        .exists());

    let (ct7, body7) = multipart_body(&[
        ("kind", None, b"..".as_slice()),
        ("file", Some("e.cbz"), &cbz_bytes("u6")),
    ]);
    let resp7 = send_raw(&app, "POST", "/api/items", Some(&cookie), &ct7, body7).await;
    assert_eq!(resp7.status(), StatusCode::BAD_REQUEST);

    let (ct6, body6) = multipart_body(&[("file", Some("evil.exe"), &cbz_bytes("u4"))]);
    let resp6 = send_raw(&app, "POST", "/api/items", Some(&cookie), &ct6, body6).await;
    assert_eq!(resp6.status(), StatusCode::BAD_REQUEST);
    assert!(!content.path().join("evil.exe").exists());
}

#[tokio::test]
async fn kinds_endpoint_and_filter() {
    let (app, cookie, _content, _d) = app_with_auth(0).await;
    for (kind, name) in [
        ("manga", "m1.cbz"),
        ("manga", "m2.cbz"),
        ("comics", "d1.cbz"),
    ] {
        let cbz = cbz_bytes(name);
        let (ct, body) =
            multipart_body(&[("kind", None, kind.as_bytes()), ("file", Some(name), &cbz)]);
        let r = send_raw(&app, "POST", "/api/items", Some(&cookie), &ct, body).await;
        assert_eq!(r.status(), StatusCode::CREATED);
    }

    let kinds = json_of(send(&app, "GET", "/api/kinds", Some(&cookie), None).await).await;
    let arr = kinds.as_array().unwrap();
    let count_of = |k: &str| {
        arr.iter()
            .find(|e| e["kind"] == k)
            .map(|e| e["count"].as_i64().unwrap())
    };
    assert_eq!(count_of("manga"), Some(2));
    assert_eq!(count_of("comics"), Some(1));

    let list = json_of(send(&app, "GET", "/api/items?kind=manga", Some(&cookie), None).await).await;
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|i| i["kind"] == "manga"));
}

#[tokio::test]
async fn emptying_a_kind_drops_it_from_kinds() {
    let (app, cookie, _content, _d) = app_with_auth(0).await;
    let mut manga_id = String::new();
    for (kind, name) in [("manga", "m.cbz"), ("comics", "d.cbz")] {
        let cbz = cbz_bytes(name);
        let (ct, body) =
            multipart_body(&[("kind", None, kind.as_bytes()), ("file", Some(name), &cbz)]);
        let j = json_of(send_raw(&app, "POST", "/api/items", Some(&cookie), &ct, body).await).await;
        if kind == "manga" {
            manga_id = j["id"].as_i64().unwrap().to_string();
        }
    }
    let names = |v: &serde_json::Value| -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|e| e["kind"].as_str().unwrap().to_string())
            .collect()
    };
    let before = json_of(send(&app, "GET", "/api/kinds", Some(&cookie), None).await).await;
    assert_eq!(names(&before), vec!["comics", "manga"]);

    let r = send(
        &app,
        "DELETE",
        &format!("/api/items/{manga_id}"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);
    let after = json_of(send(&app, "GET", "/api/kinds", Some(&cookie), None).await).await;
    assert_eq!(names(&after), vec!["comics"], "emptied kind is gone");
}

#[tokio::test]
async fn upload_same_filename_different_content_does_not_overwrite() {
    let (app, cookie, content, _d) = app_with_auth(0).await;

    let (ct_a, body_a) = multipart_body(&[("file", Some("dup.cbz"), &cbz_bytes("A"))]);
    let a = json_of(send_raw(&app, "POST", "/api/items", Some(&cookie), &ct_a, body_a).await).await;
    let (ct_b, body_b) = multipart_body(&[("file", Some("dup.cbz"), &cbz_bytes("B"))]);
    let b = json_of(send_raw(&app, "POST", "/api/items", Some(&cookie), &ct_b, body_b).await).await;

    assert_ne!(a["id"], b["id"], "different content → different items");
    let kdir = content.path().join("uncategorized");
    assert!(kdir.join("dup.cbz").exists(), "first file intact");
    assert!(
        kdir.join("dup-1.cbz").exists(),
        "second file got a -1 suffix"
    );
    let list = json_of(send(&app, "GET", "/api/items", Some(&cookie), None).await).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 2);

    let leftovers: Vec<_> = std::fs::read_dir(content.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(".arca-upload-"))
        .collect();
    assert!(leftovers.is_empty(), "no temp files left behind");
}

#[tokio::test]
async fn similar_ranks_by_thematic_cosine_with_artist_bonus() {
    let (app, cookie, _c, _d) = app_with_auth(4).await;

    let all = json_of(send(&app, "GET", "/api/items", Some(&cookie), None).await).await;
    let mut hash = std::collections::HashMap::new();
    for it in all["items"].as_array().unwrap() {
        hash.insert(
            it["name"].as_str().unwrap().to_string(),
            it["id"].as_i64().unwrap().to_string(),
        );
    }
    let edits = [
        ("book-0", "creator", "foo"),
        ("book-0", "tag", "shared"),
        ("book-1", "creator", "foo"),
        ("book-1", "tag", "x1"),
        ("book-2", "tag", "shared"),
        ("book-2", "tag", "x2"),
        ("book-3", "tag", "x3"),
    ];
    for (name, ns, v) in edits {
        let body = format!(r#"{{"namespace":"{ns}","value":"{v}"}}"#);
        let uri = format!("/api/items/{}/tags", hash[name]);
        let r = send(&app, "POST", &uri, Some(&cookie), Some(&body)).await;
        assert_eq!(r.status(), StatusCode::OK, "tag {ns}:{v}");
    }

    let sim = json_of(
        send(
            &app,
            "GET",
            &format!("/api/items/{}/similar", hash["book-0"]),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    let items = sim["items"].as_array().unwrap();
    let names: Vec<&str> = items.iter().map(|i| i["name"].as_str().unwrap()).collect();

    assert_eq!(
        names,
        vec!["book-2", "book-1"],
        "shared theme ranks above an artist-only match"
    );
    assert!(items[0]["score"].as_f64().unwrap() > items[1]["score"].as_f64().unwrap());
    assert!(items[0]["favorited"].is_boolean() && items[0]["tags"].is_array());

    let r = send(
        &app,
        "GET",
        "/api/items/999999/similar",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn similar_reflects_new_tags_without_restart() {
    let (app, cookie, _c, _d) = app_with_auth(3).await;
    let all = json_of(send(&app, "GET", "/api/items", Some(&cookie), None).await).await;
    let mut hash = std::collections::HashMap::new();
    for it in all["items"].as_array().unwrap() {
        hash.insert(
            it["name"].as_str().unwrap().to_string(),
            it["id"].as_i64().unwrap().to_string(),
        );
    }
    let post_tag = |name: &str, ns: &str, v: &str| {
        (
            format!("/api/items/{}/tags", hash[name]),
            format!(r#"{{"namespace":"{ns}","value":"{v}"}}"#),
        )
    };
    let names = |json: &serde_json::Value| -> Vec<String> {
        json["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["name"].as_str().unwrap().to_string())
            .collect()
    };

    for (name, ns, v) in [
        ("book-0", "creator", "foo"),
        ("book-0", "tag", "shared"),
        ("book-1", "creator", "foo"),
    ] {
        let (uri, body) = post_tag(name, ns, v);
        let r = send(&app, "POST", &uri, Some(&cookie), Some(&body)).await;
        assert_eq!(r.status(), StatusCode::OK);
    }

    let uri = format!("/api/items/{}/similar", hash["book-0"]);
    let first = json_of(send(&app, "GET", &uri, Some(&cookie), None).await).await;
    assert_eq!(names(&first), vec!["book-1"]);

    let (uri2, body2) = post_tag("book-2", "tag", "shared");
    let r = send(&app, "POST", &uri2, Some(&cookie), Some(&body2)).await;
    assert_eq!(r.status(), StatusCode::OK);

    let second = json_of(send(&app, "GET", &uri, Some(&cookie), None).await).await;
    assert!(
        names(&second).contains(&"book-2".to_string()),
        "new tag reflected immediately (neighbour cache invalidated): {:?}",
        names(&second)
    );
}

#[tokio::test]
async fn recommendations_needs_auth_and_is_empty_without_signals() {
    let (app, cookie, _c, _d) = app_with_auth(3).await;

    let r = json_of(send(&app, "GET", "/api/recommendations", Some(&cookie), None).await).await;
    assert!(
        r["items"].as_array().unwrap().is_empty(),
        "no signals → empty recommendations"
    );

    let resp = send(&app, "GET", "/api/recommendations", None, None).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn upcoming_lists_only_tracked_in_window_releases() {
    let (state, _c, _d) = build_state(1).await;
    let sid: i64 = sqlx::query_scalar(
        "INSERT INTO series (kind, title, folder_path, added_at) \
         VALUES ('manga','S','manga/S',0) RETURNING id",
    )
    .fetch_one(&state.write)
    .await
    .unwrap();
    let item_id: i64 = sqlx::query_scalar("SELECT id FROM items LIMIT 1")
        .fetch_one(&state.read)
        .await
        .unwrap();
    sqlx::query("UPDATE items SET series_id = ? WHERE id = ?")
        .bind(sid)
        .bind(item_id)
        .execute(&state.write)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO item_series_leaf (item_id, series_id, number_sort) VALUES (?, ?, 1.0)",
    )
    .bind(item_id)
    .bind(sid)
    .execute(&state.write)
    .await
    .unwrap();
    arcagrad::repo::set_series_tracker(&state.write, sid, "viz", "ref")
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO series_upcoming \
         (series_id, provider, provider_release_id, reference_source, label, \
          release_date, date_precision, date_status, fetched_at) \
         VALUES (?, 'viz', 'r-in', 'viz', 'Vol', date('now','+30 days'), 'day', 'announced', 0)",
    )
    .bind(sid)
    .execute(&state.write)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO series_upcoming \
         (series_id, provider, provider_release_id, reference_source, label, \
          release_date, date_precision, date_status, fetched_at) \
         VALUES (?, 'viz', 'r-out', 'viz', 'Vol', date('now','+400 days'), 'day', 'announced', 0)",
    )
    .bind(sid)
    .execute(&state.write)
    .await
    .unwrap();

    arcagrad::server::jobs::enqueue(&state.write, "check_calendar", Some("{\"manual\":true}"))
        .await
        .unwrap();

    let app = routes::router(state.clone());
    let cookie = cookie_of(&send(&app, "POST", "/api/auth/setup", None, Some(CREDS)).await);

    let res = json_of(send(&app, "GET", "/api/upcoming", Some(&cookie), None).await).await;
    assert!(
        res["next_refresh_at"].is_null(),
        "a manual (run_after=0) refresh must not be reported as next_refresh_at: {}",
        res["next_refresh_at"]
    );
    let releases = res["releases"].as_array().unwrap();
    assert_eq!(
        releases.len(),
        1,
        "only the in-window, tracked release: {releases:?}"
    );
    assert_eq!(
        releases[0]["cover_item_id"].as_i64(),
        Some(item_id),
        "cover resolves to the series' first leaf"
    );

    arcagrad::repo::delete_series_tracker(&state.write, sid, "viz")
        .await
        .unwrap();
    let res2 = json_of(send(&app, "GET", "/api/upcoming", Some(&cookie), None).await).await;
    assert_eq!(
        res2["releases"].as_array().unwrap().len(),
        0,
        "untracked → not listed"
    );
}

#[tokio::test]
async fn deleting_a_leaf_enqueues_its_series_entry_recompute() {
    let (state, _c, _d) = build_state(1).await;
    let sid: i64 = sqlx::query_scalar(
        "INSERT INTO series (kind, title, folder_path, added_at) VALUES ('manga','S','manga/S',0) RETURNING id",
    )
    .fetch_one(&state.write).await.unwrap();
    let item_id: i64 = sqlx::query_scalar("SELECT id FROM items LIMIT 1")
        .fetch_one(&state.read)
        .await
        .unwrap();
    sqlx::query("UPDATE items SET series_id = ? WHERE id = ?")
        .bind(sid)
        .bind(item_id)
        .execute(&state.write)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO item_series_leaf (item_id, series_id, number_sort) VALUES (?, ?, 1.0)",
    )
    .bind(item_id)
    .bind(sid)
    .execute(&state.write)
    .await
    .unwrap();

    let app = routes::router(state.clone());
    let cookie = cookie_of(&send(&app, "POST", "/api/auth/setup", None, Some(CREDS)).await);
    let r = send(
        &app,
        "DELETE",
        &format!("/api/items/{item_id}"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);

    let payload: Option<String> = sqlx::query_scalar(
        "SELECT payload FROM jobs WHERE kind = 'recompute_entry_neighbors' \
         AND state = 'pending' ORDER BY id DESC LIMIT 1",
    )
    .fetch_optional(&state.read)
    .await
    .unwrap()
    .flatten();
    let payload = payload.expect("a targeted entry recompute was enqueued on leaf delete");
    assert!(
        payload.contains(&format!("{}", -sid)),
        "recompute targets the series entry key {}: got {payload}",
        -sid
    );
}

#[tokio::test]
async fn bad_entry_recompute_payload_is_a_noop_not_a_full_sweep() {
    let (state, _c, _d) = build_state(3).await;
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM items ORDER BY id")
        .fetch_all(&state.read)
        .await
        .unwrap();
    for &iid in &ids[..2] {
        for v in ["a", "b"] {
            let t = arcagrad::repo::get_or_create_tag(&state.write, "tag", v)
                .await
                .unwrap();
            arcagrad::repo::add_item_tag(&state.write, iid, t, "none", "manual")
                .await
                .unwrap();
        }
    }
    let cancel = tokio_util::sync::CancellationToken::new();
    let job = |payload: Option<&str>| arcagrad::server::jobs::Job {
        id: 1,
        kind: "recompute_entry_neighbors".into(),
        payload: payload.map(String::from),
        attempts: 1,
    };
    async fn count(state: &AppState) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM entry_neighbors")
            .fetch_one(&state.read)
            .await
            .unwrap()
    }

    arcagrad::server::jobs::run_job(&state, &job(Some("{")), &cancel)
        .await
        .unwrap();
    assert_eq!(count(&state).await, 0, "bad payload must not sweep");
    arcagrad::server::jobs::run_job(&state, &job(None), &cancel)
        .await
        .unwrap();
    assert!(
        count(&state).await > 0,
        "a no-payload job does the full sweep"
    );
}

#[tokio::test]
async fn scan_builds_item_neighbors_when_not_ready_even_without_content_changes() {
    let (state, _c, _d) = build_state(2).await;
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM item_neighbors")
        .fetch_one(&state.read)
        .await
        .unwrap();
    assert_eq!(before, 0, "precondition: empty item_neighbors");

    let cancel = tokio_util::sync::CancellationToken::new();
    let scan_id = arcagrad::server::jobs::enqueue(&state.write, "scan", None)
        .await
        .unwrap();
    arcagrad::server::jobs::run_job(
        &state,
        &arcagrad::server::jobs::Job {
            id: scan_id,
            kind: "scan".into(),
            payload: None,
            attempts: 1,
        },
        &cancel,
    )
    .await
    .unwrap();

    let queued: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM jobs \
         WHERE kind = 'recompute_neighbors' AND payload IS NULL AND state = 'pending'",
    )
    .fetch_one(&state.read)
    .await
    .unwrap();
    assert!(
        queued >= 1,
        "an unbuilt item index must trigger a full sweep on a static-library scan"
    );
}

#[tokio::test]
async fn series_endpoint_and_item_series_block() {
    let content = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let s = content.path().join("manga").join("Attack on Titan");
    std::fs::create_dir_all(&s).unwrap();
    write_cbz(&s.join("Attack on Titan v01.cbz"), "v1");
    write_cbz(&s.join("Attack on Titan v02.cbz"), "v2");
    write_cbz(&content.path().join("manga").join("Loose One.cbz"), "one");

    let db = db::connect(data.path()).await.unwrap();
    scanner::scan(&db.write, content.path()).await.unwrap();
    let config = Arc::new(Config {
        content_dir: content.path().to_path_buf(),
        data_dir: data.path().to_path_buf(),
        bind: "0.0.0.0:0".into(),
        cookie_secure: false,
        read_concurrency: 8,
        allow_private_repos: false,
        watch: false,
    });
    let app = routes::router(AppState {
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
            64,
        )),
        for_you: Arc::new(arcagrad::intelligence::recommend::RecommendationCache::new(
            64,
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
    let resp = send(&app, "POST", "/api/auth/setup", None, Some(CREDS)).await;
    let cookie = cookie_of(&resp);

    let list = json_of(send(&app, "GET", "/api/items?limit=50", Some(&cookie), None).await).await;
    let cards = list["items"].as_array().unwrap();
    assert_eq!(
        cards.len(),
        2,
        "one series card + one one-shot, leaves collapsed"
    );

    let kinds = json_of(send(&app, "GET", "/api/kinds", Some(&cookie), None).await).await;
    let manga = kinds
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["kind"] == "manga")
        .expect("a manga kind");
    assert_eq!(
        manga["count"].as_i64(),
        Some(2),
        "collapsed card count (1 series + 1 one-shot), not 3 volumes"
    );

    let series_card = cards
        .iter()
        .find(|c| c["type"] == "series")
        .expect("a series card");
    assert_eq!(series_card["name"], "Attack on Titan");
    assert_eq!(series_card["leaf_count"].as_i64(), Some(2));
    let series_id = series_card["id"].as_i64().unwrap();
    let v1 = series_card["cover_item_id"].as_i64().unwrap();
    let loose_card = cards
        .iter()
        .find(|c| c["type"] == "item")
        .expect("a one-shot item card");
    assert_eq!(loose_card["name"], "Loose One");
    let loose = loose_card["id"].as_i64().unwrap();
    assert!(
        !cards.iter().any(|c| c["name"] == "Attack on Titan v01"),
        "leaves must not appear individually in the collapsed browse"
    );

    let sd0 = json_of(
        send(
            &app,
            "GET",
            &format!("/api/series/{series_id}"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    let v2 = sd0["leaves"][1]["item_id"].as_i64().unwrap();

    let d1 = json_of(
        send(
            &app,
            "GET",
            &format!("/api/items/{v1}"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(d1["reader"], "paginated-series");
    assert_eq!(d1["series"]["title"], "Attack on Titan");
    assert_eq!(d1["series"]["number_disp"], "Vol. 1");
    assert!(d1["series"]["prev_leaf_id"].is_null(), "v1 has no previous");
    assert_eq!(d1["series"]["next_leaf_id"].as_i64(), Some(v2));
    assert_eq!(
        d1["series"]["id"].as_i64(),
        Some(series_id),
        "leaf detail points back to the same series"
    );

    let d2 = json_of(
        send(
            &app,
            "GET",
            &format!("/api/items/{v2}"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(d2["series"]["prev_leaf_id"].as_i64(), Some(v1));
    assert!(d2["series"]["next_leaf_id"].is_null(), "v2 is last");

    let dl = json_of(
        send(
            &app,
            "GET",
            &format!("/api/items/{loose}"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(dl["reader"], "paginated");
    assert!(dl.get("series").is_none() || dl["series"].is_null());

    let resp = send(
        &app,
        "POST",
        &format!("/api/items/{v2}/tags"),
        Some(&cookie),
        Some(r#"{"namespace":"creator","value":"isayama"}"#),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let sd = json_of(
        send(
            &app,
            "GET",
            &format!("/api/series/{series_id}"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(sd["title"], "Attack on Titan");
    assert_eq!(sd["kind"], "manga");
    assert_eq!(sd["cover_item_id"].as_i64(), Some(v1));
    let leaves = sd["leaves"].as_array().unwrap();
    assert_eq!(leaves.len(), 2);
    assert_eq!(leaves[0]["item_id"].as_i64(), Some(v1));
    assert_eq!(leaves[0]["number_disp"], "Vol. 1");
    assert_eq!(leaves[1]["item_id"].as_i64(), Some(v2));
    assert_eq!(sd["resume_leaf_id"].as_i64(), Some(v1));
    assert_eq!(sd["read_count"].as_i64(), Some(0));
    let tags = sd["tags"].as_array().unwrap();
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[0]["namespace"], "creator");
    assert_eq!(tags[0]["value"], "isayama");

    let tf = json_of(
        send(
            &app,
            "GET",
            "/api/items?tags=creator:isayama",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    let tf_cards = tf["items"].as_array().unwrap();
    assert_eq!(tf_cards.len(), 1, "only the series matches");
    assert_eq!(tf_cards[0]["type"], "series");
    assert_eq!(tf_cards[0]["id"].as_i64(), Some(series_id));

    let sc = &tf_cards[0];
    assert_eq!(sc["favorited"], false);
    assert_eq!(sc["leaf_count"].as_i64(), Some(2));
    assert_eq!(sc["read_count"].as_i64(), Some(0), "nothing read yet");

    let resp = send(
        &app,
        "POST",
        &format!("/api/series/{series_id}/favorite"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let fav = json_of(
        send(
            &app,
            "GET",
            "/api/items?favorited=true",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    let fav_cards = fav["items"].as_array().unwrap();
    assert_eq!(fav_cards.len(), 1, "favorited browse stays collapsed");
    assert_eq!(fav_cards[0]["type"], "series");
    assert_eq!(fav_cards[0]["id"].as_i64(), Some(series_id));
    assert_eq!(fav_cards[0]["favorited"], true);

    let sd2 = json_of(
        send(
            &app,
            "GET",
            &format!("/api/series/{series_id}"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(sd2["favorited"], true);

    let resp = send(
        &app,
        "DELETE",
        &format!("/api/series/{series_id}/favorite"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let fav = json_of(
        send(
            &app,
            "GET",
            "/api/items?favorited=true",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(fav["items"].as_array().unwrap().len(), 0);

    let series_rating = |v: &str| {
        let (app, cookie) = (app.clone(), cookie.clone());
        let path = format!("/api/series/{series_id}/rating");
        let body = format!("{{\"value\":{v}}}");
        async move { send(&app, "PUT", &path, Some(&cookie), Some(&body)).await }
    };
    assert_eq!(series_rating("9").await.status(), StatusCode::OK);
    let sd_r = json_of(
        send(
            &app,
            "GET",
            &format!("/api/series/{series_id}"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(sd_r["rating"].as_i64(), Some(9), "series rating on detail");
    let by_rating =
        json_of(send(&app, "GET", "/api/items?sort=rating", Some(&cookie), None).await).await;
    let top = &by_rating["items"][0];
    assert_eq!(top["type"], "series");
    assert_eq!(top["id"].as_i64(), Some(series_id));
    assert_eq!(
        top["rating"].as_i64(),
        Some(9),
        "series rating on the sorted card"
    );
    for bad in ["0", "11"] {
        assert_eq!(series_rating(bad).await.status(), StatusCode::BAD_REQUEST);
    }
    assert_eq!(
        send(
            &app,
            "PUT",
            "/api/series/999999/rating",
            Some(&cookie),
            Some("{\"value\":5}")
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
    );
    assert_eq!(
        send(
            &app,
            "DELETE",
            &format!("/api/series/{series_id}/rating"),
            Some(&cookie),
            None
        )
        .await
        .status(),
        StatusCode::OK,
    );
    let sd_r = json_of(
        send(
            &app,
            "GET",
            &format!("/api/series/{series_id}"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert!(
        sd_r.get("rating").is_none_or(|r| r.is_null()),
        "cleared → no rating"
    );

    let resp = send(
        &app,
        "POST",
        "/api/series/999999/favorite",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let resp = send(
        &app,
        "POST",
        &format!("/api/series/{series_id}/scrape?plugin=nope"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let resp = send(
        &app,
        "POST",
        &format!("/api/series/{series_id}/scrape"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "plugin is required");

    let resp = send(&app, "GET", "/api/series/999999", Some(&cookie), None).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

async fn app_with_epub() -> (Router, String, tempfile::TempDir, tempfile::TempDir) {
    let content = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(content.path().join("books")).unwrap();
    write_epub(&content.path().join("books").join("book.epub"));

    let db = db::connect(data.path()).await.unwrap();
    scanner::scan(&db.write, content.path()).await.unwrap();

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
            64,
        )),
        for_you: Arc::new(arcagrad::intelligence::recommend::RecommendationCache::new(
            64,
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
    let resp = send(&router, "POST", "/api/auth/setup", None, Some(CREDS)).await;
    assert_eq!(resp.status(), StatusCode::OK, "admin setup failed");
    let cookie = cookie_of(&resp);
    (router, cookie, content, data)
}

async fn app_with_chaptered() -> (Router, String, tempfile::TempDir, tempfile::TempDir) {
    let content = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(content.path().join("webtoon")).unwrap();
    write_chaptered_cbz(&content.path().join("webtoon").join("series.cbz"), 3, 4, 5);

    let db = db::connect(data.path()).await.unwrap();
    scanner::scan(&db.write, content.path()).await.unwrap();

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
            64,
        )),
        for_you: Arc::new(arcagrad::intelligence::recommend::RecommendationCache::new(
            64,
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
    let resp = send(&router, "POST", "/api/auth/setup", None, Some(CREDS)).await;
    assert_eq!(resp.status(), StatusCode::OK, "admin setup failed");
    let cookie = cookie_of(&resp);
    (router, cookie, content, data)
}

#[tokio::test]
async fn chaptered_archive_exposes_chapters() {
    let (app, cookie, _c, _d) = app_with_chaptered().await;
    let detail = json_of(send(&app, "GET", "/api/items/1", Some(&cookie), None).await).await;
    assert_eq!(detail["modality"], "paginated");
    assert_eq!(
        detail["reader"], "paginated-chapters",
        "a standalone chaptered archive dispatches to the chapter-nav shell"
    );
    assert_eq!(detail["page_count"], 23);

    let chapters = detail["chapters"].as_array().unwrap();
    assert_eq!(chapters.len(), 5, "front matter + 4 chapters");
    assert!(chapters[0]["number"].is_null());
    assert_eq!(chapters[0]["title"], "Front matter");
    assert_eq!(chapters[0]["start_page"], 0);
    assert_eq!(chapters[0]["page_count"], 3);
    assert_eq!(chapters[1]["number"], "Ch. 1");
    assert_eq!(chapters[1]["start_page"], 3);
    assert_eq!(chapters[1]["page_count"], 5);
    assert_eq!(chapters[4]["number"], "Ch. 4");
    assert_eq!(chapters[4]["start_page"], 18);
}

#[tokio::test]
async fn flat_archive_reports_no_chapters() {
    let (app, cookie, _c, _d) = app_with_auth(1).await;
    let detail = json_of(send(&app, "GET", "/api/items/1", Some(&cookie), None).await).await;
    assert_eq!(detail["reader"], "paginated");
    assert_eq!(detail["chapters"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn epub_detail_is_reflowable_with_no_page_count() {
    let (app, cookie, _c, _d) = app_with_epub().await;
    let detail = json_of(send(&app, "GET", "/api/items/1", Some(&cookie), None).await).await;
    assert_eq!(
        detail["modality"], "reflowable",
        "reader dispatches on this"
    );
    assert_eq!(detail["kind"], "books", "kind is the folder");
    assert_eq!(detail["name"], "A Real Book", "OPF title, not the filename");
    assert_eq!(detail["page_count"], 0);
}

#[tokio::test]
async fn epub_manifest_lists_reading_order() {
    let (app, cookie, _c, _d) = app_with_epub().await;
    let resp = send(&app, "GET", "/api/items/1/manifest", Some(&cookie), None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("application/json"));
    let m = json_of(resp).await;
    assert_eq!(m["metadata"]["title"], "A Real Book");
    assert_eq!(m["metadata"]["author"][0], "Jane Author");
    assert_eq!(m["metadata"]["language"], "en");
    let ro = m["readingOrder"].as_array().unwrap();
    assert_eq!(ro.len(), 2);
    let h0 = ro[0]["href"].as_str().unwrap();
    assert!(
        h0.starts_with("/api/items/1/resource/@v/"),
        "versioned: {h0}"
    );
    assert!(
        h0.ends_with("/OEBPS/text/ch1.xhtml"),
        "entry preserved: {h0}"
    );
    assert_eq!(ro[0]["type"], "application/xhtml+xml");
    assert!(ro[1]["href"]
        .as_str()
        .unwrap()
        .ends_with("/OEBPS/text/ch2.xhtml"));
    let links = m["links"].as_array().unwrap();
    assert!(links.iter().any(|l| l["rel"] == "cover"
        && l["href"].as_str().is_some_and(
            |h| h.starts_with("/api/items/1/resource/@v/") && h.ends_with("/OEBPS/cover.jpg")
        )));
    let toc = m["toc"].as_array().unwrap();
    assert_eq!(toc.len(), 2);
    assert_eq!(toc[0]["title"], "Chapter One");
    assert_eq!(toc[0]["level"], 0);
    assert!(toc[0]["href"]
        .as_str()
        .unwrap()
        .ends_with("/OEBPS/text/ch1.xhtml"));
    assert_eq!(toc[1]["title"], "Chapter Two");
}

#[tokio::test]
async fn epub_resource_serves_entries_and_404s_the_rest() {
    let (app, cookie, _c, _d) = app_with_epub().await;

    let resp = send(
        &app,
        "GET",
        "/api/items/1/resource/OEBPS/text/ch1.xhtml",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "application/xhtml+xml"
    );
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"<html><body>Chapter One</body></html>");

    let resp = send(
        &app,
        "GET",
        "/api/items/1/resource/@v/anyhash/OEBPS/text/ch1.xhtml",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get("cache-control").unwrap(),
        "private, max-age=31536000, immutable"
    );
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"<html><body>Chapter One</body></html>");

    let resp = send(
        &app,
        "GET",
        "/api/items/1/resource/../../etc/passwd",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn manifest_and_resource_are_reflowable_only() {
    let (app, cookie, _c, _d) = app_with_auth(1).await;
    for uri in [
        "/api/items/1/manifest",
        "/api/items/1/resource/whatever.xhtml",
        "/api/items/999999/manifest",
    ] {
        let resp = send(&app, "GET", uri, Some(&cookie), None).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{uri}");
    }
}

#[tokio::test]
async fn epub_progress_saves_and_restores_the_locator() {
    let (app, cookie, _c, _d) = app_with_epub().await;

    let body = r#"{"value":0.5,"locator":{"spine_index":1,"progression":0.3}}"#;
    let resp = send(
        &app,
        "PUT",
        "/api/items/1/progress",
        Some(&cookie),
        Some(body),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "reflowable progress saves");

    let d = json_of(send(&app, "GET", "/api/items/1", Some(&cookie), None).await).await;
    assert_eq!(d["progress_locator"]["spine_index"], 1);
    assert_eq!(d["progress_locator"]["progression"], 0.3);
    assert_eq!(d["progress_value"], 0.5, "detail exposes overall progress");
    assert!(
        d["last_read_at"].as_i64().is_some_and(|t| t > 0),
        "detail exposes when reflowable progress was last saved"
    );
    assert!(
        d["progress"].is_null(),
        "a reflowable item has no page progress"
    );

    let body = r#"{"value":0.9,"locator":{"spine_index":4}}"#;
    let resp = send(
        &app,
        "PUT",
        "/api/items/1/progress",
        Some(&cookie),
        Some(body),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let d = json_of(send(&app, "GET", "/api/items/1", Some(&cookie), None).await).await;
    assert_eq!(d["progress_locator"]["spine_index"], 4);
    assert_eq!(d["progress_value"], 0.9);

    let resp = send(
        &app,
        "PUT",
        "/api/items/1/progress",
        Some(&cookie),
        Some(r#"{"locator":{"spine_index":5}}"#),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let d = json_of(send(&app, "GET", "/api/items/1", Some(&cookie), None).await).await;
    assert_eq!(
        d["progress_value"], 0.9,
        "invalid save leaves progress intact"
    );

    let resp = send(
        &app,
        "PUT",
        "/api/items/1/progress",
        Some(&cookie),
        Some(r#"{"value":1.5}"#),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn versioned_image_url_gets_immutable_cache() {
    let (app, cookie, _c, _d) = app_with_auth(1).await;

    let d = json_of(send(&app, "GET", "/api/items/1", Some(&cookie), None).await).await;
    let v = d["version"].as_str().unwrap();
    assert!(!v.is_empty(), "detail exposes a content version");

    let bare = send(&app, "GET", "/api/items/1/pages/0", Some(&cookie), None).await;
    assert_eq!(bare.status(), StatusCode::OK);
    assert_eq!(
        bare.headers().get("cache-control").unwrap(),
        "private, no-cache"
    );

    let versioned = send(
        &app,
        "GET",
        &format!("/api/items/1/pages/0?v={v}"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(versioned.status(), StatusCode::OK);
    assert_eq!(
        versioned.headers().get("cache-control").unwrap(),
        "private, max-age=31536000, immutable"
    );
    let bogus = send(
        &app,
        "GET",
        "/api/items/1/pages/0?v=deadbeef",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(bogus.status(), StatusCode::OK);
    let body = bogus.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"dummy-jpeg-c0-1");
}

#[tokio::test]
async fn versioned_thumbnail_served_from_cache_fast_path() {
    let (app, cookie, _c, data) = app_with_auth(1).await;
    let d = json_of(send(&app, "GET", "/api/items/1", Some(&cookie), None).await).await;
    let hash = d["version"].as_str().unwrap().to_string();

    let cover = arcagrad::media::thumbnail::cache_path(data.path(), &hash);
    std::fs::create_dir_all(cover.parent().unwrap()).unwrap();
    std::fs::write(&cover, b"COVER-BYTES").unwrap();
    let page = arcagrad::media::thumbnail::page_cache_path(data.path(), &hash, 0);
    std::fs::create_dir_all(page.parent().unwrap()).unwrap();
    std::fs::write(&page, b"PAGE-THUMB-BYTES").unwrap();

    let resp = send(
        &app,
        "GET",
        &format!("/api/items/1/thumbnail?v={hash}"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get("cache-control").unwrap(),
        "private, max-age=31536000, immutable"
    );
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        &body[..],
        b"COVER-BYTES",
        "served the cached bytes verbatim"
    );

    let resp = send(
        &app,
        "GET",
        &format!("/api/items/1/pages/0/thumbnail?v={hash}"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"PAGE-THUMB-BYTES");
}

#[tokio::test]
async fn listing_and_continue_expose_cover_version() {
    let (app, cookie, _c, _d) = app_with_auth(2).await;

    let list = json_of(send(&app, "GET", "/api/items", Some(&cookie), None).await).await;
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    for card in items {
        assert_eq!(card["type"], "item");
        assert!(
            card["cover_version"]
                .as_str()
                .is_some_and(|v| !v.is_empty()),
            "browse card carries a cover_version: {card}"
        );
    }

    let put = send(
        &app,
        "PUT",
        "/api/items/1/progress",
        Some(&cookie),
        Some(r#"{"page":0}"#),
    )
    .await;
    assert_eq!(put.status(), StatusCode::OK);
    let cont = json_of(send(&app, "GET", "/api/items/continue", Some(&cookie), None).await).await;
    let entries = cont["items"].as_array().unwrap();
    assert!(!entries.is_empty());
    assert!(entries[0]["cover_version"]
        .as_str()
        .is_some_and(|v| !v.is_empty()));
}

#[tokio::test]
async fn paginated_progress_still_requires_a_page() {
    let (app, cookie, _c, _d) = app_with_auth(1).await;
    let ok = send(
        &app,
        "PUT",
        "/api/items/1/progress",
        Some(&cookie),
        Some(r#"{"page":3}"#),
    )
    .await;
    assert_eq!(ok.status(), StatusCode::OK);

    let bad = send(
        &app,
        "PUT",
        "/api/items/1/progress",
        Some(&cookie),
        Some(r#"{"value":0.5}"#),
    )
    .await;
    assert_eq!(
        bad.status(),
        StatusCode::BAD_REQUEST,
        "paginated needs a page"
    );
}

#[tokio::test]
async fn follow_flow_baseline_discovery_and_review() {
    let (state, _c, _d) = build_state(2).await;
    let app = routes::router(state.clone());
    let resp = send(&app, "POST", "/api/auth/setup", None, Some(CREDS)).await;
    let cookie = cookie_of(&resp);

    let body = r#"{"plugin_id":"example","kind":"comics","feed":"popular","query":"artist:x"}"#;
    let resp = send(&app, "POST", "/api/follows", Some(&cookie), Some(body)).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "kind gate enforced");

    let resp = send(
        &app,
        "PUT",
        "/api/kinds/comics/plugins",
        Some(&cookie),
        Some(r#"{"plugin_ids":["example"],"auto":[]}"#),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = send(
        &app,
        "POST",
        "/api/follows",
        Some(&cookie),
        Some(r#"{"plugin_id":"example","kind":"comics","feed":"popular","query":"  "}"#),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "empty query refused"
    );
    let resp = send(
        &app,
        "POST",
        "/api/follows",
        Some(&cookie),
        Some(r#"{"plugin_id":"example","kind":"comics","feed":"nope","query":"artist:x"}"#),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "unknown feed refused"
    );

    let resp = send(&app, "POST", "/api/follows", Some(&cookie), Some(body)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let j = json_of(resp).await;
    let id = j["id"].as_i64().unwrap();
    assert_eq!(j["created"], true);
    let resp = send(&app, "POST", "/api/follows", Some(&cookie), Some(body)).await;
    let j = json_of(resp).await;
    assert_eq!(j["created"], false, "re-following is idempotent");
    assert_eq!(j["id"].as_i64().unwrap(), id);

    drain_jobs(&state).await;
    let j = json_of(send(&app, "GET", "/api/follows", Some(&cookie), None).await).await;
    let w = &j.as_array().unwrap()[0];
    assert_eq!(w["new_count"], 0, "baseline surfaces nothing");
    assert!(w["last_checked_at"].as_i64().is_some(), "baseline stamped");
    let items = json_of(
        send(
            &app,
            "GET",
            &format!("/api/follows/{id}/items"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(
        items.as_array().unwrap().len(),
        0,
        "no reviewable items yet"
    );

    sqlx::query("DELETE FROM follow_seen WHERE follow_id = ?")
        .bind(id)
        .execute(&state.write)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO follow_seen (follow_id, reference, state, seen_at) VALUES (?, 'ex-0', 'seen', 0)",
    )
    .bind(id)
    .execute(&state.write)
    .await
    .unwrap();
    let item_id: i64 = sqlx::query_scalar("SELECT id FROM items LIMIT 1")
        .fetch_one(&state.read)
        .await
        .unwrap();
    sqlx::query("INSERT INTO item_sources (item_id, source, url) VALUES (?, 'example', 'https://example.test/g/ex-1')")
        .bind(item_id)
        .execute(&state.write)
        .await
        .unwrap();

    let resp = send(
        &app,
        "POST",
        "/api/follows/check",
        Some(&cookie),
        Some(&format!(r#"{{"follows":[{id}]}}"#)),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    drain_jobs(&state).await;

    let items = json_of(
        send(
            &app,
            "GET",
            &format!("/api/follows/{id}/items"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    let arr = items.as_array().unwrap();
    assert_eq!(arr.len(), 2, "both refs surfaced as discoveries");
    let by_ref = |r: &str| {
        arr.iter()
            .find(|i| i["reference"] == r)
            .cloned()
            .unwrap_or_else(|| panic!("missing {r}"))
    };
    assert_eq!(by_ref("ex-1")["state"], "owned", "source_url match → owned");
    assert_eq!(by_ref("ex-2")["state"], "new");
    assert!(
        by_ref("ex-2")["item"]["title"].as_str().is_some(),
        "card snapshot renders without re-hitting the source"
    );
    let j = json_of(send(&app, "GET", "/api/follows", Some(&cookie), None).await).await;
    assert_eq!(j[0]["new_count"], 1, "only 'new' feeds the badge");

    let resp = send(
        &app,
        "POST",
        &format!("/api/follows/{id}/items/state"),
        Some(&cookie),
        Some(r#"{"reference":"ex-2","state":"skipped"}"#),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = send(
        &app,
        "POST",
        &format!("/api/follows/{id}/items/state"),
        Some(&cookie),
        Some(r#"{"reference":"ex-2","state":"new"}"#),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "undo back to new");
    let resp = send(
        &app,
        "POST",
        &format!("/api/follows/{id}/items/state"),
        Some(&cookie),
        Some(r#"{"reference":"ex-2","state":"owned"}"#),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "'owned' is not client-settable"
    );
    let j = json_of(
        send(
            &app,
            "POST",
            &format!("/api/follows/{id}/items/dismiss-all"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(j["moved"], 1);
    let j = json_of(send(&app, "GET", "/api/follows", Some(&cookie), None).await).await;
    assert_eq!(j[0]["new_count"], 0);

    let resp = send(&app, "GET", "/api/follows", None, None).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let resp = send(
        &app,
        "DELETE",
        &format!("/api/follows/{id}"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = send(
        &app,
        "GET",
        &format!("/api/follows/{id}/items"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "gone after delete");
}

#[tokio::test]
async fn upload_enqueues_thumbnail_sweep_for_phash() {
    let (state, content, _d) = build_state(0).await;
    let app = routes::router(state.clone());
    let setup = send(&app, "POST", "/api/auth/setup", None, Some(CREDS)).await;
    let cookie = cookie_of(&setup);
    let _ = &content;

    let cbz = cbz_bytes("phash1");
    let (ct, body) = multipart_body(&[
        ("kind", None, b"manga".as_slice()),
        ("file", Some("Cover Test.cbz"), &cbz),
    ]);
    let resp = send_raw(&app, "POST", "/api/items", Some(&cookie), &ct, body).await;
    assert_eq!(resp.status(), StatusCode::CREATED);

    let sweeps: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM jobs WHERE kind = 'thumbnail_sweep' AND state = 'pending'",
    )
    .fetch_one(&state.read)
    .await
    .unwrap();
    assert_eq!(sweeps, 1, "upload must enqueue a cover+phash sweep");
}

#[tokio::test]
async fn search_matches_series_own_title() {
    let content = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let s = content.path().join("books").join("Monogatari Series");
    std::fs::create_dir_all(&s).unwrap();
    write_cbz(&s.join("BAKEMONOGATARI Part 1.cbz"), "v1");
    write_cbz(&s.join("BAKEMONOGATARI Part 2.cbz"), "v2");

    let db = db::connect(data.path()).await.unwrap();
    scanner::scan(&db.write, content.path()).await.unwrap();
    let config = Arc::new(Config {
        content_dir: content.path().to_path_buf(),
        data_dir: data.path().to_path_buf(),
        bind: "0.0.0.0:0".into(),
        cookie_secure: false,
        read_concurrency: 8,
        allow_private_repos: false,
        watch: false,
    });
    let app = routes::router(AppState {
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
            64,
        )),
        for_you: Arc::new(arcagrad::intelligence::recommend::RecommendationCache::new(
            64,
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
    let resp = send(&app, "POST", "/api/auth/setup", None, Some(CREDS)).await;
    let cookie = cookie_of(&resp);

    let list =
        json_of(send(&app, "GET", "/api/items?q=monogatari", Some(&cookie), None).await).await;
    let cards = list["items"].as_array().unwrap();
    assert_eq!(cards.len(), 1, "own-title match surfaces the series card");
    assert_eq!(cards[0]["type"], "series");
    assert_eq!(cards[0]["name"], "Monogatari Series");

    let list = json_of(
        send(
            &app,
            "GET",
            "/api/items?q=bakemonogatari",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(
        list["items"].as_array().unwrap().len(),
        1,
        "leaf-title match keeps surfacing the series"
    );

    let list = json_of(
        send(
            &app,
            "GET",
            "/api/items?q=monogatari&page=1",
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(list["total"].as_i64(), Some(1), "count path total");

    let list =
        json_of(send(&app, "GET", "/api/items?q=zzznothing", Some(&cookie), None).await).await;
    assert!(list["items"].as_array().unwrap().is_empty());

    let sug = json_of(send(&app, "GET", "/api/suggest?q=monoga", Some(&cookie), None).await).await;
    let results = sug["results"].as_array().unwrap();
    let series_row = results
        .iter()
        .find(|r| r["type"] == "series")
        .expect("a series suggestion");
    assert_eq!(series_row["title"], "Monogatari Series");
    assert!(series_row["cover_item_id"].as_i64().is_some());
}

#[tokio::test]
async fn edit_item_metadata_title_description_modality() {
    let (state, _content, _data) = build_state(2).await;
    let app = routes::router(state.clone());
    let resp = send(&app, "POST", "/api/auth/setup", None, Some(CREDS)).await;
    let cookie = cookie_of(&resp);

    let r = send(&app, "PUT", "/api/items/1/metadata", None, Some("{}")).await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    let r = send(
        &app,
        "PUT",
        "/api/items/999/metadata",
        Some(&cookie),
        Some(r#"{"title":"x"}"#),
    )
    .await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);

    for bad in [r#"{"title":"  "}"#, r#"{"modality_override":"scroll"}"#] {
        let r = send(
            &app,
            "PUT",
            "/api/items/1/metadata",
            Some(&cookie),
            Some(bad),
        )
        .await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST, "body {bad}");
    }

    let r = send(
        &app,
        "PUT",
        "/api/items/1/metadata",
        Some(&cookie),
        Some(r#"{"title":"Renamed Classic","description":"My synopsis","modality_override":"reflowable"}"#),
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);
    let d = json_of(r).await;
    assert_eq!(d["name"], "Renamed Classic");
    assert_eq!(d["description"], "My synopsis");
    assert_eq!(d["modality"], "reflowable", "override wins over detected");

    let hits = json_of(send(&app, "GET", "/api/items?q=renamed", Some(&cookie), None).await).await;
    assert_eq!(hits["items"].as_array().unwrap().len(), 1);
    let hits = json_of(send(&app, "GET", "/api/items?q=book-0", Some(&cookie), None).await).await;
    assert!(
        hits["items"].as_array().unwrap().is_empty(),
        "old title must stop matching"
    );
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM jobs WHERE kind = 'reindex_search' AND state = 'pending'",
    )
    .fetch_one(&state.read)
    .await
    .unwrap();
    assert!(pending >= 1, "title edit must queue a search reindex");

    arcagrad::repo::set_item_description(&state.write, 1, "scraped synopsis", Some("someplugin"))
        .await
        .unwrap();
    let d = json_of(send(&app, "GET", "/api/items/1", Some(&cookie), None).await).await;
    assert_eq!(
        d["description"], "My synopsis",
        "scrape must not clobber a manual edit"
    );

    let r = send(
        &app,
        "PUT",
        "/api/items/1/metadata",
        Some(&cookie),
        Some(r#"{"description":null}"#),
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);
    arcagrad::repo::set_item_description(&state.write, 1, "scraped synopsis", Some("someplugin"))
        .await
        .unwrap();
    let d = json_of(send(&app, "GET", "/api/items/1", Some(&cookie), None).await).await;
    assert_eq!(
        d["description"], "scraped synopsis",
        "cleared → scrape may fill again"
    );
    assert_eq!(
        d["description_source"], "someplugin",
        "scrape write stamps provenance"
    );

    let r = send(
        &app,
        "PUT",
        "/api/items/1/metadata",
        Some(&cookie),
        Some(r#"{"modality_override":null}"#),
    )
    .await;
    let d = json_of(r).await;
    assert_eq!(d["modality"], "paginated", "null reverts to detected");

    let d = json_of(send(&app, "GET", "/api/items/1", Some(&cookie), None).await).await;
    assert_eq!(d["name"], "Renamed Classic");
}

#[tokio::test]
async fn forget_source_removes_url_tags_and_comments() {
    let (state, _content, _data) = build_state(1).await;
    let app = routes::router(state.clone());
    let resp = send(&app, "POST", "/api/auth/setup", None, Some(CREDS)).await;
    let cookie = cookie_of(&resp);

    use arcagrad::repo;
    repo::set_item_source(&state.write, 1, "wrongplugin", "https://wrong.example/g/1")
        .await
        .unwrap();
    for (value, source) in [("bogus one", "wrongplugin"), ("bogus two", "wrongplugin")] {
        let tid = repo::get_or_create_tag(&state.write, "tag", value)
            .await
            .unwrap();
        repo::add_item_tag(&state.write, 1, tid, "none", source)
            .await
            .unwrap();
    }
    let keep_manual = repo::get_or_create_tag(&state.write, "tag", "keep manual")
        .await
        .unwrap();
    repo::add_item_tag(&state.write, 1, keep_manual, "none", "manual")
        .await
        .unwrap();
    let keep_other = repo::get_or_create_tag(&state.write, "creator", "keep artist")
        .await
        .unwrap();
    repo::add_item_tag(&state.write, 1, keep_other, "none", "rightplugin")
        .await
        .unwrap();
    repo::set_item_description(&state.write, 1, "wrong synopsis", Some("wrongplugin"))
        .await
        .unwrap();
    repo::replace_item_comments(
        &state.write,
        1,
        "wrongplugin",
        &[arcagrad::repo::ItemComment {
            source: "wrongplugin".into(),
            external_id: "c1".into(),
            author: "someone".into(),
            posted_at: Some(1),
            score: None,
            body: "a mirrored comment".into(),
        }],
    )
    .await
    .unwrap();
    repo::reindex_item_tags(&state.write, 1).await.unwrap();

    let r = send(
        &app,
        "DELETE",
        "/api/items/1/sources/wrongplugin",
        None,
        None,
    )
    .await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    let r = send(
        &app,
        "DELETE",
        "/api/items/1/sources/nosuch",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);

    let r = send(
        &app,
        "DELETE",
        "/api/items/1/sources/wrongplugin",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);
    let d = json_of(r).await;
    let tags: Vec<String> = d["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["value"].as_str().unwrap().to_string())
        .collect();
    assert!(
        !tags.iter().any(|t| t.starts_with("bogus")),
        "source tags removed: {tags:?}"
    );
    assert!(
        tags.contains(&"keep manual".to_string()),
        "manual tag survives"
    );
    assert!(
        tags.contains(&"keep artist".to_string()),
        "other source's tag survives"
    );
    assert!(
        d["sources"].as_array().unwrap().is_empty(),
        "source URL forgotten"
    );
    assert!(
        d["comments"].as_array().is_none_or(|c| c.is_empty()),
        "comments cleared"
    );
    assert!(
        d["description"].is_null(),
        "the forgotten source's description is cleared"
    );

    repo::set_item_source(&state.write, 1, "wrongplugin", "https://wrong.example/g/1")
        .await
        .unwrap();
    repo::set_item_description(&state.write, 1, "right synopsis", Some("rightplugin"))
        .await
        .unwrap();
    let r = send(
        &app,
        "DELETE",
        "/api/items/1/sources/wrongplugin",
        Some(&cookie),
        None,
    )
    .await;
    let d = json_of(r).await;
    assert_eq!(
        d["description"], "right synopsis",
        "another source's description survives"
    );

    let r = send(
        &app,
        "DELETE",
        "/api/items/1/sources/wrongplugin",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);

    let hits = json_of(send(&app, "GET", "/api/items?q=bogus", Some(&cookie), None).await).await;
    assert!(hits["items"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn edit_series_metadata_and_forget_source() {
    let (state, content, _data) = build_state(0).await;
    let s = content.path().join("books").join("My Saga");
    std::fs::create_dir_all(&s).unwrap();
    write_cbz(&s.join("My Saga v01.cbz"), "v1");
    write_cbz(&s.join("My Saga v02.cbz"), "v2");
    scanner::scan(&state.write, content.path()).await.unwrap();
    let app = routes::router(state.clone());
    let resp = send(&app, "POST", "/api/auth/setup", None, Some(CREDS)).await;
    let cookie = cookie_of(&resp);

    use arcagrad::repo;
    let sid: i64 = sqlx::query_scalar("SELECT id FROM series")
        .fetch_one(&state.read)
        .await
        .unwrap();
    let leaf: i64 =
        sqlx::query_scalar("SELECT item_id FROM item_series_leaf ORDER BY number_sort LIMIT 1")
            .fetch_one(&state.read)
            .await
            .unwrap();

    repo::set_series_source(
        &state.write,
        sid,
        "wrongplugin",
        "https://wrong.example/s/1",
    )
    .await
    .unwrap();
    let bogus = repo::get_or_create_tag(&state.write, "tag", "bogus series tag")
        .await
        .unwrap();
    repo::add_series_tag(&state.write, sid, bogus, "none", "wrongplugin")
        .await
        .unwrap();
    let shared = repo::get_or_create_tag(&state.write, "tag", "leafshared")
        .await
        .unwrap();
    repo::add_series_tag(&state.write, sid, shared, "none", "wrongplugin")
        .await
        .unwrap();
    repo::add_item_tag(&state.write, leaf, shared, "none", "wrongplugin")
        .await
        .unwrap();

    let d = json_of(
        send(
            &app,
            "GET",
            &format!("/api/series/{sid}"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    let tag_leaf = |d: &serde_json::Value, value: &str| {
        d["tags"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["value"] == value)
            .unwrap_or_else(|| panic!("tag {value} missing"))["leaf"]
            .as_bool()
            .unwrap()
    };
    assert!(!tag_leaf(&d, "bogus series tag"), "series-only tag");
    assert!(tag_leaf(&d, "leafshared"), "leaf-backed tag");

    let r = send(&app, "PUT", "/api/series/999/metadata", None, Some("{}")).await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    let r = send(
        &app,
        "PUT",
        "/api/series/999/metadata",
        Some(&cookie),
        Some(r#"{"title":"x"}"#),
    )
    .await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    let r = send(
        &app,
        "PUT",
        &format!("/api/series/{sid}/metadata"),
        Some(&cookie),
        Some(r#"{"title":"  "}"#),
    )
    .await;
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);

    let r = send(
        &app,
        "PUT",
        &format!("/api/series/{sid}/metadata"),
        Some(&cookie),
        Some(r#"{"title":"Renamed Saga","description":"Series synopsis"}"#),
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);
    let d = json_of(r).await;
    assert_eq!(d["title"], "Renamed Saga");
    assert_eq!(d["description"], "Series synopsis");
    assert_eq!(d["description_manual"], true);

    scanner::scan(&state.write, content.path()).await.unwrap();
    let d = json_of(
        send(
            &app,
            "GET",
            &format!("/api/series/{sid}"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(
        d["title"], "Renamed Saga",
        "re-scan must not revert the title"
    );

    let hits = json_of(send(&app, "GET", "/api/items?q=renamed", Some(&cookie), None).await).await;
    assert_eq!(hits["items"].as_array().unwrap().len(), 1);
    assert_eq!(hits["items"][0]["type"], "series");

    repo::set_series_description(&state.write, sid, "scraped synopsis", Some("wrongplugin"))
        .await
        .unwrap();
    let d = json_of(
        send(
            &app,
            "GET",
            &format!("/api/series/{sid}"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(
        d["description"], "Series synopsis",
        "scrape must not clobber a manual edit"
    );
    let r = send(
        &app,
        "PUT",
        &format!("/api/series/{sid}/metadata"),
        Some(&cookie),
        Some(r#"{"description":null}"#),
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);
    repo::set_series_description(&state.write, sid, "scraped synopsis", Some("wrongplugin"))
        .await
        .unwrap();
    let d = json_of(
        send(
            &app,
            "GET",
            &format!("/api/series/{sid}"),
            Some(&cookie),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(
        d["description"], "scraped synopsis",
        "cleared → scrape may fill again"
    );
    assert_eq!(
        d["description_source"], "wrongplugin",
        "series scrape write stamps provenance"
    );

    let r = send(
        &app,
        "DELETE",
        &format!("/api/series/{sid}/sources/nosuch"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    let r = send(
        &app,
        "DELETE",
        &format!("/api/series/{sid}/sources/wrongplugin"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);
    let d = json_of(r).await;
    let tags: Vec<String> = d["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["value"].as_str().unwrap().to_string())
        .collect();
    assert!(
        !tags.contains(&"bogus series tag".to_string()),
        "series-level source tag removed: {tags:?}"
    );
    assert!(
        tags.contains(&"leafshared".to_string()),
        "leaf-backed tag survives a series forget"
    );
    assert!(
        d["sources"].as_array().unwrap().is_empty(),
        "series source URL forgotten"
    );
    assert!(
        d["description"].is_null(),
        "the forgotten source's series description is cleared"
    );

    let r = send(
        &app,
        "DELETE",
        &format!("/api/series/{sid}/sources/wrongplugin"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}
