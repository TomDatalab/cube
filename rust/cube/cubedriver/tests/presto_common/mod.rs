//! Checks shared by `prestodb_integration.rs` and `trino_integration.rs`:
//! both coordinators ship the `tpch` and `memory` catalogs.

#![allow(dead_code)]

use cubedriver::prestodb::{Engine, PrestoConfig};
use cubedriver::{
    Column, DownloadQueryResultsOptions, DownloadedData, Driver, DriverConfig, QueryOptions,
    QueryResult, SchemaName, SchemaTable, StreamOptions,
};
use futures::TryStreamExt;
use serde_json::{json, Value};

pub fn config(engine: Engine, host: &str, port: u16) -> PrestoConfig {
    let mut config = PrestoConfig::from_driver_config(DriverConfig::default(), engine);
    config.host = host.to_string();
    config.port = port;
    config.catalog = Some("tpch".into());
    config.schema = Some("tiny".into());
    config.user = Some("cube".into());
    config
}

pub async fn query(driver: &dyn Driver, sql: &str, params: &[Value]) -> QueryResult {
    driver
        .query(sql, params, &QueryOptions::default())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
}

pub async fn scalar_types_and_params(driver: &dyn Driver) {
    let data = query(
        driver,
        "SELECT
           CAST(1 AS INTEGER) AS i,
           CAST(9007199254740993 AS BIGINT) AS big,
           CAST(1.5 AS DOUBLE) AS dbl,
           CAST('12.30' AS DECIMAL(10, 2)) AS dec,
           ? AS s,
           ? AS n,
           DATE '2020-01-02' AS d,
           TIMESTAMP '2020-01-02 03:04:05.678' AS ts,
           true AS b,
           CAST(NULL AS VARCHAR) AS nothing",
        &[json!("it's"), json!(42)],
    )
    .await;
    assert_eq!(data.len(), 1);
    assert_eq!(
        data.columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        vec!["i", "big", "dbl", "dec", "s", "n", "d", "ts", "b", "nothing"]
    );
    assert_eq!(data.columns[0], Column::new("i", "int"));
    assert_eq!(data.columns[1], Column::new("big", "bigint"));
    assert_eq!(data.get(0, "i"), Some(&json!(1)));
    // i64 precision survives (Node lost it in JSON.parse).
    assert_eq!(data.get(0, "big"), Some(&json!(9007199254740993i64)));
    assert_eq!(data.get(0, "dbl"), Some(&json!(1.5)));
    assert_eq!(data.get(0, "dec"), Some(&json!("12.30")));
    assert_eq!(data.get(0, "s"), Some(&json!("it's")));
    assert_eq!(data.get(0, "n"), Some(&json!(42)));
    assert_eq!(data.get(0, "d"), Some(&json!("2020-01-02")));
    assert_eq!(data.get(0, "ts"), Some(&json!("2020-01-02 03:04:05.678")));
    assert_eq!(data.get(0, "b"), Some(&json!(true)));
    assert_eq!(data.get(0, "nothing"), Some(&Value::Null));
}

/// ~60k rows arrive in several pages; the order must survive.
pub async fn multi_page_results_keep_order(driver: &dyn Driver) {
    let data = query(
        driver,
        "SELECT orderkey, linenumber FROM lineitem ORDER BY orderkey, linenumber",
        &[],
    )
    .await;
    assert_eq!(data.len(), 60175);
    let keys: Vec<(i64, i64)> = data
        .rows
        .iter()
        .map(|r| (r[0].as_i64().unwrap(), r[1].as_i64().unwrap()))
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
}

pub async fn streaming(driver: &dyn Driver) {
    let stream = driver
        .stream(
            "SELECT orderkey, comment FROM lineitem WHERE linenumber = ?",
            &[json!(1)],
            &StreamOptions {
                high_water_mark: 100,
                request_id: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        stream.columns,
        vec![
            Column::new("orderkey", "bigint"),
            Column::new("comment", "varchar(44)")
        ]
    );
    let rows: Vec<_> = stream.rows.try_collect().await.unwrap();
    assert_eq!(rows.len(), 15000);

    // Dropping a stream early must not hang or poison the driver.
    let stream = driver
        .stream("SELECT * FROM lineitem", &[], &StreamOptions::default())
        .await
        .unwrap();
    drop(stream);

    let types = driver
        .query_column_types(
            "SELECT nationkey, name FROM nation",
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        types,
        vec![
            Column::new("nationkey", "bigint"),
            Column::new("name", "varchar(25)")
        ]
    );

    let DownloadedData::Stream(stream) = driver
        .download_query_results(
            "SELECT name FROM nation ORDER BY name",
            &[],
            &DownloadQueryResultsOptions {
                stream_import: true,
                ..Default::default()
            },
        )
        .await
        .unwrap()
    else {
        panic!("expected a stream");
    };
    let rows: Vec<_> = stream.rows.try_collect().await.unwrap();
    assert_eq!(rows.len(), 25);
    assert_eq!(rows[0], vec![json!("ALGERIA")]);

    let DownloadedData::Memory(memory) = driver
        .download_query_results(
            "SELECT nationkey FROM nation",
            &[],
            &DownloadQueryResultsOptions::default(),
        )
        .await
        .unwrap()
    else {
        panic!("expected memory data");
    };
    assert_eq!(memory.len(), 25);
    assert_eq!(memory.columns, vec![Column::new("nationkey", "int")]);
}

pub async fn introspection(driver: &dyn Driver) {
    let structure = driver.tables_schema().await.unwrap();
    // CUBEJS_DB_NAME=tiny restricts the information schema query.
    assert_eq!(structure.keys().collect::<Vec<_>>(), vec!["tiny"]);
    let nation = &structure["tiny"]["nation"];
    assert_eq!(
        nation.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        vec!["comment", "name", "nationkey", "regionkey"]
    );

    let schemas = driver.get_schemas().await.unwrap();
    assert!(schemas.iter().any(|s| s.schema_name == "sf1"));
    let tables = driver
        .get_tables_for_specific_schemas(&[SchemaName {
            schema_name: "tiny".into(),
        }])
        .await
        .unwrap();
    assert!(tables.iter().any(|t| t.table_name == "lineitem"));
    let columns = driver
        .get_columns_for_specific_tables(&[SchemaTable {
            schema_name: "tiny".into(),
            table_name: "region".into(),
        }])
        .await
        .unwrap();
    let mut names: Vec<_> = columns.iter().map(|c| c.column_name.as_str()).collect();
    names.sort();
    assert_eq!(names, vec!["comment", "name", "regionkey"]);
    assert_eq!(
        driver.get_tables_query("tiny").await.unwrap().len(),
        tables.len()
    );
}

pub async fn errors(driver: &dyn Driver) {
    let err = driver
        .query(
            "SELECT no_such_column FROM nation",
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("no_such_column"),
        "unexpected error: {err}"
    );
}

/// Writes through the `memory` catalog: schema, CTAS, drop.
pub async fn memory_catalog_writes(driver: &dyn Driver) {
    driver.create_schema_if_not_exists("cube_it").await.unwrap();
    let _ = driver
        .query(
            "DROP TABLE IF EXISTS memory.cube_it.t",
            &[],
            &QueryOptions::default(),
        )
        .await;
    query(
        driver,
        "CREATE TABLE memory.cube_it.t AS SELECT nationkey, name FROM tpch.tiny.nation WHERE regionkey = ?",
        &[json!(1)],
    )
    .await;
    let data = query(driver, "SELECT count(*) AS c FROM memory.cube_it.t", &[]).await;
    assert_eq!(data.get(0, "c"), Some(&json!(5)));
    driver
        .drop_table("memory.cube_it.t", &QueryOptions::default())
        .await
        .unwrap();
}
