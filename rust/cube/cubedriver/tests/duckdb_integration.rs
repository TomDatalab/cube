#![cfg(feature = "duckdb")]
//! DuckDB driver tests against real in-memory / temp-file databases (no
//! Docker needed). Ports `packages/cubejs-duckdb-driver/test/DuckDBDriver.test.ts`
//! plus the driver's configuration and introspection overrides.

use std::collections::HashMap;

use cubedriver::duckdb::{DuckDbConfig, DuckDbDriver};
use cubedriver::{Column, Driver, DriverError, QueryOptions, QueryResult, StreamOptions};
use futures::TryStreamExt;
use serde_json::{json, Value};

fn memory_driver() -> DuckDbDriver {
    DuckDbDriver::new(DuckDbConfig::default()).unwrap()
}

async fn q(driver: &DuckDbDriver, sql: &str, params: &[Value]) -> QueryResult {
    driver
        .query(sql, params, &QueryOptions::default())
        .await
        .unwrap()
}

fn objects(columns: &[Column], rows: &[Vec<Value>]) -> Vec<HashMap<String, Value>> {
    rows.iter()
        .map(|r| {
            columns
                .iter()
                .zip(r)
                .map(|(c, v)| (c.name.clone(), v.clone()))
                .collect()
        })
        .collect()
}

fn expected_rows() -> Vec<HashMap<String, Value>> {
    [
        (
            "1",
            "2020-01-01T01:01:01.111Z",
            "2020-01-01T00:00:00.000Z",
            "100",
        ),
        (
            "2",
            "2020-02-02T02:02:02.222Z",
            "2020-02-02T00:00:00.000Z",
            "200",
        ),
        (
            "3",
            "2020-03-03T03:03:03.333Z",
            "2020-03-03T00:00:00.000Z",
            "300",
        ),
    ]
    .into_iter()
    .map(|(id, created, created_date, price)| {
        HashMap::from([
            ("id".to_string(), json!(id)),
            ("created".to_string(), json!(created)),
            ("created_date".to_string(), json!(created_date)),
            ("price".to_string(), json!(price)),
        ])
    })
    .collect()
}

/// `beforeAll` of the Node test.
async fn select_test_driver() -> DuckDbDriver {
    let driver = memory_driver();
    q(&driver, "CREATE SCHEMA IF NOT EXISTS test;", &[]).await;
    let columns = vec![
        Column::new("id", "bigint"),
        Column::new("created", "timestamp"),
        Column::new("created_date", "date"),
        Column::new("price", "decimal"),
    ];
    let data = QueryResult::new(
        columns.clone(),
        vec![
            vec![
                json!(1),
                json!("2020-01-01 01:01:01.11111"),
                json!("2020-01-01"),
                json!("100"),
            ],
            vec![
                json!(2),
                json!("2020-02-02 02:02:02.22222"),
                json!("2020-02-02"),
                json!("200"),
            ],
            vec![
                json!(3),
                json!("2020-03-03 03:03:03.33333"),
                json!("2020-03-03"),
                json!("300"),
            ],
        ],
    );
    driver
        .upload_table("test.select_test", &columns, &data)
        .await
        .unwrap();
    driver
}

#[tokio::test]
async fn query() {
    let driver = select_test_driver().await;
    let result = q(
        &driver,
        "select * from test.select_test ORDER BY id ASC",
        &[],
    )
    .await;
    assert_eq!(objects(&result.columns, &result.rows), expected_rows());
    driver.release().await.unwrap();
}

#[tokio::test]
async fn column_types() {
    let driver = select_test_driver().await;
    assert_eq!(
        driver.table_column_types("test.select_test").await.unwrap(),
        vec![
            Column::new("id", "bigint"),
            Column::new("created", "timestamp"),
            Column::new("created_date", "timestamp"),
            Column::new("price", "decimal(18,3)"),
        ]
    );
}

#[tokio::test]
async fn stream() {
    let driver = select_test_driver().await;
    let data = driver
        .stream(
            "select * from test.select_test ORDER BY id ASC",
            &[],
            &StreamOptions {
                high_water_mark: 1000,
                request_id: None,
            },
        )
        .await
        .unwrap();
    let columns = data.columns.clone();
    let rows: Vec<Vec<Value>> = data.rows.try_collect().await.unwrap();
    assert_eq!(objects(&columns, &rows), expected_rows());
}

#[tokio::test]
async fn stream_with_small_buffer_and_params() {
    let driver = memory_driver();
    let data = driver
        .stream(
            "SELECT range AS n FROM range(0, 50) WHERE range >= ?",
            &[json!(10)],
            &StreamOptions {
                high_water_mark: 1,
                request_id: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(data.columns, vec![Column::new("n", "bigint")]);
    let rows: Vec<Vec<Value>> = data.rows.try_collect().await.unwrap();
    assert_eq!(rows.len(), 40);
    assert_eq!(rows[0], vec![json!("10")]);

    // A dropped stream stops its worker; the driver keeps working.
    let data = driver
        .stream(
            "SELECT * FROM range(0, 100000)",
            &[],
            &StreamOptions::default(),
        )
        .await
        .unwrap();
    drop(data);
    driver.test_connection().await.unwrap();

    let err = driver
        .stream("SELECT * FROM missing", &[], &StreamOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(err, DriverError::Database { .. }), "{err:?}");
}

#[tokio::test]
async fn value_transformation() {
    let driver = memory_driver();
    let result = q(
        &driver,
        "SELECT 1::INTEGER AS i, 2.5::DOUBLE AS d, 1.25::DECIMAL(10,2) AS dec,
                'x' AS s, true AS b, NULL AS n,
                DATE '2021-05-06' AS dt, TIMESTAMPTZ '2021-05-06 07:08:09.123456+00' AS tz,
                170141183460469231731687303715884105727::HUGEINT AS h,
                [1, 2] AS l, {'a': 1} AS st, TIME '01:02:03' AS t",
        &[],
    )
    .await;
    assert_eq!(
        result.rows[0],
        vec![
            json!("1"),
            json!("2.5"),
            json!("1.25"),
            json!("x"),
            json!(true),
            Value::Null,
            json!("2021-05-06T00:00:00.000Z"),
            json!("2021-05-06T07:08:09.123Z"),
            json!("170141183460469231731687303715884105727"),
            json!([1, 2]),
            json!({"a": 1}),
            json!("01:02:03"),
        ]
    );
    let types: Vec<String> = result.columns.iter().map(|c| c.type_.to_string()).collect();
    assert_eq!(types[0], "int");
    assert_eq!(types[1], "double");
    assert_eq!(types[2], "decimal(10, 2)");
    assert_eq!(types[3], "text");
    assert_eq!(types[4], "boolean");
    assert_eq!(types[6], "timestamp");
}

#[tokio::test]
async fn params_are_positional() {
    let driver = memory_driver();
    let result = q(
        &driver,
        "SELECT ?::VARCHAR AS a, CAST(? AS DOUBLE) + 1 AS b, ? IS NULL AS c",
        &[json!("x"), json!(1), Value::Null],
    )
    .await;
    assert_eq!(result.rows, vec![vec![json!("x"), json!("2"), json!(true)]]);
    assert_eq!(driver.param(3), "?");
}

#[tokio::test]
async fn introspection() {
    let driver = select_test_driver().await;
    let schema = driver.tables_schema().await.unwrap();
    let table = &schema["test"]["select_test"];
    let names: Vec<_> = table.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["created", "created_date", "id", "price"]);

    let schemas = driver.get_schemas().await.unwrap();
    assert!(schemas.iter().any(|s| s.schema_name == "test"));
    assert_eq!(
        driver.get_tables_query("test").await.unwrap(),
        vec!["select_test".to_string()]
    );
    driver.create_schema_if_not_exists("test").await.unwrap();
    driver.create_schema_if_not_exists("other").await.unwrap();
    assert!(driver
        .get_schemas()
        .await
        .unwrap()
        .iter()
        .all(|s| s.schema_name != "information_schema"));
}

#[tokio::test]
async fn schema_restricts_introspection_to_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cat.duckdb");
    let driver = DuckDbDriver::new(DuckDbConfig {
        database_path: Some(path.to_str().unwrap().to_string()),
        schema: Some("cat".to_string()),
        ..Default::default()
    })
    .unwrap();
    assert!(driver
        .information_schema_query()
        .ends_with("AND table_catalog = 'cat'"));
    q(&driver, "CREATE TABLE t (x INTEGER)", &[]).await;
    q(&driver, "ATTACH ':memory:' AS other", &[]).await;
    q(&driver, "CREATE TABLE other.main.u (y INTEGER)", &[]).await;

    let schema = driver.tables_schema().await.unwrap();
    assert!(schema["main"].contains_key("t"));
    assert!(!schema["main"].contains_key("u"));
    let schemas = driver.get_schemas().await.unwrap();
    assert_eq!(schemas.len(), 1);
    assert_eq!(schemas[0].schema_name, "main");
}

#[tokio::test]
async fn database_path_persists_and_init_sql_runs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db.duckdb").to_str().unwrap().to_string();
    let config = DuckDbConfig {
        database_path: Some(path.clone()),
        init_sql: Some("CREATE TABLE IF NOT EXISTS init_t AS SELECT 42 AS v".to_string()),
        memory_limit: Some("512MB".to_string()),
        ..Default::default()
    };
    let driver = DuckDbDriver::new(config.clone()).unwrap();
    assert_eq!(
        q(&driver, "SELECT v FROM init_t", &[]).await.rows,
        vec![vec![json!("42")]]
    );
    assert_eq!(
        q(&driver, "SELECT current_setting('memory_limit') AS m", &[])
            .await
            .rows[0][0]
            .as_str()
            .unwrap()
            .replace(' ', ""),
        "488.2MiB"
    );
    driver.release().await.unwrap();
    driver.release().await.unwrap();

    // Re-open after release; the file database kept the table.
    let driver = DuckDbDriver::new(config).unwrap();
    assert_eq!(
        q(&driver, "SELECT v FROM init_t", &[]).await.rows,
        vec![vec![json!("42")]]
    );
}

#[tokio::test]
async fn failing_init_sql_and_settings_are_skipped() {
    let driver = DuckDbDriver::new(DuckDbConfig {
        init_sql: Some("THIS IS NOT SQL".to_string()),
        s3_url_style: Some("no-such-style".to_string()),
        ..Default::default()
    })
    .unwrap();
    driver.test_connection().await.unwrap();
}

#[tokio::test]
async fn failing_extension_install_is_a_named_error_and_retried() {
    let driver = DuckDbDriver::new(DuckDbConfig {
        extensions: Some(vec!["cube_no_such_extension".to_string()]),
        ..Default::default()
    })
    .unwrap();
    for _ in 0..2 {
        let err = driver.test_connection().await.unwrap_err();
        assert!(matches!(err, DriverError::Connection { .. }), "{err:?}");
        assert!(
            err.to_string()
                .contains("DuckDB - error on installing cube_no_such_extension"),
            "{err}"
        );
    }
}

#[tokio::test]
async fn driver_properties() {
    let driver = memory_driver();
    assert!(!driver.read_only());
    assert_eq!(driver.quote_identifier("a"), "\"a\"");
    assert_eq!(driver.capabilities(), Default::default());
    assert_eq!(
        driver.wrap_query_with_limit("SELECT 1", 5),
        cubedriver::sql::wrap_query_with_limit("SELECT 1", 5)
    );
    let limited = q(
        &driver,
        &driver.wrap_query_with_limit("SELECT * FROM range(10)", 3),
        &[],
    )
    .await;
    assert_eq!(limited.len(), 3);
}
