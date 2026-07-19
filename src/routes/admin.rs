//! Library maintenance: rescan, metrics, jobs, and metadata export/import.

use super::*;

#[derive(Serialize, ToSchema)]
pub(crate) struct RescanQueued {
    queued: bool,
    job_id: i64,
}

/// Queue a background rescan and return immediately (a worker drains it).
/// Admin-only
#[utoipa::path(
    post, path = "/api/rescan", tag = "admin",
    security(("sessionCookie" = []), ("apiKey" = [])),
    responses(
        (status = 200, description = "Scan job queued", body = RescanQueued),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
    ),
)]
pub(crate) async fn rescan(
    _admin: AdminUser,
    State(state): State<AppState>,
) -> Result<Json<RescanQueued>, AppError> {
    let job_id = jobs::enqueue(&state.write, "scan", None).await?;
    Ok(Json(RescanQueued {
        queued: true,
        job_id,
    }))
}

/// Return current library, job, storage, and system metrics. Rate fields are
/// calculated from the previous request rather than sampled in the background.
#[utoipa::path(
    get, path = "/api/metrics", tag = "admin",
    security(("sessionCookie" = []), ("apiKey" = [])),
    responses(
        (status = 200, description = "Server-health snapshot", body = crate::server::metrics::Metrics),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Admin role required"),
    ),
)]
pub(crate) async fn server_metrics(
    _admin: AdminUser,
    State(state): State<AppState>,
) -> Result<Json<metrics::Metrics>, AppError> {
    Ok(Json(metrics::collect(&state).await?))
}

/// A background job's persisted state and optional terminal result.
#[derive(Serialize, ToSchema)]
pub(crate) struct JobStatusResponse {
    id: i64,
    kind: String,
    state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
}

impl From<jobs::JobStatus> for JobStatusResponse {
    fn from(s: jobs::JobStatus) -> Self {
        JobStatusResponse {
            id: s.id,
            kind: s.kind,
            state: s.state,
            result: s.result.and_then(|r| serde_json::from_str(&r).ok()),
        }
    }
}

/// Poll an admin job. Returns 404 when the job is unknown or has been pruned.
#[utoipa::path(
    get, path = "/api/jobs/{id}", tag = "admin",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(
        ("id" = i64, Path, description = "Job id from a queueing response"),
        ("wait" = Option<bool>, Query, description =
            "Long-poll: hold the request until the job is terminal (or the server's \
             long-poll window elapses), so a slow job needs a couple of held requests \
             instead of a poll storm"),
    ),
    responses(
        (status = 200, description = "Job status", body = JobStatusResponse),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
        (status = 404, description = "Unknown or pruned job"),
    ),
)]
pub(crate) async fn job_status(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(params): Query<JobStatusParams>,
) -> Result<Json<JobStatusResponse>, AppError> {
    if params.wait {
        if let Some(s) = jobs::wait_for_terminal(&state.read, id, JOB_LONGPOLL).await? {
            return Ok(Json(s.into()));
        }
    }
    let status = jobs::status(&state.read, id)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(Json(status.into()))
}

pub(crate) const JOB_LONGPOLL: std::time::Duration = std::time::Duration::from_secs(25);

#[derive(Deserialize)]
pub(crate) struct JobStatusParams {
    /// Hold the request until the job is terminal (or the long-poll window
    /// elapses) instead of returning the current status immediately.
    #[serde(default)]
    wait: bool,
}
