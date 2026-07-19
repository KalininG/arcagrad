//! Library management: listing, kinds, upload, delete, and download.

use super::*;
use crate::server::auth;

/// Listing page size.
pub(crate) const DEFAULT_LIMIT: i64 = 50;

pub(crate) const MAX_LIMIT: i64 = 200;

/// Maximum BM25 candidates considered by relevance-sorted listings.
pub(crate) const RELEVANCE_CAP: usize = 500;

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct ListParams {
    /// Page size (default 50, capped at 200).
    limit: Option<i64>,
    /// Keyset Next: an opaque cursor from a previous response's `next_cursor`.
    cursor: Option<String>,
    /// Keyset Prev: an opaque cursor from a previous response's `prev_cursor`.
    before: Option<String>,
    /// Keyset Last: `true` jumps to the oldest page.
    last: Option<bool>,
    /// Jump to a 1-based page. Mutually exclusive with cursor pagination.
    page: Option<i64>,
    /// Full-text query over title + tags. Composes with `tags` (search ∩ tags)
    /// and every nav mode; tokens are prefix-matched (as-you-type).
    q: Option<String>,
    /// Comma-separated `namespace:value` filters. Prefix a tag with `-` to
    /// exclude matching items.
    tags: Option<String>,
    /// Comma-separated tags to exclude. One matching exclusion hides the item.
    exclude: Option<String>,
    /// `all` (default, AND) or `any` (OR) across the requested tags.
    #[serde(rename = "match")]
    match_mode: Option<String>,
    /// Sort by `added_at`, `title`, `page_count`, `creator`, or `relevance`.
    /// Relevance requires `q` and uses page-based pagination.
    sort: Option<String>,
    /// Sort direction: `asc` or `desc`. Default is per-column (added_at/page_count
    /// desc, title/creator asc). A `?cursor=`/`?before=` from a different sort is a 400.
    order: Option<String>,
    /// Restrict to the caller's favorites (`true`) or non-favorites (`false`).
    /// Omitted = all items. Composes with search/tags/sort/pagination.
    favorited: Option<bool>,
    /// Restrict to untagged items (`true`, items with no tags at all) or tagged
    /// items (`false`). Omitted = all. Composes with everything else.
    untagged: Option<bool>,
    /// Restrict to the caller's completed/finished items (`true`) or not-yet-
    /// completed ones (`false`, unread or in progress). Omitted = all. Per-user.
    completed: Option<bool>,
    /// Restrict to one kind (top-level folder name, e.g. `manga`). Omitted = all
    /// kinds. The UI's per-tab query; composes with search/tags/sort/pagination.
    kind: Option<String>,
}

/// `(namespace, value)` pairs from a comma-separated tag list.
type TagPairs = Vec<(String, String)>;

/// Parse `namespace:value,-namespace:value` into `(include, exclude)` pairs. A
/// leading `-` (booru-style) excludes. Malformed bits are dropped.
pub(crate) fn parse_tag_pairs(raw: &str) -> (TagPairs, TagPairs) {
    let (mut include, mut exclude) = (Vec::new(), Vec::new());
    for t in raw.split(',') {
        let t = t.trim();
        let (negated, t) = match t.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, t),
        };
        if let Some((ns, value)) = t.split_once(':') {
            let (ns, value) = (ns.trim(), value.trim());
            if !ns.is_empty() && !value.is_empty() {
                let pair = (ns.to_string(), value.to_string());
                if negated { &mut exclude } else { &mut include }.push(pair);
            }
        }
    }
    (include, exclude)
}

/// List the visible library with filters and bidirectional keyset pagination.
#[utoipa::path(
    get, path = "/api/items", tag = "library",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(ListParams),
    responses(
        (status = 200, description = "A page of items (keyset paginated)", body = ListResult),
        (status = 400, description = "Invalid cursor or parameters"),
        (status = 401, description = "Not authenticated"),
    ),
)]
pub(crate) async fn list_items(
    Viewer(user): Viewer,
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> Result<Json<ListResult>, AppError> {
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

    let nav = params.cursor.is_some() as u8
        + params.before.is_some() as u8
        + (params.last == Some(true)) as u8
        + params.page.is_some() as u8;
    if nav > 1 {
        return Err(AppError::BadRequest(
            "provide at most one of: cursor, before, last, page".into(),
        ));
    }

    let match_all = params.match_mode.as_deref() != Some("any");
    let (pairs, mut neg_pairs) = params
        .tags
        .as_deref()
        .map(parse_tag_pairs)
        .unwrap_or_default();
    if let Some(x) = params.exclude.as_deref() {
        let (bare, negated) = parse_tag_pairs(x);
        neg_pairs.extend(bare);
        neg_pairs.extend(negated);
    }
    let mut impossible = false;
    let mut include_ids = Vec::new();
    if !pairs.is_empty() {
        for (ns, value) in &pairs {
            match repo::tag_id(&state.read, ns, value).await? {
                Some(id) => include_ids.push(id),
                None if match_all => {
                    impossible = true;
                    break;
                }
                None => {}
            }
        }
        if !impossible && include_ids.is_empty() {
            impossible = true;
        }
    }
    let filter = (!pairs.is_empty() && !impossible).then(|| repo::TagFilter {
        tag_ids: include_ids.clone(),
        match_all,
    });

    let mut exclude_ids = Vec::new();
    for (ns, value) in &neg_pairs {
        if let Some(id) = repo::tag_id(&state.read, ns, value).await? {
            exclude_ids.push(id);
        }
    }
    for id in repo::blocklist_tag_ids(&state.read, user.id).await? {
        if !include_ids.contains(&id) {
            exclude_ids.push(id);
        }
    }

    let search = params.q.as_deref().and_then(repo::fts_query);

    let relevance = params.sort.as_deref() == Some("relevance");
    let field = match params.sort.as_deref() {
        None | Some("relevance") => repo::SortField::AddedAt,
        Some(s) => repo::SortField::parse(s)
            .ok_or_else(|| AppError::BadRequest(format!("unknown sort '{s}'")))?,
    };
    let descending = match params.order.as_deref() {
        None => field.default_descending(),
        Some("desc") => true,
        Some("asc") => false,
        Some(o) => return Err(AppError::BadRequest(format!("unknown order '{o}'"))),
    };
    let sort = repo::Sort { field, descending };

    let mut filters = repo::ListFilters {
        tags: filter,
        exclude_tags: exclude_ids,
        search,
        favorited: params.favorited,
        untagged: params.untagged,
        completed: params.completed,
        kind: params.kind.filter(|k| !k.is_empty()),
        deny_kinds: auth::hidden_kinds_for(&state.read, &user).await?,
    };

    if relevance {
        let q = params
            .q
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| AppError::BadRequest("sort=relevance requires a query (?q=)".into()))?;
        if impossible {
            return Ok(Json(ListResult::empty()));
        }
        let ranked = state
            .search
            .search_ids(q, RELEVANCE_CAP)
            .map_err(AppError::Internal)?;
        let ranked_ids: Vec<i64> = ranked.into_iter().map(|(id, _)| id).collect();
        filters.search = None;
        let page = params.page.unwrap_or(1).max(1);
        let offset = (page - 1) * limit;
        let (entries, total) =
            repo::relevance_page(&state.read, user.id, &ranked_ids, offset, limit, &filters)
                .await?;
        let page_count = if total == 0 {
            0
        } else {
            (total + limit - 1) / limit
        };
        return Ok(Json(ListResult {
            items: entries,
            prev_cursor: None,
            next_cursor: None,
            total: Some(total),
            page: Some(page.min(page_count.max(1))),
            page_count: Some(page_count),
        }));
    }

    if let Some(req_page) = params.page {
        if req_page < 1 {
            return Err(AppError::BadRequest("page must be >= 1".into()));
        }
        let total = if impossible {
            0
        } else {
            repo::count_catalog(&state.read, user.id, &filters).await?
        };
        let page_count = (total + limit - 1) / limit;
        let page = if page_count == 0 {
            1
        } else {
            req_page.min(page_count)
        };
        let mut result = if total == 0 {
            ListResult::empty()
        } else {
            let offset = (page - 1) * limit;
            if offset > 0 {
                tracing::debug!(
                    offset,
                    page,
                    page_count,
                    "offset page-jump (O(depth)); sequential nav should use cursors"
                );
            }
            repo::list_catalog(
                &state.read,
                user.id,
                limit,
                repo::CatalogSeek::Offset(offset),
                sort,
                &filters,
            )
            .await?
        };
        result.total = Some(total);
        result.page = Some(page);
        result.page_count = Some(page_count);
        return Ok(Json(result));
    }

    if impossible {
        return Ok(Json(ListResult::empty()));
    }
    let decode = |raw: &str, what: &str| -> Result<repo::Cursor, AppError> {
        let cur = repo::decode_cursor(raw)
            .ok_or_else(|| AppError::BadRequest(format!("invalid {what}")))?;
        if cur.sort != sort.signature() {
            return Err(AppError::BadRequest(format!(
                "{what} does not match the requested sort"
            )));
        }
        Ok(cur)
    };
    let seek = if let Some(c) = params.cursor.as_deref() {
        let cur = decode(c, "cursor")?;
        repo::CatalogSeek::After {
            value: cur.value,
            typ: cur.typ.unwrap_or_default(),
            id: cur.id,
        }
    } else if let Some(b) = params.before.as_deref() {
        let cur = decode(b, "before cursor")?;
        repo::CatalogSeek::Before {
            value: cur.value,
            typ: cur.typ.unwrap_or_default(),
            id: cur.id,
        }
    } else if params.last == Some(true) {
        repo::CatalogSeek::Last
    } else {
        repo::CatalogSeek::First
    };
    Ok(Json(
        repo::list_catalog(&state.read, user.id, limit, seek, sort, &filters).await?,
    ))
}

/// Distinct kinds (top-level folder names) with item counts, driving the client's
/// per-kind tabs. Any authed user.
#[utoipa::path(
    get, path = "/api/kinds", tag = "library",
    security(("sessionCookie" = []), ("apiKey" = [])),
    responses(
        (status = 200, description = "Kinds present, with item counts, by name", body = Vec<repo::KindCount>),
        (status = 401, description = "Not authenticated"),
    ),
)]
pub(crate) async fn list_kinds(
    Viewer(user): Viewer,
    State(state): State<AppState>,
) -> Result<Json<Vec<repo::KindCount>>, AppError> {
    let hidden = auth::hidden_kinds_for(&state.read, &user).await?;
    let mut kinds = repo::list_kinds(&state.read).await?;
    if !hidden.is_empty() {
        kinds.retain(|k| !hidden.contains(&k.kind));
    }
    Ok(Json(kinds))
}

/// Monotonic counter for unique upload temp-file names (concurrent uploads can't
/// collide on the same `.tmp`).
pub(crate) static UPLOAD_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Result of an upload: the item's integer `id` plus whether a new item was
/// created (`false` = an identical file already existed and was returned instead).
#[derive(Serialize, ToSchema)]
pub(crate) struct CreatedItem {
    id: i64,
    title: String,
    kind: String,
    created: bool,
}

/// Documentation schema for the streamed multipart item upload.
#[allow(dead_code)]
#[derive(ToSchema)]
pub(crate) struct ItemUploadForm {
    #[schema(value_type = String, format = Binary)]
    file: String,
    /// Open kind/top-level folder name. Defaults to `uncategorized`.
    kind: Option<String>,
}

/// Upload a supported archive as multipart `file`, with an optional `kind`.
/// The filename is sanitized and structural identity deduplicates existing media.
#[utoipa::path(
    post, path = "/api/items", tag = "library",
    security(("sessionCookie" = []), ("apiKey" = [])),
    request_body(content = ItemUploadForm, description = "`file` is required; `kind` is optional", content_type = "multipart/form-data"),
    responses(
        (status = 201, description = "Item created", body = CreatedItem),
        (status = 200, description = "Identical file already existed; existing item returned", body = CreatedItem),
        (status = 400, description = "Missing/invalid file, bad kind, or not a readable archive"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
    ),
)]
pub(crate) async fn create_item(
    _admin: AdminUser,
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<CreatedItem>), AppError> {
    let content_dir = state.config.content_dir.clone();
    let mut kind = scanner::DEFAULT_KIND.to_string();
    let mut up: Option<StreamedUpload> = None;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("malformed multipart: {e}")))?
    {
        match field.name() {
            Some("kind") => {
                kind = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("read kind field: {e}")))?;
            }
            Some("file") => {
                let filename = field.file_name().map(|s| s.to_string());
                if let Some(f) = &filename {
                    if !crate::media::format::is_uploadable(std::path::Path::new(f)) {
                        return Err(AppError::BadRequest(format!(
                            "unsupported file type — expected one of: {}",
                            crate::media::format::uploadable_exts().join(", ")
                        )));
                    }
                }
                up = Some(stream_to_temp(&content_dir, &mut field, filename).await?);
            }
            _ => {
                // Drain ignored multipart fields before reading the next one.
                while matches!(field.chunk().await, Ok(Some(_))) {}
            }
        }
    }

    let up = up.ok_or_else(|| AppError::BadRequest("missing `file` part".into()))?;

    let kind = library::safe_filename(&kind)
        .ok_or_else(|| AppError::BadRequest(format!("invalid kind {kind:?}")))?;

    let outcome = finalize_upload(&state, &content_dir, &kind, &up).await;
    let _ = tokio::fs::remove_file(&up.temp).await;
    outcome
}

/// A `file` field streamed to a temp file: its path and the client's filename (if
/// any). Identity/dedup is computed by the ingest step from the file itself.
pub(crate) struct StreamedUpload {
    temp: std::path::PathBuf,
    filename: Option<String>,
}

/// Stream an upload to a same-filesystem temporary file while hashing it.
pub(crate) async fn stream_to_temp(
    content_dir: &std::path::Path,
    field: &mut axum::extract::multipart::Field<'_>,
    filename: Option<String>,
) -> Result<StreamedUpload, AppError> {
    let seq = UPLOAD_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = content_dir.join(format!(".arca-upload-{seq}.tmp"));
    let mut file = tokio::fs::File::create(&temp)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("create temp: {e}")))?;
    let mut size: u64 = 0;
    loop {
        match field.chunk().await {
            Ok(Some(chunk)) => {
                size += chunk.len() as u64;
                if let Err(e) = file.write_all(&chunk).await {
                    let _ = tokio::fs::remove_file(&temp).await;
                    return Err(AppError::Internal(anyhow::anyhow!("write temp: {e}")));
                }
            }
            Ok(None) => break,
            Err(e) => {
                let _ = tokio::fs::remove_file(&temp).await;
                return Err(AppError::BadRequest(format!("read file field: {e}")));
            }
        }
    }
    let _ = file.flush().await;
    drop(file);
    if size == 0 {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(AppError::BadRequest("empty file".into()));
    }
    Ok(StreamedUpload { temp, filename })
}

/// Validate, deduplicate, atomically commit, and ingest an upload.
pub(crate) async fn finalize_upload(
    state: &AppState,
    content_dir: &std::path::Path,
    kind: &str,
    up: &StreamedUpload,
) -> Result<(StatusCode, Json<CreatedItem>), AppError> {
    let result = library::ingest_committed_temp(
        &state.read,
        &state.write,
        content_dir,
        kind,
        &up.temp,
        up.filename.as_deref(),
        now_secs(),
    )
    .await
    .map_err(|e| match e {
        library::IngestError::BadArchive(m) => AppError::BadRequest(m),
        library::IngestError::Internal(e) => AppError::Internal(e),
    })?;

    if result.created {
        if let Err(e) = jobs::enqueue_ingest_followup(state, &[result.id]).await {
            tracing::warn!("upload {} ingest follow-up failed: {e:#}", result.id);
        }
    }
    let status = if result.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((
        status,
        Json(CreatedItem {
            id: result.id,
            title: result.title,
            kind: result.kind,
            created: result.created,
        }),
    ))
}

/// Permanently delete an item's source file, database row, and derived media.
/// This operation is admin-only and irreversible.
#[utoipa::path(
    delete, path = "/api/items/{id}", tag = "library",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = i64, Path, description = "Item id")),
    responses(
        (status = 200, description = "Item + its file and all derived data deleted", body = OkResponse),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
        (status = 404, description = "Unknown item"),
        (status = 500, description = "Source file could not be removed"),
    ),
)]
pub(crate) async fn delete_item(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<OkResponse>, AppError> {
    let meta = repo::item_meta(&state.read, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let leaf_series = repo::leaf_series_map(&state.read, &[id])
        .await
        .ok()
        .and_then(|m| m.get(&id).copied());

    match tokio::fs::remove_file(&meta.path).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(AppError::Internal(anyhow::anyhow!(
                "could not remove file {}: {e}",
                meta.path
            )))
        }
    }

    repo::delete_item(&state.write, id).await?;

    thumbnail::remove_item(&state.config.data_dir, &meta.structural_hash).await;

    state.clear_recommendation_caches();
    if let Some(sid) = leaf_series {
        let _ = jobs::enqueue_entry_recompute_for_series(&state, sid).await;
    }
    if let Err(e) = jobs::enqueue_reindex_search(&state.write, &[], &[id]).await {
        tracing::warn!("failed to queue search delete for item {id}: {e:?}");
    }
    Ok(ok())
}

/// Stream the original archive for offline reading or backup. Kind visibility
/// rules still apply.
#[utoipa::path(
    get, path = "/api/items/{id}/download", tag = "library",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = i64, Path, description = "Item id")),
    responses(
        (status = 200, description = "The original file, as an attachment", content(
            (Vec<u8> = "application/zip"),
            (Vec<u8> = "application/octet-stream")
        )),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "Unknown item"),
    ),
)]
pub(crate) async fn download_item_file(
    Viewer(user): Viewer,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let meta = repo::item_meta(&state.read, id)
        .await?
        .ok_or(AppError::NotFound)?;
    ensure_kind_visible(&state, &user, &meta.kind).await?;
    let path = std::path::PathBuf::from(&meta.path);
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("open {}: {e}", meta.path)))?;
    let len = file
        .metadata()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("stat {}: {e}", meta.path)))?
        .len();
    let filename = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("item-{id}.cbz"));
    let ascii: String = filename
        .chars()
        .map(|c| {
            if c.is_ascii() && c != '"' && c != '\\' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let encoded: String = filename
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect();
    let mime = match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("zip") | Some("cbz") | Some("epub") => "application/zip",
        _ => "application/octet-stream",
    };
    let body = axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(file));
    Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_LENGTH, len)
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}"),
        )
        .body(body)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("response build: {e}")))
}
