//! LRU cache validated against file size and modification time.

use std::borrow::Borrow;
use std::hash::Hash;
use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::time::SystemTime;

use lru::LruCache;

struct Entry<V> {
    value: V,
    len: u64,
    mtime: Option<SystemTime>,
}

pub(crate) struct StatValidatedLru<K: Hash + Eq, V: Clone> {
    inner: Mutex<LruCache<K, Entry<V>>>,
}

impl<K: Hash + Eq, V: Clone> StatValidatedLru<K, V> {
    pub(crate) fn new(capacity: usize) -> Self {
        let cap = NonZeroUsize::new(capacity).expect("stat-validated cache capacity must be > 0");
        Self {
            inner: Mutex::new(LruCache::new(cap)),
        }
    }

    /// Return a value only when its file stamp still matches.
    pub(crate) fn get_validated<Q>(&self, key: &Q, len: u64, mtime: Option<SystemTime>) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let mut inner = self.inner.lock().unwrap();
        if let Some(e) = inner.get(key) {
            if e.len == len && e.mtime == mtime {
                return Some(e.value.clone());
            }
        }
        inner.pop(key);
        None
    }

    pub(crate) fn put(&self, key: K, value: V, len: u64, mtime: Option<SystemTime>) {
        self.inner
            .lock()
            .unwrap()
            .put(key, Entry { value, len, mtime });
    }
}
