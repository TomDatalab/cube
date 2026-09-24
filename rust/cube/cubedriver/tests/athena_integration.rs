#![cfg(feature = "athena")]
//! Port of `test/AthenaDriver.test.ts` against real Amazon Athena.
//!
//! Needs an AWS account: `CUBEJS_AWS_REGION`, `CUBEJS_AWS_S3_OUTPUT_LOCATION`
//! and credentials (`CUBEJS_AWS_KEY` / `CUBEJS_AWS_SECRET` or the SDK chain),
//! gated on `CUBEJS_TEST_ATHENA=true` so a developer's ambient AWS profile is
//! never used by accident. `CUBEJS_DB_EXPORT_BUCKET` enables the unload test.
//! LocalStack's Athena is a Pro feature, so the protocol is covered by the
//! mock-server tests in `src/athena/tests.rs` instead.

use std::time::Duration;

use cubedriver::types::{QueryOptions, StreamOptions, UnloadOptions, UnloadQuery};
use cubedriver::{AthenaConfig, AthenaDriver, Driver};
use futures::TryStreamExt;
use serde_json::json;

fn config() -> Option<AthenaConfig> {
    if std::env::var("CUBEJS_TEST_ATHENA").ok().as_deref() != Some("true") {
        eprintln!("skipping: set CUBEJS_TEST_ATHENA=true and the CUBEJS_AWS_* variables");
        return None;
    }
    Some(AthenaConfig::from_env(None).unwrap())
}

const QUERY: &str = "SELECT * FROM (VALUES ('new', 300), ('processed', 400), (NULL, 500)) \
                     AS t (orders__status, orders__amount) WHERE orders__amount > ?";

#[tokio::test]
async fn query_and_stream() {
    let Some(config) = config() else { return };
    let driver = AthenaDriver::new(config).unwrap();
    driver.test_connection().await.unwrap();

    let result = driver
        .query(QUERY, &[json!(0)], &QueryOptions::default())
        .await
        .unwrap();
    // `expectStringFields`: Athena returns every value as a string.
    assert_eq!(
        result.rows,
        vec![
            vec![json!("new"), json!("300")],
            vec![json!("processed"), json!("400")],
            vec![serde_json::Value::Null, json!("500")],
        ]
    );

    let stream = driver
        .stream(QUERY, &[json!(0)], &StreamOptions::default())
        .await
        .unwrap();
    assert_eq!(stream.columns[0].name, "orders__status");
    let rows: Vec<_> = stream.rows.try_collect().await.unwrap();
    assert_eq!(rows.len(), 3);
}

#[tokio::test]
async fn unload_to_the_export_bucket() {
    let Some(config) = config() else { return };
    if config.export_bucket.is_none() {
        eprintln!("skipping unload: CUBEJS_DB_EXPORT_BUCKET is not set");
        return;
    }
    let driver = AthenaDriver::new(config).unwrap();
    let table = format!(
        "cube_unload_test_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );
    let data = driver
        .unload(
            &table,
            &UnloadOptions {
                query: Some(UnloadQuery {
                    sql: QUERY.to_string(),
                    params: vec![json!(0)],
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(!data.csv_file.is_empty());
    assert!(data.csv_no_header);
    assert_eq!(data.csv_delimiter.as_deref(), Some("^A"));
    for url in &data.csv_file {
        let response = reqwest::get(url).await.unwrap();
        assert!(response.status().is_success(), "{url}");
    }
}

#[tokio::test]
async fn poll_timeout_cancels_the_in_flight_query() {
    let Some(mut config) = config() else { return };
    config.poll_timeout = Duration::from_secs(5);
    let driver = AthenaDriver::new(config).unwrap();
    let slow = "SELECT count(*) AS c FROM ( \
        SELECT length(regexp_replace(CONCAT(CAST(a.i * b.j + 7919 AS VARCHAR), '-', CAST(a.i AS VARCHAR)), '[0-9]', 'd')) AS n \
        FROM unnest(sequence(1, 49999)) AS a(i) CROSS JOIN unnest(sequence(1, 49999)) AS b(j)) WHERE n > 0";
    let err = driver
        .query(slow, &[], &QueryOptions::default())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Athena job timeout"), "{err}");
}
