//! The /api/follows surface (subscribe to a source query; stage new uploads).

use super::*;

/// One follow: a saved browse query + its review counters.
#[derive(Serialize, ToSchema)]
pub(crate) struct FollowInfo {
    id: i64,
    plugin_id: String,
    kind: String,
    feed: String,
    query: String,
    created_at: i64,
    last_checked_at: Option<i64>,
    /// Last check failure (the plugin's own message); NULL = healthy.
    last_error: Option<String>,
    /// Undismissed discoveries: the badge/pill number.
    new_count: i64,
}

impl From<repo::Follow> for FollowInfo {
    fn from(w: repo::Follow) -> Self {
        FollowInfo {
            id: w.id,
            plugin_id: w.plugin_id,
            kind: w.kind,
            feed: w.feed,
            query: w.query,
            created_at: w.created_at,
            last_checked_at: w.last_checked_at,
            last_error: w.last_error,
            new_count: w.new_count,
        }
    }
}

/// List every follow (admin).
#[utoipa::path(
    get, path = "/api/follows", tag = "follows",
    security(("sessionCookie" = []), ("apiKey" = [])),
    responses(
        (status = 200, description = "All follows", body = [FollowInfo]),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
    ),
)]
pub(crate) async fn follows_list(
    _admin: AdminUser,
    State(state): State<AppState>,
) -> Result<Json<Vec<FollowInfo>>, AppError> {
    let rows = repo::list_follows(&state.read).await?;
    Ok(Json(rows.into_iter().map(FollowInfo::from).collect()))
}

#[derive(Deserialize, ToSchema)]
pub(crate) struct CreateFollowBody {
    plugin_id: String,
    kind: String,
    feed: String,
    /// The committed filter. Required: following an unfiltered feed is noise.
    query: String,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct FollowCreated {
    id: i64,
    /// false = this exact follow already existed (create is idempotent).
    created: bool,
}

/// Create a follow for future discoveries. The initial check establishes a
/// baseline and does not import the source's back catalog.
#[utoipa::path(
    post, path = "/api/follows", tag = "follows",
    security(("sessionCookie" = []), ("apiKey" = [])),
    request_body = CreateFollowBody,
    responses(
        (status = 200, description = "Follow created (or already existed)", body = FollowCreated),
        (status = 400, description = "Unknown plugin/feed, or an empty query"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Plugin not enabled for this kind"),
    ),
)]
pub(crate) async fn follows_create(
    _admin: AdminUser,
    State(state): State<AppState>,
    Json(body): Json<CreateFollowBody>,
) -> Result<Json<FollowCreated>, AppError> {
    let query = body.query.trim().to_string();
    if query.is_empty() {
        return Err(AppError::BadRequest(
            "a follow needs a filter query — following a whole feed is not supported".into(),
        ));
    }
    let manifest = state
        .scrapers
        .get(&body.plugin_id)
        .map(|s| s.manifest())
        .ok_or_else(|| AppError::BadRequest(format!("unknown plugin '{}'", body.plugin_id)))?;
    if !manifest.feeds.iter().any(|f| f.id == body.feed) {
        return Err(AppError::BadRequest(format!(
            "plugin '{}' has no feed '{}'",
            body.plugin_id, body.feed
        )));
    }
    let kind = library::safe_filename(&body.kind)
        .ok_or_else(|| AppError::BadRequest("invalid kind".into()))?;
    if !repo::plugins_for_kind(&state.read, &kind)
        .await?
        .contains(&body.plugin_id)
    {
        return Err(AppError::Forbidden);
    }
    let (id, created) =
        repo::create_follow(&state.write, &body.plugin_id, &kind, &body.feed, &query).await?;
    if created {
        let payload = serde_json::json!({ "follows": [id] }).to_string();
        let _ = jobs::enqueue(&state.write, "check_follows", Some(&payload)).await;
    }
    Ok(Json(FollowCreated { id, created }))
}

/// Remove a follow (admin). Its seen-set goes with it; downloaded items stay.
#[utoipa::path(
    delete, path = "/api/follows/{id}", tag = "follows",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = i64, Path, description = "Follow id")),
    responses(
        (status = 200, description = "Removed", body = OkResponse),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
        (status = 404, description = "No such follow"),
    ),
)]
pub(crate) async fn follows_delete(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<OkResponse>, AppError> {
    if !repo::delete_follow(&state.write, id).await? {
        return Err(AppError::NotFound);
    }
    Ok(ok())
}

#[derive(Deserialize, ToSchema, Default)]
pub(crate) struct CheckFollowsBody {
    /// Specific follow ids to check; empty/omitted = check every follow.
    #[serde(default)]
    follows: Vec<i64>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct FollowCheckQueued {
    job_id: i64,
}

/// Queue an immediate follow check. Results are recorded on each follow.
#[utoipa::path(
    post, path = "/api/follows/check", tag = "follows",
    security(("sessionCookie" = []), ("apiKey" = [])),
    request_body(content = CheckFollowsBody, description = "Optional follow-id filter"),
    responses(
        (status = 200, description = "Check queued", body = FollowCheckQueued),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
    ),
)]
pub(crate) async fn follows_check(
    _admin: AdminUser,
    State(state): State<AppState>,
    body: Option<Json<CheckFollowsBody>>,
) -> Result<Json<FollowCheckQueued>, AppError> {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let payload = (!body.follows.is_empty())
        .then(|| serde_json::json!({ "follows": body.follows }).to_string());
    let job_id = jobs::enqueue(&state.write, "check_follows", payload.as_deref()).await?;
    Ok(Json(FollowCheckQueued { job_id }))
}

/// One reviewable discovery of a follow.
#[derive(Serialize, ToSchema)]
pub(crate) struct FollowItemInfo {
    reference: String,
    /// 'new' | 'queued' | 'downloaded' | 'skipped' | 'owned'
    state: String,
    seen_at: i64,
    /// The browse card captured at discovery (renders without re-hitting the
    /// source). Absent only for legacy/baseline rows.
    item: Option<crate::plugins::scraper::BrowseItem>,
}

/// A follow's discoveries, newest first (admin). Baseline rows are excluded.
#[utoipa::path(
    get, path = "/api/follows/{id}/items", tag = "follows",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = i64, Path, description = "Follow id")),
    responses(
        (status = 200, description = "Discoveries, newest first", body = [FollowItemInfo]),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
        (status = 404, description = "No such follow"),
    ),
)]
pub(crate) async fn follow_items(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<FollowItemInfo>>, AppError> {
    if repo::follow_by_id(&state.read, id).await?.is_none() {
        return Err(AppError::NotFound);
    }
    let rows = repo::follow_items(&state.read, id).await?;
    let out = rows
        .into_iter()
        .map(|r| FollowItemInfo {
            item: r
                .card_json
                .as_deref()
                .and_then(|j| serde_json::from_str(j).ok()),
            reference: r.reference,
            state: r.state,
            seen_at: r.seen_at,
        })
        .collect();
    Ok(Json(out))
}

#[derive(Deserialize, ToSchema)]
pub(crate) struct FollowItemStateBody {
    reference: String,
    /// 'skipped' (dismiss), 'new' (undo), 'queued' / 'downloaded' (download flow).
    state: String,
}

/// Move one discovery to a new state (admin): dismiss, undo, or record the
/// download the client started via POST /api/plugins/{id}/download.
#[utoipa::path(
    post, path = "/api/follows/{id}/items/state", tag = "follows",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = i64, Path, description = "Follow id")),
    request_body = FollowItemStateBody,
    responses(
        (status = 200, description = "State updated", body = OkResponse),
        (status = 400, description = "Invalid state"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
        (status = 404, description = "No such follow/item"),
    ),
)]
pub(crate) async fn follow_item_state(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<FollowItemStateBody>,
) -> Result<Json<OkResponse>, AppError> {
    if !matches!(
        body.state.as_str(),
        "new" | "skipped" | "queued" | "downloaded"
    ) {
        return Err(AppError::BadRequest("invalid state".into()));
    }
    if !repo::set_follow_item_state(&state.write, id, &body.reference, &body.state).await? {
        return Err(AppError::NotFound);
    }
    Ok(ok())
}

#[derive(Serialize, ToSchema)]
pub(crate) struct FollowBulkResult {
    moved: u64,
}

/// Dismiss every 'new' discovery of a follow (admin): the "Dismiss all" button.
#[utoipa::path(
    post, path = "/api/follows/{id}/items/dismiss-all", tag = "follows",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = i64, Path, description = "Follow id")),
    responses(
        (status = 200, description = "Moved to skipped", body = FollowBulkResult),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
        (status = 404, description = "No such follow"),
    ),
)]
pub(crate) async fn follow_dismiss_all(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<FollowBulkResult>, AppError> {
    if repo::follow_by_id(&state.read, id).await?.is_none() {
        return Err(AppError::NotFound);
    }
    let moved = repo::set_follow_new_items_state(&state.write, id, "skipped").await?;
    Ok(Json(FollowBulkResult { moved }))
}
