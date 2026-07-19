//! Accounts, sessions, and API keys.

use super::*;
use crate::server::auth;

/// First-run / session state (`/api/auth/status`).
#[derive(Serialize, ToSchema)]
pub(crate) struct AuthStatus {
    setup_required: bool,
    authenticated: bool,
    user: Option<MeResponse>,
    signup_enabled: bool,
    guest_enabled: bool,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct ApiKeyCreated {
    id: i64,
    label: String,
    key: String,
}

#[derive(Deserialize, ToSchema)]
pub(crate) struct Credentials {
    username: String,
    password: String,
}

pub(crate) fn validate(creds: &Credentials) -> Result<(), AppError> {
    if creds.username.trim().is_empty() {
        return Err(AppError::BadRequest("username is required".into()));
    }
    if creds.password.len() < 8 {
        return Err(AppError::BadRequest(
            "password must be at least 8 characters".into(),
        ));
    }
    Ok(())
}

/// Build a response that sets the session cookie and returns the user.
pub(crate) fn session_response(state: &AppState, token: String, user: AuthUser) -> Response {
    let cookie = auth::build_cookie(state.config.cookie_secure, &token, auth::SESSION_TTL_SECS);
    ([(header::SET_COOKIE, cookie)], Json(user)).into_response()
}

/// Tells an unauthenticated client whether to show first-run setup or login, and
/// reflects the current session. Intentionally public (no auth required).
#[utoipa::path(
    get, path = "/api/auth/status", tag = "auth", security(),
    responses((status = 200, description = "First-run / session status", body = AuthStatus)),
)]
pub(crate) async fn auth_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AuthStatus>, AppError> {
    let setup_required = auth::count_users(&state.read).await? == 0;
    let user = match auth::bearer_token(&headers) {
        Some(t) => auth::lookup_api_key(&state.read, &t).await?,
        None => match auth::cookie_token(&headers) {
            Some(t) => auth::lookup_session(&state.read, &t).await?,
            None => None,
        },
    };
    let signup_enabled =
        auth::get_bool_setting(&state.read, auth::SETTING_SIGNUP_ENABLED, false).await?;
    let guest_enabled =
        auth::get_bool_setting(&state.read, auth::SETTING_GUEST_ENABLED, false).await?;
    let user = match user {
        Some(u) => {
            let created_at = auth::user_created_at(&state.read, u.id).await?;
            let avatar_version = avatar_mtime(&avatar_path(&state, u.id)).await;
            let banner_version = avatar_mtime(&banner_path(&state, u.id)).await;
            Some(MeResponse {
                id: u.id,
                username: u.username,
                role: u.role,
                created_at,
                avatar_version,
                banner_version,
            })
        }
        None => None,
    };
    Ok(Json(AuthStatus {
        setup_required,
        authenticated: user.is_some(),
        user,
        signup_enabled,
        guest_enabled,
    }))
}

/// Self-signup: create a regular account and sign it in. Only when the admin
/// has enabled it (Settings → Users); globally throttled against abuse.
#[utoipa::path(
    post, path = "/api/auth/register", tag = "auth", security(),
    request_body = Credentials,
    responses(
        (status = 200, description = "Account created; session cookie set", body = AuthUser),
        (status = 400, description = "Invalid credentials"),
        (status = 403, description = "Self-signup is disabled"),
        (status = 409, description = "Username already taken"),
        (status = 429, description = "Too many registrations right now"),
    ),
)]
pub(crate) async fn register(
    State(state): State<AppState>,
    Json(creds): Json<Credentials>,
) -> Result<Response, AppError> {
    if !auth::get_bool_setting(&state.read, auth::SETTING_SIGNUP_ENABLED, false).await? {
        return Err(AppError::Forbidden);
    }
    validate(&creds)?;
    if !auth::register_allowed() {
        return Err(AppError::TooManyRequests);
    }
    let username = creds.username.trim().to_string();
    if username.len() > 50 {
        return Err(AppError::BadRequest(
            "username must be 1–50 characters".into(),
        ));
    }
    let id = match auth::create_user(&state.write, &username, &creds.password, "user").await {
        Ok(id) => id,
        Err(e) if e.to_string().contains("UNIQUE") => {
            return Err(AppError::Conflict("username already taken".into()));
        }
        Err(e) => return Err(e.into()),
    };
    let token = auth::create_session(&state.write, id).await?;
    Ok(session_response(
        &state,
        token,
        AuthUser {
            id,
            username,
            role: "user".into(),
        },
    ))
}

/// First-run only: create the admin account. 409 once any user exists.
#[utoipa::path(
    post, path = "/api/auth/setup", tag = "auth", security(),
    request_body = Credentials,
    responses(
        (status = 200, description = "Admin created; session cookie set", body = AuthUser),
        (status = 400, description = "Invalid credentials"),
        (status = 409, description = "Setup already completed"),
    ),
)]
pub(crate) async fn setup(
    State(state): State<AppState>,
    Json(creds): Json<Credentials>,
) -> Result<Response, AppError> {
    validate(&creds)?;
    let username = creds.username.trim().to_string();
    let Some(id) = auth::create_first_admin(&state.write, &username, &creds.password).await? else {
        return Err(AppError::Conflict("setup already completed".into()));
    };
    let token = auth::create_session(&state.write, id).await?;
    Ok(session_response(
        &state,
        token,
        AuthUser {
            id,
            username,
            role: "admin".into(),
        },
    ))
}

/// Authenticates and logs in a user.
#[utoipa::path(
    post, path = "/api/auth/login", tag = "auth", security(),
    request_body = Credentials,
    responses(
        (status = 200, description = "Authenticated; session cookie set", body = AuthUser),
        (status = 401, description = "Bad credentials"),
    ),
)]
pub(crate) async fn login(
    State(state): State<AppState>,
    Json(creds): Json<Credentials>,
) -> Result<Response, AppError> {
    let username = creds.username.trim();
    if auth::login_throttled(username) {
        return Err(AppError::TooManyRequests);
    }
    let Some(user) = auth::authenticate(&state.read, username, &creds.password).await? else {
        auth::note_login_failure(username);
        return Err(AppError::Unauthorized);
    };
    auth::clear_login_failures(username);
    let token = auth::create_session(&state.write, user.id).await?;
    Ok(session_response(&state, token, user))
}

/// Ends the current session of the calling user.
#[utoipa::path(
    post, path = "/api/auth/logout", tag = "auth", security(),
    responses((status = 200, description = "Logged out; cookie cleared", body = OkResponse)),
)]
pub(crate) async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    if let Some(t) = auth::cookie_token(&headers) {
        let _ = auth::delete_session(&state.write, &t).await;
    }
    let cookie = auth::build_cookie(state.config.cookie_secure, "", 0);
    Ok(([(header::SET_COOKIE, cookie)], ok()).into_response())
}

/// A password change to apply. Session-cookie only: a leaked API key must not
/// be able to take over the account.
#[derive(Deserialize, ToSchema)]
pub(crate) struct ChangePasswordBody {
    current: String,
    new: String,
}

/// Change the caller's password and revoke their other sessions. API keys and
/// the current session remain valid.
#[utoipa::path(
    put, path = "/api/auth/password", tag = "auth",
    security(("sessionCookie" = [])),
    request_body = ChangePasswordBody,
    responses(
        (status = 200, description = "Password changed; other sessions ended", body = OkResponse),
        (status = 400, description = "Wrong current password, or invalid new password"),
        (status = 401, description = "Not authenticated (cookie required)"),
    ),
)]
pub(crate) async fn change_password(
    SessionUser(user): SessionUser,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ChangePasswordBody>,
) -> Result<Json<OkResponse>, AppError> {
    if body.new.len() < 8 {
        return Err(AppError::BadRequest(
            "new password must be at least 8 characters".into(),
        ));
    }
    if body.new.len() > 512 {
        return Err(AppError::BadRequest("new password is too long".into()));
    }
    if !auth::change_password(&state.write, user.id, &body.current, &body.new).await? {
        return Err(AppError::BadRequest("current password is incorrect".into()));
    }
    if let Some(token) = auth::cookie_token(&headers) {
        auth::delete_other_sessions(&state.write, user.id, &token).await?;
    }
    Ok(ok())
}

/// End every session of the calling user, including this one ("log out
/// everywhere"). API keys stay valid. The response clears the cookie.
#[utoipa::path(
    post, path = "/api/auth/logout-all", tag = "auth",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "All sessions ended; cookie cleared", body = OkResponse),
        (status = 401, description = "Not authenticated (cookie required)"),
    ),
)]
pub(crate) async fn logout_all(
    SessionUser(user): SessionUser,
    State(state): State<AppState>,
) -> Result<Response, AppError> {
    auth::delete_all_sessions(&state.write, user.id).await?;
    let cookie = auth::build_cookie(state.config.cookie_secure, "", 0);
    Ok(([(header::SET_COOKIE, cookie)], ok()).into_response())
}

#[derive(Deserialize, ToSchema)]
pub(crate) struct NewKey {
    label: String,
}

/// Create an API key. Requires an interactive web login (`SessionUser`).
/// The raw key is returned once. Users must copy/save it then.
#[utoipa::path(
    post, path = "/api/auth/keys", tag = "auth",
    security(("sessionCookie" = [])),
    request_body = NewKey,
    responses(
        (status = 200, description = "Key created; raw secret returned once", body = ApiKeyCreated),
        (status = 400, description = "Invalid label"),
        (status = 401, description = "Not authenticated (cookie required)"),
    ),
)]
pub(crate) async fn create_key(
    SessionUser(user): SessionUser,
    State(state): State<AppState>,
    Json(body): Json<NewKey>,
) -> Result<Json<ApiKeyCreated>, AppError> {
    let label = body.label.trim();
    if label.is_empty() {
        return Err(AppError::BadRequest("label is required".into()));
    }
    if label.len() > 100 {
        return Err(AppError::BadRequest("label must be <= 100 chars".into()));
    }
    let (id, key) = auth::create_api_key(&state.write, user.id, label).await?;
    Ok(Json(ApiKeyCreated {
        id,
        label: label.to_string(),
        key,
    }))
}

/// List the caller's API keys (labels + metadata, never the secret). Web-only.
#[utoipa::path(
    get, path = "/api/auth/keys", tag = "auth",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "The caller's API keys", body = Vec<ApiKeyInfo>),
        (status = 401, description = "Not authenticated (cookie required)"),
    ),
)]
pub(crate) async fn list_keys(
    SessionUser(user): SessionUser,
    State(state): State<AppState>,
) -> Result<Json<Vec<ApiKeyInfo>>, AppError> {
    Ok(Json(auth::list_api_keys(&state.read, user.id).await?))
}

/// Revoke one of the caller's API keys. Web-only.
#[utoipa::path(
    delete, path = "/api/auth/keys/{id}", tag = "auth",
    security(("sessionCookie" = [])),
    params(("id" = i64, Path, description = "API key id")),
    responses(
        (status = 200, description = "Revoked", body = OkResponse),
        (status = 401, description = "Not authenticated (cookie required)"),
        (status = 404, description = "No such key for this user"),
    ),
)]
pub(crate) async fn revoke_key(
    SessionUser(user): SessionUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<OkResponse>, AppError> {
    if !auth::delete_api_key(&state.write, user.id, id).await? {
        return Err(AppError::NotFound);
    }
    Ok(ok())
}
