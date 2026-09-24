//! `QueryOrchestrator` — `fetchQuery` end to end.
//!
//! Port of `QO/QueryOrchestrator.ts`.

use std::{collections::BTreeMap, sync::Arc};

use cubequeue::{QueryStage, QueryStream};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    cache::{CachedQueryOutcome, DriverFactory, LoggerFn, QueryCache, QueryCacheOptions},
    error::OrchError,
    preaggs::{get_last_updated_at_timestamp, PreAggregations, PreAggregationsOptions},
    types::{PreAggTableToTempTable, QueryBody, UsedPreAggregation},
};

/// The result `fetchQuery` hands to `OrchestratorApi` (`QO/QueryOrchestrator.ts:284-290`
/// plus the two fields `SC/OrchestratorApi.ts:118-119` stamps on).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchQueryResult {
    pub data: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_key_values: Option<Vec<Value>>,
    /// Epoch milliseconds; the gateway renders it as an ISO string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_refresh_time: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external: Option<bool>,
    pub used_pre_aggregations: BTreeMap<String, UsedPreAggregation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub db_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ext_db_type: Option<String>,
    /// Only set on the continue-wait/stale path (`SC/OrchestratorApi.ts:151-154`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub slow_query: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

/// What `fetchQuery` returns, which depends on the request
/// (`QO/QueryOrchestrator.ts:252-266`).
#[derive(Clone, Debug, PartialEq)]
pub enum FetchQueryOutcome {
    /// A query with a `query` — the ordinary `/v1/load` case.
    Result(Box<FetchQueryResult>),
    /// A persistent query: the rows travel on the stream, which `fetchQuery` hands back
    /// untouched (`:279-282`). Pre-aggregations are loaded and the queue is used the same
    /// way, but nothing about the result is known here, `lastRefreshTime` included.
    Stream(QueryStream),
    /// A build only request: `{ usedPreAggregations, lastRefreshTime }`.
    BuildOnly {
        used_pre_aggregations: BTreeMap<String, UsedPreAggregation>,
        last_refresh_time: Option<i64>,
    },
    /// A build job (`/cubejs-system/v1/pre-aggregations/jobs`).
    Job(Vec<Value>),
}

impl FetchQueryOutcome {
    pub fn into_result(self) -> Option<FetchQueryResult> {
        match self {
            FetchQueryOutcome::Result(result) => Some(*result),
            _ => None,
        }
    }

    pub fn into_stream(self) -> Option<QueryStream> {
        match self {
            FetchQueryOutcome::Stream(stream) => Some(stream),
            _ => None,
        }
    }
}

/// `QueryOrchestratorOptions` (`:31-38`).
#[derive(Clone, Debug, Default)]
pub struct QueryOrchestratorOptions {
    /// `CUBEJS_ROLLUP_ONLY`: refuse anything a pre-aggregation does not serve.
    pub rollup_only_mode: bool,
    pub query_cache_options: QueryCacheOptions,
    pub pre_aggregations_options: PreAggregationsOptions,
}

pub struct QueryOrchestrator {
    query_cache: Arc<QueryCache>,
    pre_aggregations: Arc<PreAggregations>,
    rollup_only_mode: bool,
}

impl std::fmt::Debug for QueryOrchestrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryOrchestrator")
            .field("rollup_only_mode", &self.rollup_only_mode)
            .finish_non_exhaustive()
    }
}

/// The message `rollupOnlyMode` refuses a query with (`:240-245`).
pub const ROLLUP_ONLY_MESSAGE: &str =
    "No pre-aggregation table has been built for this query yet. \
                                       Please check your refresh worker configuration if it \
                                       persists.";

impl QueryOrchestrator {
    pub fn new(
        cache_prefix: impl Into<String>,
        driver_factory: DriverFactory,
        external_driver_factory: Option<DriverFactory>,
        logger: LoggerFn,
        options: QueryOrchestratorOptions,
    ) -> Arc<Self> {
        let cache_prefix = cache_prefix.into();

        let mut cache_builder = QueryCache::builder(cache_prefix.clone(), driver_factory.clone())
            .logger(logger.clone())
            .options(options.query_cache_options.clone());

        if let Some(external) = external_driver_factory.clone() {
            cache_builder = cache_builder.external_driver_factory(external);
        }

        let query_cache = cache_builder.build();
        let pre_aggregations = PreAggregations::new(
            cache_prefix,
            driver_factory,
            external_driver_factory,
            logger,
            query_cache.clone(),
            options.pre_aggregations_options,
        );

        Arc::new(Self {
            query_cache,
            pre_aggregations,
            rollup_only_mode: options.rollup_only_mode,
        })
    }

    /// Builds an orchestrator around already constructed parts, for tests and for a server
    /// that wants to share one cache driver between several orchestrators.
    pub fn from_parts(
        query_cache: Arc<QueryCache>,
        pre_aggregations: Arc<PreAggregations>,
        rollup_only_mode: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            query_cache,
            pre_aggregations,
            rollup_only_mode,
        })
    }

    pub fn query_cache(&self) -> &Arc<QueryCache> {
        &self.query_cache
    }

    pub fn pre_aggregations(&self) -> &Arc<PreAggregations> {
        &self.pre_aggregations
    }

    /// `usedPreAggregations` (`:225-238`).
    fn used_pre_aggregations(
        pre_aggregation_tables: &[PreAggTableToTempTable],
    ) -> BTreeMap<String, UsedPreAggregation> {
        pre_aggregation_tables
            .iter()
            .map(|(table_name, temp_table)| {
                (table_name.clone(), UsedPreAggregation::from(temp_table))
            })
            .collect()
    }

    /// `QueryOrchestrator.fetchQuery(queryBody)` (`:212-291`).
    pub async fn fetch_query(
        &self,
        query_body: &QueryBody,
    ) -> Result<FetchQueryOutcome, OrchError> {
        let loaded = self
            .pre_aggregations
            .load_all_pre_aggregations_if_needed(query_body)
            .await?;
        let pre_aggregation_tables = loaded.tables;

        // `BUILD_RANGE_START_LOCAL`/`BUILD_RANGE_END_LOCAL` in the query's own values are
        // only known once the build range has been read, so the body the cache key and the
        // driver see is the rewritten one.
        let rewritten;
        let query_body = match loaded.values {
            Some(values) => {
                rewritten = QueryBody {
                    values: Some(values),
                    ..query_body.clone()
                };
                &rewritten
            }
            None => query_body,
        };

        let used_pre_aggregations = Self::used_pre_aggregations(&pre_aggregation_tables);

        if self.rollup_only_mode && used_pre_aggregations.is_empty() {
            return Err(OrchError::orchestration(ROLLUP_ONLY_MESSAGE));
        }

        let last_refresh_timestamp = get_last_updated_at_timestamp(
            &pre_aggregation_tables
                .iter()
                .map(|(_, temp_table)| temp_table.last_updated_at)
                .collect::<Vec<_>>(),
        );

        if query_body.query.is_none() {
            if query_body.is_job {
                return Ok(FetchQueryOutcome::Job(
                    pre_aggregation_tables
                        .iter()
                        .map(|(table_name, temp_table)| {
                            let mut value = serde_json::to_value(temp_table)
                                .unwrap_or(Value::Object(Default::default()));

                            if let Some(object) = value.as_object_mut() {
                                object.insert(
                                    "preAggregation".to_string(),
                                    Value::String(
                                        temp_table
                                            .pre_aggregation_id
                                            .clone()
                                            .or_else(|| {
                                                query_body
                                                    .pre_aggregations
                                                    .first()
                                                    .and_then(|p| p.pre_aggregation_id.clone())
                                            })
                                            .unwrap_or_default(),
                                    ),
                                );
                                object.insert(
                                    "tableName".to_string(),
                                    Value::String(table_name.clone()),
                                );
                            }

                            value
                        })
                        .collect(),
                ));
            }

            return Ok(FetchQueryOutcome::BuildOnly {
                used_pre_aggregations,
                last_refresh_time: last_refresh_timestamp,
            });
        }

        let result = match self
            .query_cache
            .cached_query_result(query_body, &pre_aggregation_tables)
            .await?
        {
            CachedQueryOutcome::Result(result) => result,
            // `if (result instanceof QueryStream) return result`.
            CachedQueryOutcome::Stream(stream) => return Ok(FetchQueryOutcome::Stream(stream)),
        };

        // The result is only as fresh as the oldest thing behind it.
        let last_refresh_time =
            get_last_updated_at_timestamp(&[last_refresh_timestamp, result.last_refresh_time]);

        Ok(FetchQueryOutcome::Result(Box::new(FetchQueryResult {
            data: result.data,
            refresh_key_values: result.refresh_key_values,
            last_refresh_time,
            data_source: query_body.data_source.clone(),
            external: query_body.external,
            used_pre_aggregations,
            db_type: None,
            ext_db_type: None,
            slow_query: false,
            total: None,
        })))
    }

    /// `QueryOrchestrator.loadRefreshKeys(query)`.
    pub async fn load_refresh_keys(&self, query_body: &QueryBody) -> Result<Vec<Value>, OrchError> {
        self.query_cache
            .load_refresh_keys(
                query_body.cache_key_queries(),
                query_body.expire_secs(),
                &crate::cache::LoadRefreshKeyOptions {
                    request_id: query_body.request_id.clone(),
                    data_source: query_body.data_source().to_string(),
                    ..Default::default()
                },
            )
            .await
    }

    /// `QueryOrchestrator.queryStage(queryBody)` (`:297-343`) — what a continue-wait
    /// response tells the client it is waiting for.
    pub async fn query_stage(
        &self,
        query_body: &QueryBody,
    ) -> Result<Option<QueryStage>, OrchError> {
        let total = query_body.pre_aggregations.len();

        for (index, pre_aggregation) in query_body.pre_aggregations.iter().enumerate() {
            let queue = self
                .pre_aggregations
                .get_queue(pre_aggregation.data_source());
            let stage = queue
                .query_stage(
                    &PreAggregations::pre_aggregation_query_cache_key(pre_aggregation),
                    Some(10),
                    None,
                )
                .await?;

            if stage.is_none() {
                continue;
            }

            // The first pending pre-aggregation is the one the client is waiting for. Its own
            // queue position is reported without the priority filter.
            let stage = queue
                .query_stage(
                    &PreAggregations::pre_aggregation_query_cache_key(pre_aggregation),
                    None,
                    None,
                )
                .await?;

            let Some(stage) = stage else {
                return Ok(None);
            };

            let message = format!("Building pre-aggregation {}/{}", index + 1, total);

            return Ok(Some(QueryStage {
                stage: if stage.stage.contains("queue") {
                    format!("{message}: {}", stage.stage)
                } else {
                    message
                },
                time_elapsed: stage.time_elapsed,
            }));
        }

        let queue = self.query_cache.get_queue(query_body.data_source());

        queue
            .query_stage(&QueryCache::query_body_cache_key(query_body), None, None)
            .await
            .map_err(OrchError::from)
    }

    /// `QueryOrchestrator.resultFromCacheIfExists(queryBody)`.
    pub async fn result_from_cache_if_exists(
        &self,
        query_body: &QueryBody,
    ) -> Result<Option<FetchQueryResult>, OrchError> {
        Ok(self
            .query_cache
            .result_from_cache_if_exists(query_body)
            .await?
            .map(|cached| FetchQueryResult {
                data: cached.data,
                last_refresh_time: cached.last_refresh_time,
                data_source: query_body.data_source.clone(),
                external: query_body.external,
                ..Default::default()
            }))
    }

    pub async fn test_connections(&self) -> Result<(), OrchError> {
        self.query_cache.test_connection().await
    }
}
