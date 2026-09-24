//! Single process queue driver.
//!
//! Port of `LocalQueueDriver` and `LocalQueueDriverConnection`
//! (`QO/LocalQueueDriver.ts`, `QO/LocalQueueDriverConnection.ts`), the
//! `cacheAndQueueDriver: 'memory'` backend.

use std::{
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use async_trait::async_trait;
use cubecache::CacheKey;
use indexmap::IndexMap;
use serde_json::Value;
use tokio::sync::Notify;

use crate::{
    driver::{redis_hash, QueueDriver, QueueDriverConnection, QueueDriverOptions},
    error::QueueError,
    types::{
        AddToQueueOptions, AddToQueueResponse, ExecutionResult, QueryDef, QueryDefUpdate,
        QueryKeysTuple, QueryStageState, QueueId, RetrieveForProcessingSuccess,
    },
};

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct QueueItem {
    /// Sort key. For `to_process` it encodes priority and arrival time, for `recent` and
    /// `heart_beat` it is a deadline in epoch milliseconds.
    order: i64,
    key: String,
    queue_id: QueueId,
}

/// The slot a blocking reader waits on. It outlives its entry in the state map, so a
/// reader which is already waiting still sees the value after the slot was consumed.
#[derive(Debug, Default)]
struct ResultSlot {
    value: Mutex<Option<ExecutionResult>>,
    notify: Notify,
}

impl ResultSlot {
    fn value(&self) -> Option<ExecutionResult> {
        self.value
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }

    fn resolve(&self, result: ExecutionResult) {
        *self.value.lock().unwrap_or_else(|err| err.into_inner()) = Some(result);
        self.notify.notify_waiters();
    }
}

/// `LocalQueueDriverConnectionState`.
///
/// Every map is insertion ordered because the Node implementation iterates plain objects,
/// and ties in the sort order fall back to insertion order there.
#[derive(Debug, Default)]
struct State {
    result_slots: IndexMap<String, Arc<ResultSlot>>,
    query_def: IndexMap<String, QueryDef>,
    to_process: IndexMap<String, QueueItem>,
    recent: IndexMap<String, QueueItem>,
    active: IndexMap<String, QueueItem>,
    heart_beat: IndexMap<String, QueueItem>,
}

impl State {
    fn result_slot(&mut self, result_list_key: &str) -> Arc<ResultSlot> {
        self.result_slots
            .entry(result_list_key.to_string())
            .or_default()
            .clone()
    }

    fn remove_query(&mut self, query_key_hash: &str) -> Option<QueryDef> {
        let query = self.query_def.shift_remove(query_key_hash);

        self.active.shift_remove(query_key_hash);
        self.heart_beat.shift_remove(query_key_hash);
        self.to_process.shift_remove(query_key_hash);
        self.recent.shift_remove(query_key_hash);

        query
    }
}

fn ordered_items(
    queue: &IndexMap<String, QueueItem>,
    order_filter_less_than: Option<i64>,
) -> Vec<QueueItem> {
    let mut items: Vec<QueueItem> = queue
        .values()
        .filter(|item| match order_filter_less_than {
            Some(limit) => item.order < limit,
            None => true,
        })
        .cloned()
        .collect();

    items.sort_by_key(|item| item.order);

    items
}

fn queue_array(queue: &IndexMap<String, QueueItem>) -> Vec<String> {
    ordered_items(queue, None)
        .into_iter()
        .map(|item| item.key)
        .collect()
}

fn queue_array_as_tuple(
    queue: &IndexMap<String, QueueItem>,
    order_filter_less_than: Option<i64>,
) -> Vec<QueryKeysTuple> {
    ordered_items(queue, order_filter_less_than)
        .into_iter()
        .map(|item| (item.key, item.queue_id))
        .collect()
}

/// In-memory queue driver. Every connection it hands out shares one state.
///
/// `LocalQueueDriver` keeps that state in a module level map keyed by queue prefix, so
/// two drivers with the same prefix share it process wide. Here the driver owns it: a
/// process runs one driver per prefix anyway, and the singleton only ever leaked state
/// between tests.
#[derive(Debug)]
pub struct LocalQueueDriver {
    options: QueueDriverOptions,
    state: Arc<Mutex<State>>,
}

impl LocalQueueDriver {
    pub fn new(options: QueueDriverOptions) -> Self {
        Self {
            options,
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    pub fn options(&self) -> &QueueDriverOptions {
        &self.options
    }
}

#[async_trait]
impl QueueDriver for LocalQueueDriver {
    fn redis_hash(&self, query_key: &CacheKey) -> String {
        redis_hash(query_key, &self.options.process_uid)
    }

    async fn create_connection(&self) -> Result<Arc<dyn QueueDriverConnection>, QueueError> {
        Ok(Arc::new(LocalQueueDriverConnection {
            options: self.options.clone(),
            state: self.state.clone(),
        }))
    }

    fn release(&self, connection: Arc<dyn QueueDriverConnection>) {
        connection.release();
    }
}

pub struct LocalQueueDriverConnection {
    options: QueueDriverOptions,
    state: Arc<Mutex<State>>,
}

impl LocalQueueDriverConnection {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// `queryRedisKey(queryKey, suffix)`
    fn query_redis_key(&self, query_key_hash: &str, suffix: &str) -> String {
        format!(
            "{}_{}_{}",
            self.options.redis_queue_prefix, query_key_hash, suffix
        )
    }

    fn result_list_key(&self, query_key_hash: &str) -> String {
        self.query_redis_key(query_key_hash, "RESULT")
    }
}

#[async_trait]
impl QueueDriverConnection for LocalQueueDriverConnection {
    fn redis_hash(&self, query_key: &CacheKey) -> String {
        redis_hash(query_key, &self.options.process_uid)
    }

    async fn get_result_blocking(
        &self,
        query_key_hash: &str,
        _queue_id: QueueId,
    ) -> Result<Option<ExecutionResult>, QueueError> {
        let result_list_key = self.result_list_key(query_key_hash);

        let slot = {
            let mut state = self.state();

            if !state.query_def.contains_key(query_key_hash)
                && !state.result_slots.contains_key(&result_list_key)
            {
                return Ok(None);
            }

            state.result_slot(&result_list_key)
        };

        let notified = slot.notify.notified();
        tokio::pin!(notified);
        // Registers before the value is read, so a result published in between still
        // wakes this wait.
        notified.as_mut().enable();

        let result = match slot.value() {
            Some(value) => Some(value),
            None => {
                tokio::select! {
                    _ = notified => slot.value(),
                    _ = tokio::time::sleep(
                        Duration::from_secs(self.options.continue_wait_timeout)
                    ) => None,
                }
            }
        };

        if result.is_some() {
            self.state().result_slots.shift_remove(&result_list_key);
        }

        Ok(result)
    }

    async fn get_result(
        &self,
        query_key: &CacheKey,
        _external_id: Option<&str>,
    ) -> Result<Option<ExecutionResult>, QueueError> {
        let query_key_hash = self.redis_hash(query_key);
        let result_list_key = self.result_list_key(&query_key_hash);

        let resolved = self
            .state()
            .result_slots
            .get(&result_list_key)
            .map(|slot| slot.value().is_some())
            .unwrap_or(false);

        if resolved {
            return self.get_result_blocking(&query_key_hash, 0).await;
        }

        Ok(None)
    }

    async fn add_to_queue(
        &self,
        query_key: &CacheKey,
        query_handler: &str,
        query: Value,
        priority: i32,
        options: &AddToQueueOptions,
    ) -> Result<AddToQueueResponse, QueueError> {
        let time = now_ms();
        let key = self.redis_hash(query_key);

        let orphaned_timeout = options
            .orphaned_timeout
            .unwrap_or(self.options.orphaned_timeout);

        let def = QueryDef {
            queue_id: options.queue_id,
            query_handler: query_handler.to_string(),
            query,
            query_key: query_key.clone(),
            stage_query_key: options.stage_query_key.clone(),
            priority,
            request_id: options.request_id.clone(),
            added_to_queue_time: time,
            start_query_time: None,
            cancel_handler: None,
        };

        let mut state = self.state();

        state
            .query_def
            .entry(key.clone())
            .or_insert_with(|| def.clone());

        let mut added = 0;

        if !state.to_process.contains_key(&key) && !state.active.contains_key(&key) {
            state.to_process.insert(
                key.clone(),
                QueueItem {
                    // Highest priority first, oldest first within a priority. Node computes
                    // this in a double, which quantizes the time component; exact integer
                    // arithmetic keeps the same ordering without the rounding.
                    order: time + (10_000 - priority as i64) * 100_000_000_000_000,
                    queue_id: options.queue_id,
                    key: key.clone(),
                },
            );

            added = 1;
        }

        state.recent.insert(
            key.clone(),
            QueueItem {
                order: time + orphaned_timeout as i64 * 1000,
                key,
                queue_id: options.queue_id,
            },
        );

        Ok(AddToQueueResponse {
            added,
            queue_id: def.queue_id,
            queue_size: state.to_process.len(),
            added_to_queue_time: def.added_to_queue_time,
            // There is no round-trip to save in memory, the item is left for reconcile to
            // pick up.
            retrieved: None,
        })
    }

    async fn get_to_process_queries(&self) -> Result<Vec<QueryKeysTuple>, QueueError> {
        Ok(queue_array_as_tuple(&self.state().to_process, None))
    }

    async fn get_active_queries(&self) -> Result<Vec<QueryKeysTuple>, QueueError> {
        Ok(queue_array_as_tuple(&self.state().active, None))
    }

    async fn get_query_def(
        &self,
        query_key_hash: &str,
        _queue_id: Option<QueueId>,
    ) -> Result<Option<QueryDef>, QueueError> {
        Ok(self.state().query_def.get(query_key_hash).cloned())
    }

    async fn get_orphaned_queries(&self) -> Result<Vec<QueryKeysTuple>, QueueError> {
        Ok(queue_array_as_tuple(&self.state().recent, Some(now_ms())))
    }

    async fn get_stalled_queries(&self) -> Result<Vec<QueryKeysTuple>, QueueError> {
        let deadline = now_ms() - self.options.heart_beat_timeout as i64 * 1000;

        Ok(queue_array_as_tuple(
            &self.state().heart_beat,
            Some(deadline),
        ))
    }

    async fn get_query_stage_state(&self, only_keys: bool) -> Result<QueryStageState, QueueError> {
        let state = self.state();

        Ok(QueryStageState {
            active: queue_array(&state.active),
            to_process: queue_array(&state.to_process),
            defs: if only_keys {
                Vec::new()
            } else {
                state
                    .query_def
                    .iter()
                    .map(|(key, def)| (key.clone(), def.clone()))
                    .collect()
            },
        })
    }

    async fn update_heart_beat(
        &self,
        query_key_hash: &str,
        queue_id: Option<QueueId>,
    ) -> Result<(), QueueError> {
        let mut state = self.state();

        if let Some(existing) = state.heart_beat.get(query_key_hash) {
            let item = QueueItem {
                key: query_key_hash.to_string(),
                order: now_ms(),
                queue_id: queue_id.unwrap_or(existing.queue_id),
            };

            state.heart_beat.insert(query_key_hash.to_string(), item);
        }

        Ok(())
    }

    async fn retrieve_for_processing(
        &self,
        query_key_hash: &str,
        queue_id: QueueId,
    ) -> Result<Option<RetrieveForProcessingSuccess>, QueueError> {
        let mut state = self.state();

        let query = match state.query_def.get(query_key_hash) {
            Some(query) => query.clone(),
            None => return Ok(None),
        };

        let to_process_queue_id = state
            .to_process
            .get(query_key_hash)
            .map(|item| item.queue_id);

        if query.queue_id != queue_id
            || to_process_queue_id != Some(queue_id)
            || state.active.contains_key(query_key_hash)
            || state.active.len() >= self.options.concurrency
        {
            return Ok(None);
        }

        state.active.insert(
            query_key_hash.to_string(),
            QueueItem {
                key: query_key_hash.to_string(),
                order: queue_id as i64,
                queue_id,
            },
        );
        state.to_process.shift_remove(query_key_hash);
        state.heart_beat.insert(
            query_key_hash.to_string(),
            QueueItem {
                key: query_key_hash.to_string(),
                order: now_ms(),
                queue_id,
            },
        );

        Ok(Some(RetrieveForProcessingSuccess {
            active: queue_array(&state.active),
            queue_size: state.to_process.len(),
            def: query,
        }))
    }

    async fn optimistic_query_update(
        &self,
        query_key_hash: &str,
        to_update: &QueryDefUpdate,
        queue_id: QueueId,
    ) -> Result<bool, QueueError> {
        let mut state = self.state();

        if state.active.get(query_key_hash).map(|item| item.queue_id) != Some(queue_id) {
            return Ok(false);
        }

        match state.query_def.get_mut(query_key_hash) {
            Some(def) => {
                to_update.apply(def);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn cancel_query(
        &self,
        query_key: &CacheKey,
        queue_id: Option<QueueId>,
    ) -> Result<Option<QueryDef>, QueueError> {
        let hash = self.redis_hash(query_key);

        self.get_query_and_remove(&hash, queue_id).await
    }

    async fn get_query_and_remove(
        &self,
        query_key_hash: &str,
        _queue_id: Option<QueueId>,
    ) -> Result<Option<QueryDef>, QueueError> {
        Ok(self.state().remove_query(query_key_hash))
    }

    async fn set_result_and_remove_query(
        &self,
        query_key_hash: &str,
        execution_result: &ExecutionResult,
        queue_id: QueueId,
    ) -> Result<bool, QueueError> {
        let mut state = self.state();

        if state.active.get(query_key_hash).map(|item| item.queue_id) != Some(queue_id) {
            return Ok(false);
        }

        let result_list_key = self.result_list_key(query_key_hash);
        let slot = state.result_slot(&result_list_key);

        state.remove_query(query_key_hash);
        drop(state);

        slot.resolve(execution_result.clone());

        Ok(true)
    }

    async fn get_queries_to_cancel(&self) -> Result<Vec<QueryKeysTuple>, QueueError> {
        let mut queries = self.get_stalled_queries().await?;
        queries.extend(self.get_orphaned_queries().await?);

        Ok(queries)
    }

    async fn get_active_and_to_process(
        &self,
    ) -> Result<(Vec<QueryKeysTuple>, Vec<QueryKeysTuple>), QueueError> {
        let state = self.state();

        Ok((
            queue_array_as_tuple(&state.active, None),
            queue_array_as_tuple(&state.to_process, None),
        ))
    }

    fn release(&self) {}
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::types::QueuePriority;

    fn driver(concurrency: usize) -> LocalQueueDriver {
        LocalQueueDriver::new(QueueDriverOptions {
            redis_queue_prefix: "test".to_string(),
            concurrency,
            continue_wait_timeout: 1,
            orphaned_timeout: 2,
            heart_beat_timeout: 2,
            process_uid: "00000000-0000-4000-8000-000000000000".to_string(),
        })
    }

    fn add_options(queue_id: QueueId) -> AddToQueueOptions {
        AddToQueueOptions {
            queue_id,
            stage_query_key: Some(CacheKey::string(queue_id.to_string())),
            request_id: Some(queue_id.to_string()),
            ..Default::default()
        }
    }

    /// `addToQueue never retrieves in memory` of `test/unit/QueryQueue.abstract.ts`.
    #[tokio::test]
    async fn add_to_queue_never_retrieves() {
        let driver = driver(2);
        let connection = driver.create_connection().await.unwrap();
        let query_key = CacheKey::sql("select * from add_and_retrieve", &[]);

        let response = connection
            .add_to_queue(
                &query_key,
                "delay",
                json!({ "isJob": true }),
                10,
                &add_options(1),
            )
            .await
            .unwrap();

        assert_eq!(response.added, 1);
        assert_eq!(response.retrieved, None);
        assert_eq!(
            connection.get_to_process_queries().await.unwrap(),
            vec![(connection.redis_hash(&query_key), 1)]
        );

        // a second add of the same key does not queue it twice
        let response = connection
            .add_to_queue(
                &query_key,
                "delay",
                json!({ "isJob": true }),
                10,
                &add_options(2),
            )
            .await
            .unwrap();
        assert_eq!(response.added, 0);
        assert_eq!(response.queue_size, 1);
    }

    /// `an active query cannot be retrieved twice`.
    #[tokio::test]
    async fn an_active_query_cannot_be_retrieved_twice() {
        let driver = driver(2);
        let connection = driver.create_connection().await.unwrap();
        let other = driver.create_connection().await.unwrap();
        let query_key = CacheKey::string("active-retrieval");
        let hash = connection.redis_hash(&query_key);

        let response = connection
            .add_to_queue(
                &query_key,
                "handler",
                json!(["select"]),
                10,
                &add_options(1),
            )
            .await
            .unwrap();

        let retrieved = connection
            .retrieve_for_processing(&hash, response.queue_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retrieved.active, vec![hash.clone()]);
        assert_eq!(retrieved.queue_size, 0);
        assert_eq!(retrieved.def.query_key, query_key);

        assert_eq!(
            other
                .retrieve_for_processing(&hash, response.queue_id)
                .await
                .unwrap(),
            None
        );
    }

    /// `a failed retrieval does not reserve a pending query`: the concurrency budget is
    /// enforced by the driver, and a query which lost the race stays queued.
    #[tokio::test]
    async fn a_failed_retrieval_does_not_reserve_a_pending_query() {
        let driver = driver(1);
        let connection = driver.create_connection().await.unwrap();
        let other = driver.create_connection().await.unwrap();

        let first = CacheKey::string("concurrency-first");
        let second = CacheKey::string("concurrency-second");
        let first_hash = connection.redis_hash(&first);
        let second_hash = connection.redis_hash(&second);

        let first_id = connection
            .add_to_queue(&first, "handler", json!(["select"]), 10, &add_options(1))
            .await
            .unwrap()
            .queue_id;
        let second_id = connection
            .add_to_queue(&second, "handler2", json!(["select2"]), 10, &add_options(2))
            .await
            .unwrap()
            .queue_id;

        let retrieved = connection
            .retrieve_for_processing(&first_hash, first_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retrieved.active, vec![first_hash.clone()]);
        assert_eq!(retrieved.queue_size, 1);

        assert_eq!(
            other
                .retrieve_for_processing(&second_hash, second_id)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            connection.get_to_process_queries().await.unwrap(),
            vec![(second_hash.clone(), second_id)]
        );

        connection
            .get_query_and_remove(&first_hash, Some(first_id))
            .await
            .unwrap();

        let retrieved = other
            .retrieve_for_processing(&second_hash, second_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retrieved.active, vec![second_hash]);
        assert_eq!(retrieved.queue_size, 0);
        assert_eq!(retrieved.def.query_key, second);
    }

    /// `stale queueId cannot update or acknowledge a requeued query`.
    #[tokio::test]
    async fn a_stale_queue_id_cannot_update_or_acknowledge() {
        let driver = driver(2);
        let connection = driver.create_connection().await.unwrap();
        let query_key = CacheKey::string("requeued-query");
        let hash = connection.redis_hash(&query_key);

        let stale_id = connection
            .add_to_queue(&query_key, "handler", json!(["old"]), 10, &add_options(1))
            .await
            .unwrap()
            .queue_id;
        assert!(connection
            .retrieve_for_processing(&hash, stale_id)
            .await
            .unwrap()
            .is_some());
        connection
            .get_query_and_remove(&hash, Some(stale_id))
            .await
            .unwrap();

        let current_id = connection
            .add_to_queue(&query_key, "handler", json!(["new"]), 10, &add_options(2))
            .await
            .unwrap()
            .queue_id;

        assert_eq!(
            connection
                .retrieve_for_processing(&hash, stale_id)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            connection.get_to_process_queries().await.unwrap(),
            vec![(hash.clone(), current_id)]
        );
        assert!(connection
            .retrieve_for_processing(&hash, current_id)
            .await
            .unwrap()
            .is_some());

        assert!(!connection
            .optimistic_query_update(&hash, &QueryDefUpdate::start_query_time(1), stale_id)
            .await
            .unwrap());
        assert!(!connection
            .set_result_and_remove_query(&hash, &ExecutionResult::success(json!("stale")), stale_id)
            .await
            .unwrap());

        let def = connection
            .get_query_def(&hash, Some(current_id))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(def.query, json!(["new"]));
        assert_eq!(def.start_query_time, None);
        assert_eq!(
            connection.get_active_queries().await.unwrap(),
            vec![(hash, current_id)]
        );
    }

    /// `orphaned with custom ttl`: the per query timeout wins over the queue default.
    #[tokio::test]
    async fn orphaned_queries_honour_a_custom_timeout() {
        let driver = driver(2);
        let connection = driver.create_connection().await.unwrap();

        assert_eq!(connection.get_orphaned_queries().await.unwrap(), vec![]);

        let short = CacheKey::sql("1", &[]);
        let long = CacheKey::sql("2", &[]);

        connection
            .add_to_queue(
                &short,
                "delay",
                json!({ "isJob": true }),
                10,
                &AddToQueueOptions {
                    orphaned_timeout: Some(0),
                    ..add_options(1)
                },
            )
            .await
            .unwrap();
        connection
            .add_to_queue(
                &long,
                "delay",
                json!({ "isJob": true }),
                10,
                &AddToQueueOptions {
                    orphaned_timeout: Some(60),
                    ..add_options(2)
                },
            )
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;

        assert_eq!(
            connection.get_orphaned_queries().await.unwrap(),
            vec![(connection.redis_hash(&short), 1)]
        );
    }

    #[tokio::test]
    async fn to_process_is_ordered_by_priority_then_arrival() {
        let driver = driver(2);
        let connection = driver.create_connection().await.unwrap();

        let background = CacheKey::string("background");
        let interactive = CacheKey::string("interactive");
        let scheduled = CacheKey::string("scheduled");

        for (index, (key, priority)) in [
            (&background, QueuePriority::Background.value()),
            (&interactive, QueuePriority::Interactive.value()),
            (&scheduled, QueuePriority::Scheduled.value()),
        ]
        .into_iter()
        .enumerate()
        {
            connection
                .add_to_queue(
                    key,
                    "handler",
                    Value::Null,
                    priority,
                    &add_options(index as QueueId + 1),
                )
                .await
                .unwrap();
        }

        let keys: Vec<String> = connection
            .get_to_process_queries()
            .await
            .unwrap()
            .into_iter()
            .map(|(key, _)| key)
            .collect();

        assert_eq!(
            keys,
            vec![
                "interactive".to_string(),
                "background".to_string(),
                "scheduled".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn a_blocking_read_times_out_and_returns_nothing() {
        let driver = driver(2);
        let connection = driver.create_connection().await.unwrap();
        let query_key = CacheKey::string("never-finishes");
        let hash = connection.redis_hash(&query_key);

        // nothing queued at all: the read returns straight away
        assert_eq!(
            connection.get_result_blocking(&hash, 1).await.unwrap(),
            None
        );

        connection
            .add_to_queue(&query_key, "handler", Value::Null, 10, &add_options(1))
            .await
            .unwrap();

        let started = std::time::Instant::now();
        assert_eq!(
            connection.get_result_blocking(&hash, 1).await.unwrap(),
            None
        );
        assert!(started.elapsed() >= Duration::from_secs(1));
    }

    #[tokio::test]
    async fn concurrent_blocking_reads_share_one_result() {
        let driver = driver(2);
        let connection = driver.create_connection().await.unwrap();
        let query_key = CacheKey::string("shared-result");
        let hash = connection.redis_hash(&query_key);

        let queue_id = connection
            .add_to_queue(&query_key, "handler", Value::Null, 10, &add_options(1))
            .await
            .unwrap()
            .queue_id;
        connection
            .retrieve_for_processing(&hash, queue_id)
            .await
            .unwrap()
            .unwrap();

        let first = {
            let connection = driver.create_connection().await.unwrap();
            let hash = hash.clone();
            tokio::spawn(async move { connection.get_result_blocking(&hash, queue_id).await })
        };
        let second = {
            let connection = driver.create_connection().await.unwrap();
            let hash = hash.clone();
            tokio::spawn(async move { connection.get_result_blocking(&hash, queue_id).await })
        };

        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(connection
            .set_result_and_remove_query(&hash, &ExecutionResult::success(json!("ok")), queue_id)
            .await
            .unwrap());

        assert_eq!(
            first.await.unwrap().unwrap(),
            Some(ExecutionResult::success(json!("ok")))
        );
        assert_eq!(
            second.await.unwrap().unwrap(),
            Some(ExecutionResult::success(json!("ok")))
        );
    }

    #[tokio::test]
    async fn an_unread_result_is_served_without_queueing() {
        let driver = driver(2);
        let connection = driver.create_connection().await.unwrap();
        let query_key = CacheKey::string("kept-result");
        let hash = connection.redis_hash(&query_key);

        assert_eq!(connection.get_result(&query_key, None).await.unwrap(), None);

        let queue_id = connection
            .add_to_queue(&query_key, "handler", Value::Null, 10, &add_options(1))
            .await
            .unwrap()
            .queue_id;
        connection
            .retrieve_for_processing(&hash, queue_id)
            .await
            .unwrap();
        connection
            .set_result_and_remove_query(&hash, &ExecutionResult::success(json!(1)), queue_id)
            .await
            .unwrap();

        assert_eq!(
            connection.get_result(&query_key, None).await.unwrap(),
            Some(ExecutionResult::success(json!(1)))
        );
        // the local driver consumes the result on the first read
        assert_eq!(connection.get_result(&query_key, None).await.unwrap(), None);
    }

    #[tokio::test]
    async fn heart_beat_updates_keep_a_query_out_of_the_stalled_set() {
        let driver = driver(2);
        let connection = driver.create_connection().await.unwrap();
        let query_key = CacheKey::string("long-running");
        let hash = connection.redis_hash(&query_key);

        let queue_id = connection
            .add_to_queue(&query_key, "handler", Value::Null, 10, &add_options(1))
            .await
            .unwrap()
            .queue_id;
        connection
            .retrieve_for_processing(&hash, queue_id)
            .await
            .unwrap();

        // heartBeatTimeout is 2s here; without updates the query would go stalled
        for _ in 0..4 {
            tokio::time::sleep(Duration::from_millis(600)).await;
            connection
                .update_heart_beat(&hash, Some(queue_id))
                .await
                .unwrap();
            assert_eq!(connection.get_stalled_queries().await.unwrap(), vec![]);
        }
    }

    #[tokio::test]
    async fn an_unacknowledged_query_goes_stalled() {
        let driver = LocalQueueDriver::new(QueueDriverOptions {
            heart_beat_timeout: 0,
            ..driver(2).options().clone()
        });
        let connection = driver.create_connection().await.unwrap();
        let query_key = CacheKey::string("stalled");
        let hash = connection.redis_hash(&query_key);

        let queue_id = connection
            .add_to_queue(&query_key, "handler", Value::Null, 10, &add_options(1))
            .await
            .unwrap()
            .queue_id;
        connection
            .retrieve_for_processing(&hash, queue_id)
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(20)).await;

        assert_eq!(
            connection.get_stalled_queries().await.unwrap(),
            vec![(hash, queue_id)]
        );
    }
}
