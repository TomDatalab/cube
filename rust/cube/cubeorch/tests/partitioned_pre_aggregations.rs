//! Partitioned pre-aggregations and the two external build strategies.
//!
//! Mirrors `packages/cubejs-query-orchestrator/test/unit/PreAggregations.test.js` and the
//! partition scenarios of `QueryOrchestrator.test.js`, against in-memory `Driver`s.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use cubedriver::{
    Column, Driver, DriverCapabilities, DriverConfig, DriverError, ExternalCreateTableOptions,
    GenericType, IndexSql, QueryOptions, QueryResult, TableMemoryData,
};
use cubeorch::{
    preaggs::PreAggregationsOptions,
    types::{PreAggregationDescription, QueryWithParams, RefreshKeyQueryOptions},
    DriverFactory, LoadOptions, OrchError, PreAggregationLoadCache,
    PreAggregationPartitionRangeLoader, PreAggregations, QueryBody, QueryOrchestrator,
    QueryOrchestratorOptions, FROM_PARTITION_RANGE, TO_PARTITION_RANGE,
};
use serde_json::{json, Value};

// ----------------------------------------------------------------------------
// A driver that records every statement with its parameters.
// ----------------------------------------------------------------------------

/// One `uploadTableWithIndexes` call against the external store.
#[derive(Clone, Debug, PartialEq)]
struct Upload {
    table: String,
    columns: Vec<String>,
    rows: usize,
    indexes: Vec<String>,
    unique_key_columns: Vec<String>,
    aggregations_columns: Vec<String>,
    create_table_indexes: Vec<String>,
    seal_at: Option<String>,
}

#[derive(Default)]
struct FakeDriverState {
    answers: HashMap<String, (Vec<String>, Vec<Vec<Value>>)>,
    executed: Vec<(String, Vec<Value>)>,
    tables: Vec<String>,
    uploads: Vec<Upload>,
}

struct FakeDriver {
    config: DriverConfig,
    state: Mutex<FakeDriverState>,
    read_only: bool,
    capabilities: DriverCapabilities,
    /// When set, `unload` reports these CSV files instead of refusing, which
    /// is what a driver with an export bucket does.
    unload_to: Option<cubedriver::types::TableCsvData>,
}

impl FakeDriver {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            config: DriverConfig::default(),
            state: Mutex::new(FakeDriverState::default()),
            read_only: false,
            capabilities: DriverCapabilities::default(),
            unload_to: None,
        })
    }

    /// A read-only driver whose export bucket unloads to `csv`, so the
    /// external build takes the CSV branch.
    fn read_only_unloading_to_csv(csv: cubedriver::types::TableCsvData) -> Arc<Self> {
        Arc::new(Self {
            config: DriverConfig::default(),
            state: Mutex::new(FakeDriverState::default()),
            read_only: true,
            capabilities: DriverCapabilities::default(),
            unload_to: Some(csv),
        })
    }

    /// An external store that imports CSV natively.
    fn importing_csv() -> Arc<Self> {
        Arc::new(Self {
            config: DriverConfig::default(),
            state: Mutex::new(FakeDriverState::default()),
            read_only: false,
            capabilities: DriverCapabilities {
                csv_import: true,
                ..DriverCapabilities::default()
            },
            unload_to: None,
        })
    }

    fn read_only() -> Arc<Self> {
        Arc::new(Self {
            config: DriverConfig::default(),
            state: Mutex::new(FakeDriverState::default()),
            read_only: true,
            capabilities: DriverCapabilities::default(),
            unload_to: None,
        })
    }

    fn answer(&self, sql: &str, columns: &[&str], rows: Vec<Vec<Value>>) {
        self.state.lock().unwrap().answers.insert(
            sql.to_string(),
            (columns.iter().map(|c| c.to_string()).collect(), rows),
        );
    }

    fn executed(&self) -> Vec<(String, Vec<Value>)> {
        self.state.lock().unwrap().executed.clone()
    }

    fn statements(&self) -> Vec<String> {
        self.executed().into_iter().map(|(sql, _)| sql).collect()
    }

    fn params_of(&self, sql_prefix: &str) -> Vec<Vec<Value>> {
        self.executed()
            .into_iter()
            .filter(|(sql, _)| sql.starts_with(sql_prefix))
            .map(|(_, params)| params)
            .collect()
    }

    fn tables(&self) -> Vec<String> {
        self.state.lock().unwrap().tables.clone()
    }

    fn uploads(&self) -> Vec<Upload> {
        self.state.lock().unwrap().uploads.clone()
    }
}

#[async_trait]
impl Driver for FakeDriver {
    fn config(&self) -> &DriverConfig {
        &self.config
    }

    async fn test_connection(&self) -> Result<(), DriverError> {
        Ok(())
    }

    fn read_only(&self) -> bool {
        self.read_only
    }

    fn capabilities(&self) -> DriverCapabilities {
        self.capabilities
    }

    fn now_timestamp(&self) -> i64 {
        1_600_000_000_000
    }

    async fn create_schema_if_not_exists(&self, _schema_name: &str) -> Result<(), DriverError> {
        Ok(())
    }

    async fn load_pre_aggregation_into_table(
        &self,
        pre_aggregation_table_name: &str,
        load_sql: &str,
        params: &[Value],
        options: &QueryOptions,
    ) -> Result<QueryResult, DriverError> {
        let result = self.query(load_sql, params, options).await?;
        let bare = pre_aggregation_table_name
            .split_once('.')
            .map(|(_, name)| name)
            .unwrap_or(pre_aggregation_table_name);

        self.state.lock().unwrap().tables.push(bare.to_string());

        Ok(result)
    }

    async fn drop_table(
        &self,
        table_name: &str,
        _options: &QueryOptions,
    ) -> Result<(), DriverError> {
        let bare = table_name
            .split_once('.')
            .map(|(_, name)| name)
            .unwrap_or(table_name)
            .to_string();

        let mut state = self.state.lock().unwrap();
        state
            .executed
            .push((format!("DROP TABLE {table_name}"), Vec::new()));
        state.tables.retain(|table| *table != bare);

        Ok(())
    }

    async fn is_unload_supported(
        &self,
        _options: &cubedriver::types::UnloadOptions,
    ) -> Result<bool, DriverError> {
        Ok(self.unload_to.is_some())
    }

    async fn unload(
        &self,
        _table: &str,
        _options: &cubedriver::types::UnloadOptions,
    ) -> Result<cubedriver::types::TableCsvData, DriverError> {
        self.unload_to.clone().ok_or_else(|| {
            DriverError::NotImplemented("Driver's .unload() method is not implemented.".to_string())
        })
    }

    async fn unload_from_query(
        &self,
        _sql: &str,
        _params: &[Value],
        _options: &cubedriver::types::UnloadOptions,
    ) -> Result<cubedriver::types::TableCsvData, DriverError> {
        self.unload_to.clone().ok_or_else(|| {
            DriverError::NotImplemented(
                "Driver's .unloadFromQuery() method is not implemented.".to_string(),
            )
        })
    }

    async fn upload_table_with_indexes(
        &self,
        table: &str,
        columns: &[Column],
        table_data: &TableMemoryData,
        indexes_sql: &[IndexSql],
        unique_key_columns: &[String],
        external_options: &ExternalCreateTableOptions,
    ) -> Result<(), DriverError> {
        let bare = table
            .split_once('.')
            .map(|(_, name)| name)
            .unwrap_or(table)
            .to_string();

        let mut state = self.state.lock().unwrap();
        state.uploads.push(Upload {
            table: table.to_string(),
            columns: columns.iter().map(|column| column.name.clone()).collect(),
            rows: table_data.rows.len(),
            indexes: indexes_sql.iter().map(|index| index.sql.clone()).collect(),
            unique_key_columns: unique_key_columns.to_vec(),
            aggregations_columns: external_options.aggregations_columns.clone(),
            create_table_indexes: external_options
                .create_table_indexes
                .iter()
                .map(|index| index.index_name.clone())
                .collect(),
            seal_at: external_options.seal_at.clone(),
        });
        state.tables.push(bare);

        Ok(())
    }

    async fn query(
        &self,
        sql: &str,
        params: &[Value],
        _options: &QueryOptions,
    ) -> Result<QueryResult, DriverError> {
        let answer = {
            let mut state = self.state.lock().unwrap();
            state.executed.push((sql.to_string(), params.to_vec()));
            state.answers.get(sql).cloned()
        };

        if sql.contains("information_schema.tables") {
            return Ok(QueryResult::new(
                vec![Column::new("table_name", GenericType::String)],
                self.tables()
                    .into_iter()
                    .map(|table| vec![Value::String(table)])
                    .collect(),
            ));
        }

        let (columns, rows) =
            answer.unwrap_or_else(|| (vec!["ok".to_string()], vec![vec![json!("1")]]));

        Ok(QueryResult::new(
            columns
                .into_iter()
                .map(|name| Column::new(name, GenericType::String))
                .collect(),
            rows,
        ))
    }
}

fn factory(driver: Arc<FakeDriver>) -> DriverFactory {
    Arc::new(move |_data_source| {
        let driver = driver.clone();

        Box::pin(async move { Ok(driver as Arc<dyn Driver>) })
    })
}

// ----------------------------------------------------------------------------
// Fixtures
// ----------------------------------------------------------------------------

const MIN_QUERY: &str = "SELECT MIN(created_at) FROM orders";
const MAX_QUERY: &str = "SELECT MAX(created_at) FROM orders";

fn partitioned_description() -> PreAggregationDescription {
    PreAggregationDescription {
        table_name: "stb_pre_aggregations.orders_main".to_string(),
        pre_aggregation_id: Some("Orders.main".to_string()),
        r#type: Some("rollup".to_string()),
        pre_aggregations_schema: Some("stb_pre_aggregations".to_string()),
        timezone: Some("UTC".to_string()),
        timestamp_precision: Some(3),
        partition_granularity: Some("day".to_string()),
        load_sql: Some(QueryWithParams::new(
            "CREATE TABLE stb_pre_aggregations.orders_main AS SELECT * FROM orders WHERE \
             created_at >= ? AND created_at <= ?",
            vec![
                FROM_PARTITION_RANGE.to_string(),
                TO_PARTITION_RANGE.to_string(),
            ],
        )),
        pre_aggregation_start_end_queries: Some(vec![
            QueryWithParams::new(MIN_QUERY, vec![]),
            QueryWithParams::new(MAX_QUERY, vec![]),
        ]),
        ..Default::default()
    }
}

fn with_range(driver: &FakeDriver, min: &str, max: &str) {
    driver.answer(MIN_QUERY, &["min"], vec![vec![json!(min)]]);
    driver.answer(MAX_QUERY, &["max"], vec![vec![json!(max)]]);
}

fn orchestrator(
    driver: Arc<FakeDriver>,
    external: Option<Arc<FakeDriver>>,
    options: QueryOrchestratorOptions,
) -> Arc<QueryOrchestrator> {
    QueryOrchestrator::new(
        "test",
        factory(driver),
        external.map(factory),
        Arc::new(|_, _| {}),
        options,
    )
}

fn query_body(description: PreAggregationDescription) -> QueryBody {
    QueryBody {
        query: Some("SELECT * FROM stb_pre_aggregations.orders_main".to_string()),
        values: Some(vec![]),
        data_source: Some("default".to_string()),
        pre_aggregations: vec![description],
        ..Default::default()
    }
}

fn loader(
    pre_aggregations: &Arc<PreAggregations>,
    description: PreAggregationDescription,
) -> PreAggregationPartitionRangeLoader {
    let load_cache = Arc::new(PreAggregationLoadCache::new(
        pre_aggregations.clone(),
        "default".to_string(),
        None,
    ));

    PreAggregationPartitionRangeLoader::new(
        pre_aggregations.clone(),
        description,
        Vec::new(),
        load_cache,
        LoadOptions::default(),
    )
}

// ----------------------------------------------------------------------------
// Build range
// ----------------------------------------------------------------------------

#[tokio::test]
async fn the_build_range_comes_from_the_range_queries() {
    let driver = FakeDriver::new();
    with_range(
        &driver,
        "2021-01-01T09:15:00.000",
        "2021-01-03T10:00:00.000",
    );

    let orchestrator = orchestrator(driver.clone(), None, QueryOrchestratorOptions::default());
    let loader = loader(orchestrator.pre_aggregations(), partitioned_description());

    assert_eq!(
        loader.load_build_range(None).await.unwrap(),
        (
            "2021-01-01T09:15:00.000".to_string(),
            "2021-01-03T10:00:00.000".to_string()
        )
    );

    // Both queries ran: once to find the rough bounds and once restricted to the first and
    // the last partition (the second pass is served from the day long cache entry here,
    // because nothing invalidated it in between).
    assert!(driver.statements().iter().any(|sql| sql == MIN_QUERY));
    assert!(driver.statements().iter().any(|sql| sql == MAX_QUERY));
}

#[tokio::test]
async fn an_empty_build_range_collapses_onto_now() {
    let driver = FakeDriver::new();
    driver.answer(MIN_QUERY, &["min"], vec![]);
    driver.answer(MAX_QUERY, &["max"], vec![]);

    let orchestrator = orchestrator(driver.clone(), None, QueryOrchestratorOptions::default());
    let loader = loader(orchestrator.pre_aggregations(), partitioned_description());

    let (start, end) = loader.load_build_range(None).await.unwrap();

    assert_eq!(start, end);
    assert_eq!(start.len(), 23);
}

#[tokio::test]
async fn the_build_range_is_snapped_to_whole_partitions() {
    let driver = FakeDriver::new();
    with_range(
        &driver,
        "2021-01-01T09:15:00.000",
        "2021-01-03T10:00:00.000",
    );

    let orchestrator = orchestrator(driver.clone(), None, QueryOrchestratorOptions::default());
    let loader = loader(orchestrator.pre_aggregations(), partitioned_description());

    let partitions = loader.partition_pre_aggregations().await.unwrap();

    // Three whole days, named after the day they cover.
    assert_eq!(
        partitions
            .iter()
            .map(|partition| partition.table_name.clone())
            .collect::<Vec<_>>(),
        vec![
            "stb_pre_aggregations.orders_main20210101",
            "stb_pre_aggregations.orders_main20210102",
            "stb_pre_aggregations.orders_main20210103",
        ]
    );

    // The first partition starts at midnight even though the data starts at 09:15…
    assert_eq!(
        partitions[0].build_range_start.as_deref(),
        Some("2021-01-01T00:00:00.000")
    );
    assert_eq!(
        partitions[0].build_range_end.as_deref(),
        Some("2021-01-01T23:59:59.999")
    );
    // …and the last one is clipped to where the data actually ends, so that the partition is
    // rebuilt rather than sealed when more rows land in it.
    assert_eq!(
        partitions[2].build_range_end.as_deref(),
        Some("2021-01-03T10:00:00.000")
    );
    assert!(partitions
        .iter()
        .all(|partition| partition.expanded_partition));
}

#[tokio::test]
async fn partition_table_names_follow_the_granularity() {
    let driver = FakeDriver::new();
    with_range(
        &driver,
        "2021-02-03T04:05:06.789",
        "2021-02-03T05:00:00.000",
    );

    let orchestrator = orchestrator(driver.clone(), None, QueryOrchestratorOptions::default());

    // The suffix is the partition's own start cut to the granularity: 10 characters for a
    // day and anything coarser, 13 for an hour, 16 for a minute, with the separators dropped.
    for (granularity, expected) in [
        ("day", "stb_pre_aggregations.orders_main20210203"),
        ("month", "stb_pre_aggregations.orders_main20210201"),
        ("year", "stb_pre_aggregations.orders_main20210101"),
        ("quarter", "stb_pre_aggregations.orders_main20210101"),
        ("week", "stb_pre_aggregations.orders_main20210201"),
        ("hour", "stb_pre_aggregations.orders_main2021020304"),
        ("minute", "stb_pre_aggregations.orders_main202102030405"),
    ] {
        let mut description = partitioned_description();
        description.partition_granularity = Some(granularity.to_string());

        let partitions = loader(orchestrator.pre_aggregations(), description)
            .partition_pre_aggregations()
            .await
            .unwrap();

        assert_eq!(partitions[0].table_name, expected, "{granularity}");
    }
}

#[tokio::test]
async fn the_matched_query_range_narrows_the_partitions() {
    let driver = FakeDriver::new();
    with_range(
        &driver,
        "2021-01-01T00:00:00.000",
        "2021-01-10T00:00:00.000",
    );

    let orchestrator = orchestrator(driver.clone(), None, QueryOrchestratorOptions::default());

    let mut description = partitioned_description();
    description.matched_time_dimension_date_range = Some(vec![
        "2021-01-04T00:00:00.000".to_string(),
        "2021-01-05T23:59:59.999".to_string(),
    ]);

    let partitions = loader(orchestrator.pre_aggregations(), description)
        .partition_pre_aggregations()
        .await
        .unwrap();

    assert_eq!(
        partitions
            .iter()
            .map(|partition| partition.table_name.clone())
            .collect::<Vec<_>>(),
        vec![
            "stb_pre_aggregations.orders_main20210104",
            "stb_pre_aggregations.orders_main20210105",
        ]
    );
}

#[tokio::test]
async fn too_many_partitions_are_refused() {
    let driver = FakeDriver::new();
    with_range(
        &driver,
        "2021-01-01T00:00:00.000",
        "2021-01-10T00:00:00.000",
    );

    let orchestrator = orchestrator(
        driver.clone(),
        None,
        QueryOrchestratorOptions {
            pre_aggregations_options: PreAggregationsOptions {
                max_partitions: 3,
                ..Default::default()
            },
            ..Default::default()
        },
    );

    let error = loader(orchestrator.pre_aggregations(), partitioned_description())
        .partition_pre_aggregations()
        .await
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "Pre-aggregation 'stb_pre_aggregations.orders_main' requested to build 10 partitions \
         which exceeds the maximum number of partitions per pre-aggregation of 3"
    );
}

// ----------------------------------------------------------------------------
// Partition descriptions
// ----------------------------------------------------------------------------

#[tokio::test]
async fn partition_range_placeholders_are_substituted() {
    let driver = FakeDriver::new();
    with_range(
        &driver,
        "2021-01-01T00:00:00.000",
        "2021-01-02T10:00:00.000",
    );

    let orchestrator = orchestrator(driver.clone(), None, QueryOrchestratorOptions::default());
    let partitions = loader(orchestrator.pre_aggregations(), partitioned_description())
        .partition_pre_aggregations()
        .await
        .unwrap();

    let first = partitions[0].load_sql.as_ref().unwrap();
    assert_eq!(
        first.sql,
        "CREATE TABLE stb_pre_aggregations.orders_main20210101 AS SELECT * FROM orders WHERE \
         created_at >= ? AND created_at <= ?"
    );
    assert_eq!(
        first.params,
        vec!["2021-01-01T00:00:00.000", "2021-01-01T23:59:59.999"]
    );

    // The clipped partition loads only up to the build range end, while the structure
    // version SQL keeps the whole partition so that clipping does not rename the table.
    let last = partitions[1].load_sql.as_ref().unwrap();
    assert_eq!(
        last.params,
        vec!["2021-01-02T00:00:00.000", "2021-01-02T10:00:00.000"]
    );
    assert_eq!(
        partitions[1]
            .structure_version_load_sql
            .as_ref()
            .unwrap()
            .params,
        vec!["2021-01-02T00:00:00.000", "2021-01-02T23:59:59.999"]
    );
}

#[tokio::test]
async fn partition_bounds_are_converted_into_the_data_source_timezone() {
    let driver = FakeDriver::new();
    // The data source answers in UTC, which is one whole day of New York wall clock time.
    with_range(
        &driver,
        "2021-01-01T05:00:00.000",
        "2021-01-02T04:59:59.999",
    );

    let orchestrator = orchestrator(driver.clone(), None, QueryOrchestratorOptions::default());

    let mut description = partitioned_description();
    description.timezone = Some("America/New_York".to_string());

    let partitions = loader(orchestrator.pre_aggregations(), description)
        .partition_pre_aggregations()
        .await
        .unwrap();

    // Midnight in New York is 05:00 UTC in January.
    assert_eq!(partitions.len(), 1);
    assert_eq!(
        partitions[0].table_name,
        "stb_pre_aggregations.orders_main20210101"
    );
    assert_eq!(
        partitions[0].build_range_start.as_deref(),
        Some("2021-01-01T00:00:00.000")
    );
    assert_eq!(
        partitions[0].load_sql.as_ref().unwrap().params,
        vec!["2021-01-01T05:00:00.000", "2021-01-02T04:59:59.999"]
    );
    assert_eq!(
        partitions[0].seal_at.as_deref(),
        Some("2021-01-02T04:59:59.999Z")
    );
}

#[tokio::test]
async fn an_incremental_refresh_key_shrinks_its_threshold_past_the_update_window() {
    let driver = FakeDriver::new();
    with_range(
        &driver,
        "2021-01-01T00:00:00.000",
        "2021-01-01T23:59:59.999",
    );

    let orchestrator = orchestrator(driver.clone(), None, QueryOrchestratorOptions::default());

    let mut description = partitioned_description();
    description.update_window_seconds = Some(86400);
    description.invalidate_key_queries = Some(vec![QueryWithParams::new(
        "SELECT MAX(created_at) FROM orders WHERE created_at >= ?",
        vec![FROM_PARTITION_RANGE.to_string()],
    )
    .with_options(RefreshKeyQueryOptions {
        incremental: Some(true),
        update_window_seconds: Some(86400),
        renewal_threshold: Some(300),
        renewal_threshold_outside_update_window: Some(86400),
        ..Default::default()
    })]);

    let partitions = loader(orchestrator.pre_aggregations(), description)
        .partition_pre_aggregations()
        .await
        .unwrap();

    let key = &partitions[0].invalidate_key_queries.as_ref().unwrap()[0];
    let options = key.options.as_ref().unwrap();

    // The window closed years ago, so the threshold is the full outside-window value rather
    // than the in-window 300 seconds.
    assert_eq!(options.renewal_threshold, Some(86400));
    assert_eq!(options.incremental, Some(true));
    assert_eq!(key.params, vec!["2021-01-01T00:00:00.000"]);

    // …and the partition is sealed one update window past its end.
    assert_eq!(
        partitions[0].seal_at.as_deref(),
        Some("2021-01-02T23:59:59.999Z")
    );
}

// ----------------------------------------------------------------------------
// Loading
// ----------------------------------------------------------------------------

#[tokio::test]
async fn a_partitioned_pre_aggregation_builds_every_partition_and_unions_them() {
    let driver = FakeDriver::new();
    with_range(
        &driver,
        "2021-01-01T00:00:00.000",
        "2021-01-03T10:00:00.000",
    );

    let orchestrator = orchestrator(driver.clone(), None, QueryOrchestratorOptions::default());
    let loaded = orchestrator
        .pre_aggregations()
        .load_all_pre_aggregations_if_needed(&query_body(partitioned_description()))
        .await
        .unwrap();

    assert_eq!(loaded.tables.len(), 1);
    let (table_name, result) = &loaded.tables[0];

    assert_eq!(table_name, "stb_pre_aggregations.orders_main");
    assert!(result.is_multi_table_union);
    assert_eq!(result.last_updated_at, Some(1_600_000_000_000));
    assert_eq!(result.pre_aggregation_id.as_deref(), Some("Orders.main"));

    // Three physical tables, one per day, joined into the subquery the outer SQL reads.
    let built: Vec<String> = driver
        .tables()
        .into_iter()
        .filter(|table| table.starts_with("orders_main2021"))
        .collect();
    assert_eq!(built.len(), 3, "{built:?}");

    assert!(result.target_table_name.starts_with('('));
    assert!(result.target_table_name.ends_with(')'));
    assert_eq!(
        result.target_table_name.matches(" UNION ALL ").count(),
        2,
        "{}",
        result.target_table_name
    );
    for day in ["20210101", "20210102", "20210103"] {
        assert!(
            result
                .target_table_name
                .contains(&format!("stb_pre_aggregations.orders_main{day}_")),
            "{}",
            result.target_table_name
        );
    }

    // Every partition was built with its own bounds, the last one clipped to the build range.
    let params = driver.params_of("CREATE TABLE stb_pre_aggregations.orders_main2021");
    assert_eq!(
        params,
        vec![
            vec![
                json!("2021-01-01T00:00:00.000"),
                json!("2021-01-01T23:59:59.999")
            ],
            vec![
                json!("2021-01-02T00:00:00.000"),
                json!("2021-01-02T23:59:59.999")
            ],
            vec![
                json!("2021-01-03T00:00:00.000"),
                json!("2021-01-03T10:00:00.000")
            ],
        ]
    );

    // One partition is a plain table name rather than a union.
    let single = orchestrator
        .pre_aggregations()
        .load_all_pre_aggregations_if_needed(&query_body(PreAggregationDescription {
            matched_time_dimension_date_range: Some(vec![
                "2021-01-02T00:00:00.000".to_string(),
                "2021-01-02T23:59:59.999".to_string(),
            ]),
            ..partitioned_description()
        }))
        .await
        .unwrap();

    let result = &single.tables[0].1;
    assert!(!result.is_multi_table_union);
    assert!(result
        .target_table_name
        .starts_with("stb_pre_aggregations.orders_main20210102_"));
}

#[tokio::test]
async fn a_query_served_by_partitions_reads_the_union() {
    let driver = FakeDriver::new();
    with_range(
        &driver,
        "2021-01-01T00:00:00.000",
        "2021-01-02T23:59:59.999",
    );

    let orchestrator = orchestrator(driver.clone(), None, QueryOrchestratorOptions::default());
    let result = orchestrator
        .fetch_query(&query_body(partitioned_description()))
        .await
        .unwrap()
        .into_result()
        .unwrap();

    let used = result
        .used_pre_aggregations
        .get("stb_pre_aggregations.orders_main")
        .expect("the pre-aggregation is reported");

    assert!(used
        .target_table_name
        .as_deref()
        .unwrap()
        .contains(" UNION ALL "));

    // The query the driver ran names the union, not the logical pre-aggregation.
    assert!(driver.statements().iter().any(|sql| sql
        .starts_with("SELECT * FROM (SELECT * FROM stb_pre_aggregations.orders_main20210101_")));
}

#[tokio::test]
async fn external_refresh_refuses_partitions_that_were_never_built() {
    let driver = FakeDriver::new();
    with_range(
        &driver,
        "2021-01-01T00:00:00.000",
        "2021-01-02T23:59:59.999",
    );

    let orchestrator = orchestrator(
        driver.clone(),
        None,
        QueryOrchestratorOptions {
            pre_aggregations_options: PreAggregationsOptions {
                external_refresh: true,
                ..Default::default()
            },
            ..Default::default()
        },
    );

    let error = orchestrator
        .pre_aggregations()
        .load_all_pre_aggregations_if_needed(&query_body(partitioned_description()))
        .await
        .unwrap_err();

    assert!(
        error
            .to_string()
            .starts_with("No pre-aggregation partitions were built yet"),
        "{error}"
    );
    // The message names the partitions it looked for, not the logical pre-aggregation.
    assert!(error.to_string().contains("orders_main20210101_*_"));
    // Nothing was built.
    assert!(driver.tables().is_empty());
}

#[tokio::test]
async fn usage_mapping_points_each_usage_at_the_partitions_it_needs() {
    let driver = FakeDriver::new();
    with_range(
        &driver,
        "2021-01-01T00:00:00.000",
        "2021-01-03T23:59:59.999",
    );

    let orchestrator = orchestrator(driver.clone(), None, QueryOrchestratorOptions::default());

    let mut description = partitioned_description();
    description.usage_mapping = Some(
        serde_json::from_value(json!({
            "__first": { "dateRange": ["2021-01-01T00:00:00.000", "2021-01-01T23:59:59.999"] },
            "__all": {},
        }))
        .unwrap(),
    );

    let loaded = orchestrator
        .pre_aggregations()
        .load_all_pre_aggregations_if_needed(&query_body(description))
        .await
        .unwrap();

    let names: HashMap<String, String> = loaded
        .tables
        .iter()
        .map(|(table_name, result)| (table_name.clone(), result.target_table_name.clone()))
        .collect();

    assert_eq!(names.len(), 2);
    // The narrow usage reads one partition…
    let first = &names["stb_pre_aggregations.orders_main__first"];
    assert!(
        first.starts_with("stb_pre_aggregations.orders_main20210101_") && !first.contains("UNION"),
        "{first}"
    );
    // …while the one without a range reads them all.
    assert!(names["stb_pre_aggregations.orders_main__all"].contains(" UNION ALL "));
}

#[tokio::test]
async fn the_build_range_replaces_the_query_values() {
    let driver = FakeDriver::new();
    with_range(
        &driver,
        "2021-01-01T00:00:00.000",
        "2021-01-02T10:00:00.000",
    );

    let orchestrator = orchestrator(driver.clone(), None, QueryOrchestratorOptions::default());

    let mut body = query_body(partitioned_description());
    body.values = Some(vec![
        "keep me".to_string(),
        cubeorch::BUILD_RANGE_START_LOCAL.to_string(),
        cubeorch::BUILD_RANGE_END_LOCAL.to_string(),
    ]);

    let loaded = orchestrator
        .pre_aggregations()
        .load_all_pre_aggregations_if_needed(&body)
        .await
        .unwrap();

    assert_eq!(
        loaded.values,
        Some(vec![
            "keep me".to_string(),
            "2021-01-01T00:00:00.000".to_string(),
            "2021-01-02T10:00:00.000".to_string(),
        ])
    );
}

#[tokio::test]
async fn an_unpartitioned_description_still_goes_through_the_partition_loader() {
    let driver = FakeDriver::new();

    let orchestrator = orchestrator(driver.clone(), None, QueryOrchestratorOptions::default());

    let mut description = partitioned_description();
    description.partition_granularity = None;
    description.pre_aggregation_start_end_queries = None;
    description.load_sql = Some(QueryWithParams::new(
        "CREATE TABLE stb_pre_aggregations.orders_main AS SELECT 1",
        vec![],
    ));

    let loaded = orchestrator
        .pre_aggregations()
        .load_all_pre_aggregations_if_needed(&query_body(description))
        .await
        .unwrap();

    let result = &loaded.tables[0].1;
    assert!(!result.is_multi_table_union);
    assert!(result
        .target_table_name
        .starts_with("stb_pre_aggregations.orders_main_"));
    // No range query was needed.
    assert!(!driver.statements().iter().any(|sql| sql == MIN_QUERY));
}

#[tokio::test]
async fn lambda_rollups_are_refused_rather_than_mis_served() {
    let driver = FakeDriver::new();
    with_range(
        &driver,
        "2021-01-01T00:00:00.000",
        "2021-01-01T23:59:59.999",
    );

    let orchestrator = orchestrator(driver, None, QueryOrchestratorOptions::default());

    let mut description = partitioned_description();
    description
        .extra
        .insert("rollupLambdaId".to_string(), json!("Orders.lambda"));

    let error = orchestrator
        .pre_aggregations()
        .load_all_pre_aggregations_if_needed(&query_body(description))
        .await
        .unwrap_err();

    assert!(matches!(error, OrchError::NotImplemented(_)));
    assert!(error.to_string().contains("Lambda rollups"));
}

// ----------------------------------------------------------------------------
// External build strategies
// ----------------------------------------------------------------------------

fn external_description() -> PreAggregationDescription {
    PreAggregationDescription {
        table_name: "stb_pre_aggregations.orders_ext".to_string(),
        pre_aggregation_id: Some("Orders.ext".to_string()),
        pre_aggregations_schema: Some("stb_pre_aggregations".to_string()),
        external: Some(true),
        unique_key_columns: Some(vec!["id".to_string()]),
        aggregations_columns: Some(vec!["sum(amount)".to_string()]),
        load_sql: Some(QueryWithParams::new(
            "CREATE TABLE stb_pre_aggregations.orders_ext AS SELECT 1",
            vec![],
        )),
        sql: Some(QueryWithParams::new("SELECT 1 AS id", vec![])),
        indexes_sql: Some(json!([{
            "indexName": "stb_pre_aggregations.orders_ext_main",
            "sql": ["CREATE INDEX stb_pre_aggregations.orders_ext_main ON stb_pre_aggregations.orders_ext (id)", []],
        }])),
        create_table_indexes: Some(json!([{
            "indexName": "stb_pre_aggregations.orders_ext_agg",
            "type": "aggregate",
            "columns": ["id"],
        }])),
        seal_at: Some("2021-01-02T00:00:00.000Z".to_string()),
        ..Default::default()
    }
}

#[tokio::test]
async fn the_write_strategy_uploads_through_a_temp_table() {
    let source = FakeDriver::new();
    let external = FakeDriver::new();

    let orchestrator = orchestrator(
        source.clone(),
        Some(external.clone()),
        QueryOrchestratorOptions::default(),
    );

    let loaded = orchestrator
        .pre_aggregations()
        .load_all_pre_aggregations_if_needed(&query_body(external_description()))
        .await
        .unwrap();

    let result = &loaded.tables[0].1;

    // The version entry the loader served comes from the *external* store's listing.
    assert!(
        result
            .target_table_name
            .starts_with("stb_pre_aggregations.orders_ext_"),
        "{}",
        result.target_table_name
    );
    assert_eq!(result.last_updated_at, Some(1_600_000_000_000));

    let uploads = external.uploads();
    assert_eq!(uploads.len(), 1);
    let upload = &uploads[0];

    assert_eq!(upload.table, result.target_table_name);
    assert_eq!(upload.unique_key_columns, vec!["id".to_string()]);
    assert_eq!(upload.aggregations_columns, vec!["sum(amount)".to_string()]);
    assert_eq!(upload.seal_at.as_deref(), Some("2021-01-02T00:00:00.000Z"));
    assert_eq!(upload.rows, 1);
    // The index statements name the physical tables, and the index carries the same version
    // suffix as the table it belongs to.
    assert_eq!(upload.indexes.len(), 1);
    assert!(upload.indexes[0].contains(&result.target_table_name));
    assert_eq!(upload.create_table_indexes.len(), 1);
    assert!(upload.create_table_indexes[0].starts_with("stb_pre_aggregations.orders_ext_agg_"));

    // The source first materialized the temp table, read it back, and then dropped it again.
    let statements = source.statements();
    assert!(statements
        .iter()
        .any(|sql| sql.starts_with("CREATE TABLE stb_pre_aggregations.orders_ext_")));
    assert!(statements
        .iter()
        .any(|sql| sql.starts_with("SELECT * FROM stb_pre_aggregations.orders_ext_")));
    assert!(statements
        .iter()
        .any(|sql| sql.starts_with("DROP TABLE stb_pre_aggregations.orders_ext_")));
    assert!(source.tables().is_empty(), "{:?}", source.tables());

    // Nothing was written to the source's own pre-aggregation schema in the end, but the
    // external store keeps the table.
    assert_eq!(external.tables().len(), 1);
}

#[tokio::test]
async fn the_read_only_strategy_uploads_the_query_result_directly() {
    let source = FakeDriver::read_only();
    let external = FakeDriver::new();

    source.answer(
        "SELECT 1 AS id",
        &["id"],
        vec![vec![json!("1")], vec![json!("2")]],
    );

    let orchestrator = orchestrator(
        source.clone(),
        Some(external.clone()),
        QueryOrchestratorOptions::default(),
    );

    let loaded = orchestrator
        .pre_aggregations()
        .load_all_pre_aggregations_if_needed(&query_body(external_description()))
        .await
        .unwrap();

    let result = &loaded.tables[0].1;
    let uploads = external.uploads();

    assert_eq!(uploads.len(), 1);
    assert_eq!(uploads[0].table, result.target_table_name);
    assert_eq!(uploads[0].columns, vec!["id".to_string()]);
    assert_eq!(uploads[0].rows, 2);

    // A read only source is never written to: no temp table, no drop.
    let statements = source.statements();
    assert!(statements.iter().any(|sql| sql == "SELECT 1 AS id"));
    assert!(
        !statements
            .iter()
            .any(|sql| sql.starts_with("CREATE TABLE") || sql.starts_with("DROP TABLE")),
        "{statements:?}"
    );
}

#[tokio::test]
async fn a_failed_external_build_keeps_the_table_used_key() {
    let source = FakeDriver::new();
    let external = FakeDriver::new();

    let orchestrator = orchestrator(
        source.clone(),
        Some(external.clone()),
        QueryOrchestratorOptions::default(),
    );

    // No external driver for the upload would be the usual failure; here the description has
    // no `sql`, which the read only strategy needs.
    let mut description = external_description();
    description.read_only = Some(true);
    description.sql = None;

    let error = orchestrator
        .pre_aggregations()
        .load_all_pre_aggregations_if_needed(&query_body(description))
        .await
        .unwrap_err();

    assert!(error.to_string().contains("carries no sql"), "{error}");
    assert!(external.uploads().is_empty());
}

#[tokio::test]
async fn an_external_pre_aggregation_without_an_external_driver_is_refused() {
    let source = FakeDriver::new();

    let orchestrator = orchestrator(source, None, QueryOrchestratorOptions::default());

    let error = orchestrator
        .pre_aggregations()
        .load_all_pre_aggregations_if_needed(&query_body(external_description()))
        .await
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("externalDriverFactory is not provided"),
        "{error}"
    );
}

/// An external build whose source driver unloads to CSV files.
///
/// The branch is selected exactly as Node selects it: the external store
/// imports CSV and the source supports unloading. It used to be refused
/// because the upload only took in-memory rows; the driver trait now takes the
/// download itself, so Cube Store imports the files natively and any other
/// store collects them.
#[tokio::test]
async fn an_external_build_uploads_unloaded_csv_files() {
    let dir = tempfile::tempdir().expect("temp dir");
    let csv_path = dir.path().join("part-0.csv");
    std::fs::write(&csv_path, "id,status\n1,shipped\n2,pending\n").expect("write csv");

    // A read-only source unloads the query straight to the export bucket, so
    // no temp table is involved.
    let source = FakeDriver::read_only_unloading_to_csv(cubedriver::types::TableCsvData {
        csv_file: vec![csv_path.display().to_string()],
        // A real unload reports the column types alongside the files.
        types: Some(vec![
            cubedriver::Column::new("id", "int"),
            cubedriver::Column::new("status", "string"),
        ]),
        csv_no_header: false,
        csv_delimiter: None,
        csv_disable_quoting: false,
        export_bucket_csv_escape_symbol: None,
    });
    // The external store advertises CSV import, which is what picks the branch.
    let external = FakeDriver::importing_csv();

    let orchestrator = orchestrator(
        source.clone(),
        Some(external.clone()),
        QueryOrchestratorOptions::default(),
    );

    let loaded = orchestrator
        .pre_aggregations()
        .load_all_pre_aggregations_if_needed(&query_body(external_description()))
        .await
        .expect("the CSV build succeeds");

    let result = &loaded.tables[0].1;
    let uploads = external.uploads();

    assert_eq!(uploads.len(), 1, "{uploads:?}");
    assert_eq!(uploads[0].table, result.target_table_name);
    // The two CSV rows reached the external store.
    assert_eq!(uploads[0].rows, 2);
    assert_eq!(
        uploads[0].columns,
        vec!["id".to_string(), "status".to_string()]
    );

    // A read-only source is never written to.
    let statements = source.statements();
    assert!(
        !statements
            .iter()
            .any(|sql| sql.starts_with("CREATE TABLE") || sql.starts_with("DROP TABLE")),
        "{statements:?}"
    );
}
