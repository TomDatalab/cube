//! Query orchestrator of the Rust backend.
//!
//! Port of `packages/cubejs-query-orchestrator` on top of the three crates that were split
//! out of it first — [`cubecache`] (cache keys, cache drivers, cache decisions),
//! [`cubequeue`] (the queue engine) and [`cubedriver`] (the `Driver` trait). See
//! `rust/cube/docs/orchestrator-spec.md` for the specification this implements.
//!
//! * [`cache`] — [`QueryCache`]: `cachedQueryResult`'s four branches, `renewQuery`,
//!   `cacheQueryResult`, the refresh key pipeline, pre-aggregation table substitution and
//!   the persistent (streaming) queries that answer with a [`cubequeue::QueryStream`],
//! * [`refresh_key`] — local `every` evaluation (`CUBEJS_REFRESH_KEY_LOCAL_TIME`),
//! * [`preaggs`] — version and content hashing, table naming, version entry parsing, the
//!   load cache, the loader and the partition range loader,
//! * [`time`] — the `timeSeries`/timezone half of `@cubejs-backend/shared` the partition
//!   range loader is built on,
//! * [`orchestrator`] — [`QueryOrchestrator::fetch_query`] and `queryStage`,
//! * [`api`] — the "Continue wait" contract and the `/v1/load` response shape.
//!
//! ```no_run
//! use std::sync::Arc;
//! use cubeorch::{QueryOrchestrator, QueryOrchestratorOptions, QueryBody, DriverFactory};
//!
//! # async fn example(driver_factory: DriverFactory) -> Result<(), cubeorch::OrchError> {
//! let orchestrator = QueryOrchestrator::new(
//!     "dev",
//!     driver_factory,
//!     None,
//!     Arc::new(|_, _| {}),
//!     QueryOrchestratorOptions::default(),
//! );
//!
//! let result = orchestrator
//!     .fetch_query(&QueryBody {
//!         query: Some("SELECT 1".to_string()),
//!         ..Default::default()
//!     })
//!     .await?;
//! # let _ = result;
//! # Ok(())
//! # }
//! ```

pub mod api;
pub mod cache;
pub mod error;
pub mod orchestrator;
pub mod preaggs;
pub mod refresh_key;
pub mod time;
pub mod types;

pub use api::{
    to_iso_string, DbTypeFn, LoadOutcome, LoadResult, LoadService, OrchestratorApi, QueryCompilerFn,
};
pub use cache::{
    CacheQueryResultOptions, CachedQueryOutcome, CachedQueryResult, DriverFactory,
    LoadRefreshKeyOptions, LoggerFn, QueryCache, QueryCacheBuilder, QueryCacheOptions,
    RefreshKeyCacheOptions, RenewQueryOptions,
};
pub use cubequeue::{QueryStream, QueryStreamBatch, StreamRow};
pub use error::OrchError;
pub use orchestrator::{
    FetchQueryOutcome, FetchQueryResult, QueryOrchestrator, QueryOrchestratorOptions,
    ROLLUP_ONLY_MESSAGE,
};
pub use preaggs::{
    content_version, get_structure_version, intersect_date_ranges, partition_table_name,
    tables_to_version_entries, target_table_name, LoadAllPreAggregationsResult, LoadOptions,
    PreAggregationLoadCache, PreAggregationLoader, PreAggregationPartitionRangeLoader,
    PreAggregations, PreAggregationsOptions, TableCacheEntry, TableTimestamp, VersionEntries,
    VersionEntry,
};
pub use refresh_key::{
    evaluate_local_refresh_key, is_valid_local_refresh_key, LocalRefreshKeyDescriptor,
};
pub use time::{
    add_seconds_to_local_timestamp, local_timestamp_to_utc, now_in_time_zone,
    parse_utc_into_local_date, time_series, time_series_boundaries, utc_to_local_time_zone,
    Granularity, QueryDateRange, BUILD_RANGE_END_LOCAL, BUILD_RANGE_START_LOCAL,
    FROM_PARTITION_RANGE, TO_PARTITION_RANGE,
};
pub use types::{
    CacheKeyQueries, CacheMode, LoadPreAggregationResult, PreAggTableToTempTable,
    PreAggregationDescription, QueryBody, QueryWithParams, RefreshKeyQueryOptions,
    UsedPreAggregation,
};
