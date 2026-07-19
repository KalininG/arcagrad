//! Password, session, and API-key authentication.

use anyhow::{anyhow, Result};
use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, Uri};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::Serialize;
use sqlx::SqlitePool;
use utoipa::ToSchema;

use crate::server::error::AppError;
use crate::AppState;

const COOKIE_NAME: &str = "arca_session";
pub const SESSION_TTL_SECS: i64 = 30 * 24 * 60 * 60; // 30 days

/// An authenticated user.
#[derive(Clone, Serialize, ToSchema)]
pub struct AuthUser {
    pub id: i64,
    pub username: String,
    pub role: String,
}

fn argon() -> Argon2<'static> {
    // OWASP Argon2id baseline: m=19456 KiB, t=2, p=1.
    let params = Params::new(19456, 2, 1, None).expect("static argon2 params are valid");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = argon()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow!("argon2 hash: {e}"))?;
    Ok(hash.to_string())
}

fn verify_password(password: &str, phc: &str) -> bool {
    match PasswordHash::new(phc) {
        Ok(parsed) => argon()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn token_hash(token: &str) -> String {
    blake3::hash(token.as_bytes()).to_hex().to_string()
}

fn now_unix() -> i64 {
    crate::now_secs()
}

pub async fn count_users(pool: &SqlitePool) -> Result<i64> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await?;
    Ok(n)
}

pub async fn create_user(
    pool: &SqlitePool,
    username: &str,
    password: &str,
    role: &str,
) -> Result<i64> {
    let pw = password.to_string();
    let hash = tokio::task::spawn_blocking(move || hash_password(&pw)).await??;
    let now = now_unix();
    let id = sqlx::query(
        "INSERT INTO users (username, password_hash, role, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(username)
    .bind(hash)
    .bind(role)
    .bind(now)
    .execute(pool)
    .await?
    .last_insert_rowid();
    Ok(id)
}

pub async fn create_first_admin(
    pool: &SqlitePool,
    username: &str,
    password: &str,
) -> Result<Option<i64>> {
    let pw = password.to_string();
    let hash = tokio::task::spawn_blocking(move || hash_password(&pw)).await??;
    let now = now_unix();
    let result = sqlx::query(
        "INSERT INTO users (username, password_hash, role, created_at) \
         SELECT ?, ?, 'admin', ? \
         WHERE NOT EXISTS (SELECT 1 FROM users)",
    )
    .bind(username)
    .bind(hash)
    .bind(now)
    .execute(pool)
    .await?;
    Ok((result.rows_affected() == 1).then(|| result.last_insert_rowid()))
}

pub async fn authenticate(
    pool: &SqlitePool,
    username: &str,
    password: &str,
) -> Result<Option<AuthUser>> {
    let row: Option<(i64, String, String, String)> =
        sqlx::query_as("SELECT id, username, password_hash, role FROM users WHERE username = ?")
            .bind(username)
            .fetch_optional(pool)
            .await?;

    let Some((id, username, hash, role)) = row else {
        // Keep unknown-user timing close to a failed password check.
        let pw = password.to_string();
        let dummy = dummy_hash().to_string();
        let _ = tokio::task::spawn_blocking(move || verify_password(&pw, &dummy)).await;
        return Ok(None);
    };

    let pw = password.to_string();
    let ok = tokio::task::spawn_blocking(move || verify_password(&pw, &hash)).await?;
    Ok(ok.then_some(AuthUser { id, username, role }))
}

/// Hash used to equalize unknown-user login timing.
fn dummy_hash() -> &'static str {
    static H: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    H.get_or_init(|| hash_password("timing-equalization-placeholder").expect("hash dummy"))
}

pub async fn get_setting(pool: &SqlitePool, key: &str) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT value FROM server_settings WHERE key = ?")
            .bind(key)
            .fetch_optional(pool)
            .await?,
    )
}

pub async fn set_setting(pool: &SqlitePool, key: &str, value: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO server_settings (key, value) VALUES (?, ?) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_bool_setting(pool: &SqlitePool, key: &str, default: bool) -> Result<bool> {
    Ok(get_setting(pool, key)
        .await?
        .as_deref()
        .and_then(crate::server::config::parse_bool)
        .unwrap_or(default))
}

pub const SETTING_SIGNUP_ENABLED: &str = "signup_enabled";
pub const SETTING_GUEST_ENABLED: &str = "guest_enabled";

/// `None` means the user is unrestricted.
pub fn audience_of(user: &AuthUser) -> Option<&'static str> {
    match user.role.as_str() {
        "admin" => None,
        "guest" => Some("guest"),
        _ => Some("user"),
    }
}

pub async fn hidden_kinds_for(pool: &SqlitePool, user: &AuthUser) -> Result<Vec<String>> {
    let Some(audience) = audience_of(user) else {
        return Ok(Vec::new());
    };
    Ok(
        sqlx::query_scalar("SELECT kind FROM kind_access WHERE audience = ?")
            .bind(audience)
            .fetch_all(pool)
            .await?,
    )
}

pub async fn hidden_kinds_all(pool: &SqlitePool) -> Result<Vec<(String, String)>> {
    Ok(
        sqlx::query_as("SELECT audience, kind FROM kind_access ORDER BY audience, kind")
            .fetch_all(pool)
            .await?,
    )
}

pub async fn set_hidden_kinds(pool: &SqlitePool, audience: &str, kinds: &[String]) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM kind_access WHERE audience = ?")
        .bind(audience)
        .execute(&mut *tx)
        .await?;
    for kind in kinds {
        sqlx::query("INSERT OR IGNORE INTO kind_access (audience, kind) VALUES (?, ?)")
            .bind(audience)
            .bind(kind)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

// Process-local throttles; login failures are keyed by username.

const LOGIN_FAIL_LIMIT: usize = 10;
const LOGIN_FAIL_WINDOW_SECS: i64 = 300;
const REGISTER_LIMIT: usize = 20;
const REGISTER_WINDOW_SECS: i64 = 600;

type Stamps = std::collections::HashMap<String, Vec<i64>>;
static LOGIN_FAILURES: std::sync::LazyLock<std::sync::Mutex<Stamps>> =
    std::sync::LazyLock::new(Default::default);
static REGISTRATIONS: std::sync::LazyLock<std::sync::Mutex<Vec<i64>>> =
    std::sync::LazyLock::new(Default::default);

pub fn login_throttled(username: &str) -> bool {
    let now = now_unix();
    let mut map = match LOGIN_FAILURES.lock() {
        Ok(g) => g,
        Err(_) => return false,
    };
    match map.get_mut(username) {
        Some(stamps) => {
            stamps.retain(|&t| now - t < LOGIN_FAIL_WINDOW_SECS);
            stamps.len() >= LOGIN_FAIL_LIMIT
        }
        None => false,
    }
}

pub fn note_login_failure(username: &str) {
    let now = now_unix();
    if let Ok(mut map) = LOGIN_FAILURES.lock() {
        map.entry(username.to_string()).or_default().push(now);
    }
}

pub fn clear_login_failures(username: &str) {
    if let Ok(mut map) = LOGIN_FAILURES.lock() {
        map.remove(username);
    }
}

pub fn register_allowed() -> bool {
    let now = now_unix();
    let mut stamps = match REGISTRATIONS.lock() {
        Ok(g) => g,
        Err(_) => return true,
    };
    stamps.retain(|&t| now - t < REGISTER_WINDOW_SECS);
    if stamps.len() >= REGISTER_LIMIT {
        return false;
    }
    stamps.push(now);
    true
}

#[derive(Serialize, sqlx::FromRow, ToSchema)]
pub struct UserInfo {
    pub id: i64,
    pub username: String,
    pub role: String,
    pub created_at: i64,
}

pub async fn list_users(pool: &SqlitePool) -> Result<Vec<UserInfo>> {
    Ok(
        sqlx::query_as("SELECT id, username, role, created_at FROM users ORDER BY id")
            .fetch_all(pool)
            .await?,
    )
}

#[derive(Default, Clone, Copy)]
pub struct UserMetrics {
    pub finished: i64,
    pub favorites: i64,
    pub last_active: Option<i64>,
    pub sessions: i64,
}

pub async fn user_metrics(
    pool: &SqlitePool,
    now: i64,
) -> Result<std::collections::HashMap<i64, UserMetrics>> {
    use std::collections::HashMap;
    let mut m: HashMap<i64, UserMetrics> = HashMap::new();
    let finished: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT rp.user_id, COUNT(*) FROM read_progress rp JOIN items i ON i.id = rp.item_id \
         WHERE (rp.unit = 'page' AND i.page_count IS NOT NULL AND rp.value >= i.page_count - 1) \
            OR (rp.unit = 'percent' AND rp.value >= 0.98) \
         GROUP BY rp.user_id",
    )
    .fetch_all(pool)
    .await?;
    for (uid, n) in finished {
        m.entry(uid).or_default().finished = n;
    }
    let favs: Vec<(i64, i64)> =
        sqlx::query_as("SELECT user_id, COUNT(*) FROM favorites GROUP BY user_id")
            .fetch_all(pool)
            .await?;
    for (uid, n) in favs {
        m.entry(uid).or_default().favorites = n;
    }
    let last: Vec<(i64, i64)> =
        sqlx::query_as("SELECT user_id, MAX(updated_at) FROM read_progress GROUP BY user_id")
            .fetch_all(pool)
            .await?;
    for (uid, ts) in last {
        m.entry(uid).or_default().last_active = Some(ts);
    }
    let sess: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT user_id, COUNT(*) FROM sessions WHERE expires_at > ? GROUP BY user_id",
    )
    .bind(now)
    .fetch_all(pool)
    .await?;
    for (uid, n) in sess {
        m.entry(uid).or_default().sessions = n;
    }
    Ok(m)
}

#[derive(Serialize, ToSchema, Default)]
pub struct AdminUserStats {
    pub active_today: i64,
    pub active_week: i64,
    pub dormant_90: i64,
    pub open_sessions: i64,
    pub sign_ins_today: i64,
}

pub async fn admin_user_stats(pool: &SqlitePool, now: i64) -> Result<AdminUserStats> {
    let today = now - now.rem_euclid(86_400);
    let week = now - 7 * 86_400;
    let d90 = now - 90 * 86_400;
    let (active_today, active_week, dormant_90): (i64, i64, i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(CASE WHEN la >= ? THEN 1 ELSE 0 END), 0), \
                COALESCE(SUM(CASE WHEN la >= ? THEN 1 ELSE 0 END), 0), \
                COALESCE(SUM(CASE WHEN la IS NULL OR la < ? THEN 1 ELSE 0 END), 0) \
         FROM (SELECT (SELECT MAX(updated_at) FROM read_progress rp WHERE rp.user_id = u.id) AS la \
               FROM users u)",
    )
    .bind(today)
    .bind(week)
    .bind(d90)
    .fetch_one(pool)
    .await?;
    let open_sessions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE expires_at > ?")
            .bind(now)
            .fetch_one(pool)
            .await?;
    let sign_ins_today: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE created_at >= ?")
            .bind(today)
            .fetch_one(pool)
            .await?;
    Ok(AdminUserStats {
        active_today,
        active_week,
        dormant_90,
        open_sessions,
        sign_ins_today,
    })
}

pub async fn user_role(pool: &SqlitePool, user_id: i64) -> Result<Option<String>> {
    Ok(sqlx::query_scalar("SELECT role FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await?)
}

pub async fn count_admins(pool: &SqlitePool) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role = 'admin'")
            .fetch_one(pool)
            .await?,
    )
}

/// Caller enforces self-delete and last-admin guards.
pub async fn delete_user(pool: &SqlitePool, user_id: i64) -> Result<bool> {
    let res = sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn admin_set_password(pool: &SqlitePool, user_id: i64, new: &str) -> Result<bool> {
    if user_role(pool, user_id).await?.is_none() {
        return Ok(false);
    }
    let pw = new.to_string();
    let hash = tokio::task::spawn_blocking(move || hash_password(&pw)).await??;
    sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(hash)
        .bind(user_id)
        .execute(pool)
        .await?;
    delete_all_sessions(pool, user_id).await?;
    Ok(true)
}

pub async fn user_created_at(pool: &SqlitePool, user_id: i64) -> Result<Option<i64>> {
    Ok(
        sqlx::query_scalar("SELECT created_at FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_optional(pool)
            .await?,
    )
}

pub async fn change_password(
    pool: &SqlitePool,
    user_id: i64,
    current: &str,
    new: &str,
) -> Result<bool> {
    let row: Option<(String,)> = sqlx::query_as("SELECT password_hash FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    let Some((hash,)) = row else {
        return Ok(false);
    };
    let cur = current.to_string();
    let ok = tokio::task::spawn_blocking(move || verify_password(&cur, &hash)).await?;
    if !ok {
        return Ok(false);
    }
    let pw = new.to_string();
    let new_hash = tokio::task::spawn_blocking(move || hash_password(&pw)).await??;
    sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(new_hash)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(true)
}

pub async fn delete_other_sessions(
    pool: &SqlitePool,
    user_id: i64,
    keep_token: &str,
) -> Result<u64> {
    let res = sqlx::query("DELETE FROM sessions WHERE user_id = ? AND token_hash != ?")
        .bind(user_id)
        .bind(token_hash(keep_token))
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// API keys are unchanged.
pub async fn delete_all_sessions(pool: &SqlitePool, user_id: i64) -> Result<u64> {
    let res = sqlx::query("DELETE FROM sessions WHERE user_id = ?")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

pub async fn create_session(pool: &SqlitePool, user_id: i64) -> Result<String> {
    let token = generate_token();
    let now = now_unix();
    // Purge expired rows on login.
    sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
        .bind(now)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)",
    )
    .bind(token_hash(&token))
    .bind(user_id)
    .bind(now)
    .bind(now + SESSION_TTL_SECS)
    .execute(pool)
    .await?;
    Ok(token)
}

pub async fn lookup_session(pool: &SqlitePool, token: &str) -> Result<Option<AuthUser>> {
    let row: Option<(i64, String, String)> = sqlx::query_as(
        "SELECT u.id, u.username, u.role FROM sessions s \
         JOIN users u ON u.id = s.user_id \
         WHERE s.token_hash = ? AND s.expires_at > ?",
    )
    .bind(token_hash(token))
    .bind(now_unix())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(id, username, role)| AuthUser { id, username, role }))
}

pub async fn delete_session(pool: &SqlitePool, token: &str) -> Result<()> {
    sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
        .bind(token_hash(token))
        .execute(pool)
        .await?;
    Ok(())
}

const API_KEY_PREFIX: &str = "arca_";

/// API-key metadata without the secret.
#[derive(Serialize, sqlx::FromRow, ToSchema)]
pub struct ApiKeyInfo {
    pub id: i64,
    pub label: String,
    pub created_at: i64,
    /// Recorded at five-minute granularity.
    pub last_used: Option<i64>,
}

pub async fn create_api_key(pool: &SqlitePool, user_id: i64, label: &str) -> Result<(i64, String)> {
    let raw = format!("{API_KEY_PREFIX}{}", generate_token());
    let id = sqlx::query(
        "INSERT INTO api_keys (user_id, token_hash, label, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(user_id)
    .bind(token_hash(&raw))
    .bind(label)
    .bind(now_unix())
    .execute(pool)
    .await?
    .last_insert_rowid();
    Ok((id, raw))
}

pub async fn lookup_api_key(pool: &SqlitePool, token: &str) -> Result<Option<AuthUser>> {
    let row: Option<(i64, String, String)> = sqlx::query_as(
        "SELECT u.id, u.username, u.role FROM api_keys k \
         JOIN users u ON u.id = k.user_id \
         WHERE k.token_hash = ?",
    )
    .bind(token_hash(token))
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(id, username, role)| AuthUser { id, username, role }))
}

pub async fn list_api_keys(pool: &SqlitePool, user_id: i64) -> Result<Vec<ApiKeyInfo>> {
    Ok(sqlx::query_as(
        "SELECT id, label, created_at, last_used FROM api_keys WHERE user_id = ? ORDER BY id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?)
}

const LAST_USED_GRANULARITY_SECS: i64 = 300;

/// Update an API key's last-use timestamp without blocking authentication.
pub fn touch_api_key(write: &SqlitePool, token: &str) {
    static RECENT: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, i64>>> =
        std::sync::LazyLock::new(Default::default);

    let hash = token_hash(token);
    let now = now_unix();
    {
        let mut recent = match RECENT.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        match recent.get(&hash) {
            Some(&at) if now - at < LAST_USED_GRANULARITY_SECS => return,
            _ => recent.insert(hash.clone(), now),
        };
    }

    let write = write.clone();
    tokio::spawn(async move {
        let _ = sqlx::query(
            "UPDATE api_keys SET last_used = ? \
             WHERE token_hash = ? AND COALESCE(last_used, 0) < ?",
        )
        .bind(now)
        .bind(hash)
        .bind(now - LAST_USED_GRANULARITY_SECS)
        .execute(&write)
        .await;
    });
}

pub async fn delete_api_key(pool: &SqlitePool, user_id: i64, key_id: i64) -> Result<bool> {
    let res = sqlx::query("DELETE FROM api_keys WHERE id = ? AND user_id = ?")
        .bind(key_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub fn cookie_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';')
        .filter_map(|p| p.trim().strip_prefix(&format!("{COOKIE_NAME}=")))
        .find(|v| !v.is_empty())
        .map(str::to_string)
}

pub fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = raw.split_once(' ')?;
    (scheme.eq_ignore_ascii_case("bearer") && !token.trim().is_empty())
        .then(|| token.trim().to_string())
}

/// Read an API key from a media URL's `token` query parameter.
pub fn query_token(uri: &Uri) -> Option<String> {
    uri.query()?
        .split('&')
        .filter_map(|p| p.split_once('='))
        .find(|(k, _)| *k == "token")
        .map(|(_, v)| v.to_string())
        .filter(|v| !v.is_empty())
}

/// Read an API key inherited by an EPUB resource path.
pub fn path_token(uri: &Uri) -> Option<String> {
    let after = uri.path().split_once("/@t/")?.1;
    let token = after.split('/').next()?;
    (!token.is_empty()).then(|| token.to_string())
}

pub fn build_cookie(secure: bool, token: &str, max_age: i64) -> String {
    let mut c = format!("{COOKIE_NAME}={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}");
    if secure {
        c.push_str("; Secure");
    }
    c
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // An invalid Bearer key must not fall through to cookie auth.
        if let Some(key) = bearer_token(&parts.headers) {
            let user = lookup_api_key(&state.read, &key)
                .await?
                .ok_or(AppError::Unauthorized)?;
            touch_api_key(&state.write, &key);
            return Ok(user);
        }
        // Media requests may carry a key in the query or EPUB resource path.
        for extract in [query_token, path_token] {
            if let Some(key) = extract(&parts.uri) {
                if let Some(user) = lookup_api_key(&state.read, &key).await? {
                    touch_api_key(&state.write, &key);
                    return Ok(user);
                }
            }
        }
        let token = cookie_token(&parts.headers).ok_or(AppError::Unauthorized)?;
        lookup_session(&state.read, &token)
            .await?
            .ok_or(AppError::Unauthorized)
    }
}

pub struct SessionUser(pub AuthUser);

impl FromRequestParts<AppState> for SessionUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = cookie_token(&parts.headers).ok_or(AppError::Unauthorized)?;
        let user = lookup_session(&state.read, &token)
            .await?
            .ok_or(AppError::Unauthorized)?;
        Ok(SessionUser(user))
    }
}

pub struct AdminUser(pub AuthUser);

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, state).await?;
        if user.role != "admin" {
            return Err(AppError::Forbidden);
        }
        Ok(AdminUser(user))
    }
}

pub const GUEST_ID: i64 = 0;

pub fn guest_user() -> AuthUser {
    AuthUser {
        id: GUEST_ID,
        username: "guest".into(),
        role: "guest".into(),
    }
}

/// An authenticated user or an enabled guest viewer.
pub struct Viewer(pub AuthUser);

impl FromRequestParts<AppState> for Viewer {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        match AuthUser::from_request_parts(parts, state).await {
            Ok(user) => Ok(Viewer(user)),
            Err(AppError::Unauthorized) => {
                // Explicit invalid credentials never downgrade to guest access.
                if bearer_token(&parts.headers).is_some() {
                    return Err(AppError::Unauthorized);
                }
                if get_bool_setting(&state.read, SETTING_GUEST_ENABLED, false).await? {
                    Ok(Viewer(guest_user()))
                } else {
                    Err(AppError::Unauthorized)
                }
            }
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_roundtrip() {
        let h = hash_password("correct horse battery staple").unwrap();
        assert!(verify_password("correct horse battery staple", &h));
        assert!(!verify_password("wrong", &h));
    }

    #[test]
    fn query_token_extracts_only_the_token_param() {
        let t = |q: &str| query_token(&format!("/api/items/1/thumbnail?{q}").parse().unwrap());
        assert_eq!(t("token=arca_AbC-1_9"), Some("arca_AbC-1_9".to_string()));
        assert_eq!(t("v=deadbeef&token=arca_x9"), Some("arca_x9".to_string()));
        assert_eq!(t("token=arca_x9&v=deadbeef"), Some("arca_x9".to_string()));
        assert_eq!(t("v=deadbeef"), None);
        assert_eq!(t("token="), None);
        assert_eq!(
            query_token(&"/api/items/1/thumbnail".parse().unwrap()),
            None
        );
        assert_eq!(t("xtoken=nope"), None);
    }

    #[test]
    fn path_token_extracts_the_at_t_segment() {
        let t = |p: &str| path_token(&p.parse().unwrap());
        assert_eq!(
            t("/api/items/1/resource/@t/arca_AbC-1_9/@v/deadbeef/OEBPS/Text/p_0001.xhtml"),
            Some("arca_AbC-1_9".to_string())
        );
        assert_eq!(
            t("/api/items/1/resource/@t/arca_x9/@v/deadbeef/OEBPS/Images/x.jpg"),
            Some("arca_x9".to_string())
        );
        assert_eq!(t("/api/items/1/resource/@v/deadbeef/OEBPS/x.xhtml"), None);
        assert_eq!(t("/api/items/1/thumbnail"), None);
    }

    #[test]
    fn tokens_are_unique_and_hashed() {
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b);
        assert_ne!(token_hash(&a), a);
        assert_eq!(token_hash(&a), token_hash(&a));
    }

    #[test]
    fn cookie_parsing() {
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            "foo=1; arca_session=abc123; bar=2".parse().unwrap(),
        );
        assert_eq!(cookie_token(&h).as_deref(), Some("abc123"));
        let empty = HeaderMap::new();
        assert_eq!(cookie_token(&empty), None);
    }

    #[sqlx::test]
    async fn session_lifecycle(pool: SqlitePool) {
        let id = create_user(&pool, "admin", "password123", "admin")
            .await
            .unwrap();
        assert_eq!(count_users(&pool).await.unwrap(), 1);

        assert!(authenticate(&pool, "admin", "nope")
            .await
            .unwrap()
            .is_none());
        assert!(authenticate(&pool, "ghost", "password123")
            .await
            .unwrap()
            .is_none());
        let user = authenticate(&pool, "admin", "password123")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(user.id, id);

        let token = create_session(&pool, id).await.unwrap();
        assert_eq!(lookup_session(&pool, &token).await.unwrap().unwrap().id, id);

        delete_session(&pool, &token).await.unwrap();
        assert!(lookup_session(&pool, &token).await.unwrap().is_none());
    }

    #[sqlx::test]
    async fn first_admin_creation_is_atomic(pool: SqlitePool) {
        let (a, b) = tokio::join!(
            create_first_admin(&pool, "first", "password123"),
            create_first_admin(&pool, "second", "password123"),
        );
        let created = [a.unwrap(), b.unwrap()]
            .into_iter()
            .filter(Option::is_some)
            .count();
        assert_eq!(created, 1);
        assert_eq!(count_users(&pool).await.unwrap(), 1);
    }

    #[sqlx::test]
    async fn user_metrics_and_admin_stats(pool: SqlitePool) {
        let uid = create_user(&pool, "u", "password123", "user")
            .await
            .unwrap();
        let now = 1_800_000_000_i64;
        let item: i64 = sqlx::query_scalar(
            "INSERT INTO items (scheme_tag, structural_hash, path, size_bytes, mtime, format, \
             title, page_count, added_at, last_modified_at) \
             VALUES ('zip-structural-v1','h','/p/h',1,1,'cbz','t',10,1,0) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO read_progress (user_id, item_id, unit, value, updated_at) VALUES (?, ?, 'page', 9, ?)")
            .bind(uid).bind(item).bind(now).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO favorites (user_id, item_id, created_at) VALUES (?, ?, ?)")
            .bind(uid)
            .bind(item)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO sessions (token_hash, user_id, created_at, expires_at) VALUES ('tok', ?, ?, ?)")
            .bind(uid).bind(now).bind(now + 3600).execute(&pool).await.unwrap();

        let m = user_metrics(&pool, now).await.unwrap();
        let um = m.get(&uid).copied().unwrap();
        assert_eq!(um.finished, 1);
        assert_eq!(um.favorites, 1);
        assert_eq!(um.last_active, Some(now));
        assert_eq!(um.sessions, 1);

        let s = admin_user_stats(&pool, now).await.unwrap();
        assert_eq!((s.active_today, s.active_week, s.dormant_90), (1, 1, 0));
        assert_eq!(s.open_sessions, 1);
        assert_eq!(s.sign_ins_today, 1);
    }
}
