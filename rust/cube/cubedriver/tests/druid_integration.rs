//! Integration tests for [`DruidDriver`].
//!
//! They run only when `CUBEJS_TEST_DRUID_URL` is set, e.g. against a broker
//! started with the Druid docker image:
//!
//! ```text
//! CUBEJS_TEST_DRUID_URL=http://127.0.0.1:18082
//! ```

use cubedriver::{Column, Driver, DruidConfig, DruidDriver, QueryOptions, QueryResult};
use serde_json::{json, Value};

fn druid_url() -> Option<String> {
    std::env::var("CUBEJS_TEST_DRUID_URL")
        .ok()
        .filter(|s| !s.is_empty())
}

macro_rules! require_druid {
    () => {
        match druid_url() {
            Some(url) => url,
            None => {
                eprintln!("CUBEJS_TEST_DRUID_URL is not set, skipping");
                return;
            }
        }
    };
}

fn driver(url: &str) -> DruidDriver {
    DruidDriver::new(DruidConfig::from_url(url).unwrap()).unwrap()
}

async fn query(driver: &DruidDriver, sql: &str, params: &[Value]) -> QueryResult {
    driver
        .query(sql, params, &QueryOptions::default())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
}

#[tokio::test]
async fn test_connection_and_scalar_types() {
    let url = require_druid!();
    let driver = driver(&url);
    // The Node driver's `testConnection` is a no-op.
    driver.test_connection().await.unwrap();

    let data = query(
        &driver,
        "SELECT
           1 AS i,
           CAST(2 AS BIGINT) AS big,
           CAST(1.5 AS DOUBLE) AS dbl,
           CAST('foo' AS VARCHAR) AS s,
           TIME_PARSE('2020-01-01T00:00:00Z') AS ts",
        &[],
    )
    .await;

    assert_eq!(data.len(), 1);
    assert_eq!(
        data.columns,
        vec![
            Column::new("i", "int"),
            Column::new("big", "bigint"),
            Column::new("dbl", "double"),
            Column::new("s", "text"),
            Column::new("ts", "timestamp"),
        ]
    );
    assert_eq!(data.get(0, "i"), Some(&json!(1)));
    assert_eq!(data.get(0, "s"), Some(&json!("foo")));
    assert_eq!(data.get(0, "ts"), Some(&json!("2020-01-01T00:00:00.000Z")));
}

#[tokio::test]
async fn parameters_are_sent_as_varchar() {
    let url = require_druid!();
    let driver = driver(&url);
    let data = query(
        &driver,
        "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = ?",
        &[Value::from("INFORMATION_SCHEMA")],
    )
    .await;
    let tables: Vec<String> = (0..data.len())
        .filter_map(|i| data.get_string(i, "TABLE_NAME"))
        .collect();
    assert!(tables.contains(&"COLUMNS".to_string()), "{tables:?}");
}

#[tokio::test]
async fn schema_introspection() {
    let url = require_druid!();
    let driver = driver(&url);

    let structure = driver.tables_schema().await.unwrap();
    // INFORMATION_SCHEMA itself is filtered out by `informationSchemaQuery`;
    // the query must still succeed and the result must be a valid structure.
    assert!(!structure.contains_key("INFORMATION_SCHEMA"));

    let tables = driver.get_tables_query("INFORMATION_SCHEMA").await.unwrap();
    assert!(tables.contains(&"TABLES".to_string()), "{tables:?}");

    let err = driver
        .create_schema_if_not_exists("nope")
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Unable to create schema, Druid does not support it"
    );
}

#[tokio::test]
async fn download_query_results_carries_types() {
    let url = require_druid!();
    let driver = driver(&url);
    let data = driver
        .download_query_results(
            "SELECT 1 AS n, CAST('x' AS VARCHAR) AS s",
            &[],
            &cubedriver::DownloadQueryResultsOptions::default(),
        )
        .await
        .unwrap();
    let cubedriver::DownloadedData::Memory(memory) = data else {
        panic!("expected memory data");
    };
    assert_eq!(
        memory.columns,
        vec![Column::new("n", "int"), Column::new("s", "text")]
    );
    assert_eq!(memory.rows, vec![vec![json!(1), json!("x")]]);
}

#[tokio::test]
async fn errors_carry_the_druid_message() {
    let url = require_druid!();
    let driver = driver(&url);
    let err = driver
        .query("SELECT FROM nothing", &[], &QueryOptions::default())
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("Incorrect syntax near the keyword 'FROM'"),
        "unexpected error: {err}"
    );
}
