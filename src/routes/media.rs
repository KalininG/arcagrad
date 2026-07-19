//! Page/thumbnail/cover serving, and EPUB manifest/resource.

use super::*;
use crate::server::auth;

/// Serve one original page image from a paginated item.
#[utoipa::path(
    get, path = "/api/items/{id}/pages/{n}", tag = "media",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(
        ("id" = i64, Path, description = "Item id"),
        ("n" = usize, Path, description = "Page index (0-based)"),
        ("v" = Option<String>, Query, description = "Content version (structural_hash) — enables immutable caching"),
    ),
    responses(
        (status = 200, description = "Page image bytes in their original format", content(
            (Vec<u8> = "image/jpeg"),
            (Vec<u8> = "image/png"),
            (Vec<u8> = "image/webp"),
            (Vec<u8> = "image/gif"),
            (Vec<u8> = "image/avif"),
            (Vec<u8> = "image/bmp")
        )),
        (status = 304, description = "Not modified — the client's cached copy is current"),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "Unknown item or page index out of range"),
    ),
)]
pub(crate) async fn page(
    Viewer(user): Viewer,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, n)): Path<(i64, usize)>,
    Query(vq): Query<ImageQuery>,
) -> Result<Response, AppError> {
    let meta = repo::item_meta(&state.read, id)
        .await?
        .ok_or(AppError::NotFound)?;
    ensure_kind_visible(&state, &user, &meta.kind).await?;
    let etag = format!("{}:{n}", meta.structural_hash);
    let versioned = vq.v.is_some();
    if !versioned {
        if let Some(resp) = not_modified(&headers, &etag) {
            return Ok(resp);
        }
    }
    let path = std::path::PathBuf::from(&meta.path);

    let list = library::ensure_page_list(&state, id, path.clone()).await?;
    let name = list.get(n).ok_or(AppError::NotFound)?.clone();

    let permit = acquire(&state).await?;
    let (bytes, content_type) = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        reader::read_entry(&path, &name)
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!("task join error: {e}")))??;

    Ok(image_response(content_type, &etag, bytes, versioned))
}

/// One page's preview thumbnail (WebP), generated lazily on first request and
/// cached. Page thumbnails are a usage-driven cache; most are never generated.
#[utoipa::path(
    get, path = "/api/items/{id}/pages/{n}/thumbnail", tag = "media",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(
        ("id" = i64, Path, description = "Item id"),
        ("n" = usize, Path, description = "Page index (0-based)"),
        ("v" = Option<String>, Query, description = "Content version (structural_hash) — enables immutable caching"),
    ),
    responses(
        (status = 200, description = "Page preview thumbnail, generated on first request", body = Vec<u8>, content_type = "image/webp"),
        (status = 304, description = "Not modified — the client's cached copy is current"),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "Unknown/non-paginated item or page out of range"),
    ),
)]
pub(crate) async fn page_thumb(
    Viewer(user): Viewer,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, n)): Path<(i64, usize)>,
    Query(vq): Query<ImageQuery>,
) -> Result<Response, AppError> {
    let hidden = auth::hidden_kinds_for(&state.read, &user).await?;

    if hidden.is_empty() {
        if let Some(v) = vq.v.as_deref().filter(|v| looks_like_hash(v)) {
            let cache = thumbnail::page_cache_path(&state.config.data_dir, v, n);
            if let Ok(bytes) = tokio::fs::read(&cache).await {
                return Ok(image_response(
                    "image/webp",
                    &format!("{v}:{n}"),
                    bytes,
                    true,
                ));
            }
        }
    }

    let meta = repo::item_meta(&state.read, id)
        .await?
        .ok_or(AppError::NotFound)?;
    if hidden.contains(&meta.kind) {
        return Err(AppError::NotFound);
    }
    if meta.modality != "paginated" {
        return Err(AppError::NotFound);
    }
    let etag = format!("{}:{n}", meta.structural_hash);
    let versioned = vq.v.is_some();
    if !versioned {
        if let Some(resp) = not_modified(&headers, &etag) {
            return Ok(resp);
        }
    }
    let cache = thumbnail::page_cache_path(&state.config.data_dir, &meta.structural_hash, n);
    if let Ok(bytes) = tokio::fs::read(&cache).await {
        return Ok(image_response("image/webp", &etag, bytes, versioned));
    }

    let path = std::path::PathBuf::from(&meta.path);
    let list = library::ensure_page_list(&state, id, path.clone()).await?;
    let name = list.get(n).ok_or(AppError::NotFound)?.clone();

    let bytes =
        library::ensure_page_thumbnail(&state, &meta.structural_hash, &path, n, &name).await?;
    Ok(image_response("image/webp", &etag, bytes, versioned))
}

/// Serve the item's generated WebP cover thumbnail.
#[utoipa::path(
    get, path = "/api/items/{id}/thumbnail", tag = "media",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(
        ("id" = i64, Path, description = "Item id"),
        ("v" = Option<String>, Query, description = "Content version (structural_hash) — enables immutable caching"),
    ),
    responses(
        (status = 200, description = "Cover thumbnail", body = Vec<u8>, content_type = "image/webp"),
        (status = 304, description = "Not modified — the client's cached copy is current"),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "Unknown item"),
    ),
)]
pub(crate) async fn thumb(
    Viewer(user): Viewer,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Query(vq): Query<ImageQuery>,
) -> Result<Response, AppError> {
    let hidden = auth::hidden_kinds_for(&state.read, &user).await?;

    if hidden.is_empty() {
        if let Some(v) = vq.v.as_deref().filter(|v| looks_like_hash(v)) {
            let cache = thumbnail::cache_path(&state.config.data_dir, v);
            if let Ok(bytes) = tokio::fs::read(&cache).await {
                return Ok(image_response("image/webp", v, bytes, true));
            }
        }
    }

    let meta = repo::item_meta(&state.read, id)
        .await?
        .ok_or(AppError::NotFound)?;
    if hidden.contains(&meta.kind) {
        return Err(AppError::NotFound);
    }
    let versioned = vq.v.is_some();
    if !versioned {
        if let Some(resp) = not_modified(&headers, &meta.structural_hash) {
            return Ok(resp);
        }
    }
    let bytes = library::ensure_thumbnail(
        &state,
        id,
        &meta.structural_hash,
        &std::path::PathBuf::from(&meta.path),
        &meta.modality,
    )
    .await?;
    Ok(image_response(
        "image/webp",
        &meta.structural_hash,
        bytes,
        versioned,
    ))
}

/// The subset of a Readium Web Publication Manifest we emit. Enough for a reader to
/// render an EPUB: metadata + a self/cover link set + the reading order.
#[derive(Serialize, ToSchema)]
pub(crate) struct WebPubManifest {
    #[serde(rename = "@context")]
    context: &'static str,
    metadata: WebPubMetadata,
    links: Vec<WebPubLink>,
    #[serde(rename = "readingOrder")]
    reading_order: Vec<WebPubLink>,
    /// Flattened table of contents. `level` is the nesting depth.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    toc: Vec<WebPubTocEntry>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct WebPubTocEntry {
    href: String,
    title: String,
    level: usize,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct WebPubMetadata {
    #[serde(rename = "@type")]
    type_: &'static str,
    title: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    author: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    #[serde(rename = "conformsTo")]
    conforms_to: &'static str,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct WebPubLink {
    href: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    media_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rel: Option<String>,
}

/// MIME type for an EPUB internal resource from its name (`mime_guess` maps
/// `.xhtml` to `application/xhtml+xml`, plus css/js/svg/images/fonts).
pub(crate) fn resource_mime(name: &str) -> String {
    mime_guess::from_path(name)
        .first_or_octet_stream()
        .essence_str()
        .to_string()
}

/// The Readium manifest for a reflowable EPUB — its reading order + metadata.
#[utoipa::path(
    get, path = "/api/items/{id}/manifest", tag = "media",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = i64, Path, description = "Item id")),
    responses(
        (status = 200, description = "Readium Web Publication Manifest", body = WebPubManifest),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "Unknown item, or it isn't reflowable"),
    ),
)]
pub(crate) async fn manifest(
    Viewer(user): Viewer,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let meta = repo::item_meta(&state.read, id)
        .await?
        .ok_or(AppError::NotFound)?;
    ensure_kind_visible(&state, &user, &meta.kind).await?;
    if meta.modality != "reflowable" {
        return Err(AppError::NotFound);
    }
    let path = std::path::PathBuf::from(&meta.path);
    let epub = tokio::task::spawn_blocking(move || crate::media::epub::inspect(&path))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("task join error: {e}")))??;

    let base = format!("/api/items/{id}");
    let rbase = format!("{base}/resource/@v/{}", meta.structural_hash);
    let reading_order = epub
        .spine
        .iter()
        .map(|href| WebPubLink {
            href: format!("{rbase}/{href}"),
            media_type: Some(resource_mime(href)),
            rel: None,
        })
        .collect();
    let mut links = vec![WebPubLink {
        href: format!("{base}/manifest"),
        media_type: Some("application/webpub+json".to_string()),
        rel: Some("self".to_string()),
    }];
    if let Some(cover) = &epub.cover_href {
        links.push(WebPubLink {
            href: format!("{rbase}/{cover}"),
            media_type: Some(resource_mime(cover)),
            rel: Some("cover".to_string()),
        });
    }
    let toc = epub
        .toc
        .into_iter()
        .map(|t| WebPubTocEntry {
            href: format!("{rbase}/{}", t.href),
            title: t.label,
            level: t.level,
        })
        .collect();
    let manifest = WebPubManifest {
        context: "https://readium.org/webpub-manifest/context.jsonld",
        metadata: WebPubMetadata {
            type_: "http://schema.org/Book",
            title: meta.title,
            author: epub.authors,
            language: epub.language,
            conforms_to: "https://readium.org/webpub-manifest/profiles/epub",
        },
        links,
        reading_order,
        toc,
    };
    Ok(Json(manifest).into_response())
}

/// Serve an internal EPUB resource by archive path. This reader-only catch-all is
/// intentionally outside the OpenAPI surface.
pub(crate) async fn resource(
    Viewer(user): Viewer,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, res_path)): Path<(i64, String)>,
) -> Result<Response, AppError> {
    let meta = repo::item_meta(&state.read, id)
        .await?
        .ok_or(AppError::NotFound)?;
    ensure_kind_visible(&state, &user, &meta.kind).await?;
    if meta.modality != "reflowable" {
        return Err(AppError::NotFound);
    }
    let res_path = match res_path.strip_prefix("@t/").and_then(|r| r.split_once('/')) {
        Some((_token, rest)) => rest.to_string(),
        None => res_path,
    };
    let (entry, versioned) = match res_path.strip_prefix("@v/").and_then(|r| r.split_once('/')) {
        Some((_version, entry)) => (entry.to_string(), true),
        None => (res_path.clone(), false),
    };
    let etag = format!("{}:{entry}", meta.structural_hash);
    if !versioned {
        if let Some(resp) = not_modified(&headers, &etag) {
            return Ok(resp);
        }
    }
    let path = std::path::PathBuf::from(&meta.path);
    let permit = acquire(&state).await?;
    let name = entry.clone();
    let bytes = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        reader::read_entry(&path, &name).map(|(b, _)| b)
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!("task join error: {e}")))?
    .map_err(|_| AppError::NotFound)?;

    let cache_control = if versioned {
        IMAGE_CACHE_IMMUTABLE
    } else {
        IMAGE_CACHE_CONTROL
    };
    Ok((
        [
            (header::CONTENT_TYPE, resource_mime(&entry)),
            (header::CACHE_CONTROL, cache_control.to_string()),
            (header::ETAG, format!("\"{etag}\"")),
        ],
        bytes,
    )
        .into_response())
}

/// The revalidation directives shared by a bare-URL image `200` and its `304`.
pub(crate) const IMAGE_CACHE_CONTROL: &str = "private, no-cache";

/// Cache a private, content-versioned media URL without revalidation.
pub(crate) const IMAGE_CACHE_IMMUTABLE: &str = "private, max-age=31536000, immutable";

/// Whether a media URL includes a content version for immutable caching.
#[derive(Deserialize)]
pub(crate) struct ImageQuery {
    #[serde(default)]
    v: Option<String>,
}

/// Validate a version before using it as a thumbnail-cache path component.
/// Failure only disables the fast path.
pub(crate) fn looks_like_hash(v: &str) -> bool {
    (4..=128).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Serve media with immutable caching for versioned URLs or ETag revalidation
/// for stable URLs.
pub(crate) fn image_response(
    content_type: &str,
    etag: &str,
    bytes: Vec<u8>,
    versioned: bool,
) -> Response {
    let cache_control = if versioned {
        IMAGE_CACHE_IMMUTABLE
    } else {
        IMAGE_CACHE_CONTROL
    };
    (
        [
            (header::CONTENT_TYPE, content_type.to_string()),
            (header::CACHE_CONTROL, cache_control.to_string()),
            (header::ETAG, format!("\"{etag}\"")),
        ],
        bytes,
    )
        .into_response()
}

/// Return 304 when the request already holds the current media ETag.
pub(crate) fn not_modified(headers: &HeaderMap, etag: &str) -> Option<Response> {
    let tag = format!("\"{etag}\"");
    let inm = headers.get(header::IF_NONE_MATCH)?.to_str().ok()?;
    (inm == "*" || inm.split(',').any(|t| t.trim() == tag)).then(|| {
        (
            StatusCode::NOT_MODIFIED,
            [
                (header::CACHE_CONTROL, IMAGE_CACHE_CONTROL.to_string()),
                (header::ETAG, tag),
            ],
        )
            .into_response()
    })
}

#[cfg(test)]
mod tests {
    use super::looks_like_hash;

    #[test]
    fn looks_like_hash_gates_the_thumbnail_fast_path() {
        assert!(looks_like_hash(&"ab".repeat(32)));
        assert!(looks_like_hash("29db693ca817b0"));
        assert!(!looks_like_hash("../../etc/passwd"));
        assert!(!looks_like_hash("aa/bb/cc"));
        assert!(!looks_like_hash("abc"));
        assert!(!looks_like_hash(""));
        assert!(!looks_like_hash("g0g0g0g0"));
        assert!(!looks_like_hash(&"a".repeat(200)));
    }
}
