//! Query queue of the Rust backend.
//!
//! Port of the queueing half of `packages/cubejs-query-orchestrator`:
//!
//! * [`types`] — `QueuePriority`, the query definition and the queue value types,
//! * [`driver`] — the [`QueueDriver`]/[`QueueDriverConnection`] traits,
//! * [`local`] — [`LocalQueueDriver`], the single process implementation,
//! * [`queue`] — [`QueryQueue`], the engine: `execute_in_queue`, reconciliation,
//!   dispatch, heartbeats, execution timeouts, cancellation and query stages,
//! * [`stream`] — [`QueryStream`], the persistent (streaming) query end of the queue.
//!
//! The Cube Store backed driver and the pre-aggregation queues are separate steps of the
//! migration (see `rust/cube/docs/orchestrator-spec.md` §7).
//!
//! ```no_run
//! use std::sync::Arc;
//! use cubecache::CacheKey;
//! use cubequeue::{QueryQueue, QueryQueueConfig, QueuePriority, ExecuteInQueueOptions};
//! use serde_json::{json, Value};
//!
//! # async fn example() -> Result<(), cubequeue::QueueError> {
//! let queue = QueryQueue::builder("SQL_QUERY_dev_default")
//!     .config(QueryQueueConfig { concurrency: 2, ..Default::default() })
//!     .query_handler("query", |query: Value, _cancel| async move { Ok(query) })
//!     .build();
//!
//! let data = queue
//!     .execute_in_queue(
//!         "query",
//!         CacheKey::sql("SELECT 1", &[]),
//!         json!({ "query": "SELECT 1" }),
//!         QueuePriority::Interactive.into(),
//!         ExecuteInQueueOptions::default(),
//!     )
//!     .await?;
//! # let _ = data;
//! # Ok(())
//! # }
//! ```

pub mod driver;
pub mod error;
pub mod local;
pub mod queue;
pub mod stream;
pub mod types;

pub use driver::{redis_hash, QueueDriver, QueueDriverConnection, QueueDriverOptions};
pub use error::{QueueError, TimeoutError};
pub use local::{LocalQueueDriver, LocalQueueDriverConnection};
pub use queue::{
    cancel_handler, query_handler, stream_handler, CancelHandlerFn, CancelHandlerSetter,
    ExecuteInQueueOptions, LoggerFn, QueryHandlerFn, QueryQueue, QueryQueueBuilder,
    QueryQueueConfig, QueueDriverFactory, SendCancelMessageFn, SendProcessMessageFn,
    StreamHandlerFn,
};
pub use stream::{
    query_stream_high_water_mark, QueryStream, QueryStreamBatch, QueryStreamRegistry,
    QueryStreamWriter, StreamClosed, StreamRow, DEFAULT_QUERY_STREAM_HIGH_WATER_MARK,
    DEFAULT_QUERY_STREAM_IDLE_TIMEOUT_SECS,
};
pub use types::{
    AddToQueueOptions, AddToQueueResponse, ExecutionResult, QueryDef, QueryDefUpdate,
    QueryKeysTuple, QueryStage, QueryStageState, QueueId, QueuePriority,
    RetrieveForProcessingSuccess, MAX_PRIORITY, MIN_PRIORITY,
};
