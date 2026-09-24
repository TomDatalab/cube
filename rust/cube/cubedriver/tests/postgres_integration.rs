//! Integration tests for `PostgresDriver`, ported from
//! `packages/cubejs-postgres-driver/test/PostgresDriver.test.ts`.
//!
//! They run only when `CUBEJS_TEST_PG_URL` is set, e.g.
//! `CUBEJS_TEST_PG_URL=postgres://test:test@localhost:5432/test`.

use cubedriver::{
    Column, DownloadQueryResultsOptions, DownloadedData, Driver, DriverError, PostgresConfig,
    PostgresDriver, QueryOptions, QueryResult, SchemaName, SchemaTable, StreamOptions,
};
use futures::TryStreamExt;
use serde_json::{json, Value};

fn pg_url() -> Option<String> {
    std::env::var("CUBEJS_TEST_PG_URL")
        .ok()
        .filter(|s| !s.is_empty())
}

macro_rules! require_pg {
    () => {
        match pg_url() {
            Some(url) => url,
            None => {
                eprintln!("CUBEJS_TEST_PG_URL is not set, skipping");
                return;
            }
        }
    };
}

fn driver(url: &str) -> PostgresDriver {
    let mut config = PostgresConfig::from_url(url);
    config.max_pool_size = Some(4);
    PostgresDriver::new(config).unwrap()
}

async fn exec(driver: &PostgresDriver, sql: &str) {
    driver
        .query(sql, &[], &QueryOptions::default())
        .await
        .unwrap();
}

#[tokio::test]
async fn test_connection_and_type_coercion() {
    let url = require_pg!();
    let driver = driver(&url);
    driver.test_connection().await.unwrap();

    exec(&driver, "DROP TYPE IF EXISTS CUBEJS_TEST_ENUM CASCADE").await;
    exec(&driver, "CREATE TYPE CUBEJS_TEST_ENUM AS ENUM ('FOO')").await;

    let data = driver
        .query(
            "
        SELECT
          CAST('2020-01-01' as DATE) as date,
          CAST('2020-01-01 00:00:00' as TIMESTAMP) as timestamp,
          CAST('2020-01-01 00:00:00+02' as TIMESTAMPTZ) as timestamptz,
          CAST('1.0' as DECIMAL(10,2)) as decimal,
          CAST('FOO' as CUBEJS_TEST_ENUM) as enum,
          CAST(1 as BIGINT) as big,
          CAST(2 as INT) as small,
          CAST(1.5 as FLOAT8) as dbl,
          true as flag,
          '{\"a\": [1, 2]}'::jsonb as js,
          ARRAY[1, 2]::int[] as ints,
          NULL::text as nothing
      ",
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap();

    assert_eq!(
        data.to_json_rows(),
        vec![json!({
            "date": "2020-01-01T00:00:00.000",
            "timestamp": "2020-01-01T00:00:00.000",
            "timestamptz": "2019-12-31T22:00:00.000",
            "decimal": "1.00",
            "enum": "FOO",
            "big": "1",
            "small": 2,
            "dbl": 1.5,
            "flag": true,
            "js": {"a": [1, 2]},
            "ints": [1, 2],
            "nothing": null
        })
        .as_object()
        .unwrap()
        .clone()]
    );
    assert_eq!(
        data.columns,
        vec![
            Column::new("date", "date"),
            Column::new("timestamp", "timestamp"),
            Column::new("timestamptz", "timestamptz"),
            Column::new("decimal", "decimal"),
            Column::new("enum", "text"),
            Column::new("big", "bigint"),
            Column::new("small", "int"),
            Column::new("dbl", "double"),
            Column::new("flag", "boolean"),
            Column::new("js", "jsonb"),
            Column::new("ints", "text"),
            Column::new("nothing", "text"),
        ]
    );
    driver.release().await.unwrap();
}

#[tokio::test]
async fn params_are_sent_as_text() {
    let url = require_pg!();
    let driver = driver(&url);
    let data = driver
        .query(
            "SELECT $1::int + 1 AS n, $2::text AS s, $3::timestamp AS ts, $4::bool AS b, $5::text AS nul",
            &[json!("41"), json!("x"), json!("2020-01-01 10:00:00"), json!(true), Value::Null],
            &QueryOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        data.rows,
        vec![vec![
            json!(42),
            json!("x"),
            json!("2020-01-01T10:00:00.000"),
            json!(true),
            Value::Null
        ]]
    );

    let too_many: Vec<Value> = vec![json!("foo"); 65_536];
    let err = driver
        .query("SELECT 'foo'::TEXT", &too_many, &QueryOptions::default())
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "PostgreSQL protocol does not support more than 65535 parameters, but 65536 passed"
    );
    driver.release().await.unwrap();
}

#[tokio::test]
async fn upload_and_stream() {
    let url = require_pg!();
    let driver = driver(&url);
    driver.create_schema_if_not_exists("test").await.unwrap();
    exec(&driver, "DROP TABLE IF EXISTS test.streaming_test").await;

    let columns = vec![
        Column::new("id", "bigint"),
        Column::new("created", "date"),
        Column::new("price", "decimal"),
    ];
    let data = QueryResult::new(
        columns.clone(),
        vec![
            vec![json!(1), json!("2020-01-01"), json!("100")],
            vec![json!(2), json!("2020-01-02"), json!("200")],
            vec![json!(3), json!("2020-01-03"), json!("300")],
        ],
    );
    driver
        .upload_table("test.streaming_test", &columns, &data)
        .await
        .unwrap();

    let table_data = driver
        .stream(
            "select * from test.streaming_test order by id",
            &[],
            &StreamOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        table_data.columns,
        vec![
            Column::new("id", "bigint"),
            Column::new("created", "date"),
            Column::new("price", "decimal"),
        ]
    );
    let rows: Vec<_> = table_data.rows.try_collect().await.unwrap();
    assert_eq!(
        rows,
        vec![
            vec![json!("1"), json!("2020-01-01T00:00:00.000"), json!("100")],
            vec![json!("2"), json!("2020-01-02T00:00:00.000"), json!("200")],
            vec![json!("3"), json!("2020-01-03T00:00:00.000"), json!("300")],
        ]
    );

    // the pooled connection is returned once the stream is dropped
    let types = driver
        .table_column_types("test.streaming_test")
        .await
        .unwrap();
    assert_eq!(types, columns);

    let downloaded = driver
        .download_query_results(
            "select * from test.streaming_test order by id",
            &[],
            &DownloadQueryResultsOptions {
                stream_import: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(matches!(downloaded, DownloadedData::Stream(_)));
    let memory = downloaded.into_memory().await.unwrap();
    assert_eq!(memory.rows.len(), 3);
    assert_eq!(memory.columns, columns);

    let downloaded = driver
        .download_query_results(
            "select * from test.streaming_test order by id",
            &[],
            &DownloadQueryResultsOptions::default(),
        )
        .await
        .unwrap();
    let DownloadedData::Memory(memory) = downloaded else {
        panic!("expected memory data");
    };
    assert_eq!(memory.rows.len(), 3);

    let table = driver
        .download_table("test.streaming_test", &Default::default())
        .await
        .unwrap();
    assert_eq!(table.rows.len(), 3);

    driver
        .drop_table("test.streaming_test", &QueryOptions::default())
        .await
        .unwrap();
    let tables = driver.get_tables_query("test").await.unwrap();
    assert!(!tables.iter().any(|t| t == "streaming_test"));
    driver.release().await.unwrap();
}

#[tokio::test]
async fn stream_array_typed_columns() {
    let url = require_pg!();
    let driver = driver(&url);
    let table_data = driver
        .stream(
            "SELECT
        ARRAY['oops', 'test']::text[] as text_array,
        ARRAY[1, 2, 3]::int[] as int_array",
            &[],
            &StreamOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        table_data.columns,
        vec![
            Column::new("text_array", "text"),
            Column::new("int_array", "text"),
        ]
    );
    let rows: Vec<_> = table_data.rows.try_collect().await.unwrap();
    assert_eq!(rows, vec![vec![json!(["oops", "test"]), json!([1, 2, 3])]]);
    driver.release().await.unwrap();
}

#[tokio::test]
async fn stream_user_defined_types() {
    let url = require_pg!();
    let driver = driver(&url);
    exec(&driver, "DROP TYPE IF EXISTS CUBEJS_TEST_POINT CASCADE").await;
    exec(&driver, "DROP DOMAIN IF EXISTS CUBEJS_TEST_INT CASCADE").await;
    exec(&driver, "CREATE TYPE CUBEJS_TEST_POINT AS (x int, y int)").await;
    exec(&driver, "CREATE DOMAIN CUBEJS_TEST_INT AS int").await;

    let table_data = driver
        .stream(
            "SELECT
          ARRAY[CAST(ROW(1, 2) as CUBEJS_TEST_POINT)] as points,
          CAST(5 as CUBEJS_TEST_INT) as aliased",
            &[],
            &StreamOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        table_data.columns,
        vec![Column::new("points", "text"), Column::new("aliased", "int")]
    );
    let rows: Vec<_> = table_data.rows.try_collect().await.unwrap();
    assert_eq!(rows, vec![vec![json!(["(1,2)"]), json!(5)]]);
    driver.release().await.unwrap();
}

#[tokio::test]
async fn stream_errors() {
    let url = require_pg!();
    let driver = driver(&url);
    let err = driver
        .stream(
            "select * from test.random_name_for_table_that_doesnot_exist_sql_must_fail",
            &[],
            &StreamOptions::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "relation \"test.random_name_for_table_that_doesnot_exist_sql_must_fail\" does not exist"
    );
    assert!(matches!(err, DriverError::Database { .. }));

    let too_many: Vec<Value> = vec![json!("foo"); 65_536];
    let err = driver
        .stream("select 1", &too_many, &StreamOptions::default())
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "PostgreSQL protocol does not support more than 65535 parameters, but 65536 passed"
    );
    driver.release().await.unwrap();
}

#[tokio::test]
async fn schema_introspection() {
    let url = require_pg!();
    let driver = driver(&url);
    driver
        .create_schema_if_not_exists("cube_intro")
        .await
        .unwrap();
    exec(&driver, "DROP TABLE IF EXISTS cube_intro.orders").await;
    exec(&driver, "DROP TABLE IF EXISTS cube_intro.users").await;
    exec(
        &driver,
        "CREATE TABLE cube_intro.users (id int PRIMARY KEY, name varchar(20))",
    )
    .await;
    exec(
        &driver,
        "CREATE TABLE cube_intro.orders (id bigint PRIMARY KEY, user_id int REFERENCES cube_intro.users(id), amount numeric(10, 2), created_at timestamp)",
    )
    .await;

    let structure = driver.tables_schema().await.unwrap();
    let orders = &structure["cube_intro"]["orders"];
    let names: Vec<_> = orders.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["amount", "created_at", "id", "user_id"]);
    assert_eq!(orders[0].type_, "numeric");
    assert_eq!(orders[1].type_, "timestamp without time zone");

    let v2 = driver.tables_schema_v2().await.unwrap();
    let orders = &v2["cube_intro"]["orders"];
    let id = orders.iter().find(|c| c.name == "id").unwrap();
    assert_eq!(id.attributes, vec!["primaryKey"]);
    let user_id = orders.iter().find(|c| c.name == "user_id").unwrap();
    assert_eq!(user_id.foreign_keys.len(), 1);
    assert_eq!(user_id.foreign_keys[0].target_table, "users");
    assert_eq!(user_id.foreign_keys[0].target_column, "id");

    let schemas = driver.get_schemas().await.unwrap();
    assert!(schemas.iter().any(|s| s.schema_name == "cube_intro"));

    let tables = driver
        .get_tables_for_specific_schemas(&[SchemaName {
            schema_name: "cube_intro".into(),
        }])
        .await
        .unwrap();
    let mut table_names: Vec<_> = tables.iter().map(|t| t.table_name.clone()).collect();
    table_names.sort();
    assert_eq!(table_names, vec!["orders", "users"]);

    let columns = driver
        .get_columns_for_specific_tables(&[SchemaTable {
            schema_name: "cube_intro".into(),
            table_name: "orders".into(),
        }])
        .await
        .unwrap();
    assert_eq!(columns.len(), 4);
    let id = columns.iter().find(|c| c.column_name == "id").unwrap();
    assert_eq!(id.attributes, vec!["primaryKey"]);
    assert_eq!(id.data_type, "bigint");
    // The filtered path reports the same key as the unfiltered one. In Node.js
    // it returns nothing, because `foreignKeysQuery` gives the *referenced*
    // table the alias `columns` that the condition filters on; the Rust query
    // aliases the referencing table instead. See `PostgresDriver::foreign_keys_query`.
    let user_id = columns.iter().find(|c| c.column_name == "user_id").unwrap();
    assert_eq!(user_id.foreign_keys.len(), 1);
    assert_eq!(user_id.foreign_keys[0].target_table, "users");
    assert_eq!(user_id.foreign_keys[0].target_column, "id");

    // A table with no foreign key of its own reports none.
    let users_columns = driver
        .get_columns_for_specific_tables(&[SchemaTable {
            schema_name: "cube_intro".into(),
            table_name: "users".into(),
        }])
        .await
        .unwrap();
    assert!(users_columns.iter().all(|c| c.foreign_keys.is_empty()));

    let types = driver
        .table_column_types("cube_intro.orders")
        .await
        .unwrap();
    assert_eq!(
        types,
        vec![
            Column::new("id", "bigint"),
            Column::new("user_id", "int"),
            Column::new("amount", "decimal"),
            Column::new("created_at", "timestamp"),
        ]
    );

    let types = driver
        .query_column_types(
            "SELECT id, amount, created_at FROM cube_intro.orders",
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        types,
        vec![
            Column::new("id", "bigint"),
            Column::new("amount", "decimal"),
            Column::new("created_at", "timestamp"),
        ]
    );

    exec(&driver, "DROP TABLE cube_intro.orders").await;
    exec(&driver, "DROP TABLE cube_intro.users").await;
    driver.release().await.unwrap();
}

#[tokio::test]
async fn session_settings_and_pool() {
    let url = require_pg!();
    let driver = driver(&url);
    let data = driver
        .query(
            "SELECT current_setting('TimeZone') AS tz, current_setting('statement_timeout') AS st",
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(data.get_string(0, "tz").as_deref(), Some("UTC"));
    assert_eq!(data.get_string(0, "st").as_deref(), Some("10min"));

    // concurrent queries share the pool
    let futures = (0..8).map(|i| {
        let driver = &driver;
        async move {
            driver
                .query(
                    &format!("SELECT {i} AS n, pg_sleep(0.05)"),
                    &[],
                    &QueryOptions::default(),
                )
                .await
                .unwrap()
                .get_i64(0, "n")
                .unwrap()
        }
    });
    let mut results = futures::future::join_all(futures).await;
    results.sort();
    assert_eq!(results, (0..8).collect::<Vec<_>>());
    assert!(driver.pool_size() <= 4);

    driver.release().await.unwrap();
    driver.release().await.unwrap();
    let err = driver
        .query("SELECT 1", &[], &QueryOptions::default())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("closed"));
}

#[tokio::test]
async fn connection_errors_are_reported() {
    let url = require_pg!();
    let mut config = PostgresConfig::from_url(&url);
    config.driver.data_source.password = Some("definitely-wrong-password".into());
    config.driver.data_source.user = Some("definitely_wrong_user".into());
    let driver = PostgresDriver::new(config).unwrap();
    let err = driver.test_connection().await.unwrap_err();
    assert!(
        matches!(err, DriverError::Connection { .. }),
        "unexpected error: {err}"
    );
    assert!(err
        .to_string()
        .starts_with("Unable to connect to the database (postgres#default):"));
    let err = driver
        .query("SELECT 1", &[], &QueryOptions::default())
        .await
        .unwrap_err();
    assert!(
        matches!(err, DriverError::Connection { .. }),
        "unexpected error: {err}"
    );
    driver.release().await.unwrap();
}

/// Two schemas holding tables of the same shape.
///
/// Postgres names a generated constraint per schema, so `orders_user_id_fkey`
/// exists in both. Joining `information_schema` on the constraint name alone
/// returns their cross product, and every column came back carrying every
/// foreign key in the database. The joins carry catalog and schema too.
#[tokio::test]
async fn foreign_keys_do_not_leak_across_schemas() {
    let url = require_pg!();
    let driver = driver(&url);

    for schema in ["fk_a", "fk_b"] {
        driver.create_schema_if_not_exists(schema).await.unwrap();
        exec(&driver, &format!("DROP TABLE IF EXISTS {schema}.orders")).await;
        exec(&driver, &format!("DROP TABLE IF EXISTS {schema}.users")).await;
        exec(
            &driver,
            &format!("CREATE TABLE {schema}.users (id int PRIMARY KEY)"),
        )
        .await;
        exec(
            &driver,
            &format!(
                "CREATE TABLE {schema}.orders (
                     id int PRIMARY KEY,
                     user_id int REFERENCES {schema}.users(id)
                 )"
            ),
        )
        .await;
    }

    let structure = driver.tables_schema_v2().await.unwrap();

    for schema in ["fk_a", "fk_b"] {
        let orders = &structure[schema]["orders"];
        let user_id = orders
            .iter()
            .find(|c| c.name == "user_id")
            .expect("user_id column");

        assert_eq!(
            user_id.foreign_keys.len(),
            1,
            "{schema}.orders.user_id carries {:?}",
            user_id.foreign_keys
        );
        assert_eq!(user_id.foreign_keys[0].target_table, "users");
        assert_eq!(user_id.foreign_keys[0].target_column, "id");

        // The primary key carries none of its neighbour's keys either.
        let id = orders.iter().find(|c| c.name == "id").expect("id column");
        assert!(id.foreign_keys.is_empty(), "{:?}", id.foreign_keys);
    }

    for schema in ["fk_a", "fk_b"] {
        exec(&driver, &format!("DROP TABLE IF EXISTS {schema}.orders")).await;
        exec(&driver, &format!("DROP TABLE IF EXISTS {schema}.users")).await;
    }
}
