#![cfg(feature = "prestodb")]
//! Integration tests of the Presto driver.
//!
//! They run only when `CUBEJS_TEST_PRESTO_HOST` is set, e.g. against the
//! official image (which ships the `tpch` and `memory` catalogs):
//!
//! ```text
//! docker run -d -p 127.0.0.1:16581:8080 prestodb/presto:0.294
//! CUBEJS_TEST_PRESTO_HOST=127.0.0.1 CUBEJS_TEST_PRESTO_PORT=16581
//! ```

mod presto_common;

use cubedriver::prestodb::{Engine, PrestoConfig};
use cubedriver::Driver;
use cubedriver::PrestoDriver;

fn server() -> Option<(String, u16)> {
    let host = std::env::var("CUBEJS_TEST_PRESTO_HOST")
        .ok()
        .filter(|s| !s.is_empty())?;
    let port = std::env::var("CUBEJS_TEST_PRESTO_PORT")
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
                eprintln!("CUBEJS_TEST_PRESTO_HOST is not set, skipping");
                return;
            }
        }
    };
}

fn config(host: &str, port: u16) -> PrestoConfig {
    presto_common::config(Engine::Presto, host, port)
}

fn driver(host: &str, port: u16) -> Box<dyn Driver> {
    Box::new(PrestoDriver::new(config(host, port)).unwrap())
}

#[tokio::test]
async fn test_connection() {
    let (host, port) = require_server!();
    driver(&host, port).test_connection().await.unwrap();

    let mut select = config(&host, port);
    select.use_select_test_connection = true;
    PrestoDriver::new(select)
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
    presto_common::memory_catalog_writes(&PrestoDriver::new(memory).unwrap()).await;
}

#[tokio::test]
async fn query_timeout() {
    let (host, port) = require_server!();
    let mut config = config(&host, port);
    config.query_timeout = std::time::Duration::from_secs(3);
    let driver = PrestoDriver::new(config).unwrap();
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
