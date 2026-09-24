//! Unit tests (ported from `test/unit/params-escaping.test.ts`) and
//! protocol tests against a local mock of Athena's awsJson1.1 endpoint and
//! S3's `ListObjectsV2`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::TryStreamExt;
use serde_json::{json, Value};

use super::*;
use crate::aws_test_server::{MockRequest, MockResponse, MockServer};
use crate::types::GenericType;

// ---------------------------------------------------------------------------
// params-escaping.test.ts
// ---------------------------------------------------------------------------

const LIKE_SQL: &str =
    r"SELECT * FROM orders WHERE LOWER(name) LIKE CONCAT('%', LOWER(?), '%') ESCAPE '\'";

#[test]
fn preserves_like_escape_sequences_emitted_by_the_schema_compiler() {
    assert_eq!(
        apply_params(LIKE_SQL, &[json!(r"new\_order\%")]),
        r"SELECT * FROM orders WHERE LOWER(name) LIKE CONCAT('%', LOWER('new\_order\%'), '%') ESCAPE '\'"
    );
}

#[test]
fn does_not_double_literal_backslashes_in_like_parameters() {
    assert_eq!(
        apply_params(LIKE_SQL, &[json!(r"folder\\name")]),
        r"SELECT * FROM orders WHERE LOWER(name) LIKE CONCAT('%', LOWER('folder\\name'), '%') ESCAPE '\'"
    );
}

#[test]
fn doubles_quotes_so_a_like_value_cannot_break_out_of_the_literal() {
    assert_eq!(
        apply_params(LIKE_SQL, &[json!("o'reilly'); DROP TABLE orders; --")]),
        r"SELECT * FROM orders WHERE LOWER(name) LIKE CONCAT('%', LOWER('o''reilly''); DROP TABLE orders; --'), '%') ESCAPE '\'"
    );
}

#[test]
fn keeps_the_literal_closed_for_a_backslash_then_quote_payload() {
    assert_eq!(
        apply_params(
            "SELECT * FROM orders WHERE name = ?",
            &[json!(r"foo\' OR 1=1 --")]
        ),
        r"SELECT * FROM orders WHERE name = 'foo\'' OR 1=1 --'"
    );
}

#[test]
fn keeps_the_literal_closed_for_a_value_ending_in_a_backslash() {
    assert_eq!(
        apply_params(
            "SELECT * FROM orders WHERE name = ? AND status = ?",
            &[json!(r"payload\"), json!("new")]
        ),
        r"SELECT * FROM orders WHERE name = 'payload\' AND status = 'new'"
    );
}

#[test]
fn keeps_a_literal_percent_sign_in_an_equality_parameter_verbatim() {
    assert_eq!(
        apply_params(
            "SELECT * FROM orders WHERE discount_label = ?",
            &[json!("100% cotton")]
        ),
        "SELECT * FROM orders WHERE discount_label = '100% cotton'"
    );
}

#[test]
fn passes_an_unescaped_percent_sign_through_a_like_parameter_untouched() {
    assert_eq!(
        apply_params(LIKE_SQL, &[json!("50%")]),
        r"SELECT * FROM orders WHERE LOWER(name) LIKE CONCAT('%', LOWER('50%'), '%') ESCAPE '\'"
    );
}

#[test]
fn escapes_quotes_in_a_value_that_also_contains_percent_signs() {
    assert_eq!(
        apply_params(LIKE_SQL, &[json!("50%' OR 1=1 --")]),
        r"SELECT * FROM orders WHERE LOWER(name) LIKE CONCAT('%', LOWER('50%'' OR 1=1 --'), '%') ESCAPE '\'"
    );
}

#[test]
fn escapes_every_element_of_an_array_parameter() {
    assert_eq!(
        apply_params(
            "SELECT * FROM orders WHERE status IN (?)",
            &[json!(["it's", "b"])]
        ),
        "SELECT * FROM orders WHERE status IN ('it''s', 'b')"
    );
}

#[test]
fn substitutes_multiple_placeholders_in_order() {
    let sql = format!("{LIKE_SQL} AND status = ? AND amount > ?");
    assert_eq!(
        apply_params(&sql, &[json!(r"pending\_review"), json!("new"), json!(100)]),
        r"SELECT * FROM orders WHERE LOWER(name) LIKE CONCAT('%', LOWER('pending\_review'), '%') ESCAPE '\' AND status = 'new' AND amount > 100"
    );
}

// ---------------------------------------------------------------------------
// Configuration and SQL
// ---------------------------------------------------------------------------

#[test]
fn s3_paths() {
    assert_eq!(normalize_s3_path("bucket/"), "s3://bucket");
    assert_eq!(
        normalize_s3_path("s3://bucket/prefix//"),
        "s3://bucket/prefix"
    );
    assert_eq!(
        split_s3_path("s3://bucket/prefix/table").unwrap(),
        ("bucket".to_string(), "/prefix/table".to_string())
    );
    // `/^[a-zA-Z]+:\/\//` does not match `s3://` (the digit), as in Node.
    assert_eq!(strip_scheme("s3://bucket"), "s3://bucket");
    assert_eq!(strip_scheme("https://bucket"), "bucket");
    assert_eq!(strip_scheme("bucket"), "bucket");
}

#[test]
fn env_configuration() {
    let env: std::collections::HashMap<String, String> = [
        ("CUBEJS_AWS_KEY", "AKIA"),
        ("CUBEJS_AWS_SECRET", "secret"),
        ("CUBEJS_AWS_REGION", "eu-west-1"),
        ("CUBEJS_AWS_S3_OUTPUT_LOCATION", "s3://out/"),
        ("CUBEJS_AWS_ATHENA_CATALOG", "AwsDataCatalog"),
        ("CUBEJS_AWS_ATHENA_ASSUME_ROLE_ARN", "arn:aws:iam::1:role/x"),
        ("CUBEJS_AWS_ATHENA_ASSUME_ROLE_EXTERNAL_ID", "ext"),
        ("CUBEJS_DB_EXPORT_BUCKET", "my-bucket/"),
        ("CUBEJS_DB_NAME", "analytics"),
        ("CUBEJS_DB_POLL_TIMEOUT", "30"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let driver_config = DriverConfig::from_env_source(&env, None, false).unwrap();
    let mut config = AthenaConfig::from_driver_config(driver_config);
    config.apply_env_source(&env).unwrap();
    assert_eq!(config.access_key_id.as_deref(), Some("AKIA"));
    assert_eq!(config.secret_access_key.as_deref(), Some("secret"));
    assert_eq!(config.region.as_deref(), Some("eu-west-1"));
    assert_eq!(config.s3_output_location.as_deref(), Some("s3://out/"));
    assert_eq!(config.work_group, "primary");
    assert_eq!(config.catalog.as_deref(), Some("AwsDataCatalog"));
    assert_eq!(config.database.as_deref(), Some("analytics"));
    assert_eq!(config.schema.as_deref(), Some("analytics"));
    assert_eq!(config.export_bucket.as_deref(), Some("s3://my-bucket"));
    assert_eq!(config.poll_timeout, Duration::from_secs(30));
    assert_eq!(config.poll_max_interval, Duration::from_secs(5));
    assert_eq!(
        config.assume_role_arn.as_deref(),
        Some("arn:aws:iam::1:role/x")
    );
    assert_eq!(config.assume_role_external_id.as_deref(), Some("ext"));
    assert!(!config.read_only);

    // Poll timeout falls back to the query timeout; workgroup is read too.
    let env: std::collections::HashMap<String, String> = [
        ("CUBEJS_DB_QUERY_TIMEOUT", "2m"),
        ("CUBEJS_AWS_ATHENA_WORKGROUP", "wg"),
        ("CUBEJS_DB_SCHEMA", "legacy"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let mut config =
        AthenaConfig::from_driver_config(DriverConfig::from_env_source(&env, None, false).unwrap());
    config.apply_env_source(&env).unwrap();
    assert_eq!(config.poll_timeout, Duration::from_secs(120));
    assert_eq!(config.work_group, "wg");
    assert_eq!(config.database, None);
    assert_eq!(config.schema.as_deref(), Some("legacy"));
}

#[test]
fn data_source_specific_env() {
    let env: std::collections::HashMap<String, String> = [
        ("CUBEJS_DATASOURCES", "default,lake"),
        ("CUBEJS_DS_LAKE_AWS_REGION", "us-west-2"),
        ("CUBEJS_DS_LAKE_AWS_ATHENA_WORKGROUP", "lake-wg"),
        ("CUBEJS_AWS_REGION", "eu-west-1"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let mut config = AthenaConfig::from_driver_config(
        DriverConfig::from_env_source(&env, Some("lake"), false).unwrap(),
    );
    config.apply_env_source(&env).unwrap();
    assert_eq!(config.region.as_deref(), Some("us-west-2"));
    assert_eq!(config.work_group, "lake-wg");
}

fn test_config(endpoint: &str) -> AthenaConfig {
    let mut config = AthenaConfig::from_driver_config(DriverConfig::default());
    config.region = Some("us-east-1".to_string());
    config.access_key_id = Some("AKIDEXAMPLE".to_string());
    config.secret_access_key = Some("secret".to_string());
    config.endpoint_url = Some(endpoint.to_string());
    config.s3_output_location = Some("s3://results/".to_string());
    config.poll_timeout = Duration::from_secs(10);
    config.poll_max_interval = Duration::from_millis(10);
    config
}

#[tokio::test]
async fn sql_and_capabilities() {
    let mut config = test_config("http://127.0.0.1:1");
    config.schema = Some("analytics".to_string());
    let driver = AthenaDriver::new(config).unwrap();
    assert_eq!(driver.quote_identifier("a"), "\"a\"");
    assert_eq!(driver.param(0), "?");
    let caps = driver.capabilities();
    assert!(caps.unload_without_temp_table);
    assert!(caps.incremental_schema_loading);
    assert!(!driver.read_only());
    assert!(driver
        .information_schema_query()
        .ends_with(" AND columns.table_schema = 'analytics'"));
    assert!(!driver
        .is_unload_supported(&UnloadOptions::default())
        .await
        .unwrap());
    assert_eq!(
        driver
            .unload("t", &UnloadOptions::default())
            .await
            .unwrap_err()
            .to_string(),
        "Export bucket is not configured."
    );
    assert_eq!(DEFAULT_CONCURRENCY, 10);
}

#[test]
fn merge_schemas_keeps_the_first_definition() {
    let col = |n: &str| SchemaColumn {
        name: n.to_string(),
        type_: "varchar".to_string(),
        attributes: vec![],
        foreign_keys: vec![],
    };
    let mut a = DatabaseStructure::new();
    a.entry("s".into())
        .or_default()
        .insert("t".into(), vec![col("a")]);
    let mut b = DatabaseStructure::new();
    b.entry("s".into())
        .or_default()
        .insert("t".into(), vec![col("b")]);
    b.entry("s".into())
        .or_default()
        .insert("v".into(), vec![col("c")]);
    let merged = merge_schemas(vec![a, b]);
    assert_eq!(merged["s"]["t"][0].name, "a");
    assert_eq!(merged["s"]["v"][0].name, "c");
}

// ---------------------------------------------------------------------------
// Mock Athena
// ---------------------------------------------------------------------------

const JSON_11: &str = "application/x-amz-json-1.1";

fn target(req: &MockRequest) -> String {
    req.header("x-amz-target")
        .unwrap_or_default()
        .trim_start_matches("AmazonAthena.")
        .to_string()
}

fn datum(v: Option<&str>) -> Value {
    match v {
        Some(v) => json!({ "VarCharValue": v }),
        None => json!({}),
    }
}

fn result_page(
    columns: &[(&str, &str)],
    rows: &[Vec<Option<&str>>],
    header: bool,
    next: Option<&str>,
) -> Value {
    let mut out_rows = Vec::new();
    if header {
        out_rows.push(
            json!({ "Data": columns.iter().map(|(n, _)| datum(Some(n))).collect::<Vec<_>>() }),
        );
    }
    for row in rows {
        out_rows.push(json!({ "Data": row.iter().map(|v| datum(*v)).collect::<Vec<_>>() }));
    }
    let mut page = json!({
        "ResultSet": {
            "Rows": out_rows,
            "ResultSetMetadata": {
                "ColumnInfo": columns.iter().map(|(n, t)| json!({ "Name": n, "Type": t })).collect::<Vec<_>>()
            }
        },
        "UpdateCount": 0
    });
    if let Some(next) = next {
        page["NextToken"] = json!(next);
    }
    page
}

/// A two-page `SELECT`: the first `GetQueryExecution` says RUNNING, then
/// SUCCEEDED.
async fn select_server() -> (MockServer, Arc<AtomicUsize>) {
    let polls = Arc::new(AtomicUsize::new(0));
    let polls2 = polls.clone();
    let server = MockServer::start(move |req| {
        let body = req.json();
        match target(req).as_str() {
            "StartQueryExecution" => {
                MockResponse::json(200, JSON_11, json!({ "QueryExecutionId": "qid-1" }))
            }
            "GetQueryExecution" => {
                let n = polls2.fetch_add(1, Ordering::SeqCst);
                let state = if n == 0 { "RUNNING" } else { "SUCCEEDED" };
                MockResponse::json(
                    200,
                    JSON_11,
                    json!({ "QueryExecution": { "QueryExecutionId": "qid-1", "Status": { "State": state } } }),
                )
            }
            "GetQueryResults" => {
                let columns = [("status", "varchar"), ("amount", "bigint"), ("ts", "timestamp")];
                if body.get("NextToken").is_none() {
                    MockResponse::json(
                        200,
                        JSON_11,
                        result_page(
                            &columns,
                            &[vec![Some("new"), Some("300"), Some("2020-01-01 00:00:00.000")]],
                            true,
                            Some("page-2"),
                        ),
                    )
                } else {
                    MockResponse::json(
                        200,
                        JSON_11,
                        result_page(&columns, &[vec![None, Some("500"), None]], false, None),
                    )
                }
            }
            "StopQueryExecution" => MockResponse::json(200, JSON_11, json!({})),
            "GetWorkGroup" => MockResponse::json(
                200,
                JSON_11,
                json!({ "WorkGroup": { "Name": body["WorkGroup"], "State": "ENABLED" } }),
            ),
            other => MockResponse::json(
                400,
                JSON_11,
                json!({ "__type": "InvalidRequestException", "Message": format!("unexpected {other}") }),
            ),
        }
    })
    .await;
    (server, polls)
}

#[tokio::test]
async fn query_polls_and_pages_through_results() {
    let (server, polls) = select_server().await;
    let mut config = test_config(&server.endpoint);
    config.catalog = Some("AwsDataCatalog".to_string());
    config.database = Some("db".to_string());
    config.work_group = "wg".to_string();
    let driver = AthenaDriver::new(config).unwrap();

    driver.test_connection().await.unwrap();

    let result = driver
        .query(
            "SELECT * FROM t WHERE status = ? AND n > ?",
            &[json!("it's"), json!(5)],
            &QueryOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        result.columns,
        vec![
            Column::new("status", "text"),
            Column::new("amount", "bigint"),
            Column::new("ts", "timestamp"),
        ]
    );
    // Header row skipped, every value a string, missing VarCharValue → null.
    assert_eq!(
        result.rows,
        vec![
            vec![json!("new"), json!("300"), json!("2020-01-01 00:00:00.000")],
            vec![Value::Null, json!("500"), Value::Null],
        ]
    );
    assert_eq!(polls.load(Ordering::SeqCst), 2);

    let requests = server.requests();
    let start = requests
        .iter()
        .find(|r| target(r) == "StartQueryExecution")
        .unwrap()
        .json();
    assert_eq!(
        start["QueryString"],
        "SELECT * FROM t WHERE status = 'it''s' AND n > 5"
    );
    assert_eq!(start["WorkGroup"], "wg");
    assert_eq!(
        start["ResultConfiguration"]["OutputLocation"],
        "s3://results/"
    );
    assert_eq!(start["QueryExecutionContext"]["Catalog"], "AwsDataCatalog");
    assert_eq!(start["QueryExecutionContext"]["Database"], "db");
    // Requests are SigV4-signed with the static keys.
    let auth = requests[0].header("authorization").unwrap();
    assert!(
        auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/"),
        "{auth}"
    );
    assert!(auth.contains("/us-east-1/athena/aws4_request"), "{auth}");
    let pages: Vec<Value> = requests
        .iter()
        .filter(|r| target(r) == "GetQueryResults")
        .map(|r| r.json())
        .collect();
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[1]["NextToken"], "page-2");
    // No stop for a query that completed.
    assert!(!requests.iter().any(|r| target(r) == "StopQueryExecution"));
}

#[tokio::test]
async fn no_execution_context_without_catalog_or_database() {
    let (server, _) = select_server().await;
    let driver = AthenaDriver::new(test_config(&server.endpoint)).unwrap();
    driver
        .query("SELECT 1", &[], &QueryOptions::default())
        .await
        .unwrap();
    let start = server
        .requests()
        .into_iter()
        .find(|r| target(r) == "StartQueryExecution")
        .unwrap()
        .json();
    assert!(start.get("QueryExecutionContext").is_none());
    assert_eq!(start["WorkGroup"], "primary");
}

#[tokio::test]
async fn stream_and_download() {
    let (server, _) = select_server().await;
    let driver = AthenaDriver::new(test_config(&server.endpoint)).unwrap();
    let stream = driver
        .stream("SELECT 1", &[], &StreamOptions::default())
        .await
        .unwrap();
    assert_eq!(stream.columns.len(), 3);
    let rows: Vec<Row> = stream.rows.try_collect().await.unwrap();
    assert_eq!(rows.len(), 2);

    let options = DownloadQueryResultsOptions::default();
    match driver
        .download_query_results("SELECT 1", &[], &options)
        .await
        .unwrap()
    {
        DownloadedData::Memory(m) => {
            assert_eq!(m.rows.len(), 2);
            assert_eq!(m.columns[1].type_, GenericType::Bigint);
        }
        other => panic!("unexpected {other:?}"),
    }
    let options = DownloadQueryResultsOptions {
        stream_import: true,
        ..Default::default()
    };
    assert!(matches!(
        driver
            .download_query_results("SELECT 1", &[], &options)
            .await
            .unwrap(),
        DownloadedData::Stream(_)
    ));
}

#[tokio::test]
async fn failed_and_cancelled_queries() {
    let server = MockServer::start(|req| match target(req).as_str() {
        "StartQueryExecution" => {
            let q = req.json()["QueryString"].as_str().unwrap_or_default().to_string();
            MockResponse::json(200, JSON_11, json!({ "QueryExecutionId": q }))
        }
        "GetQueryExecution" => {
            let id = req.json()["QueryExecutionId"].as_str().unwrap_or_default().to_string();
            let status = if id == "fail" {
                json!({ "State": "FAILED", "StateChangeReason": "line 1:8: Column 'x' cannot be resolved" })
            } else {
                json!({ "State": "CANCELLED" })
            };
            MockResponse::json(200, JSON_11, json!({ "QueryExecution": { "Status": status } }))
        }
        _ => MockResponse::json(
            400,
            JSON_11,
            json!({ "__type": "InvalidRequestException", "Message": "Queries of this type are not supported" }),
        ),
    })
    .await;
    let driver = AthenaDriver::new(test_config(&server.endpoint)).unwrap();
    let err = driver
        .query("fail", &[], &QueryOptions::default())
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "line 1:8: Column 'x' cannot be resolved");
    let err = driver
        .query("cancel", &[], &QueryOptions::default())
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "Query has been cancelled");
    // Service errors keep the service message and code.
    let err = driver.test_connection().await.unwrap_err();
    match err {
        DriverError::Database { message, code } => {
            assert_eq!(message, "Queries of this type are not supported");
            assert_eq!(code.as_deref(), Some("InvalidRequestException"));
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn poll_timeout_stops_the_query() {
    let server = MockServer::start(|req| match target(req).as_str() {
        "StartQueryExecution" => {
            MockResponse::json(200, JSON_11, json!({ "QueryExecutionId": "slow" }))
        }
        "GetQueryExecution" => MockResponse::json(
            200,
            JSON_11,
            json!({ "QueryExecution": { "Status": { "State": "RUNNING" } } }),
        ),
        _ => MockResponse::json(200, JSON_11, json!({})),
    })
    .await;
    let mut config = test_config(&server.endpoint);
    config.poll_timeout = Duration::from_millis(300);
    let driver = AthenaDriver::new(config).unwrap();
    let err = driver
        .query("SELECT slow", &[], &QueryOptions::default())
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "Athena job timeout reached 300ms");
    let stops: Vec<Value> = server
        .requests()
        .iter()
        .filter(|r| target(r) == "StopQueryExecution")
        .map(|r| r.json())
        .collect();
    assert_eq!(stops, vec![json!({ "QueryExecutionId": "slow" })]);
}

#[tokio::test]
async fn dropping_a_running_query_stops_it() {
    let server = MockServer::start(|req| match target(req).as_str() {
        "StartQueryExecution" => {
            MockResponse::json(200, JSON_11, json!({ "QueryExecutionId": "dropped" }))
        }
        "GetQueryExecution" => MockResponse::json(
            200,
            JSON_11,
            json!({ "QueryExecution": { "Status": { "State": "QUEUED" } } }),
        ),
        _ => MockResponse::json(200, JSON_11, json!({})),
    })
    .await;
    let driver = AthenaDriver::new(test_config(&server.endpoint)).unwrap();
    // Drop the query future once the query is known to be running.
    let options = QueryOptions::default();
    tokio::select! {
        _ = driver.query("SELECT 1", &[], &options) => panic!("query must not finish"),
        _ = async {
            while !server.requests().iter().any(|r| target(r) == "GetQueryExecution") {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        } => {}
    }
    // The stop is spawned from Drop; give it a moment.
    for _ in 0..200 {
        if server
            .requests()
            .iter()
            .any(|r| target(r) == "StopQueryExecution")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let stop = server
        .requests()
        .into_iter()
        .find(|r| target(r) == "StopQueryExecution")
        .expect("StopQueryExecution sent on drop");
    assert_eq!(stop.json()["QueryExecutionId"], "dropped");
}

#[tokio::test]
async fn load_pre_aggregation_requires_an_output_location() {
    let (server, _) = select_server().await;
    let mut config = test_config(&server.endpoint);
    config.s3_output_location = None;
    let driver = AthenaDriver::new(config).unwrap();
    let err = driver
        .load_pre_aggregation_into_table(
            "s.t",
            "CREATE TABLE s.t AS SELECT 1",
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Unload is not configured. Please define CUBEJS_AWS_S3_OUTPUT_LOCATION env var "
    );
}

#[tokio::test]
async fn show_columns_rows_are_keyed_by_column() {
    let server = MockServer::start(|req| {
        let body = req.json();
        match target(req).as_str() {
            "StartQueryExecution" => {
                let q = body["QueryString"].as_str().unwrap_or_default();
                let id = if q.contains("information_schema.columns") {
                    "cols"
                } else if q.contains("information_schema.tables") {
                    "tables"
                } else {
                    "show"
                };
                MockResponse::json(200, JSON_11, json!({ "QueryExecutionId": id }))
            }
            "GetQueryExecution" => MockResponse::json(
                200,
                JSON_11,
                json!({ "QueryExecution": { "Status": { "State": "SUCCEEDED" } } }),
            ),
            "GetQueryResults" => {
                let page = match body["QueryExecutionId"].as_str().unwrap_or_default() {
                    "cols" => result_page(
                        &[
                            ("column_name", "varchar"),
                            ("table_name", "varchar"),
                            ("table_schema", "varchar"),
                            ("data_type", "varchar"),
                        ],
                        &[vec![Some("id"), Some("orders"), Some("db"), Some("bigint")]],
                        true,
                        None,
                    ),
                    "tables" => result_page(
                        &[("schema", "varchar"), ("name", "varchar")],
                        &[
                            vec![Some("db"), Some("orders")],
                            vec![Some("db"), Some("orders_view")],
                        ],
                        true,
                        None,
                    ),
                    // SHOW COLUMNS: no header row is really a header, and
                    // the metadata column name is irrelevant.
                    _ => result_page(
                        &[("field", "string")],
                        &[vec![Some("amount\tdouble")]],
                        true,
                        None,
                    ),
                };
                MockResponse::json(200, JSON_11, page)
            }
            _ => MockResponse::json(200, JSON_11, json!({})),
        }
    })
    .await;
    let driver = AthenaDriver::new(test_config(&server.endpoint)).unwrap();
    let schema = driver.tables_schema().await.unwrap();
    assert_eq!(schema["db"]["orders"][0].name, "id");
    assert_eq!(schema["db"]["orders_view"][0].name, "amount");
    assert_eq!(schema["db"]["orders_view"][0].type_, "double");
    let show = server
        .requests()
        .into_iter()
        .map(|r| {
            r.json()["QueryString"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .find(|q| q.starts_with("SHOW COLUMNS"))
        .unwrap();
    assert_eq!(show, "SHOW COLUMNS IN `db`.`orders_view`");
}

/// Unload: `LIMIT 0` type probe, `UNLOAD … TO`, then `ListObjectsV2` on S3
/// (same mock, path-style) and presigned URLs.
#[tokio::test]
async fn unload_with_sql_lists_and_presigns_the_files() {
    let server = MockServer::start(|req| {
        if req.header("x-amz-target").is_none() {
            // S3 ListObjectsV2 (path style): GET /bucket?list-type=2&prefix=…
            assert!(req.target.starts_with("/exports/?"), "{}", req.target);
            assert!(req.target.contains("prefix=pre%2Fschema.table"), "{}", req.target);
            let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Name>exports</Name><Prefix>pre/schema.table</Prefix><KeyCount>2</KeyCount><MaxKeys>1000</MaxKeys><IsTruncated>false</IsTruncated><Contents><Key>pre/schema.table/a.gz</Key><Size>1</Size></Contents><Contents><Key>pre/schema.table/b.gz</Key><Size>1</Size></Contents></ListBucketResult>"#;
            return MockResponse {
                status: 200,
                headers: vec![("Content-Type".into(), "application/xml".into())],
                body: xml.as_bytes().to_vec(),
            };
        }
        let body = req.json();
        match target(req).as_str() {
            "StartQueryExecution" => {
                let q = body["QueryString"].as_str().unwrap_or_default();
                let id = if q.contains("UNLOAD") { "unload" } else { "probe" };
                MockResponse::json(200, JSON_11, json!({ "QueryExecutionId": id }))
            }
            "GetQueryExecution" => MockResponse::json(
                200,
                JSON_11,
                json!({ "QueryExecution": { "Status": { "State": "SUCCEEDED" } } }),
            ),
            "GetQueryResults" => MockResponse::json(
                200,
                JSON_11,
                result_page(&[("status", "varchar"), ("n", "integer")], &[], true, None),
            ),
            _ => MockResponse::json(200, JSON_11, json!({})),
        }
    })
    .await;
    let mut config = test_config(&server.endpoint);
    config.export_bucket = Some(normalize_s3_path("s3://exports/pre/"));
    config.export_bucket_csv_escape_symbol = Some("\\".to_string());
    let driver = AthenaDriver::new(config).unwrap();
    assert!(driver
        .is_unload_supported(&UnloadOptions::default())
        .await
        .unwrap());
    let data = driver
        .unload(
            "schema.table",
            &UnloadOptions {
                query: Some(UnloadQuery {
                    sql: "SELECT status, n FROM t WHERE status = ?".to_string(),
                    params: vec![json!("new")],
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        data.types,
        Some(vec![Column::new("status", "text"), Column::new("n", "int")])
    );
    assert!(data.csv_no_header);
    assert!(data.csv_disable_quoting);
    assert_eq!(data.csv_delimiter.as_deref(), Some("^A"));
    assert_eq!(data.export_bucket_csv_escape_symbol.as_deref(), Some("\\"));
    assert_eq!(data.csv_file.len(), 2);
    for (url, key) in data.csv_file.iter().zip(["a.gz", "b.gz"]) {
        assert!(
            url.starts_with(&format!(
                "{}/exports/pre/schema.table/{key}?",
                server.endpoint
            )),
            "{url}"
        );
        assert!(url.contains("X-Amz-Signature="), "{url}");
        assert!(url.contains("X-Amz-Expires=3600"), "{url}");
    }

    let queries: Vec<String> = server
        .requests()
        .iter()
        .filter(|r| target(r) == "StartQueryExecution")
        .map(|r| r.json()["QueryString"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        queries[0],
        "SELECT status, n FROM t WHERE status = 'new' LIMIT 0"
    );
    assert!(
        queries[1].contains("UNLOAD (SELECT status, n FROM t WHERE status = 'new')"),
        "{}",
        queries[1]
    );
    assert!(
        queries[1].contains("TO 's3://exports/pre/schema.table'"),
        "{}",
        queries[1]
    );
    assert!(queries[1].contains("format = 'TEXTFILE'"));
    assert!(queries[1].contains("compression='GZIP'"));
}

/// Presigned URLs validated by a real S3 implementation: LocalStack with
/// `S3_SKIP_SIGNATURE_VALIDATION=0`, Athena still mocked. Gated on
/// `DRV_AWS_TEST_S3_ENDPOINT` (e.g. `http://127.0.0.1:16600`).
#[tokio::test]
async fn unload_presigned_urls_against_localstack_s3() {
    let Ok(s3_endpoint) = std::env::var("DRV_AWS_TEST_S3_ENDPOINT") else {
        eprintln!("skipping: DRV_AWS_TEST_S3_ENDPOINT is not set");
        return;
    };
    // Seed the bucket as Athena's UNLOAD would have.
    let s3_config = aws_sdk_s3::config::Builder::new()
        .behavior_version(aws_config::BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new("test", "test", None, None, "test"))
        .endpoint_url(s3_endpoint.clone())
        .force_path_style(true)
        .build();
    let s3 = aws_sdk_s3::Client::from_conf(s3_config);
    let bucket = "cube-athena-exports";
    let _ = s3.create_bucket().bucket(bucket).send().await;
    let files = [
        ("pre/orders_abc/0001.gz", "new\u{1}300\n"),
        ("pre/orders_abc/0002.gz", "\\N\u{1}500\n"),
    ];
    for (key, body) in files {
        s3.put_object()
            .bucket(bucket)
            .key(key)
            .body(aws_sdk_s3::primitives::ByteStream::from(
                body.as_bytes().to_vec(),
            ))
            .send()
            .await
            .unwrap();
    }
    // Another table's files share the bucket but not the prefix.
    s3.put_object()
        .bucket(bucket)
        .key("pre/other/0001.gz")
        .body(aws_sdk_s3::primitives::ByteStream::from(b"x".to_vec()))
        .send()
        .await
        .unwrap();

    let (athena, _) = select_server().await;
    let mut config = test_config(&athena.endpoint);
    config.access_key_id = Some("test".to_string());
    config.secret_access_key = Some("test".to_string());
    config.s3_endpoint_url = Some(s3_endpoint);
    config.export_bucket = Some(normalize_s3_path(&format!("{bucket}/pre")));
    let driver = AthenaDriver::new(config).unwrap();
    let data = driver
        .unload(
            "orders_abc",
            &UnloadOptions {
                query: Some(UnloadQuery {
                    sql: "SELECT 1".to_string(),
                    params: vec![],
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(data.csv_file.len(), 2);
    for (url, (_, body)) in data.csv_file.iter().zip(files) {
        let response = reqwest::get(url).await.unwrap();
        assert_eq!(response.status(), 200, "{url}");
        assert_eq!(response.text().await.unwrap(), body);
    }
    // Signature validation is on: a tampered URL is refused.
    let tampered = data.csv_file[0].replace("X-Amz-Signature=", "X-Amz-Signature=0");
    assert_eq!(reqwest::get(&tampered).await.unwrap().status(), 403);
}
