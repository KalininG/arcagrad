//! The decentralized plugin-repository index format (`repo_version` 1).

use serde::{Deserialize, Serialize};

use crate::manifest::PluginManifest;

/// A plugin repository index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoIndex {
    pub repo_version: u32,
    /// Optional human name for the repo, shown in the manage-repos UI.
    #[serde(default)]
    pub name: Option<String>,
    pub plugins: Vec<RepoEntry>,
}

/// One installable artifact and its manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoEntry {
    /// Flattened into the entry's top-level JSON object.
    #[serde(flatten)]
    pub manifest: PluginManifest,
    /// Where the host downloads the `.wasm` on install (http/https).
    pub artifact_url: String,
    /// BLAKE3 hash of the artifact, encoded as hex.
    pub artifact_hash: String,
    /// Base64-encoded WebP icon used before the artifact is installed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_data: Option<String>,
}
