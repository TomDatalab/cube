//! Query cache primitives of the Rust backend.
//!
//! Port of the caching half of `packages/cubejs-query-orchestrator`:
//!
//! * [`cache_key`] — `getCacheHash`, `queryCacheKey` and `refreshKeyIdentity`, byte
//!   compatible with the JavaScript implementation so that a Rust and a Node node share
//!   one Cube Store cache,
//! * [`entry`] — the stored [`CacheEntry`] and the pure decision functions
//!   [`decide_cache_action`] and [`is_memory_entry_usable`],
//! * [`driver`] — the [`CacheDriver`] trait,
//! * [`memory`] — the in-process driver and the bounded result cache.
//!
//! The Cube Store backed driver, the refresh key pipeline and `QueryCache` itself are
//! separate steps of the migration (see `rust/cube/docs/orchestrator-spec.md` §7).

pub mod cache_key;
pub mod driver;
pub mod entry;
pub mod error;
pub mod memory;

pub use cache_key::{
    cache_key_string, get_cache_hash, process_uid, query_cache_key, query_result_cache_key,
    refresh_key_identity, CacheKey, KeyValue, QueryCacheKeyInput, SQL_QUERY_RESULT,
};
pub use driver::{get_entry, set_entry, CacheDriver, LockCallback, SetResult};
pub use entry::{
    decide_cache_action, extract_request_uuid, is_memory_entry_usable, should_store_in_memory,
    CacheAction, CacheActionOptions, CacheEntry, IN_MEMORY_CACHE_DISABLE_PERIOD_MS,
};
pub use error::CacheError;
pub use memory::{MemoryCacheDriver, MemoryResultCache};
