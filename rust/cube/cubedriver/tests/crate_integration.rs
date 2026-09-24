#![cfg(feature = "cratedb")]
//! Integration tests for [`CrateDriver`].
//!
//! They run only when `CUBEJS_TEST_CRATE_URL` is set, e.g.
//! `docker run -d -p 5432:5432 crate:latest -Cdiscovery.type=single-node` and
//! `CUBEJS_TEST_CRATE_URL=postgres://crate@localhost:5432/doc`.

use cubedriver::{Column, CrateConfig, CrateDriver, Driver, QueryOptions, StreamOptions};
use futures::TryStreamExt;
use serde_json::{json, Value};

fn crate_url() -> Option<String> {
    std::env::var("CUBEJS_TEST_CRATE_URL")
        .ok()
        .filter(|s| !s.is_empty())
}

macro_rules! require_crate {
    () => {
        match crate_url() {
            Some(url) => url,
            None => {
                eprintln!("CUBEJS_TEST_CRATE_URL is not set, skipping");
                return;
            }
        }
    };
}

fn driver(url: &str) -> CrateDriver {
    CrateDriver::new(CrateConfig::from_url(url)).unwrap()
}

/// `DriverTests.QUERY`.
const QUERY: &str = "
    SELECT id, amount, status
    FROM (
      SELECT 1 AS id, 100 AS amount, 'new' AS status
      UNION ALL
      SELECT 2 AS id, 200 AS amount, 'new' AS status
      UNION ALL
      SELECT 3 AS id, 400 AS amount, 'processed' AS status
      UNION ALL
      SELECT 4 AS id, 500 AS amount, NULL AS status
    ) AS data
    ORDER BY 1
  ";

fn expected_rows() -> Vec<Vec<Value>> {
    vec![
        vec![json!(1), json!(100), json!("new")],
        vec![json!(2), json!(200), json!("new")],
        vec![json!(3), json!(400), json!("processed")],
        vec![json!(4), json!(500), Value::Null],
    ]
}

#[tokio::test]
async fn test_connection_and_query() {
    let url = require_crate!();
    let driver = driver(&url);
    driver.test_connection().await.unwrap();

    // `DriverTests.testQuery`
    let data = driver
        .query(QUERY, &[], &QueryOptions::default())
        .await
        .unwrap();
    assert_eq!(data.rows, expected_rows());

    let data = driver
        .query(
            "SELECT $1::int AS n, $2::text AS s",
            &[json!("41"), json!("x")],
            &QueryOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(data.get(0, "n"), Some(&json!(41)));
    assert_eq!(data.get(0, "s"), Some(&json!("x")));
    assert_eq!(
        data.columns,
        vec![Column::new("n", "int"), Column::new("s", "text")]
    );
    driver.release().await.unwrap();
}

#[tokio::test]
async fn no_session_setup_is_sent() {
    let url = require_crate!();
    let driver = driver(&url);
    // A pooled connection comes up although CrateDB rejects `SET TIME ZONE`
    // and `statement_timeout`.
    let data = driver
        .query("SELECT 1 AS one", &[], &QueryOptions::default())
        .await
        .unwrap();
    assert_eq!(data.get(0, "one"), Some(&json!(1)));
    driver.release().await.unwrap();
}

#[tokio::test]
async fn stream_rows() {
    let url = require_crate!();
    let driver = driver(&url);
    let stream = driver
        .stream(QUERY, &[], &StreamOptions::default())
        .await
        .unwrap();
    assert_eq!(
        stream
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        vec!["id", "amount", "status"]
    );
    let rows: Vec<Vec<Value>> = stream.rows.try_collect().await.unwrap();
    assert_eq!(rows, expected_rows());
    driver.release().await.unwrap();
}

#[tokio::test]
async fn load_pre_aggregation_inlines_params_and_refreshes() {
    let url = require_crate!();
    let driver = driver(&url);
    let q = |sql: &'static str| async { driver.query(sql, &[], &QueryOptions::default()).await };
    let _ = q("DROP TABLE IF EXISTS drv_pgwire.orders").await;
    let _ = q("DROP TABLE IF EXISTS drv_pgwire.orders_pa").await;
    q("CREATE TABLE drv_pgwire.orders (id INT, amount INT, created TIMESTAMP WITH TIME ZONE)")
        .await
        .unwrap();
    q("INSERT INTO drv_pgwire.orders VALUES (1, 10, '2020-01-01T00:00:00Z'), (2, 20, '2020-01-15T00:00:00Z'), (3, 30, '2020-02-01T00:00:00Z')")
        .await
        .unwrap();
    q("REFRESH TABLE drv_pgwire.orders").await.unwrap();

    driver
        .load_pre_aggregation_into_table(
            "drv_pgwire.orders_pa",
            "CREATE TABLE drv_pgwire.orders_pa AS (SELECT sum(amount) AS total FROM drv_pgwire.orders \
             WHERE created >= $1::timestamptz AND created <= $2::timestamptz)",
            &[
                json!("2020-01-01T00:00:00.000Z"),
                json!("2020-01-31T23:59:59.999Z"),
            ],
            &QueryOptions::default(),
        )
        .await
        .unwrap();

    // Readable right away thanks to `REFRESH TABLE`.
    let data = q("SELECT total FROM drv_pgwire.orders_pa").await.unwrap();
    assert_eq!(data.rows, vec![vec![json!("30")]]);

    let columns = driver
        .table_column_types("drv_pgwire.orders")
        .await
        .unwrap();
    assert_eq!(
        columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        vec!["id", "amount", "created"]
    );
    let schema = driver.tables_schema().await.unwrap();
    assert!(schema["drv_pgwire"].contains_key("orders"));

    let _ = q("DROP TABLE drv_pgwire.orders").await;
    let _ = q("DROP TABLE drv_pgwire.orders_pa").await;
    driver.release().await.unwrap();
}

#[tokio::test]
async fn errors_carry_the_server_message() {
    let url = require_crate!();
    let driver = driver(&url);
    let err = driver
        .query(
            "SELECT * FROM random_name_for_table_that_doesnot_exist_sql_must_fail",
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("random_name_for_table_that_doesnot_exist_sql_must_fail"),
        "{err}"
    );
    driver.release().await.unwrap();
}
