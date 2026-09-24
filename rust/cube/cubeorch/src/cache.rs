//! `QueryCache` — the cache/queue layer in front of the data sources.
//!
//! Port of `QO/QueryCache.ts`: `cachedQueryResult` and its four branches, `renewQuery`,
//! `cacheQueryResult`, the refresh key pipeline and the pre-aggregation table name
//! substitution.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use cubecache::{
    cache_key_string, decide_cache_action, get_cache_hash, query_cache_key, CacheAction,
    CacheActionOptions, CacheDriver, CacheEntry, CacheKey, KeyValue, MemoryCacheDriver,
    MemoryResultCache, QueryCacheKeyInput, SQL_QUERY_RESULT,
};
use cubedriver::{Driver, QueryOptions, Row, StreamOptions};
use cubequeue::{
    ExecuteInQueueOptions, QueryQueue, QueryQueueConfig, QueryStream, QueryStreamWriter,
    QueueError, QueuePriority,
};
use futures::{
    future::{BoxFuture, FutureExt, Shared},
    stream::BoxStream,
    StreamExt,
};
use serde_json::{json, Value};

use crate::{
    error::OrchError,
    refresh_key::{evaluate_local_refresh_key_now, is_valid_local_refresh_key},
    types::{
        resolve_queue_priority, CacheMode, PreAggTableToTempTable, QueryBody, QueryWithParams,
    },
};

/// Resolves the driver of a data source (`DriverFactoryByDataSource`).
pub type DriverFactory =
    Arc<dyn Fn(String) -> BoxFuture<'static, Result<Arc<dyn Driver>, OrchError>> + Send + Sync>;

/// `logger(message, event)`.
pub type LoggerFn = Arc<dyn Fn(&str, Value) + Send + Sync>;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// `QueryCacheOptions` (`QO/QueryCache.ts:164-181`), minus the driver factories, which are
/// constructor arguments here.
#[derive(Clone, Debug, Default)]
pub struct QueryCacheOptions {
    /// Overrides every refresh key's own `renewalThreshold`.
    pub refresh_key_renewal_threshold: Option<u64>,
    /// `CUBEJS_REFRESH_KEY_LOCAL_TIME`.
    pub local_refresh_key: bool,
    /// Serves the cached value and renews behind it instead of blocking.
    pub background_renew: bool,
    pub max_in_memory_cache_entries: Option<usize>,
    /// Runs external queries inline, without the external queue or the cache.
    pub skip_external_cache_and_queue: bool,
    /// Seconds a client blocks before it is told to continue waiting.
    pub continue_wait_timeout: Option<u64>,
}

/// What `cachedQueryResult` and `renewQuery` hand back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CachedQueryResult {
    /// The rows, as the driver produced them.
    pub data: Value,
    /// Present on the branches that go through `renewQuery`.
    pub refresh_key_values: Option<Vec<Value>>,
    /// Epoch milliseconds of the cache entry this result came from.
    pub last_refresh_time: Option<i64>,
}

/// What `cachedQueryResult` hands back.
///
/// A persistent query answers with the stream its rows travel on rather than with the rows
/// (`QO/QueryCache.ts:316-355`, `QueryOrchestrator.ts:279-282`).
#[derive(Clone, Debug, PartialEq)]
pub enum CachedQueryOutcome {
    Result(CachedQueryResult),
    Stream(QueryStream),
}

impl CachedQueryOutcome {
    pub fn into_result(self) -> Option<CachedQueryResult> {
        match self {
            CachedQueryOutcome::Result(result) => Some(result),
            CachedQueryOutcome::Stream(_) => None,
        }
    }

    pub fn into_stream(self) -> Option<QueryStream> {
        match self {
            CachedQueryOutcome::Stream(stream) => Some(stream),
            CachedQueryOutcome::Result(_) => None,
        }
    }

    pub fn as_result(&self) -> Option<&CachedQueryResult> {
        match self {
            CachedQueryOutcome::Result(result) => Some(result),
            CachedQueryOutcome::Stream(_) => None,
        }
    }
}

/// `CacheQueryResultOptions` (`QO/QueryCache.ts:35-50`).
#[derive(Clone, Debug)]
pub struct CacheQueryResultOptions {
    pub renewal_threshold: Option<u64>,
    pub renewal_key: Option<CacheKey>,
    pub priority: Option<i32>,
    pub external: bool,
    pub request_id: Option<String>,
    pub data_source: String,
    pub wait_for_renew: bool,
    pub force_no_cache: bool,
    pub use_in_memory: bool,
    pub persistent: bool,
    pub primary_query: bool,
    pub renew_cycle: bool,
}

impl Default for CacheQueryResultOptions {
    fn default() -> Self {
        Self {
            renewal_threshold: None,
            renewal_key: None,
            priority: None,
            external: false,
            request_id: None,
            data_source: "default".to_string(),
            wait_for_renew: false,
            force_no_cache: false,
            use_in_memory: false,
            persistent: false,
            primary_query: false,
            renew_cycle: false,
        }
    }
}

/// `RefreshKeyCacheOptions` (`:54-56`) — deliberately narrow, the cache key and the renewal
/// threshold are derived inside `cache_refresh_key_result`.
#[derive(Clone, Debug, Default)]
pub struct RefreshKeyCacheOptions {
    pub priority: Option<i32>,
    pub request_id: Option<String>,
    pub wait_for_renew: bool,
    pub data_source: String,
}

/// `LoadRefreshKeyOptions` (`:84-89`).
#[derive(Clone, Debug, Default)]
pub struct LoadRefreshKeyOptions {
    pub request_id: Option<String>,
    pub skip_refresh_key_wait_for_renew: bool,
    /// Inherited from the query the keys are refreshed for: a blocked request waits on them too.
    pub priority: Option<i32>,
    pub data_source: String,
}

/// The options `renewQuery` carries down to `cacheQueryResult`.
#[derive(Clone, Debug, Default)]
pub struct RenewQueryOptions {
    pub request_id: Option<String>,
    pub skip_refresh_key_wait_for_renew: bool,
    pub priority: Option<i32>,
    pub external: bool,
    pub force_no_cache: bool,
    pub data_source: String,
    pub persistent: bool,
    pub renew_cycle: bool,
}

/// What one `cacheQueryResult` call needs to know about its key
/// (`CacheOperationContext`, `:157-165`).
#[derive(Clone, Debug)]
struct CacheOperationContext {
    cache_key: CacheKey,
    redis_key: String,
    renewal_key: Option<String>,
    expiration: u64,
    span_id: String,
    options: CacheQueryResultOptions,
}

type SharedRefreshKey = Shared<BoxFuture<'static, Result<Vec<Value>, OrchError>>>;

pub struct QueryCache {
    cache_prefix: String,
    driver_factory: DriverFactory,
    external_driver_factory: Option<DriverFactory>,
    logger: LoggerFn,
    options: QueryCacheOptions,
    cache_driver: Arc<dyn CacheDriver>,
    memory_cache: MemoryResultCache,
    queues: Mutex<HashMap<String, Arc<QueryQueue>>>,
    external_queue: Mutex<Option<Arc<QueryQueue>>>,
    /// `@AsyncDebounce()` on `loadRefreshKey`: one in-flight load per (query, options).
    refresh_key_inflight: Mutex<HashMap<String, SharedRefreshKey>>,
    queue_config: QueryQueueConfig,
}

impl std::fmt::Debug for QueryCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryCache")
            .field("cache_prefix", &self.cache_prefix)
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl QueryCache {
    pub fn new(
        cache_prefix: impl Into<String>,
        driver_factory: DriverFactory,
        logger: LoggerFn,
        options: QueryCacheOptions,
    ) -> Arc<Self> {
        Self::builder(cache_prefix, driver_factory)
            .logger(logger)
            .options(options)
            .build()
    }

    pub fn builder(
        cache_prefix: impl Into<String>,
        driver_factory: DriverFactory,
    ) -> QueryCacheBuilder {
        QueryCacheBuilder {
            cache_prefix: cache_prefix.into(),
            driver_factory,
            external_driver_factory: None,
            logger: None,
            cache_driver: None,
            options: QueryCacheOptions::default(),
            queue_config: QueryQueueConfig::default(),
        }
    }

    pub fn cache_driver(&self) -> &Arc<dyn CacheDriver> {
        &self.cache_driver
    }

    pub fn options(&self) -> &QueryCacheOptions {
        &self.options
    }

    fn log(&self, message: &str, event: Value) {
        (self.logger)(message, event);
    }

    // ------------------------------------------------------------------
    // Keys
    // ------------------------------------------------------------------

    /// `QueryCache.getKey(catalog, key)`.
    pub fn get_key(&self, catalog: &str, key: &str) -> String {
        cache_key_string(&self.cache_prefix, catalog, key)
    }

    /// `QueryCache.queryCacheKey(cacheKey)` — the cache driver key of a result.
    pub fn query_cache_key(&self, cache_key: &CacheKey) -> String {
        self.get_key(
            SQL_QUERY_RESULT,
            &get_cache_hash(cache_key, cubecache::process_uid()),
        )
    }

    /// `QueryCache.queryCacheKey(queryBody)` (static) — the identity of a query result.
    pub fn query_body_cache_key(query_body: &QueryBody) -> CacheKey {
        query_cache_key(&QueryCacheKeyInput {
            query: query_body.query.clone(),
            values: query_body.values.clone(),
            pre_aggregation_load_sql: query_body
                .pre_aggregations
                .iter()
                .map(|pre_aggregation| {
                    pre_aggregation
                        .load_sql
                        .as_ref()
                        .map(|load_sql| load_sql.to_key_value())
                        .unwrap_or(KeyValue::Null)
                })
                .collect(),
            invalidate: query_body
                .invalidate
                .as_ref()
                .map(QueryWithParams::to_key_value),
            persistent: query_body.persistent,
        })
    }

    /// `QueryCache.refreshKeyIdentity(sqlQuery, dataSource)` (`:499-508`).
    pub fn refresh_key_identity(sql_query: &QueryWithParams, data_source: &str) -> CacheKey {
        cubecache::refresh_key_identity(
            &sql_query.sql,
            &sql_query.params,
            sql_query.is_external(),
            Some(data_source),
        )
    }

    /// `QueryCache.buildRangeInvalidateKey` (`:516-521`).
    pub fn build_range_invalidate_key(
        invalidate_key_queries: &[QueryWithParams],
        data_source: &str,
    ) -> Option<CacheKey> {
        invalidate_key_queries
            .first()
            .map(|query| Self::refresh_key_identity(query, data_source))
    }

    /// `QueryCache.refreshKeyCacheKey`.
    pub fn refresh_key_cache_key(&self, sql_query: &QueryWithParams, data_source: &str) -> String {
        self.query_cache_key(&Self::refresh_key_identity(sql_query, data_source))
    }

    // ------------------------------------------------------------------
    // Pre-aggregation table name substitution
    // ------------------------------------------------------------------

    /// `QueryCache.replacePreAggregationTableNamesInSql` (`:545-571`).
    ///
    /// One left to right pass with the source names tried longest first, which is what the
    /// alternation regex of the source does. Replacing name by name instead would corrupt
    /// names that are prefixes of other names (`name1` vs `name10`) and would rescan the
    /// target names it just inserted, which contain the source name as a prefix.
    pub fn replace_pre_aggregation_table_names_in_sql(
        sql: &str,
        replacements: &[(String, String)],
    ) -> String {
        if replacements.is_empty() {
            return sql.to_string();
        }

        let mut sorted: Vec<&(String, String)> = replacements.iter().collect();
        sorted.sort_by_key(|entry| std::cmp::Reverse(entry.0.len()));

        let mut out = String::with_capacity(sql.len());
        let mut position = 0;

        while position < sql.len() {
            let rest = &sql[position..];

            match sorted
                .iter()
                .find(|(name, _)| !name.is_empty() && rest.starts_with(name.as_str()))
            {
                Some((name, target)) => {
                    out.push_str(target);
                    position += name.len();
                }
                None => {
                    let character = rest.chars().next().expect("non empty remainder");
                    out.push(character);
                    position += character.len_utf8();
                }
            }
        }

        out
    }

    /// `QueryCache.replacePreAggregationTableNames` (`:573-584`) — the params and the query
    /// options of a `QueryWithParams` survive the substitution untouched.
    pub fn replace_pre_aggregation_table_names(
        query: &QueryWithParams,
        replacements: &[(String, String)],
    ) -> QueryWithParams {
        QueryWithParams {
            sql: Self::replace_pre_aggregation_table_names_in_sql(&query.sql, replacements),
            params: query.params.clone(),
            options: query.options.clone(),
        }
    }

    /// `[tableName, targetTableName]` pairs of the loaded pre-aggregations.
    pub fn table_name_replacements(
        pre_aggregation_tables: &[PreAggTableToTempTable],
    ) -> Vec<(String, String)> {
        pre_aggregation_tables
            .iter()
            .map(|(table_name, temp_table)| {
                (table_name.clone(), temp_table.target_table_name.clone())
            })
            .collect()
    }

    // ------------------------------------------------------------------
    // Queues
    // ------------------------------------------------------------------

    fn build_queue(
        self: &Arc<Self>,
        prefix: String,
        factory: DriverFactory,
        data_source: String,
        skip_queue: bool,
    ) -> Arc<QueryQueue> {
        let logger = self.logger.clone();
        let mut config = self.queue_config.clone();
        config.skip_queue = skip_queue;
        if let Some(continue_wait_timeout) = self.options.continue_wait_timeout {
            config.continue_wait_timeout = continue_wait_timeout;
        }
        config.logger = Some(logger.clone());

        let high_water_mark = config.query_stream_high_water_mark;
        let stream_factory = factory.clone();
        let stream_logger = logger.clone();
        let stream_data_source = data_source.clone();

        QueryQueue::builder(prefix)
            .config(config)
            .stream_handler(move |request: Value, writer: QueryStreamWriter| {
                let factory = stream_factory.clone();
                let logger = stream_logger.clone();
                let data_source = stream_data_source.clone();

                // `streamHandler` (`QO/QueryCache.ts:812-857`): pipe the driver's rows into
                // the stream the queue created, and take the source connection down with
                // whichever end finishes first.
                async move {
                    logger("Streaming SQL", payload_for_log(&request));

                    let request = match StreamRequest::from_payload(&request) {
                        Ok(request) => request,
                        Err(error) => return Err(fail_stream(&writer, error)),
                    };

                    let driver = match factory(data_source).await {
                        Ok(driver) => driver,
                        Err(error) => return Err(fail_stream(&writer, error.to_string())),
                    };

                    let source = driver
                        .stream(
                            &request.sql,
                            &request.params,
                            &StreamOptions {
                                high_water_mark,
                                request_id: request.request_id.clone(),
                            },
                        )
                        .await;

                    let source = match source {
                        Ok(source) => source,
                        // The Node code emits the error on the target stream, so the
                        // consumer is told rather than left waiting for rows.
                        Err(error) => return Err(fail_stream(&writer, error.to_string())),
                    };

                    writer.set_columns(
                        source
                            .columns
                            .iter()
                            .map(|column| column.name.clone())
                            .collect(),
                    );

                    let outcome = pump_rows(source.rows, &writer, high_water_mark).await;

                    // Dropping the row stream releases the connection behind it, which is
                    // what `source.release()` does in the source.
                    match outcome {
                        Ok(()) => {
                            logger(
                                "Streaming successfully completed",
                                json!({ "requestId": request.request_id }),
                            );

                            Ok(())
                        }
                        Err(error) => {
                            logger(
                                "Streaming done with error",
                                json!({
                                    "query": request.sql,
                                    "query_values": request.params,
                                    "error": error,
                                }),
                            );

                            Err(fail_stream(&writer, error))
                        }
                    }
                }
            })
            .query_handler("query", move |request: Value, _cancel| {
                let factory = factory.clone();
                let logger = logger.clone();
                let data_source = data_source.clone();

                async move {
                    logger("Executing SQL", payload_for_log(&request));

                    let sql = request
                        .get("query")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "Query payload carries no SQL".to_string())?
                        .to_string();
                    let params: Vec<Value> = request
                        .get("values")
                        .and_then(Value::as_array)
                        .map(|values| values.to_vec())
                        .unwrap_or_default();
                    let request_id = request
                        .get("requestId")
                        .and_then(Value::as_str)
                        .map(str::to_string);

                    let driver = factory(data_source).await.map_err(|e| e.to_string())?;
                    let options = QueryOptions {
                        request_id,
                        ..Default::default()
                    };
                    let result = driver
                        .query(&sql, &params, &options)
                        .await
                        .map_err(|e| e.to_string())?;

                    Ok(Value::Array(
                        result
                            .to_json_rows()
                            .into_iter()
                            .map(Value::Object)
                            .collect(),
                    ))
                }
            })
            .build()
    }

    /// `QueryCache.getQueue(dataSource)` — the queue prefix is `SQL_QUERY_${prefix}_${ds}`.
    pub fn get_queue(self: &Arc<Self>, data_source: &str) -> Arc<QueryQueue> {
        let data_source = if data_source.is_empty() {
            "default"
        } else {
            data_source
        };

        if let Some(queue) = self.queues.lock().unwrap().get(data_source) {
            return queue.clone();
        }

        let queue = self.build_queue(
            format!("SQL_QUERY_{}_{}", self.cache_prefix, data_source),
            self.driver_factory.clone(),
            data_source.to_string(),
            false,
        );

        let mut queues = self.queues.lock().unwrap();
        queues
            .entry(data_source.to_string())
            .or_insert(queue)
            .clone()
    }

    /// `QueryCache.getExternalQueue()` — prefix `SQL_QUERY_EXT_${prefix}`.
    pub fn get_external_queue(self: &Arc<Self>) -> Result<Arc<QueryQueue>, OrchError> {
        if let Some(queue) = self.external_queue.lock().unwrap().as_ref() {
            return Ok(queue.clone());
        }

        let factory = self.external_driver_factory.clone().ok_or_else(|| {
            OrchError::orchestration(
                "External queries require an external driver factory (Cube Store)",
            )
        })?;

        let queue = self.build_queue(
            format!("SQL_QUERY_EXT_{}", self.cache_prefix),
            factory,
            "default".to_string(),
            self.options.skip_external_cache_and_queue,
        );

        let mut external_queue = self.external_queue.lock().unwrap();

        Ok(external_queue.get_or_insert(queue).clone())
    }

    /// `QueryCache.queryWithRetryAndRelease` (`:600-648`) for the non-persistent path.
    #[allow(clippy::too_many_arguments)]
    pub async fn query_with_retry_and_release(
        self: &Arc<Self>,
        query: &str,
        values: &[String],
        cache_key: &CacheKey,
        data_source: &str,
        external: bool,
        priority: Option<i32>,
        request_id: Option<&str>,
        span_id: Option<&str>,
    ) -> Result<Value, OrchError> {
        let queue = if external {
            self.get_external_queue()?
        } else {
            self.get_queue(data_source)
        };

        let payload = json!({
            "queryKey": cache_key,
            "query": query,
            "values": values,
            "requestId": request_id,
        });

        queue
            .execute_in_queue(
                "query",
                cache_key.clone(),
                payload,
                priority.unwrap_or_else(|| QueuePriority::Interactive.value()),
                ExecuteInQueueOptions {
                    stage_query_key: Some(cache_key.clone()),
                    request_id: request_id.map(str::to_string),
                    span_id: span_id.map(str::to_string),
                },
            )
            .await
            .map_err(OrchError::from)
    }

    /// `QueryCache.queryWithRetryAndRelease` for the persistent path (`:641-647`): the
    /// queue answers with the stream its handler writes into.
    #[allow(clippy::too_many_arguments)]
    pub async fn query_stream_with_retry_and_release(
        self: &Arc<Self>,
        query: &str,
        values: &[String],
        cache_key: &CacheKey,
        data_source: &str,
        external: bool,
        priority: Option<i32>,
        request_id: Option<&str>,
        span_id: Option<&str>,
        alias_name_to_member: Option<&Value>,
    ) -> Result<QueryStream, OrchError> {
        let queue = if external {
            self.get_external_queue()?
        } else {
            self.get_queue(data_source)
        };

        let payload = json!({
            "queryKey": cache_key,
            "query": query,
            "values": values,
            "requestId": request_id,
            "aliasNameToMember": alias_name_to_member,
        });

        queue
            .execute_stream_in_queue(
                cache_key.clone(),
                payload,
                priority.unwrap_or_else(|| QueuePriority::Interactive.value()),
                ExecuteInQueueOptions {
                    stage_query_key: Some(cache_key.clone()),
                    request_id: request_id.map(str::to_string),
                    span_id: span_id.map(str::to_string),
                },
            )
            .await
            .map_err(OrchError::from)
    }

    // ------------------------------------------------------------------
    // cachedQueryResult
    // ------------------------------------------------------------------

    /// `QueryCache.cachedQueryResult(queryBody, preAggregationsTablesToTempTables)`
    /// (`:278-457`).
    pub async fn cached_query_result(
        self: &Arc<Self>,
        query_body: &QueryBody,
        pre_aggregation_tables: &[PreAggTableToTempTable],
    ) -> Result<CachedQueryOutcome, OrchError> {
        let replacements = Self::table_name_replacements(pre_aggregation_tables);
        let query = Self::replace_pre_aggregation_table_names_in_sql(
            query_body.query.as_deref().unwrap_or(""),
            &replacements,
        );
        let values = query_body.values.clone().unwrap_or_default();
        let queue_priority = resolve_queue_priority(query_body.queue_priority);
        let force_no_cache =
            query_body.force_no_cache || query_body.cache_mode == Some(CacheMode::NoCache);

        let cache_key_queries: Vec<QueryWithParams> = query_body
            .cache_key_queries()
            .iter()
            .map(|query| Self::replace_pre_aggregation_table_names(query, &replacements))
            .collect();
        let renewal_threshold = query_body.renewal_threshold();
        let expire_secs = query_body.expire_secs();
        let cache_key = Self::query_body_cache_key(query_body);
        let data_source = query_body.data_source().to_string();
        let external = query_body.is_external();

        // Branch A. The source reads `!cacheKeyQueries || ...`, but `cacheKeyQueriesFrom`
        // always returns an array and `![]` is `false` in JavaScript, so a query without
        // refresh keys does *not* take this branch — it goes through `renewQuery` below with
        // an empty key list. Reproduced rather than "fixed": the branch decides whether a
        // result is cached at all, and diverging here would change which queries are cached.
        if (external && self.options.skip_external_cache_and_queue) || query_body.persistent {
            // A persistent query is never cached and never waits for a result: its rows go
            // straight to the consumer, keyed by the full query cache key so that the
            // `@<processUid>` suffix keeps it on this process.
            if query_body.persistent {
                let stream = self
                    .query_stream_with_retry_and_release(
                        &query,
                        &values,
                        &cache_key,
                        &data_source,
                        external,
                        Some(queue_priority),
                        query_body.request_id.as_deref(),
                        None,
                        query_body.alias_name_to_member.as_ref(),
                    )
                    .await?;

                return Ok(CachedQueryOutcome::Stream(stream));
            }

            // The queue key of this branch is `[query, values]`, not the full query cache key.
            let queue_key = CacheKey::list(vec![
                KeyValue::Str(query.clone()),
                KeyValue::strings(&values),
            ]);

            let data = self
                .query_with_retry_and_release(
                    &query,
                    &values,
                    &queue_key,
                    &data_source,
                    external,
                    Some(queue_priority),
                    query_body.request_id.as_deref(),
                    None,
                )
                .await?;

            return Ok(CachedQueryOutcome::Result(CachedQueryResult {
                data,
                refresh_key_values: None,
                last_refresh_time: None,
            }));
        }

        // Branch B — `must-revalidate`.
        if query_body.cache_mode == Some(CacheMode::MustRevalidate) {
            self.log(
                "Requested renew",
                json!({ "cacheKey": cache_key, "requestId": query_body.request_id }),
            );

            return self
                .renew_query(
                    &query,
                    &values,
                    &cache_key_queries,
                    expire_secs,
                    &cache_key,
                    renewal_threshold,
                    RenewQueryOptions {
                        request_id: query_body.request_id.clone(),
                        skip_refresh_key_wait_for_renew: true,
                        priority: Some(queue_priority),
                        external,
                        force_no_cache,
                        data_source,
                        persistent: query_body.persistent,
                        renew_cycle: false,
                    },
                )
                .await
                .map(CachedQueryOutcome::Result);
        }

        // Branch C — the default: renew in the foreground, then keep the cycle running.
        if !self.options.background_renew
            && query_body.cache_mode != Some(CacheMode::StaleWhileRevalidate)
        {
            let result = self
                .renew_query(
                    &query,
                    &values,
                    &cache_key_queries,
                    expire_secs,
                    &cache_key,
                    renewal_threshold,
                    RenewQueryOptions {
                        request_id: query_body.request_id.clone(),
                        skip_refresh_key_wait_for_renew: true,
                        priority: Some(queue_priority),
                        external,
                        force_no_cache,
                        data_source: data_source.clone(),
                        persistent: query_body.persistent,
                        renew_cycle: false,
                    },
                )
                .await?;

            // Keep the cycle after the foreground renewal: concurrent passes race on a cold
            // cache, and it stays necessary when `skipRefreshKeyWaitForRenew` served a stale
            // key from a warm cache. It re-runs at Background because nothing blocks on it.
            self.start_renew_cycle(
                &query,
                &values,
                &cache_key_queries,
                expire_secs,
                &cache_key,
                renewal_threshold,
                RenewQueryOptions {
                    request_id: query_body.request_id.clone(),
                    external,
                    data_source,
                    persistent: query_body.persistent,
                    ..Default::default()
                },
            );

            return Ok(CachedQueryOutcome::Result(result));
        }

        // Branch D — background renew.
        self.log(
            "Background fetch",
            json!({ "cacheKey": cache_key, "requestId": query_body.request_id }),
        );

        let data = self
            .cache_query_result(
                &query,
                &values,
                &cache_key,
                expire_secs,
                CacheQueryResultOptions {
                    priority: Some(queue_priority),
                    force_no_cache,
                    external,
                    request_id: query_body.request_id.clone(),
                    data_source: data_source.clone(),
                    persistent: query_body.persistent,
                    ..Default::default()
                },
            )
            .await?;

        if !force_no_cache {
            self.start_renew_cycle(
                &query,
                &values,
                &cache_key_queries,
                expire_secs,
                &cache_key,
                renewal_threshold,
                RenewQueryOptions {
                    request_id: query_body.request_id.clone(),
                    external,
                    data_source,
                    persistent: query_body.persistent,
                    ..Default::default()
                },
            );
        }

        Ok(CachedQueryOutcome::Result(CachedQueryResult {
            data,
            refresh_key_values: None,
            last_refresh_time: self.last_refresh_time(&cache_key).await?,
        }))
    }

    /// `QueryCache.startRenewCycle` (`:902-932`): a detached `renewQuery` at background
    /// priority whose failures are logged rather than surfaced.
    #[allow(clippy::too_many_arguments)]
    pub fn start_renew_cycle(
        self: &Arc<Self>,
        query: &str,
        values: &[String],
        cache_key_queries: &[QueryWithParams],
        expire_secs: u64,
        cache_key: &CacheKey,
        renewal_threshold: Option<u64>,
        options: RenewQueryOptions,
    ) {
        let this = self.clone();
        let query = query.to_string();
        let values = values.to_vec();
        let cache_key_queries = cache_key_queries.to_vec();
        let cache_key = cache_key.clone();
        let options = RenewQueryOptions {
            renew_cycle: true,
            ..options
        };

        tokio::spawn(async move {
            let request_id = options.request_id.clone();
            let result = this
                .renew_query(
                    &query,
                    &values,
                    &cache_key_queries,
                    expire_secs,
                    &cache_key,
                    renewal_threshold,
                    options,
                )
                .await;

            if let Err(error) = result {
                if !error.is_continue_wait() {
                    this.log(
                        "Error while renew cycle",
                        json!({
                            "query": query,
                            "query_values": values,
                            "error": error.to_string(),
                            "requestId": request_id,
                        }),
                    );
                }
            }
        });
    }

    /// `QueryCache.renewQuery` (`:934-996`).
    #[allow(clippy::too_many_arguments)]
    pub async fn renew_query(
        self: &Arc<Self>,
        query: &str,
        values: &[String],
        cache_key_queries: &[QueryWithParams],
        expire_secs: u64,
        cache_key: &CacheKey,
        renewal_threshold: Option<u64>,
        options: RenewQueryOptions,
    ) -> Result<CachedQueryResult, OrchError> {
        let refresh_key_values = match self
            .load_refresh_keys(
                cache_key_queries,
                expire_secs,
                &LoadRefreshKeyOptions {
                    request_id: options.request_id.clone(),
                    skip_refresh_key_wait_for_renew: options.skip_refresh_key_wait_for_renew,
                    priority: options.priority,
                    data_source: options.data_source.clone(),
                },
            )
            .await
        {
            Ok(values) => values,
            // A continue wait on a refresh key is the caller's continue wait.
            Err(error) if error.is_continue_wait() => return Err(error),
            Err(error) => {
                self.log(
                    "Error fetching cache key queries",
                    json!({ "error": error.to_string(), "requestId": options.request_id }),
                );
                Vec::new()
            }
        };

        // `[cacheKeyQueries, cacheKeyQueryResults, queryCacheKey([query, values])]`.
        let renewal_key = CacheKey::list(vec![
            KeyValue::Array(
                cache_key_queries
                    .iter()
                    .map(QueryWithParams::to_key_value)
                    .collect(),
            ),
            KeyValue::Array(refresh_key_values.iter().map(KeyValue::from).collect()),
            KeyValue::Str(self.query_cache_key(&CacheKey::list(vec![
                KeyValue::Str(query.to_string()),
                KeyValue::strings(values),
            ]))),
        ]);

        let data = self
            .cache_query_result(
                query,
                values,
                cache_key,
                expire_secs,
                CacheQueryResultOptions {
                    renewal_threshold: Some(renewal_threshold.unwrap_or(6 * 60 * 60)),
                    renewal_key: Some(renewal_key),
                    wait_for_renew: true,
                    force_no_cache: options.force_no_cache,
                    priority: options.priority,
                    external: options.external,
                    request_id: options.request_id.clone(),
                    data_source: options.data_source.clone(),
                    persistent: options.persistent,
                    primary_query: true,
                    renew_cycle: options.renew_cycle,
                    use_in_memory: false,
                },
            )
            .await?;

        Ok(CachedQueryResult {
            data,
            refresh_key_values: Some(refresh_key_values),
            last_refresh_time: self.last_refresh_time(cache_key).await?,
        })
    }

    // ------------------------------------------------------------------
    // Refresh keys
    // ------------------------------------------------------------------

    /// `QueryCache.isLocalRefreshKeyActive` (`:224-226`).
    pub fn is_local_refresh_key_active(&self) -> bool {
        self.options.local_refresh_key && self.options.refresh_key_renewal_threshold.is_none()
    }

    /// `QueryCache.localRefreshKeyResult` (`:228-246`).
    pub fn local_refresh_key_result(&self, query: &QueryWithParams) -> Option<Vec<Value>> {
        let descriptor = query
            .options
            .as_ref()
            .and_then(|options| options.local_refresh_key.as_ref());

        if !self.options.local_refresh_key || !is_valid_local_refresh_key(descriptor) {
            return None;
        }

        // `refreshKeyRenewalThreshold` throttles how often the SQL result is re-read, and
        // that is also what bounds how often the key advances. A locally evaluated key has no
        // cache entry to age out, so honouring the override means staying on the SQL path.
        if !self.is_local_refresh_key_active() {
            return None;
        }

        Some(evaluate_local_refresh_key_now(descriptor?))
    }

    /// `QueryCache.cacheRefreshKeyResult` (`:523-542`).
    pub async fn cache_refresh_key_result(
        self: &Arc<Self>,
        sql_query: &QueryWithParams,
        expiration: u64,
        options: &RefreshKeyCacheOptions,
    ) -> Result<Vec<Value>, OrchError> {
        if let Some(local) = self.local_refresh_key_result(sql_query) {
            return Ok(local);
        }

        let cache_key = Self::refresh_key_identity(sql_query, &options.data_source);
        let renewal_threshold = self
            .options
            .refresh_key_renewal_threshold
            .or_else(|| {
                sql_query
                    .options
                    .as_ref()
                    .and_then(|options| options.renewal_threshold)
            })
            .unwrap_or(2 * 60);

        let result = self
            .cache_query_result(
                &sql_query.sql,
                &sql_query.params,
                &cache_key,
                expiration,
                CacheQueryResultOptions {
                    renewal_threshold: Some(renewal_threshold),
                    // A refresh key renews against its own identity.
                    renewal_key: Some(cache_key.clone()),
                    use_in_memory: true,
                    external: sql_query.is_external(),
                    priority: options.priority,
                    request_id: options.request_id.clone(),
                    wait_for_renew: options.wait_for_renew,
                    data_source: options.data_source.clone(),
                    ..Default::default()
                },
            )
            .await?;

        Ok(match result {
            Value::Array(rows) => rows,
            Value::Null => Vec::new(),
            other => vec![other],
        })
    }

    /// `QueryCache.loadRefreshKeys` (`:1011-1017`) — every key of a query, in order.
    pub async fn load_refresh_keys(
        self: &Arc<Self>,
        cache_key_queries: &[QueryWithParams],
        expire_secs: u64,
        options: &LoadRefreshKeyOptions,
    ) -> Result<Vec<Value>, OrchError> {
        let loads = cache_key_queries
            .iter()
            .map(|query| self.load_refresh_key(query, expire_secs, options));

        // `Promise.all`: the values keep the order of the queries and the first rejection wins.
        let results = futures::future::try_join_all(loads).await?;

        Ok(results.into_iter().map(Value::Array).collect())
    }

    /// `@AsyncDebounce() loadRefreshKey` (`:1019-1030`): concurrent callers asking for the
    /// same key under the same options join the load that is already running, so a query
    /// whose pre-aggregations share a refresh key runs it once.
    pub async fn load_refresh_key(
        self: &Arc<Self>,
        query: &QueryWithParams,
        expire_secs: u64,
        options: &LoadRefreshKeyOptions,
    ) -> Result<Vec<Value>, OrchError> {
        let refresh_options = RefreshKeyCacheOptions {
            wait_for_renew: !options.skip_refresh_key_wait_for_renew,
            priority: options.priority,
            request_id: options.request_id.clone(),
            data_source: options.data_source.clone(),
        };

        let debounce_key = debounce_key(query, expire_secs, &refresh_options);

        let (shared, owner) = {
            let mut inflight = self.refresh_key_inflight.lock().unwrap();

            match inflight.get(&debounce_key) {
                Some(shared) => (shared.clone(), false),
                None => {
                    let this = self.clone();
                    let query = query.clone();
                    let options = refresh_options.clone();
                    let shared: SharedRefreshKey = async move {
                        this.cache_refresh_key_result(&query, expire_secs, &options)
                            .await
                    }
                    .boxed()
                    .shared();

                    inflight.insert(debounce_key.clone(), shared.clone());
                    (shared, true)
                }
            }
        };

        let result = shared.await;

        if owner {
            self.refresh_key_inflight
                .lock()
                .unwrap()
                .remove(&debounce_key);
        }

        result
    }

    // ------------------------------------------------------------------
    // cacheQueryResult
    // ------------------------------------------------------------------

    fn cache_operation_context(
        &self,
        cache_key: &CacheKey,
        expiration: u64,
        options: CacheQueryResultOptions,
    ) -> CacheOperationContext {
        let redis_key = self.query_cache_key(cache_key);
        let renewal_key = options.renewal_key.as_ref().map(|renewal_key| {
            // A refresh key entry renews against its own key, so hashing it twice is wasted work.
            if renewal_key == cache_key {
                redis_key.clone()
            } else {
                self.query_cache_key(renewal_key)
            }
        });

        CacheOperationContext {
            cache_key: cache_key.clone(),
            redis_key,
            renewal_key,
            expiration,
            span_id: uuid::Uuid::new_v4().simple().to_string(),
            options,
        }
    }

    fn log_context(&self, ctx: &CacheOperationContext, message: &str, extra: Value) {
        let mut event = json!({
            "cacheKey": ctx.cache_key,
            "requestId": ctx.options.request_id,
            "spanId": ctx.span_id,
            "primaryQuery": ctx.options.primary_query,
            "renewCycle": ctx.options.renew_cycle,
        });

        if let (Some(event), Value::Object(extra)) = (event.as_object_mut(), extra) {
            for (key, value) in extra {
                event.insert(key, value);
            }
        }

        self.log(message, event);
    }

    /// `QueryCache.fetchAndCacheQuery` (`:1120-1170`).
    async fn fetch_and_cache_query(
        self: &Arc<Self>,
        query: &str,
        values: &[String],
        ctx: &CacheOperationContext,
    ) -> Result<Value, OrchError> {
        let result = self
            .query_with_retry_and_release(
                query,
                values,
                &ctx.cache_key,
                &ctx.options.data_source,
                ctx.options.external,
                ctx.options.priority,
                ctx.options.request_id.as_deref(),
                Some(&ctx.span_id),
            )
            .await;

        match result {
            Ok(result) => {
                let entry = CacheEntry {
                    time: now_ms(),
                    result: result.clone(),
                    renewal_key: ctx.renewal_key.clone(),
                    request_id: ctx.options.request_id.clone(),
                };

                let set = cubecache::set_entry(
                    self.cache_driver.as_ref(),
                    &ctx.redis_key,
                    &entry,
                    ctx.expiration,
                )
                .await?;

                self.log_context(ctx, "Renewed", Value::Null);
                self.log(
                    "Outgoing network usage",
                    json!({
                        "service": "cache",
                        "requestId": ctx.options.request_id,
                        "spanId": ctx.span_id,
                        "bytes": set.bytes,
                        "cacheKey": ctx.cache_key,
                    }),
                );

                Ok(result)
            }
            Err(error) => {
                // A continue wait is not a failure of the cached value, so the entry stays.
                if !error.is_continue_wait() {
                    self.log_context(ctx, "Dropping Cache", json!({ "error": error.to_string() }));

                    if let Err(remove_error) = self.cache_driver.remove(&ctx.redis_key).await {
                        self.log(
                            "Error removing key",
                            json!({
                                "cacheKey": ctx.cache_key,
                                "spanId": ctx.span_id,
                                "error": remove_error.to_string(),
                                "requestId": ctx.options.request_id,
                            }),
                        );
                    }
                }

                Err(error)
            }
        }
    }

    /// `QueryCache.fetchAndCacheQueryInBackground` (`:1172-1178`).
    fn fetch_and_cache_query_in_background(
        self: &Arc<Self>,
        query: &str,
        values: &[String],
        ctx: &CacheOperationContext,
    ) {
        let this = self.clone();
        let query = query.to_string();
        let values = values.to_vec();
        let ctx = ctx.clone();

        tokio::spawn(async move {
            if let Err(error) = this.fetch_and_cache_query(&query, &values, &ctx).await {
                if !error.is_continue_wait() {
                    this.log_context(
                        &ctx,
                        "Error renewing",
                        json!({ "error": error.to_string() }),
                    );
                }
            }
        });
    }

    /// `QueryCache.cacheQueryResult` (`:1223-1290`).
    pub async fn cache_query_result(
        self: &Arc<Self>,
        query: &str,
        values: &[String],
        cache_key: &CacheKey,
        expiration: u64,
        options: CacheQueryResultOptions,
    ) -> Result<Value, OrchError> {
        let ctx = self.cache_operation_context(cache_key, expiration, options);
        let renewal_threshold = ctx.options.renewal_threshold;

        if ctx.options.force_no_cache {
            self.log_context(&ctx, "Force no cache for", Value::Null);
            return self.fetch_and_cache_query(query, values, &ctx).await;
        }

        let mut entry = if ctx.options.use_in_memory {
            self.memory_cache.get_usable(
                &ctx.redis_key,
                ctx.expiration,
                renewal_threshold,
                ctx.renewal_key.as_deref(),
                now_ms(),
            )
        } else {
            None
        };

        if entry.is_none() {
            entry = cubecache::get_entry(self.cache_driver.as_ref(), &ctx.redis_key).await?;
        }

        let entry = match entry {
            Some(entry) => entry,
            None => {
                self.log_context(&ctx, "Missing cache for", Value::Null);
                return self.fetch_and_cache_query(query, values, &ctx).await;
            }
        };

        let renewed_ago = entry.renewed_ago(now_ms());

        self.log_context(
            &ctx,
            "Found cache entry",
            json!({
                "time": entry.time,
                "renewedAgo": renewed_ago,
                "renewalKey": entry.renewal_key,
                "newRenewalKey": ctx.renewal_key,
                "renewalThreshold": renewal_threshold,
            }),
        );

        let action = decide_cache_action(
            &entry,
            renewed_ago,
            &CacheActionOptions {
                renewal_threshold,
                request_id: ctx.options.request_id.clone(),
                wait_for_renew: ctx.options.wait_for_renew,
                renew_cycle: ctx.options.renew_cycle,
            },
            ctx.renewal_key.as_deref(),
        );

        match action {
            CacheAction::WaitForRenew => {
                self.log_context(
                    &ctx,
                    "Waiting for renew",
                    json!({ "renewalThreshold": renewal_threshold }),
                );
                return self.fetch_and_cache_query(query, values, &ctx).await;
            }
            CacheAction::RefreshSameRequest => {
                self.log_context(
                    &ctx,
                    "Same request cache hit (background refresh)",
                    json!({ "renewalThreshold": renewal_threshold }),
                );
                self.fetch_and_cache_query_in_background(query, values, &ctx);
            }
            CacheAction::RefreshBackground => {
                self.log_context(
                    &ctx,
                    "Renewing existing key",
                    json!({ "renewalThreshold": renewal_threshold }),
                );
                self.fetch_and_cache_query_in_background(query, values, &ctx);
            }
            CacheAction::ServeCached => {}
        }

        self.log_context(&ctx, "Using cache for", Value::Null);
        self.memory_cache.store_if_fresh(
            &ctx.redis_key,
            &entry,
            renewed_ago,
            ctx.options.use_in_memory,
            renewal_threshold,
        );

        Ok(entry.result)
    }

    /// `QueryCache.lastRefreshTime` (`:1292-1295`), in epoch milliseconds.
    pub async fn last_refresh_time(&self, cache_key: &CacheKey) -> Result<Option<i64>, OrchError> {
        let key = self.query_cache_key(cache_key);

        Ok(cubecache::get_entry(self.cache_driver.as_ref(), &key)
            .await?
            .map(|entry| entry.time))
    }

    /// `QueryCache.resultFromCacheIfExists(queryBody)` (`:1297-1308`) — the stale value the
    /// continue-wait path serves as a `slowQuery`.
    pub async fn result_from_cache_if_exists(
        &self,
        query_body: &QueryBody,
    ) -> Result<Option<CachedQueryResult>, OrchError> {
        let cache_key = Self::query_body_cache_key(query_body);
        let key = self.query_cache_key(&cache_key);

        Ok(cubecache::get_entry(self.cache_driver.as_ref(), &key)
            .await?
            .map(|entry| CachedQueryResult {
                data: entry.result,
                refresh_key_values: None,
                last_refresh_time: Some(entry.time),
            }))
    }

    pub async fn test_connection(&self) -> Result<(), OrchError> {
        self.cache_driver.test_connection().await?;

        Ok(())
    }
}

/// Rows of one batch of a streamed result. Bounded so that a large high-water mark buys
/// look-ahead rather than latency; the Node stream emits row by row.
const MAX_STREAM_BATCH_ROWS: usize = 1024;

/// The part of a `stream` queue payload the handler runs on.
struct StreamRequest {
    sql: String,
    params: Vec<Value>,
    request_id: Option<String>,
}

impl StreamRequest {
    fn from_payload(request: &Value) -> Result<Self, String> {
        Ok(Self {
            sql: request
                .get("query")
                .and_then(Value::as_str)
                .ok_or_else(|| "Query payload carries no SQL".to_string())?
                .to_string(),
            params: request
                .get("values")
                .and_then(Value::as_array)
                .map(|values| values.to_vec())
                .unwrap_or_default(),
            request_id: request
                .get("requestId")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }
}

/// Hands the failure to whoever is reading the stream, then returns it so that the queue
/// records it as the query's result.
fn fail_stream(writer: &QueryStreamWriter, error: String) -> String {
    writer.fail(error.clone());

    error
}

/// Pipes the driver's rows into the stream until one end is done.
async fn pump_rows(
    mut rows: BoxStream<'static, Result<Row, cubedriver::DriverError>>,
    writer: &QueryStreamWriter,
    high_water_mark: usize,
) -> Result<(), String> {
    let batch_size = high_water_mark.clamp(1, MAX_STREAM_BATCH_ROWS);
    let mut batch: Vec<Row> = Vec::with_capacity(batch_size);

    loop {
        let next = tokio::select! {
            biased;

            // The consumer went away: stop reading the data source right away rather than
            // when the next row happens to arrive.
            _ = writer.cancelled() => return Ok(()),
            next = rows.next() => next,
        };

        match next {
            Some(Ok(row)) => {
                batch.push(row);

                if batch.len() >= batch_size
                    && writer.write(std::mem::take(&mut batch)).await.is_err()
                {
                    return Ok(());
                }
            }
            Some(Err(error)) => return Err(error.to_string()),
            None => break,
        }
    }

    if !batch.is_empty() {
        let _ = writer.write(batch).await;
    }

    Ok(())
}

/// An inline table's rows are data source content and have no place in a log line, so only
/// its name and columns stay (`QueryCache.payloadForLog`, `:651-661`).
fn payload_for_log(request: &Value) -> Value {
    let mut request = request.clone();

    if let Some(object) = request.as_object_mut() {
        if let Some(Value::Array(inline_tables)) = object.get("inlineTables").cloned() {
            let redacted: Vec<Value> = inline_tables
                .into_iter()
                .map(|table| {
                    json!({
                        "name": table.get("name").cloned().unwrap_or(Value::Null),
                        "columns": table.get("columns").cloned().unwrap_or(Value::Null),
                    })
                })
                .collect();

            object.insert("inlineTables".to_string(), Value::Array(redacted));
        }
    }

    request
}

/// The `AsyncDebounce` cache key: `md5(args.map(JSON.stringify).join(','))`.
fn debounce_key(
    query: &QueryWithParams,
    expire_secs: u64,
    options: &RefreshKeyCacheOptions,
) -> String {
    use md5::{Digest, Md5};

    let payload = format!(
        "{},{},{}",
        serde_json::to_string(query).unwrap_or_default(),
        expire_secs,
        json!({
            "waitForRenew": options.wait_for_renew,
            "priority": options.priority,
            "requestId": options.request_id,
            "dataSource": options.data_source,
        })
    );

    let digest = Md5::digest(payload.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Assembles a [`QueryCache`].
pub struct QueryCacheBuilder {
    cache_prefix: String,
    driver_factory: DriverFactory,
    external_driver_factory: Option<DriverFactory>,
    logger: Option<LoggerFn>,
    cache_driver: Option<Arc<dyn CacheDriver>>,
    options: QueryCacheOptions,
    queue_config: QueryQueueConfig,
}

impl QueryCacheBuilder {
    #[must_use]
    pub fn external_driver_factory(mut self, factory: DriverFactory) -> Self {
        self.external_driver_factory = Some(factory);
        self
    }

    #[must_use]
    pub fn logger(mut self, logger: LoggerFn) -> Self {
        self.logger = Some(logger);
        self
    }

    /// Replaces the default in-process cache driver (`cacheAndQueueDriver: 'memory'`).
    #[must_use]
    pub fn cache_driver(mut self, cache_driver: Arc<dyn CacheDriver>) -> Self {
        self.cache_driver = Some(cache_driver);
        self
    }

    #[must_use]
    pub fn options(mut self, options: QueryCacheOptions) -> Self {
        self.options = options;
        self
    }

    /// Timings of the per data source query queues.
    #[must_use]
    pub fn queue_config(mut self, queue_config: QueryQueueConfig) -> Self {
        self.queue_config = queue_config;
        self
    }

    pub fn build(self) -> Arc<QueryCache> {
        Arc::new(QueryCache {
            cache_prefix: self.cache_prefix,
            driver_factory: self.driver_factory,
            external_driver_factory: self.external_driver_factory,
            logger: self.logger.unwrap_or_else(|| Arc::new(|_, _| {})),
            cache_driver: self
                .cache_driver
                .unwrap_or_else(|| Arc::new(MemoryCacheDriver::new())),
            memory_cache: MemoryResultCache::new(self.options.max_in_memory_cache_entries),
            queues: Mutex::new(HashMap::new()),
            external_queue: Mutex::new(None),
            refresh_key_inflight: Mutex::new(HashMap::new()),
            queue_config: self.queue_config,
            options: self.options,
        })
    }
}

/// Keeps `QueueError` reachable for downstream matching without a direct dependency.
pub type QueueFailure = QueueError;

#[cfg(test)]
mod tests {
    use super::*;

    fn replacements(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(name, target)| (name.to_string(), target.to_string()))
            .collect()
    }

    /// Ported from `test/unit/ReplacePreAggregationTableNames.test.ts`.
    #[test]
    fn replaces_a_single_table_name() {
        assert_eq!(
            QueryCache::replace_pre_aggregation_table_names_in_sql(
                "SELECT * FROM dev_pre_aggregations.orders_rollup",
                &replacements(&[(
                    "dev_pre_aggregations.orders_rollup",
                    "dev_pre_aggregations.orders_rollup_20250401_abc",
                )]),
            ),
            "SELECT * FROM dev_pre_aggregations.orders_rollup_20250401_abc"
        );
    }

    #[test]
    fn does_not_corrupt_names_that_are_prefixes_of_other_names() {
        let base = "dev_pre_aggregations.orders_rollup";
        let entries: Vec<(String, String)> = (0..12)
            .map(|i| {
                (
                    format!("{base}{i}"),
                    format!("(SELECT * FROM {base}_20250401_part{i})"),
                )
            })
            .collect();
        let query = entries
            .iter()
            .enumerate()
            .map(|(i, (name, _))| format!("SELECT * FROM {name} AS \"alias{i}\""))
            .collect::<Vec<_>>()
            .join(" UNION ALL ");

        let result = QueryCache::replace_pre_aggregation_table_names_in_sql(&query, &entries);

        for (i, (_, target)) in entries.iter().enumerate() {
            assert!(result.contains(&format!("{target} AS \"alias{i}\"")));
        }
        assert!(!result.contains(&format!("{base}10")));
    }

    #[test]
    fn does_not_match_source_names_inside_already_inserted_target_names() {
        assert_eq!(
            QueryCache::replace_pre_aggregation_table_names_in_sql(
                "SELECT * FROM pa.rollup10 JOIN pa.rollup1",
                &replacements(&[
                    ("pa.rollup1", "pa.rollup1_aaa_bbb_111"),
                    ("pa.rollup10", "pa.rollup10_ccc_ddd_222"),
                ]),
            ),
            "SELECT * FROM pa.rollup10_ccc_ddd_222 JOIN pa.rollup1_aaa_bbb_111"
        );
    }

    #[test]
    fn returns_the_query_as_is_for_empty_replacements() {
        assert_eq!(
            QueryCache::replace_pre_aggregation_table_names_in_sql("SELECT 1", &[]),
            "SELECT 1"
        );
    }

    #[test]
    fn treats_dollar_signs_in_target_names_literally() {
        assert_eq!(
            QueryCache::replace_pre_aggregation_table_names_in_sql(
                "SELECT * FROM pa.rollup",
                &replacements(&[("pa.rollup", "pa.rollup_$&_$1")]),
            ),
            "SELECT * FROM pa.rollup_$&_$1"
        );
    }

    #[test]
    fn does_not_mutate_the_incoming_order() {
        let entries = replacements(&[("name1", "target1"), ("name10", "target10")]);
        let before = entries.clone();

        QueryCache::replace_pre_aggregation_table_names_in_sql(
            "SELECT * FROM name1, name10",
            &entries,
        );

        assert_eq!(entries, before);
    }

    #[test]
    fn keeps_params_and_options_of_a_query_with_params() {
        let query = QueryWithParams::new(
            "SELECT * FROM dev_pre_aggregations.orders_rollup WHERE id = ?",
            vec!["1".to_string()],
        )
        .with_options(crate::types::RefreshKeyQueryOptions {
            external: Some(true),
            ..Default::default()
        });

        let result = QueryCache::replace_pre_aggregation_table_names(
            &query,
            &replacements(&[(
                "dev_pre_aggregations.orders_rollup",
                "dev_pre_aggregations.orders_rollup_20250401_abc",
            )]),
        );

        assert_eq!(
            result.sql,
            "SELECT * FROM dev_pre_aggregations.orders_rollup_20250401_abc WHERE id = ?"
        );
        assert_eq!(result.params, query.params);
        assert_eq!(result.options, query.options);
    }

    #[test]
    fn substitution_keeps_multi_byte_characters_intact() {
        assert_eq!(
            QueryCache::replace_pre_aggregation_table_names_in_sql(
                "SELECT 'ünïcødé ☃' FROM pa.rollup",
                &replacements(&[("pa.rollup", "pa.rollup_v1")]),
            ),
            "SELECT 'ünïcødé ☃' FROM pa.rollup_v1"
        );
    }

    #[test]
    fn payload_for_log_redacts_inline_table_rows() {
        let payload = json!({
            "query": "SELECT 1",
            "inlineTables": [{ "name": "lambda", "columns": ["a"], "rows": [[1]] }],
        });

        assert_eq!(
            payload_for_log(&payload),
            json!({
                "query": "SELECT 1",
                "inlineTables": [{ "name": "lambda", "columns": ["a"] }],
            })
        );
    }
}
