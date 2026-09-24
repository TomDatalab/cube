//! Integration tests for [`MySqlDriver`], ported from
//! `packages/cubejs-mysql-driver/test/MySqlDriver.test.ts`.
//!
//! They run only when `CUBEJS_TEST_MYSQL_URL` is set, e.g.
//! `CUBEJS_TEST_MYSQL_URL=mysql://root:test@127.0.0.1:3306/test`.

use cubedriver::{
    Column, DownloadQueryResultsOptions, DownloadedData, Driver, GenericType, MySqlConfig,
    MySqlDriver, QueryOptions, QueryResult, SchemaTable, StreamOptions,
};
use futures::TryStreamExt;
use serde_json::{json, Value};

fn mysql_url() -> Option<String> {
    std::env::var("CUBEJS_TEST_MYSQL_URL")
        .ok()
        .filter(|s| !s.is_empty())
}

macro_rules! require_mysql {
    () => {
        match mysql_url() {
            Some(url) => url,
            None => {
                eprintln!("CUBEJS_TEST_MYSQL_URL is not set, skipping");
                return;
            }
        }
    };
}

fn driver(url: &str) -> MySqlDriver {
    let mut config = MySqlConfig::from_url(url);
    config.read_only = false;
    config.max_pool_size = Some(4);
    // The database name is needed by `informationSchemaQuery`; `from_url`
    // already picked it up from the URL path.
    MySqlDriver::new(config).unwrap()
}

async fn exec(driver: &MySqlDriver, sql: &str) {
    driver
        .query(sql, &[], &QueryOptions::default())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

async fn query(driver: &MySqlDriver, sql: &str, params: &[Value]) -> QueryResult {
    driver
        .query(sql, params, &QueryOptions::default())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
}

#[tokio::test]
async fn test_connection_and_scalar_types() {
    let url = require_mysql!();
    let driver = driver(&url);
    driver.test_connection().await.unwrap();

    let data = query(
        &driver,
        "SELECT
           CAST('2020-01-01' AS DATE) AS `date`,
           CAST('2020-01-01 00:00:00' AS DATETIME) AS `datetime`,
           CAST(1 AS SIGNED) AS `int`,
           CAST(-9007199254740993 AS SIGNED) AS `bigint`,
           CAST(1.5 AS DOUBLE) AS `double`,
           CAST('1.000000000000000000000000001' AS DECIMAL(38,27)) AS `decimal`,
           'foo' AS `string`,
           NULL AS `null`",
        &[],
    )
    .await;

    assert_eq!(data.len(), 1);
    assert_eq!(data.get_string(0, "date").as_deref(), Some("2020-01-01"));
    assert_eq!(
        data.get_string(0, "datetime").as_deref(),
        Some("2020-01-01 00:00:00")
    );
    assert_eq!(data.get(0, "int"), Some(&json!(1)));
    // exact, unlike JavaScript's doubles
    assert_eq!(data.get(0, "bigint"), Some(&json!(-9007199254740993i64)));
    assert_eq!(data.get(0, "double"), Some(&json!(1.5)));
    assert_eq!(
        data.get_string(0, "decimal").as_deref(),
        Some("1.000000000000000000000000001")
    );
    assert_eq!(data.get_string(0, "string").as_deref(), Some("foo"));
    assert_eq!(data.get(0, "null"), Some(&Value::Null));
}

#[tokio::test]
async fn parameters_are_interpolated_and_escaped() {
    let url = require_mysql!();
    let driver = driver(&url);

    let data = query(
        &driver,
        "SELECT ? AS a, ? AS b, ? AS c, ? AS d",
        &[json!(1), json!("it's"), Value::Null, json!(true)],
    )
    .await;
    assert_eq!(data.get(0, "a"), Some(&json!(1)));
    assert_eq!(data.get_string(0, "b").as_deref(), Some("it's"));
    assert_eq!(data.get(0, "c"), Some(&Value::Null));
    // TRUE is rendered as 1 by MySQL
    assert_eq!(data.get(0, "d"), Some(&json!(1)));

    // an injection attempt stays a literal
    let data = query(&driver, "SELECT ? AS a", &[json!("x' OR 1=1 -- ")]).await;
    assert_eq!(data.get_string(0, "a").as_deref(), Some("x' OR 1=1 -- "));
}

#[tokio::test]
async fn create_upload_and_introspect_a_table() {
    let url = require_mysql!();
    let driver = driver(&url);

    exec(&driver, "DROP TABLE IF EXISTS cubedriver_upload").await;

    let columns = vec![
        Column::new("id", "bigint"),
        Column::new("name", "string"),
        Column::new("created", "timestamp"),
        Column::new("amount", "decimal"),
    ];
    let data = QueryResult::new(
        vec![
            Column::new("name", "string"),
            Column::new("id", "bigint"),
            Column::new("created", "timestamp"),
            Column::new("amount", "decimal"),
        ],
        vec![
            vec![
                json!("a"),
                json!(1),
                json!("2020-01-01T00:00:00.000Z"),
                json!("1.5"),
            ],
            vec![json!("b"), json!(2), Value::Null, Value::Null],
        ],
    );

    driver
        .upload_table("cubedriver_upload", &columns, &data)
        .await
        .unwrap();

    let rows = query(&driver, "SELECT * FROM cubedriver_upload ORDER BY id", &[]).await;
    assert_eq!(rows.len(), 2);
    // values are matched by column name, not position
    assert_eq!(rows.get(0, "id"), Some(&json!(1)));
    assert_eq!(rows.get_string(0, "name").as_deref(), Some("a"));
    // `toColumnValue` stripped the trailing Z
    assert_eq!(
        rows.get_string(0, "created").as_deref(),
        Some("2020-01-01 00:00:00")
    );
    assert_eq!(rows.get(1, "created"), Some(&Value::Null));

    // `MySqlToGenericType`: bigint maps to int, varchar to text
    let types = driver
        .table_column_types(&format!("{}.cubedriver_upload", database(&url)))
        .await
        .unwrap();
    assert_eq!(
        types,
        vec![
            Column::new("id", "int"),
            Column::new("name", "text"),
            Column::new("created", "timestamp"),
            Column::new("amount", "decimal"),
        ]
    );

    // introspection
    let tables = driver.get_tables_query(&database(&url)).await.unwrap();
    assert!(tables.iter().any(|t| t == "cubedriver_upload"));

    let columns = driver
        .get_columns_for_specific_tables(&[SchemaTable {
            schema_name: database(&url),
            table_name: "cubedriver_upload".into(),
        }])
        .await
        .unwrap();
    // `getColumnsForSpecificTablesQuery` has no ORDER BY, so the server's
    // order is whatever it is.
    let mut names: Vec<_> = columns.iter().map(|c| c.column_name.clone()).collect();
    names.sort();
    assert_eq!(names, vec!["amount", "created", "id", "name"]);

    let structure = driver.tables_schema().await.unwrap();
    assert!(structure[&database(&url)].contains_key("cubedriver_upload"));

    driver
        .drop_table("cubedriver_upload", &QueryOptions::default())
        .await
        .unwrap();
    driver.release().await.unwrap();
}

#[tokio::test]
async fn streams_rows() {
    let url = require_mysql!();
    let driver = driver(&url);

    let stream = driver
        .stream(
            "SELECT 1 AS a UNION ALL SELECT 2 UNION ALL SELECT 3",
            &[],
            &StreamOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(stream.columns.len(), 1);
    assert_eq!(stream.columns[0].name, "a");
    let rows: Vec<_> = stream.rows.try_collect().await.unwrap();
    assert_eq!(rows, vec![vec![json!(1)], vec![json!(2)], vec![json!(3)]]);

    // the same result through downloadQueryResults with streamImport
    let data = driver
        .download_query_results(
            "SELECT 1 AS a",
            &[],
            &DownloadQueryResultsOptions {
                stream_import: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let DownloadedData::Stream(s) = data else {
        panic!("expected a stream");
    };
    let rows: Vec<_> = s.rows.try_collect().await.unwrap();
    assert_eq!(rows, vec![vec![json!(1)]]);

    driver.release().await.unwrap();
}

#[tokio::test]
async fn reports_database_errors() {
    let url = require_mysql!();
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
}

#[tokio::test]
async fn result_column_types_come_from_the_wire() {
    let url = require_mysql!();
    let driver = driver(&url);
    let data = query(&driver, "SELECT CAST(1 AS SIGNED) AS n, 'x' AS s", &[]).await;
    assert_eq!(
        data.columns,
        vec![
            // LONGLONG → bigint → MySqlToGenericType → int
            Column::new("n", GenericType::Int),
            Column::new("s", GenericType::Text),
        ]
    );
    driver.release().await.unwrap();
}

/// The database part of a `mysql://user:pass@host:port/db` URL.
fn database(url: &str) -> String {
    url.rsplit('/').next().unwrap_or("test").to_string()
}

/// Foreign keys, both unfiltered and filtered by table.
///
/// `MySqlDriver.ts:216` gets all three wrong: it self-joins
/// `key_column_usage`, so the "target" repeats the referencing table; it
/// compares a table name against schema names; and it gives the referenced
/// side the alias the per-table condition filters on. The Rust query reads the
/// referenced columns MySQL already records on the row.
#[tokio::test]
async fn foreign_keys_name_the_referenced_table() {
    let url = require_mysql!();
    let driver = driver(&url);
    let database = database(&url);

    exec(&driver, "DROP TABLE IF EXISTS cubedriver_orders").await;
    exec(&driver, "DROP TABLE IF EXISTS cubedriver_users").await;
    exec(
        &driver,
        "CREATE TABLE cubedriver_users (id INT PRIMARY KEY, name VARCHAR(20))",
    )
    .await;
    exec(
        &driver,
        "CREATE TABLE cubedriver_orders (
            id INT PRIMARY KEY,
            user_id INT,
            FOREIGN KEY (user_id) REFERENCES cubedriver_users(id)
         )",
    )
    .await;

    // Filtered by the referencing table, which is the incremental
    // schema-loading path.
    let columns = driver
        .get_columns_for_specific_tables(&[SchemaTable {
            schema_name: database.clone(),
            table_name: "cubedriver_orders".into(),
        }])
        .await
        .unwrap();

    let user_id = columns
        .iter()
        .find(|c| c.column_name == "user_id")
        .expect("user_id column");
    assert_eq!(user_id.foreign_keys.len(), 1, "{:?}", user_id.foreign_keys);
    assert_eq!(user_id.foreign_keys[0].target_table, "cubedriver_users");
    assert_eq!(user_id.foreign_keys[0].target_column, "id");

    // The primary key carries no foreign key.
    let id = columns
        .iter()
        .find(|c| c.column_name == "id")
        .expect("id column");
    assert!(id.foreign_keys.is_empty(), "{:?}", id.foreign_keys);

    // The referenced table has none of its own.
    let users_columns = driver
        .get_columns_for_specific_tables(&[SchemaTable {
            schema_name: database,
            table_name: "cubedriver_users".into(),
        }])
        .await
        .unwrap();
    assert!(users_columns.iter().all(|c| c.foreign_keys.is_empty()));

    exec(&driver, "DROP TABLE IF EXISTS cubedriver_orders").await;
    exec(&driver, "DROP TABLE IF EXISTS cubedriver_users").await;
}
