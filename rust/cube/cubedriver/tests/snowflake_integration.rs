//! Integration tests for [`SnowflakeDriver`].
//!
//! Snowflake is cloud only, so these tests run exclusively when
//! `CUBEJS_TEST_SNOWFLAKE=true` and a key-pair (or OAuth) configuration is
//! present:
//!
//! ```text
//! CUBEJS_TEST_SNOWFLAKE=true
//! CUBEJS_DB_SNOWFLAKE_ACCOUNT=<account>
//! CUBEJS_DB_SNOWFLAKE_WAREHOUSE=<warehouse>
//! CUBEJS_DB_USER=<user>
//! CUBEJS_DB_NAME=<database>
//! CUBEJS_DB_SNOWFLAKE_PRIVATE_KEY_PATH=/path/to/rsa_key.p8
//! ```

use cubedriver::{Driver, QueryOptions, SnowflakeConfig, SnowflakeDriver};
use serde_json::{json, Value};

fn enabled() -> bool {
    std::env::var("CUBEJS_TEST_SNOWFLAKE").as_deref() == Ok("true")
}

macro_rules! require_snowflake {
    () => {
        if !enabled() {
            eprintln!("CUBEJS_TEST_SNOWFLAKE is not 'true', skipping");
            return;
        } else {
            SnowflakeDriver::new(SnowflakeConfig::from_env(None).unwrap()).unwrap()
        }
    };
}

#[tokio::test]
async fn test_connection_and_scalar_types() {
    let driver = require_snowflake!();
    driver.test_connection().await.unwrap();

    let data = driver
        .query(
            "SELECT
               1 AS i,
               9007199254740993::NUMBER(38, 0) AS big,
               1.25::NUMBER(10, 2) AS amount,
               1.5::FLOAT AS dbl,
               TRUE AS flag,
               'foo' AS s,
               '2020-01-01'::DATE AS d,
               '2020-01-01 12:34:56.789'::TIMESTAMP_NTZ AS ts,
               NULL::TEXT AS nothing",
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap();

    assert_eq!(data.len(), 1);
    // `getTypes` lower-cases the column names
    assert_eq!(data.get(0, "big"), Some(&json!("9007199254740993")));
    assert_eq!(data.get(0, "amount"), Some(&json!("1.25")));
    assert_eq!(data.get(0, "flag"), Some(&json!(true)));
    assert_eq!(data.get(0, "s"), Some(&json!("foo")));
    assert_eq!(data.get(0, "d"), Some(&json!("2020-01-01T00:00:00.000")));
    assert_eq!(data.get(0, "ts"), Some(&json!("2020-01-01T12:34:56.789")));
    assert_eq!(data.get(0, "nothing"), Some(&Value::Null));
}

#[tokio::test]
async fn parameters_are_bound() {
    let driver = require_snowflake!();
    let data = driver
        .query(
            "SELECT ? AS s, ? + 1 AS n",
            &[json!("x"), json!(41)],
            &QueryOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(data.get(0, "s"), Some(&json!("x")));
    assert_eq!(data.get(0, "n"), Some(&json!("42")));
}

#[tokio::test]
async fn schema_introspection() {
    let driver = require_snowflake!();
    let schema =
        std::env::var("CUBEJS_TEST_SNOWFLAKE_SCHEMA").unwrap_or_else(|_| "PUBLIC".to_string());

    let table = format!("{schema}.CUBE_DRIVER_TEST");
    driver
        .query(
            &format!("CREATE OR REPLACE TABLE {table} (ID NUMBER(38, 0), AMOUNT NUMBER(10, 2))"),
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap();

    let types = driver.table_column_types(&table).await.unwrap();
    assert_eq!(types.len(), 2);
    // `NUMBER` with scale 0 is reported as `int` by the information schema query
    assert_eq!(types[0].type_.to_string(), "int");
    assert_eq!(types[1].type_.to_string(), "decimal");

    let tables = driver.get_tables_query(&schema).await.unwrap();
    assert!(tables.contains(&"cube_driver_test".to_string()));

    let structure = driver.tables_schema().await.unwrap();
    assert!(structure.contains_key(&schema.to_uppercase()));

    driver
        .query(
            &format!("DROP TABLE {table}"),
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap();
}
