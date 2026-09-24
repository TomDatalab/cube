//! Integration tests for [`MsSqlDriver`], ported from
//! `packages/cubejs-mssql-driver/test`.
//!
//! They run only when `CUBEJS_TEST_MSSQL_URL` is set, e.g.
//!
//! ```text
//! docker run -d --name mssql -e ACCEPT_EULA=Y -e MSSQL_SA_PASSWORD='Cube_test_123!' \
//!   -p 21433:1433 mcr.microsoft.com/mssql/server:2022-latest
//! CUBEJS_TEST_MSSQL_URL='mssql://sa:Cube_test_123%21@127.0.0.1:21433/master'
//! ```

use cubedriver::{
    Column, Driver, MsSqlConfig, MsSqlDriver, QueryOptions, QueryResult, StreamOptions,
};
use futures::TryStreamExt;
use serde_json::{json, Value};

fn mssql_url() -> Option<String> {
    std::env::var("CUBEJS_TEST_MSSQL_URL")
        .ok()
        .filter(|s| !s.is_empty())
}

macro_rules! require_mssql {
    () => {
        match mssql_url() {
            Some(url) => url,
            None => {
                eprintln!("CUBEJS_TEST_MSSQL_URL is not set, skipping");
                return;
            }
        }
    };
}

fn driver(url: &str) -> MsSqlDriver {
    let mut config = MsSqlConfig::from_url(url).unwrap();
    config.read_only = false;
    MsSqlDriver::new(config).unwrap()
}

async fn query(driver: &MsSqlDriver, sql: &str, params: &[Value]) -> QueryResult {
    driver
        .query(sql, params, &QueryOptions::default())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
}

async fn exec(driver: &MsSqlDriver, sql: &str) {
    let _ = driver.query(sql, &[], &QueryOptions::default()).await;
}

#[tokio::test]
async fn test_connection_and_scalar_types() {
    let url = require_mssql!();
    let driver = driver(&url);
    driver.test_connection().await.unwrap();

    let data = query(
        &driver,
        "SELECT
           CAST(1 AS bit) AS flag,
           CAST(2 AS tinyint) AS tiny,
           CAST(3 AS smallint) AS small,
           CAST(4 AS int) AS i,
           CAST(9007199254740993 AS bigint) AS big,
           CAST(1.25 AS decimal(10, 2)) AS dec,
           CAST(1.5 AS float) AS dbl,
           CAST('foo' AS nvarchar(10)) AS s,
           CAST('2020-01-01' AS date) AS d,
           CAST('2020-01-01T12:34:56.789' AS datetime2) AS ts,
           CAST('12:34:56' AS time) AS t,
           CAST('6F9619FF-8B86-D011-B42D-00C04FC964FF' AS uniqueidentifier) AS uid,
           CAST(NULL AS int) AS nothing",
        &[],
    )
    .await;

    assert_eq!(data.len(), 1);
    assert_eq!(data.get(0, "flag"), Some(&json!(true)));
    // every numeric type is stringified, like `sql.valueHandler` does
    assert_eq!(data.get(0, "tiny"), Some(&json!("2")));
    assert_eq!(data.get(0, "small"), Some(&json!("3")));
    assert_eq!(data.get(0, "i"), Some(&json!("4")));
    assert_eq!(data.get(0, "big"), Some(&json!("9007199254740993")));
    assert_eq!(data.get(0, "dec"), Some(&json!("1.25")));
    assert_eq!(data.get(0, "dbl"), Some(&json!("1.5")));
    assert_eq!(data.get(0, "s"), Some(&json!("foo")));
    assert_eq!(data.get(0, "d"), Some(&json!("2020-01-01T00:00:00.000Z")));
    assert_eq!(data.get(0, "ts"), Some(&json!("2020-01-01T12:34:56.789Z")));
    assert_eq!(data.get(0, "t"), Some(&json!("1970-01-01T12:34:56.000Z")));
    assert_eq!(
        data.get(0, "uid")
            .and_then(|v| v.as_str())
            .map(|s| s.to_lowercase()),
        Some("6f9619ff-8b86-d011-b42d-00c04fc964ff".to_string())
    );
    assert_eq!(data.get(0, "nothing"), Some(&Value::Null));

    assert_eq!(
        data.columns,
        vec![
            Column::new("flag", "boolean"),
            Column::new("tiny", "int"),
            Column::new("small", "int"),
            Column::new("i", "int"),
            Column::new("big", "int"),
            Column::new("dec", "decimal"),
            Column::new("dbl", "double"),
            Column::new("s", "text"),
            Column::new("d", "timestamp"),
            Column::new("ts", "timestamp"),
            Column::new("t", "string"),
            // `mapFields` maps uniqueidentifier to the `string` name, which
            // `toGenericType` then resolves to `text`.
            Column::new("uid", "text"),
            Column::new("nothing", "int"),
        ]
    );
    driver.release().await.unwrap();
}

/// Nullable columns arrive as the variable-width TDS types (`intn`, `floatn`,
/// `datetimen`), whose payload width is not the declared one.
#[tokio::test]
async fn nullable_columns_of_every_width() {
    let url = require_mssql!();
    let driver = driver(&url);
    driver
        .create_schema_if_not_exists("cube_nullable")
        .await
        .unwrap();
    exec(&driver, "DROP TABLE cube_nullable.widths").await;
    query(
        &driver,
        "CREATE TABLE cube_nullable.widths (
            t tinyint NULL, s smallint NULL, i int NULL, b bigint NULL,
            f4 real NULL, f8 float NULL, d date NULL, ts datetime NULL, flag bit NULL)",
        &[],
    )
    .await;
    query(
        &driver,
        "INSERT INTO cube_nullable.widths VALUES
            (1, 2, 3, 4, 1.5, 2.5, '2020-01-01', '2020-01-01T12:00:00', 1),
            (NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL)",
        &[],
    )
    .await;

    let data = query(&driver, "SELECT * FROM cube_nullable.widths", &[]).await;
    assert_eq!(data.len(), 2);
    assert_eq!(data.get(0, "t"), Some(&json!("1")));
    assert_eq!(data.get(0, "s"), Some(&json!("2")));
    assert_eq!(data.get(0, "i"), Some(&json!("3")));
    assert_eq!(data.get(0, "b"), Some(&json!("4")));
    assert_eq!(data.get(0, "f4"), Some(&json!("1.5")));
    assert_eq!(data.get(0, "f8"), Some(&json!("2.5")));
    assert_eq!(data.get(0, "d"), Some(&json!("2020-01-01T00:00:00.000Z")));
    assert_eq!(data.get(0, "ts"), Some(&json!("2020-01-01T12:00:00.000Z")));
    assert_eq!(data.get(0, "flag"), Some(&json!(true)));
    for column in ["t", "s", "i", "b", "f4", "f8", "d", "ts", "flag"] {
        assert_eq!(data.get(1, column), Some(&Value::Null), "{column}");
    }

    exec(&driver, "DROP TABLE cube_nullable.widths").await;
    driver.release().await.unwrap();
}

#[tokio::test]
async fn parameters_use_the_cube_placeholders() {
    let url = require_mssql!();
    let driver = driver(&url);

    let data = query(
        &driver,
        "SELECT @_1 AS s, @_2 + 1 AS n, @_3 AS b, @_4 AS nothing, '@_9' AS literal",
        &[json!("x"), json!(41), json!(true), Value::Null],
    )
    .await;
    assert_eq!(data.get(0, "s"), Some(&json!("x")));
    assert_eq!(data.get(0, "n"), Some(&json!("42")));
    assert_eq!(data.get(0, "b"), Some(&json!(true)));
    assert_eq!(data.get(0, "nothing"), Some(&Value::Null));
    assert_eq!(data.get(0, "literal"), Some(&json!("@_9")));
    driver.release().await.unwrap();
}

#[tokio::test]
async fn schema_introspection() {
    let url = require_mssql!();
    let driver = driver(&url);
    driver
        .create_schema_if_not_exists("cube_introspect")
        .await
        .unwrap();
    // idempotent
    driver
        .create_schema_if_not_exists("cube_introspect")
        .await
        .unwrap();

    exec(&driver, "DROP TABLE cube_introspect.introspect").await;
    query(
        &driver,
        "CREATE TABLE cube_introspect.introspect (id bigint, name nvarchar(max), amount decimal(10, 2), created datetime2)",
        &[],
    )
    .await;

    let tables = driver.get_tables_query("cube_introspect").await.unwrap();
    assert!(tables.contains(&"introspect".to_string()));

    let types = driver
        .table_column_types("cube_introspect.introspect")
        .await
        .unwrap();
    assert_eq!(
        types,
        vec![
            Column::new("id", "bigint"),
            Column::new("name", "text"),
            Column::new("amount", "decimal"),
            Column::new("created", "timestamp"),
        ]
    );

    let structure = driver.tables_schema().await.unwrap();
    let columns = &structure["cube_introspect"]["introspect"];
    assert_eq!(columns.len(), 4);
    assert_eq!(columns[0].name, "amount");
    assert_eq!(columns[0].type_, "decimal");

    let schemas = driver.get_schemas().await.unwrap();
    assert!(schemas.iter().any(|s| s.schema_name == "cube_introspect"));

    let schema_tables = driver
        .get_tables_for_specific_schemas(&[cubedriver::SchemaName {
            schema_name: "cube_introspect".to_string(),
        }])
        .await
        .unwrap();
    assert!(schema_tables
        .iter()
        .any(|t| t.table_name == "introspect" && t.schema_name == "cube_introspect"));

    let column_infos = driver
        .get_columns_for_specific_tables(&schema_tables)
        .await
        .unwrap();
    assert!(column_infos
        .iter()
        .any(|c| c.column_name == "id" && c.data_type == "bigint"));

    exec(&driver, "DROP TABLE cube_introspect.introspect").await;
    driver.release().await.unwrap();
}

#[tokio::test]
async fn upload_query_and_stream() {
    let url = require_mssql!();
    let driver = driver(&url);
    driver
        .create_schema_if_not_exists("cube_upload")
        .await
        .unwrap();
    exec(&driver, "DROP TABLE cube_upload.upload_test").await;

    let columns = vec![
        Column::new("id", "bigint"),
        Column::new("name", "string"),
        Column::new("price", "decimal"),
        Column::new("flag", "boolean"),
    ];
    let data = QueryResult::new(
        columns.clone(),
        vec![
            vec![json!(1), json!("a"), json!("100"), json!(true)],
            vec![json!(2), json!("b"), json!("200"), json!(false)],
        ],
    );
    driver
        .upload_table("cube_upload.upload_test", &columns, &data)
        .await
        .unwrap();

    let rows = query(
        &driver,
        "SELECT * FROM cube_upload.upload_test ORDER BY id",
        &[],
    )
    .await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows.get(0, "name"), Some(&json!("a")));
    assert_eq!(rows.get(1, "flag"), Some(&json!(false)));

    let limited = query(
        &driver,
        &driver.wrap_query_with_limit("SELECT * FROM cube_upload.upload_test", 1),
        &[],
    )
    .await;
    assert_eq!(limited.len(), 1);

    let stream = driver
        .stream(
            "SELECT id, name FROM cube_upload.upload_test ORDER BY id",
            &[],
            &StreamOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        stream.columns,
        vec![Column::new("id", "int"), Column::new("name", "text")]
    );
    let streamed: Vec<_> = stream.rows.try_collect().await.unwrap();
    assert_eq!(
        streamed,
        vec![vec![json!("1"), json!("a")], vec![json!("2"), json!("b")],]
    );

    driver
        .drop_table("cube_upload.upload_test", &QueryOptions::default())
        .await
        .unwrap();
    let gone = driver
        .get_tables_query("cube_upload")
        .await
        .unwrap()
        .contains(&"upload_test".to_string());
    assert!(!gone);
    driver.release().await.unwrap();
}

#[tokio::test]
async fn errors_carry_the_server_message() {
    let url = require_mssql!();
    let driver = driver(&url);
    let err = driver
        .query(
            "SELECT * FROM definitely_missing",
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("Invalid object name"),
        "unexpected error: {err}"
    );
    driver.release().await.unwrap();
}

/// An encrypted connection, which is what `CUBEJS_DB_SSL=true` asks for and
/// what Azure SQL requires.
///
/// The server presents a self-signed certificate, so this exercises the
/// `rejectUnauthorized: false` default. It is the same handshake a managed
/// instance performs.
#[tokio::test]
async fn connects_over_tls() {
    let url = require_mssql!();

    let mut config = MsSqlConfig::from_url(&url).unwrap();
    config.read_only = false;
    config.driver.data_source.ssl = Some(cubedriver::SslConfig::default());

    let driver = MsSqlDriver::new(config).expect("the TLS configuration builds");
    driver
        .test_connection()
        .await
        .expect("the encrypted connection is accepted");

    // The session really works, not just the handshake.
    let rows = driver
        .query(
            "SELECT 1 AS one, ENCRYPT_OPTION AS encryption \
             FROM sys.dm_exec_connections WHERE session_id = @@SPID",
            &[],
            &QueryOptions::default(),
        )
        .await
        .expect("the query runs");

    assert_eq!(rows.len(), 1);
    // Integers come back as strings, which is Cube's convention.
    assert_eq!(rows.get_string(0, "one").as_deref(), Some("1"));
    // SQL Server reports whether the session is encrypted.
    assert_eq!(
        rows.get_string(0, "encryption").as_deref(),
        Some("TRUE"),
        "the connection should be encrypted"
    );
}

/// With `rejectUnauthorized` on and no CA, a self-signed certificate is
/// refused rather than silently accepted.
#[tokio::test]
async fn an_untrusted_certificate_is_refused_when_verification_is_on() {
    let url = require_mssql!();

    let mut config = MsSqlConfig::from_url(&url).unwrap();
    config.read_only = false;
    config.driver.data_source.ssl = Some(cubedriver::SslConfig {
        reject_unauthorized: true,
        ..cubedriver::SslConfig::default()
    });

    let driver = MsSqlDriver::new(config).expect("the configuration builds");
    let result = driver.test_connection().await;

    assert!(
        result.is_err(),
        "a self-signed certificate must not pass verification"
    );
}
