//! Shared in-process metadata cache primitives.
//!
//! This module intentionally keeps cache behavior simple and explicit:
//! - bounded best-effort size
//! - TTL-based expiry
//! - optional monotonic version guard for out-of-order invalidation handling
//! - explicit invalidate by key or whole-cache

use std::collections::HashMap;
use std::hash::Hash;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

#[derive(Debug, Clone)]
struct CacheEntry<V> {
    value: V,
    expires_at: Instant,
    version: Option<i64>,
}

/// Generic read-through metadata cache with TTL and optional version guards.
#[derive(Debug)]
pub struct MetadataCache<K, V> {
    ttl: Duration,
    max_entries: usize,
    entries: RwLock<HashMap<K, CacheEntry<V>>>,
}

impl<K, V> MetadataCache<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    /// Create a cache with TTL and max-entry bound.
    pub fn new(ttl: Duration, max_entries: usize) -> Self {
        Self {
            ttl,
            max_entries,
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// Get a value by key if present and not expired.
    pub async fn get(&self, key: &K) -> Option<V> {
        let mut guard = self.entries.write().await;
        let expired = guard
            .get(key)
            .is_some_and(|entry| Instant::now() >= entry.expires_at);
        if expired {
            guard.remove(key);
            return None;
        }
        guard.get(key).map(|entry| entry.value.clone())
    }

    /// Insert or replace a value without version checks.
    pub async fn insert(&self, key: K, value: V) {
        self.insert_with_version(key, value, None).await;
    }

    /// Insert or replace a value with optional monotonic version metadata.
    pub async fn insert_with_version(&self, key: K, value: V, version: Option<i64>) {
        let mut guard = self.entries.write().await;
        if guard.len() >= self.max_entries {
            // Best-effort bounded growth: remove one arbitrary key.
            if let Some(first) = guard.keys().next().cloned() {
                guard.remove(&first);
            }
        }
        guard.insert(
            key,
            CacheEntry {
                value,
                expires_at: Instant::now() + self.ttl,
                version,
            },
        );
    }

    /// Insert only when the incoming version is newer than the existing entry.
    ///
    /// If either side has no version, this method inserts unconditionally.
    pub async fn insert_if_newer(&self, key: K, value: V, version: Option<i64>) {
        let mut guard = self.entries.write().await;
        let should_insert = match (guard.get(&key).and_then(|entry| entry.version), version) {
            (Some(current), Some(incoming)) => incoming > current,
            _ => true,
        };
        if !should_insert {
            return;
        }
        if guard.len() >= self.max_entries {
            if let Some(first) = guard.keys().next().cloned() {
                guard.remove(&first);
            }
        }
        guard.insert(
            key,
            CacheEntry {
                value,
                expires_at: Instant::now() + self.ttl,
                version,
            },
        );
    }

    /// Remove a single key.
    pub async fn invalidate_key(&self, key: &K) -> bool {
        self.entries.write().await.remove(key).is_some()
    }

    /// Clear all entries.
    pub async fn invalidate_all(&self) {
        self.entries.write().await.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::MetadataCache;
    use std::time::Duration;

    #[tokio::test]
    async fn cache_entry_expires_after_ttl() {
        let cache = MetadataCache::<String, String>::new(Duration::from_millis(20), 16);
        cache.insert("a".to_string(), "v1".to_string()).await;
        assert_eq!(cache.get(&"a".to_string()).await, Some("v1".to_string()));

        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(cache.get(&"a".to_string()).await, None);
    }

    #[tokio::test]
    async fn insert_if_newer_respects_version() {
        let cache = MetadataCache::<String, String>::new(Duration::from_secs(30), 16);
        let key = "k".to_string();
        cache
            .insert_with_version(key.clone(), "v1".to_string(), Some(10))
            .await;
        cache
            .insert_if_newer(key.clone(), "older".to_string(), Some(9))
            .await;
        assert_eq!(cache.get(&key).await, Some("v1".to_string()));

        cache
            .insert_if_newer(key.clone(), "v2".to_string(), Some(11))
            .await;
        assert_eq!(cache.get(&key).await, Some("v2".to_string()));
    }
}
