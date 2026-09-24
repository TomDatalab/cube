//! The cache driver interface.
//!
//! Port of `CacheDriverInterface` (`packages/cubejs-base-driver/src/driver.interface.ts`)
//! as implemented by `LocalCacheDriver` and `CubeStoreCacheDriver`.

use async_trait::async_trait;
use futures::future::BoxFuture;
use serde_json::Value;

use crate::{entry::CacheEntry, error::CacheError};

/// What `CacheDriverInterface.set` reports back, used for the "Outgoing network usage"
/// log line of `QueryCache`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetResult {
    pub key: String,
    pub bytes: usize,
}

/// The body run while a lock is held. It is lazy, so building it does not run it.
pub type LockCallback<'a> = BoxFuture<'a, Result<(), CacheError>>;

#[async_trait]
pub trait CacheDriver: Send + Sync {
    /// Returns the stored value, or `None` when the key is missing or expired.
    async fn get(&self, key: &str) -> Result<Option<Value>, CacheError>;

    /// Stores `value` under `key` for `ttl_secs` seconds.
    async fn set(&self, key: &str, value: Value, ttl_secs: u64) -> Result<SetResult, CacheError>;

    async fn remove(&self, key: &str) -> Result<(), CacheError>;

    /// Every live key with the given prefix.
    async fn keys_starting_with(&self, prefix: &str) -> Result<Vec<String>, CacheError>;

    /// Runs `callback` while holding the lock named `key`.
    ///
    /// Returns `false` without running anything when the lock is already taken. When
    /// `free_after` is set the lock is released once the callback settles, otherwise it is
    /// left to expire after `ttl_secs`.
    async fn with_lock<'a>(
        &'a self,
        key: &'a str,
        ttl_secs: u64,
        free_after: bool,
        callback: LockCallback<'a>,
    ) -> Result<bool, CacheError>;

    async fn test_connection(&self) -> Result<(), CacheError>;
}

/// Reads a key as a [`CacheEntry`]. A value which is not an entry is reported as
/// [`CacheError::Serde`] rather than silently treated as a miss.
pub async fn get_entry(
    driver: &dyn CacheDriver,
    key: &str,
) -> Result<Option<CacheEntry>, CacheError> {
    match driver.get(key).await? {
        Some(value) => Ok(Some(serde_json::from_value(value)?)),
        None => Ok(None),
    }
}

/// Writes a [`CacheEntry`] under `key`.
pub async fn set_entry(
    driver: &dyn CacheDriver,
    key: &str,
    entry: &CacheEntry,
    ttl_secs: u64,
) -> Result<SetResult, CacheError> {
    driver
        .set(key, serde_json::to_value(entry)?, ttl_secs)
        .await
}
