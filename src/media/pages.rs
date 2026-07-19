//! Stat-validated LRU cache of ordered page lists.

use std::sync::Arc;
use std::time::SystemTime;

use crate::server::stat_lru::StatValidatedLru;

pub struct PageListCache {
    inner: StatValidatedLru<String, Arc<Vec<String>>>,
}

impl PageListCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: StatValidatedLru::new(capacity),
        }
    }

    pub fn get(&self, key: &str, len: u64, mtime: Option<SystemTime>) -> Option<Arc<Vec<String>>> {
        self.inner.get_validated(key, len, mtime)
    }

    pub fn put(&self, key: String, value: Arc<Vec<String>>, len: u64, mtime: Option<SystemTime>) {
        self.inner.put(key, value, len, mtime);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one() -> Arc<Vec<String>> {
        Arc::new(vec!["1.jpg".to_string()])
    }

    #[test]
    fn evicts_least_recently_used() {
        let cache = PageListCache::new(2);
        cache.put("a".into(), one(), 10, None);
        cache.put("b".into(), one(), 10, None);
        assert!(cache.get("a", 10, None).is_some());
        cache.put("c".into(), one(), 10, None);
        assert!(cache.get("a", 10, None).is_some());
        assert!(
            cache.get("b", 10, None).is_none(),
            "b should have been evicted"
        );
        assert!(cache.get("c", 10, None).is_some());
    }

    #[test]
    fn changed_stat_is_a_miss_and_drops_the_entry() {
        let cache = PageListCache::new(2);
        cache.put("a".into(), Arc::new(vec!["1.jpg".into()]), 100, None);
        assert!(cache.get("a", 100, None).is_some(), "same stat → hit");
        assert!(
            cache.get("a", 999, None).is_none(),
            "changed len is stale, a miss"
        );
        assert!(
            cache.get("a", 100, None).is_none(),
            "the stale entry was dropped, not merely skipped"
        );
    }
}
