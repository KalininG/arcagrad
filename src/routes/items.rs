//! Item detail, reading state, ratings, tags, and metadata.

use super::*;

/// Single-item detail.
#[derive(Serialize, ToSchema)]
pub(crate) struct ItemDetail {
    id: i64,
    /// Structural content version used as `?v=` on immutable media URLs.
    version: String,
    kind: String,
    modality: String,
    /// Reader shell derived from modality and structure: `paginated`,
    /// `paginated-series`, `paginated-chapters`, `reflowable`, or `fixed`.
    reader: String,
    name: String,
    /// Scraped synopsis as raw markup, omitted when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    /// The description is a manual edit (scrapes won't overwrite it): the metadata
    /// editor's "protected" hint.
    description_manual: bool,
    /// Which source (plugin id) wrote the description, when a scrape did: the
    /// editor's forget-source blast-radius hint. Omitted for manual/unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    description_source: Option<String>,
    /// The original filename/OPF title, brackets intact: the metadata editor's
    /// provenance line under the editable title. Omitted for pre-`raw_title` rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    raw_title: Option<String>,
    /// Content-detected modality (pre-override): the editor's "Auto (…)" label.
    /// `modality` above stays the effective value renderers dispatch on.
    modality_detected: String,
    /// The user's modality override when set (the editor's current selection).
    #[serde(skip_serializing_if = "Option::is_none")]
    modality_override: Option<String>,
    page_count: i64,
    /// Estimated word count for a reflowable EPUB (total text chars ÷ 5), omitted otherwise.
    /// The client derives a "~N hours" reading-time estimate from it (a display constant).
    #[serde(skip_serializing_if = "Option::is_none")]
    word_count: Option<i64>,
    /// Human-facing file/container format (`EPUB`, `CBZ`, …).
    format: String,
    /// Physical archive/container size in bytes.
    size_bytes: i64,
    /// Embedded EPUB publisher, omitted when the source metadata has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    publisher: Option<String>,
    /// When the item was indexed (Unix seconds).
    added_at: i64,
    /// Paginated resume: the last-read page (0-based), or null. Reflowable items use
    /// `progress_locator` instead (page is meaningless for them).
    progress: Option<i64>,
    /// Reflowable (EPUB) resume: the opaque reader locator last saved, or null. The
    /// reflowable reader restores its position from this; paginated items omit it.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Object)]
    progress_locator: Option<serde_json::Value>,
    /// Reflowable overall progression (0..1). Paginated items omit it.
    #[serde(skip_serializing_if = "Option::is_none")]
    progress_value: Option<f64>,
    /// Reflowable progress row update time (Unix seconds). Paginated/unread items omit it.
    #[serde(skip_serializing_if = "Option::is_none")]
    last_read_at: Option<i64>,
    favorited: bool,
    /// The viewing user's half-star rating (1–10 = 0.5–5.0 stars), or null if unrated.
    /// Set via `PUT /api/items/{id}/rating`, cleared via `DELETE`.
    rating: Option<i64>,
    /// The viewer's paginated reading mode: `paged` or `vertical`.
    reading_mode: String,
    /// The item's place in its series, present only when it's a series leaf; a
    /// one-shot omits it. Drives the series link + prev/next-chapter navigation.
    #[serde(skip_serializing_if = "Option::is_none")]
    series: Option<ItemSeriesContext>,
    /// Ordered page ranges for chapters embedded in this archive.
    chapters: Vec<ChapterInfo>,
    tags: Vec<ItemTag>,
    /// Canonical source URLs this item was scraped from (one per source).
    sources: Vec<ItemSource>,
    /// Scraped comments mirrored from sources (author/date/score/body, all
    /// sources). `body` is raw markup; the client sanitizes before rendering.
    comments: Vec<ItemComment>,
}

/// A leaf item's series context on the item-detail payload.
#[derive(Serialize, ToSchema)]
pub(crate) struct ItemSeriesContext {
    /// The series id (route it to `GET /api/series/{id}`).
    id: i64,
    title: String,
    /// This leaf's display label ('Vol. 3' / 'Ch. 12.5'), or null if purely positional.
    number_disp: Option<String>,
    /// Adjacent leaves for prev/next-chapter navigation (null at the ends).
    prev_leaf_id: Option<i64>,
    next_leaf_id: Option<i64>,
}

/// A contiguous chapter range within an item's page stream.
#[derive(Serialize, ToSchema)]
pub(crate) struct ChapterInfo {
    /// Display label ('Ch. 1' / 'Ch. 12.5'), or null for a leading front-matter run.
    pub(crate) number: Option<String>,
    /// A title when known; only the front-matter run carries one ('Front matter').
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
    /// 0-based absolute index of the chapter's first page in the item's page stream.
    pub(crate) start_page: i64,
    /// Pages the chapter spans.
    pub(crate) page_count: i64,
    /// Opaque read-online handle for a browse-source chapter (fed to the plugin's
    /// `pages` export). None for a local in-archive chapter (read via `start_page`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reference: Option<String>,
}

/// Derive the reader shell from modality and item structure. Series membership
/// takes precedence over embedded chapters.
pub(crate) fn reader_hint(modality: &str, is_series_leaf: bool, has_chapters: bool) -> String {
    match (modality, is_series_leaf, has_chapters) {
        ("paginated", true, _) => "paginated-series".to_string(),
        ("paginated", false, true) => "paginated-chapters".to_string(),
        (m, _, _) => m.to_string(),
    }
}

/// Return item metadata and the caller's reading state. The first request may
/// build the page list to determine page count.
#[utoipa::path(
    get, path = "/api/items/{id}", tag = "items",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = i64, Path, description = "Item id")),
    responses(
        (status = 200, description = "Item detail + the caller's progress", body = ItemDetail),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "Unknown item"),
    ),
)]
pub(crate) async fn item_detail(
    Viewer(user): Viewer,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ItemDetail>, AppError> {
    let meta = repo::item_meta(&state.read, id)
        .await?
        .ok_or(AppError::NotFound)?;
    ensure_kind_visible(&state, &user, &meta.kind).await?;

    let page_count = if meta.modality != "paginated" {
        0
    } else {
        match meta.page_count {
            Some(pc) => pc,
            None => library::ensure_page_list(&state, id, meta.path.clone().into())
                .await?
                .len() as i64,
        }
    };

    let (progress, progress_locator, progress_value, last_read_at) =
        if meta.modality == "reflowable" {
            let saved = repo::get_reflowable_progress(&state.read, user.id, id).await?;
            let locator = saved
                .as_ref()
                .and_then(|(_, locator, _)| locator.as_deref())
                .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
            let value = saved.as_ref().map(|(value, _, _)| *value);
            let updated_at = saved.map(|(_, _, updated_at)| updated_at);
            (None, locator, value, updated_at)
        } else {
            let progress = repo::clamp_progress(
                repo::get_progress(&state.read, user.id, id).await?,
                Some(page_count),
            );
            (progress, None, None, None)
        };
    let tags = repo::tags_for_item_with_counts(&state.read, id).await?;
    let favorited = repo::is_favorited(&state.read, user.id, id).await?;
    let rating = repo::get_rating(&state.read, user.id, id).await?;
    let reading_mode = repo::get_reading_mode(&state.read, user.id, id).await?;
    let sources = repo::item_sources(&state.read, id).await?;
    let mut comments = repo::item_comments(&state.read, id).await?;
    for c in &mut comments {
        c.body =
            crate::server::comments::sanitize_for_display(&c.body, c.score.is_some_and(|s| s < 0));
    }
    let series = repo::series_leaf_context(&state.read, id)
        .await?
        .map(|c| ItemSeriesContext {
            id: c.series_id,
            title: c.series_title,
            number_disp: c.number_disp,
            prev_leaf_id: c.prev_leaf_id,
            next_leaf_id: c.next_leaf_id,
        });
    let chapters: Vec<ChapterInfo> =
        library::ensure_chapters(&state, id, meta.path.clone().into(), &meta.modality)
            .await?
            .into_iter()
            .map(|c| ChapterInfo {
                number: c.number_disp,
                title: c.title,
                start_page: c.start_page,
                page_count: c.page_count,
                reference: None,
            })
            .collect();
    let reader = reader_hint(&meta.modality, series.is_some(), !chapters.is_empty());
    Ok(Json(ItemDetail {
        id,
        version: meta.structural_hash,
        kind: meta.kind,
        modality: meta.modality,
        reader,
        name: meta.title,
        description: meta.description,
        description_manual: meta.description_manual,
        description_source: meta.description_source.clone(),
        raw_title: meta.raw_title,
        modality_detected: meta.modality_detected,
        modality_override: meta.modality_override,
        page_count,
        word_count: meta.word_count,
        format: meta.format,
        size_bytes: meta.size_bytes,
        publisher: meta.publisher,
        added_at: meta.added_at,
        progress,
        progress_locator,
        progress_value,
        last_read_at,
        favorited,
        rating,
        reading_mode,
        series,
        chapters,
        tags,
        sources,
        comments,
    }))
}

/// Favorite an item for the calling user (idempotent).
#[utoipa::path(
    post, path = "/api/items/{id}/favorite", tag = "items",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = i64, Path, description = "Item id")),
    responses(
        (status = 200, description = "Favorited (idempotent)", body = OkResponse),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "Unknown item"),
    ),
)]
pub(crate) async fn favorite_item(
    user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<OkResponse>, AppError> {
    if !repo::add_favorite(&state.write, user.id, id).await? {
        return Err(AppError::NotFound);
    }
    state.for_you.invalidate(&user.id);
    Ok(ok())
}

/// Remove the calling user's favorite (idempotent).
#[utoipa::path(
    delete, path = "/api/items/{id}/favorite", tag = "items",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = i64, Path, description = "Item id")),
    responses(
        (status = 200, description = "Unfavorited (idempotent)", body = OkResponse),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "Unknown item"),
    ),
)]
pub(crate) async fn unfavorite_item(
    user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<OkResponse>, AppError> {
    if !repo::remove_favorite(&state.write, user.id, id).await? {
        return Err(AppError::NotFound);
    }
    state.for_you.invalidate(&user.id);
    Ok(ok())
}

/// A star rating to set. Per-user personal state (like favorites/progress), so any
/// authed user may rate, not admin-only.
#[derive(Deserialize, ToSchema)]
pub(crate) struct RatingBody {
    pub(crate) value: i64,
}

/// Set the calling user's star rating for an item (1–10 = 0.5–5.0 stars). Overwrites
/// any previous rating.
#[utoipa::path(
    put, path = "/api/items/{id}/rating", tag = "items",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = i64, Path, description = "Item id")),
    request_body = RatingBody,
    responses(
        (status = 200, description = "Rating saved", body = OkResponse),
        (status = 400, description = "value must be between 1 and 10"),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "Unknown item"),
    ),
)]
pub(crate) async fn rate_item(
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
    if !repo::set_rating(&state.write, user.id, id, body.value).await? {
        return Err(AppError::NotFound);
    }
    Ok(ok())
}

/// Clear the calling user's rating (idempotent).
#[utoipa::path(
    delete, path = "/api/items/{id}/rating", tag = "items",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = i64, Path, description = "Item id")),
    responses(
        (status = 200, description = "Rating cleared (idempotent)", body = OkResponse),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "Unknown item"),
    ),
)]
pub(crate) async fn unrate_item(
    user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<OkResponse>, AppError> {
    if !repo::clear_rating(&state.write, user.id, id).await? {
        return Err(AppError::NotFound);
    }
    Ok(ok())
}

/// A reading mode to set. Per-user personal state (like rating), any authed user.
#[derive(Deserialize, ToSchema)]
pub(crate) struct ReadingModeBody {
    /// `paged` (default page-flip) or `vertical` (continuous scroll).
    mode: String,
}

/// Set the caller's `paged` or `vertical` reading mode for an item.
#[utoipa::path(
    put, path = "/api/items/{id}/reading-mode", tag = "items",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = i64, Path, description = "Item id")),
    request_body = ReadingModeBody,
    responses(
        (status = 200, description = "Reading mode saved", body = OkResponse),
        (status = 400, description = "Unknown mode"),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "Unknown item"),
    ),
)]
pub(crate) async fn set_reading_mode(
    user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<ReadingModeBody>,
) -> Result<Json<OkResponse>, AppError> {
    if !repo::READING_MODES.contains(&body.mode.as_str()) {
        return Err(AppError::BadRequest(format!(
            "unknown reading mode '{}' (expected one of: {})",
            body.mode,
            repo::READING_MODES.join(", ")
        )));
    }
    if !repo::set_reading_mode(&state.write, user.id, id, &body.mode).await? {
        return Err(AppError::NotFound);
    }
    Ok(ok())
}

/// Clear the calling user's reading mode for an item (back to the default,
/// idempotent).
#[utoipa::path(
    delete, path = "/api/items/{id}/reading-mode", tag = "items",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = i64, Path, description = "Item id")),
    responses(
        (status = 200, description = "Reading mode cleared (idempotent)", body = OkResponse),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "Unknown item"),
    ),
)]
pub(crate) async fn clear_reading_mode(
    user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<OkResponse>, AppError> {
    if !repo::clear_reading_mode(&state.write, user.id, id).await? {
        return Err(AppError::NotFound);
    }
    Ok(ok())
}

/// Progress fields are selected by the stored item modality:
/// - paginated (comics): `page` (0-based).
/// - reflowable (EPUB): `value` (overall progression 0..1) + an opaque `locator`.
#[derive(Deserialize, ToSchema)]
pub(crate) struct ProgressBody {
    /// Paginated items: the last-read page (0-based).
    #[serde(default)]
    page: Option<i64>,
    /// Reflowable items: overall progression 0..1 (drives completed / continue-reading).
    #[serde(default)]
    value: Option<f64>,
    /// Reflowable items: an opaque reader locator (any JSON) round-tripped on resume.
    #[serde(default)]
    #[schema(value_type = Object)]
    locator: Option<serde_json::Value>,
}

/// Save the user's reading position. Dispatches on the item's modality: a page for
/// paginated items, a progression + opaque locator for reflowable (EPUB) items.
#[utoipa::path(
    put, path = "/api/items/{id}/progress", tag = "items",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = i64, Path, description = "Item id")),
    request_body = ProgressBody,
    responses(
        (status = 200, description = "Progress saved", body = OkResponse),
        (status = 400, description = "Missing/invalid progress for the item's modality"),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "Unknown item"),
    ),
)]
pub(crate) async fn save_progress(
    user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<ProgressBody>,
) -> Result<Json<OkResponse>, AppError> {
    let meta = repo::item_meta(&state.read, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let saved = if meta.modality == "reflowable" {
        let value = body.value.ok_or_else(|| {
            AppError::BadRequest("value is required for a reflowable item".into())
        })?;
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(AppError::BadRequest(
                "value must be a fraction between 0 and 1".into(),
            ));
        }
        let locator = body.locator.as_ref().map(|v| v.to_string());
        repo::set_reflowable_progress(&state.write, user.id, id, value, locator.as_deref()).await?
    } else {
        let page = body
            .page
            .ok_or_else(|| AppError::BadRequest("page is required for a paginated item".into()))?;
        if page < 0 {
            return Err(AppError::BadRequest("page must be >= 0".into()));
        }
        repo::set_progress(&state.write, user.id, id, page).await?
    };
    if !saved {
        return Err(AppError::NotFound);
    }
    state.for_you.invalidate(&user.id);
    Ok(ok())
}

/// A partial metadata edit (`PUT /api/items/{id}/metadata`): every field optional.
/// Omitted = unchanged, `null` = reset/clear, value = set.
#[derive(Deserialize, ToSchema)]
pub(crate) struct MetadataEdit {
    /// New display title. Never clearable (omit or `null` = unchanged); `raw_title`
    /// keeps the original filename/OPF title as provenance regardless.
    #[serde(default)]
    title: Option<String>,
    /// Synopsis as raw markup. A value protects it from scrapes; null clears it
    /// and restores scrape ownership.
    #[serde(default, deserialize_with = "tri_state")]
    #[schema(value_type = Option<String>)]
    description: Option<Option<String>>,
    /// Override modality, or null to restore content detection.
    #[serde(default, deserialize_with = "tri_state")]
    #[schema(value_type = Option<String>)]
    modality_override: Option<Option<String>>,
}

/// Edit shared item metadata. Fields are tri-state; the response contains the
/// updated detail. Tags use the separate tag endpoints.
#[utoipa::path(
    put, path = "/api/items/{id}/metadata", tag = "items",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = i64, Path, description = "Item id")),
    request_body = MetadataEdit,
    responses(
        (status = 200, description = "Updated item detail", body = ItemDetail),
        (status = 400, description = "Empty title / unknown modality / oversized description"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Unknown item"),
    ),
)]
pub(crate) async fn edit_item_metadata(
    admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<MetadataEdit>,
) -> Result<Json<ItemDetail>, AppError> {
    let mut title_changed = false;
    if let Some(t) = &body.title {
        let t = validate_title(t)?;
        if !repo::set_item_title(&state.write, id, t).await? {
            return Err(AppError::NotFound);
        }
        title_changed = true;
    }
    if let Some(desc) = &body.description {
        let d = validate_description(desc.as_deref())?;
        if !repo::set_item_description_manual(&state.write, id, d).await? {
            return Err(AppError::NotFound);
        }
    }
    if let Some(m) = &body.modality_override {
        let m = m.as_deref().map(str::trim).filter(|v| !v.is_empty());
        if m.is_some_and(|v| !matches!(v, "paginated" | "reflowable" | "fixed")) {
            return Err(AppError::BadRequest(format!(
                "unknown modality: {:?} (paginated | reflowable | fixed)",
                m.unwrap_or_default()
            )));
        }
        if !repo::set_item_modality_override(&state.write, id, m).await? {
            return Err(AppError::NotFound);
        }
    }
    if title_changed {
        if let Err(e) = jobs::enqueue_reindex_search(&state.write, &[id], &[]).await {
            tracing::warn!("failed to queue search reindex for item {id}: {e:?}");
        }
    }
    item_detail(Viewer(admin.0), State(state), Path(id)).await
}

/// Remove a plugin source's URL, tags, comments, and source-owned description
/// from an item, then return the updated detail.
#[utoipa::path(
    delete, path = "/api/items/{id}/sources/{source}", tag = "items",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(
        ("id" = i64, Path, description = "Item id"),
        ("source" = String, Path, description = "Source id (e.g. a plugin id like 'openlibrary')"),
    ),
    responses(
        (status = 200, description = "Updated item detail", body = ItemDetail),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Unknown item, or it has no such source"),
    ),
)]
pub(crate) async fn forget_item_source(
    admin: AdminUser,
    State(state): State<AppState>,
    Path((id, source)): Path<(i64, String)>,
) -> Result<Json<ItemDetail>, AppError> {
    if !repo::delete_item_source(&state.write, id, &source).await? {
        return Err(AppError::NotFound);
    }
    repo::clear_item_tags_from_source(&state.write, id, &source).await?;
    repo::replace_item_comments(&state.write, id, &source, &[]).await?;
    repo::clear_item_description_from_source(&state.write, id, &source).await?;
    after_tag_edit(&state, id).await;
    item_detail(Viewer(admin.0), State(state), Path(id)).await
}

#[derive(Deserialize, ToSchema)]
pub(crate) struct TagEdit {
    namespace: String,
    value: String,
    #[serde(default)]
    qualifier: Option<String>,
}

/// Validate + normalize a tag edit against the closed vocabulary, so a bad
/// namespace/qualifier is a clean 400 rather than a 500 from the repo backstop.
pub(crate) fn norm_tag_edit(body: &TagEdit) -> Result<(String, String, String), AppError> {
    let namespace = body.namespace.trim().to_lowercase();
    let value = body.value.trim().to_lowercase();
    let qualifier = body
        .qualifier
        .as_deref()
        .unwrap_or("none")
        .trim()
        .to_lowercase();
    if !repo::valid_namespace(&namespace) {
        return Err(AppError::BadRequest(format!(
            "invalid namespace: {namespace:?}"
        )));
    }
    if value.is_empty() {
        return Err(AppError::BadRequest("tag value is required".into()));
    }
    Ok((namespace, value, qualifier))
}

/// Attach a tag to an archive (admin only). Reindexes FTS + invalidates rec caches.
#[utoipa::path(
    post, path = "/api/items/{id}/tags", tag = "items",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = i64, Path, description = "Item id")),
    request_body = TagEdit,
    responses(
        (status = 200, description = "Tag attached", body = OkResponse),
        (status = 400, description = "Unknown namespace/qualifier or empty value"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Unknown item"),
    ),
)]
pub(crate) async fn add_tag(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<TagEdit>,
) -> Result<Json<OkResponse>, AppError> {
    let (namespace, value, qualifier) = norm_tag_edit(&body)?;
    let tag_id = repo::get_or_create_tag(&state.write, &namespace, &value).await?;
    if !repo::add_item_tag(&state.write, id, tag_id, &qualifier, "manual").await? {
        return Err(AppError::NotFound);
    }
    after_tag_edit(&state, id).await;
    Ok(ok())
}

/// Detach a specific `(tag, qualifier)` from an archive (admin only).
#[utoipa::path(
    delete, path = "/api/items/{id}/tags", tag = "items",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = i64, Path, description = "Item id")),
    request_body = TagEdit,
    responses(
        (status = 200, description = "Tag detached", body = OkResponse),
        (status = 400, description = "Unknown namespace/qualifier or empty value"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Unknown item, or it doesn't carry that tag"),
    ),
)]
pub(crate) async fn remove_tag(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<TagEdit>,
) -> Result<Json<OkResponse>, AppError> {
    let (namespace, value, qualifier) = norm_tag_edit(&body)?;
    let tag_id = repo::tag_id(&state.read, &namespace, &value)
        .await?
        .ok_or(AppError::NotFound)?;
    if !repo::remove_item_tag(&state.write, id, tag_id, &qualifier).await? {
        return Err(AppError::NotFound);
    }
    after_tag_edit(&state, id).await;
    Ok(ok())
}

/// Refresh search and recommendation state after a tag change.
pub(crate) async fn after_tag_edit(state: &AppState, item_id: i64) {
    if let Err(e) = repo::reindex_item_tags(&state.write, item_id).await {
        tracing::warn!("failed to reindex search tags for item {item_id}: {e:?}");
    }
    if let Err(e) = jobs::after_item_tags_changed(state, item_id, true).await {
        tracing::warn!("tag-change invalidation failed for item {item_id}: {e:?}");
    }
}

/// Queue an item metadata scrape. With `wait=true`, wait briefly for the result;
/// otherwise poll the returned job id.
#[utoipa::path(
    post, path = "/api/items/{id}/scrape", tag = "items",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(
        ("id" = i64, Path, description = "Item id"),
        ("plugin" = String, Query, description = "Plugin id (required) — from GET /api/plugins"),
        ("ref" = Option<String>, Query, description = "Source URL or id for an exact match; else matched by title"),
        ("wait" = Option<bool>, Query, description = "Wait ~5s for the result; else poll the job"),
    ),
    responses(
        (status = 200, description = "Scrape job queued (with result when waited)", body = ScrapeQueued),
        (status = 400, description = "Missing or unknown plugin"),
        (status = 403, description = "Plugin not enabled for this item's kind"),
        (status = 404, description = "Unknown item"),
    ),
)]
pub(crate) async fn scrape_item(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(params): Query<ScrapeParams>,
) -> Result<Json<ScrapeQueued>, AppError> {
    let plugin = params
        .plugin
        .ok_or_else(|| AppError::BadRequest("plugin is required".into()))?;
    resolve_plugin(&state, &plugin, "scrape")?;
    let meta = repo::item_meta(&state.read, id)
        .await?
        .ok_or(AppError::NotFound)?;
    if !repo::plugins_for_kind(&state.read, &meta.kind)
        .await?
        .contains(&plugin)
    {
        return Err(AppError::Forbidden);
    }
    let payload = serde_json::json!({
        "item_id": id,
        "source": plugin,
        "reference": params.reference,
    })
    .to_string();
    let job_id = jobs::enqueue(&state.write, "scrape", Some(&payload)).await?;
    let (job_state, result) = wait_result(&state, job_id, params.wait).await?;

    Ok(Json(ScrapeQueued {
        queued: true,
        job_id,
        plugin,
        state: job_state,
        result,
    }))
}
