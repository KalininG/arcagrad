//! Shared JSON types for the arcagrad plugin ABI.

pub mod contract;
#[cfg(all(feature = "guest", target_arch = "wasm32"))]
pub mod guest;
pub mod manifest;
pub mod repo;
#[cfg(feature = "validation")]
mod validation;

pub use contract::*;
pub use manifest::*;
pub use repo::*;
#[cfg(feature = "validation")]
pub use validation::*;

/// Current metadata-schema version. Independent of the host operation contract.
pub const MANIFEST_VERSION: u32 = 1;

/// Runtime ABI version.
pub const CONTRACT_VERSION: u32 = 1;

/// Plugin repository index version.
pub const REPO_VERSION: u32 = 1;

/// Capabilities accepted by strict manifest validation.
pub const CAPABILITIES: &[&str] = &[
    "scrape", "download", "browse", "read", "identify", "calendar",
];

/// Returns whether an id is safe for manifests and artifact filenames.
pub fn is_valid_plugin_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}
