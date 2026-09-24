//! Integration tests for [`ClickHouseDriver`], ported from
//! `packages/cubejs-clickhouse-driver/test`.
//!
//! They run only when `CUBEJS_TEST_CLICKHOUSE_URL` is set, e.g.
//! `CUBEJS_TEST_CLICKHOUSE_URL=http://test:test@127.0.0.1:8123/test`.

use cubedriver::{
    ClickHouseConfig, ClickHouseDriver, Column, DownloadQueryResultsOptions, DownloadedData,
    Driver, QueryOptions, QueryResult, SchemaTable, StreamOptions,
};
use futures::TryStreamExt;
use serde_json::{json, Value};

fn clickhouse_url() -> Option<String> {
    std::env::var("CUBEJS_TEST_CLICKHOUSE_URL")
        .ok()
        .filter(|s| !s.is_empty())
}

macro_rules! require_clickhouse {
    () => {
        match clickhouse_url() {
            Some(url) => url,
            None => {
                eprintln!("CUBEJS_TEST_CLICKHOUSE_URL is not set, skipping");
                return;
            }
        }
    };
}

fn driver(url: &str) -> ClickHouseDriver {
    let mut config = ClickHouseConfig::from_url(url).unwrap();
    config.read_only = Some(false);
    ClickHouseDriver::new(config).unwrap()
}

async fn query(driver: &ClickHouseDriver, sql: &str, params: &[Value]) -> QueryResult {
    driver
        .query(sql, params, &QueryOptions::default())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
}

#[tokio::test]
async fn test_connection_and_scalar_types() {
    let url = require_clickhouse!();
    let driver = driver(&url);
    driver.test_connection().await.unwrap();

    let data = query(
        &driver,
        "SELECT
           toDate('2020-01-01') AS d,
           toDateTime('2020-01-01 12:34:56', 'UTC') AS dt,
           toDateTime64('2020-01-01 12:34:56.789', 3, 'UTC') AS dt64,
           toInt32(1) AS i32,
           toInt64(-9007199254740993) AS i64,
           toFloat64(1.5) AS f64,
           toDecimal64('1.25', 2) AS dec,
           'foo' AS s,
           CAST(NULL AS Nullable(Int64)) AS n",
        &[],
    )
    .await;

    assert_eq!(data.len(), 1);
    // `dateConverter` / `DATE_TIME_CONVERTERS`
    assert_eq!(
        data.get_string(0, "d").as_deref(),
        Some("2020-01-01T00:00:00.000")
    );
    assert_eq!(
        data.get_string(0, "dt").as_deref(),
        Some("2020-01-01T12:34:56.000")
    );
    assert_eq!(
        data.get_string(0, "dt64").as_deref(),
        Some("2020-01-01T12:34:56.789")
    );
    // `numberConverter` stringifies every numeric type
    assert_eq!(data.get(0, "i32"), Some(&json!("1")));
    assert_eq!(data.get(0, "i64"), Some(&json!("-9007199254740993")));
    assert_eq!(data.get(0, "f64"), Some(&json!("1.5")));
    assert_eq!(data.get(0, "dec"), Some(&json!("1.25")));
    assert_eq!(data.get_string(0, "s").as_deref(), Some("foo"));
    assert_eq!(data.get(0, "n"), Some(&Value::Null));

    // ... and the column types come from `meta`
    assert_eq!(
        data.columns,
        vec![
            Column::new("d", "date"),
            Column::new("dt", "timestamp"),
            Column::new("dt64", "timestamp"),
            Column::new("i32", "int"),
            Column::new("i64", "bigint"),
            Column::new("f64", "double"),
            Column::new("dec", "decimal"),
            Column::new("s", "text"),
            Column::new("n", "bigint"),
        ]
    );
}

#[tokio::test]
async fn parameters_are_interpolated_and_escaped() {
    let url = require_clickhouse!();
    let driver = driver(&url);

    let data = query(
        &driver,
        "SELECT ? AS a, ? AS b, ? AS c",
        &[json!(1), json!("it's"), json!("x' OR 1=1 -- ")],
    )
    .await;
    assert_eq!(data.get(0, "a"), Some(&json!("1")));
    assert_eq!(data.get_string(0, "b").as_deref(), Some("it's"));
    assert_eq!(data.get_string(0, "c").as_deref(), Some("x' OR 1=1 -- "));
}

#[tokio::test]
async fn container_types_keep_their_json() {
    let url = require_clickhouse!();
    let driver = driver(&url);
    let data = query(
        &driver,
        "SELECT [1, 2] AS arr, map('a', 1) AS m, tuple(1, 'x') AS t",
        &[],
    )
    .await;
    // containers get no converter, so their JSON is passed through as it came
    assert_eq!(data.get(0, "arr"), Some(&json!([1, 2])));
    assert_eq!(
        data.columns,
        vec![
            Column::new("arr", "int[]"),
            Column::new("m", "text"),
            Column::new("t", "text"),
        ]
    );
}

#[tokio::test]
async fn create_upload_and_introspect_a_table() {
    let url = require_clickhouse!();
    let driver = driver(&url);
    let database = driver.clickhouse_config().database.clone();

    driver
        .command(
            "DROP TABLE IF EXISTS cubedriver_upload",
            &QueryOptions::default(),
        )
        .await
        .unwrap();

    // ClickHouse needs an engine, so the table is created directly rather than
    // through `createTable`.
    driver
        .command(
            "CREATE TABLE cubedriver_upload (id Int64, name String) ENGINE = Memory",
            &QueryOptions::default(),
        )
        .await
        .unwrap();
    driver
        .command(
            "INSERT INTO cubedriver_upload VALUES (1, 'a'), (2, 'b')",
            &QueryOptions::default(),
        )
        .await
        .unwrap();

    let rows = query(&driver, "SELECT * FROM cubedriver_upload ORDER BY id", &[]).await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows.get(0, "id"), Some(&json!("1")));
    assert_eq!(rows.get_string(1, "name").as_deref(), Some("b"));

    // `getSchemas` always reports the configured database
    let schemas = driver.get_schemas().await.unwrap();
    assert_eq!(schemas.len(), 1);
    assert_eq!(schemas[0].schema_name, database);

    let tables = driver.get_tables_query(&database).await.unwrap();
    assert!(tables.iter().any(|t| t == "cubedriver_upload"));

    let columns = driver
        .get_columns_for_specific_tables(&[SchemaTable {
            schema_name: database.clone(),
            table_name: "cubedriver_upload".into(),
        }])
        .await
        .unwrap();
    let names: Vec<_> = columns.iter().map(|c| c.column_name.clone()).collect();
    assert_eq!(names, vec!["id", "name"]);
    assert_eq!(columns[0].data_type, "Int64");

    // `queryColumnTypes` goes through DESCRIBE
    let types = driver
        .query_column_types(
            "(SELECT id, name FROM cubedriver_upload)",
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        types,
        vec![Column::new("id", "bigint"), Column::new("name", "text")]
    );

    let structure = driver.tables_schema().await.unwrap();
    assert!(structure[&database].contains_key("cubedriver_upload"));

    driver
        .drop_table("cubedriver_upload", &QueryOptions::default())
        .await
        .unwrap();
    let tables = driver.get_tables_query(&database).await.unwrap();
    assert!(!tables.iter().any(|t| t == "cubedriver_upload"));
}

#[tokio::test]
async fn creates_a_database() {
    let url = require_clickhouse!();
    let driver = driver(&url);
    driver
        .create_schema_if_not_exists("cubedriver_schema_test")
        .await
        .unwrap();
    driver
        .command(
            "DROP DATABASE IF EXISTS cubedriver_schema_test",
            &QueryOptions::default(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn streams_rows() {
    let url = require_clickhouse!();
    let driver = driver(&url);

    let stream = driver
        .stream(
            "SELECT number AS n, toDateTime('2020-01-01 00:00:00', 'UTC') AS t FROM numbers(3)",
            &[],
            &StreamOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        stream.columns,
        vec![Column::new("n", "bigint"), Column::new("t", "timestamp")]
    );
    let rows: Vec<_> = stream.rows.try_collect().await.unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0], vec![json!("0"), json!("2020-01-01T00:00:00.000")]);
    assert_eq!(rows[2][0], json!("2"));

    let data = driver
        .download_query_results(
            "SELECT 1 AS a",
            &[],
            &DownloadQueryResultsOptions {
                stream_import: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let DownloadedData::Stream(s) = data else {
        panic!("expected a stream");
    };
    let rows: Vec<_> = s.rows.try_collect().await.unwrap();
    assert_eq!(rows, vec![vec![json!("1")]]);
}

#[tokio::test]
async fn reports_database_errors() {
    let url = require_clickhouse!();
    let driver = driver(&url);
    let err = driver
        .query(
            "SELECT * FROM definitely_not_a_table",
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("definitely_not_a_table"), "{err}");

    let err = driver
        .command("NOT SQL AT ALL", &QueryOptions::default())
        .await
        .unwrap_err();
    assert!(err.to_string().starts_with("Command failed:"), "{err}");
}
