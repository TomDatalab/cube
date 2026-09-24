#![cfg(feature = "mongobi")]
//! Integration tests for [`MongoBiDriver`], ported from
//! `packages/cubejs-mongobi-driver/test/MongoBiDriver.test.ts`.
//!
//! They run only when `CUBEJS_TEST_MONGOBI_URL` is set to a `mongosqld`
//! whose MongoDB holds the fixture of `test/mongo-init.js`
//! (`test.mycol`: `{ number: 1, created: ISODate('1998-08-02T00:00:00Z') }`),
//! e.g. `CUBEJS_TEST_MONGOBI_URL=mysql://localhost:3307/test`.

use cubedriver::{
    DownloadQueryResultsOptions, DownloadedData, Driver, MongoBiConfig, MongoBiDriver,
    QueryOptions, StreamOptions,
};
use futures::TryStreamExt;
use serde_json::{json, Value};

fn mongobi_url() -> Option<String> {
    std::env::var("CUBEJS_TEST_MONGOBI_URL")
        .ok()
        .filter(|s| !s.is_empty())
}

macro_rules! require_mongobi {
    () => {
        match mongobi_url() {
            Some(url) => url,
            None => {
                eprintln!("CUBEJS_TEST_MONGOBI_URL is not set, skipping");
                return;
            }
        }
    };
}

fn driver(url: &str) -> MongoBiDriver {
    let mut config = MongoBiConfig::from_url(url);
    config.max_pool_size = Some(1);
    MongoBiDriver::new(config).unwrap()
}

#[tokio::test]
async fn should_test_connection() {
    let url = require_mongobi!();
    let driver = driver(&url);
    driver.test_connection().await.unwrap();
    driver.release().await.unwrap();
}

#[tokio::test]
async fn should_select_raw_sql() {
    let url = require_mongobi!();
    let driver = driver(&url);
    let result = driver
        .query(
            "
      SELECT number
      FROM mycol
      LIMIT 1
    ",
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        result.to_json_rows(),
        vec![json!({"number": 1}).as_object().unwrap().clone()]
    );
    driver.release().await.unwrap();
}

/// The Node `typeCast`: DATETIME reaches the caller as the string mongosqld sent.
#[tokio::test]
async fn should_select_datetime_as_string() {
    let url = require_mongobi!();
    let driver = driver(&url);
    let result = driver
        .query(
            "SELECT created FROM mycol WHERE number = ? LIMIT 1",
            &[json!(1)],
            &QueryOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(result.rows, vec![vec![json!("1998-08-02 00:00:00")]]);
    driver.release().await.unwrap();
}

#[tokio::test]
async fn download_stream_and_schema() {
    let url = require_mongobi!();
    let driver = driver(&url);

    let DownloadedData::Memory(data) = driver
        .download_query_results(
            "SELECT number, created FROM mycol",
            &[],
            &DownloadQueryResultsOptions::default(),
        )
        .await
        .unwrap()
    else {
        panic!("memory download expected");
    };
    assert_eq!(
        data.rows,
        vec![vec![json!(1), json!("1998-08-02 00:00:00")]]
    );
    assert_eq!(data.columns[1].type_.to_string(), "timestamp");

    let stream = driver
        .stream("SELECT number FROM mycol", &[], &StreamOptions::default())
        .await
        .unwrap();
    let rows: Vec<Vec<Value>> = stream.rows.try_collect().await.unwrap();
    assert_eq!(rows, vec![vec![json!(1)]]);

    let schema = driver.tables_schema().await.unwrap();
    let columns = &schema["test"]["mycol"];
    assert!(columns.iter().any(|c| c.name == "number"), "{columns:?}");
    assert!(columns.iter().any(|c| c.name == "created"), "{columns:?}");
    driver.release().await.unwrap();
}
