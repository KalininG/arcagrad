//! Repository index generation from WASM artifacts.

use std::path::Path;

use anyhow::{Context, Result};
use arcagrad_plugin_sdk::{validate_repo_index, RepoEntry, RepoIndex, REPO_VERSION};

use crate::plugins::wasm_host;

/// Build and validate a repository index from strict-manifest WASM artifacts.
/// Artifact URLs are relative unless `base_url` is supplied.
pub fn generate_from_dir(
    dir: &Path,
    base_url: Option<&str>,
    name: Option<String>,
    handle: tokio::runtime::Handle,
) -> Result<RepoIndex> {
    let base = base_url.map(|b| b.trim_end_matches('/'));
    let mut wasm_files: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("wasm"))
        .collect();
    wasm_files.sort();

    let mut plugins = Vec::new();
    for path in &wasm_files {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let manifest = wasm_host::read_manifest(&bytes, handle.clone())
            .with_context(|| format!("reading manifest of {}", path.display()))?;
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .context("artifact has no file name")?;
        let icon_data = wasm_host::read_icon(&bytes, handle.clone()).map(|b| {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(b)
        });
        plugins.push(RepoEntry {
            manifest,
            icon_data,
            artifact_url: match base {
                Some(base) => format!("{base}/{file_name}"),
                None => file_name.to_string(),
            },
            artifact_hash: wasm_host::artifact_hash(&bytes),
        });
    }

    let index = RepoIndex {
        repo_version: REPO_VERSION,
        name,
        plugins,
    };
    let errors = validate_repo_index(&index);
    if !errors.is_empty() {
        anyhow::bail!("generated index is invalid:\n  {}", errors.join("\n  "));
    }
    Ok(index)
}

pub fn to_json(index: &RepoIndex) -> Result<String> {
    Ok(serde_json::to_string_pretty(index)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn generates_index_from_wasm_dir() {
        let dir = tempfile::tempdir().unwrap();
        let src = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/plugins/openlibrary/openlibrary.wasm"
        );
        let bytes = std::fs::read(src).expect("built by build.rs on cargo build");
        std::fs::write(dir.path().join("openlibrary.wasm"), &bytes).unwrap();

        let index = generate_from_dir(
            dir.path(),
            Some("https://example.test/repo/"),
            Some("Test".into()),
            tokio::runtime::Handle::current(),
        )
        .unwrap();

        assert_eq!(index.repo_version, REPO_VERSION);
        assert_eq!(index.plugins.len(), 1);
        let e = &index.plugins[0];
        assert_eq!(e.manifest.id, "openlibrary");
        assert_eq!(e.artifact_url, "https://example.test/repo/openlibrary.wasm");
        assert_eq!(e.artifact_hash, wasm_host::artifact_hash(&bytes));
        assert!(validate_repo_index(&index).is_empty());
        let json = to_json(&index).unwrap();
        assert!(json.contains("\"id\": \"openlibrary\""));
        assert!(json.contains("artifact_url"));

        let rel =
            generate_from_dir(dir.path(), None, None, tokio::runtime::Handle::current()).unwrap();
        assert_eq!(rel.plugins[0].artifact_url, "openlibrary.wasm");
        assert!(validate_repo_index(&rel).is_empty());
    }
}
