//! The query queue engine.
//!
//! Port of `QueryQueue` (`QO/QueryQueue.ts`): `executeInQueue`, reconciliation,
//! dispatch, the heartbeat, execution timeouts, cancellation and query stages, plus the
//! persistent (streaming) queries of [`QueryQueue::execute_stream_in_queue`].

use std::{
    collections::HashMap,
    future::Future,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use cubecache::{extract_request_uuid, CacheKey};
use futures::future::BoxFuture;
use serde_json::{json, Value};

use crate::{
    driver::{QueueDriver, QueueDriverConnection, QueueDriverOptions},
    error::{QueueError, TimeoutError},
    local::LocalQueueDriver,
    stream::{
        QueryStream, QueryStreamRegistry, QueryStreamWriter, StreamWait,
        DEFAULT_QUERY_STREAM_HIGH_WATER_MARK, DEFAULT_QUERY_STREAM_IDLE_TIMEOUT_SECS,
    },
    types::{
        query_flag, query_u64, AddToQueueOptions, ExecutionResult, QueryDef, QueryDefUpdate,
        QueryStage, QueryStageState, QueueId, RetrieveForProcessingSuccess, MAX_PRIORITY,
        MIN_PRIORITY,
    },
};

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// `query.query?.aliasNameToMember` — the alias the SQL carries mapped to the member a
/// client asked for (`QO/QueryQueue.ts:942`).
fn alias_name_to_member(query: &Value) -> Option<HashMap<String, String>> {
    let object = query.get("aliasNameToMember")?.as_object()?;

    Some(
        object
            .iter()
            .filter_map(|(alias, member)| {
                member
                    .as_str()
                    .map(|member| (alias.clone(), member.to_string()))
            })
            .collect(),
    )
}

/// `logger(message, event)`
pub type LoggerFn = Arc<dyn Fn(&str, Value) + Send + Sync>;

/// Runs one query. The second argument publishes a cancellation handle which the matching
/// cancel handler receives on the query definition.
pub type QueryHandlerFn = Arc<
    dyn Fn(Value, CancelHandlerSetter) -> BoxFuture<'static, Result<Value, String>> + Send + Sync,
>;

/// Runs one persistent query, writing its rows into the stream the queue created for it
/// (`StreamHandlerFn`, `QO/QueryQueue.ts:24`).
pub type StreamHandlerFn =
    Arc<dyn Fn(Value, QueryStreamWriter) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;

/// Cancels a running query.
pub type CancelHandlerFn =
    Arc<dyn Fn(QueryDef) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;

/// Hands a retrieved query over for execution. The default spawns [`QueryQueue::execute_query`];
/// a multi-process deployment replaces it with a message to a worker.
pub type SendProcessMessageFn = Arc<
    dyn Fn(
            Arc<QueryQueue>,
            String,
            QueueId,
            RetrieveForProcessingSuccess,
        ) -> BoxFuture<'static, Result<(), String>>
        + Send
        + Sync,
>;

/// Hands a query over for cancellation. The default runs [`QueryQueue::process_cancel`].
pub type SendCancelMessageFn =
    Arc<dyn Fn(Arc<QueryQueue>, QueryDef, Option<QueueId>) -> BoxFuture<'static, ()> + Send + Sync>;

/// Builds the queue driver from the resolved driver options.
pub type QueueDriverFactory = Arc<dyn Fn(QueueDriverOptions) -> Arc<dyn QueueDriver> + Send + Sync>;

/// Wraps a plain async function as a [`QueryHandlerFn`].
pub fn query_handler<F, Fut>(handler: F) -> QueryHandlerFn
where
    F: Fn(Value, CancelHandlerSetter) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Value, String>> + Send + 'static,
{
    Arc::new(move |query, cancel| Box::pin(handler(query, cancel)))
}

/// Wraps a plain async function as a [`StreamHandlerFn`].
pub fn stream_handler<F, Fut>(handler: F) -> StreamHandlerFn
where
    F: Fn(Value, QueryStreamWriter) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    Arc::new(move |query, writer| Box::pin(handler(query, writer)))
}

/// Wraps a plain async function as a [`CancelHandlerFn`].
pub fn cancel_handler<F, Fut>(handler: F) -> CancelHandlerFn
where
    F: Fn(QueryDef) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    Arc::new(move |def| Box::pin(handler(def)))
}

/// Timings and switches of a queue (`QO/QueryQueue.ts:124-155`).
#[derive(Clone)]
pub struct QueryQueueConfig {
    /// Queries executed at the same time, per queue rather than per node.
    pub concurrency: usize,
    /// Seconds a client blocks on a result before it is told to continue waiting.
    pub continue_wait_timeout: u64,
    /// Seconds a query handler may run, `CUBEJS_DB_QUERY_TIMEOUT`.
    pub execution_timeout: u64,
    /// Seconds after which a queued query nobody waits for is cancelled.
    pub orphaned_timeout: u64,
    /// Seconds between heartbeats of a running query. The queue driver treats
    /// `heart_beat_interval * 4` without a heartbeat as stalled.
    pub heart_beat_interval: u64,
    /// Runs queries inline instead of queueing them.
    pub skip_queue: bool,
    /// Rows a persistent query may buffer ahead of its consumer,
    /// `CUBEJS_DB_QUERY_STREAM_HIGH_WATER_MARK`.
    pub query_stream_high_water_mark: usize,
    /// Seconds a stream may go undrained before it is destroyed
    /// (`QueryStream`'s 5 minute debounce).
    pub query_stream_idle_timeout: u64,
    pub process_uid: Option<String>,
    pub logger: Option<LoggerFn>,
    pub send_process_message_fn: Option<SendProcessMessageFn>,
    pub send_cancel_message_fn: Option<SendCancelMessageFn>,
}

impl Default for QueryQueueConfig {
    fn default() -> Self {
        Self {
            concurrency: 2,
            continue_wait_timeout: 10,
            execution_timeout: 10 * 60,
            orphaned_timeout: 120,
            heart_beat_interval: 30,
            skip_queue: false,
            query_stream_high_water_mark: DEFAULT_QUERY_STREAM_HIGH_WATER_MARK,
            query_stream_idle_timeout: DEFAULT_QUERY_STREAM_IDLE_TIMEOUT_SECS,
            process_uid: None,
            logger: None,
            send_process_message_fn: None,
            send_cancel_message_fn: None,
        }
    }
}

impl std::fmt::Debug for QueryQueueConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryQueueConfig")
            .field("concurrency", &self.concurrency)
            .field("continue_wait_timeout", &self.continue_wait_timeout)
            .field("execution_timeout", &self.execution_timeout)
            .field("orphaned_timeout", &self.orphaned_timeout)
            .field("heart_beat_interval", &self.heart_beat_interval)
            .field("skip_queue", &self.skip_queue)
            .field(
                "query_stream_high_water_mark",
                &self.query_stream_high_water_mark,
            )
            .field("query_stream_idle_timeout", &self.query_stream_idle_timeout)
            .field("process_uid", &self.process_uid)
            .finish_non_exhaustive()
    }
}

/// Per call options of [`QueryQueue::execute_in_queue`] (`ExecuteInQueueOptions`).
#[derive(Clone, Debug, Default)]
pub struct ExecuteInQueueOptions {
    /// Identifies the work a client asks about through [`QueryQueue::query_stage`].
    pub stage_query_key: Option<CacheKey>,
    pub request_id: Option<String>,
    pub span_id: Option<String>,
}

/// Publishes the cancellation handle of a running query.
#[derive(Clone)]
pub struct CancelHandlerSetter {
    connection: Option<Arc<dyn QueueDriverConnection>>,
    query_key_hash: String,
    queue_id: QueueId,
    local: Arc<Mutex<Option<Value>>>,
}

impl CancelHandlerSetter {
    /// A setter which only records the handle, used on the skip-queue path where there is
    /// no queue item to merge it into.
    pub fn detached() -> Self {
        Self {
            connection: None,
            query_key_hash: String::new(),
            queue_id: 0,
            local: Arc::new(Mutex::new(None)),
        }
    }

    /// `setCancelHandler(handle)` — stores the handle on the queue item so that a cancel
    /// triggered from anywhere can reach the running query.
    pub async fn set(&self, handle: Value) {
        *self.local.lock().unwrap_or_else(|err| err.into_inner()) = Some(handle.clone());

        if let Some(connection) = &self.connection {
            let _ = connection
                .optimistic_query_update(
                    &self.query_key_hash,
                    &QueryDefUpdate::cancel_handler(handle),
                    self.queue_id,
                )
                .await;
        }
    }

    pub fn get(&self) -> Option<Value> {
        self.local
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }
}

/// Assembles a [`QueryQueue`].
pub struct QueryQueueBuilder {
    prefix: String,
    config: QueryQueueConfig,
    query_handlers: HashMap<String, QueryHandlerFn>,
    cancel_handlers: HashMap<String, CancelHandlerFn>,
    stream_handler: Option<StreamHandlerFn>,
    queue_driver_factory: Option<QueueDriverFactory>,
}

impl QueryQueueBuilder {
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            config: QueryQueueConfig::default(),
            query_handlers: HashMap::new(),
            cancel_handlers: HashMap::new(),
            stream_handler: None,
            queue_driver_factory: None,
        }
    }

    #[must_use]
    pub fn config(mut self, config: QueryQueueConfig) -> Self {
        self.config = config;
        self
    }

    #[must_use]
    pub fn query_handler<F, Fut>(mut self, name: impl Into<String>, handler: F) -> Self
    where
        F: Fn(Value, CancelHandlerSetter) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value, String>> + Send + 'static,
    {
        self.query_handlers
            .insert(name.into(), query_handler(handler));
        self
    }

    #[must_use]
    pub fn cancel_handler<F, Fut>(mut self, name: impl Into<String>, handler: F) -> Self
    where
        F: Fn(QueryDef) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        self.cancel_handlers
            .insert(name.into(), cancel_handler(handler));
        self
    }

    /// The handler of persistent queries (`queryHandler === 'stream'`).
    #[must_use]
    pub fn stream_handler<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(Value, QueryStreamWriter) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        self.stream_handler = Some(stream_handler(handler));
        self
    }

    /// Replaces the default in-memory driver.
    #[must_use]
    pub fn queue_driver_factory(mut self, factory: QueueDriverFactory) -> Self {
        self.queue_driver_factory = Some(factory);
        self
    }

    pub fn build(self) -> Arc<QueryQueue> {
        let process_uid = self
            .config
            .process_uid
            .clone()
            .unwrap_or_else(|| cubecache::process_uid().to_string());

        let driver_options = QueueDriverOptions {
            redis_queue_prefix: self.prefix.clone(),
            concurrency: self.config.concurrency,
            continue_wait_timeout: self.config.continue_wait_timeout,
            orphaned_timeout: self.config.orphaned_timeout,
            heart_beat_timeout: self.config.heart_beat_interval * 4,
            process_uid: process_uid.clone(),
        };

        let queue_driver = match &self.queue_driver_factory {
            Some(factory) => factory(driver_options),
            None => Arc::new(LocalQueueDriver::new(driver_options)),
        };

        Arc::new(QueryQueue {
            redis_queue_prefix: self.prefix,
            logger: self
                .config
                .logger
                .clone()
                .unwrap_or_else(|| Arc::new(|_, _| {})),
            send_process_message_fn: self.config.send_process_message_fn.clone(),
            send_cancel_message_fn: self.config.send_cancel_message_fn.clone(),
            config: self.config,
            process_uid,
            queue_driver,
            query_handlers: self.query_handlers,
            cancel_handlers: self.cancel_handlers,
            stream_handler: self.stream_handler,
            streams: QueryStreamRegistry::new(),
            counter: AtomicU64::new(1),
            reconcile_lock: tokio::sync::Mutex::new(()),
            reconcile_again: AtomicBool::new(false),
        })
    }
}

pub struct QueryQueue {
    redis_queue_prefix: String,
    config: QueryQueueConfig,
    process_uid: String,
    queue_driver: Arc<dyn QueueDriver>,
    query_handlers: HashMap<String, QueryHandlerFn>,
    cancel_handlers: HashMap<String, CancelHandlerFn>,
    stream_handler: Option<StreamHandlerFn>,
    /// Persistent query streams of this process (`QueryQueue.streams`).
    streams: Arc<QueryStreamRegistry>,
    logger: LoggerFn,
    send_process_message_fn: Option<SendProcessMessageFn>,
    send_cancel_message_fn: Option<SendCancelMessageFn>,
    counter: AtomicU64,
    reconcile_lock: tokio::sync::Mutex<()>,
    reconcile_again: AtomicBool,
}

impl QueryQueue {
    pub fn builder(prefix: impl Into<String>) -> QueryQueueBuilder {
        QueryQueueBuilder::new(prefix)
    }

    pub fn queue_driver(&self) -> &Arc<dyn QueueDriver> {
        &self.queue_driver
    }

    pub fn concurrency(&self) -> usize {
        self.config.concurrency
    }

    pub fn process_uid(&self) -> &str {
        &self.process_uid
    }

    pub fn prefix(&self) -> &str {
        &self.redis_queue_prefix
    }

    /// Zero is falsy in the Node code and a queue id is checked for truthiness there, so
    /// the counter starts at one here too.
    pub fn generate_queue_id(&self) -> QueueId {
        self.counter.fetch_add(1, Ordering::SeqCst)
    }

    pub fn redis_hash(&self, query_key: &CacheKey) -> String {
        self.queue_driver.redis_hash(query_key)
    }

    /// The hash a persistent query is registered under: the key always carries the
    /// `persistent` flag, which is what appends `@<processUid>`
    /// (`QO/QueryCache.ts:876-878` sets the flag before hashing).
    pub fn persistent_hash(&self, query_key: &CacheKey) -> String {
        self.redis_hash(&query_key.clone().with_persistent(true))
    }

    /// `QueryQueue.streams` — the persistent query streams this process is serving.
    pub fn streams(&self) -> &Arc<QueryStreamRegistry> {
        &self.streams
    }

    /// `QueryQueue.getQueryStream(queryKeyHash)`.
    pub fn get_query_stream(&self, query_key_hash: &str) -> Option<QueryStream> {
        self.streams.get(query_key_hash)
    }

    /// `QueryQueue.createQueryStream(key, aliasNameToMember)`.
    pub fn create_query_stream(
        &self,
        query_key_hash: impl Into<String>,
        alias_name_to_member: Option<HashMap<String, String>>,
    ) -> QueryStreamWriter {
        self.streams.create(
            query_key_hash,
            alias_name_to_member,
            self.config.query_stream_high_water_mark,
            Duration::from_secs(self.config.query_stream_idle_timeout.max(1)),
        )
    }

    /// Destroys the stream of a persistent query key, which stops the query behind it
    /// (the `stream` cancel handler of `QO/QueryCache.ts:876-882`).
    pub fn destroy_query_stream(&self, query_key: &CacheKey) -> bool {
        self.streams.destroy(&self.persistent_hash(query_key))
    }

    /// `QueryQueue.waitForQueryStream(queryKeyHash)` — the stream of a persistent query,
    /// or `None` when none started within `continue_wait_timeout * 10` seconds.
    ///
    /// Returns `None` as well for a stream that already carries data: it left the map when
    /// its first batch went out, exactly like `getQueryStream() ?? null` in the source.
    pub async fn wait_for_query_stream(&self, query_key_hash: &str) -> Option<QueryStream> {
        let wait = self.streams.subscribe(query_key_hash);

        self.await_query_stream(wait, query_key_hash).await
    }

    async fn await_query_stream(
        &self,
        wait: StreamWait,
        query_key_hash: &str,
    ) -> Option<QueryStream> {
        // A stream that is already registered is handed over without waiting; `wait` is
        // dropped here, which deregisters it.
        if let Some(stream) = self.streams.get(query_key_hash) {
            return Some(stream);
        }

        // `this.continueWaitTimeout * 10000` milliseconds (`QO/QueryQueue.ts:400-406`):
        // a stream may take considerably longer to start than a result takes to arrive.
        let timeout = Duration::from_secs(self.config.continue_wait_timeout.saturating_mul(10));

        tokio::time::timeout(timeout, wait.recv())
            .await
            .unwrap_or_default()
    }

    fn log(&self, message: &str, event: Value) {
        (self.logger)(message, event);
    }

    /// `parseResult` (`QO/QueryQueue.ts:418-432`).
    fn parse_result(result: Option<ExecutionResult>) -> Result<Value, QueueError> {
        match result {
            None => Ok(Value::Null),
            Some(ExecutionResult::Error { error }) => Err(QueueError::Execution(error)),
            Some(ExecutionResult::Success { result }) => Ok(result),
            // `parseResult` reads `result.result`, which a stream result does not carry.
            Some(ExecutionResult::Stream { .. }) => Ok(Value::Null),
        }
    }

    /// Pushes a query to the queue and waits for its result.
    ///
    /// Returns [`QueueError::ContinueWait`] when the result is not ready within
    /// `continue_wait_timeout`; the caller is expected to re-issue the identical request.
    pub async fn execute_in_queue(
        self: &Arc<Self>,
        query_handler: &str,
        query_key: CacheKey,
        query: Value,
        priority: i32,
        options: ExecuteInQueueOptions,
    ) -> Result<Value, QueueError> {
        // A persistent query answers with a `QueryStream`, not with a value.
        if query_handler == "stream" {
            return Err(QueueError::StreamingUnsupported);
        }

        let queue_id = self.generate_queue_id();
        let external_id = options
            .request_id
            .as_deref()
            .map(|request_id| extract_request_uuid(request_id).to_string());

        if self.config.skip_queue {
            let def = QueryDef {
                queue_id,
                query_handler: query_handler.to_string(),
                query,
                query_key,
                stage_query_key: options.stage_query_key,
                priority,
                request_id: options.request_id,
                added_to_queue_time: now_ms(),
                start_query_time: None,
                cancel_handler: None,
            };

            self.log(
                "Waiting for query",
                json!({
                    "queueId": queue_id,
                    "spanId": options.span_id,
                    "queueSize": 0,
                    "queryKey": def.query_key,
                    "queuePrefix": self.redis_queue_prefix,
                    "requestId": def.request_id,
                }),
            );

            let result = self.process_query_skip_queue(&def, queue_id).await;

            return Self::parse_result(Some(result));
        }

        let connection = self.queue_driver.create_connection().await?;

        let result = self
            .execute_in_queue_impl(
                connection.clone(),
                query_handler,
                query_key,
                query,
                priority,
                &options,
                queue_id,
                external_id,
            )
            .await;

        self.queue_driver.release(connection);

        result
    }

    /// Pushes a persistent query to the queue and hands back the stream its handler
    /// writes into (`executeInQueue` with `queryHandler === 'stream'`).
    ///
    /// Returns [`QueueError::ContinueWait`] when no stream started in time; the caller is
    /// expected to re-issue the identical request.
    pub async fn execute_stream_in_queue(
        self: &Arc<Self>,
        query_key: CacheKey,
        query: Value,
        priority: i32,
        options: ExecuteInQueueOptions,
    ) -> Result<QueryStream, QueueError> {
        // `Streaming queries to Cube Store aren't supported` (`QO/QueryQueue.ts:233-235`):
        // the inline path has no queue item to attach a stream to.
        if self.config.skip_queue {
            return Err(QueueError::StreamingUnsupported);
        }

        if !(MIN_PRIORITY..=MAX_PRIORITY).contains(&priority) {
            return Err(QueueError::InvalidPriority);
        }

        // A stream is served by the process that started it, so the key carries its uid.
        let query_key = query_key.with_persistent(true);
        let queue_id = self.generate_queue_id();
        let external_id = options
            .request_id
            .as_deref()
            .map(|request_id| extract_request_uuid(request_id).to_string());

        let connection = self.queue_driver.create_connection().await?;

        let result = self
            .execute_stream_in_queue_impl(
                connection.clone(),
                query_key,
                query,
                priority,
                &options,
                queue_id,
                external_id,
            )
            .await;

        self.queue_driver.release(connection);

        result
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_stream_in_queue_impl(
        self: &Arc<Self>,
        connection: Arc<dyn QueueDriverConnection>,
        query_key: CacheKey,
        query: Value,
        priority: i32,
        options: &ExecuteInQueueOptions,
        queue_id: QueueId,
        external_id: Option<String>,
    ) -> Result<QueryStream, QueueError> {
        // `if (result && !result.streamResult)`: a stream result is no result at all, so
        // only a failure recorded under this key is worth reporting instead of re-running.
        if let Some(ExecutionResult::Error { error }) = connection
            .get_result(&query_key, external_id.as_deref())
            .await?
        {
            return Err(QueueError::Execution(error));
        }

        let query_key_hash = self.redis_hash(&query_key);

        // Subscribing after the dispatch would lose the `streamStarted` event of a handler
        // which starts fast.
        let wait = self.streams.subscribe(&query_key_hash);

        let add_options = AddToQueueOptions {
            queue_id,
            stage_query_key: options.stage_query_key.clone(),
            request_id: options.request_id.clone(),
            span_id: options.span_id.clone(),
            orphaned_timeout: query_u64(&query, "orphanedTimeout"),
            external_id,
        };

        let response = connection
            .add_to_queue(&query_key, "stream", query, priority, &add_options)
            .await?;

        if response.added > 0 {
            self.log(
                "Added to queue",
                json!({
                    "queueId": response.queue_id,
                    "spanId": options.span_id,
                    "priority": priority,
                    "queueSize": response.queue_size,
                    "queryKey": query_key,
                    "queuePrefix": self.redis_queue_prefix,
                    "requestId": options.request_id,
                    "addedToQueueTime": response.added_to_queue_time,
                    "persistent": true,
                }),
            );
        }

        match response.retrieved.clone() {
            Some(retrieved) => {
                self.dispatch_query(query_key_hash.clone(), response.queue_id, retrieved)
                    .await
            }
            None => self.reconcile_queue().await?,
        }

        self.log(
            "Waiting for query",
            json!({
                "queueId": response.queue_id,
                "spanId": options.span_id,
                "queryKey": query_key,
                "queuePrefix": self.redis_queue_prefix,
                "requestId": options.request_id,
                "queueSize": response.queue_size,
                "persistent": true,
            }),
        );

        match self.await_query_stream(wait, &query_key_hash).await {
            Some(stream) => Ok(stream),
            None => {
                self.log(
                    "Finished waiting for query",
                    json!({
                        "queueId": response.queue_id,
                        "spanId": options.span_id,
                        "queryKey": query_key,
                        "queuePrefix": self.redis_queue_prefix,
                        "requestId": options.request_id,
                    }),
                );

                Err(QueueError::ContinueWait)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_in_queue_impl(
        self: &Arc<Self>,
        connection: Arc<dyn QueueDriverConnection>,
        query_handler: &str,
        query_key: CacheKey,
        query: Value,
        priority: i32,
        options: &ExecuteInQueueOptions,
        queue_id: QueueId,
        external_id: Option<String>,
    ) -> Result<Value, QueueError> {
        if !(MIN_PRIORITY..=MAX_PRIORITY).contains(&priority) {
            return Err(QueueError::InvalidPriority);
        }

        let force_build = query_flag(&query, "forceBuild");
        let is_job = query_flag(&query, "isJob");

        // The result is not looked up for a forced build or a jobbed build query.
        if !force_build {
            if let Some(result) = connection
                .get_result(&query_key, external_id.as_deref())
                .await?
            {
                return Self::parse_result(Some(result));
            }
        }

        let query_key_hash = self.redis_hash(&query_key);

        if force_build
            && connection
                .get_query_def(&query_key_hash, None)
                .await?
                .is_some()
        {
            return Ok(Value::Null);
        }

        let add_options = AddToQueueOptions {
            queue_id,
            stage_query_key: options.stage_query_key.clone(),
            request_id: options.request_id.clone(),
            span_id: options.span_id.clone(),
            orphaned_timeout: query_u64(&query, "orphanedTimeout"),
            external_id,
        };

        let response = connection
            .add_to_queue(&query_key, query_handler, query, priority, &add_options)
            .await?;

        if response.added > 0 {
            self.log(
                "Added to queue",
                json!({
                    "queueId": response.queue_id,
                    "spanId": options.span_id,
                    "priority": priority,
                    "queueSize": response.queue_size,
                    "queryKey": query_key,
                    "queuePrefix": self.redis_queue_prefix,
                    "requestId": options.request_id,
                    "addedToQueueTime": response.added_to_queue_time,
                    "persistent": query_key.is_persistent(),
                }),
            );
        }

        match response.retrieved.clone() {
            // The item is active already, there is nothing for reconcile to pick up.
            Some(retrieved) => {
                self.dispatch_query(query_key_hash.clone(), response.queue_id, retrieved)
                    .await
            }
            None => self.reconcile_queue().await?,
        }

        let (active, to_process) = match &response.retrieved {
            Some(retrieved) => (retrieved.active.clone(), None),
            None => {
                let state = connection.get_query_stage_state(true).await?;
                (state.active, Some(state.to_process))
            }
        };

        self.log(
            "Waiting for query",
            json!({
                "queueId": response.queue_id,
                "spanId": options.span_id,
                "queryKey": query_key,
                "queuePrefix": self.redis_queue_prefix,
                "requestId": options.request_id,
                "queueSize": response.queue_size,
                "active": active.iter().any(|key| key == &query_key_hash),
                "queueIndex": to_process
                    .as_ref()
                    .and_then(|keys| keys.iter().position(|key| key == &query_key_hash))
                    .map(|index| index as i64)
                    .unwrap_or(-1),
            }),
        );

        // The result is not awaited for a jobbed build query.
        let result = if is_job {
            None
        } else {
            connection
                .get_result_blocking(&query_key_hash, response.queue_id)
                .await?
        };

        if !is_job && result.is_none() {
            self.log(
                "Finished waiting for query",
                json!({
                    "queueId": response.queue_id,
                    "spanId": options.span_id,
                    "queryKey": query_key,
                    "queuePrefix": self.redis_queue_prefix,
                    "requestId": options.request_id,
                }),
            );

            return Err(QueueError::ContinueWait);
        }

        Self::parse_result(result)
    }

    /// Runs reconciliation, coalescing concurrent calls the way the `reconcilePromise`
    /// of the Node implementation does (`QO/QueryQueue.ts:443-466`).
    pub async fn reconcile_queue(self: &Arc<Self>) -> Result<(), QueueError> {
        match self.reconcile_lock.try_lock() {
            Ok(guard) => {
                loop {
                    self.reconcile_again.store(false, Ordering::SeqCst);
                    self.reconcile_queue_impl().await?;

                    if !self.reconcile_again.swap(false, Ordering::SeqCst) {
                        break;
                    }
                }

                drop(guard);

                Ok(())
            }
            Err(_) => {
                self.reconcile_again.store(true, Ordering::SeqCst);

                // Waits for the reconciliation which is already running, then runs one
                // more pass when it finished before this call was registered.
                let _guard = self.reconcile_lock.lock().await;

                if self.reconcile_again.swap(false, Ordering::SeqCst) {
                    self.reconcile_queue_impl().await?;
                }

                Ok(())
            }
        }
    }

    /// Resolves once no reconciliation is running. Returns whether it had to wait.
    pub async fn shutdown(&self) -> bool {
        match self.reconcile_lock.try_lock() {
            Ok(_) => false,
            Err(_) => {
                let _guard = self.reconcile_lock.lock().await;
                true
            }
        }
    }

    /// Cancels stalled and orphaned queries, then picks queries to process
    /// (`QO/QueryQueue.ts:582-640`).
    async fn reconcile_queue_impl(self: &Arc<Self>) -> Result<(), QueueError> {
        let connection = self.queue_driver.create_connection().await?;
        let result = self.reconcile_with_connection(&connection).await;
        self.queue_driver.release(connection);

        result
    }

    async fn reconcile_with_connection(
        self: &Arc<Self>,
        connection: &Arc<dyn QueueDriverConnection>,
    ) -> Result<(), QueueError> {
        for (query_key_hash, queue_id) in connection.get_queries_to_cancel().await? {
            if let Some(def) = connection
                .get_query_and_remove(&query_key_hash, Some(queue_id))
                .await?
            {
                self.log(
                    "Removing orphaned query",
                    json!({
                        "queueId": queue_id,
                        "queryKey": def.query_key,
                        "queuePrefix": self.redis_queue_prefix,
                        "requestId": def.request_id,
                        "addedToQueueTime": def.added_to_queue_time,
                    }),
                );

                self.send_cancel_message(def, Some(queue_id)).await;
            }
        }

        let (active, to_process) = connection.get_active_and_to_process().await?;

        // Concurrency is shared by every node of a cluster, so a node which sees the
        // budget taken tries a single pick rather than competing for all of them.
        let to_process_limit = if active.len() >= self.config.concurrency {
            1
        } else {
            self.config.concurrency - active.len()
        };

        let picked: Vec<_> = to_process
            .into_iter()
            .filter(|(query_key_hash, _)| self.owns_persistent_key(query_key_hash))
            .take(to_process_limit)
            .collect();

        // Awaits the retrieval of every picked query, not their execution.
        futures::future::join_all(
            picked
                .into_iter()
                .map(|(query_key_hash, queue_id)| self.process_query(query_key_hash, queue_id)),
        )
        .await;

        Ok(())
    }

    /// A persistent key carries `@<processUid>`; only its own process may run it.
    fn owns_persistent_key(&self, query_key_hash: &str) -> bool {
        match query_key_hash.split_once('@') {
            None => true,
            Some((_, process_uid)) => process_uid == self.process_uid,
        }
    }

    /// Retrieves the query and hands it over for execution.
    ///
    /// The future is boxed on purpose: executing a query reconciles the queue when it is
    /// done, which dispatches again, and erasing the future here breaks the otherwise
    /// infinite type recursion of the `Send` check.
    pub fn process_query(
        self: &Arc<Self>,
        query_key_hash: String,
        queue_id: QueueId,
    ) -> BoxFuture<'static, ()> {
        let queue = self.clone();

        Box::pin(async move {
            let retrieved = match queue
                .retrieve_query_for_processing(&query_key_hash, queue_id)
                .await
            {
                Some(retrieved) => retrieved,
                None => return,
            };

            queue
                .dispatch_query(query_key_hash, queue_id, retrieved)
                .await;
        })
    }

    async fn retrieve_query_for_processing(
        &self,
        query_key_hash: &str,
        queue_id: QueueId,
    ) -> Option<RetrieveForProcessingSuccess> {
        let connection = match self.queue_driver.create_connection().await {
            Ok(connection) => connection,
            Err(error) => {
                self.log(
                    "Queue storage error",
                    json!({
                        "queueId": queue_id,
                        "queryKey": query_key_hash,
                        "error": error.to_string(),
                        "queuePrefix": self.redis_queue_prefix,
                    }),
                );

                return None;
            }
        };

        let retrieved = connection
            .retrieve_for_processing(query_key_hash, queue_id)
            .await;

        self.queue_driver.release(connection);

        match retrieved {
            Ok(Some(retrieved)) => Some(retrieved),
            Ok(None) => {
                self.log(
                    "Skip processing",
                    json!({
                        "queueId": queue_id,
                        "queryKey": query_key_hash,
                        "queuePrefix": self.redis_queue_prefix,
                    }),
                );

                None
            }
            Err(error) => {
                self.log(
                    "Queue storage error",
                    json!({
                        "queueId": queue_id,
                        "queryKey": query_key_hash,
                        "error": error.to_string(),
                        "queuePrefix": self.redis_queue_prefix,
                    }),
                );

                None
            }
        }
    }

    async fn dispatch_query(
        self: &Arc<Self>,
        query_key_hash: String,
        queue_id: QueueId,
        retrieved: RetrieveForProcessingSuccess,
    ) {
        let query_key = retrieved.def.query_key.clone();
        let request_id = retrieved.def.request_id.clone();

        let dispatched = match &self.send_process_message_fn {
            Some(send) => send(self.clone(), query_key_hash, queue_id, retrieved).await,
            None => {
                let queue = self.clone();
                // Boxed on purpose: `execute_query` reconciles the queue when it is done,
                // which dispatches again. Erasing the future here breaks the otherwise
                // infinite type recursion of the `Send` check.
                let execution: BoxFuture<'static, ()> = Box::pin(async move {
                    queue
                        .execute_query(query_key_hash, queue_id, retrieved)
                        .await;
                });

                tokio::spawn(execution);

                Ok(())
            }
        };

        if let Err(error) = dispatched {
            // The retrieval already made the item active, so a failing dispatch is logged
            // and left to the stalled reclaim rather than failing the waiting request.
            self.log(
                "Error while processing message",
                json!({
                    "queueId": queue_id,
                    "queryKey": query_key,
                    "requestId": request_id,
                    "error": error,
                    "queuePrefix": self.redis_queue_prefix,
                }),
            );
        }
    }

    /// Runs a retrieved query: executes its handler while keeping the heartbeat alive,
    /// then acknowledges the result (`QO/QueryQueue.ts:849-1070`).
    pub async fn execute_query(
        self: Arc<Self>,
        query_key_hash: String,
        queue_id: QueueId,
        retrieved: RetrieveForProcessingSuccess,
    ) {
        let connection = match self.queue_driver.create_connection().await {
            Ok(connection) => connection,
            Err(error) => {
                self.log(
                    "Queue storage error",
                    json!({
                        "queueId": queue_id,
                        "error": error.to_string(),
                        "queuePrefix": self.redis_queue_prefix,
                    }),
                );

                return;
            }
        };

        let RetrieveForProcessingSuccess {
            queue_size,
            def: query,
            ..
        } = retrieved;

        let start_query_time = now_ms();
        let time_in_queue = start_query_time - query.added_to_queue_time;

        self.log(
            "Performing query",
            json!({
                "queueId": queue_id,
                "queueSize": queue_size,
                "queryKey": query.query_key,
                "queuePrefix": self.redis_queue_prefix,
                "requestId": query.request_id,
                "timeInQueue": time_in_queue,
            }),
        );

        let _ = connection
            .optimistic_query_update(
                &query_key_hash,
                &QueryDefUpdate::start_query_time(start_query_time),
                queue_id,
            )
            .await;

        let setter = CancelHandlerSetter {
            connection: Some(connection.clone()),
            query_key_hash: query_key_hash.clone(),
            queue_id,
            local: Arc::new(Mutex::new(None)),
        };

        let heart_beat = self.spawn_heart_beat(
            connection.clone(),
            query_key_hash.clone(),
            queue_id,
            query.clone(),
            setter.clone(),
        );

        let execution_result = self
            .run_handler(
                &connection,
                &query_key_hash,
                queue_id,
                &query,
                queue_size,
                time_in_queue,
                start_query_time,
                setter,
            )
            .await;

        heart_beat.abort();

        match connection
            .set_result_and_remove_query(&query_key_hash, &execution_result, queue_id)
            .await
        {
            Ok(true) => {}
            Ok(false) => self.log(
                "Orphaned execution result",
                json!({
                    "queueId": queue_id,
                    "warn": "Result for query was not set because the queue item is no longer active",
                    "queryKey": query.query_key,
                    "queuePrefix": self.redis_queue_prefix,
                    "requestId": query.request_id,
                }),
            ),
            Err(error) => self.log(
                "Queue storage error",
                json!({
                    "queueId": queue_id,
                    "queryKey": query.query_key,
                    "requestId": query.request_id,
                    "error": error.to_string(),
                    "queuePrefix": self.redis_queue_prefix,
                }),
            ),
        }

        self.queue_driver.release(connection);

        let _ = self.reconcile_queue().await;
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_handler(
        self: &Arc<Self>,
        connection: &Arc<dyn QueueDriverConnection>,
        query_key_hash: &str,
        queue_id: QueueId,
        query: &QueryDef,
        queue_size: usize,
        time_in_queue: i64,
        start_query_time: i64,
        setter: CancelHandlerSetter,
    ) -> ExecutionResult {
        // A persistent query writes into a stream instead of returning rows, and it is not
        // bound by `executionTimeout`: the client reads it for as long as it wants to
        // (`QO/QueryQueue.ts:940-955`, which runs the stream handler outside `queryTimeout`).
        if query.query_handler == "stream" {
            return self
                .run_stream_handler(
                    query_key_hash,
                    queue_id,
                    query,
                    queue_size,
                    time_in_queue,
                    start_query_time,
                )
                .await;
        }

        let handler = match self.query_handlers.get(&query.query_handler) {
            Some(handler) => handler.clone(),
            None => {
                return ExecutionResult::error(format!("No handler for {}", query.query_handler))
            }
        };

        let execution = handler(query.query.clone(), setter);
        let timeout = Duration::from_secs(self.config.execution_timeout);

        match tokio::time::timeout(timeout, execution).await {
            Ok(Ok(result)) => {
                self.log(
                    "Performing query completed",
                    json!({
                        "queueId": queue_id,
                        "queueSize": queue_size,
                        "duration": now_ms() - start_query_time,
                        "queryKey": query.query_key,
                        "queuePrefix": self.redis_queue_prefix,
                        "requestId": query.request_id,
                        "timeInQueue": time_in_queue,
                    }),
                );

                ExecutionResult::success(result)
            }
            Ok(Err(error)) => {
                self.log(
                    "Error while querying",
                    json!({
                        "queueId": queue_id,
                        "queueSize": queue_size,
                        "duration": now_ms() - start_query_time,
                        "queryKey": query.query_key,
                        "queuePrefix": self.redis_queue_prefix,
                        "requestId": query.request_id,
                        "timeInQueue": time_in_queue,
                        "error": error,
                    }),
                );

                ExecutionResult::error(error)
            }
            Err(_) => {
                let error = TimeoutError(self.config.execution_timeout).to_string();

                self.log(
                    "Error while querying",
                    json!({
                        "queueId": queue_id,
                        "queueSize": queue_size,
                        "duration": now_ms() - start_query_time,
                        "queryKey": query.query_key,
                        "queuePrefix": self.redis_queue_prefix,
                        "requestId": query.request_id,
                        "timeInQueue": time_in_queue,
                        "error": error,
                    }),
                );

                // The definition carries the cancellation handle the handler published.
                if let Ok(Some(def)) = connection
                    .get_query_def(query_key_hash, Some(queue_id))
                    .await
                {
                    self.log(
                        "Cancelling query due to timeout",
                        json!({
                            "queueId": queue_id,
                            "queryKey": def.query_key,
                            "queuePrefix": self.redis_queue_prefix,
                            "requestId": def.request_id,
                        }),
                    );

                    self.send_cancel_message(def, Some(queue_id)).await;
                }

                ExecutionResult::error(error)
            }
        }
    }

    /// Runs a persistent query: the queue owns the stream, the handler only writes to it.
    async fn run_stream_handler(
        self: &Arc<Self>,
        query_key_hash: &str,
        queue_id: QueueId,
        query: &QueryDef,
        queue_size: usize,
        time_in_queue: i64,
        start_query_time: i64,
    ) -> ExecutionResult {
        let handler = match &self.stream_handler {
            Some(handler) => handler.clone(),
            None => return ExecutionResult::error("No stream handler for stream"),
        };

        let writer = self.create_query_stream(query_key_hash, alias_name_to_member(&query.query));

        // Dropping the writer unregisters the stream, which is the `finally` of the source.
        let outcome = handler(query.query.clone(), writer).await;

        match outcome {
            Ok(()) => {
                self.log(
                    "Performing query completed",
                    json!({
                        "queueId": queue_id,
                        "queueSize": queue_size,
                        "duration": now_ms() - start_query_time,
                        "queryKey": query.query_key,
                        "queuePrefix": self.redis_queue_prefix,
                        "requestId": query.request_id,
                        "timeInQueue": time_in_queue,
                    }),
                );

                ExecutionResult::stream()
            }
            Err(error) => {
                self.log(
                    "Error while querying",
                    json!({
                        "queueId": queue_id,
                        "queueSize": queue_size,
                        "duration": now_ms() - start_query_time,
                        "queryKey": query.query_key,
                        "queuePrefix": self.redis_queue_prefix,
                        "requestId": query.request_id,
                        "timeInQueue": time_in_queue,
                        "error": error,
                    }),
                );

                ExecutionResult::error(error)
            }
        }
    }

    /// Keeps the queue item alive while its handler runs and notices an external cancel.
    fn spawn_heart_beat(
        self: &Arc<Self>,
        connection: Arc<dyn QueueDriverConnection>,
        query_key_hash: String,
        queue_id: QueueId,
        query: QueryDef,
        setter: CancelHandlerSetter,
    ) -> tokio::task::JoinHandle<()> {
        let queue = self.clone();
        let interval = Duration::from_secs(self.config.heart_beat_interval.max(1));

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            // The first tick resolves immediately.
            ticker.tick().await;

            loop {
                ticker.tick().await;

                if let Err(error) = connection
                    .update_heart_beat(&query_key_hash, Some(queue_id))
                    .await
                {
                    queue.log(
                        "Error updating heartbeat",
                        json!({
                            "queueId": queue_id,
                            "queryKey": query.query_key,
                            "error": error.to_string(),
                            "queuePrefix": queue.redis_queue_prefix,
                            "requestId": query.request_id,
                        }),
                    );
                }

                // A query which was cancelled elsewhere has no definition any more.
                let cancel_handle = setter.get();

                if cancel_handle.is_some() {
                    match connection
                        .get_query_def(&query_key_hash, Some(queue_id))
                        .await
                    {
                        Ok(None) => {
                            queue.log(
                                "Cancelling query due to external cancellation",
                                json!({
                                    "queueId": queue_id,
                                    "queryKey": query.query_key,
                                    "queuePrefix": queue.redis_queue_prefix,
                                    "requestId": query.request_id,
                                }),
                            );

                            let mut cancelled = query.clone();
                            cancelled.cancel_handler = cancel_handle;

                            queue.process_cancel(cancelled, Some(queue_id)).await;

                            return;
                        }
                        Ok(Some(_)) => {}
                        Err(error) => queue.log(
                            "Error checking for external cancellation",
                            json!({
                                "queueId": queue_id,
                                "queryKey": query.query_key,
                                "error": error.to_string(),
                                "queuePrefix": queue.redis_queue_prefix,
                                "requestId": query.request_id,
                            }),
                        ),
                    }
                }
            }
        })
    }

    /// Executes a query without putting it on the queue (`skipQueue`).
    async fn process_query_skip_queue(
        &self,
        query: &QueryDef,
        queue_id: QueueId,
    ) -> ExecutionResult {
        let start_query_time = now_ms();

        self.log(
            "Performing query",
            json!({
                "queueId": queue_id,
                "queueSize": 0,
                "queryKey": query.query_key,
                "queuePrefix": self.redis_queue_prefix,
                "requestId": query.request_id,
                "timeInQueue": 0,
            }),
        );

        let handler = match self.query_handlers.get(&query.query_handler) {
            Some(handler) => handler.clone(),
            None => {
                return ExecutionResult::error(format!("No handler for {}", query.query_handler))
            }
        };

        let setter = CancelHandlerSetter::detached();
        let execution = handler(query.query.clone(), setter.clone());
        let timeout = Duration::from_secs(self.config.execution_timeout);

        match tokio::time::timeout(timeout, execution).await {
            Ok(Ok(result)) => {
                self.log(
                    "Performing query completed",
                    json!({
                        "queueId": queue_id,
                        "queueSize": 0,
                        "duration": now_ms() - start_query_time,
                        "queryKey": query.query_key,
                        "queuePrefix": self.redis_queue_prefix,
                        "requestId": query.request_id,
                        "timeInQueue": 0,
                    }),
                );

                ExecutionResult::success(result)
            }
            Ok(Err(error)) => {
                self.log(
                    "Error while querying",
                    json!({
                        "queueId": queue_id,
                        "queryKey": query.query_key,
                        "queuePrefix": self.redis_queue_prefix,
                        "requestId": query.request_id,
                        "error": error,
                    }),
                );

                ExecutionResult::error(error)
            }
            Err(_) => {
                let error = TimeoutError(self.config.execution_timeout).to_string();

                self.log(
                    "Error while querying",
                    json!({
                        "queueId": queue_id,
                        "queryKey": query.query_key,
                        "queuePrefix": self.redis_queue_prefix,
                        "requestId": query.request_id,
                        "error": error,
                    }),
                );

                if setter.get().is_some() {
                    self.log(
                        "Cancelling query due to timeout",
                        json!({
                            "queueId": queue_id,
                            "queryKey": query.query_key,
                            "queuePrefix": self.redis_queue_prefix,
                            "requestId": query.request_id,
                        }),
                    );

                    let mut cancelled = query.clone();
                    cancelled.cancel_handler = setter.get();

                    self.process_cancel(cancelled, Some(queue_id)).await;
                }

                ExecutionResult::error(error)
            }
        }
    }

    async fn send_cancel_message(self: &Arc<Self>, query: QueryDef, queue_id: Option<QueueId>) {
        match &self.send_cancel_message_fn {
            Some(send) => send(self.clone(), query, queue_id).await,
            None => self.process_cancel(query, queue_id).await,
        }
    }

    /// Runs the cancel handler of a query (`QO/QueryQueue.ts:1075-1093`).
    ///
    /// Cancelling a persistent query destroys its stream, which releases the data source
    /// behind it. The source registers that as a `stream` cancel handler reaching back into
    /// the queue's map (`QO/QueryCache.ts:876-882`); the map belongs to the queue here, so
    /// the queue does it itself and a registered handler only adds to it.
    pub async fn process_cancel(&self, query: QueryDef, queue_id: Option<QueueId>) {
        let is_stream = query.query_handler == "stream";

        if is_stream {
            self.destroy_query_stream(&query.query_key);
        }

        let handler = self.cancel_handlers.get(&query.query_handler).cloned();

        let outcome = match handler {
            Some(handler) => handler(query.clone()).await,
            None if is_stream => Ok(()),
            None => Err(format!("No cancel handler for {}", query.query_handler)),
        };

        if let Err(error) = outcome {
            self.log(
                "Error while cancel",
                json!({
                    "queueId": queue_id,
                    "queryKey": query.query_key,
                    "error": error,
                    "queuePrefix": self.redis_queue_prefix,
                    "requestId": query.request_id,
                }),
            );
        }
    }

    /// Cancels a queued or running query.
    pub async fn cancel_query(
        self: &Arc<Self>,
        query_key: &CacheKey,
        queue_id: Option<QueueId>,
    ) -> Result<bool, QueueError> {
        let connection = self.queue_driver.create_connection().await?;
        let query = connection.cancel_query(query_key, queue_id).await;
        self.queue_driver.release(connection);

        if let Some(query) = query? {
            self.log(
                "Cancelling query manual",
                json!({
                    "queueId": queue_id,
                    "queryKey": query.query_key,
                    "queuePrefix": self.redis_queue_prefix,
                    "requestId": query.request_id,
                    "addedToQueueTime": query.added_to_queue_time,
                }),
            );

            self.send_cancel_message(query, queue_id).await;
        }

        Ok(true)
    }

    /// Cancels every query of a request, matched by its uuid rather than its span.
    pub async fn cancel_query_by_request_id(
        self: &Arc<Self>,
        request_id: &str,
    ) -> Result<Vec<QueryDef>, QueueError> {
        let target = extract_request_uuid(request_id);
        let state = self.fetch_query_stage_state().await?;
        let mut cancelled = Vec::new();

        for (_, def) in state.defs {
            let matches = def
                .request_id
                .as_deref()
                .map(|request_id| extract_request_uuid(request_id) == target)
                .unwrap_or(false);

            if matches {
                self.cancel_query(&def.query_key, None).await?;
                cancelled.push(def);
            }
        }

        Ok(cancelled)
    }

    pub async fn fetch_query_stage_state(&self) -> Result<QueryStageState, QueueError> {
        let connection = self.queue_driver.create_connection().await?;
        let state = connection.get_query_stage_state(false).await;
        self.queue_driver.release(connection);

        state
    }

    /// Reports what is happening to the work identified by `stage_query_key`
    /// (`QO/QueryQueue.ts:688-712`).
    ///
    /// `priority_filter` narrows the lookup to one priority, which is how a pre-aggregation
    /// build reports its own queue position rather than the queue's.
    pub async fn query_stage(
        &self,
        stage_query_key: &CacheKey,
        priority_filter: Option<i32>,
        query_stage_state: Option<QueryStageState>,
    ) -> Result<Option<QueryStage>, QueueError> {
        let state = match query_stage_state {
            Some(state) => state,
            None => self.fetch_query_stage_state().await?,
        };

        let target = self.redis_hash(stage_query_key);

        let query_in_queue = state.defs.iter().map(|(_, def)| def).find(|def| {
            let matches_stage = def
                .stage_query_key
                .as_ref()
                .map(|key| self.redis_hash(key) == target)
                .unwrap_or(false);

            matches_stage
                && priority_filter
                    .map(|priority| def.priority == priority)
                    .unwrap_or(true)
        });

        let query_in_queue = match query_in_queue {
            Some(def) => def,
            None => return Ok(None),
        };

        let query_key_hash = self.redis_hash(&query_in_queue.query_key);

        if state.active.iter().any(|key| key == &query_key_hash) {
            return Ok(Some(QueryStage::executing(
                query_in_queue
                    .start_query_time
                    .map(|start_query_time| now_ms() - start_query_time),
            )));
        }

        let defs: HashMap<&str, &QueryDef> = state
            .defs
            .iter()
            .map(|(key, def)| (key.as_str(), def))
            .collect();

        let index = state
            .to_process
            .iter()
            .filter(|key| match priority_filter {
                Some(priority) => defs
                    .get(key.as_str())
                    .map(|def| def.priority == priority)
                    .unwrap_or(false),
                None => true,
            })
            .position(|key| key == &query_key_hash);

        Ok(index.map(QueryStage::in_queue))
    }
}

impl std::fmt::Debug for QueryQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryQueue")
            .field("redisQueuePrefix", &self.redis_queue_prefix)
            .field("config", &self.config)
            .field("processUid", &self.process_uid)
            .field(
                "queryHandlers",
                &self.query_handlers.keys().collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}
