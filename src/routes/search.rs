//! Tag listing/filtering and blended autocomplete/suggest.

use super::*;
use crate::server::auth;

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct TagsQuery {
    /// Scope the tag list + counts to a single kind (the per-kind Tags page). Omit for
    /// the global list.
    kind: Option<String>,
}

/// All tags with usage counts, for a browse/filter sidebar or the per-kind Tags page.
/// `?kind=` scopes both the list and the counts to that kind. Any authed user.
#[utoipa::path(
    get, path = "/api/tags", tag = "tags",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(TagsQuery),
    responses(
        (status = 200, description = "Tags with usage counts, most-used first", body = Vec<TagCount>),
        (status = 401, description = "Not authenticated"),
    ),
)]
pub(crate) async fn list_tags(
    Viewer(user): Viewer,
    State(state): State<AppState>,
    Query(params): Query<TagsQuery>,
) -> Result<Json<Vec<TagCount>>, AppError> {
    Ok(Json(
        match params.kind.as_deref().filter(|k| !k.is_empty()) {
            Some(k) => {
                ensure_kind_visible(&state, &user, k).await?;
                repo::tags_with_counts_for_kind(&state.read, k, None, None).await?
            }
            None => {
                let deny = auth::hidden_kinds_for(&state.read, &user).await?;
                repo::tags_with_counts(&state.read, None, None, &deny).await?
            }
        },
    ))
}

/// Return the caller's most-used tags among favorited and completed entries.
#[utoipa::path(
    get, path = "/api/tags/favorites", tag = "tags",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(TagsQuery),
    responses(
        (status = 200, description = "Favorite/finished tags for the kind, most-used first", body = Vec<TagCount>),
        (status = 400, description = "Missing kind"),
        (status = 401, description = "Not authenticated"),
    ),
)]
pub(crate) async fn favorite_tags(
    user: AuthUser,
    State(state): State<AppState>,
    Query(params): Query<TagsQuery>,
) -> Result<Json<Vec<TagCount>>, AppError> {
    let kind = params
        .kind
        .as_deref()
        .filter(|k| !k.is_empty())
        .ok_or_else(|| AppError::BadRequest("kind is required".into()))?;
    ensure_kind_visible(&state, &user, kind).await?;
    Ok(Json(
        repo::favorite_tags_for_kind(&state.read, user.id, kind).await?,
    ))
}

/// One row in the blended autocomplete dropdown. A `tag` filters the library when
/// clicked; a `title` opens that item. The client renders them in the given order.
#[derive(Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub(crate) enum Suggestion {
    /// A namespaced tag from the vocabulary (prefix-matched on value).
    Tag {
        namespace: String,
        value: String,
        /// Items/series carrying this tag.
        count: i64,
    },
    /// An item whose title (or romaji title) matches, served from the search index.
    /// Build the cover URL as `/api/items/{id}/thumbnail?v={cover_version}`.
    Title {
        id: i64,
        title: String,
        kind: String,
        /// The item's `structural_hash`; append as `?v=` for immutable cover caching.
        cover_version: String,
    },
    /// A matched series collapsed from one or more matching leaves.
    Series {
        id: i64,
        title: String,
        kind: String,
        cover_item_id: i64,
        cover_version: String,
    },
}

/// A title hit after series collapse: a standalone item, or a whole series (one row
/// for all its matching leaves). Carries the rank score for blending.
pub(crate) struct CollapsedHit {
    series: bool,
    /// Item id (standalone) or series id (series): the row's navigation target.
    id: i64,
    title: String,
    kind: String,
    /// The item whose thumbnail to show (== `id` for a standalone item).
    cover_item_id: i64,
    cover_version: String,
    score: f32,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct SuggestResponse {
    results: Vec<Suggestion>,
}

/// Minimum slots reserved for tags and titles before filling by score.
pub(crate) const SUGGEST_GROUP_FLOOR: usize = 2;

/// Increase title weight as the query becomes long enough to be specific.
pub(crate) fn suggest_title_weight(query_len: usize) -> f32 {
    (0.15 + (query_len as f32 - 2.0) * 0.18).clamp(0.15, 1.5)
}

/// Blend prefix-matched tags (popularity-ranked) and title hits (BM25-ranked) into
/// one ordered list. Pure and deterministic (unit-tested).
pub(crate) fn blend_suggestions(
    tags: &[repo::TagCount],
    titles: &[CollapsedHit],
    query_len: usize,
    limit: usize,
) -> Vec<Suggestion> {
    if limit == 0 {
        return Vec::new();
    }
    let max_count = tags.iter().map(|t| t.count).max().unwrap_or(1).max(1) as f32;
    let max_bm25 = titles
        .iter()
        .map(|t| t.score)
        .fold(0f32, f32::max)
        .max(f32::MIN_POSITIVE);
    let tw = suggest_title_weight(query_len);

    let mut scored: Vec<(f32, bool, usize)> = Vec::with_capacity(tags.len() + titles.len());
    for (i, t) in tags.iter().enumerate() {
        scored.push((t.count as f32 / max_count, false, i));
    }
    for (i, t) in titles.iter().enumerate() {
        scored.push(((t.score / max_bm25) * tw, true, i));
    }
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
            .then(a.2.cmp(&b.2))
    });

    let want_tags = tags.len().min(SUGGEST_GROUP_FLOOR);
    let want_titles = titles.len().min(SUGGEST_GROUP_FLOOR);
    let mut chosen: Vec<usize> = Vec::with_capacity(limit);
    let (mut n_tag, mut n_title) = (0usize, 0usize);
    for (si, s) in scored.iter().enumerate() {
        if chosen.len() >= limit {
            break;
        }
        let reserve = if s.1 {
            n_title < want_titles
        } else {
            n_tag < want_tags
        };
        if reserve {
            chosen.push(si);
            if s.1 {
                n_title += 1;
            } else {
                n_tag += 1;
            }
        }
    }
    for si in 0..scored.len() {
        if chosen.len() >= limit {
            break;
        }
        if !chosen.contains(&si) {
            chosen.push(si);
        }
    }
    chosen.sort_unstable();

    chosen
        .into_iter()
        .map(|si| {
            let (_, is_title, idx) = scored[si];
            if is_title {
                let t = &titles[idx];
                if t.series {
                    Suggestion::Series {
                        id: t.id,
                        title: t.title.clone(),
                        kind: t.kind.clone(),
                        cover_item_id: t.cover_item_id,
                        cover_version: t.cover_version.clone(),
                    }
                } else {
                    Suggestion::Title {
                        id: t.id,
                        title: t.title.clone(),
                        kind: t.kind.clone(),
                        cover_version: t.cover_version.clone(),
                    }
                }
            } else {
                let t = &tags[idx];
                Suggestion::Tag {
                    namespace: t.namespace.clone(),
                    value: t.value.clone(),
                    count: t.count,
                }
            }
        })
        .collect()
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct BlendedSuggestParams {
    /// The typed query. Empty means no results.
    q: Option<String>,
    /// Max rows (default 10, capped at 20).
    limit: Option<i64>,
    /// Restrict suggestions to one library kind. Omit for all visible kinds.
    kind: Option<String>,
}

/// Blend prefix-matched tags and BM25 title matches into one autocomplete list.
#[utoipa::path(
    get, path = "/api/suggest", tag = "search",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(BlendedSuggestParams),
    responses(
        (status = 200, description = "Blended tag + title suggestions, best first", body = SuggestResponse),
        (status = 401, description = "Not authenticated"),
    ),
)]
pub(crate) async fn suggest(
    Viewer(user): Viewer,
    State(state): State<AppState>,
    Query(params): Query<BlendedSuggestParams>,
) -> Result<Json<SuggestResponse>, AppError> {
    let q = params.q.unwrap_or_default();
    let q = q.trim();
    if q.is_empty() {
        return Ok(Json(SuggestResponse {
            results: Vec::new(),
        }));
    }
    let limit = params.limit.unwrap_or(10).clamp(1, 20) as usize;
    let kind = params.kind.as_deref().filter(|k| !k.is_empty());
    let deny = auth::hidden_kinds_for(&state.read, &user).await?;
    let tag_prefix = q.to_lowercase();
    let tags = match kind {
        Some(k) => {
            repo::tags_with_counts_for_kind(&state.read, k, Some(&tag_prefix), Some(limit as i64))
                .await?
        }
        None => {
            repo::tags_with_counts(&state.read, Some(&tag_prefix), Some(limit as i64), &deny)
                .await?
        }
    };
    let needs_filter = !deny.is_empty() || kind.is_some();
    let fetch = if needs_filter { limit * 3 } else { limit };
    let mut titles = state
        .search
        .suggest_titles(q, fetch)
        .map_err(AppError::Internal)?;
    if needs_filter {
        titles.retain(|t| !deny.contains(&t.kind) && kind.is_none_or(|k| t.kind == k));
        titles.truncate(limit);
    }
    let mut collapsed = collapse_series_hits(&state.read, user.id, &titles).await?;
    let direct = repo::suggest_series(&state.read, q, kind, &deny, limit as i64).await?;
    if !direct.is_empty() {
        let top = collapsed
            .iter()
            .map(|h| h.score)
            .fold(0f32, f32::max)
            .max(1.0);
        let present: std::collections::HashSet<i64> = collapsed
            .iter()
            .filter(|h| h.series)
            .map(|h| h.id)
            .collect();
        for s in direct {
            if present.contains(&s.id) {
                continue;
            }
            let Some(cover_item_id) = s.cover_item_id else {
                continue;
            };
            collapsed.push(CollapsedHit {
                series: true,
                id: s.id,
                title: s.title,
                kind: s.kind,
                cover_item_id,
                cover_version: s.cover_version.unwrap_or_default(),
                score: top,
            });
        }
    }
    let results = blend_suggestions(&tags, &collapsed, q.chars().count(), limit);
    Ok(Json(SuggestResponse { results }))
}

/// Collapse matching leaves into one series result while retaining standalone items.
pub(crate) async fn collapse_series_hits(
    pool: &sqlx::SqlitePool,
    user_id: i64,
    titles: &[crate::intelligence::search::TitleHit],
) -> Result<Vec<CollapsedHit>, AppError> {
    if titles.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<i64> = titles.iter().map(|t| t.id).collect();
    let series_of = repo::series_id_of_items(pool, &ids).await?;
    let mut series_ids = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for t in titles {
        if let Some(&sid) = series_of.get(&t.id) {
            if seen.insert(sid) {
                series_ids.push(sid);
            }
        }
    }
    let series_cards = if series_ids.is_empty() {
        std::collections::HashMap::new()
    } else {
        repo::series_cards_for_ids(pool, user_id, &series_ids, true).await?
    };
    let item_hit = |t: &crate::intelligence::search::TitleHit| CollapsedHit {
        series: false,
        id: t.id,
        title: t.title.clone(),
        kind: t.kind.clone(),
        cover_item_id: t.id,
        cover_version: t.structural_hash.clone(),
        score: t.score,
    };
    let mut out = Vec::new();
    let mut emitted_series = std::collections::HashSet::new();
    let mut emitted_titles = std::collections::HashSet::new();
    for t in titles {
        let collapse_to = series_of.get(&t.id).and_then(|sid| {
            series_cards
                .get(sid)
                .filter(|card| crate::media::series::leaf_belongs_to_series(&t.title, &card.name))
                .map(|card| (*sid, card))
        });
        match collapse_to {
            Some((sid, card)) => {
                if !emitted_series.insert(sid) {
                    continue;
                }
                out.push(CollapsedHit {
                    series: true,
                    id: sid,
                    title: card.name.clone(),
                    kind: card.kind.clone(),
                    cover_item_id: card.cover_item_id.unwrap_or(t.id),
                    cover_version: card
                        .cover_version
                        .clone()
                        .unwrap_or_else(|| t.structural_hash.clone()),
                    score: t.score,
                });
            }
            None => {
                if emitted_titles.insert(t.title.to_lowercase()) {
                    out.push(item_hit(t));
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{blend_suggestions, suggest_title_weight, CollapsedHit, Suggestion};
    use crate::repo::TagCount;

    fn tag(value: &str, count: i64) -> TagCount {
        TagCount {
            namespace: "tag".into(),
            value: value.into(),
            count,
        }
    }
    fn title(id: i64, name: &str, score: f32) -> CollapsedHit {
        CollapsedHit {
            series: false,
            id,
            title: name.into(),
            kind: "comics".into(),
            cover_item_id: id,
            cover_version: format!("h{id}"),
            score,
        }
    }

    #[test]
    fn title_weight_rises_with_query_length() {
        assert!(suggest_title_weight(1) < 0.3, "short → titles demoted");
        assert!((suggest_title_weight(7) - 1.0).abs() < 0.2, "~parity mid");
        assert!(suggest_title_weight(12) >= 1.4, "long → titles can win");
    }

    #[test]
    fn blend_short_query_puts_tags_first() {
        let tags = vec![tag("office lady", 300)];
        let titles = vec![title(1, "Of Mice and Men", 5.0)];
        let out = blend_suggestions(&tags, &titles, 2, 10);
        assert!(
            matches!(out[0], Suggestion::Tag { .. }),
            "tag first on a short query"
        );
        assert!(out
            .iter()
            .any(|s| matches!(s, Suggestion::Title { id: 1, .. })));
    }

    #[test]
    fn blend_long_query_lets_title_climb() {
        let tags = vec![tag("offer", 50)];
        let titles = vec![title(1, "Of Mice and Men", 5.0)];
        let out = blend_suggestions(&tags, &titles, 9, 10);
        assert!(
            matches!(out[0], Suggestion::Title { .. }),
            "title climbs on a long query"
        );
    }

    #[test]
    fn blend_group_floor_keeps_a_title() {
        let tags: Vec<_> = (0..8).map(|i| tag(&format!("t{i}"), 100 - i)).collect();
        let titles = vec![title(1, "Rare Title", 0.1)];
        let out = blend_suggestions(&tags, &titles, 2, 6);
        assert!(
            out.iter().any(|s| matches!(s, Suggestion::Title { .. })),
            "floor guarantees a title slot"
        );
    }

    #[test]
    fn blend_respects_limit_and_empty() {
        assert!(blend_suggestions(&[], &[], 3, 10).is_empty());
        let tags: Vec<_> = (0..10).map(|i| tag(&format!("t{i}"), 10 - i)).collect();
        assert_eq!(blend_suggestions(&tags, &[], 5, 3).len(), 3, "honors limit");
    }
}
