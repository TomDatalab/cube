#![cfg(feature = "sqlite")]
//! SQLite driver tests against real in-memory / temp-file databases (no
//! Docker needed). Ports `packages/cubejs-sqlite-driver/test/SqliteDriver.test.js`
//! plus the driver's other overrides.

use cubedriver::sqlite::{SqliteConfig, SqliteDriver, DATABASE_CLOSED, DEFAULT_CONCURRENCY};
use cubedriver::{Column, Driver, DriverError, QueryOptions, QueryResult, SchemaColumn};
use serde_json::{json, Value};

fn memory_driver() -> SqliteDriver {
    SqliteDriver::new(SqliteConfig::with_database(":memory:")).unwrap()
}

async fn q(driver: &SqliteDriver, sql: &str, params: &[Value]) -> QueryResult {
    driver
        .query(sql, params, &QueryOptions::default())
        .await
        .unwrap()
}

fn col(name: &str, type_: &str) -> SchemaColumn {
    SchemaColumn {
        name: name.to_string(),
        type_: type_.to_string(),
        attributes: vec![],
        foreign_keys: vec![],
    }
}

#[tokio::test]
async fn test_connection() {
    memory_driver().test_connection().await.unwrap();
}

#[tokio::test]
async fn table_schema() {
    let driver = memory_driver();
    q(
        &driver,
        "
       CREATE TABLE users (
         id INTEGER PRIMARY KEY AUTOINCREMENT,
         name TEXT NOT NULL,
         email TEXT UNIQUE NOT NULL,
         age INTEGER,
         created_at DATETIME DEFAULT CURRENT_TIMESTAMP
       );
    ",
        &[],
    )
    .await;
    q(
        &driver,
        "
       CREATE TABLE groups (
         id INTEGER PRIMARY KEY AUTOINCREMENT,
         name TEXT NOT NULL
       );
    ",
        &[],
    )
    .await;

    let schema = driver.tables_schema().await.unwrap();
    assert_eq!(schema.len(), 1);
    let main = &schema["main"];
    assert_eq!(main.len(), 2);
    assert_eq!(
        main["users"],
        vec![
            col("id", "INTEGER"),
            col("name", "TEXT"),
            col("email", "TEXT"),
            col("age", "INTEGER"),
            col("created_at", "DATETIME"),
        ]
    );
    assert_eq!(
        main["groups"],
        vec![col("id", "INTEGER"), col("name", "TEXT")]
    );
    // sqlite_sequence (created by AUTOINCREMENT) is hidden
    assert!(!main.contains_key("sqlite_sequence"));
}

#[tokio::test]
async fn query_values_and_params() {
    let driver = memory_driver();
    q(
        &driver,
        "CREATE TABLE t (i INTEGER, r REAL, s TEXT, b BLOB, n TEXT)",
        &[],
    )
    .await;
    q(
        &driver,
        "INSERT INTO t VALUES (?, ?, ?, ?, ?)",
        &[
            json!(42),
            json!(1.5),
            json!("x'y"),
            Value::Null,
            Value::Null,
        ],
    )
    .await;
    q(
        &driver,
        "INSERT INTO t (i, b) VALUES (?, X'0102')",
        &[json!(true)],
    )
    .await;

    let result = q(&driver, "SELECT * FROM t ORDER BY i", &[]).await;
    assert_eq!(
        result.columns,
        vec![
            Column::new("i", "int"),
            Column::new("r", "REAL"),
            Column::new("s", "text"),
            Column::new("b", "BLOB"),
            Column::new("n", "text"),
        ]
    );
    assert_eq!(
        result.rows,
        vec![
            vec![
                json!(1),
                Value::Null,
                Value::Null,
                json!({"type": "Buffer", "data": [1, 2]}),
                Value::Null
            ],
            vec![
                json!(42),
                json!(1.5),
                json!("x'y"),
                Value::Null,
                Value::Null
            ],
        ]
    );

    let filtered = q(
        &driver,
        "SELECT s FROM t WHERE i = ? AND r > ?",
        &[json!(42), json!(1)],
    )
    .await;
    assert_eq!(filtered.rows, vec![vec![json!("x'y")]]);
}

#[tokio::test]
async fn sql_errors_are_database_errors() {
    let driver = memory_driver();
    let err = driver
        .query("SELECT * FROM missing", &[], &QueryOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(err, DriverError::Database { .. }), "{err:?}");
    assert!(err.to_string().contains("no such table: missing"));
}

#[tokio::test]
async fn file_database_persists_between_drivers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cube.db");
    let path = path.to_str().unwrap();

    let driver = SqliteDriver::new(SqliteConfig::with_database(path)).unwrap();
    q(&driver, "CREATE TABLE a (x INTEGER)", &[]).await;
    q(&driver, "INSERT INTO a VALUES (7)", &[]).await;
    driver.release().await.unwrap();

    let driver = SqliteDriver::new(SqliteConfig::with_database(path)).unwrap();
    assert_eq!(
        q(&driver, "SELECT x FROM a", &[]).await.rows,
        vec![vec![json!(7)]]
    );
}

#[tokio::test]
async fn attached_schemas() {
    let dir = tempfile::tempdir().unwrap();
    let other = dir.path().join("other.db");
    let driver = memory_driver();

    // Not attached yet: no tables.
    assert!(driver.get_tables_query("other").await.unwrap().is_empty());

    q(
        &driver,
        &format!("ATTACH DATABASE '{}' AS other", other.display()),
        &[],
    )
    .await;
    q(&driver, "CREATE TABLE other.b (x INTEGER)", &[]).await;
    q(&driver, "CREATE TABLE other.a (x INTEGER)", &[]).await;

    // Already attached: no second ATTACH (which would fail).
    driver.create_schema_if_not_exists("other").await.unwrap();
    assert_eq!(
        driver.get_tables_query("other").await.unwrap(),
        vec!["a".to_string(), "b".to_string()]
    );
    // `main` is always in the database list.
    driver.create_schema_if_not_exists("main").await.unwrap();
}

#[tokio::test]
async fn upload_and_download() {
    let driver = memory_driver();
    let columns = vec![Column::new("id", "bigint"), Column::new("name", "text")];
    let data = QueryResult::new(
        columns.clone(),
        vec![vec![json!(1), json!("a")], vec![json!(2), json!("b")]],
    );
    driver.upload_table("up", &columns, &data).await.unwrap();

    let rows = driver
        .download_table("up", &Default::default())
        .await
        .unwrap();
    assert_eq!(rows.rows, data.rows);

    let wrapped = driver.wrap_query_with_limit("SELECT * FROM up ORDER BY id", 1);
    assert_eq!(
        q(&driver, &wrapped, &[]).await.rows,
        vec![vec![json!(1), json!("a")]]
    );

    driver
        .drop_table("up", &QueryOptions::default())
        .await
        .unwrap();
    assert!(driver.tables_schema().await.unwrap()["main"].is_empty());
}

#[tokio::test]
async fn driver_properties() {
    let driver = memory_driver();
    assert_eq!(DEFAULT_CONCURRENCY, 2);
    assert!(!driver.read_only());
    assert_eq!(driver.param(0), "?");
    assert_eq!(driver.quote_identifier("a"), "\"a\"");
    assert_eq!(driver.capabilities(), Default::default());
    assert!(matches!(
        driver.stream("SELECT 1", &[], &Default::default()).await,
        Err(DriverError::NotImplemented(_))
    ));
}

#[tokio::test]
async fn release_closes_the_database() {
    let driver = memory_driver();
    driver.release().await.unwrap();
    driver.release().await.unwrap();
    let err = driver.test_connection().await.unwrap_err();
    assert!(err.to_string().contains(DATABASE_CLOSED), "{err}");
}
