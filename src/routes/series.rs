//! Series detail, metadata, favorites, and scraping.

use super::*;

#[derive(Serialize, ToSchema)]
pub(crate) struct SeriesDetailResponse {
    id: i64,
    /// Catalog grouping (the folder name), mirrors an item's `kind`.
    kind: String,
    title: String,
    /// Scrape-derived synopsis (raw markup, the client sanitizes), omitted when none.
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    /// Whether the description is a manual edit (protected from scrape overwrites).
    description_manual: bool,
    /// Which source (plugin id) wrote the description, when a scrape did: the
    /// editor's forget-source blast-radius hint. Omitted for manual/unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    description_source: Option<String>,
    added_at: i64,
    /// The item whose cover represents the series (fetch its `/thumbnail`).
    cover_item_id: Option<i64>,
    /// The leaf "Read"/"Continue" resumes at (first unfinished, else the last leaf).
    resume_leaf_id: Option<i64>,
    /// Leaves the viewing user has finished; the client shows "read_count / N volumes".
    read_count: i64,
    /// Whether the viewing user has favorited this series.
    favorited: bool,
    /// The viewing user's half-star rating (1–10 = 0.5–5.0 stars), omitted when unrated.
    /// Set via `PUT /api/series/{id}/rating`, cleared via `DELETE`.
    #[serde(skip_serializing_if = "Option::is_none")]
    rating: Option<i64>,
    /// The series' effective tags (series-level scraped ∪ the leaves' tags).
    tags: Vec<ItemTag>,
    /// Series-level source URLs (one per source, e.g. AniList). Per-volume
    /// comments/sources stay on each leaf's own item detail.
    sources: Vec<ItemSource>,
    leaves: Vec<SeriesLeafEntry>,
}

/// One leaf in a series-detail listing.
#[derive(Serialize, ToSchema)]
pub(crate) struct SeriesLeafEntry {
    /// The leaf's item id (route it to `GET /api/items/{id}` / the reader).
    item_id: i64,
    name: String,
    number_disp: Option<String>,
    number_sort: f64,
    page_count: Option<i64>,
    progress: Option<i64>,
    /// Rendering modality: the client shows page/total vs a % on the volume card.
    modality: String,
    /// Reflowable leaf: progression 0..1. Absent for paginated (uses `progress`).
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<f64>,
    /// Reflowable leaf: word count (sum leaves for the series reading-time line).
    /// Absent for paginated / not yet counted.
    #[serde(skip_serializing_if = "Option::is_none")]
    word_count: Option<i64>,
}

/// Return a series and its leaves in volume/chapter reading order.
#[utoipa::path(
    get, path = "/api/series/{id}", tag = "series",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = i64, Path, description = "Series id")),
    responses(
        (status = 200, description = "Series detail + ordered leaves", body = SeriesDetailResponse),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "Unknown series"),
    ),
)]
pub(crate) async fn series_detail(
    Viewer(user): Viewer,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<SeriesDetailResponse>, AppError> {
    let d = repo::series_detail(&state.read, user.id, id)
        .await?
        .ok_or(AppError::NotFound)?;
    ensure_kind_visible(&state, &user, &d.kind).await?;
    Ok(Json(SeriesDetailResponse {
        id: d.id,
        kind: d.kind,
        title: d.title,
        description: d.description,
        description_manual: d.description_manual,
        description_source: d.description_source,
        added_at: d.added_at,
        cover_item_id: d.cover_item_id,
        resume_leaf_id: d.resume_leaf_id,
        read_count: d.read_count,
        favorited: d.favorited,
        rating: d.rating,
        tags: d.tags,
        sources: d.sources,
        leaves: d
            .leaves
            .into_iter()
            .map(|l| SeriesLeafEntry {
                item_id: l.item_id,
                name: l.name,
                number_disp: l.number_disp,
                number_sort: l.number_sort,
                page_count: l.page_count,
                progress: l.progress,
                modality: l.modality,
                value: l.value,
                word_count: l.word_count,
            })
            .collect(),
    }))
}

/// Favorite a series independently of its leaves. The operation is idempotent.
#[utoipa::path(
    post, path = "/api/series/{id}/favorite", tag = "series",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = i64, Path, description = "Series id")),
    responses(
        (status = 200, description = "Favorited (idempotent)", body = OkResponse),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "Unknown series"),
    ),
)]
pub(crate) async fn favorite_series(
    user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<OkResponse>, AppError> {
    if !repo::add_series_favorite(&state.write, user.id, id).await? {
        return Err(AppError::NotFound);
    }
    state.for_you.invalidate(&user.id);
    Ok(ok())
}

/// Remove the calling user's series favorite (idempotent).
#[utoipa::path(
    delete, path = "/api/series/{id}/favorite", tag = "series",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = i64, Path, description = "Series id")),
    responses(
        (status = 200, description = "Unfavorited (idempotent)", body = OkResponse),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "Unknown series"),
    ),
)]
pub(crate) async fn unfavorite_series(
    user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<OkResponse>, AppError> {
    if !repo::remove_series_favorite(&state.write, user.id, id).await? {
        return Err(AppError::NotFound);
    }
    state.for_you.invalidate(&user.id);
    Ok(ok())
}

/// Set the caller's series rating from 1–10 half-star units.
#[utoipa::path(
    put, path = "/api/series/{id}/rating", tag = "series",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = i64, Path, description = "Series id")),
    request_body = RatingBody,
    responses(
        (status = 200, description = "Rating saved", body = OkResponse),
        (status = 400, description = "value must be between 1 and 10"),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "Unknown series"),
    ),
)]
pub(crate) async fn rate_series(
    user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<RatingBody>,
) -> Result<Json<OkResponse>, AppError> {
    if !(1..=10).contains(&body.value) {
        return Err(AppError::BadRequest(
            "value must be between 1 and 10".into(),
        ));
    }
    if !repo::set_series_rating(&state.write, user.id, id, body.value).await? {
        return Err(AppError::NotFound);
    }
    Ok(ok())
}

/// Clear the calling user's series rating (idempotent).
#[utoipa::path(
    delete, path = "/api/series/{id}/rating", tag = "series",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = i64, Path, description = "Series id")),
    responses(
        (status = 200, description = "Rating cleared (idempotent)", body = OkResponse),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "Unknown series"),
    ),
)]
pub(crate) async fn unrate_series(
    user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<OkResponse>, AppError> {
    if !repo::clear_series_rating(&state.write, user.id, id).await? {
        return Err(AppError::NotFound);
    }
    Ok(ok())
}

/// Partial series metadata update. Series modality and tags are not editable here.
#[derive(Deserialize, ToSchema)]
pub(crate) struct SeriesMetadataEdit {
    /// Display title. A value prevents future scanner-derived title updates.
    #[serde(default)]
    title: Option<String>,
    /// Synopsis, same tri-state contract as the item editor: a value marks it
    /// manual (scrapes stop overwriting); `null` clears text and flag.
    #[serde(default, deserialize_with = "tri_state")]
    #[schema(value_type = Option<String>)]
    description: Option<Option<String>>,
}

/// Edit shared series metadata and return the updated detail.
#[utoipa::path(
    put, path = "/api/series/{id}/metadata", tag = "series",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = i64, Path, description = "Series id")),
    request_body = SeriesMetadataEdit,
    responses(
        (status = 200, description = "Updated series detail", body = SeriesDetailResponse),
        (status = 400, description = "Empty title / oversized description"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Unknown series"),
    ),
)]
pub(crate) async fn edit_series_metadata(
    admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<SeriesMetadataEdit>,
) -> Result<Json<SeriesDetailResponse>, AppError> {
    if let Some(t) = &body.title {
        let t = validate_title(t)?;
        if !repo::set_series_title(&state.write, id, t).await? {
            return Err(AppError::NotFound);
        }
    }
    if let Some(desc) = &body.description {
        let d = validate_description(desc.as_deref())?;
        if !repo::set_series_description_manual(&state.write, id, d).await? {
            return Err(AppError::NotFound);
        }
    }
    series_detail(Viewer(admin.0), State(state), Path(id)).await
}

/// Remove a plugin source's URL, tags, and source-owned description from a series.
/// Leaf metadata is unchanged.
#[utoipa::path(
    delete, path = "/api/series/{id}/sources/{source}", tag = "series",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(
        ("id" = i64, Path, description = "Series id"),
        ("source" = String, Path, description = "Source id (e.g. a plugin id like 'anilist')"),
    ),
    responses(
        (status = 200, description = "Updated series detail", body = SeriesDetailResponse),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Unknown series, or it has no such source"),
    ),
)]
pub(crate) async fn forget_series_source(
    admin: AdminUser,
    State(state): State<AppState>,
    Path((id, source)): Path<(i64, String)>,
) -> Result<Json<SeriesDetailResponse>, AppError> {
    if !repo::delete_series_source(&state.write, id, &source).await? {
        return Err(AppError::NotFound);
    }
    repo::clear_series_tags_from_source(&state.write, id, &source).await?;
    repo::clear_series_description_from_source(&state.write, id, &source).await?;
    let _ = jobs::enqueue_entry_recompute_for_series(&state, id).await;
    series_detail(Viewer(admin.0), State(state), Path(id)).await
}

/// Queue a plugin metadata scrape whose results are applied to the series.
#[utoipa::path(
    post, path = "/api/series/{id}/scrape", tag = "series",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(
        ("id" = i64, Path, description = "Series id"),
        ("plugin" = String, Query, description = "Plugin id (required) — from GET /api/plugins"),
        ("ref" = Option<String>, Query, description = "Source URL or id for an exact match; else matched by title"),
        ("wait" = Option<bool>, Query, description = "Wait ~5s for the result; else poll the job"),
    ),
    responses(
        (status = 200, description = "Scrape job queued (with result when waited)", body = ScrapeQueued),
        (status = 400, description = "Missing or unknown plugin"),
        (status = 403, description = "Plugin not enabled for this series' kind"),
        (status = 404, description = "Unknown series"),
    ),
)]
pub(crate) async fn scrape_series(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(params): Query<ScrapeParams>,
) -> Result<Json<ScrapeQueued>, AppError> {
    let plugin = params
        .plugin
        .ok_or_else(|| AppError::BadRequest("plugin is required".into()))?;
    resolve_plugin(&state, &plugin, "scrape")?;
    let kind = repo::series_kind_by_id(&state.read, id)
        .await?
        .ok_or(AppError::NotFound)?;
    if !repo::plugins_for_kind(&state.read, &kind)
        .await?
        .contains(&plugin)
    {
        return Err(AppError::Forbidden);
    }
    let payload = serde_json::json!({
        "series_id": id,
        "source": plugin,
        "reference": params.reference,
    })
    .to_string();
    let job_id = jobs::enqueue(&state.write, "scrape_series", Some(&payload)).await?;
    let (job_state, result) = wait_result(&state, job_id, params.wait).await?;
    Ok(Json(ScrapeQueued {
        queued: true,
        job_id,
        plugin,
        state: job_state,
        result,
    }))
}
