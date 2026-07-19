//! The signed-in user's own profile, avatar/banner, and preferences.

use super::*;
use crate::server::auth;

/// The `/me` payload: the authenticated user plus profile extras.
#[derive(Serialize, ToSchema)]
pub(crate) struct MeResponse {
    pub(crate) id: i64,
    pub(crate) username: String,
    pub(crate) role: String,
    /// Account creation time (unix seconds): the profile's "member since".
    pub(crate) created_at: Option<i64>,
    /// Avatar mtime used as a cache version, or null when unset.
    pub(crate) avatar_version: Option<i64>,
    /// Custom profile banner version; same contract as `avatar_version`, for
    /// `GET /api/me/banner`.
    pub(crate) banner_version: Option<i64>,
}

/// Current user, or 401 (the `AuthUser` extractor rejects when unauthenticated).
#[utoipa::path(
    get, path = "/api/me", tag = "profile",
    security(("sessionCookie" = []), ("apiKey" = [])),
    responses(
        (status = 200, description = "The authenticated user", body = MeResponse),
        (status = 401, description = "Not authenticated"),
    ),
)]
pub(crate) async fn me(
    user: AuthUser,
    State(state): State<AppState>,
) -> Result<Json<MeResponse>, AppError> {
    let created_at = auth::user_created_at(&state.read, user.id).await?;
    let avatar_version = avatar_mtime(&avatar_path(&state, user.id)).await;
    let banner_version = avatar_mtime(&banner_path(&state, user.id)).await;
    Ok(Json(MeResponse {
        id: user.id,
        username: user.username,
        role: user.role,
        created_at,
        avatar_version,
        banner_version,
    }))
}

/// Return the caller's reading totals, rankings, series progress, and activity.
#[utoipa::path(
    get, path = "/api/me/stats", tag = "profile",
    security(("sessionCookie" = []), ("apiKey" = [])),
    responses(
        (status = 200, description = "The caller's reading stats", body = crate::intelligence::stats::Stats),
        (status = 401, description = "Not authenticated"),
    ),
)]
pub(crate) async fn stats(
    user: AuthUser,
    State(state): State<AppState>,
) -> Result<Json<crate::intelligence::stats::Stats>, AppError> {
    let today = now_secs() / crate::intelligence::stats::DAY;
    let s = crate::intelligence::stats::collect(&state.read, user.id, today)
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(s))
}

/// Path to a user's normalized avatar.
pub(crate) fn avatar_path(state: &AppState, user_id: i64) -> std::path::PathBuf {
    state
        .config
        .data_dir
        .join("avatars")
        .join(format!("{user_id}.webp"))
}

/// Where a user's (normalized) profile banner lives: `<data>/banners/<user_id>.webp`.
/// Same file-keyed, no-DB-column model as the avatar.
pub(crate) fn banner_path(state: &AppState, user_id: i64) -> std::path::PathBuf {
    state
        .config
        .data_dir
        .join("banners")
        .join(format!("{user_id}.webp"))
}

/// The avatar/banner file's mtime (unix seconds), or None when unset; the version
/// the client cache-busts with.
pub(crate) async fn avatar_mtime(path: &std::path::Path) -> Option<i64> {
    let meta = tokio::fs::metadata(path).await.ok()?;
    let mtime = meta.modified().ok()?;
    mtime
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

/// Upload an image to crop, resize, and re-encode as a 256px WebP avatar.
#[utoipa::path(
    put, path = "/api/me/avatar", tag = "profile",
    security(("sessionCookie" = []), ("apiKey" = [])),
    request_body(content = Vec<u8>, content_type = "application/octet-stream", description = "Raw image bytes"),
    responses(
        (status = 200, description = "Avatar set", body = OkResponse),
        (status = 400, description = "Not a decodable image"),
        (status = 401, description = "Not authenticated"),
    ),
)]
pub(crate) async fn put_avatar(
    user: AuthUser,
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> Result<Json<OkResponse>, AppError> {
    store_profile_image(
        &state,
        avatar_path(&state, user.id),
        body,
        thumbnail::generate_avatar_webp,
    )
    .await
}

/// Normalize a profile image off-thread and atomically replace the stored file.
pub(crate) async fn store_profile_image(
    state: &AppState,
    path: std::path::PathBuf,
    body: axum::body::Bytes,
    generate: fn(&[u8]) -> anyhow::Result<Vec<u8>>,
) -> Result<Json<OkResponse>, AppError> {
    if body.is_empty() {
        return Err(AppError::BadRequest("empty body".into()));
    }
    let _permit = state.blocking_limiter.clone().acquire_owned().await;
    let webp = tokio::task::spawn_blocking(move || generate(&body))
        .await
        .map_err(|e| AppError::Internal(e.into()))?
        .map_err(|e| AppError::BadRequest(format!("not a usable image: {e}")))?;

    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| AppError::Internal(e.into()))?;
    }
    let tmp = path.with_extension("webp.tmp");
    tokio::fs::write(&tmp, &webp)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    tokio::fs::rename(&tmp, &path)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;
    Ok(ok())
}

/// Return the caller's normalized avatar with private ETag caching.
#[utoipa::path(
    get, path = "/api/me/avatar", tag = "profile",
    security(("sessionCookie" = []), ("apiKey" = [])),
    responses(
        (status = 200, description = "The avatar", body = Vec<u8>, content_type = "image/webp"),
        (status = 304, description = "Not modified"),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "No avatar set"),
    ),
)]
pub(crate) async fn get_avatar(
    user: AuthUser,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    serve_profile_image(avatar_path(&state, user.id), "av", &headers).await
}

/// Serve a normalized profile image with mtime-based ETag revalidation.
pub(crate) async fn serve_profile_image(
    path: std::path::PathBuf,
    etag_prefix: &str,
    headers: &HeaderMap,
) -> Result<Response, AppError> {
    let Some(version) = avatar_mtime(&path).await else {
        return Err(AppError::NotFound);
    };
    let etag = format!("\"{etag_prefix}{version}\"");
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|inm| inm == etag)
    {
        return Ok((StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response());
    }
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| AppError::NotFound)?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/webp".to_string()),
            (header::CACHE_CONTROL, "private, no-cache".to_string()),
            (header::ETAG, etag),
        ],
        bytes,
    )
        .into_response())
}

/// Remove the calling user's profile picture (idempotent), back to the
/// generated-initial chip.
#[utoipa::path(
    delete, path = "/api/me/avatar", tag = "profile",
    security(("sessionCookie" = []), ("apiKey" = [])),
    responses(
        (status = 200, description = "Avatar removed (idempotent)", body = OkResponse),
        (status = 401, description = "Not authenticated"),
    ),
)]
pub(crate) async fn delete_avatar(
    user: AuthUser,
    State(state): State<AppState>,
) -> Result<Json<OkResponse>, AppError> {
    let _ = tokio::fs::remove_file(avatar_path(&state, user.id)).await;
    Ok(ok())
}

/// Upload an image to crop, resize, and re-encode as a 1500×500 WebP banner.
#[utoipa::path(
    put, path = "/api/me/banner", tag = "profile",
    security(("sessionCookie" = []), ("apiKey" = [])),
    request_body(content = Vec<u8>, content_type = "application/octet-stream", description = "Raw image bytes"),
    responses(
        (status = 200, description = "Banner set", body = OkResponse),
        (status = 400, description = "Not a decodable image"),
        (status = 401, description = "Not authenticated"),
    ),
)]
pub(crate) async fn put_banner(
    user: AuthUser,
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> Result<Json<OkResponse>, AppError> {
    store_profile_image(
        &state,
        banner_path(&state, user.id),
        body,
        thumbnail::generate_banner_webp,
    )
    .await
}

/// The calling user's profile banner (normalized WebP), or 404 when none is set.
/// Same ETag revalidation + `?v=<banner_version>` cache-busting as the avatar.
#[utoipa::path(
    get, path = "/api/me/banner", tag = "profile",
    security(("sessionCookie" = []), ("apiKey" = [])),
    responses(
        (status = 200, description = "The banner", body = Vec<u8>, content_type = "image/webp"),
        (status = 304, description = "Not modified"),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "No banner set"),
    ),
)]
pub(crate) async fn get_banner(
    user: AuthUser,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    serve_profile_image(banner_path(&state, user.id), "bn", &headers).await
}

/// Remove the calling user's profile banner (idempotent).
#[utoipa::path(
    delete, path = "/api/me/banner", tag = "profile",
    security(("sessionCookie" = []), ("apiKey" = [])),
    responses(
        (status = 200, description = "Banner removed", body = OkResponse),
        (status = 401, description = "Not authenticated"),
    ),
)]
pub(crate) async fn delete_banner(
    user: AuthUser,
    State(state): State<AppState>,
) -> Result<Json<OkResponse>, AppError> {
    let _ = tokio::fs::remove_file(banner_path(&state, user.id)).await;
    Ok(ok())
}

/// The calling user's persistent tag blocklist: tags hidden from every listing
/// (the "always hide mystery" veto). Personal state, like favorites/ratings.
#[utoipa::path(
    get, path = "/api/me/tag-blocklist", tag = "profile",
    security(("sessionCookie" = []), ("apiKey" = [])),
    responses(
        (status = 200, description = "The blocked tags", body = Vec<repo::BlockedTag>),
        (status = 401, description = "Not authenticated"),
    ),
)]
pub(crate) async fn get_tag_blocklist(
    user: AuthUser,
    State(state): State<AppState>,
) -> Result<Json<Vec<repo::BlockedTag>>, AppError> {
    Ok(Json(repo::blocklist_tags(&state.read, user.id).await?))
}

/// The new complete blocklist (PUT semantics: replaces the set; empty clears).
#[derive(Deserialize, ToSchema)]
pub(crate) struct BlocklistBody {
    /// `namespace:value` strings, e.g. `["tag:mystery", "tag:horror"]`.
    tags: Vec<String>,
}

/// Replace the calling user's tag blocklist. Every listed tag must exist (tags
/// only matter once something carries them; the UI picks from `/api/tags`).
#[utoipa::path(
    put, path = "/api/me/tag-blocklist", tag = "profile",
    security(("sessionCookie" = []), ("apiKey" = [])),
    request_body = BlocklistBody,
    responses(
        (status = 200, description = "Blocklist replaced", body = OkResponse),
        (status = 400, description = "Malformed or unknown tag"),
        (status = 401, description = "Not authenticated"),
    ),
)]
pub(crate) async fn set_tag_blocklist(
    user: AuthUser,
    State(state): State<AppState>,
    Json(body): Json<BlocklistBody>,
) -> Result<Json<OkResponse>, AppError> {
    let mut ids = Vec::with_capacity(body.tags.len());
    for raw in &body.tags {
        let (ns, value) = raw
            .split_once(':')
            .map(|(n, v)| (n.trim(), v.trim()))
            .filter(|(n, v)| !n.is_empty() && !v.is_empty())
            .ok_or_else(|| AppError::BadRequest(format!("malformed tag '{raw}'")))?;
        match repo::tag_id(&state.read, ns, value).await? {
            Some(id) => ids.push(id),
            None => return Err(AppError::BadRequest(format!("unknown tag '{raw}'"))),
        }
    }
    repo::set_blocklist(&state.write, user.id, &ids).await?;
    Ok(ok())
}
