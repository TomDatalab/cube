#![cfg(feature = "trino")]
//! Integration tests of the Trino driver.
//!
//! They run only when `CUBEJS_TEST_TRINO_HOST` is set, e.g. against the
//! official image (which ships the `tpch` and `memory` catalogs):
//!
//! ```text
//! docker run -d -p 127.0.0.1:16580:8080 trinodb/trino
//! CUBEJS_TEST_TRINO_HOST=127.0.0.1 CUBEJS_TEST_TRINO_PORT=16580
//! ```

mod presto_common;

use cubedriver::prestodb::{Engine, PrestoConfig};
use cubedriver::Driver;
use cubedriver::TrinoDriver;

fn server() -> Option<(String, u16)> {
    let host = std::env::var("CUBEJS_TEST_TRINO_HOST")
        .ok()
        .filter(|s| !s.is_empty())?;
    let port = std::env::var("CUBEJS_TEST_TRINO_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    Some((host, port))
}

macro_rules! require_server {
    () => {
        match server() {
            Some(s) => s,
            None => {
                eprintln!("CUBEJS_TEST_TRINO_HOST is not set, skipping");
                return;
            }
        }
    };
}

fn config(host: &str, port: u16) -> PrestoConfig {
    presto_common::config(Engine::Trino, host, port)
}

fn driver(host: &str, port: u16) -> Box<dyn Driver> {
    Box::new(TrinoDriver::new(config(host, port)).unwrap())
}

#[tokio::test]
async fn test_connection() {
    let (host, port) = require_server!();
    driver(&host, port).test_connection().await.unwrap();

    let mut select = config(&host, port);
    select.use_select_test_connection = true;
    TrinoDriver::new(select)
        .unwrap()
        .test_connection()
        .await
        .unwrap();

    let err = driver(&host, 1).test_connection().await.unwrap_err();
    assert!(!err.to_string().is_empty());
}

#[tokio::test]
async fn scalar_types_and_params() {
    let (host, port) = require_server!();
    presto_common::scalar_types_and_params(driver(&host, port).as_ref()).await;
}

#[tokio::test]
async fn multi_page_results_keep_order() {
    let (host, port) = require_server!();
    presto_common::multi_page_results_keep_order(driver(&host, port).as_ref()).await;
}

#[tokio::test]
async fn streaming() {
    let (host, port) = require_server!();
    presto_common::streaming(driver(&host, port).as_ref()).await;
}

#[tokio::test]
async fn introspection() {
    let (host, port) = require_server!();
    presto_common::introspection(driver(&host, port).as_ref()).await;
}

#[tokio::test]
async fn errors() {
    let (host, port) = require_server!();
    presto_common::errors(driver(&host, port).as_ref()).await;
}

#[tokio::test]
async fn memory_catalog_writes() {
    let (host, port) = require_server!();
    let mut memory = config(&host, port);
    memory.catalog = Some("memory".into());
    memory.schema = Some("default".into());
    presto_common::memory_catalog_writes(&TrinoDriver::new(memory).unwrap()).await;
}

#[tokio::test]
async fn query_timeout() {
    let (host, port) = require_server!();
    let mut config = config(&host, port);
    config.query_timeout = std::time::Duration::from_secs(3);
    let driver = TrinoDriver::new(config).unwrap();
    let err = driver
        .query(
            "SELECT count(*) FROM tpch.sf1000.lineitem a CROSS JOIN tpch.sf1000.lineitem b",
            &[],
            &cubedriver::QueryOptions::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "execution error:query timed out");
}

/// Export-bucket unload through a Hive catalog on S3-compatible storage.
///
/// Needs a Trino whose `hive` catalog writes to the bucket, plus the bucket's
/// endpoint as seen from the test (SeaweedFS, which verifies SigV4):
///
/// ```text
/// CUBEJS_TEST_TRINO_HIVE_HOST=127.0.0.1 CUBEJS_TEST_TRINO_HIVE_PORT=16582
/// CUBEJS_TEST_TRINO_S3_ENDPOINT=http://127.0.0.1:16590
/// CUBEJS_TEST_TRINO_S3_BUCKET=export CUBEJS_TEST_TRINO_S3_SCHEMA_LOCATION=s3://warehouse/cube_unload
/// CUBEJS_TEST_TRINO_S3_KEY=… CUBEJS_TEST_TRINO_S3_SECRET=…
/// ```
#[tokio::test]
async fn unload_to_s3_export_bucket() {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let Some(host) = var("CUBEJS_TEST_TRINO_HIVE_HOST") else {
        eprintln!("CUBEJS_TEST_TRINO_HIVE_HOST is not set, skipping");
        return;
    };
    let port: u16 = var("CUBEJS_TEST_TRINO_HIVE_PORT")
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let mut config = config(&host, port);
    config.catalog = Some("hive".into());
    config.schema = Some("cube_unload".into());
    config.export_bucket.bucket_type = Some("s3".into());
    config.export_bucket.export_bucket = var("CUBEJS_TEST_TRINO_S3_BUCKET");
    config.export_bucket.access_key_id = var("CUBEJS_TEST_TRINO_S3_KEY");
    config.export_bucket.secret_access_key = var("CUBEJS_TEST_TRINO_S3_SECRET");
    config.export_bucket.region = Some("us-east-1".into());
    config.export_bucket.s3_endpoint = var("CUBEJS_TEST_TRINO_S3_ENDPOINT");
    let driver = TrinoDriver::new(config).unwrap();
    assert!(driver
        .is_unload_supported(&Default::default())
        .await
        .unwrap());

    let location = var("CUBEJS_TEST_TRINO_S3_SCHEMA_LOCATION").unwrap();
    presto_common::query(
        &driver,
        &format!("CREATE SCHEMA IF NOT EXISTS hive.cube_unload WITH (location = '{location}')"),
        &[],
    )
    .await;

    let table = format!(
        "cube_unload.nation_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );
    let csv = driver
        .unload(
            &table,
            &cubedriver::types::UnloadOptions {
                max_file_size: 64,
                query: Some(cubedriver::types::UnloadQuery {
                    sql: "SELECT nationkey, name FROM tpch.tiny.nation WHERE regionkey = ?".into(),
                    params: vec![serde_json::json!(1)],
                }),
                request_id: None,
            },
        )
        .await
        .unwrap();
    assert!(csv.csv_no_header);
    assert!(!csv.csv_file.is_empty());
    assert!(csv.csv_file[0].contains("X-Amz-Signature="));
    assert_eq!(
        csv.types,
        Some(vec![
            cubedriver::Column::new("nationkey", "bigint"),
            cubedriver::Column::new("name", "varchar(25)")
        ])
    );

    // The presigned URLs work (the store verifies SigV4) and a tampered one
    // is refused. The files are gzip CSV (Trino's default Hive codec), which
    // Cube Store imports natively.
    let http = reqwest::Client::new();
    let mut bytes = 0;
    for url in &csv.csv_file {
        let response = http.get(url).send().await.unwrap();
        assert_eq!(response.status(), 200, "{url}");
        bytes += response.bytes().await.unwrap().len();
    }
    assert!(bytes > 0);
    let tampered = csv.csv_file[0].replace("X-Amz-Expires=3600", "X-Amz-Expires=7200");
    assert_eq!(http.get(&tampered).send().await.unwrap().status(), 403);

    // The temporary external table is gone again.
    let tables = driver.get_tables_query("cube_unload").await.unwrap();
    assert!(!tables.iter().any(|t| table.ends_with(t.as_str())));
}
