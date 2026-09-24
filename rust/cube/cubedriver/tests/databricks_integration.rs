#![cfg(feature = "databricks")]
//! Integration tests for [`DatabricksDriver`] against a real SQL warehouse.
//!
//! Databricks is cloud only, so these tests run exclusively when
//! `CUBEJS_TEST_DATABRICKS=true`:
//!
//! ```text
//! CUBEJS_TEST_DATABRICKS=true
//! CUBEJS_DB_DATABRICKS_URL=jdbc:databricks://<host>:443;httpPath=/sql/1.0/warehouses/<id>
//! CUBEJS_DB_DATABRICKS_TOKEN=<personal access token>
//! # or CUBEJS_DB_DATABRICKS_OAUTH_CLIENT_ID / _SECRET
//! CUBEJS_DB_DATABRICKS_CATALOG=<catalog>   # optional
//! ```
//!
//! The unit tests in `src/databricks/tests.rs` cover the protocol against a
//! mock server; these check the real service's value formats.

use cubedriver::{
    DatabricksConfig, DatabricksDriver, DownloadQueryResultsOptions, DownloadedData, Driver,
    QueryOptions,
};
use futures::TryStreamExt;
use serde_json::{json, Value};

fn enabled() -> bool {
    std::env::var("CUBEJS_TEST_DATABRICKS").as_deref() == Ok("true")
}

macro_rules! require_databricks {
    () => {
        if !enabled() {
            eprintln!("CUBEJS_TEST_DATABRICKS is not 'true', skipping");
            return;
        } else {
            DatabricksDriver::new(DatabricksConfig::from_env(None).unwrap()).unwrap()
        }
    };
}

#[tokio::test]
async fn test_connection_and_scalar_types() {
    let driver = require_databricks!();
    driver.test_connection().await.unwrap();

    let data = driver
        .query(
            "SELECT
               1 AS i,
               CAST(9007199254740993 AS BIGINT) AS big,
               CAST(1.25 AS DECIMAL(10, 2)) AS amount,
               CAST(1.5 AS DOUBLE) AS dbl,
               TRUE AS flag,
               ? AS s,
               DATE'2020-01-01' AS d,
               TIMESTAMP'2020-01-01 12:34:56.789' AS ts,
               CAST(NULL AS STRING) AS nothing",
            &[json!("it's")],
            &QueryOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(data.len(), 1);
    assert_eq!(data.get(0, "i"), Some(&json!(1)));
    assert_eq!(data.get(0, "big"), Some(&json!("9007199254740993")));
    assert_eq!(data.get(0, "amount"), Some(&json!("1.25")));
    assert_eq!(data.get(0, "dbl"), Some(&json!(1.5)));
    assert_eq!(data.get(0, "flag"), Some(&json!(true)));
    assert_eq!(data.get(0, "s"), Some(&json!("it's")));
    assert_eq!(data.get(0, "d"), Some(&json!("2020-01-01")));
    assert_eq!(data.get(0, "ts"), Some(&json!("2020-01-01T12:34:56.789")));
    assert_eq!(data.get(0, "nothing"), Some(&Value::Null));
}

#[tokio::test]
async fn introspection() {
    let driver = require_databricks!();
    let schemas = driver.get_schemas().await.unwrap();
    assert!(!schemas.is_empty());
    let types = driver
        .query_column_types("SELECT 1 AS a, 'x' AS b", &[], &QueryOptions::default())
        .await
        .unwrap();
    assert_eq!(types.len(), 2);
    assert_eq!(types[0].name, "a");
}

#[tokio::test]
async fn external_links_stream() {
    let driver = require_databricks!();
    let options = DownloadQueryResultsOptions {
        stream_import: true,
        ..Default::default()
    };
    let DownloadedData::Stream(stream) = driver
        .download_query_results("SELECT id FROM range(200000)", &[], &options)
        .await
        .unwrap()
    else {
        panic!("expected a stream");
    };
    let rows: Vec<_> = stream.rows.try_collect().await.unwrap();
    assert_eq!(rows.len(), 200_000);
}
