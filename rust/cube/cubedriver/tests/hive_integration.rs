#![cfg(feature = "hive")]
//! Integration tests for [`HiveDriver`] against a real HiveServer2.
//!
//! They run only when `CUBEJS_TEST_HIVE=true`, e.g. with
//!
//! ```text
//! docker run -d --name drv-hive-hs2 -e SERVICE_NAME=hiveserver2 -p 16800:10000 apache/hive:4.0.0
//! CUBEJS_TEST_HIVE=true CUBEJS_DB_HOST=127.0.0.1 CUBEJS_DB_PORT=16800 \
//!   cargo test -p cubedriver --features hive --test hive_integration
//! ```
//!
//! The image uses `hive.server2.authentication=NONE`, i.e. SASL `PLAIN`
//! without password checks — the default mechanism of the driver.

use cubedriver::hive::{AuthMechanism, Session};
use cubedriver::{Driver, DriverError, HiveConfig, HiveDriver, QueryOptions};
use serde_json::{json, Value};

fn enabled() -> bool {
    std::env::var("CUBEJS_TEST_HIVE").as_deref() == Ok("true")
}

fn config() -> HiveConfig {
    HiveConfig::from_env(None).unwrap()
}

macro_rules! require_hive {
    () => {
        if !enabled() {
            eprintln!("CUBEJS_TEST_HIVE is not 'true', skipping");
            return;
        } else {
            HiveDriver::new(config()).unwrap()
        }
    };
}

#[tokio::test]
async fn test_connection_and_scalar_types() {
    let driver = require_hive!();
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
               CAST('2020-01-01' AS DATE) AS d,
               CAST('2020-01-01 12:34:56.789' AS TIMESTAMP) AS ts,
               CAST(NULL AS STRING) AS nothing,
               'NULL' AS null_string",
            &[json!("it's a \\ test")],
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
    assert_eq!(data.get(0, "s"), Some(&json!("it's a \\ test")));
    assert_eq!(data.get(0, "d"), Some(&json!("2020-01-01")));
    assert_eq!(data.get(0, "ts"), Some(&json!("2020-01-01 12:34:56.789")));
    assert_eq!(data.get(0, "nothing"), Some(&Value::Null));
    assert_eq!(data.get(0, "null_string"), Some(&json!("NULL")));

    let types: Vec<String> = data.columns.iter().map(|c| c.type_.to_string()).collect();
    assert_eq!(
        types,
        vec![
            "int",
            "bigint",
            "decimal",
            "double",
            "boolean",
            "text",
            "date",
            "timestamp",
            "text",
            "text"
        ]
    );
    driver.release().await.unwrap();
}

#[tokio::test]
async fn errors_are_reported_and_sessions_reused() {
    let driver = require_hive!();
    let err = driver
        .query(
            "SELECT * FROM no_such_table_xyz",
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap_err();
    match &err {
        DriverError::Database { message, code } => {
            assert!(message.contains("no_such_table_xyz"), "{message}");
            assert!(code.is_some());
        }
        other => panic!("unexpected error {other:?}"),
    }
    // the session survives a failed statement
    let data = driver
        .query("SELECT 2 AS two", &[], &QueryOptions::default())
        .await
        .unwrap();
    assert_eq!(data.get(0, "two"), Some(&json!(2)));
    driver.release().await.unwrap();
}

#[tokio::test]
async fn tables_schema_and_multi_block_fetch() {
    let driver = require_hive!();
    let opts = QueryOptions::default();
    driver
        .query("CREATE DATABASE IF NOT EXISTS cube_rust_test", &[], &opts)
        .await
        .unwrap();
    driver
        .query("DROP TABLE IF EXISTS cube_rust_test.orders", &[], &opts)
        .await
        .unwrap();
    driver
        .query(
            "CREATE TABLE cube_rust_test.orders (id INT, status STRING, amount DOUBLE)",
            &[],
            &opts,
        )
        .await
        .unwrap();
    driver
        .query(
            "INSERT INTO cube_rust_test.orders VALUES (1, 'new', 1.5), (2, 'done', NULL), (3, NULL, 3.25)",
            &[],
            &opts,
        )
        .await
        .unwrap();

    let rows = driver
        .query(
            "SELECT id, status, amount FROM cube_rust_test.orders WHERE id >= ? ORDER BY id",
            &[json!(2)],
            &opts,
        )
        .await
        .unwrap();
    assert_eq!(
        rows.rows,
        vec![
            vec![json!(2), json!("done"), Value::Null],
            vec![json!(3), Value::Null, json!(3.25)],
        ]
    );

    let mut config = config();
    config.db_name = "cube_rust_test".to_string();
    // force several FetchResults round trips
    config.max_rows = 1;
    let scoped = HiveDriver::new(config).unwrap();
    let all = scoped
        .query(
            "SELECT id FROM cube_rust_test.orders ORDER BY id",
            &[],
            &opts,
        )
        .await
        .unwrap();
    assert_eq!(
        all.rows,
        vec![vec![json!(1)], vec![json!(2)], vec![json!(3)]]
    );

    let structure = scoped.tables_schema().await.unwrap();
    let orders = &structure["cube_rust_test"]["orders"];
    let columns: Vec<(&str, &str)> = orders
        .iter()
        .map(|c| (c.name.as_str(), c.type_.as_str()))
        .collect();
    assert_eq!(
        columns,
        vec![("id", "int"), ("status", "string"), ("amount", "double")]
    );

    driver
        .query("DROP TABLE cube_rust_test.orders", &[], &opts)
        .await
        .unwrap();
    driver.release().await.unwrap();
    scoped.release().await.unwrap();
}

#[tokio::test]
async fn nosasl_is_refused_by_a_sasl_server() {
    if !enabled() {
        return;
    }
    let mut config = config();
    config.auth = AuthMechanism::NoSasl;
    config.timeout = std::time::Duration::from_secs(5);
    // HiveServer2 with NONE authentication expects a SASL handshake, so an
    // unframed OpenSession must fail cleanly rather than hang.
    let result =
        tokio::time::timeout(std::time::Duration::from_secs(30), Session::open(&config)).await;
    match result {
        Ok(Ok(_)) => panic!("NOSASL unexpectedly accepted by a SASL server"),
        Ok(Err(e)) => assert!(matches!(e, DriverError::Connection { .. }), "{e:?}"),
        Err(_) => panic!("NOSASL OpenSession hung"),
    }
}

/// Needs a second HiveServer2 with `hive.server2.authentication=NOSASL`
/// (`-e SERVICE_OPTS=-Dhive.server2.authentication=NOSASL`), whose port is
/// given in `CUBEJS_TEST_HIVE_NOSASL_PORT`.
#[tokio::test]
async fn nosasl_server() {
    if !enabled() {
        return;
    }
    let Some(port) = std::env::var("CUBEJS_TEST_HIVE_NOSASL_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
    else {
        eprintln!("CUBEJS_TEST_HIVE_NOSASL_PORT is not set, skipping");
        return;
    };
    let mut config = config();
    config.port = port;
    config.auth = AuthMechanism::NoSasl;
    let driver = HiveDriver::new(config).unwrap();
    driver.test_connection().await.unwrap();
    let data = driver
        .query(
            "SELECT ? AS s, 42 AS n",
            &[json!("x".repeat(100_000))],
            &QueryOptions::default(),
        )
        .await
        .unwrap();
    // a reply larger than one TCP read is reassembled
    assert_eq!(
        data.get(0, "s").and_then(Value::as_str).map(str::len),
        Some(100_000)
    );
    assert_eq!(data.get(0, "n"), Some(&json!(42)));
    driver.release().await.unwrap();
}
