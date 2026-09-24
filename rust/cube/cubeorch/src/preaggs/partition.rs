//! Partitioned pre-aggregations.
//!
//! Port of `QO/PreAggregationPartitionRangeLoader.ts`: the build range and its cached range
//! queries, the intersection with the query's own date range, the time series that turns
//! that into partitions, the per partition description (`FROM_PARTITION_RANGE` /
//! `TO_PARTITION_RANGE` substitution, the incremental renewal threshold and `sealAt`), and
//! the union of the partition tables that the outer query finally reads.
//!
//! Not ported, and reported rather than silently degraded:
//!
//! * **lambda rollups** (`rollupLambdaId`, `downloadLambdaTable`, `LAMBDA_TABLE_PREFIX`) —
//!   they need `renewQuery`'s CSV mode and inline tables, neither of which the `Driver`
//!   trait exposes yet. A description carrying `rollupLambdaId` is refused.
//! * `compilerCacheFn` — a memo around the pure `timeSeries`/description construction. It
//!   only saves work, so it is simply not wired here.

use std::{collections::BTreeMap, sync::Arc};

use cubecache::{CacheKey, KeyValue, QueryCacheKeyInput};
use cubequeue::QueuePriority;
use serde_json::{json, Value};

use crate::{
    cache::{CacheQueryResultOptions, QueryCache},
    error::OrchError,
    preaggs::{
        no_pre_aggregation_partitions_built_message, partition_table_name, LoadOptions,
        PreAggregationLoadCache, PreAggregationLoader, PreAggregations,
    },
    time::{
        add_seconds_to_local_timestamp, local_timestamp_to_utc, now_in_time_zone,
        parse_utc_into_local_date, time_series, time_series_boundaries, QueryDateRange,
        BUILD_RANGE_END_LOCAL, BUILD_RANGE_START_LOCAL, DEFAULT_TS_FORMAT, FROM_PARTITION_RANGE,
        TO_PARTITION_RANGE,
    },
    types::{
        LoadPreAggregationResult, PreAggTableToTempTable, PreAggregationDescription,
        QueryWithParams,
    },
};

use super::version::get_last_updated_at_timestamp;

/// One partition's bounds in both the pre-aggregation's timezone and UTC, converted once so
/// that every parameter of every query of that partition shares them.
#[derive(Clone, Debug)]
struct ResolvedQueryDateRange {
    local: QueryDateRange,
    utc: QueryDateRange,
}

/// One partition that was loaded, with the range it covers.
#[derive(Clone, Debug)]
struct PartitionLoadResult {
    result: LoadPreAggregationResult,
    range: QueryDateRange,
}

/// `PreAggregationPartitionRangeLoader` (`QO/PreAggregationPartitionRangeLoader.ts:54-617`).
pub struct PreAggregationPartitionRangeLoader {
    pre_aggregations: Arc<PreAggregations>,
    pre_aggregation: PreAggregationDescription,
    pre_aggregation_tables: Vec<PreAggTableToTempTable>,
    load_cache: Arc<PreAggregationLoadCache>,
    options: LoadOptions,
}

impl PreAggregationPartitionRangeLoader {
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
            options,
        }
    }

    fn priority(&self, default: QueuePriority) -> i32 {
        self.pre_aggregation.priority.unwrap_or(default.value())
    }

    fn timezone(&self) -> &str {
        self.pre_aggregation.timezone()
    }

    fn granularity(&self) -> Option<&str> {
        self.pre_aggregation.partition_granularity.as_deref()
    }

    // ------------------------------------------------------------------
    // Build range
    // ------------------------------------------------------------------

    /// `loadRangeQuery(rangeQuery, partitionRange?)` (`:93-118`).
    ///
    /// A build range query is cached for a day under a key that carries the identity of the
    /// pre-aggregation's first invalidation key query, so that a refresh worker and an API
    /// instance share one answer. When a partition range is given, the invalidation key
    /// values of that partition become the renewal key, which is what makes the *last*
    /// partition's range re-read as soon as new data lands while the earlier ones stay put.
    async fn load_range_query(
        &self,
        range_query: &QueryWithParams,
        partition_range: Option<&QueryDateRange>,
    ) -> Result<Value, OrchError> {
        let query_cache = self.pre_aggregations.query_cache();
        let invalidate = QueryCache::build_range_invalidate_key(
            self.pre_aggregation
                .invalidate_key_queries
                .as_deref()
                .unwrap_or(&[]),
            self.pre_aggregation.data_source(),
        );

        let cache_key = cubecache::query_cache_key(&QueryCacheKeyInput {
            query: Some(range_query.sql.clone()),
            values: Some(range_query.params.clone()),
            pre_aggregation_load_sql: Vec::new(),
            invalidate: invalidate.as_ref().map(cache_key_to_key_value),
            persistent: false,
        });

        let renewal_threshold = query_cache
            .options()
            .refresh_key_renewal_threshold
            .filter(|value| *value > 0)
            .or_else(|| {
                range_query
                    .options
                    .as_ref()
                    .and_then(|options| options.renewal_threshold)
                    .filter(|value| *value > 0)
            })
            .unwrap_or(24 * 60 * 60);

        let renewal_key = match partition_range {
            Some(range) => Some(CacheKey::list(
                self.get_invalidation_key_values(range)
                    .await?
                    .iter()
                    .map(KeyValue::from)
                    .collect(),
            )),
            None => None,
        };

        query_cache
            .cache_query_result(
                &range_query.sql,
                &range_query.params,
                &cache_key,
                24 * 60 * 60,
                CacheQueryResultOptions {
                    renewal_threshold: Some(renewal_threshold),
                    renewal_key,
                    priority: Some(self.priority(QueuePriority::Interactive)),
                    external: range_query.is_external(),
                    request_id: self.options.request_id.clone(),
                    data_source: self.pre_aggregation.data_source().to_string(),
                    wait_for_renew: self.options.wait_for_renew,
                    use_in_memory: true,
                    ..Default::default()
                },
            )
            .await
    }

    /// `getInvalidationKeyValues(range)` (`:120-138`).
    async fn get_invalidation_key_values(
        &self,
        range: &QueryDateRange,
    ) -> Result<Vec<Value>, OrchError> {
        let queries = self
            .pre_aggregation
            .invalidate_key_queries
            .clone()
            .unwrap_or_default();

        if queries.is_empty() {
            return Ok(Vec::new());
        }

        let table_name = partition_table_name(
            &self.pre_aggregation.table_name,
            self.granularity().unwrap_or("day"),
            &range.0,
        );
        let partition_range = self.resolve_partition_range(range)?;
        let mut values = Vec::with_capacity(queries.len());

        for query in &queries {
            let query =
                self.replace_partition_sql_and_params(query, &partition_range, &table_name)?;

            values.push(Value::Array(
                self.load_cache
                    .key_query_result(
                        &query,
                        self.options.wait_for_renew,
                        self.priority(QueuePriority::Interactive),
                    )
                    .await?,
            ));
        }

        Ok(values)
    }

    /// `loadBuildRange(timestampFormat)` (`:487-517`).
    ///
    /// The range queries run twice: once to find out roughly where the data starts and ends,
    /// and once more restricted to the first and the last partition, so that the bounds are
    /// the real minimum and maximum within those partitions rather than a whole-table scan's.
    pub async fn load_build_range(
        &self,
        timestamp_format: Option<&str>,
    ) -> Result<QueryDateRange, OrchError> {
        let queries = self
            .pre_aggregation
            .pre_aggregation_start_end_queries
            .clone()
            .unwrap_or_default();

        let mut dates: Vec<Option<String>> = Vec::with_capacity(2);
        for query in &queries {
            let data = self.load_range_query(query, None).await?;
            dates.push(self.extract_date(&data, timestamp_format)?);
        }
        while dates.len() < 2 {
            dates.push(None);
        }

        let rough = self.or_now_if_empty(dates[0].clone(), dates[1].clone())?;

        let Some(granularity) = self.granularity() else {
            return Ok(rough);
        };

        let (first_partition, last_partition) = time_series_boundaries(
            granularity,
            &rough,
            self.pre_aggregation.timestamp_precision(),
        )?;

        let mut bounds: Vec<Option<String>> = Vec::with_capacity(2);
        for (index, query) in queries.iter().enumerate() {
            let partition = if index == 0 {
                first_partition.as_ref()
            } else {
                last_partition.as_ref()
            };
            let data = self.load_range_query(query, partition).await?;
            bounds.push(self.extract_date(&data, timestamp_format)?);
        }
        while bounds.len() < 2 {
            bounds.push(None);
        }

        self.or_now_if_empty(bounds[0].clone(), bounds[1].clone())
    }

    fn extract_date(
        &self,
        data: &Value,
        timestamp_format: Option<&str>,
    ) -> Result<Option<String>, OrchError> {
        parse_utc_into_local_date(
            data,
            self.timezone(),
            Some(timestamp_format.unwrap_or(DEFAULT_TS_FORMAT)),
        )
    }

    /// `orNowIfEmpty` (`:523-535`) — a half open range collapses onto the bound it has, and
    /// an empty one onto "now" in the pre-aggregation's own timezone.
    fn or_now_if_empty(
        &self,
        start: Option<String>,
        end: Option<String>,
    ) -> Result<QueryDateRange, OrchError> {
        Ok(match (start, end) {
            (None, None) => {
                let now = now_in_time_zone(self.timezone())?;
                (now.clone(), now)
            }
            (None, Some(end)) => (end.clone(), end),
            (Some(start), None) => (start.clone(), start),
            (Some(start), Some(end)) => (start, end),
        })
    }

    // ------------------------------------------------------------------
    // Partition ranges
    // ------------------------------------------------------------------

    /// `partitionRanges(ignoreMatchedDateRange)` (`:452-485`).
    async fn partition_ranges(
        &self,
        ignore_matched_date_range: bool,
    ) -> Result<(QueryDateRange, Vec<QueryDateRange>), OrchError> {
        let build_range = self.load_build_range(None).await?;
        let matched = if ignore_matched_date_range {
            None
        } else {
            self.pre_aggregation.matched_time_dimension_date_range()
        };

        // No overlap between what the query asks for and what was built: the last partition
        // still gives the outer query the column types it expects.
        let date_range = intersect_date_ranges(Some(&build_range), matched.as_ref())?
            .unwrap_or_else(|| (build_range.1.clone(), build_range.1.clone()));

        let granularity = self.granularity().unwrap_or("day");
        let partition_ranges = time_series(
            granularity,
            &date_range,
            self.pre_aggregation.timestamp_precision(),
        )?;

        let max_partitions = self.pre_aggregations.options().max_partitions;
        if partition_ranges.len() > max_partitions {
            return Err(OrchError::orchestration(format!(
                "Pre-aggregation '{}' requested to build {} partitions which exceeds the maximum \
                 number of partitions per pre-aggregation of {max_partitions}",
                self.pre_aggregation.table_name,
                partition_ranges.len(),
            )));
        }

        Ok((date_range, partition_ranges))
    }

    /// `resolvePartitionRange` (`:163-171`).
    fn resolve_partition_range(
        &self,
        local: &QueryDateRange,
    ) -> Result<ResolvedQueryDateRange, OrchError> {
        Ok(ResolvedQueryDateRange {
            local: local.clone(),
            utc: (
                self.in_db_time_zone(&local.0)?,
                self.in_db_time_zone(&local.1)?,
            ),
        })
    }

    /// `PreAggregationPartitionRangeLoader.inDbTimeZone` (`:606-608`).
    fn in_db_time_zone(&self, timestamp: &str) -> Result<String, OrchError> {
        Ok(local_timestamp_to_utc(
            self.timezone(),
            self.pre_aggregation.timestamp_format.as_deref(),
            Some(timestamp),
        )?
        .unwrap_or_else(|| timestamp.to_string()))
    }

    /// `replacePartitionSqlAndParams` (`:173-204`).
    ///
    /// Three things happen at once: the logical pre-aggregation name in the SQL becomes this
    /// partition's name (the **first** occurrence only, as `String.replace` with a string
    /// argument does), the two range placeholders become the partition's UTC bounds, and an
    /// incremental refresh key's renewal threshold is shrunk once the partition's update
    /// window has closed — to the time since it closed, so that a data source whose clock
    /// runs slightly behind the server's is still re-read promptly.
    fn replace_partition_sql_and_params(
        &self,
        query: &QueryWithParams,
        range: &ResolvedQueryDateRange,
        partition_table_name: &str,
    ) -> Result<QueryWithParams, OrchError> {
        let options = query.options.clone();
        let incremental = options
            .as_ref()
            .and_then(|options| options.incremental)
            .unwrap_or(false);

        let update_window_to_boundary = if incremental {
            Some(add_seconds_to_local_timestamp(
                &range.local.1,
                self.timezone(),
                options
                    .as_ref()
                    .and_then(|options| options.update_window_seconds)
                    .unwrap_or(0) as i64,
            )?)
        } else {
            None
        };

        let sql = query
            .sql
            .replacen(&self.pre_aggregation.table_name, partition_table_name, 1);
        let params = query
            .params
            .iter()
            .map(|param| {
                if param == FROM_PARTITION_RANGE {
                    range.utc.0.clone()
                } else if param == TO_PARTITION_RANGE {
                    range.utc.1.clone()
                } else {
                    param.clone()
                }
            })
            .collect();

        let now = chrono::Utc::now();
        let renewal_threshold = match update_window_to_boundary {
            Some(boundary) if boundary < now => {
                // `Math.min(elapsed, undefined)` is `NaN` in the source, and `NaN` is falsy
                // wherever the threshold is read, so an absent outside-window threshold means
                // "always expired" — which is what an absent threshold means here.
                options
                    .as_ref()
                    .and_then(|options| options.renewal_threshold_outside_update_window)
                    .map(|outside| {
                        let elapsed =
                            ((now - boundary).num_milliseconds() as f64 / 1000.0).round() as u64;

                        elapsed.min(outside)
                    })
            }
            _ => options
                .as_ref()
                .and_then(|options| options.renewal_threshold),
        };

        // The options element is always present on a partitioned query, even when it ends up
        // empty: it takes part in the structure and content version hashes, so dropping it
        // would rename every partition table.
        let mut options = options.unwrap_or_default();
        options.renewal_threshold = renewal_threshold;

        Ok(QueryWithParams {
            sql,
            params,
            options: Some(options),
        })
    }

    /// `partitionPreAggregationDescription(range, buildRange)` (`:206-247`).
    fn partition_pre_aggregation_description(
        &self,
        range: &QueryDateRange,
        build_range: &QueryDateRange,
    ) -> Result<PreAggregationDescription, OrchError> {
        let pre_aggregation = &self.pre_aggregation;
        let table_name = partition_table_name(
            &pre_aggregation.table_name,
            self.granularity().unwrap_or("day"),
            &range.0,
        );

        let mut load_range = range.clone();
        let partition_invalidate_key_queries = pre_aggregation
            .partition_invalidate_key_queries
            .as_ref()
            .or(pre_aggregation.invalidate_key_queries.as_ref());

        // A real time pre-aggregation spells `partitionInvalidateKeyQueries: []`, and is the
        // one case where a partition is loaded past the build range end.
        let clip = partition_invalidate_key_queries
            .map(|queries| !queries.is_empty())
            .unwrap_or(true);

        if clip && build_range.1 < range.1 {
            load_range.1 = build_range.1.clone();
        }

        let clipped = load_range.1 != range.1;
        let partition_range = self.resolve_partition_range(range)?;
        let partition_load_range = if clipped {
            self.resolve_partition_range(&load_range)?
        } else {
            partition_range.clone()
        };

        let seal_at = add_seconds_to_local_timestamp(
            &load_range.1,
            self.timezone(),
            pre_aggregation.update_window_seconds.unwrap_or(0) as i64,
        )?
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string();

        let structure_version_load_sql = pre_aggregation
            .load_sql
            .as_ref()
            .map(|query| {
                self.replace_partition_sql_and_params(query, &partition_range, &table_name)
            })
            .transpose()?;
        let load_sql = if clipped {
            pre_aggregation
                .load_sql
                .as_ref()
                .map(|query| {
                    self.replace_partition_sql_and_params(query, &partition_load_range, &table_name)
                })
                .transpose()?
        } else {
            structure_version_load_sql.clone()
        };

        let map_queries = |queries: &[QueryWithParams]| -> Result<Vec<QueryWithParams>, OrchError> {
            queries
                .iter()
                .map(|query| {
                    self.replace_partition_sql_and_params(query, &partition_range, &table_name)
                })
                .collect()
        };

        let mut partition = pre_aggregation.clone();
        partition.table_name = table_name.clone();
        partition.structure_version_load_sql = structure_version_load_sql;
        partition.load_sql = load_sql;
        partition.sql = pre_aggregation
            .sql
            .as_ref()
            .map(|query| {
                self.replace_partition_sql_and_params(query, &partition_load_range, &table_name)
            })
            .transpose()?;
        partition.invalidate_key_queries = Some(map_queries(
            pre_aggregation
                .invalidate_key_queries
                .as_deref()
                .unwrap_or(&[]),
        )?);
        partition.partition_invalidate_key_queries = pre_aggregation
            .partition_invalidate_key_queries
            .as_deref()
            .map(map_queries)
            .transpose()?;
        partition.indexes_sql = Some(self.replace_indexes_sql(
            pre_aggregation.indexes_sql.as_ref(),
            &partition_range,
            &table_name,
        )?);
        partition.preview_sql = pre_aggregation
            .preview_sql
            .as_ref()
            .map(|query| {
                self.replace_partition_sql_and_params(query, &partition_range, &table_name)
            })
            .transpose()?;
        partition.build_range_start = Some(load_range.0.clone());
        partition.build_range_end = Some(load_range.1.clone());
        partition.seal_at = Some(seal_at);
        partition.expanded_partition = true;

        Ok(partition)
    }

    /// `indexesSql.map(q => ({ ...q, sql: replacePartitionSqlAndParams(q.sql, …) }))`.
    fn replace_indexes_sql(
        &self,
        indexes_sql: Option<&Value>,
        range: &ResolvedQueryDateRange,
        partition_table_name: &str,
    ) -> Result<Value, OrchError> {
        let Some(Value::Array(indexes)) = indexes_sql else {
            return Ok(Value::Array(Vec::new()));
        };

        let mut result = Vec::with_capacity(indexes.len());

        for index in indexes {
            let mut index = index.clone();
            let Some(object) = index.as_object_mut() else {
                result.push(index);
                continue;
            };

            let Some(sql) = object.get("sql") else {
                result.push(index);
                continue;
            };
            let Ok(query) = serde_json::from_value::<QueryWithParams>(sql.clone()) else {
                result.push(index);
                continue;
            };

            let replaced =
                self.replace_partition_sql_and_params(&query, range, partition_table_name)?;
            object.insert(
                "sql".to_string(),
                serde_json::to_value(&replaced)
                    .map_err(|error| OrchError::orchestration(error.to_string()))?,
            );
            result.push(index);
        }

        Ok(Value::Array(result))
    }

    // ------------------------------------------------------------------
    // Loading
    // ------------------------------------------------------------------

    fn loader_for(&self, description: PreAggregationDescription) -> PreAggregationLoader {
        PreAggregationLoader::new(
            self.pre_aggregations.clone(),
            description,
            self.pre_aggregation_tables.clone(),
            self.load_cache.clone(),
            self.options.clone(),
        )
    }

    async fn load_by_partition_ranges(
        &self,
        build_range: &QueryDateRange,
        partition_ranges: &[QueryDateRange],
    ) -> Result<(Vec<PartitionLoadResult>, Vec<PreAggregationDescription>), OrchError> {
        let mut descriptions = Vec::with_capacity(partition_ranges.len());
        let mut results = Vec::with_capacity(partition_ranges.len());

        for range in partition_ranges {
            let description = self.partition_pre_aggregation_description(range, build_range)?;
            descriptions.push(description.clone());

            // `throwOnMissingPartition: false` — a partition that is not there yet is left
            // out of the union instead of failing the whole query.
            if let Some(result) = self
                .loader_for(description)
                .load_pre_aggregation(false)
                .await?
            {
                results.push(PartitionLoadResult {
                    result,
                    range: range.clone(),
                });
            }
        }

        Ok((results, descriptions))
    }

    /// `replaceQueryBuildRangeParams(queryValues)` (`:144-160`) — the outer query's own
    /// values may name the build range, which only the loader knows.
    pub async fn replace_query_build_range_params(
        &self,
        query_values: &[String],
    ) -> Result<Option<Vec<String>>, OrchError> {
        if !query_values
            .iter()
            .any(|value| value == BUILD_RANGE_START_LOCAL || value == BUILD_RANGE_END_LOCAL)
        {
            return Ok(None);
        }

        let (start, end) = self
            .load_build_range(self.pre_aggregation.timestamp_format.as_deref())
            .await?;

        Ok(Some(
            query_values
                .iter()
                .map(|value| {
                    if value == BUILD_RANGE_START_LOCAL {
                        start.clone()
                    } else if value == BUILD_RANGE_END_LOCAL {
                        end.clone()
                    } else {
                        value.clone()
                    }
                })
                .collect(),
        ))
    }

    /// `loadPreAggregations()` (`:249-399`).
    pub async fn load_pre_aggregations(
        &self,
    ) -> Result<Option<LoadPreAggregationResult>, OrchError> {
        if self
            .pre_aggregation
            .extra
            .get("rollupLambdaId")
            .is_some_and(|value| !value.is_null())
        {
            return Err(OrchError::not_implemented(format!(
                "Lambda rollups are not ported to the Rust orchestrator yet ({})",
                self.pre_aggregation.table_name
            )));
        }

        if self.pre_aggregation.partition_granularity.is_none()
            || self.pre_aggregation.expanded_partition
        {
            let result = self
                .loader_for(self.pre_aggregation.clone())
                .load_pre_aggregation(true)
                .await?;

            return Ok(result.map(|result| self.with_flat_usage_mapping(result)));
        }

        let (build_range, partition_ranges) = self.partition_ranges(false).await?;
        let (mut results, mut descriptions) = self
            .load_by_partition_ranges(&build_range, &partition_ranges)
            .await?;

        // Nothing in the requested window has been built. A read only instance cannot build
        // it, so it widens the search to the whole build range and serves the newest
        // partition it finds, which at least gives the outer query the right table structure.
        if self.options.external_refresh && results.is_empty() {
            let (build_range, partition_ranges) = self.partition_ranges(true).await?;
            let (widened, widened_descriptions) = self
                .load_by_partition_ranges(&build_range, &partition_ranges)
                .await?;

            descriptions = widened_descriptions;
            results = match widened.last() {
                Some(last) => vec![last.clone()],
                None => Vec::new(),
            };
        }

        if self.options.external_refresh && results.is_empty() {
            return Err(OrchError::orchestration(
                no_pre_aggregation_partitions_built_message(&descriptions),
            ));
        }

        let all_table_target_names: Vec<String> = results
            .iter()
            .map(|loaded| loaded.result.target_table_name.clone())
            .collect();
        let last_updated_at = get_last_updated_at_timestamp(
            &results
                .iter()
                .map(|loaded| loaded.result.last_updated_at)
                .collect::<Vec<_>>(),
        );

        let base_target_table_name = union_of(&all_table_target_names);

        Ok(Some(LoadPreAggregationResult {
            target_table_name: base_target_table_name.clone(),
            refresh_key_values: Some(
                results
                    .iter()
                    .map(|loaded| {
                        loaded
                            .result
                            .refresh_key_values
                            .clone()
                            .map(Value::Array)
                            .unwrap_or(Value::Null)
                    })
                    .collect(),
            ),
            last_updated_at,
            build_range_end: results
                .last()
                .and_then(|loaded| loaded.result.build_range_end.clone()),
            is_multi_table_union: all_table_target_names.len() > 1,
            usage_target_table_names: self
                .usage_target_table_names(&results, &base_target_table_name)?,
            ..Default::default()
        }))
    }

    /// The unpartitioned branch of `loadPreAggregations` (`:389-396`): every usage points at
    /// the single table that was loaded.
    fn with_flat_usage_mapping(
        &self,
        mut result: LoadPreAggregationResult,
    ) -> LoadPreAggregationResult {
        let Some(usage_mapping) = &self.pre_aggregation.usage_mapping else {
            return result;
        };

        result.usage_target_table_names = usage_mapping
            .keys()
            .map(|suffix| (suffix.clone(), result.target_table_name.clone()))
            .collect();

        result
    }

    /// `usageTargetTableNames` (`:327-366`) — a usage that only spans part of the build range
    /// reads only the partitions it overlaps, so an unrelated usage's partitions do not have
    /// to be scanned.
    fn usage_target_table_names(
        &self,
        results: &[PartitionLoadResult],
        base_target_table_name: &str,
    ) -> Result<BTreeMap<String, String>, OrchError> {
        let Some(usage_mapping) = &self.pre_aggregation.usage_mapping else {
            return Ok(BTreeMap::new());
        };

        let mut names = BTreeMap::new();

        for (suffix, usage) in usage_mapping {
            let usage_range = usage
                .get("dateRange")
                .and_then(Value::as_array)
                .and_then(|range| match range.as_slice() {
                    [from, to] => Some((from.as_str()?.to_string(), to.as_str()?.to_string())),
                    _ => None,
                });

            let (Some(usage_range), Some(first), Some(last)) =
                (usage_range, results.first(), results.last())
            else {
                names.insert(suffix.clone(), base_target_table_name.to_string());
                continue;
            };

            let loaded_range = (first.range.0.clone(), last.range.1.clone());
            let Some(usage_range) = intersect_date_ranges(Some(&loaded_range), Some(&usage_range))?
            else {
                names.insert(suffix.clone(), base_target_table_name.to_string());
                continue;
            };

            let tables: Vec<String> = results
                .iter()
                .filter(|loaded| loaded.range.1 >= usage_range.0 && loaded.range.0 <= usage_range.1)
                .map(|loaded| loaded.result.target_table_name.clone())
                .collect();

            names.insert(
                suffix.clone(),
                if tables.is_empty() {
                    base_target_table_name.to_string()
                } else {
                    union_of(&tables)
                },
            );
        }

        Ok(names)
    }

    /// `partitionPreAggregations()` (`:443-450`) — the descriptions without loading them,
    /// which the pre-aggregation system endpoints report.
    pub async fn partition_pre_aggregations(
        &self,
    ) -> Result<Vec<PreAggregationDescription>, OrchError> {
        if self.pre_aggregation.partition_granularity.is_none()
            || self.pre_aggregation.expanded_partition
        {
            return Ok(vec![self.pre_aggregation.clone()]);
        }

        let (build_range, partition_ranges) = self.partition_ranges(false).await?;

        partition_ranges
            .iter()
            .map(|range| self.partition_pre_aggregation_description(range, &build_range))
            .collect()
    }

    /// Logs through the registry's logger, so a partition build is reported like any other.
    #[allow(dead_code)]
    fn log(&self, message: &str, event: Value) {
        self.pre_aggregations.log(message, event);
    }
}

/// A single table stays a table name; several become the subquery the outer SQL reads
/// (`:321-325`).
fn union_of(tables: &[String]) -> String {
    if tables.len() == 1 {
        return tables[0].clone();
    }

    format!(
        "({})",
        tables
            .iter()
            .map(|table| format!("SELECT * FROM {table}"))
            .collect::<Vec<_>>()
            .join(" UNION ALL ")
    )
}

fn cache_key_to_key_value(key: &CacheKey) -> KeyValue {
    match key {
        CacheKey::Str(value) => KeyValue::Str(value.clone()),
        CacheKey::List { items, .. } => KeyValue::Array(items.clone()),
    }
}

/// `checkDataRangeType` (`:537-553`).
fn check_date_range_type(range: Option<&QueryDateRange>) -> Result<(), OrchError> {
    let Some(range) = range else {
        return Ok(());
    };

    let lengths = (range.0.chars().count(), range.1.chars().count());
    if !matches!(lengths.0, 23 | 26) || !matches!(lengths.1, 23 | 26) {
        return Err(OrchError::orchestration(format!(
            "Date range expected to be in {DEFAULT_TS_FORMAT} format but {},{} found",
            range.0, range.1
        )));
    }

    Ok(())
}

/// `PreAggregationPartitionRangeLoader.intersectDateRanges` (`:555-573`).
///
/// Both ranges are local timestamps of the same width, so the bounds are compared as
/// strings — exactly as the source does.
pub fn intersect_date_ranges(
    range_a: Option<&QueryDateRange>,
    range_b: Option<&QueryDateRange>,
) -> Result<Option<QueryDateRange>, OrchError> {
    check_date_range_type(range_a)?;
    check_date_range_type(range_b)?;

    let (Some(range_a), Some(range_b)) = (range_a, range_b) else {
        return Ok(range_a.or(range_b).cloned());
    };

    let from = if range_a.0 > range_b.0 {
        &range_a.0
    } else {
        &range_b.0
    };
    let to = if range_a.1 < range_b.1 {
        &range_a.1
    } else {
        &range_b.1
    };

    if from > to {
        return Ok(None);
    }

    Ok(Some((from.clone(), to.clone())))
}

/// The event shape the partition loader logs a refused lambda rollup with.
#[allow(dead_code)]
fn lambda_event(table_name: &str) -> Value {
    json!({ "preAggregation": table_name })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start: &str, end: &str) -> QueryDateRange {
        (start.to_string(), end.to_string())
    }

    #[test]
    fn intersection_of_two_ranges() {
        let a = range("2021-01-01T00:00:00.000", "2021-03-31T23:59:59.999");
        let b = range("2021-02-01T00:00:00.000", "2021-04-30T23:59:59.999");

        assert_eq!(
            intersect_date_ranges(Some(&a), Some(&b)).unwrap(),
            Some(range("2021-02-01T00:00:00.000", "2021-03-31T23:59:59.999"))
        );
        assert_eq!(
            intersect_date_ranges(Some(&a), None).unwrap(),
            Some(a.clone())
        );
        assert_eq!(
            intersect_date_ranges(None, Some(&b)).unwrap(),
            Some(b.clone())
        );
        assert_eq!(intersect_date_ranges(None, None).unwrap(), None);

        let disjoint = range("2022-01-01T00:00:00.000", "2022-03-31T23:59:59.999");
        assert_eq!(
            intersect_date_ranges(Some(&a), Some(&disjoint)).unwrap(),
            None
        );
    }

    #[test]
    fn a_range_that_is_not_a_local_timestamp_is_refused() {
        let bad = range("2021-01-01", "2021-03-31");

        assert!(intersect_date_ranges(Some(&bad), None)
            .unwrap_err()
            .to_string()
            .starts_with("Date range expected to be in YYYY-MM-DDTHH:mm:ss.SSS format"));
    }

    #[test]
    fn the_union_of_one_table_is_the_table() {
        assert_eq!(union_of(&["a.b_1".to_string()]), "a.b_1");
        assert_eq!(
            union_of(&["a.b_1".to_string(), "a.b_2".to_string()]),
            "(SELECT * FROM a.b_1 UNION ALL SELECT * FROM a.b_2)"
        );
    }
}
