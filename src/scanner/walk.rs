//! Filesystem traversal and path-derived metadata.

use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use super::{is_archive, DEFAULT_KIND};

/// Return mtime in the same whole-second form used by scan comparisons.
pub(crate) fn mtime_secs(md: &std::fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub(super) fn stat_tuple(path: &Path) -> Option<(i64, i64)> {
    let md = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    Some((md.len() as i64, mtime_secs(&md)))
}

/// Return archive stats and the error count used to guard deletion detection.
pub(super) fn walk(content_dir: &Path) -> (Vec<(PathBuf, i64, i64)>, usize) {
    let mut out = Vec::new();
    let mut errors = 0usize;
    for result in WalkDir::new(content_dir) {
        let entry = match result {
            Ok(e) => e,
            Err(e) => {
                errors += 1;
                tracing::warn!("scan: walk error: {e}");
                continue;
            }
        };
        if !entry.file_type().is_file() || !is_archive(entry.path()) {
            continue;
        }
        let md = match entry.metadata() {
            Ok(m) => m,
            Err(e) => {
                errors += 1;
                tracing::warn!("scan: cannot stat {}: {e}", entry.path().display());
                continue;
            }
        };
        out.push((entry.path().to_path_buf(), md.len() as i64, mtime_secs(&md)));
    }
    // Sort by path so scan (and therefore id assignment, since `compute_plan` preserves
    // this order) is deterministic across platforms.
    out.sort_by(|a, b| a.0.cmp(&b.0));
    (out, errors)
}

pub(super) fn derive_format(path: &Path) -> String {
    crate::media::format::ext_of(path).unwrap_or_default()
}

pub(super) fn kind_from_path(content_dir: &Path, path: &Path) -> String {
    path.strip_prefix(content_dir)
        .ok()
        .and_then(|rel| {
            let mut comps = rel.components();
            let first = comps.next()?;
            comps.next()?; // Root files have no kind component.
            first.as_os_str().to_str().map(str::to_string)
        })
        .unwrap_or_else(|| DEFAULT_KIND.to_string())
}

fn modality_for(_format: &str) -> &'static str {
    "paginated"
}

/// Derive title, modality, and optional series order for a new item.
pub(super) fn presentation(
    path: &Path,
    is_epub: bool,
    format: &str,
    raw_title: &str,
) -> (String, &'static str, Option<f64>) {
    if is_epub {
        match crate::media::epub::inspect(path) {
            Ok(m) => {
                let title = m
                    .title
                    .clone()
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    // Cleaning can strip valid brackets from an EPUB title.
                    .unwrap_or_else(|| crate::media::title::clean(raw_title));
                let modality = if m.reflowable {
                    "reflowable"
                } else {
                    "paginated"
                };
                (title, modality, m.series_index)
            }
            // The OCF marker still identifies an EPUB when its OPF is unreadable.
            Err(e) => {
                tracing::warn!(
                    "scan: {} is an epub but its OPF is unreadable ({e:#}); using the filename title",
                    path.display()
                );
                (crate::media::title::clean(raw_title), "reflowable", None)
            }
        }
    } else {
        (
            crate::media::title::clean(raw_title),
            modality_for(format),
            None,
        )
    }
}

pub(super) fn derive_title(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::DEFAULT_KIND;

    #[test]
    fn kind_is_the_top_level_folder() {
        let root = Path::new("/content");
        assert_eq!(
            kind_from_path(root, Path::new("/content/manga/x.cbz")),
            "manga"
        );
        assert_eq!(
            kind_from_path(root, Path::new("/content/Manga Comics/East Blue/v1.cbz")),
            "Manga Comics"
        );
        assert_eq!(
            kind_from_path(root, Path::new("/content/loose.cbz")),
            DEFAULT_KIND
        );
    }

    #[test]
    fn modality_is_paginated_for_image_archives() {
        assert_eq!(modality_for("cbz"), "paginated");
        assert_eq!(modality_for("zip"), "paginated");
    }
}
