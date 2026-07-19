//! Plugin discovery, store, repos, credentials, and per-kind enablement.

use super::*;

/// One credential field a client renders an input for.
#[derive(Serialize, ToSchema)]
pub(crate) struct PluginAuthField {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    secret: bool,
    required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    help: Option<String>,
}

/// Credential requirements a client surfaces when configuring a plugin.
#[derive(Serialize, ToSchema)]
pub(crate) struct PluginAuth {
    scheme: String,
    fields: Vec<PluginAuthField>,
    required_for: Vec<String>,
    /// Plugin-provided guide for obtaining the credential.
    #[serde(skip_serializing_if = "Option::is_none")]
    setup: Option<String>,
}

/// A running plugin available to client source selectors.
#[derive(Serialize, ToSchema)]
pub(crate) struct PluginInfo {
    id: String,
    /// Metadata-schema version; absent = a legacy prerelease manifest.
    #[serde(skip_serializing_if = "Option::is_none")]
    manifest_version: Option<u32>,
    /// `strict` (validated manifest v1) | `legacy`.
    metadata_status: String,
    /// Provenance, host-assigned (a plugin cannot claim it): `bundled` |
    /// `local` | `community`.
    origin: String,
    /// The artifact's release version (semver; legacy manifests report the
    /// compatibility placeholder "0.0.0"; clients hide it).
    version: String,
    /// Author/maintainer ("Unknown" placeholder for legacy manifests).
    author: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repository: Option<String>,
    name: String,
    /// One-line human summary for the enable/disable UI (blank if the plugin
    /// doesn't declare one).
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    source: String,
    capabilities: Vec<String>,
    /// Hosts the plugin may reach (empty = no allowlist), so users see its reach.
    hosts: Vec<String>,
    auth: Option<PluginAuth>,
    /// Browsable feeds exposed as source tabs.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    feeds: Vec<crate::plugins::scraper::Feed>,
    /// Whether "Follow a search" is meaningful here: false for a fixed catalog like
    /// Gutenberg.
    followable: bool,
    /// Default source reader: `paged` or `vertical`.
    reading_mode: String,
    /// Whether this source serves adult content.
    nsfw: bool,
    /// Source-specific label, placeholder and help for each capability's opaque
    /// reference input. Missing entries use neutral client wording.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    reference_inputs: std::collections::BTreeMap<String, PluginReferenceInput>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct PluginReferenceInput {
    label: String,
    placeholder: String,
    help: String,
    required: bool,
}

/// A plugin plus whether it's enabled for a specific kind: the row the per-kind
/// enable/disable screen renders (`GET /api/kinds/{kind}/plugins`).
#[derive(Serialize, ToSchema)]
pub(crate) struct KindPluginInfo {
    #[serde(flatten)]
    plugin: PluginInfo,
    /// Whether the admin has turned this plugin on for this kind.
    enabled: bool,
    /// Whether the plugin auto-runs when the watcher adds an item of this kind.
    auto: bool,
}

/// A configured credential source and which fields are set, never the values.
#[derive(Serialize, ToSchema)]
pub(crate) struct CredentialInfo {
    source: String,
    fields: Vec<String>,
}

/// Body for setting a source's credentials: field to value (e.g. `{"api_key":"…"}`).
#[derive(Deserialize, ToSchema)]
pub(crate) struct CredentialBody {
    data: std::collections::BTreeMap<String, String>,
}

#[derive(Deserialize)]
pub(crate) struct PluginQuery {
    capability: Option<String>,
}

/// List running plugins, optionally filtered by capability. This source-level
/// endpoint is not filtered by library kind.
#[utoipa::path(
    get, path = "/api/plugins", tag = "plugins",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(
        ("capability" = Option<String>, Query, description = "e.g. 'scrape' | 'download'"),
    ),
    responses((status = 200, description = "Matching plugins", body = [PluginInfo])),
)]
pub(crate) async fn list_plugins(
    _user: AuthUser,
    State(state): State<AppState>,
    Query(q): Query<PluginQuery>,
) -> Json<Vec<PluginInfo>> {
    let plugins = state
        .scrapers
        .manifests()
        .into_iter()
        .map(plugin_info)
        .filter(|p| {
            q.capability
                .as_ref()
                .is_none_or(|c| p.capabilities.iter().any(|x| x == c))
        })
        .collect();
    Json(plugins)
}

/// Plugin manifest and installation state for the store shelf.
#[derive(Serialize, ToSchema)]
pub(crate) struct CatalogPluginInfo {
    plugin: PluginInfo,
    installed: bool,
    /// Set when the installed artifact failed to load at boot; the store shows
    /// it broken with a reinstall path.
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
    /// For a community plugin: the repo it came from (for provenance display).
    /// Absent for bundled + local plugins.
    #[serde(skip_serializing_if = "Option::is_none")]
    repo_url: Option<String>,
    /// Newer repository version, when one is available.
    #[serde(skip_serializing_if = "Option::is_none")]
    update_available: Option<String>,
}

/// A configured plugin repository + its last fetch status (the manage-repos UI).
#[derive(Serialize, ToSchema)]
pub(crate) struct PluginRepoInfo {
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    added_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_fetched: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
}

/// Map a repo index entry's strict manifest to the client DTO (community plugins
/// are validated strict by construction; `origin` is host-assigned).
pub(crate) fn plugin_info_from_manifest(m: arcagrad_plugin_sdk::PluginManifest) -> PluginInfo {
    PluginInfo {
        id: m.id,
        manifest_version: Some(m.manifest_version),
        metadata_status: "strict".to_string(),
        origin: "community".to_string(),
        version: m.version,
        author: m.author,
        icon: m.icon,
        repository: m.repository,
        name: m.name,
        description: Some(m.description),
        source: m.source,
        capabilities: m
            .capabilities
            .into_iter()
            .filter(|c| crate::plugins::scraper::SUPPORTED_CAPABILITIES.contains(&c.as_str()))
            .collect(),
        hosts: m.hosts,
        feeds: m
            .feeds
            .into_iter()
            .map(|f| crate::plugins::scraper::Feed {
                id: f.id,
                label: f.label,
                ranges: f.ranges,
                query: f.query,
                auth: f.auth,
                cache_ttl: f.cache_ttl,
            })
            .collect(),
        followable: m.followable,
        reading_mode: m.reading_mode,
        nsfw: m.nsfw,
        reference_inputs: m
            .reference_inputs
            .into_iter()
            .map(|(capability, input)| {
                (
                    capability,
                    PluginReferenceInput {
                        label: input.label,
                        placeholder: input.placeholder,
                        help: input.help,
                        required: input.required,
                    },
                )
            })
            .collect(),
        auth: m.auth.map(|a| PluginAuth {
            scheme: a.scheme,
            fields: a
                .fields
                .into_iter()
                .map(|f| PluginAuthField {
                    name: f.name,
                    label: f.label,
                    secret: f.secret,
                    required: f.required,
                    help: f.help,
                })
                .collect(),
            required_for: a.required_for,
            setup: a.setup,
        }),
    }
}

/// List bundled store entries and loaded local plugins with installation state.
#[utoipa::path(
    get, path = "/api/plugin-catalog", tag = "plugins",
    security(("sessionCookie" = []), ("apiKey" = [])),
    responses(
        (status = 200, description = "Every available plugin + install state", body = [CatalogPluginInfo]),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
    ),
)]
pub(crate) async fn plugin_catalog(
    _admin: AdminUser,
    State(state): State<AppState>,
) -> Result<Json<Vec<CatalogPluginInfo>>, AppError> {
    let installs: std::collections::HashMap<String, crate::repo::PluginInstall> =
        repo::list_plugin_installs(&state.read)
            .await?
            .into_iter()
            .map(|r| (r.plugin_id.clone(), r))
            .collect();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<CatalogPluginInfo> = state
        .plugin_catalog
        .iter()
        .map(|entry| {
            let row = installs.get(&entry.manifest.id);
            seen.insert(entry.manifest.id.clone());
            CatalogPluginInfo {
                plugin: plugin_info(entry.manifest.clone()),
                installed: row.is_some(),
                last_error: row.and_then(|r| r.last_error.clone()),
                repo_url: None,
                update_available: None,
            }
        })
        .collect();
    for m in state.scrapers.manifests() {
        if seen.insert(m.id.clone()) {
            let row = installs.get(&m.id);
            let update_available = row
                .filter(|r| r.repo_url.is_some())
                .zip(state.marketplace.version_of(&m.id))
                .filter(|(r, v)| crate::plugins::marketplace::version_newer(v, &r.version))
                .map(|(_, v)| v);
            out.push(CatalogPluginInfo {
                plugin: plugin_info(m),
                installed: true,
                last_error: row.and_then(|r| r.last_error.clone()),
                repo_url: row.and_then(|r| r.repo_url.clone()),
                update_available,
            });
        }
    }
    for (repo_url, entry) in state.marketplace.entries() {
        let id = entry.manifest.id.clone();
        if !seen.insert(id.clone()) {
            continue;
        }
        let row = installs.get(&id);
        let has_icon = entry.icon_data.is_some();
        let mut plugin = plugin_info_from_manifest(entry.manifest);
        if has_icon && plugin.icon.is_none() {
            plugin.icon = Some(format!("/api/plugins/{id}/icon"));
        }
        out.push(CatalogPluginInfo {
            plugin,
            installed: row.is_some(),
            last_error: row.and_then(|r| r.last_error.clone()),
            repo_url: Some(repo_url),
            update_available: None,
        });
    }
    Ok(Json(out))
}

/// List configured plugin repositories + their fetch status (admin).
#[utoipa::path(
    get, path = "/api/plugin-repos", tag = "plugins",
    security(("sessionCookie" = []), ("apiKey" = [])),
    responses(
        (status = 200, description = "Configured repos", body = [PluginRepoInfo]),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
    ),
)]
pub(crate) async fn list_plugin_repos(
    _admin: AdminUser,
    State(state): State<AppState>,
) -> Result<Json<Vec<PluginRepoInfo>>, AppError> {
    let repos = repo::list_plugin_repos(&state.read)
        .await?
        .into_iter()
        .map(|r| PluginRepoInfo {
            url: r.url,
            name: r.name,
            added_at: r.added_at,
            last_fetched: r.last_fetched,
            last_error: r.last_error,
        })
        .collect();
    Ok(Json(repos))
}

#[derive(Deserialize, ToSchema)]
pub(crate) struct AddRepoBody {
    url: String,
}

/// Add a plugin repository after validating its index.
#[utoipa::path(
    post, path = "/api/plugin-repos", tag = "plugins",
    security(("sessionCookie" = []), ("apiKey" = [])),
    request_body = AddRepoBody,
    responses(
        (status = 200, description = "Repo added + fetched", body = OkResponse),
        (status = 400, description = "The URL didn't serve a valid repo index"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
    ),
)]
pub(crate) async fn add_plugin_repo(
    _admin: AdminUser,
    State(state): State<AppState>,
    Json(body): Json<AddRepoBody>,
) -> Result<Json<OkResponse>, AppError> {
    crate::plugins::marketplace::add_repo(&state, body.url.trim())
        .await
        .map_err(|e| AppError::BadRequest(format!("{e:#}")))?;
    Ok(ok())
}

/// Remove a plugin repository (admin). Every plugin installed from it is
/// uninstalled too (credentials + kind assignments are kept for a reinstall).
#[utoipa::path(
    delete, path = "/api/plugin-repos", tag = "plugins",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("url" = String, Query, description = "The repo URL to remove")),
    responses(
        (status = 200, description = "Removed", body = OkResponse),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
        (status = 404, description = "No such repo"),
    ),
)]
pub(crate) async fn remove_plugin_repo(
    _admin: AdminUser,
    State(state): State<AppState>,
    Query(q): Query<RemoveRepoQuery>,
) -> Result<Json<OkResponse>, AppError> {
    if !crate::plugins::marketplace::remove_repo(&state, &q.url).await? {
        return Err(AppError::NotFound);
    }
    Ok(ok())
}

#[derive(Deserialize)]
pub(crate) struct RemoveRepoQuery {
    url: String,
}

/// Re-fetch every configured repo (admin): the manual refresh button.
#[utoipa::path(
    post, path = "/api/plugin-repos/refresh", tag = "plugins",
    security(("sessionCookie" = []), ("apiKey" = [])),
    responses(
        (status = 200, description = "Refreshed (per-repo errors are in the repo list)", body = OkResponse),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
    ),
)]
pub(crate) async fn refresh_plugin_repos(
    _admin: AdminUser,
    State(state): State<AppState>,
) -> Result<Json<OkResponse>, AppError> {
    crate::plugins::marketplace::refresh_all(&state).await;
    Ok(ok())
}

/// Install request: `id` is a plugin id from the catalog shelf.
#[derive(Deserialize, ToSchema)]
pub(crate) struct InstallPluginBody {
    id: String,
}

/// Validate, install, and hot-load a catalog plugin. Existing credentials and
/// kind assignments for its id are retained.
#[utoipa::path(
    post, path = "/api/plugin-installs", tag = "plugins",
    security(("sessionCookie" = []), ("apiKey" = [])),
    request_body = InstallPluginBody,
    responses(
        (status = 200, description = "Installed and running", body = OkResponse),
        (status = 400, description = "Unknown plugin id, or the artifact failed to load"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
    ),
)]
pub(crate) async fn install_plugin(
    _admin: AdminUser,
    State(state): State<AppState>,
    Json(body): Json<InstallPluginBody>,
) -> Result<Json<OkResponse>, AppError> {
    let id = body.id.as_str();
    let result = if state.plugin_catalog.iter().any(|p| p.manifest.id == id) {
        crate::plugins::plugin_store::install(&state, id).await
    } else {
        crate::plugins::marketplace::install_from_repo(&state, id).await
    };
    result.map_err(|e| AppError::BadRequest(format!("{e:#}")))?;
    Ok(ok())
}

/// Unload and remove a managed plugin while retaining its configuration.
#[utoipa::path(
    delete, path = "/api/plugin-installs/{id}", tag = "plugins",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = String, Path, description = "Plugin id")),
    responses(
        (status = 200, description = "Uninstalled", body = OkResponse),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
        (status = 404, description = "Nothing installed under this id"),
    ),
)]
pub(crate) async fn uninstall_plugin(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<OkResponse>, AppError> {
    if !crate::plugins::plugin_store::uninstall(&state, &id)
        .await
        .map_err(|e| AppError::BadRequest(format!("{e:#}")))?
    {
        return Err(AppError::NotFound);
    }
    Ok(ok())
}

/// A plugin artifact is a few hundred KB; 32 MB is ample headroom.
pub(crate) const PLUGIN_FILE_MAX_BYTES: usize = 32 * 1024 * 1024;

#[derive(Serialize, ToSchema)]
pub(crate) struct InstalledFromFile {
    /// The plugin id read from the artifact's own manifest.
    id: String,
}

/// Documentation schema for the multipart plugin artifact upload.
#[allow(dead_code)]
#[derive(ToSchema)]
pub(crate) struct PluginFileUploadForm {
    #[schema(value_type = String, format = Binary)]
    file: String,
}

/// Install an uploaded WASM plugin. Bundled plugin ids are reserved.
#[utoipa::path(
    post, path = "/api/plugin-installs/file", tag = "plugins",
    security(("sessionCookie" = []), ("apiKey" = [])),
    request_body(content = PluginFileUploadForm, description = "`file` is the required .wasm artifact", content_type = "multipart/form-data"),
    responses(
        (status = 200, description = "Installed and running", body = InstalledFromFile),
        (status = 400, description = "Missing file, not a valid plugin artifact, or a reserved bundled id"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
    ),
)]
pub(crate) async fn install_plugin_file(
    _admin: AdminUser,
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<InstalledFromFile>, AppError> {
    let mut bytes: Option<Vec<u8>> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("malformed multipart: {e}")))?
    {
        if field.name() == Some("file") {
            let data = field
                .bytes()
                .await
                .map_err(|e| AppError::BadRequest(format!("reading upload: {e}")))?;
            bytes = Some(data.to_vec());
        }
    }
    let bytes = bytes.ok_or_else(|| AppError::BadRequest("missing `file` field".into()))?;
    if bytes.is_empty() {
        return Err(AppError::BadRequest("empty file".into()));
    }
    let id = crate::plugins::plugin_store::install_from_file(&state, bytes)
        .await
        .map_err(|e| AppError::BadRequest(format!("{e:#}")))?;
    Ok(Json(InstalledFromFile { id }))
}

/// Map a manifest to capabilities supported by this host.
pub(crate) fn plugin_info(m: crate::plugins::scraper::ScraperManifest) -> PluginInfo {
    PluginInfo {
        id: m.id,
        manifest_version: m.manifest_version,
        metadata_status: m.metadata_status,
        origin: m.origin,
        version: m.version,
        author: m.author,
        icon: m.icon,
        repository: m.repository,
        name: m.name,
        description: m.description,
        source: m.source,
        capabilities: m
            .capabilities
            .into_iter()
            .filter(|c| crate::plugins::scraper::SUPPORTED_CAPABILITIES.contains(&c.as_str()))
            .collect(),
        hosts: m.hosts,
        feeds: m.feeds,
        followable: m.followable,
        reading_mode: m.reading_mode,
        nsfw: m.nsfw,
        reference_inputs: m
            .reference_inputs
            .into_iter()
            .map(|(capability, input)| {
                (
                    capability,
                    PluginReferenceInput {
                        label: input.label,
                        placeholder: input.placeholder,
                        help: input.help,
                        required: input.required,
                    },
                )
            })
            .collect(),
        auth: m.auth.map(|a| PluginAuth {
            scheme: a.scheme,
            fields: a
                .fields
                .into_iter()
                .map(|f| PluginAuthField {
                    name: f.name,
                    label: f.label,
                    secret: f.secret,
                    required: f.required,
                    help: f.help,
                })
                .collect(),
            required_for: a.required_for,
            setup: a.setup,
        }),
    }
}

/// Return a plugin's embedded icon, or 404 when it has none.
#[utoipa::path(
    get, path = "/api/plugins/{id}/icon", tag = "plugins",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("id" = String, Path, description = "Plugin id")),
    responses(
        (status = 200, description = "Icon image bytes", content(
            (Vec<u8> = "image/webp"),
            (Vec<u8> = "image/png"),
            (Vec<u8> = "image/jpeg"),
            (Vec<u8> = "image/svg+xml"),
            (Vec<u8> = "application/octet-stream")
        )),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "Unknown plugin, or no embedded icon"),
    ),
)]
pub(crate) async fn plugin_icon(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let bytes = state
        .scrapers
        .get(&id)
        .and_then(|s| s.icon_bytes().map(<[u8]>::to_vec))
        .or_else(|| {
            state
                .plugin_catalog
                .iter()
                .find(|p| p.manifest.id == id)
                .and_then(|p| p.icon.clone())
        })
        .or_else(|| {
            use base64::Engine as _;
            state.marketplace.get(&id).and_then(|(_, e)| {
                e.icon_data
                    .as_deref()
                    .and_then(|d| base64::engine::general_purpose::STANDARD.decode(d).ok())
            })
        })
        .ok_or(AppError::NotFound)?;
    let content_type = if bytes.starts_with(b"RIFF") {
        "image/webp"
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else if bytes.starts_with(b"<svg") || bytes.starts_with(b"<?xml") {
        "image/svg+xml"
    } else {
        "application/octet-stream"
    };
    Ok((
        [
            (header::CONTENT_TYPE, content_type.to_string()),
            (header::CACHE_CONTROL, "private, max-age=86400".to_string()),
        ],
        bytes,
    )
        .into_response())
}

/// Body for `PUT /api/kinds/{kind}/plugins`: the exact set of plugin ids enabled
/// for the kind (replaces whatever was there). Empty = disable all for the kind.
#[derive(Deserialize, ToSchema)]
pub(crate) struct KindPluginsBody {
    plugin_ids: Vec<String>,
    /// Subset of `plugin_ids` to auto-run when the watcher adds an item of this
    /// kind. Must be a subset of `plugin_ids` (else 400). Omit for none.
    #[serde(default)]
    auto: Vec<String>,
}

/// List installed plugins and their enablement for one library kind.
#[utoipa::path(
    get, path = "/api/kinds/{kind}/plugins", tag = "plugins",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("kind" = String, Path, description = "Kind (top-level folder name)")),
    responses(
        (status = 200, description = "All plugins with per-kind enabled flags", body = [KindPluginInfo]),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
    ),
)]
pub(crate) async fn list_kind_plugins(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(kind): Path<String>,
) -> Result<Json<Vec<KindPluginInfo>>, AppError> {
    let enabled: std::collections::HashSet<String> = repo::plugins_for_kind(&state.read, &kind)
        .await?
        .into_iter()
        .collect();
    let auto: std::collections::HashSet<String> = repo::auto_plugins_for_kind(&state.read, &kind)
        .await?
        .into_iter()
        .collect();
    let plugins = state
        .scrapers
        .manifests()
        .into_iter()
        .map(|m| {
            let info = plugin_info(m);
            let is_on = enabled.contains(&info.id);
            let is_auto = auto.contains(&info.id);
            KindPluginInfo {
                plugin: info,
                enabled: is_on,
                auto: is_auto,
            }
        })
        .collect();
    Ok(Json(plugins))
}

/// Replace the enabled plugin set for a library kind. Unknown plugin ids return 400.
#[utoipa::path(
    put, path = "/api/kinds/{kind}/plugins", tag = "plugins",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("kind" = String, Path, description = "Kind (top-level folder name)")),
    request_body = KindPluginsBody,
    responses(
        (status = 200, description = "Saved", body = [KindPluginInfo]),
        (status = 400, description = "Unknown plugin id"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
    ),
)]
pub(crate) async fn set_kind_plugins(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(kind): Path<String>,
    Json(body): Json<KindPluginsBody>,
) -> Result<Json<Vec<KindPluginInfo>>, AppError> {
    let known: std::collections::HashSet<String> = state.scrapers.ids().into_iter().collect();
    if let Some(bad) = body.plugin_ids.iter().find(|id| !known.contains(*id)) {
        return Err(AppError::BadRequest(format!("unknown plugin '{bad}'")));
    }
    let enabled_set: std::collections::HashSet<&String> = body.plugin_ids.iter().collect();
    if let Some(bad) = body.auto.iter().find(|id| !enabled_set.contains(id)) {
        return Err(AppError::BadRequest(format!(
            "plugin '{bad}' can't be auto without being enabled"
        )));
    }
    repo::set_plugins_for_kind(&state.write, &kind, &body.plugin_ids, &body.auto).await?;
    list_kind_plugins(_admin, State(state), Path(kind)).await
}

/// List configured credential sources + the field names that are set (admin).
/// Never returns the secret values themselves.
#[utoipa::path(
    get, path = "/api/credentials", tag = "plugins",
    security(("sessionCookie" = []), ("apiKey" = [])),
    responses(
        (status = 200, description = "Configured sources + field names", body = [CredentialInfo]),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
    ),
)]
pub(crate) async fn list_credentials(
    _admin: AdminUser,
    State(state): State<AppState>,
) -> Result<Json<Vec<CredentialInfo>>, AppError> {
    let infos = repo::list_credentials(&state.read)
        .await?
        .into_iter()
        .map(|(source, data)| {
            let fields = serde_json::from_str::<
                std::collections::BTreeMap<String, serde_json::Value>,
            >(&data)
            .map(|m| m.into_keys().collect())
            .unwrap_or_default();
            CredentialInfo { source, fields }
        })
        .collect();
    Ok(Json(infos))
}

/// Set (upsert) a source's credentials (admin). The values are stored as-is and
/// never returned by the API.
#[utoipa::path(
    put, path = "/api/credentials/{source}", tag = "plugins",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("source" = String, Path, description = "Vendor/source id, e.g. 'openlibrary'")),
    request_body = CredentialBody,
    responses(
        (status = 200, description = "Stored", body = OkResponse),
        (status = 400, description = "No fields provided"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
    ),
)]
pub(crate) async fn put_credential(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(source): Path<String>,
    Json(body): Json<CredentialBody>,
) -> Result<Json<OkResponse>, AppError> {
    if body.data.is_empty() {
        return Err(AppError::BadRequest("no credential fields provided".into()));
    }
    let data = serde_json::to_string(&body.data).map_err(|e| AppError::Internal(e.into()))?;
    repo::set_credential(&state.write, &source, &data).await?;
    Ok(ok())
}

/// Remove a source's credentials (admin).
#[utoipa::path(
    delete, path = "/api/credentials/{source}", tag = "plugins",
    security(("sessionCookie" = []), ("apiKey" = [])),
    params(("source" = String, Path, description = "Vendor/source id")),
    responses(
        (status = 200, description = "Removed", body = OkResponse),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Not an admin"),
        (status = 404, description = "No credentials for that source"),
    ),
)]
pub(crate) async fn delete_credential(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(source): Path<String>,
) -> Result<Json<OkResponse>, AppError> {
    if !repo::delete_credential(&state.write, &source).await? {
        return Err(AppError::NotFound);
    }
    Ok(ok())
}
