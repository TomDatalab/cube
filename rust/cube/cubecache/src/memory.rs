//! In-process cache implementations.
//!
//! [`MemoryCacheDriver`] is the port of `LocalCacheDriver`
//! (`QO/LocalCacheDriver.ts`), the `cacheAndQueueDriver: 'memory'` backend.
//! [`MemoryResultCache`] is the port of the `LRUCache` that `QueryCache` keeps in front of
//! it (`QO/QueryCache.ts:230-232, 1189-1221`).

use std::{
    collections::HashMap,
    num::NonZeroUsize,
    sync::{Mutex, MutexGuard},
};

use async_trait::async_trait;
use lru::LruCache;
use serde_json::Value;

use crate::{
    driver::{CacheDriver, LockCallback, SetResult},
    entry::{is_memory_entry_usable, should_store_in_memory, CacheEntry},
    error::CacheError,
};

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[derive(Clone, Debug)]
struct Bucket {
    value: Value,
    /// Epoch milliseconds at which the bucket stops being served.
    exp: i64,
}

/// In-memory cache driver with per key TTL.
///
/// Unlike `LocalCacheDriver`, whose backing store is a module level singleton shared by
/// every instance in the process, each driver owns its store. The singleton is invisible
/// in production (one `QueryCache` per process) but it leaks state between tests.
#[derive(Debug, Default)]
pub struct MemoryCacheDriver {
    store: Mutex<HashMap<String, Bucket>>,
}

impl MemoryCacheDriver {
    pub fn new() -> Self {
        Self::default()
    }

    fn store(&self) -> MutexGuard<'_, HashMap<String, Bucket>> {
        self.store.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// Drops every key, `LocalCacheDriver.reset`.
    pub fn reset(&self) {
        self.store().clear();
    }

    /// Number of stored keys, expired ones included.
    pub fn len(&self) -> usize {
        self.store().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait]
impl CacheDriver for MemoryCacheDriver {
    async fn get(&self, key: &str) -> Result<Option<Value>, CacheError> {
        let mut store = self.store();

        if let Some(bucket) = store.get(key) {
            if bucket.exp < now_ms() {
                store.remove(key);
            }
        }

        Ok(store.get(key).map(|bucket| bucket.value.clone()))
    }

    async fn set(&self, key: &str, value: Value, ttl_secs: u64) -> Result<SetResult, CacheError> {
        let bytes = serde_json::to_string(&value)?.len();

        self.store().insert(
            key.to_string(),
            Bucket {
                value,
                exp: now_ms() + ttl_secs as i64 * 1000,
            },
        );

        Ok(SetResult {
            key: key.to_string(),
            bytes,
        })
    }

    async fn remove(&self, key: &str) -> Result<(), CacheError> {
        self.store().remove(key);

        Ok(())
    }

    async fn keys_starting_with(&self, prefix: &str) -> Result<Vec<String>, CacheError> {
        let now = now_ms();

        Ok(self
            .store()
            .iter()
            .filter(|(key, bucket)| key.starts_with(prefix) && bucket.exp > now)
            .map(|(key, _)| key.clone())
            .collect())
    }

    async fn with_lock<'a>(
        &'a self,
        key: &'a str,
        ttl_secs: u64,
        free_after: bool,
        callback: LockCallback<'a>,
    ) -> Result<bool, CacheError> {
        {
            let mut store = self.store();

            if store.contains_key(key) {
                // Faithful to `LocalCacheDriver.withLock`: an expired lock is dropped here
                // but the call still reports the lock as taken, so the first caller after
                // the expiry loses and the next one wins.
                if store.get(key).map(|bucket| bucket.exp).unwrap_or(0) < now_ms() {
                    store.remove(key);
                }

                return Ok(false);
            }

            store.insert(
                key.to_string(),
                Bucket {
                    value: Value::Bool(true),
                    exp: now_ms() + ttl_secs as i64 * 1000,
                },
            );
        }

        let result = callback.await;

        if free_after {
            self.store().remove(key);
        }

        result.map(|()| true)
    }

    async fn test_connection(&self) -> Result<(), CacheError> {
        Ok(())
    }
}

/// The bounded in-memory result cache `QueryCache` keeps in front of the cache driver.
///
/// `maxInMemoryCacheEntries` defaults to 10000 (`QO/QueryCache.ts:230-232`).
#[derive(Debug)]
pub struct MemoryResultCache {
    inner: Mutex<LruCache<String, CacheEntry>>,
}

impl MemoryResultCache {
    pub const DEFAULT_MAX_ENTRIES: usize = 10000;

    /// `max_entries` of `None` or `0` means `maxInMemoryCacheEntries || 10000`.
    pub fn new(max_entries: Option<usize>) -> Self {
        let max = max_entries
            .and_then(NonZeroUsize::new)
            .unwrap_or_else(|| NonZeroUsize::new(Self::DEFAULT_MAX_ENTRIES).unwrap());

        Self {
            inner: Mutex::new(LruCache::new(max)),
        }
    }

    fn inner(&self) -> MutexGuard<'_, LruCache<String, CacheEntry>> {
        self.inner.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// Reads an entry and marks it as recently used.
    pub fn get(&self, key: &str) -> Option<CacheEntry> {
        self.inner().get(key).cloned()
    }

    pub fn set(&self, key: &str, entry: CacheEntry) {
        self.inner().put(key.to_string(), entry);
    }

    pub fn remove(&self, key: &str) {
        self.inner().pop(key);
    }

    pub fn len(&self) -> usize {
        self.inner().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn max_entries(&self) -> usize {
        self.inner().cap().get()
    }

    /// Port of `QueryCache.getFromMemoryCache` (`QO/QueryCache.ts:1189-1213`): returns the
    /// entry only while it is usable and evicts it otherwise.
    pub fn get_usable(
        &self,
        key: &str,
        expiration_secs: u64,
        renewal_threshold: Option<u64>,
        renewal_key: Option<&str>,
        now_ms: i64,
    ) -> Option<CacheEntry> {
        let entry = self.get(key)?;
        let renewed_ago = entry.renewed_ago(now_ms);

        if !is_memory_entry_usable(
            &entry,
            renewed_ago,
            expiration_secs,
            renewal_threshold,
            renewal_key,
        ) {
            self.remove(key);
            return None;
        }

        Some(entry)
    }

    /// Port of `QueryCache.storeInMemoryCache` (`QO/QueryCache.ts:1215-1221`).
    pub fn store_if_fresh(
        &self,
        key: &str,
        entry: &CacheEntry,
        renewed_ago_ms: i64,
        use_in_memory: bool,
        renewal_threshold: Option<u64>,
    ) -> bool {
        if should_store_in_memory(renewed_ago_ms, use_in_memory, renewal_threshold) {
            self.set(key, entry.clone());
            return true;
        }

        false
    }
}

impl Default for MemoryResultCache {
    fn default() -> Self {
        Self::new(None)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::{driver::get_entry, driver::set_entry, entry::IN_MEMORY_CACHE_DISABLE_PERIOD_MS};

    #[tokio::test]
    async fn get_set_and_remove() {
        let driver = MemoryCacheDriver::new();

        assert_eq!(driver.get("missing").await.unwrap(), None);

        let set = driver.set("k", json!({ "a": 1 }), 60).await.unwrap();
        assert_eq!(set.key, "k");
        assert_eq!(set.bytes, r#"{"a":1}"#.len());
        assert_eq!(driver.get("k").await.unwrap(), Some(json!({ "a": 1 })));

        driver.remove("k").await.unwrap();
        assert_eq!(driver.get("k").await.unwrap(), None);
        assert!(driver.is_empty());
    }

    #[tokio::test]
    async fn expired_keys_are_dropped_on_read() {
        let driver = MemoryCacheDriver::new();
        driver.set("k", json!(1), 0).await.unwrap();

        tokio::time::sleep(Duration::from_millis(20)).await;

        assert_eq!(driver.get("k").await.unwrap(), None);
        assert!(driver.is_empty());
    }

    #[tokio::test]
    async fn keys_starting_with_skips_expired_keys() {
        let driver = MemoryCacheDriver::new();
        driver.set("pre:a", json!(1), 60).await.unwrap();
        driver.set("pre:b", json!(2), 0).await.unwrap();
        driver.set("other", json!(3), 60).await.unwrap();

        tokio::time::sleep(Duration::from_millis(20)).await;

        assert_eq!(
            driver.keys_starting_with("pre:").await.unwrap(),
            vec!["pre:a".to_string()]
        );
    }

    #[tokio::test]
    async fn with_lock_runs_the_callback_once() {
        let driver = MemoryCacheDriver::new();

        let taken = driver
            .with_lock(
                "lock:k",
                60,
                true,
                Box::pin(async {
                    // the lock is visible to a concurrent caller while the body runs
                    Ok(())
                }),
            )
            .await
            .unwrap();
        assert!(taken);
        // freeAfter released it
        assert!(driver.is_empty());

        // holding the lock rejects the second caller
        driver.set("lock:k", json!(1), 60).await.unwrap();
        assert!(!driver
            .with_lock("lock:k", 60, true, Box::pin(async { Ok(()) }))
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn with_lock_keeps_the_lock_when_free_after_is_false() {
        let driver = MemoryCacheDriver::new();

        assert!(driver
            .with_lock("lock:k", 60, false, Box::pin(async { Ok(()) }))
            .await
            .unwrap());
        assert!(!driver
            .with_lock("lock:k", 60, false, Box::pin(async { Ok(()) }))
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn with_lock_frees_the_lock_when_the_callback_fails() {
        let driver = MemoryCacheDriver::new();

        let error = driver
            .with_lock(
                "lock:k",
                60,
                true,
                Box::pin(async { Err(CacheError::driver("boom")) }),
            )
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), "Cache driver error: boom");
        assert!(driver.is_empty());
    }

    #[tokio::test]
    async fn an_expired_lock_is_reclaimed_by_the_next_caller() {
        let driver = MemoryCacheDriver::new();

        assert!(driver
            .with_lock("lock:k", 0, false, Box::pin(async { Ok(()) }))
            .await
            .unwrap());

        tokio::time::sleep(Duration::from_millis(20)).await;

        // `LocalCacheDriver` drops the expired lock but still reports it as taken…
        assert!(!driver
            .with_lock("lock:k", 60, false, Box::pin(async { Ok(()) }))
            .await
            .unwrap());
        // …so only the call after that acquires it
        assert!(driver
            .with_lock("lock:k", 60, false, Box::pin(async { Ok(()) }))
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn cache_entries_round_trip_through_the_driver() {
        let driver = MemoryCacheDriver::new();
        let entry = CacheEntry::new(42, json!([1, 2])).with_renewal_key(Some("rk".into()));

        set_entry(&driver, "k", &entry, 60).await.unwrap();

        assert_eq!(get_entry(&driver, "k").await.unwrap(), Some(entry));
        assert_eq!(get_entry(&driver, "other").await.unwrap(), None);
    }

    #[test]
    fn result_cache_evicts_the_least_recently_used_entry() {
        let cache = MemoryResultCache::new(Some(2));
        assert_eq!(cache.max_entries(), 2);

        cache.set("a", CacheEntry::new(1, json!("a")));
        cache.set("b", CacheEntry::new(2, json!("b")));
        // touching "a" makes "b" the least recently used one
        assert!(cache.get("a").is_some());
        cache.set("c", CacheEntry::new(3, json!("c")));

        assert_eq!(cache.len(), 2);
        assert!(cache.get("a").is_some());
        assert!(cache.get("b").is_none());
        assert!(cache.get("c").is_some());
    }

    #[test]
    fn result_cache_defaults_to_ten_thousand_entries() {
        assert_eq!(MemoryResultCache::new(None).max_entries(), 10000);
        assert_eq!(MemoryResultCache::new(Some(0)).max_entries(), 10000);
        assert_eq!(MemoryResultCache::default().max_entries(), 10000);
    }

    #[test]
    fn result_cache_evicts_unusable_entries_on_read() {
        let cache = MemoryResultCache::new(Some(4));
        let now = 1_000_000_000;

        cache.set(
            "k",
            CacheEntry::new(now, json!("fresh")).with_renewal_key(Some("rk".into())),
        );
        assert!(cache
            .get_usable("k", 3600, Some(3600), Some("rk"), now)
            .is_some());

        // a renewal key which no longer matches makes the entry unusable and evicts it
        assert!(cache
            .get_usable("k", 3600, Some(3600), Some("other"), now)
            .is_none());
        assert!(cache.get("k").is_none());

        // so does ageing past the disable period
        cache.set("k", CacheEntry::new(now, json!("fresh")));
        assert!(cache
            .get_usable(
                "k",
                3600,
                None,
                None,
                now + IN_MEMORY_CACHE_DISABLE_PERIOD_MS + 1
            )
            .is_none());
        assert!(cache.get("k").is_none());
    }

    #[test]
    fn result_cache_only_stores_entries_worth_keeping() {
        let cache = MemoryResultCache::new(Some(4));
        let entry = CacheEntry::new(1, json!("v"));

        assert!(!cache.store_if_fresh("k", &entry, 0, false, Some(3600)));
        assert!(!cache.store_if_fresh("k", &entry, 0, true, None));
        assert!(cache.is_empty());

        assert!(cache.store_if_fresh("k", &entry, 0, true, Some(3600)));
        assert_eq!(cache.len(), 1);
    }
}
