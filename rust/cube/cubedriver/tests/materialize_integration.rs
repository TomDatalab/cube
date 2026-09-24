#![cfg(feature = "materialize")]
//! Integration tests for [`MaterializeDriver`], ported from
//! `packages/cubejs-materialize-driver/test/MaterializeDriver.test.ts`.
//!
//! They run only when `CUBEJS_TEST_MATERIALIZE_URL` is set, e.g.
//! `docker run -d -p 6875:6875 materialize/materialized` and
//! `CUBEJS_TEST_MATERIALIZE_URL=postgres://materialize@localhost:6875/materialize`.
//! The URL decides SSL (`?sslmode=require`); it is off otherwise.

use cubedriver::{
    Column, Driver, MaterializeConfig, MaterializeDriver, QueryOptions, QueryResult, StreamOptions,
};
use futures::TryStreamExt;
use serde_json::{json, Value};

fn materialize_url() -> Option<String> {
    std::env::var("CUBEJS_TEST_MATERIALIZE_URL")
        .ok()
        .filter(|s| !s.is_empty())
}

macro_rules! require_materialize {
    () => {
        match materialize_url() {
            Some(url) => url,
            None => {
                eprintln!("CUBEJS_TEST_MATERIALIZE_URL is not set, skipping");
                return;
            }
        }
    };
}

fn driver(url: &str, cluster: Option<&str>) -> MaterializeDriver {
    let mut config = MaterializeConfig::from_url(url);
    config.apply_env_values(None, cluster);
    MaterializeDriver::new(config).unwrap()
}

async fn query(driver: &MaterializeDriver, sql: &str) -> QueryResult {
    driver
        .query(sql, &[], &QueryOptions::default())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
}

#[tokio::test]
async fn type_coercion() {
    let url = require_materialize!();
    let driver = driver(&url, None);
    driver.test_connection().await.unwrap();
    let data = query(
        &driver,
        "
        SELECT
          CAST('2020-01-01' as DATE) as date,
          CAST('2020-01-01 00:00:00' as TIMESTAMP) as timestamp,
          CAST('2020-01-01 00:00:00+02' as TIMESTAMPTZ) as timestamptz,
          CAST('1.0' as DECIMAL(10,2)) as decimal
      ",
    )
    .await;
    assert_eq!(
        data.rows,
        vec![vec![
            // Date in UTC
            json!("2020-01-01T00:00:00.000"),
            json!("2020-01-01T00:00:00.000"),
            // converted to utc
            json!("2019-12-31T22:00:00.000"),
            // Numerics as string
            json!("1"),
        ]]
    );
    driver.release().await.unwrap();
}

#[tokio::test]
async fn schema_detection() {
    let url = require_materialize!();
    let driver = driver(&url, None);
    for sql in [
        "DROP VIEW IF EXISTS drv_v",
        "DROP MATERIALIZED VIEW IF EXISTS drv_mv",
        "DROP TABLE IF EXISTS drv_a",
        "CREATE TABLE drv_a (a INT, b BIGINT, c TEXT, d DOUBLE, e FLOAT)",
        "CREATE VIEW drv_v AS SELECT * FROM drv_a",
        "CREATE MATERIALIZED VIEW drv_mv AS SELECT * FROM drv_a",
    ] {
        query(&driver, sql).await;
    }

    let schema = driver.tables_schema().await.unwrap();
    let public = &schema["public"];
    let mut a: Vec<(String, String)> = public["drv_a"]
        .iter()
        .map(|c| (c.name.clone(), c.type_.clone()))
        .collect();
    a.sort();
    assert_eq!(
        a,
        vec![
            ("a".to_string(), "integer".to_string()),
            ("b".to_string(), "bigint".to_string()),
            ("c".to_string(), "text".to_string()),
            ("d".to_string(), "double precision".to_string()),
            ("e".to_string(), "double precision".to_string()),
        ]
    );
    assert!(public.contains_key("drv_mv"));
    // plain views are not queryable without a cluster to compute them
    assert!(!public.contains_key("drv_v"));

    query(&driver, "DROP VIEW drv_v").await;
    query(&driver, "DROP MATERIALIZED VIEW drv_mv").await;
    query(&driver, "DROP TABLE drv_a").await;
    driver.release().await.unwrap();
}

#[tokio::test]
async fn stream_through_a_cursor() {
    let url = require_materialize!();
    let driver = driver(&url, None);
    driver
        .create_schema_if_not_exists("drv_test")
        .await
        .unwrap();
    // idempotent (`SHOW SCHEMAS WHERE name = …`)
    driver
        .create_schema_if_not_exists("drv_test")
        .await
        .unwrap();
    let _ = driver
        .query(
            "DROP TABLE IF EXISTS drv_test.streaming_test",
            &[],
            &QueryOptions::default(),
        )
        .await;

    // `uploadTable` is BaseDriver's row-by-row insert.
    let data = QueryResult::new(
        vec![
            Column::new("id", "bigint"),
            Column::new("created", "date"),
            Column::new("price", "decimal"),
        ],
        vec![
            vec![json!(1), json!("2020-01-01"), json!("100")],
            vec![json!(2), json!("2020-01-02"), json!("200")],
            vec![json!(3), json!("2020-01-03"), json!("300")],
        ],
    );
    driver
        .upload_table(
            "drv_test.streaming_test",
            &[
                Column::new("id", "bigint"),
                Column::new("created", "date"),
                Column::new("price", "decimal"),
            ],
            &data,
        )
        .await
        .unwrap();

    let stream = driver
        .stream(
            "select * from drv_test.streaming_test where id >= $1 order by id",
            &[json!(1)],
            &StreamOptions {
                high_water_mark: 1000,
                request_id: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        stream.columns,
        vec![
            Column::new("id", "bigint"),
            Column::new("created", "date"),
            Column::new("price", "decimal"),
        ]
    );
    let rows: Vec<Vec<Value>> = stream.rows.try_collect().await.unwrap();
    assert_eq!(
        rows,
        vec![
            vec![json!("1"), json!("2020-01-01T00:00:00.000"), json!("100")],
            vec![json!("2"), json!("2020-01-02T00:00:00.000"), json!("200")],
            vec![json!("3"), json!("2020-01-03T00:00:00.000"), json!("300")],
        ]
    );

    // More rows than one FETCH returns.
    let stream = driver
        .stream(
            "SELECT generate_series AS n FROM generate_series(1, 2500)",
            &[],
            &StreamOptions::default(),
        )
        .await
        .unwrap();
    let rows: Vec<Vec<Value>> = stream.rows.try_collect().await.unwrap();
    assert_eq!(rows.len(), 2500);

    // The connection went back to the pool with its transaction closed.
    let data = query(&driver, "SELECT 1 AS one").await;
    assert_eq!(data.get(0, "one"), Some(&json!(1)));

    query(&driver, "DROP TABLE drv_test.streaming_test").await;
    driver.release().await.unwrap();
}

#[tokio::test]
async fn stream_exception() {
    let url = require_materialize!();
    let driver = driver(&url, None);
    let err = driver
        .stream(
            "select * from public.random_name_for_table_that_doesnot_exist_sql_must_fail",
            &[],
            &StreamOptions::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "unknown catalog item 'public.random_name_for_table_that_doesnot_exist_sql_must_fail'"
    );
    // the failed transaction does not poison the pool
    let data = query(&driver, "SELECT 1 AS one").await;
    assert_eq!(data.get(0, "one"), Some(&json!(1)));
    driver.release().await.unwrap();
}

#[tokio::test]
async fn cluster() {
    let url = require_materialize!();
    let driver = driver(&url, Some("quickstart"));
    let data = query(&driver, "SHOW CLUSTER;").await;
    assert_eq!(data.rows, vec![vec![json!("quickstart")]]);
    let data = query(&driver, "SHOW application_name").await;
    assert_eq!(data.rows, vec![vec![json!("cubejs-materialize-driver")]]);
    driver.release().await.unwrap();
}
