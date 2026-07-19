//! The plugin `manifest` export's document: a plugin's self-description.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// One credential requested by a plugin.
#[derive(Debug, Clone, Serialize)]
pub struct AuthField {
    /// The credential key the plugin reads (e.g. `"api_key"`, `"ipb_member_id"`).
    pub name: String,
    /// UI label; clients fall back to `name` when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Mask the input (password field). Defaults `true`: credentials are secret.
    pub secret: bool,
    /// Whether the field must be filled before saving.
    pub required: bool,
    /// Help text, e.g. where to find the value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}

impl<'de> Deserialize<'de> for AuthField {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Name(String),
            Full {
                name: String,
                #[serde(default)]
                label: Option<String>,
                #[serde(default = "default_true")]
                secret: bool,
                #[serde(default = "default_true")]
                required: bool,
                #[serde(default)]
                help: Option<String>,
            },
        }
        Ok(match Raw::deserialize(d)? {
            Raw::Name(name) => AuthField {
                name,
                label: None,
                secret: true,
                required: true,
                help: None,
            },
            Raw::Full {
                name,
                label,
                secret,
                required,
                help,
            } => AuthField {
                name,
                label,
                secret,
                required,
                help,
            },
        })
    }
}

/// Credential fields and the capabilities that require them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSpec {
    /// e.g. `"api_key"` — tells the UI what kind of input to collect.
    pub scheme: String,
    /// The credential fields — bare names or full [`AuthField`] objects.
    #[serde(default)]
    pub fields: Vec<AuthField>,
    /// Capabilities that require auth (e.g. `["download"]`); others run anon.
    #[serde(default)]
    pub required_for: Vec<String>,
    /// Instructions for obtaining the credentials.
    #[serde(default)]
    pub setup: Option<String>,
}

/// Source-specific presentation for a capability's opaque `ref` argument.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferenceInput {
    pub label: String,
    pub placeholder: String,
    pub help: String,
    pub required: bool,
}

/// Rate limits applied by the host to this plugin's requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimit {
    /// Per-endpoint limits, first-substring-match wins (specific → general).
    #[serde(default)]
    pub rules: Vec<RateRule>,
    /// Max concurrent in-flight requests to this vendor at once (0/absent means 1).
    #[serde(default)]
    pub max_concurrency: usize,
}

/// Request allowance for matching URLs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateRule {
    /// URL substring matched by this rule; empty matches every URL.
    #[serde(rename = "match", default)]
    pub match_pattern: String,
    /// Allowed requests per `per_ms` window (the vendor's published numerator).
    pub requests: u32,
    /// The window length in ms (the vendor's published denominator; `60000` = /1min).
    pub per_ms: u64,
}

impl RateLimit {
    /// `max_concurrency` with the 0/absent default of 1 applied.
    pub fn concurrency(&self) -> usize {
        self.max_concurrency.max(1)
    }
}

/// One browsable feed exposed by a plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Feed {
    /// Stable feed id, echoed back in `BrowseRequest::feed` (e.g. "popular", "recent").
    pub id: String,
    /// Human tab label (e.g. "Popular").
    pub label: String,
    /// Time ranges accepted by the feed.
    #[serde(default)]
    pub ranges: Vec<String>,
    /// Whether this feed takes a free-text query (the client shows a filter box).
    #[serde(default)]
    pub query: bool,
    /// Whether this feed needs the plugin's stored credential (e.g. "favorites").
    #[serde(default)]
    pub auth: bool,
    /// Cache lifetime in seconds.
    #[serde(default)]
    pub cache_ttl: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub manifest_version: u32,
    pub id: String,
    pub version: String,
    pub author: String,
    pub icon: Option<String>,
    pub repository: Option<String>,
    pub name: String,
    pub description: String,
    pub source: String,
    pub capabilities: Vec<String>,
    pub hosts: Vec<String>,
    pub auth: Option<AuthSpec>,
    pub rate_limit: Option<RateLimit>,
    pub feeds: Vec<Feed>,
    /// Source-specific UI copy for the opaque `ref` accepted by a capability.
    #[serde(default)]
    pub reference_inputs: BTreeMap<String, ReferenceInput>,
    pub item_cache_ttl: u64,
    pub image_headers: BTreeMap<String, String>,
    pub clean_titles: bool,
    /// Whether users may follow this source for new results.
    #[serde(default = "default_true")]
    pub followable: bool,
    /// Default reader: `"paged"` or `"vertical"`.
    #[serde(default = "default_reading_mode")]
    pub reading_mode: String,
    /// Whether the store should hide this source until the user opts to show it.
    #[serde(default)]
    pub nsfw: bool,
    pub contract_version: u32,
}

/// Default for manifest flags that were added with a true value.
pub fn default_true() -> bool {
    true
}

/// Default for an omitted `reading_mode`.
pub fn default_reading_mode() -> String {
    "paged".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_field_accepts_bare_name_or_full_object() {
        let f: AuthField = serde_json::from_str(r#""api_key""#).unwrap();
        assert_eq!(f.name, "api_key");
        assert!(f.secret && f.required && f.label.is_none() && f.help.is_none());

        let f: AuthField = serde_json::from_str(
            r#"{"name":"igneous","label":"igneous cookie","required":false,"help":"only for the members area"}"#,
        )
        .unwrap();
        assert_eq!(f.name, "igneous");
        assert_eq!(f.label.as_deref(), Some("igneous cookie"));
        assert!(f.secret, "secret defaults true");
        assert!(!f.required);
        assert_eq!(f.help.as_deref(), Some("only for the members area"));
    }

    #[test]
    fn feed_flags_default_off() {
        let f: Feed = serde_json::from_str(r#"{"id":"recent","label":"Recent"}"#).unwrap();
        assert!(f.ranges.is_empty() && !f.query && !f.auth);
        assert_eq!(f.cache_ttl, 0);
    }

    #[test]
    fn rate_limit_concurrency_floor_is_one() {
        let rl: RateLimit = serde_json::from_str(r#"{"rules":[]}"#).unwrap();
        assert_eq!(rl.concurrency(), 1, "0/absent max_concurrency → 1");
        let rule: RateRule = serde_json::from_str(r#"{"requests":10,"per_ms":60000}"#).unwrap();
        assert_eq!(rule.match_pattern, "", "omitted match = catch-all");
    }
}
