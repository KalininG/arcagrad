//! Strict host-side validation for manifests and repository indexes.

use serde::Serialize;

use crate::manifest::PluginManifest;
use crate::repo::RepoIndex;
use crate::{is_valid_plugin_id, CAPABILITIES, MANIFEST_VERSION, REPO_VERSION};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataStatus {
    Strict,
    Legacy,
    Invalid,
}

#[derive(Debug, Clone, Serialize)]
pub struct ManifestInspection {
    pub valid: bool,
    pub metadata_status: MetadataStatus,
    pub manifest: Option<PluginManifest>,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

/// Parse and inspect exactly the JSON returned by a plugin's `manifest` export.
pub fn inspect_manifest_json(input: &str) -> ManifestInspection {
    let value: serde_json::Value = match serde_json::from_str(input) {
        Ok(value) => value,
        Err(error) => return invalid(format!("manifest is not valid JSON: {error}")),
    };
    let version = value.get("manifest_version").and_then(|v| v.as_u64());
    match version {
        None => ManifestInspection {
            valid: true,
            metadata_status: MetadataStatus::Legacy,
            manifest: None,
            warnings: vec![
                "legacy plugin manifest: migrate to typed manifest_version 1 metadata".into(),
            ],
            errors: Vec::new(),
        },
        Some(1) => {
            let manifest: PluginManifest = match serde_json::from_value(value) {
                Ok(manifest) => manifest,
                Err(error) => return invalid(format!("invalid strict manifest shape: {error}")),
            };
            let errors = validate_manifest(&manifest);
            ManifestInspection {
                valid: errors.is_empty(),
                metadata_status: if errors.is_empty() {
                    MetadataStatus::Strict
                } else {
                    MetadataStatus::Invalid
                },
                manifest: Some(manifest),
                warnings: Vec::new(),
                errors,
            }
        }
        Some(version) => invalid(format!(
            "unsupported manifest_version {version}; host supports {MANIFEST_VERSION}"
        )),
    }
}

pub fn validate_manifest(manifest: &PluginManifest) -> Vec<String> {
    use std::collections::HashSet;

    let mut errors = Vec::new();
    if manifest.manifest_version != MANIFEST_VERSION {
        errors.push(format!(
            "unsupported manifest_version {}; host supports {MANIFEST_VERSION}",
            manifest.manifest_version
        ));
    }
    if !is_valid_plugin_id(&manifest.id) {
        errors.push("manifest.id must contain only ASCII letters, digits, '.', '_' or '-'".into());
    }
    if semver::Version::parse(&manifest.version).is_err() {
        errors.push(format!(
            "manifest.version '{}' is not valid SemVer",
            manifest.version
        ));
    }
    for (name, value) in [
        ("author", manifest.author.as_str()),
        ("name", manifest.name.as_str()),
        ("description", manifest.description.as_str()),
        ("source", manifest.source.as_str()),
    ] {
        if value.trim().is_empty() {
            errors.push(format!("manifest.{name} is required"));
        }
    }
    if let Some(icon) = manifest.icon.as_deref() {
        let relative = icon.starts_with('/') && !icon.starts_with("//");
        let absolute = matches!(
            url::Url::parse(icon)
                .map(|u| u.scheme().to_string())
                .as_deref(),
            Ok("http" | "https")
        );
        if !relative && !absolute {
            errors.push("manifest.icon must be an HTTP(S) URL or a root-relative path".into());
        }
    }
    if let Some(repo) = manifest.repository.as_deref() {
        match url::Url::parse(repo) {
            Ok(url) if matches!(url.scheme(), "http" | "https") => {}
            _ => errors.push("manifest.repository must be a valid HTTP(S) URL".into()),
        }
    }

    let mut capabilities = HashSet::new();
    for capability in &manifest.capabilities {
        if !CAPABILITIES.contains(&capability.as_str()) {
            errors.push(format!(
                "manifest declares unknown capability '{capability}'"
            ));
        }
        if !capabilities.insert(capability.as_str()) {
            errors.push(format!("manifest repeats capability '{capability}'"));
        }
    }
    if capabilities.is_empty() {
        errors.push("manifest.capabilities must not be empty".into());
    }

    let mut hosts = HashSet::new();
    for host in &manifest.hosts {
        let valid = host == "*"
            || (!host.is_empty()
                && !host.contains('/')
                && url::Url::parse(&format!("https://{host}"))
                    .ok()
                    .and_then(|url| url.host_str().map(str::to_owned))
                    .as_deref()
                    == Some(host.as_str()));
        if !valid {
            errors.push(format!("manifest host '{host}' is not a valid hostname"));
        }
        if !hosts.insert(host) {
            errors.push(format!("manifest repeats host '{host}'"));
        }
    }

    let mut feeds = HashSet::new();
    for feed in &manifest.feeds {
        if feed.id.trim().is_empty() || !feeds.insert(feed.id.as_str()) {
            errors.push("manifest feed ids must be non-empty and unique".into());
        }
    }
    if !manifest.feeds.is_empty() && !capabilities.contains("browse") {
        errors.push("manifest feeds require the 'browse' capability".into());
    }
    for (capability, input) in &manifest.reference_inputs {
        if !capabilities.contains(capability.as_str()) {
            errors.push(format!(
                "manifest reference_inputs declares unavailable capability '{capability}'"
            ));
        }
        if input.label.trim().is_empty() {
            errors.push(format!(
                "manifest reference_inputs.{capability}.label is required"
            ));
        }
    }

    if let Some(auth) = &manifest.auth {
        if auth.scheme.trim().is_empty() {
            errors.push("manifest.auth.scheme is required".into());
        }
        let mut fields = HashSet::new();
        for field in &auth.fields {
            if field.name.trim().is_empty() || !fields.insert(field.name.as_str()) {
                errors.push("manifest credential field names must be non-empty and unique".into());
            }
            if field.label.as_deref().unwrap_or("").trim().is_empty()
                || field.help.as_deref().unwrap_or("").trim().is_empty()
            {
                errors.push(format!(
                    "manifest credential field '{}' requires label and help",
                    field.name
                ));
            }
        }
        for capability in &auth.required_for {
            if !capabilities.contains(capability.as_str()) {
                errors.push(format!(
                    "manifest.auth.required_for references undeclared capability '{capability}'"
                ));
            }
        }
    }
    if !matches!(manifest.reading_mode.as_str(), "paged" | "vertical") {
        errors.push(format!(
            "manifest.reading_mode must be \"paged\" or \"vertical\", got \"{}\"",
            manifest.reading_mode
        ));
    }
    errors
}

/// Validates a repository index and all of its entries.
pub fn validate_repo_index(index: &RepoIndex) -> Vec<String> {
    use std::collections::HashSet;

    let mut errors = Vec::new();
    if index.repo_version != REPO_VERSION {
        errors.push(format!(
            "unsupported repo_version {}; host supports {REPO_VERSION}",
            index.repo_version
        ));
    }
    let mut ids = HashSet::new();
    for entry in &index.plugins {
        let id = entry.manifest.id.as_str();
        for e in validate_manifest(&entry.manifest) {
            errors.push(format!("[{id}] {e}"));
        }
        if !ids.insert(id) {
            errors.push(format!("[{id}] duplicate plugin id in index"));
        }
        match url::Url::parse(&entry.artifact_url) {
            Ok(u) if matches!(u.scheme(), "http" | "https") => {}
            Err(url::ParseError::RelativeUrlWithoutBase) if !entry.artifact_url.is_empty() => {}
            _ => errors.push(format!(
                "[{id}] artifact_url must be an HTTP(S) URL or a relative path"
            )),
        }
        if entry.artifact_hash.len() != 64
            || !entry.artifact_hash.bytes().all(|b| b.is_ascii_hexdigit())
        {
            errors.push(format!("[{id}] artifact_hash must be 64 hex characters"));
        }
    }
    errors
}

fn invalid(error: String) -> ManifestInspection {
    ManifestInspection {
        valid: false,
        metadata_status: MetadataStatus::Invalid,
        manifest: None,
        warnings: Vec::new(),
        errors: vec![error],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AuthField, AuthSpec, Feed, ReferenceInput, RepoEntry};
    use std::collections::BTreeMap;

    fn valid() -> PluginManifest {
        PluginManifest {
            manifest_version: 1,
            id: "test-plugin".into(),
            version: "1.2.3".into(),
            author: "Test Author".into(),
            icon: Some("https://example.test/icon.webp".into()),
            repository: Some("https://example.test/repository".into()),
            name: "Test plugin".into(),
            description: "A test plugin.".into(),
            source: "test".into(),
            capabilities: vec!["scrape".into()],
            hosts: vec!["example.test".into()],
            auth: None,
            rate_limit: None,
            feeds: Vec::new(),
            reference_inputs: BTreeMap::new(),
            item_cache_ttl: 0,
            image_headers: BTreeMap::new(),
            clean_titles: true,
            followable: true,
            reading_mode: "paged".into(),
            nsfw: false,
            contract_version: 1,
        }
    }

    #[test]
    fn accepts_complete_strict_manifest() {
        assert!(validate_manifest(&valid()).is_empty());
    }

    #[test]
    fn reading_mode_must_be_paged_or_vertical() {
        let mut m = valid();
        m.reading_mode = "vertical".into();
        assert!(validate_manifest(&m).is_empty(), "vertical is valid");
        m.reading_mode = "webtoon".into();
        assert!(
            validate_manifest(&m)
                .iter()
                .any(|e| e.contains("reading_mode")),
            "an unknown reading_mode is rejected"
        );
    }

    fn repo_entry() -> RepoEntry {
        RepoEntry {
            manifest: valid(),
            artifact_url: "https://example.test/plugin.wasm".into(),
            icon_data: None,
            artifact_hash: "a".repeat(64),
        }
    }

    #[test]
    fn accepts_valid_repo_index() {
        let index = RepoIndex {
            repo_version: 1,
            name: Some("Test repo".into()),
            plugins: vec![repo_entry()],
        };
        assert!(validate_repo_index(&index).is_empty());
        let json = serde_json::to_value(&index).unwrap();
        let e = &json["plugins"][0];
        assert_eq!(e["id"], "test-plugin");
        assert_eq!(e["artifact_url"], "https://example.test/plugin.wasm");
        assert!(e.get("manifest").is_none(), "manifest must be flattened");
    }

    #[test]
    fn rejects_bad_repo_entries() {
        let mut bad = repo_entry();
        bad.artifact_url = "ftp://nope/x.wasm".into();
        bad.artifact_hash = "short".into();
        let dup = repo_entry();
        let index = RepoIndex {
            repo_version: 9,
            name: None,
            plugins: vec![bad, dup.clone(), dup],
        };
        let errs = validate_repo_index(&index).join(" ");
        assert!(errs.contains("unsupported repo_version"));
        assert!(errs.contains("artifact_url must be"));
        assert!(errs.contains("artifact_hash must be"));
        assert!(errs.contains("duplicate plugin id"));
    }

    #[test]
    fn missing_required_field_is_a_parse_error() {
        let mut value = serde_json::to_value(valid()).unwrap();
        value.as_object_mut().unwrap().remove("author");
        let inspected = inspect_manifest_json(&value.to_string());
        assert!(!inspected.valid);
        assert!(inspected.errors.join(" ").contains("author"));
    }

    #[test]
    fn rejects_invalid_version_urls_and_unknown_capability() {
        let mut manifest = valid();
        manifest.version = "latest".into();
        manifest.repository = Some("file:///tmp/plugin".into());
        manifest.capabilities.push("shell".into());
        let errors = validate_manifest(&manifest).join(" ");
        assert!(errors.contains("SemVer"));
        assert!(errors.contains("HTTP(S)"));
        assert!(errors.contains("unknown capability"));

        let mut wildcard = valid();
        wildcard.hosts = vec!["*".into()];
        assert!(validate_manifest(&wildcard).is_empty());
    }

    #[test]
    fn validates_capability_specific_reference_inputs() {
        let mut manifest = valid();
        manifest.reference_inputs.insert(
            "scrape".into(),
            ReferenceInput {
                label: "Gallery URL".into(),
                placeholder: "https://example.test/g/1".into(),
                help: "Selects an exact gallery.".into(),
                required: false,
            },
        );
        assert!(validate_manifest(&manifest).is_empty());
        assert!(inspect_manifest_json(&serde_json::to_string(&manifest).unwrap()).valid);

        manifest.reference_inputs.insert(
            "download".into(),
            ReferenceInput {
                label: String::new(),
                placeholder: String::new(),
                help: String::new(),
                required: true,
            },
        );
        let errors = validate_manifest(&manifest).join(" ");
        assert!(errors.contains("unavailable capability 'download'"));
        assert!(errors.contains("download.label is required"));
    }

    #[test]
    fn icon_accepts_root_relative_paths() {
        let mut manifest = valid();
        manifest.icon = Some("/plugin-icons/example.webp".into());
        assert!(validate_manifest(&manifest).is_empty());

        manifest.icon = Some("//evil.example/icon.png".into());
        assert!(!validate_manifest(&manifest).is_empty());
        manifest.icon = Some("icon.png".into());
        assert!(!validate_manifest(&manifest).is_empty());
    }

    #[test]
    fn rejects_duplicates_and_inconsistent_feed_or_auth() {
        let mut manifest = valid();
        manifest.capabilities.push("scrape".into());
        manifest.hosts.push("example.test".into());
        manifest.feeds = vec![Feed {
            id: "popular".into(),
            label: "Popular".into(),
            ranges: Vec::new(),
            query: false,
            auth: false,
            cache_ttl: 0,
        }];
        manifest.auth = Some(AuthSpec {
            scheme: "api_key".into(),
            fields: vec![
                AuthField {
                    name: "key".into(),
                    label: Some("Key".into()),
                    secret: true,
                    required: true,
                    help: Some("Help".into()),
                },
                AuthField {
                    name: "key".into(),
                    label: Some("Key".into()),
                    secret: true,
                    required: true,
                    help: Some("Help".into()),
                },
            ],
            required_for: vec!["download".into()],
            setup: None,
        });
        let errors = validate_manifest(&manifest).join(" ");
        assert!(errors.contains("repeats capability"));
        assert!(errors.contains("repeats host"));
        assert!(errors.contains("feeds require"));
        assert!(errors.contains("credential field names"));
        assert!(errors.contains("undeclared capability"));
    }

    #[test]
    fn recognizes_legacy_and_unsupported_manifest_versions() {
        let legacy = inspect_manifest_json(r#"{"id":"old","contract_version":1}"#);
        assert!(legacy.valid);
        assert_eq!(legacy.metadata_status, MetadataStatus::Legacy);

        let mut value = serde_json::to_value(valid()).unwrap();
        value["manifest_version"] = serde_json::json!(99);
        let unsupported = inspect_manifest_json(&value.to_string());
        assert!(!unsupported.valid);
        assert!(unsupported.errors.join(" ").contains("unsupported"));
    }

    #[test]
    fn strict_manifest_rejects_bare_string_auth_fields() {
        let mut value = serde_json::to_value(valid()).unwrap();
        value["auth"] = serde_json::json!({"scheme": "api_key", "fields": ["api_key"]});
        let inspected = inspect_manifest_json(&value.to_string());
        assert!(!inspected.valid);
        assert!(inspected
            .errors
            .join(" ")
            .contains("requires label and help"));
    }
}
