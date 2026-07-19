//! Tauri host for the shared web client.

use serde::{Deserialize, Serialize};

#[cfg(not(target_os = "android"))]
const KEYRING_SERVICE: &str = "com.arcagrad.client";

#[cfg(target_os = "android")]
static ANDROID_DATA_DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

#[derive(Serialize, Deserialize, Clone)]
struct Session {
    server_url: String,
    token: String,
}

// Unsigned macOS builds cannot retain a stable Keychain grant, so macOS and
// Android use owner-only files. Other platforms use their native credential store.
#[cfg(not(any(target_os = "macos", target_os = "android")))]
mod token_store {
    use super::{Session, KEYRING_SERVICE};
    use keyring::Entry;

    const KEYRING_ACCOUNT: &str = "session";

    fn session() -> Result<Entry, String> {
        Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT).map_err(|e| e.to_string())
    }

    pub fn read_session() -> Option<Session> {
        serde_json::from_str(&session().ok()?.get_password().ok()?).ok()
    }

    pub fn write_session(s: &Session) -> Result<(), String> {
        let blob = serde_json::to_string(s).map_err(|e| e.to_string())?;
        session()?.set_password(&blob).map_err(|e| e.to_string())
    }

    pub fn clear_session() -> Result<(), String> {
        match session()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn read_install_id() -> Option<String> {
        Entry::new(KEYRING_SERVICE, "install-id")
            .ok()?
            .get_password()
            .ok()
    }

    pub fn write_install_id(id: &str) {
        if let Ok(e) = Entry::new(KEYRING_SERVICE, "install-id") {
            let _ = e.set_password(id);
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "android"))]
mod token_store {
    use super::Session;
    #[cfg(target_os = "macos")]
    use super::KEYRING_SERVICE;
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::PathBuf;

    fn dir() -> Option<PathBuf> {
        #[cfg(target_os = "android")]
        return super::ANDROID_DATA_DIR.get().cloned();
        #[cfg(target_os = "macos")]
        return Some(
            PathBuf::from(std::env::var_os("HOME")?)
                .join("Library/Application Support")
                .join(KEYRING_SERVICE),
        );
    }

    // Keep replacement atomic and owner-only.
    fn write_private(path: &PathBuf, bytes: &[u8]) -> Result<(), String> {
        let parent = path.parent().ok_or("bad path")?;
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("tmp");
        {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)
                .map_err(|e| e.to_string())?;
            f.write_all(bytes).map_err(|e| e.to_string())?;
            f.sync_all().ok();
        }
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    pub fn read_session() -> Option<Session> {
        let raw = std::fs::read_to_string(dir()?.join("session.json")).ok()?;
        serde_json::from_str(&raw).ok()
    }

    pub fn write_session(s: &Session) -> Result<(), String> {
        let path = dir().ok_or("no HOME")?.join("session.json");
        let blob = serde_json::to_string(s).map_err(|e| e.to_string())?;
        write_private(&path, blob.as_bytes())
    }

    pub fn clear_session() -> Result<(), String> {
        let Some(path) = dir().map(|d| d.join("session.json")) else {
            return Ok(());
        };
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn read_install_id() -> Option<String> {
        let id = std::fs::read_to_string(dir()?.join("install-id"))
            .ok()?
            .trim()
            .to_string();
        (!id.is_empty()).then_some(id)
    }

    pub fn write_install_id(id: &str) {
        if let Some(d) = dir() {
            let _ = write_private(&d.join("install-id"), id.as_bytes());
        }
    }
}

fn store(session: &Session) -> Result<(), String> {
    token_store::write_session(session)
}

// Prevent devices with the same hostname from sharing an API-key label.
fn install_id() -> String {
    if let Some(id) = token_store::read_install_id() {
        return id;
    }
    use std::hash::{BuildHasher, Hasher};
    let seed = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    let id = format!("{:08x}", seed as u32);
    token_store::write_install_id(&id);
    id
}

fn label() -> String {
    let host = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "client".into());
    format!("desktop-{host}-{}", install_id())
}

#[tauri::command]
async fn connect(
    server_url: String,
    username: String,
    password: String,
) -> Result<Session, String> {
    let base = server_url.trim().trim_end_matches('/').to_string();
    if base.is_empty() {
        return Err("Server URL is required.".into());
    }

    let client = reqwest::Client::builder()
        .cookie_store(true)
        .user_agent(concat!("arcagrad-client/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;

    let login = client
        .post(format!("{base}/api/auth/login"))
        .json(&serde_json::json!({ "username": username, "password": password }))
        .send()
        .await
        .map_err(|e| format!("Could not reach {base}: {e}"))?;
    if !login.status().is_success() {
        return Err(if login.status() == reqwest::StatusCode::UNAUTHORIZED {
            "Invalid username or password.".into()
        } else {
            format!("Login failed: HTTP {}", login.status())
        });
    }

    // Replace this device's previous key when reconnecting.
    #[derive(Deserialize)]
    struct KeyInfo {
        id: i64,
        label: String,
    }
    if let Ok(resp) = client.get(format!("{base}/api/auth/keys")).send().await {
        if let Ok(keys) = resp.json::<Vec<KeyInfo>>().await {
            for k in keys.iter().filter(|k| k.label == label()) {
                let _ = client
                    .delete(format!("{base}/api/auth/keys/{}", k.id))
                    .send()
                    .await;
            }
        }
    }

    let resp = client
        .post(format!("{base}/api/auth/keys"))
        .json(&serde_json::json!({ "label": label() }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!(
            "Could not create an API key: HTTP {}",
            resp.status()
        ));
    }
    #[derive(Deserialize)]
    struct KeyResponse {
        key: String,
    }
    let minted: KeyResponse = resp.json().await.map_err(|e| e.to_string())?;

    let session = Session {
        server_url: base,
        token: minted.key,
    };
    store(&session)?;
    Ok(session)
}

#[tauri::command]
fn stored_session() -> Option<Session> {
    token_store::read_session()
}

#[tauri::command]
fn disconnect() -> Result<(), String> {
    token_store::clear_session()
}

fn downloads_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    use tauri::Manager;
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("downloads");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn find_downloaded(dir: &std::path::Path, id: i64) -> Option<std::path::PathBuf> {
    let marker = format!("[{id}]");
    std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let p = e.path();
        p.file_stem()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.ends_with(&marker))
            .then_some(p)
    })
}

#[tauri::command]
fn item_downloaded(app: tauri::AppHandle, id: i64) -> bool {
    downloads_dir(&app)
        .ok()
        .and_then(|d| find_downloaded(&d, id))
        .is_some()
}

#[tauri::command]
fn remove_download(app: tauri::AppHandle, id: i64) -> Result<(), String> {
    let dir = downloads_dir(&app)?;
    if let Some(path) = find_downloaded(&dir, id) {
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// Download through a .part file so interrupted transfers are not treated as complete.
#[tauri::command]
async fn download_item(app: tauri::AppHandle, id: i64, name: String) -> Result<String, String> {
    use tauri::Emitter;

    let session = stored_session().ok_or("Not connected.")?;
    let dir = downloads_dir(&app)?;
    if let Some(existing) = find_downloaded(&dir, id) {
        return Ok(existing.to_string_lossy().into_owned());
    }

    let client = reqwest::Client::builder()
        .user_agent(concat!("arcagrad-client/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(format!("{}/api/items/{id}/download", session.server_url))
        .bearer_auth(&session.token)
        .send()
        .await
        .map_err(|e| format!("Could not reach the server: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Download failed: HTTP {}", resp.status()));
    }

    let ext = resp
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            let f = v.split("filename=\"").nth(1)?.split('"').next()?;
            Some(f.rsplit('.').next()?.to_ascii_lowercase())
        })
        .filter(|e| e.chars().all(|c| c.is_ascii_alphanumeric()) && e.len() <= 5)
        .unwrap_or_else(|| "cbz".into());
    let total = resp.content_length().unwrap_or(0);

    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_().,'".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim()
        .chars()
        .take(120)
        .collect();
    let final_path = dir.join(format!("{safe} [{id}].{ext}"));
    let part_path = dir.join(format!("{safe} [{id}].{ext}.part"));

    let mut file = tokio::fs::File::create(&part_path)
        .await
        .map_err(|e| e.to_string())?;
    let mut received: u64 = 0;
    let mut last_emit = std::time::Instant::now();
    let mut resp = resp;
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                use tokio::io::AsyncWriteExt;
                file.write_all(&chunk).await.map_err(|e| {
                    let _ = std::fs::remove_file(&part_path);
                    e.to_string()
                })?;
                received += chunk.len() as u64;
                // Avoid flooding the webview on fast transfers.
                if last_emit.elapsed().as_millis() >= 100 {
                    last_emit = std::time::Instant::now();
                    let _ = app.emit(
                        "arca:download-progress",
                        serde_json::json!({ "id": id, "received": received, "total": total }),
                    );
                }
            }
            Ok(None) => break,
            Err(e) => {
                let _ = std::fs::remove_file(&part_path);
                return Err(format!("Download interrupted: {e}"));
            }
        }
    }
    use tokio::io::AsyncWriteExt;
    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);
    tokio::fs::rename(&part_path, &final_path)
        .await
        .map_err(|e| e.to_string())?;
    let _ = app.emit(
        "arca:download-progress",
        serde_json::json!({ "id": id, "received": received, "total": total.max(received) }),
    );
    Ok(final_path.to_string_lossy().into_owned())
}

#[derive(serde::Serialize)]
struct UpdateInfo {
    version: String,
    current_version: String,
    notes: Option<String>,
}

#[tauri::command]
async fn check_for_update(app: tauri::AppHandle) -> Result<Option<UpdateInfo>, String> {
    #[cfg(desktop)]
    {
        use tauri_plugin_updater::UpdaterExt;
        let updater = app.updater().map_err(|e| e.to_string())?;
        let found = updater.check().await.map_err(|e| e.to_string())?;
        Ok(found.map(|u| UpdateInfo {
            version: u.version.clone(),
            current_version: u.current_version.clone(),
            notes: u.body.clone(),
        }))
    }
    #[cfg(not(desktop))]
    {
        let _ = app;
        Ok(None)
    }
}

#[tauri::command]
async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(desktop)]
    {
        use tauri_plugin_updater::UpdaterExt;
        let updater = app.updater().map_err(|e| e.to_string())?;
        let Some(update) = updater.check().await.map_err(|e| e.to_string())? else {
            return Ok(());
        };
        update
            .download_and_install(|_downloaded, _total| {}, || {})
            .await
            .map_err(|e| e.to_string())?;
        app.restart();
    }
    #[cfg(not(desktop))]
    {
        let _ = app;
        Err("auto-update is desktop-only — update through the app store".into())
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    // Let the web UI handle mobile safe-area padding.
    #[cfg(any(target_os = "android", target_os = "ios"))]
    let builder = builder.plugin(tauri_plugin_ios_webview_insets::init());
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    builder
        .setup(|app| {
            #[cfg(target_os = "android")]
            {
                use tauri::Manager;
                if let Ok(dir) = app.path().app_data_dir() {
                    let _ = ANDROID_DATA_DIR.set(dir);
                }
            }
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            connect,
            stored_session,
            disconnect,
            item_downloaded,
            download_item,
            remove_download,
            check_for_update,
            install_update
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
