#![cfg(feature = "dremio")]
//! Integration tests for [`DremioDriver`].
//!
//! They run only when `CUBEJS_TEST_DREMIO_HOST` is set, e.g. against
//! `dremio/dremio-oss` with a bootstrapped first user and a `cubetest` space
//! holding the view `orders`:
//!
//! ```text
//! CUBEJS_TEST_DREMIO_HOST=127.0.0.1 CUBEJS_TEST_DREMIO_PORT=16747
//! CUBEJS_TEST_DREMIO_USER=cube CUBEJS_TEST_DREMIO_PASS=cube12345
//! CUBEJS_TEST_DREMIO_SPACE=cubetest
//! ```
//!
//! ```sql
//! CREATE VIEW cubetest.orders AS SELECT 1 AS id, 100 AS amount, 'new' AS status,
//!   TIMESTAMP '2020-01-01 10:00:00' AS created_at
//! ```

use std::time::Duration;

use cubedriver::{Column, DremioConfig, DremioDriver, Driver, DriverConfig, QueryOptions};
use serde_json::{json, Value};

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty())
}

macro_rules! require_dremio {
    () => {
        match env("CUBEJS_TEST_DREMIO_HOST") {
            Some(_) => {}
            None => {
                eprintln!("CUBEJS_TEST_DREMIO_HOST is not set, skipping");
                return;
            }
        }
    };
}

fn software_config() -> DremioConfig {
    let mut driver = DriverConfig::default();
    driver.data_source.host = env("CUBEJS_TEST_DREMIO_HOST");
    driver.data_source.port = env("CUBEJS_TEST_DREMIO_PORT").map(|p| p.parse().unwrap());
    driver.data_source.user = env("CUBEJS_TEST_DREMIO_USER");
    driver.data_source.password = env("CUBEJS_TEST_DREMIO_PASS");
    driver.data_source.database = env("CUBEJS_TEST_DREMIO_SPACE");
    DremioConfig::from_driver_config(driver)
}

fn driver() -> DremioDriver {
    DremioDriver::new(software_config()).unwrap()
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

#[tokio::test]
async fn test_connection_and_driver_tests_query() {
    require_dremio!();
    let driver = driver();
    driver.test_connection().await.unwrap();

    let result = driver
        .query(QUERY, &[], &QueryOptions::default())
        .await
        .unwrap();
    assert_eq!(
        result.columns,
        vec![
            Column::new("id", "int"),
            Column::new("amount", "int"),
            Column::new("status", "text"),
        ]
    );
    // `DriverTests.ROWS` with `expectStringFields: false`.
    assert_eq!(
        result.to_json_rows(),
        vec![
            json!({ "id": 1, "amount": 100, "status": "new" }),
            json!({ "id": 2, "amount": 200, "status": "new" }),
            json!({ "id": 3, "amount": 400, "status": "processed" }),
            json!({ "id": 4, "amount": 500, "status": null }),
        ]
        .into_iter()
        .map(|v| v.as_object().unwrap().clone())
        .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn parameters_are_interpolated_safely() {
    require_dremio!();
    let driver = driver();
    let result = driver
        .query(
            "SELECT ? AS s, ? AS n, ? AS b",
            &[json!(r"o'reilly\"), json!(42), json!(true)],
            &QueryOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(result.get(0, "s"), Some(&json!(r"o'reilly\")));
    assert_eq!(result.get(0, "n"), Some(&json!(42)));
    assert_eq!(result.get(0, "b"), Some(&json!(true)));
}

#[tokio::test]
async fn results_are_paged() {
    require_dremio!();
    let driver = driver();
    let digits = "(VALUES (0), (1), (2), (3), (4), (5), (6), (7), (8), (9))";
    let sql = format!(
        "SELECT a.x * 100 + b.x * 10 + c.x AS n
         FROM {digits} AS a(x) CROSS JOIN {digits} AS b(x) CROSS JOIN {digits} AS c(x)
         UNION ALL SELECT 1000 + a.x * 10 + b.x AS n FROM {digits} AS a(x) CROSS JOIN {digits} AS b(x)
         ORDER BY 1"
    );
    let result = driver
        .query(&sql, &[], &QueryOptions::default())
        .await
        .unwrap();
    assert_eq!(result.len(), 1100);
    for (i, row) in result.rows.iter().enumerate() {
        assert_eq!(row[0], json!(i), "row {i}");
    }
}

#[tokio::test]
async fn empty_results_keep_their_columns() {
    require_dremio!();
    let result = driver()
        .query(
            "SELECT x AS n FROM (VALUES (1)) AS t(x) WHERE x = 0",
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.is_empty());
    assert_eq!(result.columns, vec![Column::new("n", "int")]);
}

#[tokio::test]
async fn failed_jobs_report_dremio_errors() {
    require_dremio!();
    let err = driver()
        .query(
            "SELECT * FROM no_such_table_xyz",
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no_such_table_xyz"), "{err}");
}

#[tokio::test]
async fn poll_timeout_is_enforced() {
    require_dremio!();
    let mut config = software_config();
    config.poll_timeout = Duration::ZERO;
    let err = DremioDriver::new(config)
        .unwrap()
        .query("SELECT 1", &[], &QueryOptions::default())
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "DremioQuery job timeout reached 0ms");
}

#[tokio::test]
async fn wrong_password_fails() {
    require_dremio!();
    let mut config = software_config();
    config.password = Some("wrong-password".into());
    let err = DremioDriver::new(config)
        .unwrap()
        .test_connection()
        .await
        .unwrap_err();
    assert!(!err.to_string().is_empty());
}

#[tokio::test]
async fn tables_schema_refreshes_the_space() {
    require_dremio!();
    let Some(space) = env("CUBEJS_TEST_DREMIO_SPACE") else {
        eprintln!("CUBEJS_TEST_DREMIO_SPACE is not set, skipping");
        return;
    };
    let schema = driver().tables_schema().await.unwrap();
    let orders = schema
        .get(&space)
        .and_then(|t| t.get("orders"))
        .unwrap_or_else(|| panic!("{space}.orders missing from {schema:?}"));
    // `information_schema.columns` has no guaranteed order (nor has Node's query).
    let mut columns: Vec<(&str, &str)> = orders
        .iter()
        .map(|c| (c.name.as_str(), c.type_.as_str()))
        .collect();
    columns.sort();
    assert_eq!(
        columns,
        vec![
            ("amount", "INTEGER"),
            ("created_at", "TIMESTAMP"),
            ("id", "INTEGER"),
            ("status", "CHARACTER VARYING"),
        ]
    );
    assert!(!schema.contains_key("INFORMATION_SCHEMA"));
    assert!(!schema.contains_key("sys.cache"));
}

/// Dremio Cloud mode: `CUBEJS_DB_URL` is the API root and requests carry a
/// `Bearer` token. Dremio Software accepts its session tokens as bearer
/// tokens, so the same code path is exercised against `<host>/api/v3`.
#[tokio::test]
async fn bearer_token_mode() {
    require_dremio!();
    let software = software_config();
    let (url, _) = software.endpoint();
    let login: Value = reqwest::Client::new()
        .post(format!("{url}/apiv2/login"))
        .json(&json!({ "userName": software.user, "password": software.password }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = login["token"].as_str().unwrap().to_string();

    let mut driver_config = DriverConfig::default();
    driver_config.data_source.url = Some(format!("{url}/api/v3"));
    let mut config = DremioConfig::from_driver_config(driver_config.clone());
    config.auth_token = Some(token);
    let driver = DremioDriver::new(config).unwrap();
    driver.test_connection().await.unwrap();
    let result = driver
        .query(QUERY, &[], &QueryOptions::default())
        .await
        .unwrap();
    assert_eq!(result.len(), 4);

    let mut config = DremioConfig::from_driver_config(driver_config);
    config.auth_token = Some("bogus".into());
    let err = DremioDriver::new(config)
        .unwrap()
        .test_connection()
        .await
        .unwrap_err();
    assert!(err.to_string().contains("401"), "{err}");
}
