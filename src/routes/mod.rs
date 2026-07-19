//! Router assembly, OpenAPI registration, and shared route helpers.

mod admin;
mod auth;
mod browse;
mod follows;
mod items;
mod media;
mod plugins;
mod profile;
mod recommend;
mod search;
mod series;
mod settings;
mod users;

#[path = "calendar.rs"]
mod calendar_routes;
#[path = "library.rs"]
mod library_routes;

use self::admin::*;
use self::auth::*;
use self::browse::*;
use self::calendar_routes::*;
use self::follows::*;
use self::items::*;
use self::library_routes::*;
use self::media::*;
use self::plugins::*;
use self::profile::*;
use self::recommend::*;
use self::search::*;
use self::series::*;
use self::settings::*;
use self::users::*;

pub(crate) use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, State};
pub(crate) use axum::http::{header, HeaderMap, StatusCode};
pub(crate) use axum::response::{IntoResponse, Response};
pub(crate) use axum::Json;
pub(crate) use serde::{Deserialize, Serialize};
pub(crate) use tokio::io::AsyncWriteExt;
pub(crate) use tokio::sync::OwnedSemaphorePermit;
pub(crate) use utoipa::{IntoParams, ToSchema};

pub(crate) use crate::media::{library, reader, thumbnail};
pub(crate) use crate::plugins::calendar;
pub(crate) use crate::repo::{
    ContinueEntry, ItemComment, ItemSource, ItemTag, ListResult, TagCount,
};
pub(crate) use crate::server::auth::{AdminUser, ApiKeyInfo, AuthUser, SessionUser, Viewer};
pub(crate) use crate::server::error::AppError;
pub(crate) use crate::server::{jobs, metrics};
pub(crate) use crate::{repo, scanner, AppState};

use axum::Router;
use tower_http::compression::predicate::{NotForContentType, Predicate, SizeAbove};
use tower_http::compression::CompressionLayer;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tower_http::CompressionLevel;
use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::openapi::{Content, Ref, RefOr};
use utoipa::{Modify, OpenApi};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use utoipa_swagger_ui::SwaggerUi;

/// Multipart upload limit. Uploads stream to disk rather than memory.
pub(crate) const MAX_UPLOAD_BYTES: usize = 4 * 1024 * 1024 * 1024;

/// Maximum source image size for avatar uploads.
pub(crate) const AVATAR_MAX_BYTES: usize = 16 * 1024 * 1024;

/// `{ "ok": true }`, the shape every mutating endpoint returns on success.
#[derive(Serialize, ToSchema)]
pub(crate) struct OkResponse {
    ok: bool,
}

pub(crate) fn ok() -> Json<OkResponse> {
    Json(OkResponse { ok: true })
}

/// A queued job with an optional result when `wait=true` completes in time.
#[derive(Serialize, ToSchema)]
pub(crate) struct ScrapeQueued {
    queued: bool,
    job_id: i64,
    plugin: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
}

/// OpenAPI metadata and security schemes. Routes register their own paths and schemas.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "Arcagrad API",
        description = "Native REST API for Arcagrad — a self-hosted multi-type library. \
            Items carry an open `kind` (the top-level folder; the catalog/tab grouping) and a \
            `modality` (how they render). Generated from the code (utoipa). Two auth schemes on \
            protected routes: the `arca_session` cookie (web UI) and \
            `Authorization: Bearer arca_<token>` (API keys, created via the web UI). \
            Key-management endpoints accept the session cookie only. Errors share the \
            shape `{ \"error\": \"<message>\" }`."
    ),
    modifiers(&SecurityAddon),
    components(schemas(crate::server::error::ErrorResponse)),
    tags(
        (name = "meta", description = "Liveness."),
        (name = "auth", description = "Sessions, registration, and API keys."),
        (name = "profile", description = "The signed-in user's own profile, avatar/banner, and preferences."),
        (name = "users", description = "User administration (admin)."),
        (name = "settings", description = "Server settings (admin)."),
        (name = "library", description = "Library management — add/remove items, kinds, downloads."),
        (name = "items", description = "Item detail, reading state, ratings, tags, and metadata."),
        (name = "media", description = "Page images, thumbnails, and manifests."),
        (name = "series", description = "Series detail, metadata, and favorites."),
        (name = "tags", description = "Namespaced tag listing and filtering."),
        (name = "search", description = "Autocomplete/suggest."),
        (name = "recommendations", description = "Similar items and personalized shelves."),
        (name = "browse", description = "Source browsing, previews, ownership matching, and downloads."),
        (name = "plugins", description = "Plugin discovery, installation, credentials, and per-kind enablement."),
        (name = "calendar", description = "Upcoming releases and series trackers."),
        (name = "follows", description = "Followed source queries."),
        (name = "admin", description = "Library maintenance and background jobs."),
    ),
)]
pub(crate) struct ApiDoc;

/// Registers the two security schemes handlers reference by name.
pub(crate) struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "sessionCookie",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new("arca_session"))),
        );
        components.add_security_scheme(
            "apiKey",
            SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).build()),
        );
    }
}

/// Utoipa applies `OpenApi` modifiers before `OpenApiRouter` adds handler paths,
/// so response-wide normalization must run on the completed document.
fn document_error_bodies(openapi: &mut utoipa::openapi::OpenApi) {
    fn add(operation: &mut utoipa::openapi::path::Operation) {
        for (status, response) in &mut operation.responses.responses {
            if !(status.starts_with('4') || status.starts_with('5')) {
                continue;
            }
            if let RefOr::T(response) = response {
                response
                    .content
                    .entry("application/json".into())
                    .or_insert_with(|| Content::new(Some(Ref::from_schema_name("ErrorResponse"))));
            }
        }
    }

    for path in openapi.paths.paths.values_mut() {
        if let Some(operation) = path.get.as_mut() {
            add(operation);
        }
        if let Some(operation) = path.put.as_mut() {
            add(operation);
        }
        if let Some(operation) = path.post.as_mut() {
            add(operation);
        }
        if let Some(operation) = path.delete.as_mut() {
            add(operation);
        }
        if let Some(operation) = path.options.as_mut() {
            add(operation);
        }
        if let Some(operation) = path.head.as_mut() {
            add(operation);
        }
        if let Some(operation) = path.patch.as_mut() {
            add(operation);
        }
        if let Some(operation) = path.trace.as_mut() {
            add(operation);
        }
    }
}

pub fn router(state: AppState) -> Router {
    let (api_router, mut api) = OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(health))
        .routes(routes!(auth_status))
        .routes(routes!(setup))
        .routes(routes!(login))
        .routes(routes!(register))
        .routes(routes!(get_auth_settings, put_auth_settings))
        .routes(routes!(get_kind_access, put_kind_access))
        .routes(routes!(logout))
        .routes(routes!(logout_all))
        .routes(routes!(me))
        .routes(routes!(change_password))
        .routes(routes!(list_users, create_user_route))
        .routes(routes!(users_stats))
        .routes(routes!(delete_user_route))
        .routes(routes!(reset_user_password))
        .routes(routes!(get_user_avatar))
        .merge(
            OpenApiRouter::new()
                .routes(routes!(get_avatar, put_avatar, delete_avatar))
                .routes(routes!(get_banner, put_banner, delete_banner))
                .layer(DefaultBodyLimit::max(AVATAR_MAX_BYTES)),
        )
        .routes(routes!(list_keys, create_key))
        .routes(routes!(revoke_key))
        .routes(routes!(list_tags))
        .routes(routes!(favorite_tags))
        .routes(routes!(list_kinds))
        .routes(routes!(suggest))
        .routes(routes!(list_plugins))
        .routes(routes!(plugin_catalog))
        .routes(routes!(install_plugin))
        .routes(routes!(uninstall_plugin))
        .merge(
            OpenApiRouter::new()
                .routes(routes!(install_plugin_file))
                .layer(DefaultBodyLimit::max(PLUGIN_FILE_MAX_BYTES)),
        )
        .routes(routes!(
            list_plugin_repos,
            add_plugin_repo,
            remove_plugin_repo
        ))
        .routes(routes!(refresh_plugin_repos))
        .routes(routes!(follows_list, follows_create))
        .routes(routes!(follows_delete))
        .routes(routes!(follows_check))
        .routes(routes!(follow_items))
        .routes(routes!(follow_item_state))
        .routes(routes!(follow_dismiss_all))
        .routes(routes!(plugin_icon))
        .routes(routes!(plugin_browse))
        .routes(routes!(plugin_image))
        .routes(routes!(plugin_item))
        .routes(routes!(plugin_pages))
        .routes(routes!(library_match))
        .routes(routes!(list_kind_plugins, set_kind_plugins))
        .routes(routes!(list_credentials))
        .routes(routes!(put_credential, delete_credential))
        .routes(routes!(list_items))
        .merge(
            OpenApiRouter::new()
                .routes(routes!(create_item))
                .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)),
        )
        .routes(routes!(continue_reading))
        .routes(routes!(recently_finished))
        .routes(routes!(similar_items))
        .routes(routes!(recommendations))
        .routes(routes!(upcoming_releases))
        .routes(routes!(refresh_upcoming))
        .routes(routes!(stats))
        .routes(routes!(item_detail, delete_item))
        .routes(routes!(edit_item_metadata))
        .routes(routes!(forget_item_source))
        .routes(routes!(edit_series_metadata))
        .routes(routes!(forget_series_source))
        .routes(routes!(list_all_series_trackers))
        .routes(routes!(list_series_trackers))
        .routes(routes!(put_series_tracker, delete_series_tracker))
        .routes(routes!(series_detail))
        .routes(routes!(series_similar))
        .routes(routes!(favorite_series, unfavorite_series))
        .routes(routes!(rate_series, unrate_series))
        .routes(routes!(scrape_series))
        .routes(routes!(page))
        .routes(routes!(download_item_file))
        .routes(routes!(page_thumb))
        .routes(routes!(thumb))
        .routes(routes!(manifest))
        .routes(routes!(save_progress))
        .routes(routes!(add_tag, remove_tag))
        .routes(routes!(favorite_item, unfavorite_item))
        .routes(routes!(rate_item, unrate_item))
        .routes(routes!(set_reading_mode, clear_reading_mode))
        .routes(routes!(get_tag_blocklist, set_tag_blocklist))
        .routes(routes!(scrape_item))
        .routes(routes!(identify_item))
        .routes(routes!(download_item))
        .routes(routes!(job_status))
        .routes(routes!(rescan))
        .routes(routes!(server_metrics))
        .split_for_parts();
    document_error_bodies(&mut api);

    api_router
        .route(
            "/api/items/{id}/resource/{*path}",
            axum::routing::get(resource),
        )
        .merge(SwaggerUi::new("/api/docs").url("/api/openapi.json", api))
        .fallback(crate::server::web::static_handler)
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(
            TraceLayer::new_for_http().make_span_with(|req: &axum::http::Request<_>| {
                tracing::info_span!(
                    "request",
                    method = %req.method(),
                    path = %req.uri().path(),
                )
            }),
        )
        .layer(
            CompressionLayer::new()
                .quality(CompressionLevel::Precise(5))
                .compress_when(
                    SizeAbove::new(10240)
                        .and(NotForContentType::IMAGES)
                        .and(NotForContentType::GRPC)
                        .and(NotForContentType::SSE),
                ),
        )
        .with_state(state)
}

/// Check whether the server is running.
#[utoipa::path(
    get, path = "/health", tag = "meta", security(),
    responses((status = 200, description = "Server is up", body = String)),
)]
pub(crate) async fn health() -> &'static str {
    "ok"
}

pub(crate) async fn ensure_kind_visible(
    state: &AppState,
    user: &AuthUser,
    kind: &str,
) -> Result<(), AppError> {
    let hidden = crate::server::auth::hidden_kinds_for(&state.read, user).await?;
    if hidden.iter().any(|k| k == kind) {
        return Err(AppError::NotFound);
    }
    Ok(())
}

pub(crate) fn now_secs() -> i64 {
    crate::now_secs()
}

/// Deserialize omitted, null, and populated fields as distinct states.
pub(crate) fn tri_state<'de, T, D>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    T: serde::Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    serde::Deserialize::deserialize(de).map(Some)
}

/// Metadata-edit title validation, shared by the item and series editors so the
/// limits can't drift between them.
pub(crate) fn validate_title(t: &str) -> Result<&str, AppError> {
    let t = t.trim();
    if t.is_empty() {
        return Err(AppError::BadRequest("title cannot be empty".into()));
    }
    if t.len() > 1000 {
        return Err(AppError::BadRequest("title too long".into()));
    }
    Ok(t)
}

/// Metadata-edit description validation (item + series): trim, an all-whitespace
/// value degrades to a clear (`None`), and a 64 KiB cap applies.
pub(crate) fn validate_description(desc: Option<&str>) -> Result<Option<&str>, AppError> {
    let d = desc.map(str::trim).filter(|d| !d.is_empty());
    if d.is_some_and(|d| d.len() > 64 * 1024) {
        return Err(AppError::BadRequest("description too long".into()));
    }
    Ok(d)
}

/// Acquire a permit gating heavy image work; held for the lifetime of the
/// returned permit (move it into the `spawn_blocking` closure).
pub(crate) async fn acquire(state: &AppState) -> Result<OwnedSemaphorePermit, AppError> {
    state
        .blocking_limiter
        .clone()
        .acquire_owned()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("semaphore closed: {e}")))
}

/// Resolve a browse-family plugin + its manifest, or a 400 if the plugin is unknown or
/// doesn't declare `capability`. The single gate shared by every browse endpoint.
pub(crate) fn resolve_plugin(
    state: &AppState,
    id: &str,
    capability: &str,
) -> Result<
    (
        std::sync::Arc<dyn crate::plugins::scraper::MetadataScraper>,
        crate::plugins::scraper::ScraperManifest,
    ),
    AppError,
> {
    let scraper = state
        .scrapers
        .get(id)
        .ok_or_else(|| AppError::BadRequest(format!("unknown plugin '{id}'")))?;
    let manifest = scraper.manifest();
    if !manifest.capabilities.iter().any(|c| c == capability) {
        return Err(AppError::BadRequest(format!(
            "plugin '{id}' does not support {capability}"
        )));
    }
    Ok((scraper, manifest))
}

/// Expose actionable plugin failures as upstream errors.
pub(crate) fn upstream(e: anyhow::Error) -> AppError {
    let msg = format!("{e:#}");
    let msg = match msg
        .strip_prefix("plugin '")
        .and_then(|r| r.split_once("' failed: "))
    {
        Some((_, rest)) => rest.to_string(),
        None => msg,
    };
    AppError::Upstream(msg)
}

pub(crate) const SCRAPE_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

pub(crate) async fn wait_result(
    state: &AppState,
    job_id: i64,
    wait: bool,
) -> Result<(Option<String>, Option<serde_json::Value>), AppError> {
    if !wait {
        return Ok((None, None));
    }
    Ok(
        match jobs::wait_for_terminal(&state.read, job_id, SCRAPE_WAIT).await? {
            Some(s) => (
                Some(s.state),
                s.result.and_then(|r| serde_json::from_str(&r).ok()),
            ),
            None => (None, None),
        },
    )
}

#[derive(Deserialize)]
pub(crate) struct ScrapeParams {
    plugin: Option<String>,
    /// A source URL or id (`?ref=https://openlibrary.org/g/12345/` or `?ref=12345`)
    /// for an exact match; omitted, the plugin matches by the item's title.
    #[serde(rename = "ref")]
    reference: Option<String>,
    /// Block up to [`SCRAPE_WAIT`] for the result instead of returning immediately.
    #[serde(default)]
    wait: bool,
}
