//! Shared library operations for HTTP handlers and background jobs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use anyhow::{Context, Result};
use sqlx::SqlitePool;
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

use crate::media::identity::{self, Identity};
use crate::media::{reader, thumbnail};
use crate::{repo, AppState};

/// Outcome of ingesting an archive into the library (upload or download).
pub struct IngestResult {
    pub id: i64,
    pub title: String,
    pub kind: String,
    /// `false` = an identical file already existed (deduped, nothing written).
    pub created: bool,
}

/// Separates invalid input from internal ingest failures.
#[derive(Debug)]
pub enum IngestError {
    /// The bytes aren't a readable comic archive (or are empty) — a 400.
    BadArchive(String),
    /// Filesystem / DB failure — a 500.
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for IngestError {
    fn from(e: anyhow::Error) -> Self {
        IngestError::Internal(e)
    }
}

impl std::fmt::Display for IngestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IngestError::BadArchive(m) => write!(f, "{m}"),
            IngestError::Internal(e) => write!(f, "{e:#}"),
        }
    }
}

pub async fn ingest_committed_temp(
    read: &SqlitePool,
    write: &SqlitePool,
    content_dir: &Path,
    kind: &str,
    temp: &Path,
    filename: Option<&str>,
    now: i64,
) -> Result<IngestResult, IngestError> {
    let size = tokio::fs::metadata(temp)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    if size == 0 {
        let _ = tokio::fs::remove_file(temp).await;
        return Err(IngestError::BadArchive("empty file".into()));
    }
    let probe = temp.to_path_buf();
    let ident = tokio::task::spawn_blocking(move || identity::identify(&probe))
        .await
        .map_err(|e| anyhow::anyhow!("join: {e}"))?;
    let (scheme, structural_hash, pages) = match ident {
        Ok(Identity::Ready {
            scheme,
            structural_hash,
            pages,
        }) => (scheme, structural_hash, pages),
        _ => {
            let _ = tokio::fs::remove_file(temp).await;
            return Err(IngestError::BadArchive(
                "not a readable comic archive (zip/cbz with image pages)".into(),
            ));
        }
    };
    let is_epub = scheme == identity::Scheme::EpubStructuralV1;
    if !is_epub && pages.is_none_or(|p| p.is_empty()) {
        let _ = tokio::fs::remove_file(temp).await;
        return Err(IngestError::BadArchive(
            "not a readable comic archive (zip/cbz with image pages)".into(),
        ));
    }
    let modality = if is_epub { "reflowable" } else { "paginated" };
    let key = (format!("{}:{}", scheme.tag(), structural_hash), 0usize);
    let lock = INGEST_LOCKS.acquire(key.clone());
    let guard = lock.lock().await;
    let outcome = dedup_and_commit(
        read,
        write,
        content_dir,
        kind,
        temp,
        filename,
        now,
        scheme.tag(),
        &structural_hash,
        size as i64,
        modality,
    )
    .await;
    drop(guard);
    INGEST_LOCKS.release(&key);
    outcome
}

static INGEST_LOCKS: LazyLock<KeyedLocks> = LazyLock::new(KeyedLocks::default);

#[allow(clippy::too_many_arguments)]
async fn dedup_and_commit(
    read: &SqlitePool,
    write: &SqlitePool,
    content_dir: &Path,
    kind: &str,
    temp: &Path,
    filename: Option<&str>,
    now: i64,
    scheme_tag: &str,
    structural_hash: &str,
    size: i64,
    modality: &str,
) -> Result<IngestResult, IngestError> {
    if let Some(meta) = repo::item_by_bucket(read, scheme_tag, structural_hash).await? {
        let _ = tokio::fs::remove_file(temp).await;
        return Ok(IngestResult {
            id: meta.id,
            title: meta.title,
            kind: meta.kind,
            created: false,
        });
    }
    let fallback_ext = if modality == "reflowable" {
        "epub"
    } else {
        "cbz"
    };
    let name = filename
        .and_then(safe_filename)
        .unwrap_or_else(|| format!("{structural_hash}.{fallback_ext}"));
    let kind_dir = content_dir.join(kind);
    tokio::fs::create_dir_all(&kind_dir)
        .await
        .with_context(|| format!("create {}", kind_dir.display()))?;
    let dest = unique_path(&kind_dir, &name);
    tokio::fs::rename(temp, &dest)
        .await
        .with_context(|| format!("commit ingest -> {}", dest.display()))?;
    let mtime = tokio::fs::metadata(&dest)
        .await
        .map(|m| crate::scanner::mtime_secs(&m))
        .unwrap_or(0);
    let raw_title = dest
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_string();
    let format = dest
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_else(|| "cbz".into());
    let id = repo::create_item(
        write,
        scheme_tag,
        structural_hash,
        &dest.to_string_lossy(),
        size,
        mtime,
        &format,
        &raw_title,
        kind,
        modality,
        now,
    )
    .await?;
    Ok(IngestResult {
        id,
        title: crate::media::title::clean(&raw_title),
        kind: kind.to_string(),
        created: true,
    })
}

/// Reduce an untrusted filename to a traversal-safe basename.
pub fn safe_filename(raw: &str) -> Option<String> {
    let name = Path::new(raw).file_name()?.to_str()?;
    if name.is_empty() || name == "." || name == ".." || name.contains(['\\', '\0']) {
        return None;
    }
    Some(name.to_string())
}

/// Find the first available path, adding a numeric suffix when needed.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    unique_suffixed(dir, name)
}

pub(crate) fn unique_suffixed(dir: &Path, name: &str) -> PathBuf {
    let p = Path::new(name);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let ext = p.extension().and_then(|s| s.to_str());
    for i in 1.. {
        let candidate = match ext {
            Some(ext) => format!("{stem}-{i}.{ext}"),
            None => format!("{stem}-{i}"),
        };
        let cand = dir.join(candidate);
        if !cand.exists() {
            return cand;
        }
    }
    unreachable!("an unused filename always exists")
}

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

type KeyLock = Arc<AsyncMutex<()>>;

#[derive(Default)]
pub struct KeyedLocks {
    map: Mutex<HashMap<(String, usize), KeyLock>>,
}

impl KeyedLocks {
    fn acquire(&self, key: (String, usize)) -> KeyLock {
        self.map.lock().unwrap().entry(key).or_default().clone()
    }

    fn release(&self, key: &(String, usize)) {
        let mut map = self.map.lock().unwrap();
        if map.get(key).map(Arc::strong_count).unwrap_or(0) <= 2 {
            map.remove(key);
        }
    }
}

/// The sorted page-name list for an archive — from cache, or built once off the
/// runtime and cached. A cold build also lazily backfills `page_count`.
pub async fn ensure_page_list(
    state: &AppState,
    item_id: i64,
    path: PathBuf,
) -> Result<Arc<Vec<String>>> {
    let key = item_id.to_string();
    let (len, mtime) = match tokio::fs::metadata(&path).await {
        Ok(m) => (m.len(), m.modified().ok()),
        Err(_) => (0, None),
    };
    if let Some(list) = state.page_lists.get(&key, len, mtime) {
        return Ok(list);
    }
    let built = tokio::task::spawn_blocking(move || reader::list_pages(&path)).await??;
    let count = built.len() as i64;
    let list = Arc::new(built);
    state.page_lists.put(key, list.clone(), len, mtime);
    if let Err(e) = repo::set_page_count(&state.write, item_id, count).await {
        tracing::warn!("persist page_count for item {item_id} failed: {e}");
    }
    Ok(list)
}

pub async fn ensure_chapters(
    state: &AppState,
    item_id: i64,
    path: PathBuf,
    modality: &str,
) -> Result<Vec<repo::ChapterRow>> {
    if modality != "paginated" {
        return Ok(Vec::new());
    }
    if repo::chapters_scanned(&state.read, item_id).await? {
        return repo::item_chapters(&state.read, item_id).await;
    }
    let pages = ensure_page_list(state, item_id, path).await?;
    let parsed = crate::media::chapters::parse_chapters(&pages);
    repo::replace_item_chapters(&state.write, item_id, &parsed).await?;
    Ok(parsed
        .into_iter()
        .map(|c| repo::ChapterRow {
            number_sort: c.number_sort,
            number_disp: c.number_disp,
            title: c.title,
            start_page: c.start_page as i64,
            page_count: c.page_count as i64,
        })
        .collect())
}

pub async fn ingest_comicinfo_metadata(
    write: &SqlitePool,
    item_id: i64,
    path: &Path,
) -> Result<usize> {
    let p = path.to_path_buf();
    let info = tokio::task::spawn_blocking(move || {
        Ok::<_, anyhow::Error>(
            crate::media::comicinfo::read_from_archive(&p)?
                .and_then(|xml| crate::media::comicinfo::parse(&xml)),
        )
    })
    .await??;
    let Some(info) = info else { return Ok(0) };

    if let Some(summary) = info.summary.as_deref().filter(|s| !s.is_empty()) {
        repo::set_item_description(write, item_id, summary, Some("comicinfo")).await?;
    }
    if let Some(url) = info.web.as_deref().filter(|u| !u.is_empty()) {
        repo::set_item_source(write, item_id, "comicinfo", url).await?;
    }

    let mut applied = 0usize;
    for (name, role) in &info.creators {
        let tag_id = repo::get_or_create_tag(write, "creator", name).await?;
        if repo::add_item_tag_with_role(write, item_id, tag_id, "none", role, "comicinfo").await? {
            applied += 1;
        }
    }
    for value in &info.content_tags {
        let tag_id = repo::get_or_create_tag(write, "tag", value).await?;
        if repo::add_item_tag(write, item_id, tag_id, "none", "comicinfo").await? {
            applied += 1;
        }
    }
    if let Some(lang) = info.language_iso.as_deref() {
        let value = crate::media::epub::normalize_language(lang);
        if !value.is_empty() {
            let tag_id = repo::get_or_create_tag(write, "language", &value).await?;
            if repo::add_item_tag(write, item_id, tag_id, "none", "comicinfo").await? {
                applied += 1;
            }
        }
    }
    repo::reindex_item_tags(write, item_id).await?;
    Ok(applied)
}

pub async fn ingest_epub_metadata(write: &SqlitePool, item_id: i64, path: &Path) -> Result<usize> {
    let p = path.to_path_buf();
    let meta = tokio::task::spawn_blocking(move || crate::media::epub::inspect(&p)).await??;

    repo::set_item_isbn(write, item_id, meta.isbn.as_deref()).await?;
    repo::set_item_publisher(write, item_id, meta.publisher.as_deref()).await?;
    if let Some(description) = meta.description.as_deref().filter(|d| !d.is_empty()) {
        repo::set_item_description(write, item_id, description, Some("epub")).await?;
    }

    {
        let p2 = path.to_path_buf();
        let spine = meta.spine.clone();
        let words =
            tokio::task::spawn_blocking(move || crate::media::epub::count_words(&p2, &spine))
                .await?
                .unwrap_or(0);
        if words > 0 {
            repo::set_item_word_count(write, item_id, words as i64).await?;
        }
    }

    let mut applied = 0usize;
    for (creator, role) in meta
        .authors
        .iter()
        .flat_map(|a| crate::media::epub::split_creators(a))
    {
        let tag_id = repo::get_or_create_tag(write, "creator", &creator).await?;
        if repo::add_item_tag_with_role(write, item_id, tag_id, "none", role, "epub").await? {
            applied += 1;
        }
    }
    if let Some(lang) = meta.language.as_deref() {
        let value = crate::media::epub::normalize_language(lang);
        if !value.is_empty() {
            let tag_id = repo::get_or_create_tag(write, "language", &value).await?;
            if repo::add_item_tag(write, item_id, tag_id, "none", "epub").await? {
                applied += 1;
            }
        }
    }
    repo::reindex_item_tags(write, item_id).await?;
    Ok(applied)
}

pub async fn repair_epub_descriptions(write: &SqlitePool) -> Result<usize> {
    let rows: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT id, path, description FROM items \
         WHERE modality = 'reflowable' AND description IS NOT NULL AND (\
           lower(description) LIKE '<p>genre:%' OR \
           lower(description) LIKE '%<p>ebook,%')",
    )
    .fetch_all(write)
    .await?;

    let mut repaired = 0;
    for (id, path, old) in rows {
        let parsed =
            tokio::task::spawn_blocking(move || crate::media::epub::inspect(Path::new(&path)))
                .await??;
        if let Some(description) = parsed.description.filter(|d| !d.is_empty() && d != &old) {
            repo::set_item_description(write, id, &description, Some("epub")).await?;
            repaired += 1;
        }
    }
    Ok(repaired)
}

pub async fn ensure_thumbnail(
    state: &AppState,
    item_id: i64,
    cache_key: &str,
    path: &Path,
    modality: &str,
) -> Result<Vec<u8>> {
    let cache = thumbnail::cache_path(&state.config.data_dir, cache_key);
    if let Ok(bytes) = tokio::fs::read(&cache).await {
        return Ok(bytes);
    }

    let cover = if modality == "reflowable" {
        let p = path.to_path_buf();
        tokio::task::spawn_blocking(move || crate::media::epub::cover_entry(&p)).await??
    } else {
        let list = ensure_page_list(state, item_id, path.to_path_buf()).await?;
        list.first().context("archive has no image pages")?.clone()
    };

    let permit = state.blocking_limiter.clone().acquire_owned().await?;
    let read_path = path.to_path_buf();
    let bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
        let _permit = permit;
        let (cover_bytes, _) = reader::read_entry(&read_path, &cover)?;
        thumbnail::generate_webp_thumbnail(&cover_bytes, thumbnail::COVER_WIDTH, 80)
    })
    .await??;

    write_atomic(&cache, &bytes).await;
    Ok(bytes)
}

pub async fn ensure_page_thumbnail(
    state: &AppState,
    cache_key: &str,
    path: &Path,
    page: usize,
    page_name: &str,
) -> Result<Vec<u8>> {
    let cache = thumbnail::page_cache_path(&state.config.data_dir, cache_key, page);
    if let Ok(bytes) = tokio::fs::read(&cache).await {
        return Ok(bytes);
    }

    let key = (cache_key.to_string(), page);
    let lock = state.page_thumb_locks.acquire(key.clone());
    let guard = lock.lock().await;

    let result = match tokio::fs::read(&cache).await {
        Ok(bytes) => Ok(bytes),
        Err(_) => generate_page_thumbnail(state, &cache, path, page_name).await,
    };
    drop(guard);
    state.page_thumb_locks.release(&key);
    result
}

/// Decode + resize one page to a cached WebP. Assumes the single-flight lock is
/// held and the cache is confirmed missing.
async fn generate_page_thumbnail(
    state: &AppState,
    cache: &Path,
    path: &Path,
    page_name: &str,
) -> Result<Vec<u8>> {
    let permit = state.blocking_limiter.clone().acquire_owned().await?;
    let read_path = path.to_path_buf();
    let name = page_name.to_string();
    let bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
        let _permit = permit;
        let (page_bytes, _) = reader::read_entry(&read_path, &name)?;
        thumbnail::generate_webp_thumbnail(&page_bytes, thumbnail::PAGE_THUMB_WIDTH, 70)
    })
    .await??;

    write_atomic(cache, &bytes).await;
    Ok(bytes)
}

async fn write_atomic(cache: &Path, bytes: &[u8]) {
    if let Some(parent) = cache.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = cache.with_extension(format!("tmp{seq}"));
    match tokio::fs::write(&tmp, bytes).await {
        Ok(()) => {
            if let Err(e) = tokio::fs::rename(&tmp, cache).await {
                tracing::warn!("thumbnail cache rename {} failed: {e}", cache.display());
                let _ = tokio::fs::remove_file(&tmp).await;
            }
        }
        Err(e) => {
            tracing::warn!("thumbnail cache write {} failed: {e}", tmp.display());
            let _ = tokio::fs::remove_file(&tmp).await;
        }
    }
}

pub async fn sweep_thumbnails(state: &AppState, cancel: &CancellationToken) -> Result<Vec<String>> {
    let cap = state.config.read_concurrency.max(1) * 2;
    let mut set: tokio::task::JoinSet<Option<String>> = tokio::task::JoinSet::new();
    let mut last_id = 0i64;
    let mut queued = 0usize;
    let mut failed: Vec<String> = Vec::new();

    loop {
        if cancel.is_cancelled() {
            break;
        }
        let batch: Vec<(i64, String, String, Option<i64>)> = sqlx::query_as(
            "SELECT id, structural_hash, COALESCE(modality_override, modality), phash \
             FROM items WHERE id > ? ORDER BY id LIMIT 200",
        )
        .bind(last_id)
        .fetch_all(&state.read)
        .await?;
        if batch.is_empty() {
            break;
        }

        for (aid, structural, modality, phash) in batch {
            if cancel.is_cancelled() {
                break;
            }
            last_id = aid;
            let cache = thumbnail::cache_path(&state.config.data_dir, &structural);
            let cover_cached = tokio::fs::try_exists(&cache).await.unwrap_or(false);
            let needs_phash = phash.is_none();
            if cover_cached && !needs_phash {
                continue;
            }
            let path = if cover_cached {
                None
            } else {
                match repo::path_of(&state.read, aid).await? {
                    Some(p) => Some(p),
                    None => continue,
                }
            };

            while set.len() >= cap {
                if let Some(Ok(Some(p))) = set.join_next().await {
                    failed.push(p);
                }
            }
            let st = state.clone();
            set.spawn(async move {
                let cover: Option<Vec<u8>> = if cover_cached {
                    tokio::fs::read(&cache).await.ok()
                } else {
                    let p = path.expect("path present when generating");
                    match ensure_thumbnail(&st, aid, &structural, &p, &modality).await {
                        Ok(bytes) => Some(bytes),
                        Err(e) if e.downcast_ref::<crate::media::epub::NoCover>().is_some() => {
                            tracing::debug!(
                                "thumbnail unavailable for item {aid}: EPUB contains no cover"
                            );
                            return None;
                        }
                        Err(e) => {
                            tracing::warn!("thumbnail sweep failed for item {aid}: {e:#}");
                            return Some(p.to_string_lossy().into_owned());
                        }
                    }
                };
                if needs_phash {
                    if let Some(bytes) = cover {
                        if let Ok(Some(h)) =
                            tokio::task::spawn_blocking(move || thumbnail::dhash(&bytes)).await
                        {
                            if let Err(e) = repo::set_phash(&st.write, aid, h).await {
                                tracing::warn!("set_phash failed for item {aid}: {e:#}");
                            }
                        }
                    }
                }
                None
            });
            queued += 1;
        }
    }

    while let Some(res) = set.join_next().await {
        if let Ok(Some(p)) = res {
            failed.push(p);
        }
    }
    tracing::info!(
        missing = queued,
        failed = failed.len(),
        "thumbnail sweep complete"
    );
    Ok(failed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn write_cbz(path: &Path, content: &str) {
        let f = std::fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default();
        z.start_file("001.jpg", opts).unwrap();
        z.write_all(format!("img-{content}").as_bytes()).unwrap();
        z.finish().unwrap();
    }

    fn write_epub_rich(path: &Path, title: &str, creator: &str, lang: &str, isbn: &str) {
        let f = std::fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default();
        let mut put = |name: &str, data: &[u8]| {
            z.start_file(name, opts).unwrap();
            z.write_all(data).unwrap();
        };
        put(
            "META-INF/container.xml",
            br#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
        );
        let opf = format!(
            r#"<package xmlns="http://www.idpf.org/2007/opf"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>{title}</dc:title><dc:creator>{creator}</dc:creator><dc:language>{lang}</dc:language><dc:identifier>{isbn}</dc:identifier><dc:description>&lt;p&gt;A useful synopsis.&lt;/p&gt;</dc:description></metadata><manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#
        );
        put("content.opf", opf.as_bytes());
        put("c1.xhtml", b"<html/>");
        z.finish().unwrap();
    }

    fn write_cbz_with_comicinfo(path: &Path) {
        let f = std::fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default();
        let mut put = |name: &str, data: &[u8]| {
            z.start_file(name, opts).unwrap();
            z.write_all(data).unwrap();
        };
        put(
            "ComicInfo.xml",
            br#"<?xml version="1.0"?>
<ComicInfo xmlns:xsd="http://www.w3.org/2001/XMLSchema">
  <Series>Batman: The Dark Knight Returns</Series>
  <Title>Book One</Title>
  <Web>https://example.test/comic/330474</Web>
  <Summary>Frank Miller reinvents the legend of Batman.</Summary>
  <Writer>Frank Miller</Writer>
  <Penciller>Frank Miller</Penciller>
  <Inker>Klaus Janson</Inker>
  <Publisher>DC</Publisher>
  <Genre>Crime, Superhero</Genre>
  <LanguageISO>en</LanguageISO>
</ComicInfo>"#,
        );
        put("Release Folder/DKR-000.jpg", b"img0");
        put("Release Folder/DKR-001.jpg", b"img1");
        z.finish().unwrap();
    }

    #[sqlx::test]
    async fn comicinfo_enrichment_maps_creators_tags_language_and_provenance(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let content = dir.path();
        std::fs::create_dir_all(content.join("comics")).unwrap();
        let p = content.join("comics").join("dkr.cbz");
        write_cbz_with_comicinfo(&p);
        write_cbz(&content.join("comics").join("bare.cbz"), "bare");

        crate::scanner::scan(&pool, content).await.unwrap();
        let (id, path): (i64, String) =
            sqlx::query_as("SELECT id, path FROM items WHERE path LIKE '%dkr.cbz'")
                .fetch_one(&pool)
                .await
                .unwrap();
        let (bare_id, bare_path): (i64, String) =
            sqlx::query_as("SELECT id, path FROM items WHERE path LIKE '%bare.cbz'")
                .fetch_one(&pool)
                .await
                .unwrap();

        let applied = ingest_comicinfo_metadata(&pool, id, Path::new(&path))
            .await
            .unwrap();
        assert_eq!(applied, 6);
        assert_eq!(
            ingest_comicinfo_metadata(&pool, bare_id, Path::new(&bare_path))
                .await
                .unwrap(),
            0,
            "an archive without ComicInfo.xml is a no-op"
        );

        let (description, description_source): (Option<String>, Option<String>) =
            sqlx::query_as("SELECT description, description_source FROM items WHERE id = ?")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            description.as_deref(),
            Some("Frank Miller reinvents the legend of Batman.")
        );
        assert_eq!(description_source.as_deref(), Some("comicinfo"));

        let url: Option<String> = sqlx::query_scalar(
            "SELECT url FROM item_sources WHERE item_id = ? AND source = 'comicinfo'",
        )
        .bind(id)
        .fetch_optional(&pool)
        .await
        .unwrap();
        assert_eq!(url.as_deref(), Some("https://example.test/comic/330474"));

        let mut tags: Vec<(String, String, String, String)> = sqlx::query_as(
            "SELECT t.namespace, t.value, it.role, it.source              FROM item_tags it JOIN tags t ON t.id = it.tag_id WHERE it.item_id = ?",
        )
        .bind(id)
        .fetch_all(&pool)
        .await
        .unwrap();
        tags.sort();
        assert_eq!(
            tags,
            vec![
                (
                    "creator".into(),
                    "frank miller".into(),
                    "penciller".into(),
                    "comicinfo".into()
                ),
                (
                    "creator".into(),
                    "frank miller".into(),
                    "writer".into(),
                    "comicinfo".into()
                ),
                (
                    "creator".into(),
                    "klaus janson".into(),
                    "inker".into(),
                    "comicinfo".into()
                ),
                (
                    "language".into(),
                    "english".into(),
                    "none".into(),
                    "comicinfo".into()
                ),
                (
                    "tag".into(),
                    "crime".into(),
                    "none".into(),
                    "comicinfo".into()
                ),
                (
                    "tag".into(),
                    "superhero".into(),
                    "none".into(),
                    "comicinfo".into()
                ),
            ]
        );
        let title: String = sqlx::query_scalar("SELECT title FROM items WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            title, "dkr",
            "ComicInfo Title/Series never override the title"
        );
    }

    #[sqlx::test]
    async fn epub_enrichment_writes_author_language_tags_and_isbn(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let content = dir.path();
        std::fs::create_dir_all(content.join("light novel")).unwrap();
        let p = content.join("light novel").join("86 v01 [Yen Press].epub");
        write_epub_rich(
            &p,
            "86—EIGHTY-SIX, Vol. 01",
            "Asato Asato and Shirabii",
            "en",
            "9781975303136",
        );

        crate::scanner::scan(&pool, content).await.unwrap();
        let (id, path): (i64, String) =
            sqlx::query_as("SELECT id, path FROM items WHERE modality = 'reflowable'")
                .fetch_one(&pool)
                .await
                .unwrap();

        let applied = ingest_epub_metadata(&pool, id, Path::new(&path))
            .await
            .unwrap();
        assert_eq!(applied, 3, "two creators + one language");

        let isbn: Option<String> = sqlx::query_scalar("SELECT isbn FROM items WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(isbn.as_deref(), Some("9781975303136"));

        let description: Option<String> =
            sqlx::query_scalar("SELECT description FROM items WHERE id = ?")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(description.as_deref(), Some("<p>A useful synopsis.</p>"));

        let mut tags: Vec<(String, String)> = sqlx::query_as(
            "SELECT t.namespace, t.value FROM item_tags it JOIN tags t ON t.id = it.tag_id \
             WHERE it.item_id = ? ORDER BY t.namespace, t.value",
        )
        .bind(id)
        .fetch_all(&pool)
        .await
        .unwrap();
        tags.sort();
        assert_eq!(
            tags,
            vec![
                ("creator".to_string(), "asato asato".to_string()),
                ("creator".to_string(), "shirabii".to_string()),
                ("language".to_string(), "english".to_string()),
            ]
        );

        let sort_creator: Option<String> =
            sqlx::query_scalar("SELECT sort_creator FROM items WHERE id = ?")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(sort_creator.as_deref(), Some("asato asato"));
    }

    #[sqlx::test]
    async fn concurrent_ingest_of_identical_content_creates_one_item(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let content_dir = dir.path();
        let t1 = content_dir.join(".up1.tmp");
        let t2 = content_dir.join(".up2.tmp");
        write_cbz(&t1, "same");
        write_cbz(&t2, "same");

        let a = ingest_committed_temp(&pool, &pool, content_dir, "manga", &t1, Some("a.cbz"), 0);
        let b = ingest_committed_temp(&pool, &pool, content_dir, "manga", &t2, Some("b.cbz"), 0);
        let (ra, rb) = tokio::join!(a, b);
        let ra = ra.unwrap();
        let rb = rb.unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM items")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            count, 1,
            "concurrent identical ingests must collapse to ONE item"
        );
        assert_eq!(ra.id, rb.id, "both results point at the same item");
        assert_ne!(
            ra.created, rb.created,
            "exactly one created, the other deduped"
        );
    }

    #[sqlx::test]
    async fn ingest_accepts_an_epub_as_a_reflowable_item(pool: SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let content_dir = dir.path();
        let temp = content_dir.join(".book.tmp");
        write_epub_rich(&temp, "Pride and Prejudice", "Jane Austen", "en", "");

        let res = ingest_committed_temp(
            &pool,
            &pool,
            content_dir,
            "books",
            &temp,
            Some("Pride and Prejudice.epub"),
            0,
        )
        .await
        .expect("an EPUB download must ingest, not be rejected as a non-comic archive");
        assert!(res.created);

        let (modality, format, kind): (String, String, String) =
            sqlx::query_as("SELECT modality, format, kind FROM items WHERE id = ?")
                .bind(res.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(modality, "reflowable");
        assert_eq!(format, "epub");
        assert_eq!(kind, "books");
    }
}
