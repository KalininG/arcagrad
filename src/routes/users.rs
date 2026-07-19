//! User administration (admin).

use super::*;
use crate::server::auth;

/// One row of the admin user list: the account, its avatar version (the custom pfp
/// cache-buster; null = initial chip), and activity metrics for the table columns.
#[derive(Serialize, ToSchema)]
pub(crate) struct UserEntry {
    #[serde(flatten)]
    info: auth::UserInfo,
    avatar_version: Option<i64>,
    /// Items this user has finished.
    finished: i64,
    /// Items this user has favorited.
    favorites: i64,
    /// Newest read-progress touch (unix secs); null = never active.
    last_active: Option<i64>,
    /// Unexpired sessions for this user.
    sessions: i64,
}

/// Every account, for the admin accounts table. Admin-only. Carries per-user activity
/// metrics (finished / favorites / last active / sessions) alongside the account.
#[utoipa::path(
    get, path = "/api/users", tag = "users",
    security(("sessionCookie" = []), ("apiKey" = [])),
    responses(
        (status = 200, description = "All user accounts", body = Vec<UserEntry>),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
    ),
)]
pub(crate) async fn list_users(
    _admin: AdminUser,
    State(state): State<AppState>,
) -> Result<Json<Vec<UserEntry>>, AppError> {
    let metrics = auth::user_metrics(&state.read, now_secs()).await?;
    let mut out = Vec::new();
    for info in auth::list_users(&state.read).await? {
        let avatar_version = avatar_mtime(&avatar_path(&state, info.id)).await;
        let m = metrics.get(&info.id).copied().unwrap_or_default();
        out.push(UserEntry {
            info,
            avatar_version,
            finished: m.finished,
            favorites: m.favorites,
            last_active: m.last_active,
            sessions: m.sessions,
        });
    }
    Ok(Json(out))
}

/// Aggregate tiles for the admin accounts page (active/dormant, sessions, reads
/// today). Admin-only.
#[utoipa::path(
    get, path = "/api/users/stats", tag = "users",
    security(("sessionCookie" = []), ("apiKey" = [])),
    responses(
        (status = 200, description = "Account activity tiles", body = auth::AdminUserStats),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
    ),
)]
pub(crate) async fn users_stats(
    _admin: AdminUser,
    State(state): State<AppState>,
) -> Result<Json<auth::AdminUserStats>, AppError> {
    Ok(Json(auth::admin_user_stats(&state.read, now_secs()).await?))
}

/// A new account to create (admin-only manual creation).
#[derive(Deserialize, ToSchema)]
pub(crate) struct NewUser {
    username: String,
    password: String,
    /// `user` (default) or `admin`.
    #[serde(default)]
    role: Option<String>,
}

/// Create a user account (admin). Role is `user` or `admin`; a duplicate
/// username is a 409.
#[utoipa::path(
    post, path = "/api/users", tag = "users",
    security(("sessionCookie" = []), ("apiKey" = [])),
    request_body = NewUser,
    responses(
        (status = 200, description = "Account created", body = auth::UserInfo),
        (status = 400, description = "Invalid username, password, or role"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
        (status = 409, description = "Username already taken"),
    ),
)]
pub(crate) async fn create_user_route(
    _admin: AdminUser,
    State(state): State<AppState>,
    Json(body): Json<NewUser>,
) -> Result<Json<auth::UserInfo>, AppError> {
    let username = body.username.trim().to_string();
    if username.is_empty() || username.len() > 50 {
        return Err(AppError::BadRequest(
            "username must be 1–50 characters".into(),
        ));
    }
    if body.password.len() < 8 {
        return Err(AppError::BadRequest(
            "password must be at least 8 characters".into(),
        ));
    }
    let role = body.role.as_deref().unwrap_or("user");
    if role != "user" && role != "admin" {
        return Err(AppError::BadRequest(
            "role must be 'user' or 'admin'".into(),
        ));
    }
    let id = match auth::create_user(&state.write, &username, &body.password, role).await {
        Ok(id) => id,
        Err(e) if e.to_string().contains("UNIQUE") => {
            return Err(AppError::Conflict("username already taken".into()));
        }
        Err(e) => return Err(e.into()),
    };
    Ok(Json(auth::UserInfo {
        id,
        username,
        role: role.to_string(),
        created_at: auth::user_created_at(&state.read, id).await?.unwrap_or(0),
    }))
}

/// Delete a user and their personal state. The current user and last admin
/// cannot be deleted.
#[utoipa::path(
    delete, path = "/api/users/{id}", tag = "users",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = i64, Path, description = "User id")),
    responses(
        (status = 200, description = "Account deleted", body = OkResponse),
        (status = 400, description = "Refused: yourself, or the last admin"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
        (status = 404, description = "Unknown user"),
    ),
)]
pub(crate) async fn delete_user_route(
    AdminUser(admin): AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<OkResponse>, AppError> {
    if id == admin.id {
        return Err(AppError::BadRequest(
            "you can't delete your own account".into(),
        ));
    }
    let Some(role) = auth::user_role(&state.read, id).await? else {
        return Err(AppError::NotFound);
    };
    if role == "admin" && auth::count_admins(&state.read).await? <= 1 {
        return Err(AppError::BadRequest(
            "the last admin account can't be deleted".into(),
        ));
    }
    if !auth::delete_user(&state.write, id).await? {
        return Err(AppError::NotFound);
    }
    let _ = tokio::fs::remove_file(avatar_path(&state, id)).await;
    let _ = tokio::fs::remove_file(banner_path(&state, id)).await;
    Ok(ok())
}

/// A password to set on someone's account (admin reset, no current needed).
#[derive(Deserialize, ToSchema)]
pub(crate) struct ResetPasswordBody {
    new: String,
}

/// Admin password reset for a user who forgot theirs: sets the new password and
/// ends every session of that user (they sign back in with the new one).
#[utoipa::path(
    put, path = "/api/users/{id}/password", tag = "users",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = i64, Path, description = "User id")),
    request_body = ResetPasswordBody,
    responses(
        (status = 200, description = "Password reset; the user's sessions ended", body = OkResponse),
        (status = 400, description = "Invalid password"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
        (status = 404, description = "Unknown user"),
    ),
)]
pub(crate) async fn reset_user_password(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<ResetPasswordBody>,
) -> Result<Json<OkResponse>, AppError> {
    if body.new.len() < 8 {
        return Err(AppError::BadRequest(
            "password must be at least 8 characters".into(),
        ));
    }
    if !auth::admin_set_password(&state.write, id, &body.new).await? {
        return Err(AppError::NotFound);
    }
    Ok(ok())
}

/// Any user's avatar, by id (the Settings → Users list). Admin-only, matching
/// the list itself.
#[utoipa::path(
    get, path = "/api/users/{id}/avatar", tag = "users",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = i64, Path, description = "User id")),
    responses(
        (status = 200, description = "The avatar", body = Vec<u8>, content_type = "image/webp"),
        (status = 304, description = "Not modified"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
        (status = 404, description = "No avatar set"),
    ),
)]
pub(crate) async fn get_user_avatar(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    serve_profile_image(avatar_path(&state, id), "av", &headers).await
}
