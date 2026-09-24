#![cfg(feature = "vertica")]
//! Integration tests for [`VerticaDriver`], ported from
//! `packages/cubejs-vertica-driver/test/VerticaDriver.test.js`.
//!
//! They run only when `CUBEJS_TEST_VERTICA_URL` is set, e.g.
//! `CUBEJS_TEST_VERTICA_URL=vertica://dbadmin@localhost:5433/docker`. The
//! user must be allowed to create schemas and users.

use cubedriver::{Column, Driver, QueryOptions, QueryResult, VerticaConfig, VerticaDriver};
use serde_json::{json, Value};

fn vertica_url() -> Option<String> {
    std::env::var("CUBEJS_TEST_VERTICA_URL")
        .ok()
        .filter(|s| !s.is_empty())
}

macro_rules! require_vertica {
    () => {
        match vertica_url() {
            Some(url) => url,
            None => {
                eprintln!("CUBEJS_TEST_VERTICA_URL is not set, skipping");
                return;
            }
        }
    };
}

fn driver(url: &str) -> VerticaDriver {
    VerticaDriver::new(VerticaConfig::from_url(url).unwrap()).unwrap()
}

async fn query(driver: &VerticaDriver, sql: &str, params: &[Value]) -> QueryResult {
    driver
        .query(sql, params, &QueryOptions::default())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
}

#[tokio::test]
async fn test_connection() {
    let url = require_vertica!();
    let driver = driver(&url);
    driver.test_connection().await.unwrap();
    let data = query(&driver, "SELECT 1 AS n", &[]).await;
    assert_eq!(data.to_json_rows()[0]["n"], json!(1));
    driver.release().await.unwrap();
}

#[tokio::test]
async fn default_timezone_is_utc() {
    let url = require_vertica!();
    let driver = driver(&url);
    let data = query(&driver, "SHOW TIMEZONE", &[]).await;
    assert_eq!(data.get_string(0, "name").as_deref(), Some("timezone"));
    assert_eq!(data.get_string(0, "setting").as_deref(), Some("UTC"));
    driver.release().await.unwrap();
}

#[tokio::test]
async fn simple_query() {
    let url = require_vertica!();
    let driver = driver(&url);
    let data = query(
        &driver,
        "
        SELECT
          '2020-01-01'::date                      AS date,
          '2020-01-01 00:00:00'::timestamp        AS timestamp,
          '2020-01-01 21:30:45.015004'::timestamp AS timestamp_us,
          '2020-01-01 00:00:00+02'::timestamptz   AS timestamptz,
          '1.01'::decimal(10,2)                   AS decimal,
          1::int                                  AS integer
      ",
        &[],
    )
    .await;
    assert_eq!(
        data.rows,
        vec![vec![
            json!("2020-01-01"),
            json!("2020-01-01 00:00:00"),
            json!("2020-01-01 21:30:45.015004"),
            json!("2019-12-31 22:00:00+00"),
            json!("1.01"),
            json!(1),
        ]]
    );
    assert_eq!(
        data.columns,
        vec![
            Column::new("date", "date"),
            Column::new("timestamp", "timestamp"),
            Column::new("timestamp_us", "timestamp"),
            Column::new("timestamptz", "timestamp"),
            Column::new("decimal", "decimal"),
            Column::new("integer", "bigint"),
        ]
    );
    driver.release().await.unwrap();
}

#[tokio::test]
async fn parameterized_query() {
    let url = require_vertica!();
    let driver = driver(&url);
    let data = query(
        &driver,
        "
        WITH testdata AS (
          select 1 as id, 'foo' as val union all
          select 2 as id, 'bar' as val union all
          select 3 as id, 'baz' as val union all
          select 4 as id, 'qux' as val
        )
        SELECT *
        FROM testdata
        WHERE id = ?
           OR val = ?
        ORDER BY id
      ",
        &[json!(1), json!("baz")],
    )
    .await;
    assert_eq!(
        data.rows,
        vec![vec![json!(1), json!("foo")], vec![json!(3), json!("baz")]]
    );

    // quotes are escaped, not interpreted
    let data = query(
        &driver,
        "SELECT ? AS s, ? AS b, ? AS n",
        &[json!("it's"), json!(true), Value::Null],
    )
    .await;
    assert_eq!(
        data.rows,
        vec![vec![json!("it's"), json!(true), Value::Null]]
    );
    driver.release().await.unwrap();
}

#[tokio::test]
async fn get_tables_column_types_and_schema() {
    let url = require_vertica!();
    let driver = driver(&url);
    query(
        &driver,
        "DROP SCHEMA IF EXISTS test_get_tables CASCADE;",
        &[],
    )
    .await;
    query(&driver, "CREATE SCHEMA test_get_tables;", &[]).await;
    query(&driver, "CREATE TABLE test_get_tables.tab (id int);", &[]).await;
    assert_eq!(
        driver.get_tables_query("test_get_tables").await.unwrap(),
        vec!["tab".to_string()]
    );

    query(
        &driver,
        "DROP SCHEMA IF EXISTS test_column_types CASCADE;",
        &[],
    )
    .await;
    query(&driver, "CREATE SCHEMA test_column_types;", &[]).await;
    query(
        &driver,
        "
      CREATE TABLE test_column_types.tab (
        integer_col   int,
        date_col      date,
        timestamp_col timestamp,
        decimal_col   decimal(10,2),
        varchar_col   varchar(64),
        char_col      char(2)
      );
    ",
        &[],
    )
    .await;
    let columns = driver
        .table_column_types("test_column_types.tab")
        .await
        .unwrap();
    assert_eq!(
        columns,
        vec![
            Column::new("integer_col", "bigint"),
            Column::new("date_col", "date"),
            Column::new("timestamp_col", "timestamp"),
            Column::new("decimal_col", "decimal"),
            Column::new("varchar_col", "text"),
            Column::new("char_col", "text"),
        ]
    );

    let schema = driver.tables_schema().await.unwrap();
    let tab = &schema["test_column_types"]["tab"];
    assert!(tab
        .iter()
        .any(|c| c.name == "decimal_col" && c.type_ == "numeric(10,2)"));

    query(&driver, "DROP SCHEMA test_get_tables CASCADE;", &[]).await;
    query(&driver, "DROP SCHEMA test_column_types CASCADE;", &[]).await;
    driver.release().await.unwrap();
}

#[tokio::test]
async fn create_schema() {
    let url = require_vertica!();
    let driver = driver(&url);
    query(&driver, "DROP SCHEMA IF EXISTS new_schema CASCADE;", &[]).await;
    driver
        .create_schema_if_not_exists("new_schema")
        .await
        .unwrap();
    driver
        .create_schema_if_not_exists("new_schema")
        .await
        .unwrap();
    let data = query(
        &driver,
        "
      SELECT count(1) AS cnt
      FROM v_catalog.schemata
      WHERE schema_name = 'new_schema';
    ",
        &[],
    )
    .await;
    assert_eq!(data.rows, vec![vec![json!(1)]]);
    query(&driver, "DROP SCHEMA new_schema CASCADE;", &[]).await;
    driver.release().await.unwrap();
}

#[tokio::test]
async fn errors_and_password_authentication() {
    let url = require_vertica!();
    let driver = driver(&url);
    let err = driver
        .query("select nope", &[], &QueryOptions::default())
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "Column \"nope\" does not exist");
    // the connection is still usable after an error
    driver.test_connection().await.unwrap();

    query(&driver, "DROP USER IF EXISTS drv_pgwire_pw;", &[]).await;
    query(
        &driver,
        "CREATE USER drv_pgwire_pw IDENTIFIED BY 'se''cret';",
        &[],
    )
    .await;

    let mut config = VerticaConfig::from_url(&url).unwrap();
    config.driver.data_source.user = Some("drv_pgwire_pw".to_string());
    config.driver.data_source.password = Some("se'cret".to_string());
    let user_driver = VerticaDriver::new(config.clone()).unwrap();
    let data = query(&user_driver, "SELECT current_user() AS u", &[]).await;
    assert_eq!(data.rows, vec![vec![json!("drv_pgwire_pw")]]);
    user_driver.release().await.unwrap();

    config.driver.data_source.password = Some("wrong".to_string());
    let bad = VerticaDriver::new(config).unwrap();
    assert!(bad.test_connection().await.is_err());

    query(&driver, "DROP USER drv_pgwire_pw;", &[]).await;
    driver.release().await.unwrap();
}

/// Hash (`MD5` / `SHA512`) authentication. Needs a user granted an
/// authentication record of method `hash`, e.g.
/// `CUBEJS_TEST_VERTICA_HASH_URL=vertica://h512:secret@localhost:5433/docker`
/// (with `ALTER USER h512 SECURITY_ALGORITHM 'SHA512' IDENTIFIED BY 'secret'`).
#[tokio::test]
async fn hash_authentication() {
    let Some(url) = std::env::var("CUBEJS_TEST_VERTICA_HASH_URL")
        .ok()
        .filter(|s| !s.is_empty())
    else {
        eprintln!("CUBEJS_TEST_VERTICA_HASH_URL is not set, skipping");
        return;
    };
    let driver = driver(&url);
    driver.test_connection().await.unwrap();
    driver.release().await.unwrap();

    let mut config = VerticaConfig::from_url(&url).unwrap();
    config.driver.data_source.password = Some("wrong".to_string());
    let bad = VerticaDriver::new(config).unwrap();
    assert!(bad.test_connection().await.is_err());
}
