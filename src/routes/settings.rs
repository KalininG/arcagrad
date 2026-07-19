//! Server settings (admin).

use super::*;
use crate::server::auth;

/// The admin access toggles (Settings → Users).
#[derive(Serialize, Deserialize, ToSchema)]
pub(crate) struct AuthSettings {
    signup_enabled: bool,
    guest_enabled: bool,
}

/// Read the access toggles. Admin-only (the public flags ride /api/auth/status).
#[utoipa::path(
    get, path = "/api/settings/auth", tag = "settings",
    security(("sessionCookie" = []), ("apiKey" = [])),
    responses(
        (status = 200, description = "Current access toggles", body = AuthSettings),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
    ),
)]
pub(crate) async fn get_auth_settings(
    _admin: AdminUser,
    State(state): State<AppState>,
) -> Result<Json<AuthSettings>, AppError> {
    Ok(Json(AuthSettings {
        signup_enabled: auth::get_bool_setting(&state.read, auth::SETTING_SIGNUP_ENABLED, false)
            .await?,
        guest_enabled: auth::get_bool_setting(&state.read, auth::SETTING_GUEST_ENABLED, false)
            .await?,
    }))
}

/// Set the access toggles. Admin-only.
#[utoipa::path(
    put, path = "/api/settings/auth", tag = "settings",
    security(("sessionCookie" = []), ("apiKey" = [])),
    request_body = AuthSettings,
    responses(
        (status = 200, description = "Toggles saved", body = OkResponse),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
    ),
)]
pub(crate) async fn put_auth_settings(
    _admin: AdminUser,
    State(state): State<AppState>,
    Json(body): Json<AuthSettings>,
) -> Result<Json<OkResponse>, AppError> {
    auth::set_setting(
        &state.write,
        auth::SETTING_SIGNUP_ENABLED,
        if body.signup_enabled { "1" } else { "0" },
    )
    .await?;
    auth::set_setting(
        &state.write,
        auth::SETTING_GUEST_ENABLED,
        if body.guest_enabled { "1" } else { "0" },
    )
    .await?;
    Ok(ok())
}

/// Kinds hidden from users and guests. Admins are always unrestricted.
#[derive(Serialize, Deserialize, ToSchema)]
pub(crate) struct KindAccess {
    /// Kinds hidden from signed-in non-admin accounts.
    user: Vec<String>,
    /// Kinds hidden from anonymous guests.
    guest: Vec<String>,
}

/// Read the per-kind visibility blocklists. Admin-only.
#[utoipa::path(
    get, path = "/api/settings/kind-access", tag = "settings",
    security(("sessionCookie" = []), ("apiKey" = [])),
    responses(
        (status = 200, description = "Hidden kinds per audience", body = KindAccess),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
    ),
)]
pub(crate) async fn get_kind_access(
    _admin: AdminUser,
    State(state): State<AppState>,
) -> Result<Json<KindAccess>, AppError> {
    let mut out = KindAccess {
        user: Vec::new(),
        guest: Vec::new(),
    };
    for (audience, kind) in auth::hidden_kinds_all(&state.read).await? {
        match audience.as_str() {
            "user" => out.user.push(kind),
            "guest" => out.guest.push(kind),
            _ => {}
        }
    }
    Ok(Json(out))
}

/// Replace the complete user and guest kind blocklists. Unknown historical kinds
/// are retained but ignored until they exist again.
#[utoipa::path(
    put, path = "/api/settings/kind-access", tag = "settings",
    security(("sessionCookie" = []), ("apiKey" = [])),
    request_body = KindAccess,
    responses(
        (status = 200, description = "Visibility saved", body = OkResponse),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
    ),
)]
pub(crate) async fn put_kind_access(
    _admin: AdminUser,
    State(state): State<AppState>,
    Json(body): Json<KindAccess>,
) -> Result<Json<OkResponse>, AppError> {
    let clean = |v: Vec<String>| -> Vec<String> {
        let mut v: Vec<String> = v
            .into_iter()
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
            .collect();
        v.sort();
        v.dedup();
        v
    };
    auth::set_hidden_kinds(&state.write, "user", &clean(body.user)).await?;
    auth::set_hidden_kinds(&state.write, "guest", &clean(body.guest)).await?;
    Ok(ok())
}
