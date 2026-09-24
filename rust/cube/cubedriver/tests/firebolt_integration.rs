//! Integration tests for [`FireboltDriver`].
//!
//! Firebolt is cloud only, so these tests run exclusively when
//! `CUBEJS_TEST_FIREBOLT=true` and a service account is configured:
//!
//! ```text
//! CUBEJS_TEST_FIREBOLT=true
//! CUBEJS_DB_USER=<client id>        CUBEJS_DB_PASS=<client secret>
//! CUBEJS_DB_NAME=<database>         CUBEJS_FIREBOLT_ACCOUNT=<account>
//! CUBEJS_FIREBOLT_ENGINE_NAME=<engine>
//! ```

use cubedriver::{Column, Driver, FireboltConfig, FireboltDriver, QueryOptions};
use serde_json::{json, Value};

fn enabled() -> bool {
    std::env::var("CUBEJS_TEST_FIREBOLT").as_deref() == Ok("true")
}

macro_rules! require_firebolt {
    () => {
        if !enabled() {
            eprintln!("CUBEJS_TEST_FIREBOLT is not 'true', skipping");
            return;
        } else {
            FireboltDriver::new(FireboltConfig::from_env(None).unwrap()).unwrap()
        }
    };
}

#[tokio::test]
async fn test_connection_and_scalar_types() {
    let driver = require_firebolt!();
    driver.test_connection().await.unwrap();

    let data = driver
        .query(
            "SELECT
               1 AS i,
               CAST(2 AS BIGINT) AS big,
               CAST(1.5 AS DOUBLE PRECISION) AS dbl,
               'foo' AS s,
               true AS flag,
               NULL AS nothing",
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap();

    assert_eq!(data.len(), 1);
    // numeric values are stringified (`getHydratedValue`)
    assert_eq!(data.get(0, "i"), Some(&json!("1")));
    assert_eq!(data.get(0, "big"), Some(&json!("2")));
    assert_eq!(data.get(0, "dbl"), Some(&json!("1.5")));
    assert_eq!(data.get(0, "s"), Some(&json!("foo")));
    assert_eq!(data.get(0, "flag"), Some(&json!(true)));
    assert_eq!(data.get(0, "nothing"), Some(&Value::Null));
}

#[tokio::test]
async fn parameters_are_interpolated() {
    let driver = require_firebolt!();
    let data = driver
        .query(
            "SELECT ? AS s, ? + 1 AS n",
            &[json!("it's"), json!(41)],
            &QueryOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(data.get(0, "s"), Some(&json!("it's")));
    assert_eq!(data.get(0, "n"), Some(&json!("42")));
}

#[tokio::test]
async fn tables_and_column_types() {
    let driver = require_firebolt!();
    driver
        .query(
            "DROP TABLE IF EXISTS cube_driver_test",
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap();
    driver
        .query(
            &driver.create_table_sql(
                "cube_driver_test",
                &[Column::new("id", "bigint"), Column::new("name", "string")],
            ),
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap();

    let tables = driver.get_tables_query("public").await.unwrap();
    assert!(tables.contains(&"cube_driver_test".to_string()));

    let types = driver.table_column_types("cube_driver_test").await.unwrap();
    assert_eq!(types.len(), 2);
    assert_eq!(types[0].name, "id");

    driver
        .drop_table("public.cube_driver_test", &QueryOptions::default())
        .await
        .unwrap();
}
