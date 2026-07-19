//! Scanner planning for full and targeted reconciliation.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rayon::prelude::*;
use sqlx::SqlitePool;

use crate::media::identity::{self, Identity, Scheme};

use super::series::series_folder_of;
use super::walk::{
    derive_format, derive_title, kind_from_path, mtime_secs, presentation, stat_tuple,
};
use super::{Mutation, Plan, ScanStats};

#[derive(sqlx::FromRow, Clone)]
pub(super) struct ExistingRow {
    id: i64,
    scheme_tag: String,
    structural_hash: String,
    deep_hash: Option<String>,
    path: String,
    kind: String,
    series_id: Option<i64>,
    size_bytes: i64,
    mtime: i64,
}

impl ExistingRow {
    fn bucket(&self) -> Bucket {
        (self.scheme_tag.clone(), self.structural_hash.clone())
    }

    /// Force legacy byte fingerprints through archive identification.
    fn is_unchanged(&self, size: i64, mtime: i64) -> bool {
        self.size_bytes == size && self.mtime == mtime && self.scheme_tag != Scheme::BytesV1.tag()
    }
}

pub(super) type Bucket = (String, String);

pub(super) const SELECT_ROW: &str =
    "SELECT id, scheme_tag, structural_hash, deep_hash, path, kind, series_id, size_bytes, mtime FROM items";

struct Pending {
    path: PathBuf,
    path_str: String,
    size: i64,
    mtime: i64,
    known: Option<usize>,
}

enum Identified {
    Ready {
        scheme: Scheme,
        structural_hash: String,
        page_count: Option<i64>,
        chapters: Vec<crate::media::chapters::Chapter>,
    },
    NotReady,
    Errored(String),
}

/// Recognize unsupported container magic under an archive extension.
fn known_other_format(path: &Path) -> Option<&'static str> {
    use std::io::Read;
    let mut buf = [0u8; 8];
    let mut f = std::fs::File::open(path).ok()?;
    let n = f.read(&mut buf).ok()?;
    let b = &buf[..n];
    if b.starts_with(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C]) {
        Some("7z")
    } else if b.starts_with(b"%PDF") {
        Some("pdf")
    } else {
        None
    }
}

/// Identify a stable, complete archive snapshot.
/// Files that change during identification are deferred.
fn identify_for_scan(path: &Path, size: i64, mtime: i64) -> Identified {
    if let Some(fmt) = known_other_format(path) {
        return Identified::Errored(format!(
            "unsupported archive format ({fmt} with a zip extension) — only zip/cbz \
             is readable yet; repack it as zip/cbz"
        ));
    }
    match identity::identify(path) {
        Ok(Identity::Ready {
            scheme: Scheme::BytesV1,
            ..
        }) => {
            tracing::debug!(
                "scan: {} has an archive extension but no zip magic (mid-write or \
                 not an archive) — deferring",
                path.display()
            );
            Identified::NotReady
        }
        Ok(Identity::Ready {
            scheme,
            structural_hash,
            pages,
            ..
        }) => match stat_tuple(path) {
            Some((s, m)) if s == size && m == mtime => {
                let page_count = pages.as_ref().map(|p| p.len() as i64);
                let chapters = pages
                    .as_deref()
                    .map(crate::media::chapters::parse_chapters)
                    .unwrap_or_default();
                Identified::Ready {
                    scheme,
                    structural_hash,
                    page_count,
                    chapters,
                }
            }
            _ => {
                tracing::debug!(
                    "scan: {} changed while being read (still copying) — deferring",
                    path.display()
                );
                Identified::NotReady
            }
        },
        Ok(Identity::NotReady) => {
            tracing::debug!(
                "scan: {} isn't a complete readable archive yet — deferring",
                path.display()
            );
            Identified::NotReady
        }
        Err(e) => Identified::Errored(format!("{e:#}")),
    }
}

/// Build a deterministic plan while identifying files in parallel.
pub(super) fn compute_plan(
    content_dir: &Path,
    existing: Vec<ExistingRow>,
    _now: i64,
) -> Result<Plan> {
    let mut by_path: HashMap<&str, usize> = HashMap::with_capacity(existing.len());
    let mut by_bucket: HashMap<Bucket, Vec<usize>> = HashMap::with_capacity(existing.len());
    for (i, r) in existing.iter().enumerate() {
        by_path.insert(r.path.as_str(), i);
        by_bucket.entry(r.bucket()).or_default().push(i);
    }

    let mut seen: HashSet<i64> = HashSet::new();
    let mut new_by_bucket: HashMap<Bucket, PathBuf> = HashMap::new();
    let mut mutations = Vec::new();
    let mut stats = ScanStats::default();

    let (files, walk_errors) = super::walk::walk(content_dir);
    stats.total = files.len();

    // Fast-path unchanged files before opening archives.
    let mut pending: Vec<Pending> = Vec::new();
    for (path, size, mtime) in files {
        let path_str = path.to_string_lossy().into_owned();
        let known = by_path.get(path_str.as_str()).copied();
        if let Some(idx) = known {
            let r = &existing[idx];
            if r.is_unchanged(size, mtime) {
                seen.insert(r.id);
                stats.unchanged += 1;
                continue;
            }
        }
        pending.push(Pending {
            path,
            path_str,
            size,
            mtime,
            known,
        });
    }

    // Identity reads are independent; ordered collection preserves walk order.
    let identified: Vec<Identified> = pending
        .par_iter()
        .map(|p| identify_for_scan(&p.path, p.size, p.mtime))
        .collect();

    for (p, id) in pending.into_iter().zip(identified) {
        let Pending {
            path,
            path_str,
            size,
            mtime,
            known,
        } = p;
        let (scheme, structural, page_count, chapters) = match id {
            Identified::Ready {
                scheme,
                structural_hash,
                page_count,
                chapters,
            } => (scheme, structural_hash, page_count, chapters),
            Identified::NotReady => {
                stats.deferred.push(path_str);
                if let Some(idx) = known {
                    seen.insert(existing[idx].id);
                }
                continue;
            }
            Identified::Errored(msg) => {
                tracing::warn!("scan: skipping unreadable {}: {msg}", path.display());
                stats.errored += 1;
                if let Some(idx) = known {
                    seen.insert(existing[idx].id);
                }
                continue;
            }
        };
        let bucket: Bucket = (scheme.tag().to_string(), structural);

        if let Some(idx) = known {
            let r = &existing[idx];
            decide_known(
                r,
                bucket,
                &path,
                size,
                mtime,
                page_count,
                chapters,
                &mut new_by_bucket,
                &mut mutations,
                &mut stats,
            );
            seen.insert(r.id);
            continue;
        }

        let candidates: Vec<&ExistingRow> = by_bucket
            .get(&bucket)
            .map(|idxs| idxs.iter().map(|&i| &existing[i]).collect())
            .unwrap_or_default();
        decide_new(
            content_dir,
            &path,
            path_str,
            size,
            mtime,
            Some(scheme),
            bucket,
            &candidates,
            page_count,
            chapters,
            &mut new_by_bucket,
            &mut mutations,
            &mut stats,
            |p| Path::new(p).exists(),
            &mut seen,
        );
    }

    // Never infer deletions from a partial filesystem walk.
    if walk_errors == 0 {
        for r in &existing {
            if !seen.contains(&r.id) {
                mutations.push(Mutation::Delete { id: r.id });
                stats.removed += 1;
                stats.removed_thumbs.push(r.structural_hash.clone());
                stats.removed_ids.push(r.id);
            }
        }
    } else {
        let unseen = existing.iter().filter(|r| !seen.contains(&r.id)).count();
        stats.errored += walk_errors;
        tracing::warn!(
            "scan hit {walk_errors} filesystem error(s); keeping {unseen} unseen \
             archive(s) rather than deleting from a partial view of disk"
        );
    }

    Ok(Plan { mutations, stats })
}

#[allow(clippy::too_many_arguments)]
/// Update an already-indexed path without replacing its item id.
fn decide_known(
    r: &ExistingRow,
    bucket: Bucket,
    path: &Path,
    size: i64,
    mtime: i64,
    page_count: Option<i64>,
    chapters: Vec<crate::media::chapters::Chapter>,
    new_by_bucket: &mut HashMap<Bucket, PathBuf>,
    mutations: &mut Vec<Mutation>,
    stats: &mut ScanStats,
) {
    if r.bucket() == bucket {
        mutations.push(Mutation::Touch {
            id: r.id,
            size,
            mtime,
        });
    } else {
        let (scheme_tag, structural_hash) = bucket.clone();
        new_by_bucket.insert(bucket, path.to_path_buf());
        mutations.push(Mutation::UpdateContent {
            id: r.id,
            scheme_tag,
            structural_hash,
            size,
            mtime,
            page_count,
            chapters,
        });
    }
    stats.updated += 1;
}

#[allow(clippy::too_many_arguments)]
/// Resolve a new path as an insert, move, duplicate, or hash collision.
fn decide_new(
    content_dir: &Path,
    path: &Path,
    ps: String,
    size: i64,
    mtime: i64,
    scheme: Option<Scheme>,
    bucket: Bucket,
    candidates: &[&ExistingRow],
    page_count: Option<i64>,
    chapters: Vec<crate::media::chapters::Chapter>,
    new_by_bucket: &mut HashMap<Bucket, PathBuf>,
    mutations: &mut Vec<Mutation>,
    stats: &mut ScanStats,
    other_present: impl Fn(&str) -> bool,
    mark: &mut HashSet<i64>,
) {
    let matched = match candidates {
        [] => None,
        [only] => Some(*only),
        many => scheme.and_then(|s| resolve_multi(path, s, many.iter().copied(), mutations)),
    };
    if let Some(r) = matched {
        if same_file(path, Path::new(&r.path)) {
            push_repoint(mutations, stats, content_dir, r, ps, size, mtime);
            mark.insert(r.id);
        } else if other_present(&r.path) {
            stats.duplicates += 1;
        } else {
            push_repoint(mutations, stats, content_dir, r, ps, size, mtime);
            mark.insert(r.id);
        }
        return;
    }

    if new_by_bucket.contains_key(&bucket) {
        stats.duplicates += 1;
        return;
    }

    let deep_hash = if candidates.is_empty() {
        None
    } else {
        scheme.and_then(|s| identity::deep_hash(path, s).ok())
    };
    let (scheme_tag, structural_hash) = bucket.clone();
    new_by_bucket.insert(bucket, path.to_path_buf());
    let format = derive_format(path);
    let raw_title = derive_title(path);
    let is_epub = scheme == Some(Scheme::EpubStructuralV1);
    let (title, modality, series_index) = presentation(path, is_epub, &format, &raw_title);
    mutations.push(Mutation::Insert {
        scheme_tag,
        structural_hash,
        deep_hash,
        title,
        raw_title,
        kind: kind_from_path(content_dir, path),
        modality,
        format,
        path: ps,
        size,
        mtime,
        page_count,
        chapters,
        series_index,
    });
    stats.added += 1;
}

pub(super) enum Probe {
    Present {
        path: PathBuf,
        size: i64,
        mtime: i64,
        scheme_tag: String,
        structural_hash: String,
        page_count: Option<i64>,
        chapters: Vec<crate::media::chapters::Chapter>,
    },
    Unchanged {
        path: PathBuf,
    },
    Absent {
        path: PathBuf,
    },
    Unreadable,
    NotReady {
        path: PathBuf,
    },
}

pub(super) async fn rows_by_path(
    pool: &SqlitePool,
    paths: &[String],
) -> Result<HashMap<String, ExistingRow>> {
    if paths.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = vec!["?"; paths.len()].join(", ");
    let sql = format!("{SELECT_ROW} WHERE path IN ({placeholders})");
    let mut q = sqlx::query_as::<_, ExistingRow>(sqlx::AssertSqlSafe(sql));
    for p in paths {
        q = q.bind(p);
    }
    Ok(q.fetch_all(pool)
        .await?
        .into_iter()
        .map(|r| (r.path.clone(), r))
        .collect())
}

pub(super) async fn buckets_by_structural(
    pool: &SqlitePool,
    hashes: &[String],
) -> Result<HashMap<Bucket, Vec<ExistingRow>>> {
    let mut out: HashMap<Bucket, Vec<ExistingRow>> = HashMap::new();
    if hashes.is_empty() {
        return Ok(out);
    }
    let placeholders = vec!["?"; hashes.len()].join(", ");
    let sql = format!("{SELECT_ROW} WHERE structural_hash IN ({placeholders})");
    let mut q = sqlx::query_as::<_, ExistingRow>(sqlx::AssertSqlSafe(sql));
    for h in hashes {
        q = q.bind(h);
    }
    for r in q.fetch_all(pool).await? {
        out.entry(r.bucket()).or_default().push(r);
    }
    Ok(out)
}

/// Probe affected paths without blocking the async runtime.
pub(super) fn probe_paths(paths: &[PathBuf], by_path: &HashMap<String, ExistingRow>) -> Vec<Probe> {
    paths
        .par_iter()
        .map(|p| {
            let ps = p.to_string_lossy();
            let md = match std::fs::metadata(p) {
                Ok(m) if m.is_file() => m,
                _ => return Probe::Absent { path: p.clone() },
            };
            let size = md.len() as i64;
            let mtime = mtime_secs(&md);
            if let Some(r) = by_path.get(ps.as_ref()) {
                if r.is_unchanged(size, mtime) {
                    return Probe::Unchanged { path: p.clone() };
                }
            }
            match identify_for_scan(p, size, mtime) {
                Identified::Ready {
                    scheme,
                    structural_hash,
                    page_count,
                    chapters,
                } => Probe::Present {
                    path: p.clone(),
                    size,
                    mtime,
                    scheme_tag: scheme.tag().to_string(),
                    structural_hash,
                    page_count,
                    chapters,
                },
                Identified::NotReady => Probe::NotReady { path: p.clone() },
                Identified::Errored(msg) => {
                    tracing::warn!("targeted scan: skipping unreadable {}: {msg}", p.display());
                    Probe::Unreadable
                }
            }
        })
        .collect()
}

/// Build a plan for watcher-targeted paths.
pub(super) fn decide_targeted(
    content_dir: &Path,
    probes: Vec<Probe>,
    by_path: &HashMap<String, ExistingRow>,
    by_bucket: &HashMap<Bucket, Vec<ExistingRow>>,
) -> Plan {
    let present: HashSet<PathBuf> = probes
        .iter()
        .filter_map(|pr| match pr {
            Probe::Present { path, .. } | Probe::Unchanged { path } | Probe::NotReady { path } => {
                Some(path.clone())
            }
            _ => None,
        })
        .collect();
    let mut other_present: HashSet<String> = HashSet::new();
    for rows in by_bucket.values() {
        for r in rows {
            let rp = PathBuf::from(&r.path);
            let in_set = present.contains(&rp)
                || probes
                    .iter()
                    .any(|pr| matches!(pr, Probe::Absent { path } if *path == rp));
            if !in_set && std::fs::metadata(&rp).map(|m| m.is_file()).unwrap_or(false) {
                other_present.insert(r.path.clone());
            }
        }
    }
    let is_present = |p: &str| present.contains(Path::new(p)) || other_present.contains(p);

    let mut mutations = Vec::new();
    let mut stats = ScanStats::default();
    let mut repointed: HashSet<i64> = HashSet::new();
    let mut new_by_bucket: HashMap<Bucket, PathBuf> = HashMap::new();

    for pr in &probes {
        let Probe::Present {
            path,
            size,
            mtime,
            scheme_tag,
            structural_hash,
            page_count,
            chapters,
        } = pr
        else {
            match pr {
                Probe::Unreadable => stats.errored += 1,
                Probe::NotReady { path } => {
                    stats.deferred.push(path.to_string_lossy().into_owned())
                }
                _ => {}
            }
            continue;
        };
        stats.total += 1;
        let (size, mtime) = (*size, *mtime);
        let ps = path.to_string_lossy().into_owned();
        let bucket: Bucket = (scheme_tag.clone(), structural_hash.clone());

        if let Some(r) = by_path.get(&ps) {
            decide_known(
                r,
                bucket,
                path,
                size,
                mtime,
                *page_count,
                chapters.clone(),
                &mut new_by_bucket,
                &mut mutations,
                &mut stats,
            );
            continue;
        }

        let scheme = Scheme::from_tag(scheme_tag);
        let candidates: Vec<&ExistingRow> = by_bucket
            .get(&bucket)
            .map(|rows| rows.iter().collect())
            .unwrap_or_default();
        decide_new(
            content_dir,
            path,
            ps,
            size,
            mtime,
            scheme,
            bucket,
            &candidates,
            *page_count,
            chapters.clone(),
            &mut new_by_bucket,
            &mut mutations,
            &mut stats,
            |p| is_present(p),
            &mut repointed,
        );
    }

    // Delete absent rows only after moves have had a chance to repoint them.
    for pr in &probes {
        if let Probe::Absent { path } = pr {
            if let Some(r) = by_path.get(path.to_string_lossy().as_ref()) {
                if !repointed.contains(&r.id) {
                    mutations.push(Mutation::Delete { id: r.id });
                    stats.removed += 1;
                    stats.removed_thumbs.push(r.structural_hash.clone());
                    stats.removed_ids.push(r.id);
                }
            }
        }
    }

    Plan { mutations, stats }
}

/// Resolve a structural-hash collision with lazy deep hashes.
fn resolve_multi<'a>(
    path: &Path,
    scheme: Scheme,
    candidates: impl IntoIterator<Item = &'a ExistingRow>,
    mutations: &mut Vec<Mutation>,
) -> Option<&'a ExistingRow> {
    let this_deep = identity::deep_hash(path, scheme).ok()?;
    for c in candidates {
        let c_deep = match &c.deep_hash {
            Some(d) => d.clone(),
            None => match identity::deep_hash(Path::new(&c.path), scheme) {
                Ok(d) => {
                    mutations.push(Mutation::SetDeepHash {
                        id: c.id,
                        deep_hash: d.clone(),
                    });
                    d
                }
                Err(_) => continue,
            },
        };
        if c_deep == this_deep {
            return Some(c);
        }
    }
    None
}

/// Drop watcher echoes that match the indexed file stat.
pub async fn changed_paths(
    pool: &SqlitePool,
    content_dir: &Path,
    paths: &[PathBuf],
) -> Result<Vec<PathBuf>> {
    let candidates: Vec<PathBuf> = paths
        .iter()
        .filter(|p| super::is_archive(p) && under(content_dir, p))
        .cloned()
        .collect();
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let path_strs: Vec<String> = candidates
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let by_path = rows_by_path(pool, &path_strs).await?;
    let kept = tokio::task::spawn_blocking(move || {
        candidates
            .into_iter()
            .filter(|p| {
                let ps = p.to_string_lossy();
                let row = by_path.get(ps.as_ref());
                match std::fs::metadata(p) {
                    Ok(m) if m.is_file() => {
                        let (size, mtime) = (m.len() as i64, mtime_secs(&m));
                        match row {
                            Some(r) => !r.is_unchanged(size, mtime),
                            None => true,
                        }
                    }
                    _ => row.is_some(),
                }
            })
            .collect::<Vec<_>>()
    })
    .await
    .context("changed_paths stat join")?;
    Ok(kept)
}

#[allow(clippy::too_many_arguments)]
/// Repoint an item and record recommendation-relevant folder changes.
fn push_repoint(
    mutations: &mut Vec<Mutation>,
    stats: &mut ScanStats,
    content_dir: &Path,
    r: &ExistingRow,
    path_str: String,
    size: i64,
    mtime: i64,
) {
    let new_kind = kind_from_path(content_dir, Path::new(&path_str));
    if r.kind != new_kind {
        stats.moved_kind_changed.push(r.id);
    }
    if r.series_id.is_some() != series_folder_of(content_dir, Path::new(&path_str)).is_some() {
        stats.moved_leaf_status_changed.push(r.id);
    }
    mutations.push(Mutation::Repoint {
        id: r.id,
        kind: new_kind,
        path: path_str,
        size,
        mtime,
    });
    stats.moved += 1;
}

/// Check whether a path belongs to the library root.
pub(super) fn under(content_dir: &Path, p: &Path) -> bool {
    if p.starts_with(content_dir) {
        return true;
    }
    matches!(
        (std::fs::canonicalize(p).ok(), std::fs::canonicalize(content_dir).ok()),
        (Some(cp), Some(cd)) if cp.starts_with(&cd)
    )
}

/// Compare two paths after canonicalization.
fn same_file(a: &Path, b: &Path) -> bool {
    matches!(
        (std::fs::canonicalize(a).ok(), std::fs::canonicalize(b).ok()),
        (Some(ca), Some(cb)) if ca == cb
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::test_util::{items, structural_of, write_cbz, write_epub};
    use crate::scanner::walk::stat_tuple;
    use crate::scanner::{reconcile, reconcile_paths, scan, DEFAULT_KIND};

    fn zip_row(
        id: i64,
        path: &Path,
        structural: &str,
        kind: &str,
        size: i64,
        mtime: i64,
    ) -> ExistingRow {
        ExistingRow {
            id,
            scheme_tag: "zip-structural-v1".into(),
            structural_hash: structural.into(),
            deep_hash: None,
            path: path.to_string_lossy().into_owned(),
            kind: kind.into(),
            series_id: None,
            size_bytes: size,
            mtime,
        }
    }

    #[test]
    fn new_files_are_inserted() {
        let dir = tempfile::tempdir().unwrap();
        write_cbz(&dir.path().join("a.cbz"), "a");
        write_cbz(&dir.path().join("b.cbz"), "b");

        let plan = compute_plan(dir.path(), vec![], 0).unwrap();
        assert_eq!(plan.stats.total, 2);
        assert_eq!(plan.stats.added, 2);
        assert_eq!(plan.mutations.len(), 2);
    }

    #[test]
    fn unchanged_files_hit_the_fast_path() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.cbz");
        write_cbz(&p, "a");
        let md = std::fs::metadata(&p).unwrap();
        let mtime = md
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let existing = vec![zip_row(
            1,
            &p,
            "irrelevant-not-rehashed",
            DEFAULT_KIND,
            md.len() as i64,
            mtime,
        )];

        let plan = compute_plan(dir.path(), existing, 0).unwrap();
        assert_eq!(plan.stats.unchanged, 1);
        assert_eq!(plan.stats.added, 0);
        assert!(plan.mutations.is_empty());
    }

    #[test]
    fn mid_write_partial_without_zip_magic_is_deferred_not_indexed() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.cbz");
        std::fs::write(&p, b"JFIF-not-a-zip-yet").unwrap();

        let plan = compute_plan(dir.path(), vec![], 0).unwrap();
        assert_eq!(plan.stats.added, 0);
        assert!(plan.mutations.is_empty(), "no insert for a partial");
        assert_eq!(
            plan.stats.deferred,
            vec![p.to_string_lossy().into_owned()],
            "reported deferred so the settle follow-up re-observes it"
        );

        write_cbz(&p, "a");
        let plan = compute_plan(dir.path(), vec![], 0).unwrap();
        assert_eq!(plan.stats.added, 1);
        assert!(plan.stats.deferred.is_empty());
    }

    #[test]
    fn unparseable_rar_defers_indistinguishable_from_mid_write() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("Vagabond v03.cbz");
        std::fs::write(&p, b"Rar!\x1a\x07\x01\x00rest-of-rar").unwrap();

        let plan = compute_plan(dir.path(), vec![], 0).unwrap();
        assert_eq!(plan.stats.added, 0);
        assert_eq!(
            plan.stats.errored, 0,
            "no longer a hard error — RAR is read now"
        );
        assert!(
            !plan.stats.deferred.is_empty(),
            "deferred as a maybe-mid-write rar (settle chain bounds it)"
        );
        assert!(plan.mutations.is_empty());
    }

    #[test]
    fn poisoned_bytes_row_bypasses_fast_path_and_heals() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.cbz");
        write_cbz(&p, "a");
        let (size, mtime) = stat_tuple(&p).unwrap();

        let existing = vec![ExistingRow {
            id: 7,
            scheme_tag: "bytes-v1".into(),
            structural_hash: "partial-fingerprint".into(),
            deep_hash: None,
            path: p.to_string_lossy().into_owned(),
            kind: DEFAULT_KIND.into(),
            series_id: None,
            size_bytes: size,
            mtime,
        }];

        let plan = compute_plan(dir.path(), existing, 0).unwrap();
        assert_eq!(plan.stats.unchanged, 0, "fast path bypassed for bytes-v1");
        assert_eq!(plan.stats.updated, 1);
        match plan.mutations.as_slice() {
            [Mutation::UpdateContent {
                id: 7, scheme_tag, ..
            }] => {
                assert_eq!(scheme_tag, "zip-structural-v1", "healed to the zip scheme")
            }
            other => panic!(
                "expected one UpdateContent, got {} mutation(s)",
                other.len()
            ),
        }
    }

    #[test]
    fn targeted_probe_defers_partial_and_reports_it() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.cbz");
        std::fs::write(&p, b"still-copying").unwrap();

        let by_path = HashMap::new();
        let probes = probe_paths(std::slice::from_ref(&p), &by_path);
        assert!(matches!(probes.as_slice(), [Probe::NotReady { .. }]));

        let plan = decide_targeted(dir.path(), probes, &by_path, &HashMap::new());
        assert!(plan.mutations.is_empty());
        assert_eq!(plan.stats.deferred, vec![p.to_string_lossy().into_owned()]);
    }

    #[test]
    fn moved_file_repoints_instead_of_delete_plus_add() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old.cbz");
        let new = dir.path().join("sub").join("new.cbz");
        std::fs::create_dir_all(new.parent().unwrap()).unwrap();
        write_cbz(&new, "same");
        let structural = structural_of(&new);

        let existing = vec![zip_row(42, &old, &structural, DEFAULT_KIND, 0, 0)];

        let plan = compute_plan(dir.path(), existing, 0).unwrap();
        assert_eq!(plan.stats.moved, 1);
        assert_eq!(plan.stats.added, 0);
        assert_eq!(plan.stats.removed, 0);
        assert_eq!(plan.stats.moved_kind_changed, vec![42]);
        assert!(matches!(
            plan.mutations.as_slice(),
            [Mutation::Repoint { id: 42, .. }]
        ));
    }

    #[test]
    fn within_kind_move_does_not_flag_recompute() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("manga").join("arc1").join("v1.cbz");
        let new = dir.path().join("manga").join("arc2").join("v1.cbz");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::create_dir_all(new.parent().unwrap()).unwrap();
        write_cbz(&new, "same");
        let structural = structural_of(&new);

        let mut row = zip_row(9, &old, &structural, "manga", 0, 0);
        row.series_id = Some(7);
        let existing = vec![row];

        let plan = compute_plan(dir.path(), existing, 0).unwrap();
        assert_eq!(plan.stats.moved, 1);
        assert!(
            plan.stats.moved_kind_changed.is_empty(),
            "a within-kind move must not flag a recompute"
        );
        assert!(
            plan.stats.moved_leaf_status_changed.is_empty(),
            "leaf-to-leaf keeps item-graph ownership"
        );
    }

    #[test]
    fn oneshot_to_series_move_flags_item_graph_ownership_change() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("manga").join("v1.cbz");
        let new = dir.path().join("manga").join("Series").join("v1.cbz");
        std::fs::create_dir_all(new.parent().unwrap()).unwrap();
        write_cbz(&new, "same");
        let structural = structural_of(&new);
        let existing = vec![zip_row(9, &old, &structural, "manga", 0, 0)];

        let plan = compute_plan(dir.path(), existing, 0).unwrap();
        assert_eq!(plan.stats.moved_leaf_status_changed, vec![9]);
        assert!(plan.stats.moved_kind_changed.is_empty());
    }

    #[test]
    fn duplicate_content_is_skipped_not_inserted() {
        let dir = tempfile::tempdir().unwrap();
        write_cbz(&dir.path().join("a.cbz"), "same");
        write_cbz(&dir.path().join("b.cbz"), "same");

        let plan = compute_plan(dir.path(), vec![], 0).unwrap();
        assert_eq!(plan.stats.total, 2);
        assert_eq!(plan.stats.added, 1);
        assert_eq!(plan.stats.duplicates, 1);
    }

    #[test]
    fn parallel_identify_preserves_dedup_at_volume() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..100 {
            write_cbz(&dir.path().join(format!("a{i:03}.cbz")), &format!("c{i}"));
            write_cbz(&dir.path().join(format!("b{i:03}.cbz")), &format!("c{i}"));
        }
        let plan = compute_plan(dir.path(), vec![], 0).unwrap();
        assert_eq!(plan.stats.total, 200);
        assert_eq!(plan.stats.added, 100, "one item per unique content");
        assert_eq!(
            plan.stats.duplicates, 100,
            "the second copy of each is a dup"
        );
        assert_eq!(plan.mutations.len(), 100, "100 inserts, no other mutations");

        let plan2 = compute_plan(dir.path(), vec![], 0).unwrap();
        assert_eq!(
            (plan2.stats.added, plan2.stats.duplicates),
            (100, 100),
            "stable across runs"
        );
    }

    #[test]
    #[ignore = "perf measurement; run with --ignored --nocapture"]
    fn perf_scan_parallel_vs_sequential_identify() {
        use std::io::Write as _;
        fn write_multipage_cbz(path: &Path, seed: usize, pages: usize) {
            let f = std::fs::File::create(path).unwrap();
            let mut z = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            for pg in 0..pages {
                z.start_file(format!("{pg:03}.jpg"), opts).unwrap();
                z.write_all(format!("img-{seed}-{pg}").repeat(64).as_bytes())
                    .unwrap();
            }
            z.finish().unwrap();
        }

        let dir = tempfile::tempdir().unwrap();
        let n = 4000usize;
        let mut paths = Vec::with_capacity(n);
        for i in 0..n {
            let p = dir.path().join(format!("f{i:05}.cbz"));
            write_multipage_cbz(&p, i, 24);
            paths.push(p);
        }

        let _ = paths
            .iter()
            .filter(|p| identity::identify(p).is_ok())
            .count();

        let t = std::time::Instant::now();
        let seq = paths
            .iter()
            .filter(|p| identity::identify(p).is_ok())
            .count();
        let seq_ms = t.elapsed().as_secs_f64() * 1000.0;

        let t = std::time::Instant::now();
        let par = paths
            .par_iter()
            .filter(|p| identity::identify(p).is_ok())
            .count();
        let par_ms = t.elapsed().as_secs_f64() * 1000.0;

        let t = std::time::Instant::now();
        let plan = compute_plan(dir.path(), vec![], 0).unwrap();
        let plan_ms = t.elapsed().as_secs_f64() * 1000.0;

        let cores = std::thread::available_parallelism()
            .map(|c| c.get())
            .unwrap_or(1);
        eprintln!(
            "\n[perf] {n} archives (~24 entries each), {cores} cores:\n  \
             identify  sequential = {seq_ms:.0} ms   parallel = {par_ms:.0} ms   \
             speedup = {:.1}x\n  \
             full compute_plan (walk + parallel identify + decide) = {plan_ms:.0} ms  \
             (added={})\n",
            seq_ms / par_ms.max(0.001),
            plan.stats.added
        );
        assert_eq!((seq, par, plan.stats.added), (n, n, n));
    }

    #[test]
    fn vanished_file_is_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let ghost = dir.path().join("ghost.cbz");
        let existing = vec![zip_row(7, &ghost, "gone", DEFAULT_KIND, 1, 1)];

        let plan = compute_plan(dir.path(), existing, 0).unwrap();
        assert_eq!(plan.stats.removed, 1);
        assert!(matches!(
            plan.mutations.as_slice(),
            [Mutation::Delete { id: 7 }]
        ));
        assert_eq!(plan.stats.removed_thumbs, vec!["gone".to_string()]);
    }

    #[sqlx::test]
    async fn targeted_add_inserts_item(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("manga")).unwrap();
        let p = root.join("manga").join("a.cbz");
        write_cbz(&p, "a");

        let stats = reconcile_paths(&pool, root, vec![p.clone()]).await.unwrap();
        assert_eq!(stats.added, 1);
        let rows = items(&pool).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].3, "manga", "kind derived from the folder");
        assert_eq!(
            stats.added_ids,
            vec![(rows[0].0, "manga".to_string())],
            "added_ids exposes the inserted (id, kind) for watcher auto-scrape"
        );
    }

    #[sqlx::test]
    async fn targeted_add_indexes_an_epub(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("books")).unwrap();
        let p = root.join("books").join("live.epub");
        write_epub(&p, "Dropped While Running", "hello");

        assert!(
            crate::scanner::is_archive(&p),
            "the watcher's filter must accept .epub"
        );
        let stats = reconcile_paths(&pool, root, vec![p.clone()]).await.unwrap();
        assert_eq!(
            stats.added, 1,
            "the targeted (watcher) path indexes the epub"
        );
        let (scheme, modality): (String, String) =
            sqlx::query_as("SELECT scheme_tag, modality FROM items")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(scheme, "epub-structural-v1");
        assert_eq!(modality, "reflowable");

        std::fs::remove_file(&p).unwrap();
        let stats = reconcile_paths(&pool, root, vec![p.clone()]).await.unwrap();
        assert_eq!(stats.removed, 1, "deleting the epub removes its item");
        assert!(items(&pool).await.is_empty());
    }

    #[sqlx::test]
    async fn targeted_move_has_no_added_ids(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("manga")).unwrap();
        let old = root.join("manga").join("a.cbz");
        write_cbz(&old, "a");
        reconcile(&pool, root).await.unwrap();

        std::fs::create_dir_all(root.join("comics")).unwrap();
        let new = root.join("comics").join("a.cbz");
        std::fs::rename(&old, &new).unwrap();
        let stats = reconcile_paths(&pool, root, vec![old, new]).await.unwrap();
        assert_eq!(stats.added, 0, "a move is a repoint, not an insert");
        assert!(
            stats.added_ids.is_empty(),
            "a move must not enqueue auto-scrape"
        );
    }

    #[sqlx::test]
    async fn changed_paths_drops_echoes_keeps_real_changes(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("manga")).unwrap();
        let a = root.join("manga").join("a.cbz");
        write_cbz(&a, "a");
        reconcile(&pool, root).await.unwrap();

        assert!(
            changed_paths(&pool, root, std::slice::from_ref(&a))
                .await
                .unwrap()
                .is_empty(),
            "unchanged indexed file is not re-scanned"
        );

        let txt = root.join("manga").join("notes.txt");
        std::fs::write(&txt, b"x").unwrap();
        assert!(
            changed_paths(&pool, root, std::slice::from_ref(&txt))
                .await
                .unwrap()
                .is_empty(),
            "non-archive dropped"
        );

        let b = root.join("manga").join("b.cbz");
        write_cbz(&b, "b");
        assert_eq!(
            changed_paths(&pool, root, std::slice::from_ref(&b))
                .await
                .unwrap(),
            vec![b],
            "a new archive needs a scan"
        );

        write_cbz(&a, "a-with-different-and-bigger-content");
        assert_eq!(
            changed_paths(&pool, root, std::slice::from_ref(&a))
                .await
                .unwrap(),
            vec![a.clone()],
            "a changed archive needs a scan"
        );

        std::fs::remove_file(&a).unwrap();
        assert_eq!(
            changed_paths(&pool, root, std::slice::from_ref(&a))
                .await
                .unwrap(),
            vec![a],
            "a deleted known archive needs a scan"
        );
    }

    #[sqlx::test]
    async fn targeted_modify_updates_content_in_place(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let p = root.join("a.cbz");
        write_cbz(&p, "before");
        reconcile(&pool, root).await.unwrap();
        let (id, structural_before) =
            sqlx::query_as::<_, (i64, String)>("SELECT id, structural_hash FROM items")
                .fetch_one(&pool)
                .await
                .unwrap();

        write_cbz(&p, "after-different-content");
        let stats = reconcile_paths(&pool, root, vec![p.clone()]).await.unwrap();
        assert_eq!(stats.updated, 1);
        let rows = items(&pool).await;
        assert_eq!(rows.len(), 1, "no duplicate row");
        assert_eq!(rows[0].0, id, "same row id — updated in place");
        assert_ne!(
            rows[0].1, structural_before,
            "structural hash reflects new bytes"
        );
    }

    #[sqlx::test]
    async fn targeted_touch_refreshes_without_reinserting(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let p = root.join("a.cbz");
        write_cbz(&p, "a");
        reconcile(&pool, root).await.unwrap();
        let (id, structural) =
            sqlx::query_as::<_, (i64, String)>("SELECT id, structural_hash FROM items")
                .fetch_one(&pool)
                .await
                .unwrap();
        sqlx::query("UPDATE items SET mtime = mtime - 100")
            .execute(&pool)
            .await
            .unwrap();

        let stats = reconcile_paths(&pool, root, vec![p.clone()]).await.unwrap();
        assert_eq!(
            stats.updated, 1,
            "a Touch (content unchanged, stat refreshed)"
        );
        assert_eq!(stats.added, 0);
        let rows = items(&pool).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, id);
        assert_eq!(rows[0].1, structural, "structural hash unchanged");
    }

    #[sqlx::test]
    async fn targeted_delete_removes_row(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let p = root.join("a.cbz");
        write_cbz(&p, "a");
        reconcile(&pool, root).await.unwrap();
        assert_eq!(items(&pool).await.len(), 1);

        std::fs::remove_file(&p).unwrap();
        let stats = reconcile_paths(&pool, root, vec![p.clone()]).await.unwrap();
        assert_eq!(stats.removed, 1);
        assert!(items(&pool).await.is_empty());
    }

    #[sqlx::test]
    async fn targeted_move_preserves_id_and_tags(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("comics")).unwrap();
        std::fs::create_dir_all(root.join("manga")).unwrap();
        let old = root.join("comics").join("a.cbz");
        write_cbz(&old, "a");
        scan(&pool, root).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items")
            .fetch_one(&pool)
            .await
            .unwrap();
        let tag = crate::repo::get_or_create_tag(&pool, "creator", "x")
            .await
            .unwrap();
        crate::repo::add_item_tag(&pool, id, tag, "none", "manual")
            .await
            .unwrap();

        let new = root.join("manga").join("a.cbz");
        std::fs::rename(&old, &new).unwrap();
        let stats = reconcile_paths(&pool, root, vec![old.clone(), new.clone()])
            .await
            .unwrap();

        assert_eq!(stats.moved, 1);
        assert_eq!((stats.added, stats.removed), (0, 0));
        assert_eq!(
            stats.moved_kind_changed,
            vec![id],
            "kind change flagged for recompute"
        );
        let rows = items(&pool).await;
        assert_eq!(rows.len(), 1, "no duplicate/orphan");
        assert_eq!(rows[0].0, id, "SAME row id — repoint, not delete+insert");
        assert_eq!(rows[0].3, "manga", "kind re-derived from the new folder");
        assert_eq!(
            crate::repo::tags_for_item(&pool, id).await.unwrap().len(),
            1,
            "tag survived the move"
        );
    }

    #[sqlx::test]
    async fn targeted_move_detected_from_create_event_alone(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("manga")).unwrap();
        let old = root.join("a.cbz");
        write_cbz(&old, "a");
        reconcile(&pool, root).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items")
            .fetch_one(&pool)
            .await
            .unwrap();

        let new = root.join("manga").join("a.cbz");
        std::fs::rename(&old, &new).unwrap();
        let stats = reconcile_paths(&pool, root, vec![new.clone()])
            .await
            .unwrap();

        assert_eq!(stats.moved, 1, "move detected from the create alone");
        let rows = items(&pool).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, id, "same row — repointed");
        assert!(rows[0].2.contains("manga"));
    }

    #[sqlx::test]
    async fn targeted_duplicate_not_inserted(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let a = root.join("a.cbz");
        write_cbz(&a, "same");
        scan(&pool, root).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items")
            .fetch_one(&pool)
            .await
            .unwrap();

        let b = root.join("b.cbz");
        write_cbz(&b, "same");
        let stats = reconcile_paths(&pool, root, vec![b.clone()]).await.unwrap();

        assert_eq!(stats.duplicates, 1);
        assert_eq!(stats.added, 0);
        let rows = items(&pool).await;
        assert_eq!(rows.len(), 1, "the duplicate copy is not indexed");
        assert_eq!(rows[0].0, id, "original untouched");
    }

    #[sqlx::test]
    async fn targeted_is_scoped_full_scan_is_the_backstop(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let a = root.join("a.cbz");
        let b = root.join("b.cbz");
        write_cbz(&a, "a");
        write_cbz(&b, "b");
        reconcile(&pool, root).await.unwrap();
        assert_eq!(items(&pool).await.len(), 2);

        std::fs::remove_file(&b).unwrap();
        reconcile_paths(&pool, root, vec![a.clone()]).await.unwrap();
        assert_eq!(
            items(&pool).await.len(),
            2,
            "targeted didn't touch un-flagged b"
        );

        let stats = scan(&pool, root).await.unwrap();
        assert_eq!(stats.removed, 1);
        assert_eq!(items(&pool).await.len(), 1);
    }

    #[sqlx::test]
    async fn targeted_noops_are_safe(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let p = root.join("a.cbz");
        write_cbz(&p, "a");
        scan(&pool, root).await.unwrap();
        let before = items(&pool).await;

        let outside = tempfile::tempdir().unwrap();
        let stats = reconcile_paths(
            &pool,
            root,
            vec![
                p.clone(),
                root.join("notes.txt"),
                outside.path().join("x.cbz"),
                root.join("never-existed.cbz"),
            ],
        )
        .await
        .unwrap();
        assert_eq!(
            (stats.added, stats.updated, stats.removed, stats.moved),
            (0, 0, 0, 0),
            "all no-ops"
        );
        assert_eq!(items(&pool).await, before, "DB unchanged");
    }

    #[sqlx::test]
    async fn targeted_stale_remove_of_present_file_is_noop(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let p = root.join("a.cbz");
        write_cbz(&p, "a");
        reconcile(&pool, root).await.unwrap();
        let before = items(&pool).await;

        let stats = reconcile_paths(&pool, root, vec![p.clone()]).await.unwrap();
        assert_eq!(
            stats.removed, 0,
            "a present file is never deleted on a stale event"
        );
        assert_eq!(items(&pool).await, before, "row untouched");
    }

    #[sqlx::test]
    async fn targeted_defers_incomplete_archive_until_readable(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let p = root.join("a.cbz");

        std::fs::write(&p, b"PK\x03\x04 still-transferring, not a complete zip yet").unwrap();
        let stats = reconcile_paths(&pool, root, vec![p.clone()]).await.unwrap();
        assert_eq!(
            (stats.added, stats.errored),
            (0, 0),
            "incomplete archive is deferred, not indexed and not an error"
        );
        assert!(items(&pool).await.is_empty(), "no partial item created");

        write_cbz(&p, "a");
        let stats = reconcile_paths(&pool, root, vec![p.clone()]).await.unwrap();
        assert_eq!(
            stats.added, 1,
            "indexed once the archive is complete/readable"
        );
        assert_eq!(items(&pool).await.len(), 1);
    }

    #[sqlx::test]
    async fn targeted_partial_write_does_not_delete_other_items(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_cbz(&root.join("a.cbz"), "a");
        write_cbz(&root.join("b.cbz"), "b");
        reconcile(&pool, root).await.unwrap();
        let before = items(&pool).await;
        assert_eq!(before.len(), 2);

        let c = root.join("c.cbz");
        std::fs::write(&c, b"PK\x03\x04 partial, mid-transfer, not a zip yet").unwrap();
        let stats = reconcile_paths(&pool, root, vec![c.clone()]).await.unwrap();

        assert_eq!(stats.removed, 0, "a partial write must NOT delete anything");
        assert_eq!(
            items(&pool).await,
            before,
            "a and b are completely untouched"
        );
    }

    #[cfg(unix)]
    #[sqlx::test]
    async fn full_scan_after_content_dir_form_change_preserves_items(pool: SqlitePool) {
        let real = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(real.path().join("manga")).unwrap();
        write_cbz(&real.path().join("manga").join("a.cbz"), "a");
        let link = real.path().parent().unwrap().join("arca-link-content");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(real.path(), &link).unwrap();

        reconcile(&pool, &link).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items")
            .fetch_one(&pool)
            .await
            .unwrap();
        let tag = crate::repo::get_or_create_tag(&pool, "creator", "x")
            .await
            .unwrap();
        crate::repo::add_item_tag(&pool, id, tag, "none", "manual")
            .await
            .unwrap();

        let stats = scan(&pool, real.path()).await.unwrap();
        assert_eq!(
            stats.removed, 0,
            "a content_dir form change must NOT delete items"
        );
        let rows = items(&pool).await;
        assert_eq!(rows.len(), 1, "item survived the migration");
        assert_eq!(rows[0].0, id, "SAME id — repointed, not deleted+re-added");
        assert_eq!(
            crate::repo::tags_for_item(&pool, id).await.unwrap().len(),
            1,
            "tag survived the migration"
        );
        let _ = std::fs::remove_file(&link);
    }
}
