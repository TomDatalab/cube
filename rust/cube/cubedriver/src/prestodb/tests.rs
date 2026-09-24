//! Unit tests of the Presto driver, including the ports of
//! `test/unit/params-escaping.test.ts` and `test/unit/headers.test.ts`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::TryStreamExt;
use serde_json::json;

use super::mock_http::{new_log, start};
use super::*;

fn config(host: &str, port: u16) -> PrestoConfig {
    let mut config = PrestoConfig::from_driver_config(DriverConfig::default(), Engine::Presto);
    config.host = host.to_string();
    config.port = port;
    config.catalog = Some("test".into());
    config.schema = Some("default".into());
    config.user = Some("cube".into());
    config
}

fn driver() -> PrestoDriver {
    PrestoDriver::new(config("localhost", 8080)).unwrap()
}

const LIKE: &str =
    r"SELECT * FROM orders WHERE LOWER(name) LIKE CONCAT('%', LOWER(?), '%') ESCAPE '\'";

// --- params-escaping.test.ts ---------------------------------------------

#[test]
fn preserves_like_escape_sequences_emitted_by_the_schema_compiler() {
    assert_eq!(
        driver().prepare_query_with_params(LIKE, &[json!(r"new\_order\%")]),
        r"SELECT * FROM orders WHERE LOWER(name) LIKE CONCAT('%', LOWER('new\_order\%'), '%') ESCAPE '\'"
    );
}

#[test]
fn does_not_double_literal_backslashes_in_like_parameters() {
    assert_eq!(
        driver().prepare_query_with_params(LIKE, &[json!(r"folder\\name")]),
        r"SELECT * FROM orders WHERE LOWER(name) LIKE CONCAT('%', LOWER('folder\\name'), '%') ESCAPE '\'"
    );
}

#[test]
fn doubles_quotes_so_a_like_value_cannot_break_out_of_the_literal() {
    assert_eq!(
        driver().prepare_query_with_params(LIKE, &[json!("o'reilly'); DROP TABLE orders; --")]),
        r"SELECT * FROM orders WHERE LOWER(name) LIKE CONCAT('%', LOWER('o''reilly''); DROP TABLE orders; --'), '%') ESCAPE '\'"
    );
}

#[test]
fn keeps_the_literal_closed_for_a_backslash_then_quote_payload() {
    assert_eq!(
        driver().prepare_query_with_params(
            "SELECT * FROM orders WHERE name = ?",
            &[json!(r"foo\' OR 1=1 --")]
        ),
        r"SELECT * FROM orders WHERE name = 'foo\'' OR 1=1 --'"
    );
}

#[test]
fn keeps_a_literal_percent_sign_in_an_equality_parameter_verbatim() {
    assert_eq!(
        driver().prepare_query_with_params(
            "SELECT * FROM orders WHERE discount_label = ?",
            &[json!("100% cotton")]
        ),
        "SELECT * FROM orders WHERE discount_label = '100% cotton'"
    );
}

#[test]
fn passes_an_unescaped_percent_sign_through_a_like_parameter_untouched() {
    assert_eq!(
        driver().prepare_query_with_params(LIKE, &[json!("50%")]),
        r"SELECT * FROM orders WHERE LOWER(name) LIKE CONCAT('%', LOWER('50%'), '%') ESCAPE '\'"
    );
}

#[test]
fn escapes_quotes_in_a_value_that_also_contains_percent_signs() {
    assert_eq!(
        driver().prepare_query_with_params(LIKE, &[json!("50%' OR 1=1 --")]),
        r"SELECT * FROM orders WHERE LOWER(name) LIKE CONCAT('%', LOWER('50%'' OR 1=1 --'), '%') ESCAPE '\'"
    );
}

#[test]
fn substitutes_multiple_placeholders_in_order() {
    assert_eq!(
        driver().prepare_query_with_params(
            &format!("{LIKE} AND status = ? AND amount > ?"),
            &[json!(r"pending\_review"), json!("new"), json!(100)]
        ),
        r"SELECT * FROM orders WHERE LOWER(name) LIKE CONCAT('%', LOWER('pending\_review'), '%') ESCAPE '\' AND status = 'new' AND amount > 100"
    );
}

// --- headers.test.ts -------------------------------------------------------

#[tokio::test]
async fn sends_custom_headers_on_every_request_including_next_uri_polls() {
    let log = new_log();
    // The poll goes to a different "worker" server, as `nextUri` may name
    // another host than the coordinator.
    let worker = start(log.clone(), |_| {
        (
            200,
            json!({
                "id": "q1",
                "infoUri": "http://coordinator.local/v1/query/q1",
                "stats": { "state": "FINISHED" },
                "columns": [{ "name": "one", "type": "integer" }],
                "data": [[1]],
            })
            .to_string(),
        )
    })
    .await;
    let worker_url = worker.url();
    let coordinator = start(log.clone(), move |_| {
        (
            200,
            json!({
                "id": "q1",
                "infoUri": "http://coordinator.local/v1/query/q1",
                "nextUri": format!("{worker_url}/v1/statement/q1/1"),
                "stats": { "state": "QUEUED" },
            })
            .to_string(),
        )
    })
    .await;

    let mut config = config("127.0.0.1", coordinator.port());
    config.check_interval = Duration::from_millis(1);
    config.headers = vec![
        ("X-Custom-Header".into(), "custom-value".into()),
        ("Proxy-Authorization".into(), "Basic dGVzdA==".into()),
    ];
    let driver = PrestoDriver::new(config).unwrap();

    let rows = driver
        .query("SELECT 1", &[], &QueryOptions::default())
        .await
        .unwrap();
    assert_eq!(
        rows.to_json_rows(),
        vec![json!({ "one": 1 }).as_object().unwrap().clone()]
    );
    assert_eq!(rows.columns, vec![Column::new("one", "int")]);

    let requests = log.lock().unwrap().clone();
    let post = requests.iter().find(|r| r.method == "POST").unwrap();
    let poll = requests.iter().find(|r| r.method == "GET").unwrap();

    assert_eq!(post.port, coordinator.port());
    assert_eq!(post.path, "/v1/statement");
    assert_eq!(post.body, "SELECT 1");
    assert_eq!(post.header("X-Custom-Header"), Some("custom-value"));
    assert_eq!(post.header("Proxy-Authorization"), Some("Basic dGVzdA=="));
    assert_eq!(post.header("X-Presto-Catalog"), Some("test"));
    assert_eq!(post.header("X-Presto-Schema"), Some("default"));
    assert_eq!(post.header("X-Presto-User"), Some("cube"));
    assert_eq!(post.header("X-Presto-Source"), Some("nodejs-client"));
    assert_eq!(post.header("X-Presto-Session"), None);

    // The poll follows the nextUri host *and* carries the custom headers.
    assert_eq!(poll.port, worker.port());
    assert_eq!(poll.path, "/v1/statement/q1/1");
    assert_eq!(poll.header("X-Custom-Header"), Some("custom-value"));
    assert_eq!(poll.header("Proxy-Authorization"), Some("Basic dGVzdA=="));
    assert_eq!(poll.header("X-Presto-User"), Some("cube"));
}

// --- protocol ----------------------------------------------------------------

/// A coordinator that serves `pages` (one per poll) after the POST.
async fn paged_server(pages: Vec<serde_json::Value>) -> super::mock_http::MockServer {
    let counter = Arc::new(AtomicUsize::new(0));
    let pages = Arc::new(pages);
    let log = new_log();
    start(log, move |r| {
        let port = r.port;
        if r.method == "DELETE" {
            return (204, String::new());
        }
        let i = if r.method == "POST" {
            0
        } else {
            counter.fetch_add(1, Ordering::SeqCst) + 1
        };
        let mut page = if i == 0 {
            json!({ "stats": { "state": "QUEUED" } })
        } else {
            pages[i - 1].clone()
        };
        let obj = page.as_object_mut().unwrap();
        obj.insert("id".into(), json!("q1"));
        obj.insert(
            "infoUri".into(),
            json!(format!("http://127.0.0.1:{port}/v1/query/q1")),
        );
        if i < pages.len() && !obj.contains_key("error") {
            obj.insert(
                "nextUri".into(),
                json!(format!("http://127.0.0.1:{port}/v1/statement/q1/{}", i + 1)),
            );
        }
        (200, page.to_string())
    })
    .await
}

fn fast_driver(port: u16) -> PrestoDriver {
    let mut config = config("127.0.0.1", port);
    config.check_interval = Duration::from_millis(1);
    PrestoDriver::new(config).unwrap()
}

#[tokio::test]
async fn pages_are_concatenated_in_server_order() {
    let columns = json!([{ "name": "n", "type": "bigint" }, { "name": "s", "type": "varchar(3)" }]);
    let server = paged_server(vec![
        json!({ "stats": { "state": "RUNNING" } }),
        json!({ "stats": { "state": "RUNNING" }, "columns": columns, "data": [[1, "a"], [2, "b"]] }),
        json!({ "stats": { "state": "RUNNING" }, "columns": columns, "data": [[3, "c"]] }),
        json!({ "stats": { "state": "FINISHED" }, "columns": columns }),
    ])
    .await;
    let driver = fast_driver(server.port());
    let result = driver
        .query(
            "SELECT n, s FROM t ORDER BY n",
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        result.rows,
        vec![
            vec![json!(1), json!("a")],
            vec![json!(2), json!("b")],
            vec![json!(3), json!("c")]
        ]
    );
    assert_eq!(
        result.columns,
        vec![Column::new("n", "bigint"), Column::new("s", "varchar(3)")]
    );
}

#[tokio::test]
async fn stream_resolves_with_columns_and_streams_every_page() {
    let columns = json!([{ "name": "n", "type": "integer" }]);
    let server = paged_server(vec![
        json!({ "stats": { "state": "RUNNING" }, "columns": columns, "data": [[1]] }),
        json!({ "stats": { "state": "RUNNING" }, "columns": columns, "data": [[2], [3]] }),
        json!({ "stats": { "state": "FINISHED" }, "columns": columns }),
    ])
    .await;
    let driver = fast_driver(server.port());
    let stream = driver
        .stream("SELECT n FROM t", &[], &StreamOptions::default())
        .await
        .unwrap();
    assert_eq!(stream.columns, vec![Column::new("n", "int")]);
    let rows: Vec<Row> = stream.rows.try_collect().await.unwrap();
    assert_eq!(rows, vec![vec![json!(1)], vec![json!(2)], vec![json!(3)]]);

    // Streaming sets the session run-time limit (600 s by default).
    let post = server
        .requests()
        .into_iter()
        .find(|r| r.method == "POST")
        .unwrap();
    assert_eq!(
        post.header("X-Presto-Session"),
        Some("query_max_run_time=600s")
    );
}

#[tokio::test]
async fn query_errors_carry_the_server_message() {
    let server = paged_server(vec![json!({
        "stats": { "state": "FAILED" },
        "error": { "message": "line 1:8: Column 'x' cannot be resolved", "errorName": "COLUMN_NOT_FOUND" }
    })])
    .await;
    let driver = fast_driver(server.port());
    let err = driver
        .query("SELECT x", &[], &QueryOptions::default())
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "line 1:8: Column 'x' cannot be resolved");
    assert!(matches!(
        err,
        DriverError::Database { code: Some(ref c), .. } if c == "COLUMN_NOT_FOUND"
    ));
}

#[tokio::test]
async fn unavailable_responses_are_retried() {
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let server = start(new_log(), move |r| {
        let n = h.fetch_add(1, Ordering::SeqCst);
        if n < 2 {
            return (503, String::new());
        }
        let port = r.port;
        if r.method == "POST" {
            (
                200,
                json!({ "id": "q", "infoUri": "x", "nextUri": format!("http://127.0.0.1:{port}/v1/statement/q/1"), "stats": { "state": "QUEUED" } }).to_string(),
            )
        } else {
            (
                200,
                json!({ "id": "q", "infoUri": "x", "stats": { "state": "FINISHED" }, "columns": [{"name": "a", "type": "boolean"}], "data": [[true]] }).to_string(),
            )
        }
    })
    .await;
    let driver = fast_driver(server.port());
    let result = driver
        .query("SELECT true", &[], &QueryOptions::default())
        .await
        .unwrap();
    assert_eq!(result.rows, vec![vec![json!(true)]]);
    assert_eq!(hits.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn http_errors_and_malformed_responses() {
    let server = start(new_log(), |_| (400, "Bad statement".to_string())).await;
    let err = fast_driver(server.port())
        .query("SELECT", &[], &QueryOptions::default())
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "execution error:Bad statement");

    let server = start(new_log(), |_| (200, "not json".to_string())).await;
    let err = fast_driver(server.port())
        .query("SELECT", &[], &QueryOptions::default())
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "execution error:could not parse response");

    let server = start(new_log(), |_| (200, json!({ "id": "q" }).to_string())).await;
    let err = fast_driver(server.port())
        .query("SELECT", &[], &QueryOptions::default())
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "nextUri missing in response for POST /v1/statement"
    );
}

#[tokio::test]
async fn the_query_timeout_cancels_the_query() {
    let server = start(new_log(), |r| {
        let port = r.port;
        if r.method == "DELETE" {
            return (204, String::new());
        }
        (
            200,
            json!({ "id": "q", "infoUri": "x", "nextUri": format!("http://127.0.0.1:{port}/v1/statement/q/1"), "stats": { "state": "QUEUED" } }).to_string(),
        )
    })
    .await;
    let mut config = config("127.0.0.1", server.port());
    config.check_interval = Duration::from_millis(20);
    config.query_timeout = Duration::from_millis(100);
    let driver = PrestoDriver::new(config).unwrap();
    let err = driver
        .query("SELECT sleep()", &[], &QueryOptions::default())
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "execution error:query timed out");
    let deletes: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|r| r.method == "DELETE")
        .collect();
    assert_eq!(deletes.len(), 1);
    assert_eq!(deletes[0].path, "/v1/statement/q/1");
}

#[tokio::test]
async fn catalog_is_required() {
    let mut config = config("127.0.0.1", 1);
    config.catalog = None;
    let err = PrestoDriver::new(config)
        .unwrap()
        .query("SELECT 1", &[], &QueryOptions::default())
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Catalog not specified; catalog is required if schema is specified"
    );
}

#[tokio::test]
async fn test_connection_lists_nodes_or_selects() {
    let server = start(new_log(), |r| match r.path.as_str() {
        "/v1/node" => (200, "[]".to_string()),
        _ => (500, String::new()),
    })
    .await;
    let driver = fast_driver(server.port());
    driver.test_connection().await.unwrap();
    assert_eq!(server.requests()[0].method, "GET");
    assert_eq!(server.requests()[0].header("X-Presto-User"), Some("cube"));

    let server = start(new_log(), |_| (401, "Unauthorized".to_string())).await;
    let err = fast_driver(server.port())
        .test_connection()
        .await
        .unwrap_err();
    assert!(err
        .to_string()
        .contains("node list api returns error:Unauthorized"));

    // useSelectTestConnection runs `SELECT 1` instead.
    let server = paged_server(vec![json!({
        "stats": { "state": "FINISHED" }, "columns": [{"name": "_col0", "type": "integer"}], "data": [[1]]
    })])
    .await;
    let mut config = config("127.0.0.1", server.port());
    config.check_interval = Duration::from_millis(1);
    config.use_select_test_connection = true;
    PrestoDriver::new(config)
        .unwrap()
        .test_connection()
        .await
        .unwrap();
    let post = server
        .requests()
        .into_iter()
        .find(|r| r.method == "POST")
        .unwrap();
    assert_eq!(post.body, "SELECT 1");
}

// --- configuration -----------------------------------------------------------

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn from_env(pairs: &[(&str, &str)]) -> Result<PrestoConfig> {
    let env = env(pairs);
    let driver = DriverConfig::from_env_source(&env, None, false)?;
    let mut config = PrestoConfig::from_driver_config(driver, Engine::Presto);
    config.apply_env_source(&env)?;
    Ok(config)
}

#[test]
fn env_configuration() {
    let config = from_env(&[
        ("CUBEJS_DB_HOST", "presto.local"),
        ("CUBEJS_DB_PORT", "8443"),
        ("CUBEJS_DB_PRESTO_CATALOG", "hive"),
        ("CUBEJS_DB_CATALOG", "ignored"),
        ("CUBEJS_DB_SCHEMA", "sf1"),
        ("CUBEJS_DB_USER", "u"),
        ("CUBEJS_DB_PASS", "p"),
        ("CUBEJS_DB_SSL", "true"),
        ("CUBEJS_DB_QUERY_TIMEOUT", "5m"),
        ("CUBEJS_DB_USE_SELECT_TEST_CONNECTION", "true"),
    ])
    .unwrap();
    assert_eq!(config.host, "presto.local");
    assert_eq!(config.port, 8443);
    assert_eq!(config.catalog.as_deref(), Some("hive"));
    assert_eq!(config.schema.as_deref(), Some("sf1"));
    assert!(config.ssl.is_some());
    assert_eq!(config.query_timeout, Duration::from_secs(300));
    assert!(config.use_select_test_connection);
    assert_eq!(
        config.authorization().unwrap().as_deref(),
        Some("Basic dTpw")
    );
    let driver = PrestoDriver::new(config).unwrap();
    assert_eq!(driver.client().base_url(), "https://presto.local:8443");

    // Defaults; CUBEJS_DB_NAME wins over CUBEJS_DB_SCHEMA; deprecated catalog.
    let config = from_env(&[
        ("CUBEJS_DB_NAME", "a"),
        ("CUBEJS_DB_SCHEMA", "b"),
        ("CUBEJS_DB_CATALOG", "tpch"),
        ("CUBEJS_DB_PRESTO_AUTH_TOKEN", "tok"),
    ])
    .unwrap();
    assert_eq!(config.host, "localhost");
    assert_eq!(config.port, 8080);
    assert_eq!(config.schema.as_deref(), Some("a"));
    assert_eq!(config.catalog.as_deref(), Some("tpch"));
    assert_eq!(config.query_timeout, Duration::from_secs(600));
    assert_eq!(
        config.authorization().unwrap().as_deref(),
        Some("Bearer tok")
    );
    assert!(config.export_bucket.export_bucket.is_none());
}

#[test]
fn env_configuration_errors() {
    let config = from_env(&[
        ("CUBEJS_DB_PASS", "p"),
        ("CUBEJS_DB_PRESTO_AUTH_TOKEN", "tok"),
    ])
    .unwrap();
    assert_eq!(
        PrestoDriver::new(config).unwrap_err().to_string(),
        "Both user/password and auth token are set. Please remove password or token."
    );

    let err = from_env(&[("CUBEJS_DB_EXPORT_BUCKET_TYPE", "azure")]).unwrap_err();
    assert_eq!(
        err.to_string(),
        "The CUBEJS_DB_EXPORT_BUCKET_TYPE must be one of the [gcs, s3]."
    );

    let err = from_env(&[("CUBEJS_DB_USE_SELECT_TEST_CONNECTION", "yes")]).unwrap_err();
    assert_eq!(
        err.to_string(),
        "The CUBEJS_DB_USE_SELECT_TEST_CONNECTION must be either 'true' or 'false'."
    );

    let mut config = self::config("h", 1);
    config.ssl = Some(SslConfig {
        passphrase: Some("x".into()),
        ..Default::default()
    });
    assert!(matches!(
        PrestoDriver::new(config).unwrap_err(),
        DriverError::NotImplemented(_)
    ));
}

#[test]
fn data_source_specific_env() {
    let env = env(&[
        ("CUBEJS_DATASOURCES", "default,lake"),
        ("CUBEJS_DS_LAKE_DB_HOST", "lake"),
        ("CUBEJS_DS_LAKE_DB_PRESTO_CATALOG", "iceberg"),
        ("CUBEJS_DS_LAKE_DB_EXPORT_BUCKET_TYPE", "s3"),
        ("CUBEJS_DS_LAKE_DB_EXPORT_BUCKET", "bkt"),
        ("CUBEJS_DB_PRESTO_CATALOG", "hive"),
    ]);
    let driver = DriverConfig::from_env_source(&env, Some("lake"), false).unwrap();
    let mut config = PrestoConfig::from_driver_config(driver, Engine::Presto);
    config.apply_env_source(&env).unwrap();
    assert_eq!(config.host, "lake");
    assert_eq!(config.catalog.as_deref(), Some("iceberg"));
    assert_eq!(config.export_bucket.bucket_type.as_deref(), Some("s3"));
    assert_eq!(config.export_bucket.export_bucket.as_deref(), Some("bkt"));
}

// --- SQL -----------------------------------------------------------------------

#[test]
fn information_schema_queries() {
    let mut config = config("h", 1);
    config.schema = Some("sf1".into());
    config.catalog = Some("tpch".into());
    let driver = PrestoDriver::new(config).unwrap();
    let q = driver.information_schema_query();
    assert!(q.contains("columns.table_schema = 'sf1'"));
    assert!(q.contains("FROM tpch.information_schema.columns"));
    assert!(q.contains("columns.column_name as \"column_name\""));
    assert!(driver
        .get_schemas_query()
        .contains("FROM tpch.information_schema.tables\n"));
    assert!(driver
        .get_tables_for_specific_schemas_query("?, ?")
        .contains(
            "FROM tpch.information_schema.tables as columns\n      WHERE table_schema IN (?, ?)"
        ));
    assert!(driver
        .get_columns_for_specific_tables_query("x = ?")
        .contains("columns.table_schema as \"schema_name\""));

    let mut config = self::config("h", 1);
    config.schema = None;
    config.catalog = None;
    let driver = PrestoDriver::new(config).unwrap();
    let q = driver.information_schema_query();
    assert!(q.contains("FROM information_schema.columns"));
    assert!(!q.contains("AND columns.table_schema ="));
}

#[test]
fn driver_contract() {
    let driver = driver();
    assert_eq!(driver.param(3), "?");
    assert_eq!(driver.quote_identifier("a"), "\"a\"");
    assert!(!driver.read_only());
    assert!(driver.capabilities().unload_without_temp_table);
    assert_eq!(
        driver.wrap_query_with_limit("SELECT 1", 10),
        "SELECT * FROM (SELECT 1) AS t LIMIT 10"
    );
    assert_eq!(DEFAULT_CONCURRENCY, 2);
}

#[tokio::test]
async fn unload_sql_and_errors() {
    let mut config = config("h", 1);
    config.catalog = Some("hive".into());
    let driver = PrestoDriver::new(config.clone()).unwrap();
    assert!(!driver
        .is_unload_supported(&UnloadOptions::default())
        .await
        .unwrap());
    assert_eq!(
        driver
            .unload("s.t", &UnloadOptions::default())
            .await
            .unwrap_err()
            .to_string(),
        "Export bucket is not configured."
    );

    config.export_bucket.export_bucket = Some("bucket".into());
    let driver = PrestoDriver::new(config.clone()).unwrap();
    assert!(driver
        .is_unload_supported(&UnloadOptions::default())
        .await
        .unwrap());
    assert_eq!(
        driver
            .unload("s.t", &UnloadOptions::default())
            .await
            .unwrap_err()
            .to_string(),
        "Unsupported export bucket type: undefined"
    );

    config.export_bucket.bucket_type = Some("s3".into());
    let driver = PrestoDriver::new(config.clone()).unwrap();
    let types = vec![Column::new("id", "int"), Column::new("name", "text")];
    let (create, drop) = driver
        .unload_create_table_sql("s.t", &types, "SELECT * FROM x")
        .unwrap();
    assert_eq!(
        create,
        "CREATE TABLE hive.s.t WITH ( external_location = 's3://bucket/s/t', format = 'CSV') AS (SELECT CAST(id AS varchar) id, CAST(name AS varchar) name FROM (SELECT * FROM x))"
    );
    assert_eq!(drop, "DROP TABLE IF EXISTS hive.s.t");

    config.export_bucket.s3_advanced_fs = true;
    let driver = PrestoDriver::new(config.clone()).unwrap();
    assert_eq!(
        driver.external_location("s", "t").unwrap(),
        "s3a://bucket/s/t"
    );
    config.export_bucket.bucket_type = Some("gcs".into());
    let driver = PrestoDriver::new(config).unwrap();
    assert_eq!(
        driver.external_location("s", "t").unwrap(),
        "gs://bucket/s/t"
    );
}

#[test]
fn table_full_name_split() {
    assert_eq!(
        split_table_full_name("a.b.c"),
        ("a".to_string(), "b".to_string())
    );
}
