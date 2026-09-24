//! Integration tests for [`BigQueryDriver`].
//!
//! BigQuery is cloud only, so these tests run exclusively when
//! `CUBEJS_TEST_BIGQUERY=true` and real credentials are configured:
//!
//! ```text
//! CUBEJS_TEST_BIGQUERY=true
//! CUBEJS_DB_BQ_PROJECT_ID=<project>
//! CUBEJS_DB_BQ_CREDENTIALS=<base64 encoded service account JSON>
//! CUBEJS_TEST_BIGQUERY_DATASET=<dataset that may be written to>
//! ```

use cubedriver::{BigQueryConfig, BigQueryDriver, Column, Driver, QueryOptions};
use serde_json::{json, Value};

fn enabled() -> bool {
    std::env::var("CUBEJS_TEST_BIGQUERY").as_deref() == Ok("true")
}

macro_rules! require_bigquery {
    () => {
        if !enabled() {
            eprintln!("CUBEJS_TEST_BIGQUERY is not 'true', skipping");
            return;
        } else {
            BigQueryDriver::new(BigQueryConfig::from_env(None).unwrap()).unwrap()
        }
    };
}

fn dataset() -> String {
    std::env::var("CUBEJS_TEST_BIGQUERY_DATASET").unwrap_or_else(|_| "cube_test".to_string())
}

#[tokio::test]
async fn test_connection_and_scalar_types() {
    let driver = require_bigquery!();
    driver.test_connection().await.unwrap();

    let data = driver
        .query(
            "SELECT
               1 AS i,
               CAST(9007199254740993 AS INT64) AS big,
               CAST(1.5 AS FLOAT64) AS dbl,
               CAST('1.25' AS NUMERIC) AS num,
               TRUE AS flag,
               'foo' AS s,
               DATE '2020-01-01' AS d,
               TIMESTAMP '2020-01-01 12:34:56.789 UTC' AS ts,
               CAST(NULL AS STRING) AS nothing",
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap();

    assert_eq!(data.len(), 1);
    // integers, floats and numerics arrive as strings (`transformRow`)
    assert_eq!(data.get(0, "big"), Some(&json!("9007199254740993")));
    assert_eq!(data.get(0, "dbl"), Some(&json!("1.5")));
    assert_eq!(data.get(0, "num"), Some(&json!("1.25")));
    assert_eq!(data.get(0, "flag"), Some(&json!(true)));
    assert_eq!(data.get(0, "s"), Some(&json!("foo")));
    assert_eq!(data.get(0, "d"), Some(&json!("2020-01-01")));
    assert_eq!(data.get(0, "ts"), Some(&json!("2020-01-01T12:34:56.789Z")));
    assert_eq!(data.get(0, "nothing"), Some(&Value::Null));
}

#[tokio::test]
async fn parameters_are_positional() {
    let driver = require_bigquery!();
    let data = driver
        .query(
            "SELECT ? AS s, ? + 1 AS n",
            &[json!("x"), json!(41)],
            &QueryOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(data.get(0, "s"), Some(&json!("x")));
    assert_eq!(data.get(0, "n"), Some(&json!("42")));
}

#[tokio::test]
async fn schema_introspection() {
    let driver = require_bigquery!();
    let dataset = dataset();
    driver.create_schema_if_not_exists(&dataset).await.unwrap();

    let schemas = driver.get_schemas().await.unwrap();
    assert!(schemas.iter().any(|s| s.schema_name == dataset));

    let table = format!("{dataset}.cube_driver_test");
    driver
        .query(
            &format!("DROP TABLE IF EXISTS {table}"),
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap();
    driver
        .query(
            &format!("CREATE TABLE {table} (id INT64, amount NUMERIC, created TIMESTAMP)"),
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap();

    let tables = driver.get_tables_query(&dataset).await.unwrap();
    assert!(tables.contains(&"cube_driver_test".to_string()));

    let types = driver.table_column_types(&table).await.unwrap();
    assert_eq!(
        types,
        vec![
            Column::new("id", "INT64"),
            // NUMERIC is always (38, 9)
            Column::new("amount", "decimal"),
            Column::new("created", "TIMESTAMP"),
        ]
    );

    let structure = driver.tables_schema().await.unwrap();
    assert!(structure[&dataset].contains_key("cube_driver_test"));

    driver
        .query(
            &format!("DROP TABLE {table}"),
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap();
}
