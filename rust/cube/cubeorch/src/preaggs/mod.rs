//! Pre-aggregation loading.
//!
//! Port of `QO/PreAggregations.ts`, `QO/PreAggregationLoadCache.ts` and
//! `QO/PreAggregationLoader.ts`. See [`version`] for the pure naming and hashing half and
//! [`partition`] for `PreAggregationPartitionRangeLoader`.
//!
//! What is **not** ported yet, and is reported as [`OrchError::NotImplemented`] rather
//! than silently degraded:
//!
//! * lambda rollups (`rollupLambdaId`, inline lambda tables), see [`partition`].
//! * lambda rollups only. An external build whose source driver unloads to CSV
//!   files works: `cubedriver::Driver::upload_downloaded_table_with_indexes`
//!   takes the file list, which Cube Store imports natively.

pub mod partition;
pub mod version;

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, Weak},
};

use cubecache::{CacheKey, KeyValue};
use cubedriver::types::{UnloadOptions, UnloadQuery};
use cubedriver::{
    Column, CreateTableIndex, DownloadQueryResultsOptions, DownloadTableOptions, DownloadedData,
    Driver, DriverCapabilities, ExternalCreateTableOptions, IndexSql, QueryOptions, StreamOptions,
};
use cubequeue::{ExecuteInQueueOptions, QueryQueue, QueryQueueConfig, QueuePriority};
use futures::FutureExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    cache::{DriverFactory, LoggerFn, QueryCache, RefreshKeyCacheOptions},
    error::OrchError,
    types::{
        LoadPreAggregationResult, PreAggTableToTempTable, PreAggregationDescription, QueryBody,
        QueryWithParams,
    },
};

pub use partition::{intersect_date_ranges, PreAggregationPartitionRangeLoader};

pub use version::{
    content_version, decode_time_stamp, encode_time_stamp, get_last_updated_at_timestamp,
    get_structure_version, no_pre_aggregation_partitions_built_message, partition_table_name,
    tables_to_version_entries, target_table_name, TableCacheEntry, TableTimestamp, VersionEntry,
};

const TABLES_USED: &str = "SQL_PRE_AGGREGATIONS_TABLES_USED";
const TABLES_TOUCH: &str = "SQL_PRE_AGGREGATIONS_TABLES_TOUCH";
const REFRESH_END_REACHED: &str = "SQL_PRE_AGGREGATIONS_REFRESH_END_REACHED";
const TABLES_CACHE: &str = "SQL_PRE_AGGREGATIONS_TABLES";

/// `PreAggregationsOptions` (`QO/PreAggregations.ts:248-270`) with the defaults of §6.
#[derive(Clone, Debug)]
pub struct PreAggregationsOptions {
    /// `CUBEJS_MAX_PARTITIONS_PER_CUBE`
    pub max_partitions: usize,
    /// Seconds a `SQL_PRE_AGGREGATIONS_TABLES_USED` key lives.
    pub used_table_persist_time: u64,
    /// `CUBEJS_TOUCH_PRE_AGG_TIMEOUT`
    pub touch_table_persist_time: u64,
    /// Seconds an older structure version is kept before the orphan sweep drops it.
    pub structure_version_persist_time: u64,
    /// `CUBEJS_DROP_PRE_AGG_WITHOUT_TOUCH`
    pub drop_pre_aggregations_without_touch: bool,
    /// Seconds the schema table listing is cached for.
    pub pre_aggregations_schema_cache_expire: u64,
    /// The refresh worker maintains the tables; this instance only serves them.
    pub external_refresh: bool,
    pub skip_external_cache_and_queue: bool,
    pub continue_wait_timeout: Option<u64>,
}

impl Default for PreAggregationsOptions {
    fn default() -> Self {
        Self {
            max_partitions: 10000,
            used_table_persist_time: 600,
            touch_table_persist_time: 86400,
            structure_version_persist_time: 930,
            drop_pre_aggregations_without_touch: true,
            pre_aggregations_schema_cache_expire: 60 * 60,
            external_refresh: false,
            skip_external_cache_and_queue: false,
            continue_wait_timeout: None,
        }
    }
}

/// Per description options of a load (`PreAggregationLoader`'s `options`).
#[derive(Clone, Debug, Default)]
pub struct LoadOptions {
    pub is_job: bool,
    pub wait_for_renew: bool,
    pub force_build: bool,
    pub request_id: Option<String>,
    pub external_refresh: bool,
}

/// `PreAggregations` — the registry of pre-aggregation queues and of the table bookkeeping
/// keys the orphan sweep reads.
pub struct PreAggregations {
    cache_prefix: String,
    driver_factory: DriverFactory,
    external_driver_factory: Option<DriverFactory>,
    logger: LoggerFn,
    query_cache: Arc<QueryCache>,
    options: PreAggregationsOptions,
    queue_config: QueryQueueConfig,
    queues: Mutex<HashMap<String, Arc<QueryQueue>>>,
    /// Breaks the cycle between a queue's build handler and the registry that owns it.
    self_ref: Mutex<Weak<PreAggregations>>,
    used_cache: Mutex<HashSet<String>>,
    touch_cache: Mutex<HashSet<String>>,
}

impl std::fmt::Debug for PreAggregations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreAggregations")
            .field("cache_prefix", &self.cache_prefix)
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl PreAggregations {
    pub fn new(
        cache_prefix: impl Into<String>,
        driver_factory: DriverFactory,
        external_driver_factory: Option<DriverFactory>,
        logger: LoggerFn,
        query_cache: Arc<QueryCache>,
        options: PreAggregationsOptions,
    ) -> Arc<Self> {
        let pre_aggregations = Arc::new(Self {
            cache_prefix: cache_prefix.into(),
            driver_factory,
            external_driver_factory,
            logger,
            query_cache,
            queue_config: QueryQueueConfig {
                // `concurrency: 1` — pre-aggregation builds are serialized per data source.
                concurrency: 1,
                ..Default::default()
            },
            options,
            queues: Mutex::new(HashMap::new()),
            self_ref: Mutex::new(Weak::new()),
            used_cache: Mutex::new(HashSet::new()),
            touch_cache: Mutex::new(HashSet::new()),
        });

        *pre_aggregations.self_ref.lock().unwrap() = Arc::downgrade(&pre_aggregations);

        pre_aggregations
    }

    pub fn options(&self) -> &PreAggregationsOptions {
        &self.options
    }

    pub fn query_cache(&self) -> &Arc<QueryCache> {
        &self.query_cache
    }

    fn log(&self, message: &str, event: Value) {
        (self.logger)(message, event);
    }

    /// `PreAggregations.preAggregationQueryCacheKey(preAggregation)` (`:806-808`).
    pub fn pre_aggregation_query_cache_key(
        pre_aggregation: &PreAggregationDescription,
    ) -> CacheKey {
        CacheKey::string(pre_aggregation.table_name.clone())
    }

    // ------------------------------------------------------------------
    // Bookkeeping keys
    // ------------------------------------------------------------------

    fn tables_used_key(&self, table_name: &str) -> String {
        self.query_cache.get_key(TABLES_USED, table_name)
    }

    fn tables_touch_key(&self, table_name: &str) -> String {
        self.query_cache.get_key(TABLES_TOUCH, table_name)
    }

    fn refresh_end_reached_key(&self) -> String {
        self.query_cache.get_key(REFRESH_END_REACHED, "")
    }

    /// `addTableUsed` (`:359-376`): the in-process set short circuits the write.
    pub async fn add_table_used(&self, table_name: &str) -> Result<(), OrchError> {
        if !self
            .used_cache
            .lock()
            .unwrap()
            .insert(table_name.to_string())
        {
            return Ok(());
        }

        let result = self
            .query_cache
            .cache_driver()
            .set(
                &self.tables_used_key(table_name),
                Value::Bool(true),
                self.options.used_table_persist_time,
            )
            .await;

        if result.is_err() {
            self.used_cache.lock().unwrap().remove(table_name);
        }

        result.map(|_| ()).map_err(OrchError::from)
    }

    pub async fn tables_used(&self) -> Result<Vec<String>, OrchError> {
        let prefix = self.tables_used_key("");

        Ok(self
            .query_cache
            .cache_driver()
            .keys_starting_with(&prefix)
            .await?
            .into_iter()
            .map(|key| key.replacen(&prefix, "", 1))
            .collect())
    }

    pub async fn remove_table_used(&self, table_name: &str) -> Result<(), OrchError> {
        self.used_cache.lock().unwrap().remove(table_name);
        self.query_cache
            .cache_driver()
            .remove(&self.tables_used_key(table_name))
            .await?;

        Ok(())
    }

    /// `updateLastTouch` (`:392-409`).
    pub async fn update_last_touch(&self, table_name: &str) -> Result<(), OrchError> {
        if !self
            .touch_cache
            .lock()
            .unwrap()
            .insert(table_name.to_string())
        {
            return Ok(());
        }

        let result = self
            .query_cache
            .cache_driver()
            .set(
                &self.tables_touch_key(table_name),
                json!(chrono::Utc::now().timestamp_millis()),
                self.options.touch_table_persist_time,
            )
            .await;

        if result.is_err() {
            self.touch_cache.lock().unwrap().remove(table_name);
        }

        result.map(|_| ()).map_err(OrchError::from)
    }

    pub async fn tables_touched(&self) -> Result<Vec<String>, OrchError> {
        let prefix = self.tables_touch_key("");

        Ok(self
            .query_cache
            .cache_driver()
            .keys_starting_with(&prefix)
            .await?
            .into_iter()
            .map(|key| key.replacen(&prefix, "", 1))
            .collect())
    }

    pub async fn remove_table_touched(&self, table_name: &str) -> Result<(), OrchError> {
        self.touch_cache.lock().unwrap().remove(table_name);
        self.query_cache
            .cache_driver()
            .remove(&self.tables_touch_key(table_name))
            .await?;

        Ok(())
    }

    pub async fn update_refresh_end_reached(&self) -> Result<(), OrchError> {
        self.query_cache
            .cache_driver()
            .set(
                &self.refresh_end_reached_key(),
                json!(chrono::Utc::now().timestamp_millis()),
                self.options.touch_table_persist_time,
            )
            .await?;

        Ok(())
    }

    pub async fn get_refresh_end_reached(&self) -> Result<Option<i64>, OrchError> {
        Ok(self
            .query_cache
            .cache_driver()
            .get(&self.refresh_end_reached_key())
            .await?
            .and_then(|value| value.as_i64()))
    }

    // ------------------------------------------------------------------
    // Queue
    // ------------------------------------------------------------------

    /// The build queue of a data source, prefix `SQL_PRE_AGGREGATIONS_${prefix}_${ds}`.
    pub fn get_queue(&self, data_source: &str) -> Arc<QueryQueue> {
        let data_source = if data_source.is_empty() {
            "default"
        } else {
            data_source
        };

        if let Some(queue) = self.queues.lock().unwrap().get(data_source) {
            return queue.clone();
        }

        let mut config = self.queue_config.clone();
        if let Some(continue_wait_timeout) = self.options.continue_wait_timeout {
            config.continue_wait_timeout = continue_wait_timeout;
        }
        config.logger = Some(self.logger.clone());

        let registry = self.self_ref.lock().unwrap().clone();
        let data_source_owned = data_source.to_string();

        let queue = QueryQueue::builder(format!(
            "SQL_PRE_AGGREGATIONS_{}_{}",
            self.cache_prefix, data_source
        ))
        .config(config)
        .query_handler("query", move |request: Value, _cancel| {
            let registry = registry.clone();
            let data_source = data_source_owned.clone();

            async move {
                let registry = registry
                    .upgrade()
                    .ok_or_else(|| "Pre-aggregation registry is gone".to_string())?;

                let build: BuildRequest =
                    serde_json::from_value(request).map_err(|e| e.to_string())?;

                registry
                    .run_build(&data_source, build)
                    .await
                    .map(|_| Value::Null)
                    .map_err(|e| e.to_string())
            }
        })
        .build();

        let mut queues = self.queues.lock().unwrap();

        queues
            .entry(data_source.to_string())
            .or_insert(queue)
            .clone()
    }

    async fn driver(
        &self,
        data_source: &str,
        external: bool,
    ) -> Result<Arc<dyn Driver>, OrchError> {
        if external {
            let factory = self.external_driver_factory.clone().ok_or_else(|| {
                OrchError::orchestration(
                    "externalDriverFactory is not provided. Please provide Cube Store connection \
                     env variables for external pre-aggregations.",
                )
            })?;

            factory(data_source.to_string()).await
        } else {
            (self.driver_factory)(data_source.to_string()).await
        }
    }

    /// The queue handler: `PreAggregationLoader.refresh(newVersionEntry, invalidationKeys, client)`
    /// (`QO/PreAggregationLoader.ts:466-522`).
    async fn run_build(
        self: &Arc<Self>,
        data_source: &str,
        build: BuildRequest,
    ) -> Result<(), OrchError> {
        let target = target_table_name(
            &build.new_version_entry.table_name,
            &build.new_version_entry.content_version,
            &build.new_version_entry.structure_version,
            TableTimestamp::At(build.new_version_entry.last_updated_at),
            build.new_version_entry.naming_version,
        );

        self.update_last_touch(&target).await.ok();

        let client = self.driver(data_source, false).await?;
        let load_cache = PreAggregationLoadCache::new(
            self.clone(),
            data_source.to_string(),
            build.request_id.clone(),
        );

        let external = build.pre_aggregation.external.unwrap_or(false);
        // An external build's target table lives in the external store, so the `tablesUsed`
        // key of the source schema is not the one that would have to be released.
        let drop_table_used_key = !external;

        let result = if external {
            // `preAggregation.readOnly` or a driver that refuses to write: the pre-aggregation
            // SQL is streamed out of the source instead of first landing in a temp table.
            let read_only = build.pre_aggregation.read_only.unwrap_or(false) || client.read_only();

            if read_only {
                self.refresh_read_only_external_strategy(
                    &build,
                    &target,
                    client.as_ref(),
                    &load_cache,
                )
                .await
            } else {
                self.refresh_write_strategy(&build, &target, client.as_ref(), &load_cache)
                    .await
            }
        } else {
            self.refresh_store_in_source_strategy(&build, &target, client.as_ref())
                .await
        };

        if result.is_err() {
            // Touch keys are unique per run and table, so a failed build would leave a large
            // number of them behind.
            self.remove_table_touched(&target).await.ok();

            if drop_table_used_key {
                self.remove_table_used(&target).await.ok();
            }
        }

        result
    }

    /// `refreshStoreInSourceStrategy` (`QO/PreAggregationLoader.ts:542-580`).
    async fn refresh_store_in_source_strategy(
        self: &Arc<Self>,
        build: &BuildRequest,
        target: &str,
        client: &dyn Driver,
    ) -> Result<(), OrchError> {
        let load_sql = build.pre_aggregation.load_sql.clone().ok_or_else(|| {
            OrchError::orchestration("Pre-aggregation description carries no loadSql")
        })?;

        let mut replacements = QueryCache::table_name_replacements(&build.pre_aggregation_tables);
        // The load SQL still names the pre-aggregation itself; it becomes the new table.
        replacements.push((build.pre_aggregation.table_name.clone(), target.to_string()));

        let query =
            QueryCache::replace_pre_aggregation_table_names_in_sql(&load_sql.sql, &replacements);
        let params: Vec<Value> = load_sql
            .params
            .iter()
            .map(|param| Value::String(param.clone()))
            .collect();
        let options = QueryOptions {
            request_id: build.request_id.clone(),
            ..Default::default()
        };

        self.log(
            "Executing Load Pre Aggregation SQL",
            json!({
                "targetTableName": target,
                "requestId": build.request_id,
                "values": load_sql.params,
            }),
        );

        let load = async {
            client
                .load_pre_aggregation_into_table(target, &query, &params, &options)
                .await?;
            self.create_indexes(build, target, client, &options).await?;

            Ok::<(), OrchError>(())
        }
        .await;

        // The orphan sweep runs whether the build succeeded or not.
        let sweep = self
            .drop_orphaned_tables(&build.pre_aggregation, target, client, false)
            .await;

        load?;
        sweep
    }

    // ------------------------------------------------------------------
    // External build strategies
    // ------------------------------------------------------------------

    fn query_options(&self, build: &BuildRequest) -> QueryOptions {
        QueryOptions {
            request_id: build.request_id.clone(),
            ..Default::default()
        }
    }

    /// `getUnloadOptions` (`:794-800`).
    fn unload_options(&self, build: &BuildRequest) -> UnloadOptions {
        UnloadOptions {
            // 64 MB, because the drivers' own default (16 MB for Snowflake) makes for a lot
            // of very small files.
            max_file_size: 64,
            query: None,
            request_id: build.request_id.clone(),
        }
    }

    /// `getStreamingOptions` (`:802-808`).
    fn streaming_options(&self, build: &BuildRequest) -> StreamOptions {
        StreamOptions {
            high_water_mark: 10000,
            request_id: build.request_id.clone(),
        }
    }

    fn download_table_options(
        pre_aggregation: &PreAggregationDescription,
        capabilities: &DriverCapabilities,
    ) -> DownloadTableOptions {
        DownloadTableOptions {
            csv_import: capabilities.csv_import,
            stream_import: capabilities.stream_import,
            stream_offset: pre_aggregation
                .stream_offset
                .as_ref()
                .is_some_and(|value| !matches!(value, Value::Null | Value::Bool(false))),
            output_column_types: pre_aggregation
                .output_column_types
                .clone()
                .and_then(|value| serde_json::from_value(value).ok()),
        }
    }

    /// `refreshWriteStrategy` (`:606-625`) — an external build against a data source this
    /// instance may write to.
    ///
    /// A driver that can unload a query straight to an export bucket needs no temp table;
    /// everything else first materializes the pre-aggregation in the source schema, reads it
    /// back and then drops it again.
    async fn refresh_write_strategy(
        self: &Arc<Self>,
        build: &BuildRequest,
        target: &str,
        client: &dyn Driver,
        load_cache: &PreAggregationLoadCache,
    ) -> Result<(), OrchError> {
        let capabilities = client.capabilities();
        let with_temp_table = !capabilities.unload_without_temp_table;
        // A streaming source's "temp table" is the stream itself and must survive the build.
        let drop_source_temp_table = !capabilities.streaming_source;

        if with_temp_table {
            client
                .create_schema_if_not_exists(&PreAggregationLoadCache::schema(
                    &build.pre_aggregation,
                ))
                .await?;

            // `prepareWriteStrategy` runs outside the cleanup: there is nothing to clean up
            // when the temp table was never created.
            self.load_into_temp_table(build, target, client).await?;
        }

        let load = async {
            let table_data = self
                .download_external_pre_aggregation(build, target, client, with_temp_table)
                .await?;

            self.upload_external_pre_aggregation(build, target, table_data, client, load_cache)
                .await
        }
        .await;

        let cleanup = self
            .cleanup_write_strategy(
                build,
                target,
                client,
                load_cache,
                with_temp_table,
                drop_source_temp_table,
            )
            .await;

        load?;
        cleanup
    }

    /// The temp table half of `prepareWriteStrategy` (`:686-727`): the same statement
    /// `refreshStoreInSourceStrategy` runs, except that the table it produces is temporary.
    async fn load_into_temp_table(
        &self,
        build: &BuildRequest,
        target: &str,
        client: &dyn Driver,
    ) -> Result<(), OrchError> {
        let load_sql = build.pre_aggregation.load_sql.clone().ok_or_else(|| {
            OrchError::orchestration("Pre-aggregation description carries no loadSql")
        })?;

        let mut replacements = QueryCache::table_name_replacements(&build.pre_aggregation_tables);
        replacements.push((build.pre_aggregation.table_name.clone(), target.to_string()));

        let query =
            QueryCache::replace_pre_aggregation_table_names_in_sql(&load_sql.sql, &replacements);
        let params = string_params(&load_sql.params);

        self.log(
            "Executing Load Pre Aggregation SQL",
            json!({
                "targetTableName": target,
                "requestId": build.request_id,
                "values": load_sql.params,
            }),
        );

        client
            .load_pre_aggregation_into_table(target, &query, &params, &self.query_options(build))
            .await?;

        Ok(())
    }

    /// `downloadExternalPreAggregation` (`:817-848`) plus the two `getTableData*` helpers
    /// (`:850-931`).
    async fn download_external_pre_aggregation(
        self: &Arc<Self>,
        build: &BuildRequest,
        target: &str,
        client: &dyn Driver,
        with_temp_table: bool,
    ) -> Result<DownloadedData, OrchError> {
        let external = self
            .driver(build.pre_aggregation.data_source(), true)
            .await?;
        let capabilities = external.capabilities();

        self.log(
            "Downloading external pre-aggregation",
            json!({ "targetTableName": target, "requestId": build.request_id }),
        );

        let unload_options = self.unload_options(build);

        if with_temp_table {
            if capabilities.csv_import && client.is_unload_supported(&unload_options).await? {
                return Ok(DownloadedData::Csv(
                    client.unload(target, &unload_options).await?,
                ));
            }

            if capabilities.stream_import {
                return Ok(DownloadedData::Stream(
                    client
                        .stream(
                            &format!("SELECT * FROM {target}"),
                            &[],
                            &self.streaming_options(build),
                        )
                        .await?,
                ));
            }

            return Ok(DownloadedData::Memory(
                client
                    .download_table(
                        target,
                        &Self::download_table_options(&build.pre_aggregation, &capabilities),
                    )
                    .await?,
            ));
        }

        let sql = build.pre_aggregation.sql.clone().ok_or_else(|| {
            OrchError::orchestration(
                "Pre-aggregation description carries no sql, which an unload without a temp \
                 table needs",
            )
        })?;
        let params = string_params(&sql.params);

        if capabilities.csv_import && client.is_unload_supported(&unload_options).await? {
            return Ok(DownloadedData::Csv(
                client
                    .unload(
                        target,
                        &UnloadOptions {
                            query: Some(UnloadQuery {
                                sql: sql.sql.clone(),
                                params: params.clone(),
                            }),
                            ..unload_options
                        },
                    )
                    .await?,
            ));
        }

        if capabilities.stream_import {
            return Ok(DownloadedData::Stream(
                client
                    .stream(&sql.sql, &params, &self.streaming_options(build))
                    .await?,
            ));
        }

        Ok(DownloadedData::Memory(
            client
                .query(&sql.sql, &params, &self.query_options(build))
                .await?,
        ))
    }

    /// `uploadExternalPreAggregation` (`:933-965`).
    async fn upload_external_pre_aggregation(
        self: &Arc<Self>,
        build: &BuildRequest,
        target: &str,
        table_data: DownloadedData,
        client: &dyn Driver,
        load_cache: &PreAggregationLoadCache,
    ) -> Result<(), OrchError> {
        let external = self
            .driver(build.pre_aggregation.data_source(), true)
            .await?;

        // A driver that reports no column types for what it produced is asked for them
        // separately, because the external store needs them to create the table.
        let mut columns: Vec<Column> = table_data
            .columns()
            .filter(|columns| !columns.is_empty())
            .map(<[Column]>::to_vec)
            .unwrap_or_default();

        if columns.is_empty() {
            columns = match &build.pre_aggregation.sql {
                Some(sql) if matches!(table_data, DownloadedData::Memory(_)) => {
                    client
                        .query_column_types(
                            &sql.sql,
                            &string_params(&sql.params),
                            &self.query_options(build),
                        )
                        .await?
                }
                _ => client.table_column_types(target).await?,
            };
        }

        // `upload_downloaded_table_with_indexes` takes every download shape:
        // Cube Store imports CSV files natively, and any other external store
        // falls back to collecting them. A `Memory` download with no columns
        // of its own carries the ones resolved above.
        let table_data = match table_data {
            DownloadedData::Memory(mut memory) if memory.columns.is_empty() => {
                memory.columns = columns.clone();
                DownloadedData::Memory(memory)
            }
            other => other,
        };

        self.log(
            "Uploading external pre-aggregation",
            json!({ "targetTableName": target, "requestId": build.request_id }),
        );

        external
            .upload_downloaded_table_with_indexes(
                target,
                &columns,
                table_data,
                &self.prepare_indexes_sql(build, target),
                build
                    .pre_aggregation
                    .unique_key_columns
                    .as_deref()
                    .unwrap_or(&[]),
                &ExternalCreateTableOptions {
                    aggregations_columns: build
                        .pre_aggregation
                        .aggregations_columns
                        .clone()
                        .unwrap_or_default(),
                    create_table_indexes: self.prepare_create_table_indexes(build),
                    seal_at: build.pre_aggregation.seal_at.clone(),
                },
            )
            .await?;

        self.log(
            "Uploading external pre-aggregation completed",
            json!({ "targetTableName": target, "requestId": build.request_id }),
        );

        load_cache.fetch_tables(&build.pre_aggregation).await?;
        self.drop_orphaned_tables(&build.pre_aggregation, target, external.as_ref(), true)
            .await
    }

    /// `cleanupWriteStrategy` (`:651-684`).
    async fn cleanup_write_strategy(
        self: &Arc<Self>,
        build: &BuildRequest,
        target: &str,
        client: &dyn Driver,
        load_cache: &PreAggregationLoadCache,
        with_temp_table: bool,
        drop_source_temp_table: bool,
    ) -> Result<(), OrchError> {
        if with_temp_table && drop_source_temp_table {
            let lock_key = format!(
                "lock:drop-temp-table:{}:{target}",
                build.pre_aggregation.data_source()
            );
            let this = self.clone();
            let logged = target.to_string();

            let ran = self
                .query_cache
                .cache_driver()
                .with_lock(
                    &lock_key,
                    60 * 5,
                    true,
                    async move {
                        this.log(
                            "Dropping source temp table",
                            json!({ "targetTableName": logged }),
                        );
                        Ok(())
                    }
                    .boxed(),
                )
                .await?;

            if ran {
                let schema = PreAggregationLoadCache::schema(&build.pre_aggregation);
                let tables = client.get_tables_query(&schema).await?;

                if tables
                    .iter()
                    .any(|table| format!("{schema}.{table}") == target)
                {
                    client
                        .drop_table(target, &self.query_options(build))
                        .await?;
                }
            }
        }

        load_cache.fetch_tables(&build.pre_aggregation).await?;
        // The source schema is swept too: the temp tables of earlier failed builds live there.
        self.drop_orphaned_tables(&build.pre_aggregation, target, client, false)
            .await
    }

    /// `refreshReadOnlyExternalStrategy` (`:729-792`) — the source may not be written to, so
    /// the pre-aggregation SQL itself is run and its result streamed into the external store.
    async fn refresh_read_only_external_strategy(
        self: &Arc<Self>,
        build: &BuildRequest,
        target: &str,
        client: &dyn Driver,
        load_cache: &PreAggregationLoadCache,
    ) -> Result<(), OrchError> {
        let sql = build.pre_aggregation.sql.clone().ok_or_else(|| {
            OrchError::orchestration(
                "Pre-aggregation description carries no sql, which a read only external build \
                 needs",
            )
        })?;
        let params = string_params(&sql.params);

        self.log(
            "Downloading external pre-aggregation via query",
            json!({
                "targetTableName": target,
                "requestId": build.request_id,
                "values": sql.params,
            }),
        );

        let external = self
            .driver(build.pre_aggregation.data_source(), true)
            .await?;
        let capabilities = external.capabilities();
        let unload_options = self.unload_options(build);

        let table_data =
            if capabilities.csv_import && client.is_unload_supported(&unload_options).await? {
                DownloadedData::Csv(
                    client
                        .unload_from_query(&sql.sql, &params, &unload_options)
                        .await?,
                )
            } else {
                let options = Self::download_table_options(&build.pre_aggregation, &capabilities);

                client
                    .download_query_results(
                        &sql.sql,
                        &params,
                        &DownloadQueryResultsOptions {
                            stream: self.streaming_options(build),
                            csv_import: options.csv_import,
                            stream_import: options.stream_import,
                            stream_offset: options.stream_offset,
                            output_column_types: options.output_column_types,
                        },
                    )
                    .await?
            };

        self.log(
            "Downloading external pre-aggregation via query completed",
            json!({ "targetTableName": target, "requestId": build.request_id }),
        );

        self.upload_external_pre_aggregation(build, target, table_data, client, load_cache)
            .await?;

        load_cache.fetch_tables(&build.pre_aggregation).await?;

        Ok(())
    }

    /// `prepareIndexesSql` (`:980-996`) — the index statements with every pre-aggregation
    /// name in them replaced by the physical table it became, including the index's own.
    fn prepare_indexes_sql(&self, build: &BuildRequest, target: &str) -> Vec<IndexSql> {
        let Some(Value::Array(indexes)) = build.pre_aggregation.indexes_sql.clone() else {
            return Vec::new();
        };

        let mut prepared = Vec::with_capacity(indexes.len());

        for index in indexes {
            let Some(index_name) = index.get("indexName").and_then(Value::as_str) else {
                continue;
            };
            let Some(Value::Array(sql)) = index.get("sql") else {
                continue;
            };
            let Some(query) = sql.first().and_then(Value::as_str) else {
                continue;
            };
            let params: Vec<Value> = sql
                .get(1)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();

            let index_target = self.index_target_table_name(build, index_name);

            let mut replacements =
                QueryCache::table_name_replacements(&build.pre_aggregation_tables);
            replacements.push((build.pre_aggregation.table_name.clone(), target.to_string()));
            replacements.push((index_name.to_string(), index_target));

            prepared.push(IndexSql {
                sql: QueryCache::replace_pre_aggregation_table_names_in_sql(query, &replacements),
                params,
            });
        }

        prepared
    }

    /// An index is versioned like the table it belongs to, so that dropping the table's
    /// orphans drops the index's too.
    fn index_target_table_name(&self, build: &BuildRequest, index_name: &str) -> String {
        target_table_name(
            index_name,
            &build.new_version_entry.content_version,
            &build.new_version_entry.structure_version,
            TableTimestamp::At(build.new_version_entry.last_updated_at),
            build.new_version_entry.naming_version,
        )
    }

    /// `prepareCreateTableIndexes` (`:998-1010`) — the indexes the external store creates as
    /// part of `CREATE TABLE`, rather than as separate statements.
    fn prepare_create_table_indexes(&self, build: &BuildRequest) -> Vec<CreateTableIndex> {
        let Some(Value::Array(indexes)) = build.pre_aggregation.create_table_indexes.clone() else {
            return Vec::new();
        };

        indexes
            .iter()
            .filter_map(|index| {
                let index_name = index.get("indexName").and_then(Value::as_str)?;

                Some(CreateTableIndex {
                    index_name: self.index_target_table_name(build, index_name),
                    type_: index
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    columns: index
                        .get("columns")
                        .and_then(Value::as_array)
                        .map(|columns| {
                            columns
                                .iter()
                                .filter_map(|column| {
                                    column.as_str().map(std::string::ToString::to_string)
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                })
            })
            .collect()
    }

    /// `createIndexes` (`:967-978`).
    async fn create_indexes(
        &self,
        build: &BuildRequest,
        target: &str,
        client: &dyn Driver,
        options: &QueryOptions,
    ) -> Result<(), OrchError> {
        for index in self.prepare_indexes_sql(build, target) {
            self.log(
                "Creating pre-aggregation index",
                json!({ "targetTableName": target, "requestId": build.request_id }),
            );

            client.query(&index.sql, &index.params, options).await?;
        }

        Ok(())
    }

    /// `dropOrphanedTables` (`QO/PreAggregationLoader.ts:1016-1083`).
    ///
    /// Runs under `lock:drop-orphaned-tables[-external|:<dataSource>]` with a five minute
    /// TTL, so only one node sweeps a schema at a time.
    pub async fn drop_orphaned_tables(
        self: &Arc<Self>,
        pre_aggregation: &PreAggregationDescription,
        just_created_table: &str,
        client: &dyn Driver,
        external: bool,
    ) -> Result<(), OrchError> {
        self.add_table_used(just_created_table).await?;

        let lock_key = if external {
            "lock:drop-orphaned-tables-external".to_string()
        } else {
            format!(
                "lock:drop-orphaned-tables:{}",
                pre_aggregation.data_source()
            )
        };

        let schema = pre_aggregation
            .pre_aggregations_schema
            .clone()
            .unwrap_or_else(|| pre_aggregation.schema().to_string());

        let actual_tables = client.get_tables_query(&schema).await?;
        let version_entries = tables_to_version_entries(
            &schema,
            &actual_tables
                .iter()
                .map(|table| TableCacheEntry::new(table.clone()))
                .collect::<Vec<_>>(),
        );

        // The newest entry per table name, and the newest per (table name, structure version)
        // while the structure is still within `structureVersionPersistTime`.
        let mut latest_per_table: Vec<&VersionEntry> = Vec::new();
        let mut seen_tables: HashSet<&str> = HashSet::new();
        let mut latest_per_structure: Vec<&VersionEntry> = Vec::new();
        let mut seen_structures: HashSet<String> = HashSet::new();
        let now = chrono::Utc::now().timestamp_millis();

        for entry in &version_entries {
            if seen_tables.insert(entry.table_name.as_str()) {
                latest_per_table.push(entry);
            }

            if now - entry.last_updated_at
                < self.options.structure_version_persist_time as i64 * 1000
                && seen_structures.insert(entry.structure_key())
            {
                latest_per_structure.push(entry);
            }
        }

        let refresh_end_reached = self.get_refresh_end_reached().await?.is_some();
        let mut to_save: HashSet<String> = self.tables_used().await?.into_iter().collect();

        if self.options.drop_pre_aggregations_without_touch && refresh_end_reached {
            to_save.extend(self.tables_touched().await?);
        } else {
            to_save.extend(
                latest_per_structure
                    .iter()
                    .chain(latest_per_table.iter())
                    .map(|entry| entry.target_table_name()),
            );
        }
        to_save.insert(just_created_table.to_string());

        let to_drop: Vec<String> = actual_tables
            .iter()
            .map(|table| format!("{schema}.{table}"))
            .filter(|table| !to_save.contains(table))
            .collect();

        let this = self.clone();
        let logged_to_drop = to_drop.clone();

        // `withLock` owns the whole sweep, not just the drops, so two nodes cannot both
        // decide what is orphaned from two different listings.
        let ran = self
            .query_cache
            .cache_driver()
            .with_lock(
                &lock_key,
                60 * 5,
                true,
                async move {
                    this.log(
                        "Dropping orphaned tables",
                        json!({ "external": external, "tablesToDrop": logged_to_drop }),
                    );
                    Ok(())
                }
                .boxed(),
            )
            .await?;

        if !ran {
            return Ok(());
        }

        for table in &to_drop {
            client.drop_table(table, &QueryOptions::default()).await?;
        }

        self.log(
            "Dropping orphaned tables completed",
            json!({ "external": external, "tablesToDrop": to_drop }),
        );

        Ok(())
    }

    // ------------------------------------------------------------------
    // loadAllPreAggregationsIfNeeded
    // ------------------------------------------------------------------

    /// `PreAggregations.loadAllPreAggregationsIfNeeded(queryBody)` (`:529-632`).
    ///
    /// The descriptions are loaded **sequentially**: a later one may reference the table a
    /// former one just produced.
    pub async fn load_all_pre_aggregations_if_needed(
        self: &Arc<Self>,
        query_body: &QueryBody,
    ) -> Result<LoadAllPreAggregationsResult, OrchError> {
        let mut loaded: Vec<PreAggTableToTempTable> = Vec::new();
        let mut values: Option<Vec<String>> = None;
        let last = query_body.pre_aggregations.len().saturating_sub(1);
        let mut load_caches: HashMap<String, Arc<PreAggregationLoadCache>> = HashMap::new();

        for (index, pre_aggregation) in query_body.pre_aggregations.iter().enumerate() {
            let cache_key = format!(
                "{}_{}",
                pre_aggregation.data_source(),
                pre_aggregation
                    .pre_aggregations_schema
                    .clone()
                    .unwrap_or_default()
            );
            let load_cache = load_caches
                .entry(cache_key)
                .or_insert_with(|| {
                    Arc::new(PreAggregationLoadCache::new(
                        self.clone(),
                        pre_aggregation.data_source().to_string(),
                        query_body.request_id.clone(),
                    ))
                })
                .clone();

            let loader = PreAggregationPartitionRangeLoader::new(
                self.clone(),
                pre_aggregation.clone(),
                loaded.clone(),
                load_cache,
                LoadOptions {
                    is_job: query_body.is_job,
                    wait_for_renew: query_body.cache_mode
                        == Some(crate::types::CacheMode::MustRevalidate),
                    // Only the last description may force a build: the earlier ones are its
                    // dependencies and forcing them would make every request wait on a
                    // rebuild of the whole chain.
                    force_build: index == last && query_body.force_build_pre_aggregations,
                    request_id: query_body.request_id.clone(),
                    external_refresh: self.options.external_refresh,
                },
            );

            if let Some(mut result) = loader.load_pre_aggregations().await? {
                result.pre_aggregation_id = pre_aggregation.pre_aggregation_id.clone();
                result.r#type = pre_aggregation.r#type.clone();
                result.data_source = Some(pre_aggregation.data_source().to_string());
                result.timezone = pre_aggregation.timezone.clone();

                if !result.is_multi_table_union {
                    self.add_table_used(&result.target_table_name).await?;
                }

                // The outer query may name the build range, which only this loader knows.
                if index == last {
                    if let Some(query_values) = &query_body.values {
                        values = loader
                            .replace_query_build_range_params(query_values)
                            .await?;
                    }
                }

                // One description serving several usages becomes one entry per usage, so the
                // SQL rewriter can point each `tableName<suffix>` at the partitions it needs.
                if result.usage_target_table_names.is_empty() {
                    loaded.push((pre_aggregation.table_name.clone(), result));
                } else {
                    let usages = std::mem::take(&mut result.usage_target_table_names);

                    for (suffix, target_table_name) in usages {
                        loaded.push((
                            format!("{}{suffix}", pre_aggregation.table_name),
                            LoadPreAggregationResult {
                                target_table_name,
                                ..result.clone()
                            },
                        ));
                    }
                }
            }
        }

        Ok(LoadAllPreAggregationsResult {
            tables: loaded,
            values,
        })
    }
}

/// `loadAllPreAggregationsIfNeeded`'s two outputs (`QO/PreAggregations.ts:628-631`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoadAllPreAggregationsResult {
    pub tables: Vec<PreAggTableToTempTable>,
    /// The query's own values with `BUILD_RANGE_START_LOCAL`/`BUILD_RANGE_END_LOCAL`
    /// resolved, when the last pre-aggregation named them.
    pub values: Option<Vec<String>>,
}

/// Driver parameters from the string list the data model emits.
fn string_params(params: &[String]) -> Vec<Value> {
    params.iter().cloned().map(Value::String).collect()
}

/// The payload of one queued build.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuildRequest {
    pre_aggregation: PreAggregationDescription,
    #[serde(default)]
    pre_aggregation_tables: Vec<PreAggTableToTempTable>,
    new_version_entry: VersionEntry,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    invalidation_keys: Vec<Value>,
}

/// The per request, per data source memo of `PreAggregationLoadCache`
/// (`QO/PreAggregationLoadCache.ts`).
pub struct PreAggregationLoadCache {
    pre_aggregations: Arc<PreAggregations>,
    data_source: String,
    request_id: Option<String>,
    tables: Mutex<HashMap<String, Vec<TableCacheEntry>>>,
    version_entries: Mutex<HashMap<String, VersionEntries>>,
    query_results: Mutex<HashMap<String, Vec<Value>>>,
}

/// `VersionEntriesObj` (`QO/PreAggregations.ts:98-103`), as the indices the loader reads.
#[derive(Clone, Debug, Default)]
pub struct VersionEntries {
    pub version_entries: Vec<VersionEntry>,
    by_content: HashMap<String, VersionEntry>,
    by_structure: HashMap<String, VersionEntry>,
    by_table_name: HashMap<String, VersionEntry>,
}

impl VersionEntries {
    fn index(version_entries: Vec<VersionEntry>) -> Self {
        let mut by_content = HashMap::new();
        let mut by_structure = HashMap::new();
        let mut by_table_name = HashMap::new();

        // The list is newest first, so the first entry of each key wins.
        for entry in &version_entries {
            by_content
                .entry(entry.content_key())
                .or_insert_with(|| entry.clone());
            by_structure
                .entry(entry.structure_key())
                .or_insert_with(|| entry.clone());
            by_table_name
                .entry(entry.table_name.clone())
                .or_insert_with(|| entry.clone());
        }

        Self {
            version_entries,
            by_content,
            by_structure,
            by_table_name,
        }
    }

    pub fn by_content(&self, table_name: &str, content_version: &str) -> Option<&VersionEntry> {
        self.by_content
            .get(&format!("{table_name}_{content_version}"))
    }

    pub fn by_structure(&self, table_name: &str, structure_version: &str) -> Option<&VersionEntry> {
        self.by_structure
            .get(&format!("{table_name}_{structure_version}"))
    }

    pub fn by_table_name(&self, table_name: &str) -> Option<&VersionEntry> {
        self.by_table_name.get(table_name)
    }
}

impl PreAggregationLoadCache {
    pub fn new(
        pre_aggregations: Arc<PreAggregations>,
        data_source: String,
        request_id: Option<String>,
    ) -> Self {
        Self {
            pre_aggregations,
            data_source,
            request_id,
            tables: Mutex::new(HashMap::new()),
            version_entries: Mutex::new(HashMap::new()),
            query_results: Mutex::new(HashMap::new()),
        }
    }

    /// `tablesCachePrefixKey` (`QO/PreAggregationLoadCache.ts:108-110`).
    pub fn tables_cache_prefix_key(&self, pre_aggregation: &PreAggregationDescription) -> String {
        self.pre_aggregations.query_cache.get_key(
            TABLES_CACHE,
            &format!(
                "{}{}{}",
                pre_aggregation.data_source(),
                pre_aggregation
                    .pre_aggregations_schema
                    .clone()
                    .unwrap_or_default(),
                if pre_aggregation.external.unwrap_or(false) {
                    "_EXT"
                } else {
                    ""
                }
            ),
        )
    }

    fn schema(pre_aggregation: &PreAggregationDescription) -> String {
        pre_aggregation
            .pre_aggregations_schema
            .clone()
            .unwrap_or_else(|| pre_aggregation.schema().to_string())
    }

    /// `fetchTables` — the listing goes through the cache driver so that concurrent loads of
    /// one schema do not each hit the data source.
    pub async fn fetch_tables(
        &self,
        pre_aggregation: &PreAggregationDescription,
    ) -> Result<Vec<TableCacheEntry>, OrchError> {
        let external = pre_aggregation.external.unwrap_or(false);
        let client = self
            .pre_aggregations
            .driver(&self.data_source, external)
            .await?;
        let tables: Vec<TableCacheEntry> = client
            .get_tables_query(&Self::schema(pre_aggregation))
            .await?
            .into_iter()
            .map(TableCacheEntry::new)
            .collect();

        self.pre_aggregations
            .query_cache
            .cache_driver()
            .set(
                &self.tables_cache_prefix_key(pre_aggregation),
                json!(tables
                    .iter()
                    .map(|table| table.table_name.clone())
                    .collect::<Vec<_>>()),
                self.pre_aggregations
                    .options
                    .pre_aggregations_schema_cache_expire,
            )
            .await?;

        self.tables.lock().unwrap().insert(
            self.tables_cache_prefix_key(pre_aggregation),
            tables.clone(),
        );

        Ok(tables)
    }

    async fn get_tables_query(
        &self,
        pre_aggregation: &PreAggregationDescription,
    ) -> Result<Vec<TableCacheEntry>, OrchError> {
        let key = self.tables_cache_prefix_key(pre_aggregation);

        if let Some(tables) = self.tables.lock().unwrap().get(&key) {
            return Ok(tables.clone());
        }

        let cached = self
            .pre_aggregations
            .query_cache
            .cache_driver()
            .get(&key)
            .await?;

        let tables = match cached {
            Some(Value::Array(names)) => names
                .into_iter()
                .filter_map(|name| name.as_str().map(TableCacheEntry::new))
                .collect(),
            _ => return self.fetch_tables(pre_aggregation).await,
        };

        self.tables.lock().unwrap().insert(key, tables);

        Ok(self
            .tables
            .lock()
            .unwrap()
            .get(&self.tables_cache_prefix_key(pre_aggregation))
            .cloned()
            .unwrap_or_default())
    }

    /// `getVersionEntries` (`:179-191`), memoized per table cache prefix.
    pub async fn get_version_entries(
        &self,
        pre_aggregation: &PreAggregationDescription,
    ) -> Result<VersionEntries, OrchError> {
        let key = self.tables_cache_prefix_key(pre_aggregation);

        if let Some(entries) = self.version_entries.lock().unwrap().get(&key) {
            return Ok(entries.clone());
        }

        let tables = self.get_tables_query(pre_aggregation).await?;
        let entries = VersionEntries::index(tables_to_version_entries(
            &Self::schema(pre_aggregation),
            &tables,
        ));

        self.version_entries
            .lock()
            .unwrap()
            .insert(key, entries.clone());

        Ok(entries)
    }

    /// Drops the memo so the next read sees what a build just produced (`reset`).
    pub async fn reset(
        &self,
        pre_aggregation: &PreAggregationDescription,
    ) -> Result<(), OrchError> {
        self.fetch_tables(pre_aggregation).await?;
        self.version_entries.lock().unwrap().clear();

        Ok(())
    }

    /// `hasKeyQueryResult` (`:212-214`) — whether this key was already resolved in this request.
    pub fn has_key_query_result(&self, key_query: &QueryWithParams) -> bool {
        let memo_key = self
            .pre_aggregations
            .query_cache
            .refresh_key_cache_key(key_query, &self.data_source);

        self.query_results.lock().unwrap().contains_key(&memo_key)
    }

    /// `keyQueryResult` (`:193-210`) — refresh keys are cached for an hour and memoized per
    /// load cache, so one key shared by several pre-aggregations resolves once.
    pub async fn key_query_result(
        &self,
        key_query: &QueryWithParams,
        wait_for_renew: bool,
        priority: i32,
    ) -> Result<Vec<Value>, OrchError> {
        let memo_key = self
            .pre_aggregations
            .query_cache
            .refresh_key_cache_key(key_query, &self.data_source);

        if let Some(result) = self.query_results.lock().unwrap().get(&memo_key) {
            return Ok(result.clone());
        }

        let result = self
            .pre_aggregations
            .query_cache
            .cache_refresh_key_result(
                key_query,
                60 * 60,
                &RefreshKeyCacheOptions {
                    wait_for_renew,
                    priority: Some(priority),
                    request_id: self.request_id.clone(),
                    data_source: self.data_source.clone(),
                },
            )
            .await?;

        self.query_results
            .lock()
            .unwrap()
            .insert(memo_key, result.clone());

        Ok(result)
    }
}

/// `PreAggregationLoader` (`QO/PreAggregationLoader.ts`) for one description.
pub struct PreAggregationLoader {
    pre_aggregations: Arc<PreAggregations>,
    pre_aggregation: PreAggregationDescription,
    pre_aggregation_tables: Vec<PreAggTableToTempTable>,
    load_cache: Arc<PreAggregationLoadCache>,
    options: LoadOptions,
}

impl PreAggregationLoader {
    pub fn new(
        pre_aggregations: Arc<PreAggregations>,
        pre_aggregation: PreAggregationDescription,
        pre_aggregation_tables: Vec<PreAggTableToTempTable>,
        load_cache: Arc<PreAggregationLoadCache>,
        options: LoadOptions,
    ) -> Self {
        Self {
            pre_aggregations,
            pre_aggregation,
            pre_aggregation_tables,
            load_cache,
            // `externalRefresh` owns the "partition is not built yet" handling and may not
            // enqueue a build, so it silently degrades `waitForRenew`.
            options: LoadOptions {
                wait_for_renew: options.wait_for_renew && !options.external_refresh,
                ..options
            },
        }
    }

    fn priority(&self, default: QueuePriority) -> i32 {
        self.pre_aggregation.priority.unwrap_or(default.value())
    }

    fn serve(&self, entry: &VersionEntry) -> LoadPreAggregationResult {
        LoadPreAggregationResult {
            target_table_name: entry.target_table_name(),
            refresh_key_values: Some(Vec::new()),
            last_updated_at: Some(entry.last_updated_at),
            build_range_end: entry.build_range_end.clone(),
            ..Default::default()
        }
    }

    /// `loadPreAggregation(throwOnMissingPartition)` (`:132-210`).
    pub async fn load_pre_aggregation(
        &self,
        throw_on_missing_partition: bool,
    ) -> Result<Option<LoadPreAggregationResult>, OrchError> {
        let invalidation_keys_loaded = || {
            self.pre_aggregation
                .invalidate_key_queries
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .all(|key_query| self.load_cache.has_key_query_result(key_query))
        };

        if self.options.is_job
            || (!self.options.external_refresh
                && (self.options.wait_for_renew || invalidation_keys_loaded()))
        {
            let mut result = self.load_pre_aggregation_with_keys().await?;
            result.refresh_key_values = Some(self.get_invalidation_key_values().await?);

            return Ok(Some(result));
        }

        // Serve whatever version already exists rather than making this request wait.
        let structure_version = get_structure_version(&self.pre_aggregation);
        let version_entries = self
            .load_cache
            .get_version_entries(&self.pre_aggregation)
            .await?;
        let by_structure =
            version_entries.by_structure(&self.pre_aggregation.table_name, &structure_version);

        if self.options.external_refresh {
            return match by_structure {
                Some(entry) => Ok(Some(self.serve(entry))),
                None if throw_on_missing_partition => Err(OrchError::orchestration(
                    no_pre_aggregation_partitions_built_message(std::slice::from_ref(
                        &self.pre_aggregation,
                    )),
                )),
                None => Ok(None),
            };
        }

        match by_structure {
            // Trigger an asynchronous load but immediately return the data already there.
            Some(entry) => {
                let result = self.serve(entry);
                self.load_pre_aggregation_with_keys_in_background();

                Ok(Some(result))
            }
            // No rollup has been built yet — build it as part of answering this request.
            None => Ok(Some(self.load_pre_aggregation_with_keys().await?)),
        }
    }

    fn load_pre_aggregation_with_keys_in_background(&self) {
        // A detached copy of the loader: the background build must not borrow the request.
        let loader = PreAggregationLoader {
            pre_aggregations: self.pre_aggregations.clone(),
            pre_aggregation: self.pre_aggregation.clone(),
            pre_aggregation_tables: self.pre_aggregation_tables.clone(),
            load_cache: self.load_cache.clone(),
            options: self.options.clone(),
        };

        tokio::spawn(async move {
            if let Err(error) = loader.load_pre_aggregation_with_keys().await {
                if !error.is_continue_wait() {
                    loader.pre_aggregations.log(
                        "Error loading pre-aggregation",
                        json!({
                            "error": error.to_string(),
                            "preAggregation": loader.pre_aggregation.table_name,
                            "requestId": loader.options.request_id,
                        }),
                    );
                }
            }
        });
    }

    async fn get_invalidation_key_values(&self) -> Result<Vec<Value>, OrchError> {
        self.resolve_key_queries(
            self.pre_aggregation
                .invalidate_key_queries
                .as_deref()
                .unwrap_or(&[]),
        )
        .await
    }

    async fn get_partition_invalidation_key_values(&self) -> Result<Vec<Value>, OrchError> {
        match &self.pre_aggregation.partition_invalidate_key_queries {
            Some(queries) => self.resolve_key_queries(queries).await,
            None => self.get_invalidation_key_values().await,
        }
    }

    async fn resolve_key_queries(
        &self,
        queries: &[QueryWithParams],
    ) -> Result<Vec<Value>, OrchError> {
        let mut values = Vec::with_capacity(queries.len());

        for query in queries {
            values.push(Value::Array(
                self.load_cache
                    .key_query_result(
                        query,
                        self.options.wait_for_renew,
                        self.priority(QueuePriority::Interactive),
                    )
                    .await?,
            ));
        }

        Ok(values)
    }

    /// `preAggregationQueryKey(invalidationKeys)` (`:455-459`).
    fn pre_aggregation_query_key(&self, invalidation_keys: &[Value]) -> CacheKey {
        let load_sql = self
            .pre_aggregation
            .load_sql
            .as_ref()
            .map(QueryWithParams::to_key_value)
            .unwrap_or(KeyValue::Null);
        let keys = KeyValue::Array(invalidation_keys.iter().map(KeyValue::from).collect());

        match &self.pre_aggregation.indexes_sql {
            Some(indexes_sql)
                if indexes_sql
                    .as_array()
                    .map(|a| !a.is_empty())
                    .unwrap_or(false) =>
            {
                CacheKey::list(vec![load_sql, KeyValue::from(indexes_sql), keys])
            }
            _ => CacheKey::list(vec![load_sql, keys]),
        }
    }

    async fn execute_in_queue(
        &self,
        invalidation_keys: &[Value],
        priority: i32,
        new_version_entry: &VersionEntry,
    ) -> Result<(), OrchError> {
        let queue = self
            .pre_aggregations
            .get_queue(self.pre_aggregation.data_source());

        let payload = serde_json::to_value(BuildRequest {
            pre_aggregation: self.pre_aggregation.clone(),
            pre_aggregation_tables: self.pre_aggregation_tables.clone(),
            new_version_entry: new_version_entry.clone(),
            request_id: self.options.request_id.clone(),
            invalidation_keys: invalidation_keys.to_vec(),
        })
        .map_err(|e| OrchError::orchestration(e.to_string()))?;

        queue
            .execute_in_queue(
                "query",
                self.pre_aggregation_query_key(invalidation_keys),
                payload,
                priority,
                ExecuteInQueueOptions {
                    stage_query_key: Some(PreAggregations::pre_aggregation_query_cache_key(
                        &self.pre_aggregation,
                    )),
                    request_id: self.options.request_id.clone(),
                    span_id: None,
                },
            )
            .await?;

        Ok(())
    }

    async fn most_recent_result(
        &self,
        content_version: &str,
    ) -> Result<LoadPreAggregationResult, OrchError> {
        self.load_cache.reset(&self.pre_aggregation).await?;

        let entries = self
            .load_cache
            .get_version_entries(&self.pre_aggregation)
            .await?;
        let entry = entries
            .by_content(&self.pre_aggregation.table_name, content_version)
            .ok_or_else(|| {
                OrchError::orchestration(format!(
                    "Pre-aggregation table is not found for {} after it was successfully created",
                    self.pre_aggregation.table_name
                ))
            })?;

        let result = self.serve(entry);
        self.pre_aggregations
            .update_last_touch(&result.target_table_name)
            .await
            .ok();

        Ok(result)
    }

    /// `loadPreAggregationWithKeys()` (`:212-366`).
    pub async fn load_pre_aggregation_with_keys(
        &self,
    ) -> Result<LoadPreAggregationResult, OrchError> {
        let invalidation_keys = self.get_partition_invalidation_key_values().await?;
        let content_version = content_version(
            &self.pre_aggregation,
            &KeyValue::Array(invalidation_keys.iter().map(KeyValue::from).collect()),
        );
        let structure_version = get_structure_version(&self.pre_aggregation);
        let version_entries = self
            .load_cache
            .get_version_entries(&self.pre_aggregation)
            .await?;

        if !self.options.force_build {
            if let Some(entry) =
                version_entries.by_content(&self.pre_aggregation.table_name, &content_version)
            {
                let result = self.serve(entry);
                self.pre_aggregations
                    .update_last_touch(&result.target_table_name)
                    .await
                    .ok();

                return Ok(result);
            }
        }

        if !self.options.wait_for_renew && !self.options.force_build {
            if let Some(entry) =
                version_entries.by_structure(&self.pre_aggregation.table_name, &structure_version)
            {
                let result = self.serve(entry);
                self.pre_aggregations
                    .update_last_touch(&result.target_table_name)
                    .await
                    .ok();

                return Ok(result);
            }
        }

        let external = self.pre_aggregation.external.unwrap_or(false);
        let client = self
            .pre_aggregations
            .driver(self.pre_aggregation.data_source(), external)
            .await?;

        if version_entries.version_entries.is_empty() {
            client
                .create_schema_if_not_exists(&PreAggregationLoadCache::schema(
                    &self.pre_aggregation,
                ))
                .await?;
        }

        // Find the structure version before invalidating anything.
        let version_entry = version_entries
            .by_structure(&self.pre_aggregation.table_name, &structure_version)
            .or_else(|| version_entries.by_table_name(&self.pre_aggregation.table_name))
            .cloned();

        let new_version_entry = VersionEntry {
            table_name: self.pre_aggregation.table_name.clone(),
            structure_version: structure_version.clone(),
            content_version: content_version.clone(),
            last_updated_at: client.now_timestamp(),
            build_range_end: None,
            naming_version: Some(2),
        };

        if self.options.force_build {
            self.pre_aggregations.log(
                "Force build pre-aggregation",
                json!({ "preAggregation": self.pre_aggregation.table_name,
                        "requestId": self.options.request_id }),
            );

            self.execute_in_queue(
                &invalidation_keys,
                self.priority(QueuePriority::Interactive),
                &new_version_entry,
            )
            .await?;

            return self.most_recent_result(&content_version).await;
        }

        match version_entry {
            Some(entry) => {
                if entry.structure_version != new_version_entry.structure_version {
                    self.pre_aggregations.log(
                        "Invalidating pre-aggregation structure",
                        json!({ "preAggregation": self.pre_aggregation.table_name,
                                "requestId": self.options.request_id }),
                    );
                    self.execute_in_queue(
                        &invalidation_keys,
                        self.priority(QueuePriority::Interactive),
                        &new_version_entry,
                    )
                    .await?;

                    return self.most_recent_result(&content_version).await;
                }

                if entry.content_version != new_version_entry.content_version {
                    if self.options.wait_for_renew {
                        self.pre_aggregations.log(
                            "Waiting for pre-aggregation renew",
                            json!({ "preAggregation": self.pre_aggregation.table_name,
                                    "requestId": self.options.request_id }),
                        );
                        self.execute_in_queue(
                            &invalidation_keys,
                            self.priority(QueuePriority::Background),
                            &new_version_entry,
                        )
                        .await?;

                        return self.most_recent_result(&content_version).await;
                    }

                    self.schedule_refresh(&invalidation_keys, &new_version_entry);
                }

                let result = self.serve(&entry);
                self.pre_aggregations
                    .update_last_touch(&result.target_table_name)
                    .await
                    .ok();

                Ok(result)
            }
            None => {
                self.pre_aggregations.log(
                    "Creating pre-aggregation from scratch",
                    json!({ "preAggregation": self.pre_aggregation.table_name,
                            "requestId": self.options.request_id }),
                );
                self.execute_in_queue(
                    &invalidation_keys,
                    self.priority(QueuePriority::Interactive),
                    &new_version_entry,
                )
                .await?;

                self.most_recent_result(&content_version).await
            }
        }
    }

    /// `scheduleRefresh` (`:415-429`) — a detached background build.
    fn schedule_refresh(&self, invalidation_keys: &[Value], new_version_entry: &VersionEntry) {
        self.pre_aggregations.log(
            "Refreshing pre-aggregation content",
            json!({ "preAggregation": self.pre_aggregation.table_name,
                    "requestId": self.options.request_id }),
        );

        let loader = PreAggregationLoader {
            pre_aggregations: self.pre_aggregations.clone(),
            pre_aggregation: self.pre_aggregation.clone(),
            pre_aggregation_tables: self.pre_aggregation_tables.clone(),
            load_cache: self.load_cache.clone(),
            options: self.options.clone(),
        };
        let invalidation_keys = invalidation_keys.to_vec();
        let new_version_entry = new_version_entry.clone();

        tokio::spawn(async move {
            let priority = loader.priority(QueuePriority::Background);

            if let Err(error) = loader
                .execute_in_queue(&invalidation_keys, priority, &new_version_entry)
                .await
            {
                if !error.is_continue_wait() {
                    loader.pre_aggregations.log(
                        "Error refreshing pre-aggregation",
                        json!({
                            "error": error.to_string(),
                            "preAggregation": loader.pre_aggregation.table_name,
                            "requestId": loader.options.request_id,
                        }),
                    );
                }
            }
        });
    }
}
