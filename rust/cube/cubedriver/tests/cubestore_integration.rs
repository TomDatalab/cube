//! Integration tests for [`CubeStoreDriver`], ported from
//! `packages/cubejs-cubestore-driver`.
//!
//! They run only when `CUBEJS_TEST_CUBESTORE_URL` is set, e.g.
//! `CUBEJS_TEST_CUBESTORE_URL=ws://127.0.0.1:3030`.

use cubedriver::cubestore::{CreateTableOptions, CubeStoreCapability};
use cubedriver::{
    Column, CreateTableIndex, CubeStoreConfig, CubeStoreDriver, DownloadedData, Driver,
    ExternalCreateTableOptions, GenericType, QueryOptions, QueryResult, TableCsvData,
};
use serde_json::{json, Value};

fn cubestore_url() -> Option<String> {
    std::env::var("CUBEJS_TEST_CUBESTORE_URL")
        .ok()
        .filter(|s| !s.is_empty())
}

macro_rules! require_cubestore {
    () => {
        match cubestore_url() {
            Some(url) => url,
            None => {
                eprintln!("CUBEJS_TEST_CUBESTORE_URL is not set, skipping");
                return;
            }
        }
    };
}

fn driver(url: &str) -> CubeStoreDriver {
    CubeStoreDriver::new(CubeStoreConfig::from_url(url)).unwrap()
}

/// A driver that always inlines the parameters client-side.
fn inlining_driver(url: &str) -> CubeStoreDriver {
    let mut config = CubeStoreConfig::from_url(url);
    config.sendable_parameters = false;
    CubeStoreDriver::new(config).unwrap()
}

async fn exec(driver: &CubeStoreDriver, sql: &str) {
    driver
        .query(sql, &[], &QueryOptions::default())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

async fn query(driver: &CubeStoreDriver, sql: &str, params: &[Value]) -> QueryResult {
    driver
        .query(sql, params, &QueryOptions::default())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
}

#[tokio::test]
async fn test_connection_and_version() {
    let url = require_cubestore!();
    let driver = driver(&url);
    driver.test_connection().await.unwrap();

    let version = driver.cube_store_version().await.unwrap();
    assert!(!version.is_empty());
    eprintln!("Cube Store version: {version}");

    // the capability table is consistent with the reported version
    let sendable = driver
        .has_capability(CubeStoreCapability::SendableParameters)
        .await
        .unwrap();
    let arrow = driver
        .has_capability(CubeStoreCapability::ArrowFormat)
        .await
        .unwrap();
    eprintln!("sendableParameters: {sendable}, arrowFormat: {arrow}");

    driver.release().await.unwrap();
}

#[tokio::test]
async fn scalar_types_round_trip() {
    let url = require_cubestore!();
    let driver = driver(&url);

    let data = query(
        &driver,
        "SELECT
           CAST(1 AS int) AS i,
           CAST(-9007199254740993 AS bigint) AS b,
           CAST(1.5 AS double) AS d,
           CAST('foo' AS varchar(10)) AS s,
           CAST(TRUE AS boolean) AS t,
           CAST('2020-01-01T12:34:56.789' AS timestamp) AS ts",
        &[],
    )
    .await;

    assert_eq!(data.len(), 1);
    assert_eq!(data.get(0, "i"), Some(&json!(1)));
    assert_eq!(data.get(0, "b"), Some(&json!(-9007199254740993i64)));
    assert_eq!(data.get(0, "d"), Some(&json!(1.5)));
    assert_eq!(data.get_string(0, "s").as_deref(), Some("foo"));
    assert_eq!(data.get(0, "t"), Some(&json!(true)));
    assert_eq!(
        data.get_string(0, "ts").as_deref(),
        Some("2020-01-01T12:34:56.789")
    );

    driver.release().await.unwrap();
}

/// The default path inlines the values client-side (`formatSql`), which is
/// what every caller but the cache and queue drivers gets.
#[tokio::test]
async fn parameters_are_inlined_by_default() {
    let url = require_cubestore!();
    let driver = driver(&url);

    // `send_parameters` is opt-in, so an ordinary query never sends them.
    assert!(!driver
        .send_parameters(&QueryOptions::default())
        .await
        .unwrap());

    exec(&driver, "CREATE SCHEMA IF NOT EXISTS cubedriver_test").await;
    let rows = query(
        &driver,
        "SELECT table_schema FROM information_schema.tables WHERE table_schema = ?",
        &[json!("cubedriver_test")],
    )
    .await;
    assert!(rows.rows.iter().all(|r| r[0] == json!("cubedriver_test")));

    // an injection attempt stays a literal
    let rows = query(
        &driver,
        "SELECT table_schema FROM information_schema.tables WHERE table_schema = ?",
        &[json!("x' OR 1=1 -- ")],
    )
    .await;
    assert!(rows.is_empty());

    driver.release().await.unwrap();
}

/// Cube Store's own commands take bound parameters, and that is the only path
/// that sets `sendParameters` (the cache and queue drivers).
#[tokio::test]
async fn bound_parameters_are_sent_when_requested() {
    let url = require_cubestore!();
    let driver = driver(&url);
    let options = QueryOptions::default().with_send_parameters(true);

    assert!(
        driver.send_parameters(&options).await.unwrap(),
        "the test server should advertise sendableParameters"
    );

    let key = "cubedriver:test:key";
    driver
        .query(
            "CACHE SET TTL ? ? ?",
            &[json!(60), json!(key), json!("it's a value")],
            &options,
        )
        .await
        .unwrap();

    let rows = driver
        .query("CACHE GET ?", &[json!(key)], &options)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows.get_string(0, "value").as_deref(), Some("it's a value"));

    driver
        .query("CACHE REMOVE ?", &[json!(key)], &options)
        .await
        .unwrap();
    let rows = driver
        .query("CACHE GET ?", &[json!(key)], &options)
        .await
        .unwrap();
    // Cube Store answers a missing key with a single NULL-valued row.
    assert_eq!(rows.len(), 1);
    assert_eq!(rows.get(0, "value"), Some(&Value::Null));

    driver.release().await.unwrap();
}

/// With `CUBEJS_CUBESTORE_SENDABLE_PARAMETERS=false` the same call inlines.
#[tokio::test]
async fn sendable_parameters_can_be_disabled() {
    let url = require_cubestore!();
    let driver = inlining_driver(&url);
    let options = QueryOptions::default().with_send_parameters(true);
    assert!(!driver.send_parameters(&options).await.unwrap());

    let key = "cubedriver:test:inlined";
    driver
        .query(
            "CACHE SET TTL ? ? ?",
            &[json!(60), json!(key), json!("it's a value")],
            &options,
        )
        .await
        .unwrap();
    let rows = driver
        .query("CACHE GET ?", &[json!(key)], &options)
        .await
        .unwrap();
    assert_eq!(rows.get_string(0, "value").as_deref(), Some("it's a value"));
    driver
        .query("CACHE REMOVE ?", &[json!(key)], &options)
        .await
        .unwrap();

    driver.release().await.unwrap();
}

#[tokio::test]
async fn create_upload_and_introspect_a_table() {
    let url = require_cubestore!();
    let driver = driver(&url);

    exec(&driver, "CREATE SCHEMA IF NOT EXISTS cubedriver_test").await;
    let _ = driver
        .drop_table("cubedriver_test.uploaded", &QueryOptions::default())
        .await;

    let columns = vec![
        Column::new("id", "bigint"),
        Column::new("name", "string"),
        Column::new("created", "timestamp"),
        Column::new("flag", "boolean"),
    ];
    // the rows are deliberately in a different column order
    let data = QueryResult::new(
        vec![
            Column::new("name", "string"),
            Column::new("id", "bigint"),
            Column::new("created", "timestamp"),
            Column::new("flag", "boolean"),
        ],
        vec![
            vec![
                json!("a"),
                json!(1),
                json!("2020-01-01T00:00:00.000Z"),
                json!("true"),
            ],
            vec![
                json!("b"),
                json!(2),
                json!("2020-01-02T00:00:00.000Z"),
                json!("false"),
            ],
        ],
    );

    driver
        .upload_table_with_indexes(
            "cubedriver_test.uploaded",
            &columns,
            &data,
            &[],
            &[],
            &ExternalCreateTableOptions {
                create_table_indexes: vec![CreateTableIndex {
                    index_name: "by_name".into(),
                    type_: "regular".into(),
                    columns: vec!["name".into()],
                }],
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let rows = query(
        &driver,
        "SELECT id, name, created, flag FROM cubedriver_test.uploaded ORDER BY id",
        &[],
    )
    .await;
    assert_eq!(rows.len(), 2);
    // values are matched by column name, not position
    assert_eq!(rows.get(0, "id"), Some(&json!(1)));
    assert_eq!(rows.get_string(0, "name").as_deref(), Some("a"));
    // `toColumnValue` stripped the trailing Z and coerced the boolean string
    assert_eq!(
        rows.get_string(0, "created").as_deref(),
        Some("2020-01-01T00:00:00.000")
    );
    assert_eq!(rows.get(0, "flag"), Some(&json!(true)));
    assert_eq!(rows.get(1, "flag"), Some(&json!(false)));

    // `tableColumnTypes` uses the CubeStore-specific information_schema query
    let types = driver
        .table_column_types("cubedriver_test.uploaded")
        .await
        .unwrap();
    // Cube Store reports its 64-bit integer as `int`, which the base mapping
    // turns into the generic `int`.
    assert_eq!(
        types,
        vec![
            Column::new("id", GenericType::Int),
            Column::new("name", GenericType::Text),
            Column::new("created", GenericType::Timestamp),
            Column::new("flag", GenericType::Boolean),
        ]
    );

    // `getTablesQuery` and its build_range_end variant
    let tables = driver.get_tables_query("cubedriver_test").await.unwrap();
    assert!(tables.iter().any(|t| t == "uploaded"), "{tables:?}");

    let with_range = driver
        .get_tables_with_build_range("cubedriver_test")
        .await
        .unwrap();
    assert!(with_range.column_index("build_range_end").is_some());

    let prefixed = driver
        .get_prefix_tables_query("cubedriver_test", &["upload".to_string()])
        .await
        .unwrap();
    assert_eq!(prefixed.len(), 1);

    // `informationSchemaQuery`
    let structure = driver.tables_schema().await.unwrap();
    assert!(structure["cubedriver_test"].contains_key("uploaded"));

    driver
        .drop_table("cubedriver_test.uploaded", &QueryOptions::default())
        .await
        .unwrap();
    let tables = driver.get_tables_query("cubedriver_test").await.unwrap();
    assert!(!tables.iter().any(|t| t == "uploaded"));

    driver.release().await.unwrap();
}

#[tokio::test]
async fn create_table_with_options_is_accepted() {
    let url = require_cubestore!();
    let driver = driver(&url);

    exec(&driver, "CREATE SCHEMA IF NOT EXISTS cubedriver_test").await;
    let _ = driver
        .drop_table("cubedriver_test.with_options", &QueryOptions::default())
        .await;

    let columns = vec![Column::new("id", "bigint"), Column::new("name", "string")];
    let options = CreateTableOptions {
        build_range_end: Some("2020-01-01T00:00:00.000".into()),
        indexes: Some("INDEX by_name (name)".into()),
        ..Default::default()
    };
    // the SQL the driver will send
    let sql =
        driver.create_table_sql_with_options("cubedriver_test.with_options", &columns, &options);
    assert_eq!(
        sql,
        "CREATE TABLE cubedriver_test.with_options (`id` bigint, `name` varchar(255)) \
         WITH (build_range_end = '2020-01-01T00:00:00.000') INDEX by_name (name)"
    );

    driver
        .create_table_with_options(
            "cubedriver_test.with_options",
            &columns,
            &options,
            &QueryOptions::default(),
        )
        .await
        .unwrap();

    let tables = driver.get_tables_query("cubedriver_test").await.unwrap();
    assert!(tables.iter().any(|t| t == "with_options"), "{tables:?}");

    driver
        .drop_table("cubedriver_test.with_options", &QueryOptions::default())
        .await
        .unwrap();
    driver.release().await.unwrap();
}

#[tokio::test]
async fn reports_query_errors() {
    let url = require_cubestore!();
    let driver = driver(&url);
    let err = driver
        .query(
            "SELECT * FROM definitely_not_a_table",
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("definitely_not_a_table"), "{err}");
    driver.release().await.unwrap();
}

/// The pre-aggregation build hands whatever the source driver downloaded to
/// `upload_downloaded_table_with_indexes`; the in-memory branch must behave
/// exactly like `upload_table_with_indexes`.
#[tokio::test]
async fn uploads_a_downloaded_memory_table() {
    let url = require_cubestore!();
    let driver = driver(&url);
    exec(&driver, "CREATE SCHEMA IF NOT EXISTS cubedriver_test").await;
    let _ = driver
        .drop_table("cubedriver_test.downloaded", &QueryOptions::default())
        .await;

    let columns = vec![
        Column::new("id", "bigint"),
        Column::new("name", "string"),
        Column::new("amount", "decimal"),
    ];
    let downloaded = DownloadedData::Memory(QueryResult::new(
        columns.clone(),
        vec![
            vec![json!(1), json!("a"), json!("100")],
            vec![json!(2), json!("b"), json!("200")],
        ],
    ));

    driver
        .upload_downloaded_table_with_indexes(
            "cubedriver_test.downloaded",
            &columns,
            downloaded,
            &[],
            &[],
            &ExternalCreateTableOptions {
                create_table_indexes: vec![CreateTableIndex {
                    index_name: "by_name".to_string(),
                    type_: "regular".to_string(),
                    columns: vec!["name".to_string()],
                }],
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let rows = query(
        &driver,
        "SELECT id, name, amount FROM cubedriver_test.downloaded ORDER BY id",
        &[],
    )
    .await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows.get_string(0, "name").as_deref(), Some("a"));
    assert_eq!(rows.get_string(1, "amount").as_deref(), Some("200"));

    driver
        .drop_table("cubedriver_test.downloaded", &QueryOptions::default())
        .await
        .unwrap();
}

/// The CSV branch: Cube Store imports the unloaded files itself, so the driver
/// must issue `CREATE TABLE … LOCATION` instead of inserting rows.
///
/// Needs a CSV file the *Cube Store server* can download, which is why it is
/// behind its own variable:
/// `CUBEJS_TEST_CUBESTORE_CSV_URL=http://host-reachable-by-cubestore/data.csv`
/// (contents: `id,name,amount` + `1,a,100` + `2,b,200`).
#[tokio::test]
async fn imports_a_downloaded_csv_file() {
    let url = require_cubestore!();
    let Ok(csv_url) = std::env::var("CUBEJS_TEST_CUBESTORE_CSV_URL") else {
        eprintln!("CUBEJS_TEST_CUBESTORE_CSV_URL is not set, skipping the CSV import test");
        return;
    };

    let driver = driver(&url);
    exec(&driver, "CREATE SCHEMA IF NOT EXISTS cubedriver_test").await;
    let _ = driver
        .drop_table("cubedriver_test.imported", &QueryOptions::default())
        .await;

    let columns = vec![
        Column::new("id", "bigint"),
        Column::new("name", "string"),
        Column::new("amount", "bigint"),
    ];
    let downloaded = DownloadedData::Csv(TableCsvData {
        csv_file: vec![csv_url],
        types: Some(columns.clone()),
        ..Default::default()
    });

    driver
        .upload_downloaded_table("cubedriver_test.imported", &columns, downloaded)
        .await
        .unwrap();

    let rows = query(
        &driver,
        "SELECT id, name, amount FROM cubedriver_test.imported ORDER BY id",
        &[],
    )
    .await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows.get_string(0, "name").as_deref(), Some("a"));
    assert_eq!(rows.get_i64(1, "amount"), Some(200));

    driver
        .drop_table("cubedriver_test.imported", &QueryOptions::default())
        .await
        .unwrap();
}
