#![cfg(feature = "oracle")]
//! Integration tests for [`OracleDriver`] against a real Oracle Database.
//!
//! They run only when `CUBEJS_TEST_ORACLE_URL` is set, e.g.
//!
//! ```text
//! docker run -d --name drv-oracle-free -p 16901:1521 -e ORACLE_PASSWORD=Cube_test_123 \
//!   -e APP_USER=cube -e APP_USER_PASSWORD=cube_pw gvenzl/oracle-free:slim
//! CUBEJS_TEST_ORACLE_URL='oracle://cube:cube_pw@127.0.0.1:16901/FREEPDB1'
//! ```
//!
//! No Oracle Instant Client is needed: the driver speaks the thin protocol.

use cubedriver::{
    Column, DownloadQueryResultsOptions, DownloadedData, Driver, DriverError, GenericType,
    OracleConfig, OracleDriver, QueryOptions, QueryResult,
};
use serde_json::{json, Value};

fn oracle_url() -> Option<String> {
    std::env::var("CUBEJS_TEST_ORACLE_URL")
        .ok()
        .filter(|s| !s.is_empty())
}

macro_rules! require_oracle {
    () => {
        match oracle_url() {
            Some(url) => url,
            None => {
                eprintln!("CUBEJS_TEST_ORACLE_URL is not set, skipping");
                return;
            }
        }
    };
}

fn driver(url: &str) -> OracleDriver {
    OracleDriver::new(OracleConfig::from_url(url).unwrap()).unwrap()
}

async fn query(driver: &OracleDriver, sql: &str, params: &[Value]) -> QueryResult {
    driver
        .query(sql, params, &QueryOptions::default())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
}

async fn exec(driver: &OracleDriver, sql: &str) {
    let _ = driver.query(sql, &[], &QueryOptions::default()).await;
}

fn row(result: &QueryResult, i: usize) -> serde_json::Map<String, Value> {
    result.to_json_rows().remove(i)
}

#[tokio::test]
async fn connection_and_scalar_types() {
    let url = require_oracle!();
    let driver = driver(&url);
    driver.test_connection().await.unwrap();

    let data = query(
        &driver,
        "SELECT 1 AS n, 1.5 AS d, -0.25 AS neg, 12345678901234567890123 AS big,
                'abc' AS s, N'ñ' AS ns, CAST('x' AS CHAR(3)) AS c, CAST(NULL AS VARCHAR2(1)) AS nul,
                DATE '2020-01-02' AS dt,
                TIMESTAMP '2020-01-02 03:04:05.123456' AS ts,
                TIMESTAMP '2020-01-02 03:04:05 +02:00' AS tstz,
                CAST(TIMESTAMP '2020-01-02 03:04:05 +02:00' AS TIMESTAMP WITH LOCAL TIME ZONE) AS ltz,
                CAST(1.5 AS BINARY_DOUBLE) AS bd, CAST(0.1 AS BINARY_FLOAT) AS bf,
                TO_CLOB('a clob') AS cl, HEXTORAW('DEADBEEF') AS rw
         FROM DUAL",
        &[],
    )
    .await;
    assert_eq!(
        row(&data, 0),
        json!({
            "N": "1", "D": "1.5", "NEG": "-0.25", "BIG": "12345678901234567890123",
            "S": "abc", "NS": "ñ", "C": "x  ", "NUL": null,
            "DT": "2020-01-02T00:00:00.000Z",
            "TS": "2020-01-02T03:04:05.123Z",
            "TSTZ": "2020-01-02T01:04:05.000Z",
            "LTZ": "2020-01-02T01:04:05.000Z",
            "BD": "1.5", "BF": "0.1",
            "CL": "a clob", "RW": "3q2+7w=="
        })
        .as_object()
        .unwrap()
        .clone()
    );
    let types: Vec<(String, String)> = data
        .columns
        .iter()
        .map(|c| (c.name.clone(), c.type_.to_string()))
        .collect();
    assert_eq!(
        types,
        [
            ("N", "decimal"),
            ("D", "decimal"),
            ("NEG", "decimal"),
            ("BIG", "decimal"),
            ("S", "text"),
            ("NS", "text"),
            ("C", "text"),
            ("NUL", "text"),
            ("DT", "timestamp"),
            ("TS", "timestamp"),
            ("TSTZ", "timestamp"),
            ("LTZ", "timestamp"),
            ("BD", "double"),
            ("BF", "float"),
            ("CL", "text"),
            ("RW", "text"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect::<Vec<_>>()
    );
    driver.release().await.unwrap();
}

#[tokio::test]
async fn parameters_are_named_binds() {
    let url = require_oracle!();
    let driver = driver(&url);

    let data = query(
        &driver,
        "SELECT ? AS a, ? AS b, ? AS c, ? AS d FROM DUAL",
        &[json!(42), json!("text"), json!(null), json!(2.5)],
    )
    .await;
    assert_eq!(
        row(&data, 0),
        json!({"A": "42", "B": "text", "C": null, "D": "2.5"})
            .as_object()
            .unwrap()
            .clone()
    );

    // Repeated values share one bind, so the CASE in SELECT and GROUP BY is
    // textually identical (ORA-00979 otherwise).
    let data = query(
        &driver,
        "SELECT CASE WHEN lvl > ? THEN 'big' ELSE 'small' END AS k, COUNT(*) AS cnt
         FROM (SELECT LEVEL lvl FROM DUAL CONNECT BY LEVEL <= 5)
         GROUP BY CASE WHEN lvl > ? THEN 'big' ELSE 'small' END
         ORDER BY 1",
        &[json!(3), json!(3)],
    )
    .await;
    assert_eq!(
        data.rows,
        vec![
            vec![json!("big"), json!("2")],
            vec![json!("small"), json!("3")]
        ]
    );

    // The session NLS formats make ISO date strings convert implicitly.
    let data = query(
        &driver,
        "SELECT CAST(? AS TIMESTAMP) AS ts, CAST(? AS DATE) AS dt FROM DUAL",
        &[json!("2021-03-04"), json!("2021-03-05")],
    )
    .await;
    assert_eq!(
        data.rows[0],
        vec![
            json!("2021-03-04T00:00:00.000Z"),
            json!("2021-03-05T00:00:00.000Z")
        ]
    );
    driver.release().await.unwrap();
}

#[tokio::test]
async fn limit_wrapping_errors_and_row_cap() {
    let url = require_oracle!();
    let driver = driver(&url);

    let wrapped =
        driver.wrap_query_with_limit("SELECT LEVEL AS l FROM DUAL CONNECT BY LEVEL <= 10", 3);
    let data = query(&driver, &wrapped, &[]).await;
    assert_eq!(data.len(), 3);

    let err = driver
        .query("SELECT * FROM no_such_table", &[], &QueryOptions::default())
        .await
        .unwrap_err();
    match err {
        DriverError::Database { message, code } => {
            assert!(message.contains("ORA-00942"), "{message}");
            assert_eq!(code.as_deref(), Some("ORA-00942"));
        }
        other => panic!("{other:?}"),
    }

    let mut config = OracleConfig::from_url(&url).unwrap();
    config.max_rows = 5;
    let capped = OracleDriver::new(config).unwrap();
    let data = query(
        &capped,
        "SELECT LEVEL FROM DUAL CONNECT BY LEVEL <= 10",
        &[],
    )
    .await;
    assert_eq!(data.len(), 5);

    // Region-named zones are refused by the client with a named error, not a crash.
    let res = driver
        .query(
            "SELECT TIMESTAMP '2020-01-01 00:00:00 Europe/Paris' AS t FROM DUAL",
            &[],
            &QueryOptions::default(),
        )
        .await;
    match res {
        Err(DriverError::NotImplemented(message)) => {
            assert!(message.contains("SYS_EXTRACT_UTC"), "{message}")
        }
        other => panic!("expected a named error, got {other:?}"),
    }
    let data = query(
        &driver,
        "SELECT SYS_EXTRACT_UTC(TIMESTAMP '2020-01-01 00:00:00 Europe/Paris') AS t FROM DUAL",
        &[],
    )
    .await;
    assert_eq!(data.rows[0][0], json!("2019-12-31T23:00:00.000Z"));
    // the pool still works afterwards
    assert_eq!(
        query(&driver, "SELECT 1 AS x FROM DUAL", &[]).await.len(),
        1
    );
    driver.release().await.unwrap();
}

#[tokio::test]
async fn schema_ddl_and_download() {
    let url = require_oracle!();
    let driver = driver(&url);
    exec(&driver, "DROP TABLE cube_rs_orders").await;
    query(
        &driver,
        "CREATE TABLE cube_rs_orders (id NUMBER(10) PRIMARY KEY, amount NUMBER(12,2) NOT NULL,
            status VARCHAR2(20), created_at TIMESTAMP, code VARCHAR2(5) UNIQUE)",
        &[],
    )
    .await;
    query(
        &driver,
        "INSERT INTO cube_rs_orders VALUES (?, ?, ?, ?, ?)",
        &[
            json!(1),
            json!(10.5),
            json!("new"),
            json!("2020-01-01"),
            json!("A"),
        ],
    )
    .await;
    query(
        &driver,
        "INSERT INTO cube_rs_orders VALUES (2, 20, 'done', TIMESTAMP '2020-02-01 10:00:00', 'B')",
        &[],
    )
    .await;

    // committed: visible from another pooled session too
    let other = OracleDriver::new(OracleConfig::from_url(&url).unwrap()).unwrap();
    let data = query(&other, "SELECT COUNT(*) AS c FROM cube_rs_orders", &[]).await;
    assert_eq!(data.rows[0][0], json!("2"));

    let structure = driver.tables_schema().await.unwrap();
    let schema = structure
        .values()
        .find(|s| s.contains_key("CUBE_RS_ORDERS"))
        .expect("table in schema");
    let columns = &schema["CUBE_RS_ORDERS"];
    let summary: Vec<(String, String, bool)> = columns
        .iter()
        .map(|c| (c.name.clone(), c.type_.clone(), !c.attributes.is_empty()))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("ID".to_string(), "NUMBER".to_string(), true),
            ("AMOUNT".to_string(), "NUMBER".to_string(), false),
            ("STATUS".to_string(), "VARCHAR2".to_string(), false),
            ("CREATED_AT".to_string(), "TIMESTAMP(6)".to_string(), false),
            ("CODE".to_string(), "VARCHAR2".to_string(), true),
        ]
    );

    let downloaded = driver
        .download_query_results(
            "SELECT id, amount, status, created_at FROM cube_rs_orders ORDER BY id",
            &[],
            &DownloadQueryResultsOptions::default(),
        )
        .await
        .unwrap();
    let DownloadedData::Memory(m) = downloaded else {
        panic!("expected memory data")
    };
    assert_eq!(
        m.columns,
        vec![
            Column::new("ID", GenericType::Decimal(None)),
            Column::new("AMOUNT", GenericType::Decimal(None)),
            Column::new("STATUS", GenericType::Text),
            Column::new("CREATED_AT", GenericType::Timestamp),
        ]
    );
    assert_eq!(
        m.rows,
        vec![
            vec![
                json!("1"),
                json!("10.5"),
                json!("new"),
                json!("2020-01-01T00:00:00.000Z")
            ],
            vec![
                json!("2"),
                json!("20"),
                json!("done"),
                json!("2020-02-01T10:00:00.000Z")
            ],
        ]
    );

    driver
        .drop_table("cube_rs_orders", &QueryOptions::default())
        .await
        .unwrap();
    driver.release().await.unwrap();
    other.release().await.unwrap();
}

#[tokio::test]
async fn pool_is_shared_and_reused() {
    let url = require_oracle!();
    let mut config = OracleConfig::from_url(&url).unwrap();
    config.max_pool_size = 2;
    let driver = std::sync::Arc::new(OracleDriver::new(config).unwrap());
    let tasks: Vec<_> = (0..6)
        .map(|i| {
            let driver = driver.clone();
            tokio::spawn(async move {
                driver
                    .query(
                        "SELECT ? AS v FROM DUAL",
                        &[json!(i)],
                        &QueryOptions::default(),
                    )
                    .await
                    .unwrap()
            })
        })
        .collect();
    for (i, t) in tasks.into_iter().enumerate() {
        assert_eq!(t.await.unwrap().rows[0][0], json!(i.to_string()));
    }
    assert!(driver.pool_size() <= 2 && driver.pool_size() >= 1);
    driver.release().await.unwrap();
    assert_eq!(driver.pool_size(), 0);
}

#[tokio::test]
async fn bad_credentials_are_a_connection_error() {
    let url = require_oracle!();
    let mut config = OracleConfig::from_url(&url).unwrap();
    config.password = Some("wrong".to_string());
    let driver = OracleDriver::new(config).unwrap();
    let err = driver.test_connection().await.unwrap_err();
    assert!(
        matches!(err, DriverError::Connection { .. }),
        "unexpected error: {err:?}"
    );
    assert!(err.to_string().contains("ORA-01017"), "{err}");
}
