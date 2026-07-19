//! Upcoming releases and series trackers.

use super::*;

/// Upcoming publisher releases for series linked through calendar-capable plugins.
#[utoipa::path(
    get, path = "/api/upcoming", tag = "calendar",
    security(("sessionCookie" = []), ("apiKey" = [])),
    responses(
        (status = 200, description = "Upcoming releases from linked sources", body = calendar::UpcomingResponse),
        (status = 401, description = "Not authenticated"),
    ),
)]
pub(crate) async fn upcoming_releases(
    _user: AuthUser,
    State(state): State<AppState>,
) -> Result<Json<calendar::UpcomingResponse>, AppError> {
    Ok(Json(calendar::list(&state).await?))
}

/// Queue an immediate calendar refresh (admin). Coalescing prevents repeated
/// clicks from stacking equivalent work.
#[utoipa::path(
    post, path = "/api/upcoming/refresh", tag = "calendar",
    security(("sessionCookie" = []), ("apiKey" = [])),
    responses(
        (status = 202, description = "Calendar refresh queued", body = OkResponse),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Admin required"),
    ),
)]
pub(crate) async fn refresh_upcoming(
    _admin: AdminUser,
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<OkResponse>), AppError> {
    jobs::enqueue_calendar_refresh(&state.write).await?;
    Ok((StatusCode::ACCEPTED, ok()))
}

#[derive(Serialize, ToSchema)]
pub(crate) struct SeriesTrackerResponse {
    plugin_id: String,
    reference: String,
    created_at: i64,
    updated_at: i64,
}

impl From<repo::SeriesTracker> for SeriesTrackerResponse {
    fn from(value: repo::SeriesTracker) -> Self {
        Self {
            plugin_id: value.plugin_id,
            reference: value.reference,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Deserialize, ToSchema)]
pub(crate) struct SetSeriesTrackerRequest {
    reference: String,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct TrackedSeriesResponse {
    series_id: i64,
    title: String,
    kind: String,
    leaf_count: i64,
    cover_item_id: Option<i64>,
    cover_version: Option<String>,
    plugin_id: String,
    plugin_name: String,
    reference: String,
    created_at: i64,
    updated_at: i64,
    status: String,
    last_checked_at: Option<i64>,
    last_error: Option<String>,
    next_label: Option<String>,
    next_release_date: Option<String>,
}

/// List all configured series release trackers (admin).
#[utoipa::path(
    get, path = "/api/trackers", tag = "calendar",
    security(("sessionCookie" = []), ("apiKey" = [])),
    responses(
        (status = 200, description = "Every configured series release tracker", body = [TrackedSeriesResponse]),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Admin role required"),
    ),
)]
pub(crate) async fn list_all_series_trackers(
    _admin: AdminUser,
    State(state): State<AppState>,
) -> Result<Json<Vec<TrackedSeriesResponse>>, AppError> {
    let names: std::collections::HashMap<String, String> = state
        .scrapers
        .manifests()
        .into_iter()
        .map(|manifest| (manifest.id, manifest.name))
        .collect();
    let rows = repo::tracked_series(&state.read).await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| TrackedSeriesResponse {
                status: if row.last_error.is_some() {
                    "error"
                } else if row.last_checked_at.is_none() {
                    "pending"
                } else {
                    "active"
                }
                .to_string(),
                plugin_name: names
                    .get(&row.plugin_id)
                    .cloned()
                    .unwrap_or_else(|| row.plugin_id.clone()),
                series_id: row.series_id,
                title: row.title,
                kind: row.kind,
                leaf_count: row.leaf_count,
                cover_item_id: row.cover_item_id,
                cover_version: row.cover_version,
                plugin_id: row.plugin_id,
                reference: row.reference,
                created_at: row.created_at,
                updated_at: row.updated_at,
                last_checked_at: row.last_checked_at,
                last_error: row.last_error,
                next_label: row.next_label,
                next_release_date: row.next_release_date,
            })
            .collect(),
    ))
}

/// List release trackers configured for one series.
#[utoipa::path(
    get, path = "/api/series/{id}/trackers", tag = "calendar",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = i64, Path, description = "Series id")),
    responses(
        (status = 200, description = "Configured release trackers", body = [SeriesTrackerResponse]),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "Unknown series"),
    ),
)]
pub(crate) async fn list_series_trackers(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<SeriesTrackerResponse>>, AppError> {
    if repo::series_kind_by_id(&state.read, id).await?.is_none() {
        return Err(AppError::NotFound);
    }
    Ok(Json(
        repo::series_trackers(&state.read, id)
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}

/// Create or replace a release tracker for one series and plugin.
#[utoipa::path(
    put, path = "/api/series/{id}/trackers/{plugin}", tag = "calendar",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(
        ("id" = i64, Path, description = "Series id"),
        ("plugin" = String, Path, description = "Calendar plugin id"),
    ),
    request_body = SetSeriesTrackerRequest,
    responses(
        (status = 200, description = "Tracker saved", body = [SeriesTrackerResponse]),
        (status = 400, description = "Missing reference or plugin has no calendar capability"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Admin role required or plugin disabled for this kind"),
        (status = 404, description = "Unknown series or plugin"),
    ),
)]
pub(crate) async fn put_series_tracker(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path((id, plugin_id)): Path<(i64, String)>,
    Json(body): Json<SetSeriesTrackerRequest>,
) -> Result<Json<Vec<SeriesTrackerResponse>>, AppError> {
    let reference = body.reference.trim();
    if reference.is_empty() {
        return Err(AppError::BadRequest(
            "tracking reference is required".into(),
        ));
    }
    let kind = repo::series_kind_by_id(&state.read, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let manifest = state
        .scrapers
        .manifests()
        .into_iter()
        .find(|m| m.id == plugin_id)
        .ok_or(AppError::NotFound)?;
    if !manifest.capabilities.iter().any(|c| c == "calendar") {
        return Err(AppError::BadRequest(
            "plugin does not support release tracking".into(),
        ));
    }
    if !repo::plugins_for_kind(&state.read, &kind)
        .await?
        .iter()
        .any(|id| id == &plugin_id)
    {
        return Err(AppError::Forbidden);
    }
    if !repo::set_series_tracker(&state.write, id, &plugin_id, reference).await? {
        return Err(AppError::NotFound);
    }
    let _ = jobs::enqueue_calendar_refresh(&state.write).await;
    Ok(Json(
        repo::series_trackers(&state.read, id)
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}

/// Remove a series release tracker.
#[utoipa::path(
    delete, path = "/api/series/{id}/trackers/{plugin}", tag = "calendar",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(
        ("id" = i64, Path, description = "Series id"),
        ("plugin" = String, Path, description = "Calendar plugin id"),
    ),
    responses(
        (status = 200, description = "Tracker removed", body = [SeriesTrackerResponse]),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Unknown tracker"),
    ),
)]
pub(crate) async fn delete_series_tracker(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path((id, plugin_id)): Path<(i64, String)>,
) -> Result<Json<Vec<SeriesTrackerResponse>>, AppError> {
    if !repo::delete_series_tracker(&state.write, id, &plugin_id).await? {
        return Err(AppError::NotFound);
    }
    Ok(Json(
        repo::series_trackers(&state.read, id)
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}
