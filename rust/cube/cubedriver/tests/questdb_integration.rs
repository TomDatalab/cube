#![cfg(feature = "questdb")]
//! Integration tests for [`QuestDriver`], ported from
//! `packages/cubejs-questdb-driver/test/QuestDriver.test.ts`.
//!
//! They run only when `CUBEJS_TEST_QUESTDB_URL` is set, e.g.
//! `docker run -d -p 8812:8812 questdb/questdb` and
//! `CUBEJS_TEST_QUESTDB_URL=postgres://admin:quest@localhost:8812/qdb`.

use cubedriver::{
    Column, DownloadQueryResultsOptions, DownloadedData, Driver, QueryOptions, QueryResult,
    QuestConfig, QuestDriver,
};
use serde_json::json;

fn questdb_url() -> Option<String> {
    std::env::var("CUBEJS_TEST_QUESTDB_URL")
        .ok()
        .filter(|s| !s.is_empty())
}

macro_rules! require_questdb {
    () => {
        match questdb_url() {
            Some(url) => url,
            None => {
                eprintln!("CUBEJS_TEST_QUESTDB_URL is not set, skipping");
                return;
            }
        }
    };
}

fn driver(url: &str) -> QuestDriver {
    QuestDriver::new(QuestConfig::from_url(url)).unwrap()
}

#[tokio::test]
async fn query_upload_and_schema() {
    let url = require_questdb!();
    let driver = driver(&url);
    driver.test_connection().await.unwrap();
    let _ = driver
        .query(
            "DROP TABLE IF EXISTS query_test",
            &[],
            &QueryOptions::default(),
        )
        .await;

    let columns = vec![
        Column::new("id", "long"),
        Column::new("created", "date"),
        Column::new("price", "double"),
    ];
    let data = QueryResult::new(
        columns.clone(),
        vec![
            vec![json!(1), json!("2020-01-01T00:00:00.000Z"), json!(100.5)],
            vec![json!(2), json!("2020-01-02T00:00:00.000Z"), json!(200.5)],
            vec![json!(3), json!("2020-01-03T00:00:00.000Z"), json!(300.5)],
        ],
    );
    driver
        .upload_table("query_test", &columns, &data)
        .await
        .unwrap();

    let result = driver
        .query("select * from query_test", &[], &QueryOptions::default())
        .await
        .unwrap();
    assert_eq!(
        result.rows,
        vec![
            vec![json!("1"), json!("2020-01-01T00:00:00.000"), json!(100.5)],
            vec![json!("2"), json!("2020-01-02T00:00:00.000"), json!(200.5)],
            vec![json!("3"), json!("2020-01-03T00:00:00.000"), json!(300.5)],
        ]
    );

    // `downloadQueryResults` maps the fields through the pg-types names.
    let DownloadedData::Memory(download) = driver
        .download_query_results(
            "select * from query_test where id = $1",
            &[json!(2)],
            &DownloadQueryResultsOptions::default(),
        )
        .await
        .unwrap()
    else {
        panic!("memory download expected");
    };
    assert_eq!(download.rows.len(), 1);
    assert_eq!(download.columns[0].name, "id");
    assert_eq!(download.columns[0].type_.to_string(), "bigint");

    // `tablesSchema`: every table under the empty schema name.
    let schema = driver.tables_schema().await.unwrap();
    let table = &schema[""]["query_test"];
    assert_eq!(
        table
            .iter()
            .map(|c| (c.name.as_str(), c.type_.as_str()))
            .collect::<Vec<_>>(),
        vec![("id", "LONG"), ("created", "date"), ("price", "DOUBLE")]
    );
    assert!(schema[""].keys().all(|t| !t.starts_with("sys.")));

    // no-op: QuestDB has no schemas
    driver
        .create_schema_if_not_exists("anything")
        .await
        .unwrap();

    driver
        .query("DROP TABLE query_test", &[], &QueryOptions::default())
        .await
        .unwrap();
    driver.release().await.unwrap();
}

#[tokio::test]
async fn query_exception() {
    let url = require_questdb!();
    let driver = driver(&url);
    let err = driver
        .query(
            "select * from random_name_for_table_that_doesnot_exist_sql_must_fail",
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "table does not exist [table=random_name_for_table_that_doesnot_exist_sql_must_fail]"
    );
    driver.release().await.unwrap();
}
