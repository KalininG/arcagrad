//! Similar-items, For-You recommendations, and continue-reading.

use super::*;
use crate::server::auth;

/// The continue-reading shelf.
#[derive(Serialize, ToSchema)]
pub(crate) struct ContinueResponse {
    items: Vec<ContinueEntry>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct ContinueParams {
    limit: Option<i64>,
    /// Restrict to one kind (top-level folder name). Omitted = all kinds.
    kind: Option<String>,
}

/// The caller's continue-reading shelf — started-but-unfinished items, newest
/// first. A bounded shelf, so a plain limit (default 20, max 50), no cursor.
#[utoipa::path(
    get, path = "/api/items/continue", tag = "recommendations",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(ContinueParams),
    responses(
        (status = 200, description = "The continue-reading shelf", body = ContinueResponse),
        (status = 401, description = "Not authenticated"),
    ),
)]
pub(crate) async fn continue_reading(
    user: AuthUser,
    State(state): State<AppState>,
    Query(params): Query<ContinueParams>,
) -> Result<Json<ContinueResponse>, AppError> {
    let limit = params.limit.unwrap_or(20).clamp(1, 50);
    let kind = params.kind.as_deref().filter(|k| !k.is_empty());
    let mut items = repo::continue_reading(&state.read, user.id, kind, limit).await?;
    let hidden = auth::hidden_kinds_for(&state.read, &user).await?;
    if !hidden.is_empty() {
        items.retain(|c| !hidden.contains(&c.kind));
    }
    Ok(Json(ContinueResponse { items }))
}

/// Return the caller's recently finished entries, newest first.
#[utoipa::path(
    get, path = "/api/items/finished", tag = "recommendations",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(ContinueParams),
    responses(
        (status = 200, description = "Recently finished items, newest first", body = ContinueResponse),
        (status = 401, description = "Not authenticated"),
    ),
)]
pub(crate) async fn recently_finished(
    user: AuthUser,
    State(state): State<AppState>,
    Query(params): Query<ContinueParams>,
) -> Result<Json<ContinueResponse>, AppError> {
    let limit = params.limit.unwrap_or(20).clamp(1, 50);
    let kind = params.kind.as_deref().filter(|k| !k.is_empty());
    let mut items = repo::recently_finished(&state.read, user.id, kind, limit).await?;
    let hidden = auth::hidden_kinds_for(&state.read, &user).await?;
    if !hidden.is_empty() {
        items.retain(|c| !hidden.contains(&c.kind));
    }
    Ok(Json(ContinueResponse { items }))
}

/// A catalog card and its normalized recommendation score.
#[derive(Serialize, ToSchema)]
pub(crate) struct SimilarEntry {
    #[serde(flatten)]
    card: repo::CatalogEntry,
    /// Relative relevance in `(0, 1]`; the best personalized match is `1.0`.
    score: f32,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct SimilarResponse {
    items: Vec<SimilarEntry>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct SimilarParams {
    limit: Option<i64>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct RecommendParams {
    limit: Option<i64>,
    /// Restrict recommendations to one kind (top-level folder name, the per-tab
    /// "For You"). Omitted = across all kinds.
    kind: Option<String>,
}

/// Load or build the cached item-level recommendation scorer.
pub(crate) async fn corpus_of(
    state: &AppState,
) -> Result<std::sync::Arc<crate::intelligence::recommend::Corpus>, AppError> {
    if let Some(c) = state.corpus.get() {
        return Ok(c);
    }
    let c = std::sync::Arc::new(repo::build_corpus(&state.read).await?);
    state.corpus.set(c.clone());
    Ok(c)
}

/// Entry-level scorer/df map. A series counts once regardless of its number of
/// leaves, so this cannot share the item corpus without biasing common series tags.
async fn entry_corpus_of(
    state: &AppState,
) -> Result<std::sync::Arc<crate::intelligence::recommend::Corpus>, AppError> {
    if let Some(c) = state.entry_corpus.get() {
        return Ok(c);
    }
    let c = std::sync::Arc::new(repo::build_entry_corpus(&state.read).await?);
    state.entry_corpus.set(c.clone());
    Ok(c)
}

/// Return content-similar entries with the caller's state overlaid on each card.
#[utoipa::path(
    get, path = "/api/items/{id}/similar", tag = "recommendations",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(
        ("id" = i64, Path, description = "Item id"),
        SimilarParams,
    ),
    responses(
        (status = 200, description = "Similar items, most similar first", body = SimilarResponse),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "Unknown item"),
    ),
)]
pub(crate) async fn similar_items(
    Viewer(user): Viewer,
    State(state): State<AppState>,
    Path(item_id): Path<i64>,
    Query(params): Query<SimilarParams>,
) -> Result<Json<SimilarResponse>, AppError> {
    let limit = params.limit.unwrap_or(12).clamp(1, 50);
    match repo::item_kind_by_id(&state.read, item_id).await? {
        Some(k) => ensure_kind_visible(&state, &user, &k).await?,
        None => return Err(AppError::NotFound),
    }

    let leaf = repo::leaf_series_map(&state.read, &[item_id]).await?;
    let cards = if leaf.contains_key(&item_id) {
        let table = repo::read_neighbors(
            &state.read,
            item_id,
            crate::intelligence::recommend::NEIGHBOR_CAP as i64,
        )
        .await?;
        let neighbors = if !table.is_empty() {
            std::sync::Arc::new(table)
        } else {
            match state.similar.get(&item_id) {
                Some(cached) => cached,
                None => {
                    let corpus = corpus_of(&state).await?;
                    let computed = std::sync::Arc::new(
                        repo::neighbors_of(&state.read, &corpus, item_id).await?,
                    );
                    state.similar.put(item_id, computed.clone());
                    computed
                }
            }
        };
        let own_series = leaf.get(&item_id).copied();
        let mut cards =
            repo::collapse_ranked(&state.read, user.id, &neighbors, None, limit + 1, false).await?;
        cards.retain(|(c, _)| !(c.kind_of == "series" && Some(c.id) == own_series));
        cards.truncate(limit as usize);
        cards
    } else {
        let table = repo::read_entry_neighbors(
            &state.read,
            item_id,
            crate::intelligence::recommend::NEIGHBOR_CAP as i64,
        )
        .await?;
        let ranked = if !table.is_empty() {
            std::sync::Arc::new(table)
        } else {
            match state.similar.get(&item_id) {
                Some(cached) => cached,
                None => {
                    let corpus = entry_corpus_of(&state).await?;
                    let computed = std::sync::Arc::new(
                        repo::entry_neighbors_of(&state.read, &corpus, item_id).await?,
                    );
                    state.similar.put(item_id, computed.clone());
                    computed
                }
            }
        };
        repo::collapse_entry_ranked(&state.read, user.id, &ranked, None, limit, false).await?
    };

    let items = cards
        .into_iter()
        .map(|(card, score)| SimilarEntry { card, score })
        .collect();
    Ok(Json(SimilarResponse { items }))
}

/// A series-collapsed catalog card and its personalized taste score.
#[derive(Serialize, ToSchema)]
pub(crate) struct RecommendationCard {
    #[serde(flatten)]
    card: repo::CatalogEntry,
    /// Relative taste score in `(0, 1]`, best match = 1.0 (×10 for a 0–10 badge).
    score: f32,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct RecommendationsResponse {
    items: Vec<RecommendationCard>,
}

/// "For You" — a personalized shelf from the caller's favorited + completed
/// items. The ranked list is per-user (cached); cards overlay at serve time.
#[utoipa::path(
    get, path = "/api/recommendations", tag = "recommendations",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(RecommendParams),
    responses(
        (status = 200, description = "Personalized recommendations, best first", body = RecommendationsResponse),
        (status = 401, description = "Not authenticated"),
    ),
)]
pub(crate) async fn recommendations(
    user: AuthUser,
    State(state): State<AppState>,
    Query(params): Query<RecommendParams>,
) -> Result<Json<RecommendationsResponse>, AppError> {
    let limit = params.limit.unwrap_or(20).clamp(1, 50);
    let kind = params.kind.as_deref().filter(|k| !k.is_empty());
    let ranked = match state.for_you.get(&user.id) {
        Some(cached) => cached,
        None => {
            let corpus = entry_corpus_of(&state).await?;
            let computed = std::sync::Arc::new(
                repo::recommend_for_you(&state.read, &corpus, user.id, now_secs()).await?,
            );
            state.for_you.put(user.id, computed.clone());
            computed
        }
    };
    let hidden = auth::hidden_kinds_for(&state.read, &user).await?;
    let items = repo::collapse_entry_ranked(&state.read, user.id, &ranked, kind, limit, false)
        .await?
        .into_iter()
        .filter(|(card, _)| !hidden.contains(&card.kind))
        .map(|(card, score)| RecommendationCard { card, score })
        .collect();
    Ok(Json(RecommendationsResponse { items }))
}

/// Return similar series and one-shots of the same kind, ranked from the series'
/// own tags and its leaves' tags.
#[utoipa::path(
    get, path = "/api/series/{id}/similar", tag = "recommendations",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(
        ("id" = i64, Path, description = "Series id"),
        SimilarParams,
    ),
    responses(
        (status = 200, description = "Similar works (series + one-shots), best first", body = RecommendationsResponse),
        (status = 404, description = "Unknown series"),
    ),
)]
pub(crate) async fn series_similar(
    Viewer(user): Viewer,
    State(state): State<AppState>,
    Path(series_id): Path<i64>,
    Query(params): Query<SimilarParams>,
) -> Result<Json<RecommendationsResponse>, AppError> {
    let limit = params.limit.unwrap_or(12).clamp(1, 50);
    let key = repo::entry_key_series(series_id);
    match repo::entry_kind(&state.read, key).await? {
        Some(k) => ensure_kind_visible(&state, &user, &k).await?,
        None => return Err(AppError::NotFound),
    }
    let ranked = repo::read_entry_neighbors(
        &state.read,
        key,
        crate::intelligence::recommend::NEIGHBOR_CAP as i64,
    )
    .await?;
    let items = repo::collapse_entry_ranked(&state.read, user.id, &ranked, None, limit, false)
        .await?
        .into_iter()
        .map(|(card, score)| RecommendationCard { card, score })
        .collect();
    Ok(Json(RecommendationsResponse { items }))
}
