//! Integration tests for [`RedshiftDriver`].
//!
//! They run only when `CUBEJS_TEST_REDSHIFT_URL` is set, e.g.
//! `CUBEJS_TEST_REDSHIFT_URL=postgres://user:pass@cluster.eu-central-1.redshift.amazonaws.com:5439/dev`.
//!
//! Redshift speaks the PostgreSQL wire protocol, so the tests below also pass
//! against a plain PostgreSQL server, which is how the wrapper is exercised in
//! CI. The Redshift-only statements (`SHOW SCHEMAS FROM DATABASE`, external
//! Spectrum schemas) additionally need `CUBEJS_TEST_REDSHIFT_NATIVE=true`.

use cubedriver::{
    Column, Driver, QueryOptions, QueryResult, RedshiftConfig, RedshiftDriver, SchemaName,
    StreamOptions,
};
use futures::TryStreamExt;
use serde_json::{json, Value};

fn redshift_url() -> Option<String> {
    std::env::var("CUBEJS_TEST_REDSHIFT_URL")
        .ok()
        .filter(|s| !s.is_empty())
}

fn is_native_redshift() -> bool {
    std::env::var("CUBEJS_TEST_REDSHIFT_NATIVE").as_deref() == Ok("true")
}

macro_rules! require_redshift {
    () => {
        match redshift_url() {
            Some(url) => url,
            None => {
                eprintln!("CUBEJS_TEST_REDSHIFT_URL is not set, skipping");
                return;
            }
        }
    };
}

fn driver(url: &str) -> RedshiftDriver {
    RedshiftDriver::new(RedshiftConfig::from_url(url)).unwrap()
}

async fn query(driver: &RedshiftDriver, sql: &str, params: &[Value]) -> QueryResult {
    driver
        .query(sql, params, &QueryOptions::default())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
}

async fn exec(driver: &RedshiftDriver, sql: &str) {
    let _ = driver.query(sql, &[], &QueryOptions::default()).await;
}

#[tokio::test]
async fn test_connection_and_query() {
    let url = require_redshift!();
    let driver = driver(&url);
    // Only the pool connection is checked; no statement is billed.
    driver.test_connection().await.unwrap();

    let data = query(
        &driver,
        "SELECT $1::int AS n, $2::text AS s, $3::timestamp AS ts",
        &[json!("41"), json!("x"), json!("2020-01-01 10:00:00")],
    )
    .await;
    assert_eq!(data.get(0, "n"), Some(&json!(41)));
    assert_eq!(data.get(0, "s"), Some(&json!("x")));
    assert_eq!(data.get(0, "ts"), Some(&json!("2020-01-01T10:00:00.000")));
    assert_eq!(
        data.columns,
        vec![
            Column::new("n", "int"),
            Column::new("s", "text"),
            Column::new("ts", "timestamp"),
        ]
    );
    driver.release().await.unwrap();
}

#[tokio::test]
async fn parameter_limit_is_enforced_before_the_server() {
    let url = require_redshift!();
    let driver = driver(&url);
    let params: Vec<Value> = vec![json!("x"); 32_768];
    let err = driver
        .query("SELECT 1", &params, &QueryOptions::default())
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Redshift server does not support more than 32767 parameters, but 32768 passed"
    );
    driver.release().await.unwrap();
}

#[tokio::test]
async fn upload_introspect_and_stream() {
    let url = require_redshift!();
    let driver = driver(&url);
    driver
        .create_schema_if_not_exists("redshift_test")
        .await
        .unwrap();
    // idempotent (the check goes through pg_namespace)
    driver
        .create_schema_if_not_exists("redshift_test")
        .await
        .unwrap();
    exec(&driver, "DROP TABLE redshift_test.uploaded").await;

    let columns = vec![
        Column::new("id", "bigint"),
        Column::new("name", "string"),
        Column::new("price", "decimal"),
    ];
    let data = QueryResult::new(
        columns.clone(),
        vec![
            vec![json!(1), json!("a"), json!("100")],
            vec![json!(2), json!("b"), json!("200")],
        ],
    );
    driver
        .upload_table("redshift_test.uploaded", &columns, &data)
        .await
        .unwrap();

    let types = driver
        .table_column_types("redshift_test.uploaded")
        .await
        .unwrap();
    assert_eq!(
        types,
        vec![
            Column::new("id", "bigint"),
            Column::new("name", "text"),
            Column::new("price", "decimal"),
        ]
    );

    // `information_schema` based introspection works on both servers.
    let columns_query = driver.information_schema_query();
    assert!(columns_query.contains("'pg_internal'"));
    let all = query(&driver, &columns_query, &[]).await;
    assert!(!all.is_empty());

    // No primary/foreign keys are queried on Redshift.
    let tables = vec![cubedriver::SchemaTable {
        schema_name: "redshift_test".to_string(),
        table_name: "uploaded".to_string(),
    }];
    let column_infos = driver
        .get_columns_for_specific_tables(&tables)
        .await
        .unwrap();
    assert_eq!(column_infos.len(), 3);
    assert!(column_infos.iter().all(|c| c.attributes.is_empty()));
    assert!(column_infos.iter().all(|c| c.foreign_keys.is_empty()));

    let stream = driver
        .stream(
            "SELECT id, name FROM redshift_test.uploaded ORDER BY id",
            &[],
            &StreamOptions::default(),
        )
        .await
        .unwrap();
    let rows: Vec<_> = stream.rows.try_collect().await.unwrap();
    assert_eq!(
        rows,
        vec![vec![json!("1"), json!("a")], vec![json!("2"), json!("b")]]
    );

    driver
        .drop_table("redshift_test.uploaded", &QueryOptions::default())
        .await
        .unwrap();
    driver.release().await.unwrap();
}

#[tokio::test]
async fn long_table_names_are_allowed_up_to_127_characters() {
    let url = require_redshift!();
    let driver = driver(&url);
    driver
        .create_schema_if_not_exists("redshift_test")
        .await
        .unwrap();

    let name = format!("redshift_test.{}", "t".repeat(100));
    exec(&driver, &format!("DROP TABLE {name}")).await;
    // PostgreSQL's own driver rejects anything over 63 characters; Redshift's
    // limit is 127, so this has to reach the server.
    driver
        .create_table(&name, &[Column::new("id", "bigint")])
        .await
        .unwrap();
    exec(&driver, &format!("DROP TABLE {name}")).await;

    let too_long = "t".repeat(128);
    let err = driver
        .create_table(&too_long, &[Column::new("id", "bigint")])
        .await
        .unwrap_err();
    assert!(err
        .to_string()
        .starts_with("Redshift can not work with table names longer than 127 symbols."));
    driver.release().await.unwrap();
}

#[tokio::test]
async fn show_schemas_lists_external_schemas() {
    let url = require_redshift!();
    if !is_native_redshift() {
        eprintln!("CUBEJS_TEST_REDSHIFT_NATIVE is not 'true', skipping the SHOW SCHEMAS test");
        return;
    }
    let driver = driver(&url);
    let schemas: Vec<SchemaName> = driver.get_schemas().await.unwrap();
    assert!(!schemas.is_empty());
    // System schemas are filtered out by IGNORED_SCHEMAS.
    assert!(!schemas.iter().any(|s| s.schema_name == "pg_catalog"));
    assert!(!schemas.iter().any(|s| s.schema_name == "pg_internal"));

    // `tablesSchema` merges information_schema with the external schemas,
    // which also goes through `SHOW SCHEMAS FROM DATABASE`.
    let structure = driver.tables_schema().await.unwrap();
    assert!(!structure.is_empty());
    driver.release().await.unwrap();
}
