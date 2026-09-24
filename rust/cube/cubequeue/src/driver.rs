//! The queue driver interface.
//!
//! Port of `QueueDriverInterface` / `QueueDriverConnectionInterface`
//! (`packages/cubejs-base-driver/src/queue-driver.interface.ts`) and of
//! `BaseQueueDriver` (`QO/BaseQueueDriver.ts`), whose only behaviour is delegating the
//! hash to `getCacheHash`.

use std::sync::Arc;

use async_trait::async_trait;
use cubecache::{get_cache_hash, CacheKey};

use crate::{
    error::QueueError,
    types::{
        AddToQueueOptions, AddToQueueResponse, ExecutionResult, QueryDef, QueryDefUpdate,
        QueryKeysTuple, QueryStageState, QueueId, RetrieveForProcessingSuccess,
    },
};

/// `BaseQueueDriver.redisHash`
pub fn redis_hash(query_key: &CacheKey, process_uid: &str) -> String {
    get_cache_hash(query_key, process_uid)
}

/// Options every queue driver is configured with (`QueueDriverOptions`).
#[derive(Clone, Debug)]
pub struct QueueDriverOptions {
    pub redis_queue_prefix: String,
    pub concurrency: usize,
    /// Seconds a blocking result read waits before giving up.
    pub continue_wait_timeout: u64,
    /// Seconds after which a queued query nobody waits for is cancelled.
    pub orphaned_timeout: u64,
    /// Seconds without a heartbeat after which an active query is considered stalled.
    pub heart_beat_timeout: u64,
    pub process_uid: String,
}

#[async_trait]
pub trait QueueDriverConnection: Send + Sync {
    fn redis_hash(&self, query_key: &CacheKey) -> String;

    /// Waits up to `continueWaitTimeout` for the result of an in-flight query.
    async fn get_result_blocking(
        &self,
        query_key_hash: &str,
        queue_id: QueueId,
    ) -> Result<Option<ExecutionResult>, QueueError>;

    /// Returns an already computed result without queueing anything.
    async fn get_result(
        &self,
        query_key: &CacheKey,
        external_id: Option<&str>,
    ) -> Result<Option<ExecutionResult>, QueueError>;

    /// Adds the query to the queue. A driver may also retrieve the item for processing in
    /// the same operation, which saves the caller a retrieval round-trip.
    async fn add_to_queue(
        &self,
        query_key: &CacheKey,
        query_handler: &str,
        query: serde_json::Value,
        priority: i32,
        options: &AddToQueueOptions,
    ) -> Result<AddToQueueResponse, QueueError>;

    /// Query keys sorted by priority and time.
    async fn get_to_process_queries(&self) -> Result<Vec<QueryKeysTuple>, QueueError>;

    async fn get_active_queries(&self) -> Result<Vec<QueryKeysTuple>, QueueError>;

    async fn get_query_def(
        &self,
        query_key_hash: &str,
        queue_id: Option<QueueId>,
    ) -> Result<Option<QueryDef>, QueueError>;

    /// Queries which were added to the queue but are not needed any more.
    async fn get_orphaned_queries(&self) -> Result<Vec<QueryKeysTuple>, QueueError>;

    /// Active queries whose heartbeat went stale.
    async fn get_stalled_queries(&self) -> Result<Vec<QueryKeysTuple>, QueueError>;

    async fn get_query_stage_state(&self, only_keys: bool) -> Result<QueryStageState, QueueError>;

    async fn update_heart_beat(
        &self,
        query_key_hash: &str,
        queue_id: Option<QueueId>,
    ) -> Result<(), QueueError>;

    /// Atomically moves a queue item to active. Returns `None` when another node is
    /// already processing the query or the concurrency budget is full.
    async fn retrieve_for_processing(
        &self,
        query_key_hash: &str,
        queue_id: QueueId,
    ) -> Result<Option<RetrieveForProcessingSuccess>, QueueError>;

    /// Merges fields into the definition of an active query. Returns `false` when the
    /// item is not active under this `queue_id` any more.
    async fn optimistic_query_update(
        &self,
        query_key_hash: &str,
        to_update: &QueryDefUpdate,
        queue_id: QueueId,
    ) -> Result<bool, QueueError>;

    async fn cancel_query(
        &self,
        query_key: &CacheKey,
        queue_id: Option<QueueId>,
    ) -> Result<Option<QueryDef>, QueueError>;

    async fn get_query_and_remove(
        &self,
        query_key_hash: &str,
        queue_id: Option<QueueId>,
    ) -> Result<Option<QueryDef>, QueueError>;

    /// Publishes the result and removes the query. Returns `false` when the item is no
    /// longer active under this `queue_id`, i.e. the result is orphaned.
    async fn set_result_and_remove_query(
        &self,
        query_key_hash: &str,
        execution_result: &ExecutionResult,
        queue_id: QueueId,
    ) -> Result<bool, QueueError>;

    /// Stalled and orphaned queries together.
    async fn get_queries_to_cancel(&self) -> Result<Vec<QueryKeysTuple>, QueueError>;

    async fn get_active_and_to_process(
        &self,
    ) -> Result<(Vec<QueryKeysTuple>, Vec<QueryKeysTuple>), QueueError>;

    fn release(&self);
}

#[async_trait]
pub trait QueueDriver: Send + Sync {
    fn redis_hash(&self, query_key: &CacheKey) -> String;

    async fn create_connection(&self) -> Result<Arc<dyn QueueDriverConnection>, QueueError>;

    fn release(&self, connection: Arc<dyn QueueDriverConnection>);
}
