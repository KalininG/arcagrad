//! Source browsing, previews, ownership matching, and downloads.

use super::*;

/// Query for `GET /api/plugins/{id}/browse`.
#[derive(Deserialize)]
pub(crate) struct BrowseQuery {
    /// A `Feed.id` the plugin declared (e.g. "popular", "recent").
    feed: String,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    range: Option<String>,
    #[serde(default = "one_page")]
    page: u32,
}

pub(crate) fn one_page() -> u32 {
    1
}

/// Serve or populate a cached plugin JSON response.
pub(crate) async fn cached_or_compute<T, F, Fut>(
    cache: &crate::plugins::browse_cache::BrowseCache,
    key: &str,
    ttl: u64,
    compute: F,
) -> Result<Response, AppError>
where
    T: serde::Serialize,
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, AppError>>,
{
    if let Some(bytes) = cache.get(key, std::time::Duration::from_secs(ttl)).await {
        return Ok(browse_cache_response(bytes, ttl));
    }
    let value = compute().await?;
    let bytes = serde_json::to_vec(&value).map_err(|e| AppError::Internal(e.into()))?;
    if ttl > 0 {
        cache.put(key, &bytes).await;
    }
    Ok(browse_cache_response(bytes, ttl))
}

/// Fetch one page from a plugin feed. Requires the `browse` capability and does
/// not modify the library.
#[utoipa::path(
    get, path = "/api/plugins/{id}/browse", tag = "browse",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(
        ("id" = String, Path, description = "Plugin id"),
        ("feed" = String, Query, description = "Feed id (a Feed.id the plugin declares)"),
        ("query" = Option<String>, Query, description = "Free-text filter, if the feed accepts one"),
        ("range" = Option<String>, Query, description = "Time range (one of the feed's ranges)"),
        ("page" = Option<u32>, Query, description = "1-based page number"),
    ),
    responses(
        (status = 200, description = "A page of browse results", body = crate::plugins::scraper::BrowsePage),
        (status = 400, description = "Unknown plugin, or it doesn't support browse"),
        (status = 401, description = "Not authenticated"),
        (status = 502, description = "The source/plugin failed (network, rate limit, parse)"),
    ),
)]
pub(crate) async fn plugin_browse(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<BrowseQuery>,
) -> Result<Response, AppError> {
    let (scraper, manifest) = resolve_plugin(&state, &id, "browse")?;
    let req = crate::plugins::scraper::BrowseRequest {
        feed: q.feed,
        query: q.query.filter(|s| !s.is_empty()),
        range: q.range.filter(|s| !s.is_empty()),
        page: q.page.max(1),
    };
    let ttl = manifest
        .feeds
        .iter()
        .find(|f| f.id == req.feed)
        .map(|f| f.cache_ttl)
        .unwrap_or(0);
    let cache = crate::plugins::browse_cache::BrowseCache::new(&state.config.data_dir);
    let key = format!(
        "browse\0{id}\0{}\0{}\0{}\0{}",
        req.feed,
        req.range.as_deref().unwrap_or(""),
        req.query.as_deref().unwrap_or(""),
        req.page
    );
    cached_or_compute(&cache, &key, ttl, || async {
        let mut page = scraper
            .browse(&req, &*state.fetcher)
            .await
            .map_err(upstream)?;
        if manifest.clean_titles {
            for it in &mut page.items {
                it.title = crate::media::title::clean(&it.title);
            }
        }
        Ok(page)
    })
    .await
}

/// Build a JSON response whose browser cache policy matches the plugin TTL.
pub(crate) fn browse_cache_response(bytes: Vec<u8>, ttl: u64) -> Response {
    let cache_control = if ttl > 0 {
        format!("private, max-age={ttl}")
    } else {
        "private, no-store".to_string()
    };
    (
        [
            (header::CONTENT_TYPE, "application/json".to_string()),
            (header::CACHE_CONTROL, cache_control),
        ],
        bytes,
    )
        .into_response()
}

/// Query for the browse image proxy.
#[derive(Deserialize)]
pub(crate) struct PluginImageQuery {
    /// The full remote image URL (a source-CDN cover). Must be on a host the plugin
    /// declared in its manifest `hosts`.
    url: String,
}

/// Proxy a plugin cover or thumbnail through the server. The URL must satisfy
/// anti-SSRF checks and the plugin's host allowlist.
#[utoipa::path(
    get, path = "/api/plugins/{id}/image", tag = "browse",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(
        ("id" = String, Path, description = "Plugin id"),
        ("url" = String, Query, description = "Full remote image URL (must be on a host the plugin declared)"),
    ),
    responses(
        (status = 200, description = "The proxied image bytes", content(
            (Vec<u8> = "image/jpeg"),
            (Vec<u8> = "image/png"),
            (Vec<u8> = "image/webp"),
            (Vec<u8> = "image/gif"),
            (Vec<u8> = "image/avif"),
            (Vec<u8> = "image/svg+xml"),
            (Vec<u8> = "application/octet-stream")
        )),
        (status = 400, description = "Unknown plugin, no browse/calendar artwork capability, or a bad url"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "URL host is not in the plugin's allowlist"),
        (status = 404, description = "The upstream image wasn't found"),
    ),
)]
pub(crate) async fn plugin_image(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<PluginImageQuery>,
) -> Result<Response, AppError> {
    let scraper = state
        .scrapers
        .get(&id)
        .ok_or_else(|| AppError::BadRequest(format!("unknown plugin '{id}'")))?;
    let m = scraper.manifest();
    if !m
        .capabilities
        .iter()
        .any(|capability| capability == "browse" || capability == "calendar")
    {
        return Err(AppError::BadRequest(format!(
            "plugin '{id}' does not provide browsable or calendar artwork"
        )));
    }
    let host = crate::plugins::scraper::host_of(&q.url)
        .ok_or_else(|| AppError::BadRequest("invalid url".into()))?;
    if !crate::plugins::wasm_host::host_allowed(&m.hosts, &host) {
        return Err(AppError::Forbidden);
    }
    let mut req = crate::plugins::scraper::FetchRequest::get(&q.url);
    req.headers = m
        .image_headers
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let (status, upstream_ct, body) = state.fetcher.fetch_asset(req).await?;
    if !(200..300).contains(&status) {
        return Err(AppError::NotFound);
    }
    {
        let hash_bytes = body.clone();
        let hash_url = q.url.clone();
        let hash_cache = state.cover_hashes.clone();
        let hash = tokio::task::spawn_blocking(move || {
            let h = crate::media::thumbnail::dhash(&hash_bytes);
            if let Some(h) = h {
                hash_cache.insert(hash_url, h);
            }
            h
        })
        .await
        .ok()
        .flatten();
        if let Some(h) = hash {
            let write = state.write.clone();
            let url = q.url.clone();
            tokio::spawn(async move {
                if let Err(e) = repo::upsert_cover_hash(&write, &url, h).await {
                    tracing::debug!("persist cover hash failed: {e:#}");
                }
            });
        }
    }
    let content_type = upstream_ct
        .filter(|c| c.starts_with("image/"))
        .or_else(|| image_mime_from_url(&q.url))
        .unwrap_or_else(|| "image/jpeg".to_string());
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "private, max-age=86400".to_string()),
        ],
        body,
    )
        .into_response())
}

/// Best-effort image content-type from a URL's path extension (`.jpg`/`.webp`/`.png`).
/// Returns `None` when the extension isn't a recognized image type.
pub(crate) fn image_mime_from_url(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let m = mime_guess::from_path(path).first()?;
    let s = m.essence_str().to_string();
    s.starts_with("image/").then_some(s)
}

/// Query for a browse detail preview: `?ref=<opaque reference>`.
#[derive(Deserialize)]
pub(crate) struct BrowseItemQuery {
    #[serde(rename = "ref")]
    reference: String,
}

/// One namespaced tag on a remote item preview.
#[derive(Serialize, ToSchema)]
pub(crate) struct BrowseTag {
    namespace: String,
    value: String,
    qualifier: String,
}

/// Metadata for a remote item that has not been added to the library.
#[derive(Serialize, ToSchema)]
pub(crate) struct BrowseDetail {
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    /// Scrape-derived synopsis (raw markup; the client sanitizes, like a comment body).
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    /// The canonical source URL, feeding the preview's "Open in browser".
    #[serde(skip_serializing_if = "Option::is_none")]
    source_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    /// Cover URL (full, resolved); proxy it via `…/image`. Makes a deep-linked preview
    /// self-sufficient (no reliance on the browse card's nav state).
    #[serde(skip_serializing_if = "Option::is_none")]
    cover_url: Option<String>,
    /// Page count (the "N pages" badge).
    #[serde(skip_serializing_if = "Option::is_none")]
    page_count: Option<i64>,
    /// Source popularity (favourites badge).
    #[serde(skip_serializing_if = "Option::is_none")]
    favorites: Option<i64>,
    /// Capabilities used to enable reading and download actions in the client.
    capabilities: Vec<String>,
    /// Default reader engine: `paged` or `vertical`.
    reading_mode: String,
    /// Mapped tags (closed-namespace, qualifiered): the metadata rows.
    tags: Vec<BrowseTag>,
    /// Source comments in the same shape as local item comments.
    comments: Vec<ItemComment>,
    /// Remote chapters in local `ChapterInfo` shape. Empty for one-shots.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    chapters: Vec<ChapterInfo>,
}

/// Fetch remote item details without adding the item to the library. Requires the
/// plugin's `browse` capability.
#[utoipa::path(
    get, path = "/api/plugins/{id}/item", tag = "browse",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(
        ("id" = String, Path, description = "Plugin id"),
        ("ref" = String, Query, description = "Opaque source reference (a browse item's `reference`)"),
    ),
    responses(
        (status = 200, description = "The browse item's metadata", body = BrowseDetail),
        (status = 400, description = "Unknown plugin, not a browse plugin, or missing ref"),
        (status = 401, description = "Not authenticated"),
        (status = 502, description = "The source/plugin failed (network, rate limit, parse)"),
    ),
)]
pub(crate) async fn plugin_item(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<BrowseItemQuery>,
) -> Result<Response, AppError> {
    let reference = q.reference;
    if reference.trim().is_empty() {
        return Err(AppError::BadRequest("ref is required".into()));
    }
    let (scraper, manifest) = resolve_plugin(&state, &id, "browse")?;
    let ttl = manifest.item_cache_ttl;
    let cache = crate::plugins::browse_cache::BrowseCache::new(&state.config.data_dir);
    let key = format!("item\0{id}\0{reference}");
    cached_or_compute(&cache, &key, ttl, || async {
        let candidate = crate::plugins::scraper::Candidate {
            id: reference.clone(),
            title: String::new(),
            score: 1.0,
        };
        let meta = scraper
            .fetch_details(&candidate, &*state.fetcher)
            .await
            .map_err(upstream)?;
        let comments = meta
            .comments
            .into_iter()
            .map(|c| {
                let body = crate::server::comments::sanitize_for_display(
                    &c.body,
                    c.score.is_some_and(|s| s < 0),
                );
                ItemComment {
                    source: id.clone(),
                    external_id: c.external_id,
                    author: c.author,
                    posted_at: c.posted_at,
                    score: c.score,
                    body,
                }
            })
            .collect();
        Ok(BrowseDetail {
            title: meta.title.map(|t| {
                if manifest.clean_titles {
                    crate::media::title::clean(&t)
                } else {
                    t
                }
            }),
            description: meta.description,
            source_url: meta.source_url,
            language: meta.language,
            cover_url: meta.cover_url,
            page_count: meta.page_count,
            favorites: meta.favorites,
            capabilities: manifest.capabilities.clone(),
            reading_mode: manifest.reading_mode.clone(),
            tags: meta
                .mapped_tags
                .into_iter()
                .map(|t| BrowseTag {
                    namespace: t.namespace,
                    value: t.value,
                    qualifier: t.qualifier,
                })
                .collect(),
            comments,
            chapters: meta
                .chapters
                .into_iter()
                .map(|c| ChapterInfo {
                    number: c.number,
                    title: c.title,
                    start_page: 0,
                    page_count: c.page_count,
                    reference: c.reference,
                })
                .collect(),
        })
    })
    .await
}

/// Return proxied image and thumbnail URLs for remote reading. Requires the
/// plugin's `read` capability.
#[utoipa::path(
    get, path = "/api/plugins/{id}/pages", tag = "browse",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(
        ("id" = String, Path, description = "Plugin id"),
        ("ref" = String, Query, description = "Opaque source reference (a browse item's `reference`)"),
    ),
    responses(
        (status = 200, description = "The item's ordered page list", body = crate::plugins::scraper::BrowsePages),
        (status = 400, description = "Unknown plugin, plugin does not provide read, or missing ref"),
        (status = 401, description = "Not authenticated"),
        (status = 502, description = "The source/plugin failed (network, rate limit, parse)"),
    ),
)]
pub(crate) async fn plugin_pages(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<BrowseItemQuery>,
) -> Result<Response, AppError> {
    let reference = q.reference;
    if reference.trim().is_empty() {
        return Err(AppError::BadRequest("ref is required".into()));
    }
    let (scraper, manifest) = resolve_plugin(&state, &id, "read")?;
    let ttl = manifest.item_cache_ttl;
    let cache = crate::plugins::browse_cache::BrowseCache::new(&state.config.data_dir);
    let key = format!("pages\0{id}\0{reference}");
    cached_or_compute(&cache, &key, ttl, || async {
        scraper
            .pages(&reference, &*state.fetcher)
            .await
            .map_err(upstream)
    })
    .await
}

#[derive(Deserialize, utoipa::IntoParams)]
pub(crate) struct IdentifyParams {
    plugin: Option<String>,
}

/// A reverse-image candidate and the installed plugin that can scrape it.
#[derive(Serialize, ToSchema)]
pub(crate) struct IdentifyCandidateOut {
    source: String,
    reference: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    similarity: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    page_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    /// Vendor cover URL. The client shows it through the image proxy
    /// (`/api/plugins/{plugin_id}/image?url=`).
    #[serde(skip_serializing_if = "Option::is_none")]
    cover_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    plugin_id: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct IdentifyResponse {
    candidates: Vec<IdentifyCandidateOut>,
}

/// Hash the first page when available for exact reverse-image lookup.
pub(crate) async fn first_page_sha1(state: &AppState, meta: &repo::ItemMeta) -> Option<String> {
    if meta.modality != "paginated" {
        return None;
    }
    let path = std::path::PathBuf::from(&meta.path);
    let list = library::ensure_page_list(state, meta.id, path.clone())
        .await
        .ok()?;
    let name = list.first()?.clone();
    let bytes = tokio::task::spawn_blocking(move || crate::media::reader::read_entry(&path, &name))
        .await
        .ok()?
        .ok()?
        .0;
    use sha1::{Digest, Sha1};
    let mut h = Sha1::new();
    h.update(&bytes);
    Some(h.finalize().iter().map(|b| format!("{b:02X}")).collect())
}

/// Find metadata candidates for an item without modifying it. The plugin must be
/// enabled for the item's kind and support `identify`.
#[utoipa::path(
    post, path = "/api/items/{id}/identify", tag = "browse",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(
        ("id" = i64, Path, description = "Item id"),
        ("plugin" = String, Query, description = "identify-capable plugin id"),
    ),
    responses(
        (status = 200, description = "Ranked reverse-image candidates", body = IdentifyResponse),
        (status = 400, description = "Missing/unknown plugin or plugin lacks identify"),
        (status = 403, description = "Plugin not enabled for this item's kind"),
        (status = 404, description = "Unknown item"),
    ),
)]
pub(crate) async fn identify_item(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(params): Query<IdentifyParams>,
) -> Result<Json<IdentifyResponse>, AppError> {
    let plugin = params
        .plugin
        .ok_or_else(|| AppError::BadRequest("plugin is required".into()))?;
    let (scraper, _manifest) = resolve_plugin(&state, &plugin, "identify")?;
    let meta = repo::item_meta(&state.read, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let enabled_for_kind = repo::plugins_for_kind(&state.read, &meta.kind).await?;
    if !enabled_for_kind.contains(&plugin) {
        return Err(AppError::Forbidden);
    }

    let req = crate::plugins::scraper::IdentifyRequest {
        sha1: first_page_sha1(&state, &meta).await,
        page_count: meta.page_count,
        title_hint: Some(meta.title.clone()),
    };
    let result = scraper
        .identify(&req, &*state.fetcher)
        .await
        .map_err(upstream)?;

    let manifests = state.scrapers.manifests();
    let candidates = result
        .candidates
        .into_iter()
        .map(|c| {
            let plugin_id = manifests
                .iter()
                .find(|m| m.source == c.source && enabled_for_kind.contains(&m.id))
                .map(|m| m.id.clone());
            IdentifyCandidateOut {
                source: c.source,
                reference: c.reference,
                title: c.title,
                similarity: c.similarity,
                page_count: c.page_count,
                url: c.url,
                cover_url: c.cover_url,
                plugin_id,
            }
        })
        .collect();
    Ok(Json(IdentifyResponse { candidates }))
}

#[derive(Deserialize)]
pub(crate) struct DownloadParams {
    /// The item to download: a source id/ref the plugin understands (e.g. a
    /// source's item id `?ref=12345`). Required.
    #[serde(rename = "ref")]
    reference: Option<String>,
    /// Kind (top-level folder) for the new item; omitted uses `DEFAULT_KIND`.
    kind: Option<String>,
    #[serde(default)]
    wait: bool,
}

/// Queue a plugin download and ingest it through the normal deduplication path.
/// With `wait=true`, wait briefly for the result before returning the job.
#[utoipa::path(
    post, path = "/api/plugins/{id}/download", tag = "browse",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(
        ("id" = String, Path, description = "Plugin id (must support `download`)"),
        ("ref" = String, Query, description = "Source id/ref to download (e.g. a source's item id)"),
        ("kind" = Option<String>, Query, description = "Kind/folder for the new item; default 'uncategorized'"),
        ("wait" = Option<bool>, Query, description = "Wait ~5s for the result; else poll the job"),
    ),
    responses(
        (status = 200, description = "Download job queued (with result when waited)", body = ScrapeQueued),
        (status = 400, description = "Unknown plugin, missing ref, or plugin lacks download"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin, or plugin not enabled for the chosen kind"),
    ),
)]
pub(crate) async fn download_item(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(plugin): Path<String>,
    Query(params): Query<DownloadParams>,
) -> Result<Json<ScrapeQueued>, AppError> {
    let reference = params
        .reference
        .filter(|r| !r.is_empty())
        .ok_or_else(|| AppError::BadRequest("ref is required".into()))?;
    let manifest = state
        .scrapers
        .get(&plugin)
        .map(|s| s.manifest())
        .ok_or_else(|| AppError::BadRequest(format!("unknown plugin '{plugin}'")))?;
    if !manifest.capabilities.iter().any(|c| c == "download") {
        return Err(AppError::BadRequest(format!(
            "plugin '{plugin}' does not support download"
        )));
    }
    let kind = params
        .kind
        .and_then(|k| library::safe_filename(&k))
        .unwrap_or_else(|| scanner::DEFAULT_KIND.to_string());
    if !repo::plugins_for_kind(&state.read, &kind)
        .await?
        .contains(&plugin)
    {
        return Err(AppError::Forbidden);
    }
    let payload = serde_json::json!({
        "source": plugin,
        "reference": reference,
        "kind": kind,
    })
    .to_string();
    let job_id = jobs::enqueue(&state.write, "download", Some(&payload)).await?;
    let (job_state, result) = wait_result(&state, job_id, params.wait).await?;
    Ok(Json(ScrapeQueued {
        queued: true,
        job_id,
        plugin,
        state: job_state,
        result,
    }))
}

/// One "do I already have this?" query for a browse item.
#[derive(Deserialize, ToSchema)]
pub(crate) struct MatchQuery {
    /// Canonical source URL: the exact match against local `item_sources` (you
    /// downloaded/scraped it from this source).
    #[serde(default)]
    source_url: Option<String>,
    /// Remote cover URL used by the server-side perceptual-hash cache.
    #[serde(default)]
    cover_url: Option<String>,
    /// The browse item's page count, to disambiguate a fuzzy cover match.
    #[serde(default)]
    page_count: Option<i64>,
}

/// The ownership verdict for one query (fields omitted when absent).
#[derive(Serialize, ToSchema)]
pub(crate) struct MatchResult {
    /// Exact: a local item with the same source URL. Highest confidence, "In your library".
    #[serde(skip_serializing_if = "Option::is_none")]
    owned_item_id: Option<i64>,
    /// Fuzzy: best cover-hash + page-count match when there's no exact one, "Likely owned".
    #[serde(skip_serializing_if = "Option::is_none")]
    likely_item_id: Option<i64>,
    /// The fuzzy match's Hamming distance (0–64, lower = more similar), driving UI confidence.
    #[serde(skip_serializing_if = "Option::is_none")]
    distance: Option<i64>,
}

/// Compare cover hashes with a stricter threshold when page counts differ.
pub(crate) const PHASH_MAX_PAGE_DELTA: i64 = 2;

/// Reject near-constant dHashes because flat covers provide no useful identity.
pub(crate) const PHASH_MIN_TEXTURE_BITS: u32 = 8;

/// True when a cover hash carries enough texture to be a usable identity signal.
pub(crate) fn phash_informative(h: u64) -> bool {
    let p = h.count_ones();
    (PHASH_MIN_TEXTURE_BITS..=64 - PHASH_MIN_TEXTURE_BITS).contains(&p)
}

/// Check whether remote browse items are already owned. Exact source URLs are
/// preferred; cover hash and page count provide the fallback. Order is preserved.
#[utoipa::path(
    post, path = "/api/browse/match", tag = "browse",
    security(("sessionCookie" = []), ("apiKey" = [])),
    request_body = Vec<MatchQuery>,
    responses(
        (status = 200, description = "Per-query ownership verdicts, in request order", body = Vec<MatchResult>),
        (status = 401, description = "Not authenticated"),
    ),
)]
pub(crate) async fn library_match(
    _user: AuthUser,
    State(state): State<AppState>,
    Json(queries): Json<Vec<MatchQuery>>,
) -> Result<Json<Vec<MatchResult>>, AppError> {
    let urls: Vec<String> = queries
        .iter()
        .filter_map(|q| q.source_url.clone())
        .collect();
    let owned = repo::item_ids_by_source_urls(&state.read, &urls).await?;

    let mut hashes: Vec<Option<u64>> = queries
        .iter()
        .map(|q| {
            q.cover_url
                .as_deref()
                .and_then(|u| state.cover_hashes.get(u))
        })
        .collect();
    let misses: Vec<String> = queries
        .iter()
        .zip(&hashes)
        .filter(|(q, h)| h.is_none() && q.cover_url.is_some())
        .filter_map(|(q, _)| q.cover_url.clone())
        .collect();
    if !misses.is_empty() {
        let stored = repo::cover_hashes_for(&state.read, &misses).await?;
        for (q, h) in queries.iter().zip(hashes.iter_mut()) {
            if h.is_none() {
                if let Some(found) = q.cover_url.as_deref().and_then(|u| stored.get(u)) {
                    *h = Some(*found);
                    state
                        .cover_hashes
                        .insert(q.cover_url.clone().unwrap(), *found);
                }
            }
        }
    }

    let needs_fuzzy = queries
        .iter()
        .zip(&hashes)
        .any(|(q, h)| h.is_some() && q.source_url.as_ref().and_then(|u| owned.get(u)).is_none());
    let corpus = if needs_fuzzy {
        repo::phash_corpus(&state.read).await?
    } else {
        Vec::new()
    };

    let results: Vec<MatchResult> = queries
        .iter()
        .zip(&hashes)
        .map(|(q, h)| {
            match_query(
                q,
                *h,
                crate::server::config::PHASH_MAX_HAMMING,
                crate::server::config::PHASH_SAME_PAGE_HAMMING,
                &owned,
                &corpus,
            )
        })
        .collect();

    let with_cover = queries.iter().filter(|q| q.cover_url.is_some()).count();
    if with_cover > 0 {
        let hashed = hashes.iter().filter(|h| h.is_some()).count();
        let exact = results.iter().filter(|r| r.owned_item_id.is_some()).count();
        let likely = results
            .iter()
            .filter(|r| r.likely_item_id.is_some())
            .count();
        tracing::info!(
            target: "library_match",
            items = queries.len(),
            with_cover,
            hashed,
            exact,
            likely,
            corpus = corpus.len(),
            "browse match: {hashed}/{with_cover} covers hashed, {exact} owned, {likely} likely \
             (thresholds {}/{}, corpus {})",
            crate::server::config::PHASH_MAX_HAMMING,
            crate::server::config::PHASH_SAME_PAGE_HAMMING,
            corpus.len(),
        );
    }
    Ok(Json(results))
}

/// The pure per-query verdict: exact `source_url` ownership first, else the nearest cover
/// within the Hamming + page-delta bounds. Extracted so the tiering can be unit-tested.
pub(crate) fn match_query(
    q: &MatchQuery,
    qhash: Option<u64>,
    max_hamming: u32,
    same_page_hamming: u32,
    owned: &std::collections::HashMap<String, i64>,
    corpus: &[(i64, i64, Option<i64>)],
) -> MatchResult {
    if let Some(id) = q.source_url.as_ref().and_then(|u| owned.get(u)) {
        return MatchResult {
            owned_item_id: Some(*id),
            likely_item_id: None,
            distance: None,
        };
    }
    if let Some(qh) = qhash.filter(|h| phash_informative(*h)) {
        let mut best: Option<(i64, u32)> = None;
        let mut nearest: Option<(i64, u32, Option<i64>)> = None;
        for &(id, ph, pc) in corpus {
            if !phash_informative(ph as u64) {
                continue;
            }
            let dist = (qh ^ ph as u64).count_ones();
            if nearest.is_none_or(|(_, nd, _)| dist < nd) {
                nearest = Some((id, dist, pc));
            }
            let threshold = match (q.page_count, pc) {
                (Some(a), Some(b)) if a == b => same_page_hamming,
                _ => max_hamming,
            };
            if dist > threshold {
                continue;
            }
            if let (Some(a), Some(b)) = (q.page_count, pc) {
                if (a - b).abs() > PHASH_MAX_PAGE_DELTA {
                    continue;
                }
            }
            if best.is_none_or(|(_, bd)| dist < bd) {
                best = Some((id, dist));
            }
        }
        if best.is_none() {
            if let Some((id, nd, pc)) = nearest {
                tracing::debug!(
                    target: "library_match",
                    nearest_item = id,
                    distance = nd,
                    threshold = max_hamming,
                    query_pages = ?q.page_count,
                    item_pages = ?pc,
                    "no fuzzy match; nearest cover {nd}/64 bits away (strict threshold {max_hamming})"
                );
            }
        }
        if let Some((id, dist)) = best {
            return MatchResult {
                owned_item_id: None,
                likely_item_id: Some(id),
                distance: Some(dist as i64),
            };
        }
    }
    MatchResult {
        owned_item_id: None,
        likely_item_id: None,
        distance: None,
    }
}

#[cfg(test)]
mod tests {
    use super::{match_query, phash_informative, MatchQuery};
    use std::collections::HashMap;

    fn q(source_url: Option<&str>, page_count: Option<i64>) -> MatchQuery {
        MatchQuery {
            source_url: source_url.map(String::from),
            cover_url: None,
            page_count,
        }
    }

    #[test]
    fn match_query_tiers_and_thresholds() {
        const NEAR: u32 = 6;
        const SAME: u32 = 10;

        const BASE: u64 = 0xAAAA_AAAA_AAAA_AAAA;
        let flip = |k: u32| BASE ^ ((1u64 << k) - 1);

        let mut owned = HashMap::new();
        owned.insert("https://openlibrary.org/g/1/".to_string(), 7i64);
        let corpus = vec![(9i64, BASE as i64, Some(20i64))];

        let r = match_query(
            &q(Some("https://openlibrary.org/g/1/"), Some(20)),
            Some(BASE),
            NEAR,
            SAME,
            &owned,
            &corpus,
        );
        assert_eq!(r.owned_item_id, Some(7));
        assert_eq!(r.likely_item_id, None);

        let r = match_query(
            &q(None, Some(21)),
            Some(flip(4)),
            NEAR,
            SAME,
            &owned,
            &corpus,
        );
        assert_eq!(r.likely_item_id, Some(9));
        assert_eq!(r.distance, Some(4));

        assert_eq!(
            match_query(
                &q(None, Some(21)),
                Some(flip(16)),
                NEAR,
                SAME,
                &owned,
                &corpus
            )
            .likely_item_id,
            None
        );

        assert_eq!(
            match_query(
                &q(None, Some(20)),
                Some(flip(9)),
                NEAR,
                SAME,
                &owned,
                &corpus
            )
            .likely_item_id,
            Some(9)
        );
        assert_eq!(
            match_query(
                &q(None, Some(20)),
                Some(flip(13)),
                NEAR,
                SAME,
                &owned,
                &corpus
            )
            .likely_item_id,
            None
        );

        assert_eq!(
            match_query(
                &q(None, Some(30)),
                Some(flip(4)),
                NEAR,
                SAME,
                &owned,
                &corpus
            )
            .likely_item_id,
            None
        );

        let r = match_query(&q(None, None), None, NEAR, SAME, &owned, &corpus);
        assert!(r.owned_item_id.is_none() && r.likely_item_id.is_none());
    }

    #[test]
    fn flat_covers_never_fuzzy_match() {
        const NEAR: u32 = 10;
        const SAME: u32 = 12;
        let owned = HashMap::new();

        let corpus = vec![(9i64, 0b111i64, None)];
        let r = match_query(&q(None, None), Some(0b101), NEAR, SAME, &owned, &corpus);
        assert_eq!(r.likely_item_id, None, "two flat covers must not match");

        let corpus = vec![(9i64, 0xAAAA_AAAA_AAAA_AAAAu64 as i64, Some(20))];
        let r = match_query(&q(None, Some(20)), Some(0), NEAR, SAME, &owned, &corpus);
        assert_eq!(r.likely_item_id, None);

        assert!(!phash_informative(u64::MAX));
        assert!(!phash_informative(u64::MAX ^ 0b11));
        assert!(phash_informative(0xAAAA_AAAA_AAAA_AAAA));
    }
}
