//! Short-lived disk cache for external browse responses.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static WRITE_SEQ: AtomicU64 = AtomicU64::new(0);

pub struct BrowseCache {
    dir: PathBuf,
}

impl BrowseCache {
    pub fn new(data_dir: &Path) -> Self {
        BrowseCache {
            dir: data_dir.join("cache").join("browse"),
        }
    }

    fn path_for(&self, key: &str) -> PathBuf {
        let hex = blake3::hash(key.as_bytes()).to_hex();
        let hex = hex.as_str();
        self.dir.join(&hex[0..2]).join(format!("{hex}.json"))
    }

    /// Returns `None` for missing, stale, or disabled (`ttl == 0`) entries.
    pub async fn get(&self, key: &str, ttl: Duration) -> Option<Vec<u8>> {
        if ttl.is_zero() {
            return None;
        }
        let path = self.path_for(key);
        let meta = tokio::fs::metadata(&path).await.ok()?;
        let fresh = meta
            .modified()
            .ok()
            .and_then(|m| m.elapsed().ok())
            .is_some_and(|age| age <= ttl);
        if !fresh {
            return None;
        }
        tokio::fs::read(&path).await.ok()
    }

    /// Stores an entry atomically. Cache write failures are ignored.
    pub async fn put(&self, key: &str, bytes: &[u8]) {
        let path = self.path_for(key);
        let Some(parent) = path.parent() else {
            return;
        };
        if tokio::fs::create_dir_all(parent).await.is_err() {
            return;
        }
        let seq = WRITE_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = parent.join(format!(".tmp-{seq}"));
        if tokio::fs::write(&tmp, bytes).await.is_ok()
            && tokio::fs::rename(&tmp, &path).await.is_err()
        {
            let _ = tokio::fs::remove_file(&tmp).await;
        }
    }

    pub async fn clear(&self) {
        let _ = tokio::fs::remove_dir_all(&self.dir).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn put_get_and_clear() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = BrowseCache::new(tmp.path());
        let ttl = Duration::from_secs(300);

        assert!(cache.get("browse:openlibrary:popular", ttl).await.is_none());

        cache.put("browse:openlibrary:popular", b"[1,2,3]").await;
        assert_eq!(
            cache
                .get("browse:openlibrary:popular", ttl)
                .await
                .as_deref(),
            Some(&b"[1,2,3]"[..])
        );

        assert!(cache
            .get("browse:openlibrary:popular", Duration::ZERO)
            .await
            .is_none());
        assert!(cache
            .get("browse:openlibrary:popular", Duration::ZERO)
            .await
            .is_none());

        assert!(cache.get("browse:openlibrary:recent", ttl).await.is_none());

        cache.clear().await;
        assert!(cache.get("browse:openlibrary:popular", ttl).await.is_none());
    }
}
