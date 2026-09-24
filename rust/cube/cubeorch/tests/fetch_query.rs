//! `fetch_query` end to end against an in-memory `Driver`.
//!
//! Mirrors the scenarios of `packages/cubejs-query-orchestrator/test/unit/QueryOrchestrator.test.js`
//! and `QueryCache.abstract.ts` that this port covers.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use async_trait::async_trait;
use cubedriver::{
    Column, Driver, DriverConfig, DriverError, GenericType, QueryOptions, QueryResult,
};
use cubeorch::{
    api::LoadOutcome,
    cache::QueryCacheOptions,
    preaggs::PreAggregationsOptions,
    types::{CacheKeyQueries, CacheMode, PreAggregationDescription, QueryWithParams},
    DriverFactory, FetchQueryOutcome, OrchError, OrchestratorApi, QueryBody, QueryOrchestrator,
    QueryOrchestratorOptions,
};
use serde_json::{json, Value};

// ----------------------------------------------------------------------------
// A driver whose answers are programmed per SQL statement.
// ----------------------------------------------------------------------------

#[derive(Default)]
struct FakeDriverState {
    /// `sql -> rows`, each row a list of cells matched to `columns`.
    answers: HashMap<String, (Vec<String>, Vec<Vec<Value>>)>,
    /// SQL statements this driver was asked to run, in order.
    executed: Vec<String>,
    /// Tables of the pre-aggregation schema.
    tables: Vec<String>,
    /// SQL that fails with this message instead of answering.
    failures: HashMap<String, String>,
    /// SQL that blocks for this long before answering.
    delays: HashMap<String, Duration>,
}

struct FakeDriver {
    config: DriverConfig,
    state: Mutex<FakeDriverState>,
    queries: AtomicUsize,
    now: Mutex<i64>,
}

impl FakeDriver {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            config: DriverConfig::default(),
            state: Mutex::new(FakeDriverState::default()),
            queries: AtomicUsize::new(0),
            now: Mutex::new(1_600_000_000_000),
        })
    }

    fn answer(&self, sql: &str, columns: &[&str], rows: Vec<Vec<Value>>) {
        self.state.lock().unwrap().answers.insert(
            sql.to_string(),
            (columns.iter().map(|c| c.to_string()).collect(), rows),
        );
    }

    fn fail(&self, sql: &str, message: &str) {
        self.state
            .lock()
            .unwrap()
            .failures
            .insert(sql.to_string(), message.to_string());
    }

    fn clear_failure(&self, sql: &str) {
        self.state.lock().unwrap().failures.remove(sql);
    }

    fn delay(&self, sql: &str, duration: Duration) {
        self.state
            .lock()
            .unwrap()
            .delays
            .insert(sql.to_string(), duration);
    }

    fn executed(&self) -> Vec<String> {
        self.state.lock().unwrap().executed.clone()
    }

    fn times_executed(&self, sql: &str) -> usize {
        self.executed().iter().filter(|s| *s == sql).count()
    }

    fn set_tables(&self, tables: &[&str]) {
        self.state.lock().unwrap().tables = tables.iter().map(|t| t.to_string()).collect();
    }

    fn tables(&self) -> Vec<String> {
        self.state.lock().unwrap().tables.clone()
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

    fn now_timestamp(&self) -> i64 {
        *self.now.lock().unwrap()
    }

    /// A build registers the table it created, the way a real `CREATE TABLE` would show up
    /// in the next schema listing.
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

    async fn query(
        &self,
        sql: &str,
        _params: &[Value],
        _options: &QueryOptions,
    ) -> Result<QueryResult, DriverError> {
        self.queries.fetch_add(1, Ordering::SeqCst);

        let (failure, delay, answer) = {
            let mut state = self.state.lock().unwrap();
            state.executed.push(sql.to_string());

            (
                state.failures.get(sql).cloned(),
                state.delays.get(sql).cloned(),
                state.answers.get(sql).cloned(),
            )
        };

        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }

        if let Some(failure) = failure {
            return Err(DriverError::Query(failure));
        }

        // `SELECT table_name FROM information_schema.tables ...` of the base driver.
        if sql.contains("information_schema.tables") {
            return Ok(QueryResult::new(
                vec![Column::new("table_name", GenericType::String)],
                self.tables()
                    .into_iter()
                    .map(|table| vec![Value::String(table)])
                    .collect(),
            ));
        }

        if sql.contains("information_schema.schemata") {
            return Ok(QueryResult::new(
                vec![Column::new("schema_name", GenericType::String)],
                vec![vec![Value::String("stb_pre_aggregations".into())]],
            ));
        }

        let (columns, rows) =
            answer.unwrap_or_else(|| (vec!["ok".to_string()], vec![vec![json!(1)]]));

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

fn orchestrator(driver: Arc<FakeDriver>) -> Arc<QueryOrchestrator> {
    orchestrator_with(driver, QueryOrchestratorOptions::default())
}

fn orchestrator_with(
    driver: Arc<FakeDriver>,
    options: QueryOrchestratorOptions,
) -> Arc<QueryOrchestrator> {
    QueryOrchestrator::new("test", factory(driver), None, Arc::new(|_, _| {}), options)
}

fn body(query: &str) -> QueryBody {
    QueryBody {
        query: Some(query.to_string()),
        values: Some(vec![]),
        data_source: Some("default".to_string()),
        ..Default::default()
    }
}

fn with_refresh_key(mut body: QueryBody, sql: &str) -> QueryBody {
    body.cache_key_queries = Some(CacheKeyQueries::List(vec![QueryWithParams::new(
        sql,
        vec![],
    )]));
    body
}

fn data_of(outcome: FetchQueryOutcome) -> Value {
    outcome.into_result().expect("a data result").data
}

/// Gives a detached background refresh a chance to run before the assertions.
async fn settle() {
    for _ in 0..50 {
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

// ----------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------

#[tokio::test]
async fn cold_query_runs_against_the_driver_and_returns_its_rows() {
    let driver = FakeDriver::new();
    driver.answer("SELECT 1", &["count"], vec![vec![json!(42)]]);

    let orchestrator = orchestrator(driver.clone());
    let result = orchestrator
        .fetch_query(&body("SELECT 1"))
        .await
        .unwrap()
        .into_result()
        .unwrap();

    assert_eq!(result.data, json!([{ "count": 42 }]));
    assert_eq!(result.data_source.as_deref(), Some("default"));
    assert!(result.used_pre_aggregations.is_empty());
    // A cold query has a refresh key list (empty here) and a cache entry to date it.
    assert_eq!(result.refresh_key_values, Some(vec![]));
    assert!(result.last_refresh_time.is_some());
    assert_eq!(driver.times_executed("SELECT 1"), 1);
}

#[tokio::test]
async fn a_second_identical_query_is_served_from_the_cache() {
    let driver = FakeDriver::new();
    driver.answer("SELECT 2", &["count"], vec![vec![json!(7)]]);
    driver.answer("SELECT MAX(id) FROM t", &["max"], vec![vec![json!("1")]]);

    let orchestrator = orchestrator(driver.clone());
    let query = with_refresh_key(body("SELECT 2"), "SELECT MAX(id) FROM t");

    let first = data_of(orchestrator.fetch_query(&query).await.unwrap());
    let second = data_of(orchestrator.fetch_query(&query).await.unwrap());

    assert_eq!(first, json!([{ "count": 7 }]));
    assert_eq!(second, first);
    // The refresh key did not move, so the main query ran exactly once.
    assert_eq!(driver.times_executed("SELECT 2"), 1);
}

#[tokio::test]
async fn a_changed_refresh_key_renews_the_result() {
    let driver = FakeDriver::new();
    driver.answer("SELECT 3", &["count"], vec![vec![json!(1)]]);
    driver.answer("SELECT MAX(id) FROM t", &["max"], vec![vec![json!("1")]]);

    let orchestrator = orchestrator(driver.clone());

    // A refresh key that is re-read every second rather than every two minutes.
    let mut query = body("SELECT 3");
    query.cache_key_queries = Some(CacheKeyQueries::List(vec![QueryWithParams::new(
        "SELECT MAX(id) FROM t",
        vec![],
    )
    .with_options(cubeorch::RefreshKeyQueryOptions {
        renewal_threshold: Some(1),
        ..Default::default()
    })]));

    let mut first = query.clone();
    first.request_id = Some("req-a".to_string());
    assert_eq!(
        data_of(orchestrator.fetch_query(&first).await.unwrap()),
        json!([{ "count": 1 }])
    );
    assert_eq!(driver.times_executed("SELECT 3"), 1);

    // The source moves on.
    driver.answer("SELECT MAX(id) FROM t", &["max"], vec![vec![json!("2")]]);
    driver.answer("SELECT 3", &["count"], vec![vec![json!(2)]]);
    tokio::time::sleep(Duration::from_millis(1200)).await;

    // `skipRefreshKeyWaitForRenew` serves the stale key once and refreshes it behind the
    // request, so this pass still answers from the cache.
    let mut second = query.clone();
    second.request_id = Some("req-b".to_string());
    assert_eq!(
        data_of(orchestrator.fetch_query(&second).await.unwrap()),
        json!([{ "count": 1 }])
    );

    settle().await;

    // The next request sees the new refresh key value, so the renewal key no longer matches
    // the cache entry and the main query is re-run.
    let mut third = query.clone();
    third.request_id = Some("req-c".to_string());
    assert_eq!(
        data_of(orchestrator.fetch_query(&third).await.unwrap()),
        json!([{ "count": 2 }])
    );
    assert_eq!(driver.times_executed("SELECT 3"), 2);
}

#[tokio::test]
async fn a_slow_query_answers_continue_wait_and_then_converges() {
    let driver = FakeDriver::new();
    driver.answer("SELECT slow", &["count"], vec![vec![json!(5)]]);
    driver.delay("SELECT slow", Duration::from_millis(400));

    let orchestrator = orchestrator(driver.clone());
    // One second of patience, so the 400 ms query outruns the first poll but not the queue.
    let api = OrchestratorApi::new(orchestrator).with_continue_wait_timeout(0);

    let mut query = body("SELECT slow");
    query.request_id = Some("req-1-span-1".to_string());

    // The first poll times out before the driver answers.
    let first = api.execute_query(&query).await.unwrap();
    assert!(matches!(first, LoadOutcome::ContinueWait { .. }));

    // The client re-issues the identical request; once the build lands, it gets the result.
    let api = OrchestratorApi::new(orchestrator_with(
        driver.clone(),
        QueryOrchestratorOptions::default(),
    ))
    .with_continue_wait_timeout(30);

    let mut second = query.clone();
    second.request_id = Some("req-1-span-2".to_string());

    let outcome = api.execute_query(&second).await.unwrap();

    match outcome {
        LoadOutcome::Result(result) => assert_eq!(result.data, json!([{ "count": 5 }])),
        other => panic!("expected a result, got {other:?}"),
    }
}

#[tokio::test]
async fn a_failing_refresh_key_does_not_fail_the_query() {
    let driver = FakeDriver::new();
    driver.answer("SELECT 4", &["count"], vec![vec![json!(9)]]);
    driver.fail("SELECT MAX(id) FROM t", "refresh key is broken");

    let orchestrator = orchestrator(driver.clone());
    let query = with_refresh_key(body("SELECT 4"), "SELECT MAX(id) FROM t");

    // `renewQuery` logs the failure and renews against an empty key list.
    let result = data_of(orchestrator.fetch_query(&query).await.unwrap());

    assert_eq!(result, json!([{ "count": 9 }]));

    driver.clear_failure("SELECT MAX(id) FROM t");
}

#[tokio::test]
async fn rollup_only_mode_refuses_a_query_without_a_pre_aggregation() {
    let driver = FakeDriver::new();
    let orchestrator = orchestrator_with(
        driver,
        QueryOrchestratorOptions {
            rollup_only_mode: true,
            ..Default::default()
        },
    );

    let error = orchestrator
        .fetch_query(&body("SELECT 5"))
        .await
        .unwrap_err();

    assert_eq!(
        error,
        OrchError::Orchestration(cubeorch::ROLLUP_ONLY_MESSAGE.to_string())
    );
    assert!(error
        .to_string()
        .starts_with("No pre-aggregation table has been built for this query yet."));
}

#[tokio::test]
async fn a_query_served_by_a_pre_aggregation_reports_it() {
    let driver = FakeDriver::new();
    // The refresh worker already built this partition; the structure version is the golden
    // one of `get_structure_version(['CREATE TABLE ... ', []])`.
    let structure_version = cubeorch::get_structure_version(&PreAggregationDescription {
        table_name: "stb_pre_aggregations.orders_main".to_string(),
        load_sql: Some(QueryWithParams::new(
            "CREATE TABLE stb_pre_aggregations.orders_main AS SELECT 1",
            vec![],
        )),
        ..Default::default()
    });
    let table = format!("orders_main_abcdefgh_{structure_version}_1fm6652");
    driver.set_tables(&[&table]);
    driver.answer(
        "SELECT * FROM stb_pre_aggregations.orders_main",
        &["count"],
        vec![vec![json!(3)]],
    );
    driver.answer(
        &format!("SELECT * FROM stb_pre_aggregations.{table}"),
        &["count"],
        vec![vec![json!(3)]],
    );

    let orchestrator = orchestrator_with(
        driver.clone(),
        QueryOrchestratorOptions {
            // The refresh worker owns the tables; this instance only serves them.
            pre_aggregations_options: PreAggregationsOptions {
                external_refresh: true,
                ..Default::default()
            },
            query_cache_options: QueryCacheOptions::default(),
            ..Default::default()
        },
    );

    let mut query = body("SELECT * FROM stb_pre_aggregations.orders_main");
    query.pre_aggregations = vec![PreAggregationDescription {
        table_name: "stb_pre_aggregations.orders_main".to_string(),
        pre_aggregation_id: Some("Orders.main".to_string()),
        r#type: Some("rollup".to_string()),
        pre_aggregations_schema: Some("stb_pre_aggregations".to_string()),
        load_sql: Some(QueryWithParams::new(
            "CREATE TABLE stb_pre_aggregations.orders_main AS SELECT 1",
            vec![],
        )),
        ..Default::default()
    }];

    let result = orchestrator
        .fetch_query(&query)
        .await
        .unwrap()
        .into_result()
        .unwrap();

    let used = result
        .used_pre_aggregations
        .get("stb_pre_aggregations.orders_main")
        .expect("the pre-aggregation is reported");

    // The table name carries content, structure and the base32 timestamp of naming version 2.
    assert_eq!(
        used.target_table_name.as_deref(),
        Some(format!("stb_pre_aggregations.{table}").as_str())
    );
    assert_eq!(used.pre_aggregation_id.as_deref(), Some("Orders.main"));
    assert_eq!(used.r#type.as_deref(), Some("rollup"));
    assert_eq!(used.last_updated_at, Some(1600329890000));
    // The SQL that ran names the physical table, not the logical pre-aggregation.
    assert!(driver
        .executed()
        .iter()
        .any(|sql| sql == &format!("SELECT * FROM stb_pre_aggregations.{table}")));
    assert_eq!(result.data, json!([{ "count": 3 }]));
}

#[tokio::test]
async fn external_refresh_refuses_a_pre_aggregation_that_was_never_built() {
    let driver = FakeDriver::new();
    driver.set_tables(&[]);

    let orchestrator = orchestrator_with(
        driver,
        QueryOrchestratorOptions {
            pre_aggregations_options: PreAggregationsOptions {
                external_refresh: true,
                ..Default::default()
            },
            ..Default::default()
        },
    );

    let mut query = body("SELECT * FROM stb_pre_aggregations.orders_main");
    query.pre_aggregations = vec![PreAggregationDescription {
        table_name: "stb_pre_aggregations.orders_main".to_string(),
        pre_aggregations_schema: Some("stb_pre_aggregations".to_string()),
        load_sql: Some(QueryWithParams::new("CREATE TABLE x AS SELECT 1", vec![])),
        ..Default::default()
    }];

    let error = orchestrator.fetch_query(&query).await.unwrap_err();

    assert!(error
        .to_string()
        .starts_with("No pre-aggregation partitions were built yet"));
    assert!(error.to_string().contains("Expected table name patterns:"));
}

#[tokio::test]
async fn a_pre_aggregation_is_built_when_nothing_exists_yet() {
    let driver = FakeDriver::new();
    driver.set_tables(&[]);

    let orchestrator = orchestrator(driver.clone());

    let mut query = body("SELECT * FROM stb_pre_aggregations.orders_main");
    query.pre_aggregations = vec![PreAggregationDescription {
        table_name: "stb_pre_aggregations.orders_main".to_string(),
        pre_aggregation_id: Some("Orders.main".to_string()),
        pre_aggregations_schema: Some("stb_pre_aggregations".to_string()),
        load_sql: Some(QueryWithParams::new(
            "CREATE TABLE stb_pre_aggregations.orders_main AS SELECT 1",
            vec![],
        )),
        ..Default::default()
    }];

    // The build runs synchronously as part of answering the request.
    let result = orchestrator
        .fetch_query(&query)
        .await
        .unwrap()
        .into_result()
        .unwrap();

    let used = result
        .used_pre_aggregations
        .get("stb_pre_aggregations.orders_main")
        .expect("the pre-aggregation is reported");

    let structure_version = cubeorch::get_structure_version(&query.pre_aggregations[0]);
    let content_version = cubeorch::content_version(
        &query.pre_aggregations[0],
        &cubecache::KeyValue::Array(vec![]),
    );

    // The table the build created carries the content and structure versions of the
    // description and the base32 timestamp of naming version 2.
    assert_eq!(
        used.target_table_name.as_deref(),
        Some(
            format!(
                "stb_pre_aggregations.orders_main_{content_version}_{structure_version}_{}",
                cubeorch::preaggs::encode_time_stamp(1_600_000_000_000)
            )
            .as_str()
        )
    );
    assert_eq!(used.last_updated_at, Some(1600000000000));
    assert!(driver
        .executed()
        .iter()
        .any(|sql| sql.starts_with("CREATE TABLE stb_pre_aggregations.orders_main_")));
}

#[tokio::test]
async fn a_partitioned_pre_aggregation_builds_one_table_per_partition() {
    let driver = FakeDriver::new();
    driver.set_tables(&[]);
    driver.answer(
        "SELECT MIN(t) FROM orders",
        &["min"],
        vec![vec![json!("2021-01-01T00:00:00.000")]],
    );
    driver.answer(
        "SELECT MAX(t) FROM orders",
        &["max"],
        vec![vec![json!("2021-01-02T23:59:59.999")]],
    );

    let orchestrator = orchestrator(driver.clone());

    let mut query = body("SELECT * FROM stb_pre_aggregations.orders_main");
    query.pre_aggregations = vec![PreAggregationDescription {
        table_name: "stb_pre_aggregations.orders_main".to_string(),
        pre_aggregations_schema: Some("stb_pre_aggregations".to_string()),
        partition_granularity: Some("day".to_string()),
        timezone: Some("UTC".to_string()),
        load_sql: Some(QueryWithParams::new(
            "CREATE TABLE stb_pre_aggregations.orders_main AS SELECT 1",
            vec![],
        )),
        pre_aggregation_start_end_queries: Some(vec![
            QueryWithParams::new("SELECT MIN(t) FROM orders", vec![]),
            QueryWithParams::new("SELECT MAX(t) FROM orders", vec![]),
        ]),
        ..Default::default()
    }];

    let result = orchestrator
        .fetch_query(&query)
        .await
        .unwrap()
        .into_result()
        .unwrap();

    let used = result
        .used_pre_aggregations
        .get("stb_pre_aggregations.orders_main")
        .expect("the pre-aggregation is reported");

    // Two days, so the outer query reads the union of the two partition tables.
    assert!(used
        .target_table_name
        .as_deref()
        .unwrap()
        .contains(" UNION ALL "));
    assert_eq!(
        driver
            .tables()
            .iter()
            .filter(|table| table.starts_with("orders_main2021"))
            .count(),
        2
    );
}

#[tokio::test]
async fn a_build_only_request_reports_what_it_built() {
    let driver = FakeDriver::new();
    let orchestrator = orchestrator(driver);

    let outcome = orchestrator
        .fetch_query(&QueryBody {
            data_source: Some("default".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    match outcome {
        FetchQueryOutcome::BuildOnly {
            used_pre_aggregations,
            last_refresh_time,
        } => {
            assert!(used_pre_aggregations.is_empty());
            assert_eq!(last_refresh_time, None);
        }
        other => panic!("expected a build only outcome, got {other:?}"),
    }
}

#[tokio::test]
async fn a_slow_query_in_a_stale_cache_mode_is_served_from_the_cache_as_a_slow_query() {
    let driver = FakeDriver::new();
    driver.answer("SELECT stale", &["count"], vec![vec![json!(1)]]);

    let orchestrator = orchestrator(driver.clone());
    let query = body("SELECT stale");

    // Warm the cache.
    let api = OrchestratorApi::new(orchestrator.clone()).with_continue_wait_timeout(30);
    api.execute_query(&query).await.unwrap();

    // The same query, now slow and asked to bypass the cache, so it runs into continue wait.
    driver.delay("SELECT stale", Duration::from_millis(500));

    let mut stale = query.clone();
    stale.cache_mode = Some(CacheMode::StaleIfSlow);
    stale.force_no_cache = true;

    let api = OrchestratorApi::new(orchestrator).with_continue_wait_timeout(0);

    match api.execute_query(&stale).await.unwrap() {
        LoadOutcome::Result(result) => {
            assert!(result.slow_query);
            assert_eq!(result.data, json!([{ "count": 1 }]));
        }
        other => panic!("expected the stale cached result, got {other:?}"),
    }
}

#[tokio::test]
async fn a_scheduled_refresh_continue_wait_reports_no_stage() {
    let driver = FakeDriver::new();
    driver.answer("SELECT scheduled", &["count"], vec![vec![json!(1)]]);
    driver.delay("SELECT scheduled", Duration::from_millis(500));

    let api = OrchestratorApi::new(orchestrator(driver)).with_continue_wait_timeout(0);

    let mut query = body("SELECT scheduled");
    query.scheduled_refresh = true;

    assert_eq!(
        api.execute_query(&query).await.unwrap(),
        LoadOutcome::ContinueWait { stage: None }
    );
}
