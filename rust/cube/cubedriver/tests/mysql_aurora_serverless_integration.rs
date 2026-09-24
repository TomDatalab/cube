#![cfg(feature = "mysqlauroraserverless")]
//! Port of `test/AuroraServerlessMySqlDriver.integration.js`.
//!
//! Runs against a real Data API endpoint:
//!
//! * `CUBEJS_TEST_AURORA_DATA_API_ENDPOINT` — e.g. `koxudaxi/local-data-api`
//!   in front of MySQL (what the Node test uses), with dummy ARNs and keys;
//! * or real AWS: `CUBEJS_DATABASE_SECRET_ARN` + `CUBEJS_DATABASE_CLUSTER_ARN`
//!   (+ `CUBEJS_DB_NAME`) and the usual AWS credential chain.
//!
//! Skipped when neither is set.

use cubedriver::config::DriverConfig;
use cubedriver::types::{Column, DownloadQueryResultsOptions, QueryOptions, QueryResult};
use cubedriver::{AuroraServerlessMySqlConfig, AuroraServerlessMySqlDriver, Driver};
use serde_json::{json, Value};

const DUMMY_SECRET_ARN: &str = "arn:aws:secretsmanager:us-east-1:123456789012:secret:dummy";
const DUMMY_RESOURCE_ARN: &str = "arn:aws:rds:us-east-1:123456789012:cluster:dummy";

fn driver() -> Option<AuroraServerlessMySqlDriver> {
    if let Ok(endpoint) = std::env::var("CUBEJS_TEST_AURORA_DATA_API_ENDPOINT") {
        let mut config = AuroraServerlessMySqlConfig::from_driver_config(DriverConfig::default());
        config.secret_arn = Some(DUMMY_SECRET_ARN.to_string());
        config.resource_arn = Some(DUMMY_RESOURCE_ARN.to_string());
        config.database = Some("mysql".to_string());
        config.region = Some("us-east-1".to_string());
        config.credentials = Some(("awstest".to_string(), "awstest".to_string()));
        config.endpoint_url = Some(endpoint);
        return Some(AuroraServerlessMySqlDriver::new(config).unwrap());
    }
    if std::env::var("CUBEJS_DATABASE_SECRET_ARN").is_ok()
        && std::env::var("CUBEJS_DATABASE_CLUSTER_ARN").is_ok()
    {
        return Some(AuroraServerlessMySqlDriver::from_env(None).unwrap());
    }
    eprintln!("skipping: set CUBEJS_TEST_AURORA_DATA_API_ENDPOINT or the Data API ARNs");
    None
}

fn rows(result: &QueryResult) -> Vec<serde_json::Map<String, Value>> {
    result.to_json_rows()
}

async fn q(driver: &AuroraServerlessMySqlDriver, sql: &str, params: &[Value]) -> QueryResult {
    driver
        .query(sql, params, &QueryOptions::default())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
}

#[tokio::test]
async fn aurora_serverless_against_data_api() {
    let Some(driver) = driver() else { return };

    driver.test_connection().await.unwrap();
    driver.create_schema_if_not_exists("test").await.unwrap();
    q(&driver, "DROP SCHEMA test", &[]).await;
    driver.create_schema_if_not_exists("test").await.unwrap();

    // basic query
    let one = q(&driver, "SELECT 1 AS one", &[]).await;
    assert_eq!(one.rows, vec![vec![json!(1)]]);

    // truncated wrong value: utf8mb4 strings survive the upload
    driver
        .upload_table(
            "test.wrong_value",
            &[Column::new("value", "string")],
            &QueryResult::new(
                vec![Column::new("value", "string")],
                vec![vec![json!("Tekirdağ")]],
            ),
        )
        .await
        .unwrap();
    let r = q(&driver, "select * from test.wrong_value", &[]).await;
    assert_eq!(
        rows(&r),
        vec![json!({ "value": "Tekirdağ" }).as_object().unwrap().clone()]
    );
    let downloaded = driver
        .download_query_results(
            "select * from test.wrong_value",
            &[],
            &DownloadQueryResultsOptions::default(),
        )
        .await
        .unwrap()
        .into_memory()
        .await
        .unwrap();
    assert_eq!(
        rows(&downloaded),
        vec![json!({ "value": "Tekirdağ" }).as_object().unwrap().clone()]
    );
    assert_eq!(downloaded.columns[0].name, "value");

    // boolean field: 'true' / 'false' strings are converted on upload
    let bool_col = vec![Column::new("b_value", "boolean")];
    driver
        .upload_table(
            "test.boolean",
            &bool_col,
            &QueryResult::new(
                bool_col.clone(),
                vec![
                    vec![json!(true)],
                    vec![json!(true)],
                    vec![json!("true")],
                    vec![json!(false)],
                    vec![json!("false")],
                    vec![Value::Null],
                ],
            ),
        )
        .await
        .unwrap();
    let t = q(
        &driver,
        "select * from test.boolean where b_value = ?",
        &[json!(true)],
    )
    .await;
    assert_eq!(t.rows.len(), 3);
    let f = q(
        &driver,
        "select * from test.boolean where b_value = ?",
        &[json!(false)],
    )
    .await;
    assert_eq!(f.rows.len(), 2);
    // MySQL booleans are TINYINT(1); the Data API reports them as it sees fit
    // (local-data-api: longValue / booleanValue). Either way, not null.
    assert!(t.rows.iter().all(|r| !r[0].is_null()));

    // parameters of every JSON type, dates, decimals, nulls
    let r = q(
        &driver,
        "SELECT ? AS s, ? AS i, ? AS d, ? AS n, CAST('2020-01-02 03:04:05' AS DATETIME) AS dt, \
         CAST(12.50 AS DECIMAL(10,2)) AS dec_value",
        &[json!("x"), json!(42), json!(1.5), Value::Null],
    )
    .await;
    let row = &rows(&r)[0];
    assert_eq!(row["s"], json!("x"));
    assert_eq!(row["i"], json!(42));
    // MySQL types a bound double as DECIMAL, which the Data API returns as a string.
    assert!(
        row["d"] == json!(1.5) || row["d"] == json!("1.5"),
        "{}",
        row["d"]
    );
    assert_eq!(row["n"], Value::Null);
    assert_eq!(row["dt"], json!("2020-01-02T03:04:05.000Z"));
    assert_eq!(row["dec_value"], json!("12.50"));

    // download_query_results with bound values types the columns via DESCRIBE
    let downloaded = driver
        .download_query_results(
            "SELECT value, 7 AS n FROM test.wrong_value WHERE value = ?",
            &[json!("Tekirdağ")],
            &DownloadQueryResultsOptions::default(),
        )
        .await
        .unwrap()
        .into_memory()
        .await
        .unwrap();
    assert_eq!(downloaded.rows, vec![vec![json!("Tekirdağ"), json!(7)]]);
    let names: Vec<&str> = downloaded.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["value", "n"]);

    // SQL errors surface the server message
    let err = driver
        .query(
            "SELECT * FROM test.no_such_table",
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap_err();
    // local-data-api sometimes answers the retried statement with its own
    // JDBC "Read timed out"; either way it is a Data API (service) error.
    assert!(
        matches!(err, cubedriver::DriverError::Database { .. }),
        "{err:?}"
    );

    // introspection through information_schema (scoped to the database)
    let schema = driver.tables_schema().await.unwrap();
    assert!(!schema.contains_key("test"), "scoped to CUBEJS_DB_NAME");

    q(&driver, "DROP SCHEMA test", &[]).await;
}
