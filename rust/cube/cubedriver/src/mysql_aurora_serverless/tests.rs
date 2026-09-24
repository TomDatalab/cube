//! Unit tests (ported from `test/AuroraServerlessMySqlDriver.test.js` plus
//! the `data-api-client` behaviours the driver relies on) and protocol tests
//! against a local mock of the Data API's restJson1 endpoint.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aws_sdk_rdsdata::primitives::Blob;
use aws_sdk_rdsdata::types::{Field, TypeHint};
use serde_json::{json, Value};

use super::data_api::*;
use super::*;
use crate::aws_test_server::{MockRequest, MockResponse, MockServer};

const DUMMY_SECRET_ARN: &str = "arn:aws:secretsmanager:us-east-1:123456789012:secret:dummy";
const DUMMY_RESOURCE_ARN: &str = "arn:aws:rds:us-east-1:123456789012:cluster:dummy";

fn config(endpoint: Option<&str>) -> AuroraServerlessMySqlConfig {
    let mut config = AuroraServerlessMySqlConfig::from_driver_config(DriverConfig::default());
    config.secret_arn = Some(DUMMY_SECRET_ARN.to_string());
    config.resource_arn = Some(DUMMY_RESOURCE_ARN.to_string());
    config.database = Some("mysql".to_string());
    config.region = Some("us-east-1".to_string());
    config.credentials = Some(("awstest".to_string(), "awstest".to_string()));
    config.endpoint_url = endpoint.map(str::to_string);
    config.retry.delay_scale = 0.001;
    config
}

fn driver() -> AuroraServerlessMySqlDriver {
    AuroraServerlessMySqlDriver::new(config(None)).unwrap()
}

// ---------------------------------------------------------------------------
// AuroraServerlessMySqlDriver.test.js
// ---------------------------------------------------------------------------

#[test]
fn quote_identifier() {
    assert_eq!(driver().quote_identifier("test"), "`test`");
}

#[test]
fn position_bindings_test() {
    assert_eq!(
        position_bindings("select * from something where val = ?"),
        "select * from something where val = :b0"
    );
    assert_eq!(
        position_bindings(r"select ? , '\?', ?"),
        "select :b0 , '?', :b1"
    );
}

// ---------------------------------------------------------------------------
// Driver behaviour
// ---------------------------------------------------------------------------

#[test]
fn requires_the_arns() {
    let mut c = config(None);
    c.secret_arn = None;
    assert_eq!(
        AuroraServerlessMySqlDriver::new(c).unwrap_err().to_string(),
        "'secretArn' string value required (set CUBEJS_DATABASE_SECRET_ARN)"
    );
    let mut c = config(None);
    c.resource_arn = None;
    assert!(AuroraServerlessMySqlDriver::new(c)
        .unwrap_err()
        .to_string()
        .starts_with("'resourceArn' string value required"));
}

#[test]
fn env_configuration() {
    let env: HashMap<String, String> = [
        ("CUBEJS_DATABASE_SECRET_ARN", DUMMY_SECRET_ARN),
        ("CUBEJS_DATABASE_CLUSTER_ARN", DUMMY_RESOURCE_ARN),
        ("CUBEJS_DATABASE", "legacy_db"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let mut c = AuroraServerlessMySqlConfig::from_driver_config(
        DriverConfig::from_env_source(&env, None, false).unwrap(),
    );
    c.apply_env_source(&env).unwrap();
    assert_eq!(c.secret_arn.as_deref(), Some(DUMMY_SECRET_ARN));
    assert_eq!(c.resource_arn.as_deref(), Some(DUMMY_RESOURCE_ARN));
    assert_eq!(c.database.as_deref(), Some("legacy_db"));

    // CUBEJS_DB_NAME wins over CUBEJS_DATABASE; data-source prefixes apply.
    let env: HashMap<String, String> = [
        ("CUBEJS_DATASOURCES", "default,aurora"),
        ("CUBEJS_DS_AURORA_DATABASE_SECRET_ARN", "s"),
        ("CUBEJS_DS_AURORA_DATABASE_CLUSTER_ARN", "r"),
        ("CUBEJS_DS_AURORA_DB_NAME", "db"),
        ("CUBEJS_DS_AURORA_DATABASE", "legacy"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let mut c = AuroraServerlessMySqlConfig::from_driver_config(
        DriverConfig::from_env_source(&env, Some("aurora"), false).unwrap(),
    );
    c.apply_env_source(&env).unwrap();
    assert_eq!(c.secret_arn.as_deref(), Some("s"));
    assert_eq!(c.resource_arn.as_deref(), Some("r"));
    assert_eq!(c.database.as_deref(), Some("db"));
}

#[test]
fn sql_dialect() {
    let d = driver();
    assert_eq!(d.param(3), "?");
    assert_eq!(
        d.from_generic_type(&GenericType::String),
        "varchar(255) CHARACTER SET utf8mb4"
    );
    assert_eq!(
        d.from_generic_type(&GenericType::Text),
        "varchar(255) CHARACTER SET utf8mb4"
    );
    assert_eq!(d.from_generic_type(&GenericType::Boolean), "boolean");
    assert!(d
        .information_schema_query()
        .ends_with(" AND columns.table_schema = 'mysql'"));
    assert_eq!(
        d.create_table_sql("t.x", &[Column::new("a", "string")]),
        "CREATE TABLE t.x (`a` varchar(255) CHARACTER SET utf8mb4)"
    );
    assert!(!d.read_only());
    assert_eq!(DEFAULT_CONCURRENCY, 2);
}

#[test]
fn to_column_value() {
    let d = driver();
    assert_eq!(
        d.to_column_value(&json!("2020-01-01T00:00:00.000Z"), &GenericType::Timestamp),
        json!("2020-01-01T00:00:00.000")
    );
    assert_eq!(
        d.to_column_value(&json!("TRUE"), &GenericType::Boolean),
        json!(true)
    );
    assert_eq!(
        d.to_column_value(&json!("false"), &GenericType::Boolean),
        json!(false)
    );
    assert_eq!(
        d.to_column_value(&json!("yes"), &GenericType::Boolean),
        json!("yes")
    );
    assert_eq!(
        d.to_column_value(&json!(true), &GenericType::Boolean),
        json!(true)
    );
    assert_eq!(
        d.to_column_value(&json!("Z"), &GenericType::Text),
        json!("Z")
    );
}

#[test]
fn create_table_as_rewrite() {
    assert_eq!(
        create_table_as_to_insert("CREATE TABLE s.t AS SELECT 1"),
        "INSERT INTO s.t SELECT 1"
    );
    assert_eq!(
        create_table_as_to_insert("create table s.t as SELECT 1"),
        "INSERT INTO s.t SELECT 1"
    );
    assert_eq!(create_table_as_to_insert("SELECT 1"), "SELECT 1");
}

// ---------------------------------------------------------------------------
// data-api-client
// ---------------------------------------------------------------------------

#[test]
fn sql_params_detection() {
    let p = sql_params("SELECT :b0, :b1::jsonb, ::ident, '10:30'");
    assert_eq!(p.get("b0"), Some(&SqlParamKind::Placeholder));
    assert_eq!(p.get("b1"), Some(&SqlParamKind::Placeholder));
    // `::jsonb` right after `:b1` is a cast, not an identifier placeholder.
    assert_eq!(p.get("jsonb"), None);
    assert_eq!(p.get("ident"), Some(&SqlParamKind::Identifier));
    assert_eq!(p.get("30"), Some(&SqlParamKind::Placeholder));
}

#[test]
fn parameter_types() {
    let p = |v: Value| format_param("b0", &v).unwrap();
    assert_eq!(p(json!("x")).value(), Some(&Field::StringValue("x".into())));
    assert_eq!(p(json!(true)).value(), Some(&Field::BooleanValue(true)));
    assert_eq!(p(json!(5)).value(), Some(&Field::LongValue(5)));
    assert_eq!(p(json!(5.0)).value(), Some(&Field::LongValue(5)));
    assert_eq!(p(json!(1.5)).value(), Some(&Field::DoubleValue(1.5)));
    assert_eq!(p(json!(1e21)).value(), Some(&Field::DoubleValue(1e21)));
    assert_eq!(p(Value::Null).value(), Some(&Field::IsNull(true)));
    assert_eq!(
        p(json!({ "longValue": 7 })).value(),
        Some(&Field::LongValue(7))
    );
    let obj = p(json!({ "a": 1 }));
    assert_eq!(obj.value(), Some(&Field::StringValue(r#"{"a":1}"#.into())));
    assert_eq!(obj.type_hint(), Some(&TypeHint::Json));
    assert_eq!(
        format_param("b0", &json!(["a"])).unwrap_err().to_string(),
        "'b0' is an invalid type"
    );
}

#[test]
fn build_statement_binds_only_used_placeholders() {
    let (sql, params) = build_statement("SELECT ? AS a", &[json!(1), json!(2)]).unwrap();
    assert_eq!(sql, "SELECT :b0 AS a");
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].name(), Some("b0"));
    // engine `pg` casts plain object parameters to jsonb, as in Node.
    let (sql, _) = build_statement("SELECT ?", &[json!({ "a": 1 })]).unwrap();
    assert_eq!(sql, "SELECT :b0::jsonb");
}

#[test]
fn record_values() {
    let f = |field: Field, t: &str| format_record_value(&field, Some(t), false).unwrap();
    assert_eq!(f(Field::IsNull(true), "VARCHAR"), Value::Null);
    assert_eq!(f(Field::StringValue("a".into()), "VARCHAR"), json!("a"));
    assert_eq!(f(Field::LongValue(3), "BIGINT"), json!(3));
    assert_eq!(f(Field::DoubleValue(1.5), "DOUBLE"), json!(1.5));
    assert_eq!(f(Field::BooleanValue(true), "BIT"), json!(true));
    assert_eq!(
        f(Field::StringValue("12.50".into()), "DECIMAL"),
        json!("12.50")
    );
    // deserializeDate: UTC, rendered like JSON.stringify(Date).
    assert_eq!(
        f(Field::StringValue("2020-01-02 03:04:05".into()), "DATETIME"),
        json!("2020-01-02T03:04:05.000Z")
    );
    assert_eq!(
        f(
            Field::StringValue("2020-01-02 03:04:05.123456".into()),
            "TIMESTAMP"
        ),
        json!("2020-01-02T03:04:05.123Z")
    );
    assert_eq!(
        f(Field::StringValue("2020-01-02".into()), "date"),
        json!("2020-01-02T00:00:00.000Z")
    );
    assert_eq!(f(Field::StringValue("garbage".into()), "DATE"), Value::Null);
    assert_eq!(f(Field::StringValue("2021".into()), "YEAR"), json!(2021));
    assert_eq!(
        f(Field::StringValue(r#"{"a":[1]}"#.into()), "JSON"),
        json!({ "a": [1] })
    );
    assert_eq!(
        f(Field::BlobValue(Blob::new(b"hi".to_vec())), "BLOB"),
        json!("aGk=")
    );
    assert_eq!(
        format_record_value(
            &Field::BlobValue(Blob::new(b"int(11)".to_vec())),
            Some("BLOB"),
            true
        )
        .unwrap(),
        json!("int(11)")
    );
}

#[test]
fn retry_policy() {
    let o = RetryOptions::default();
    // Resuming cluster: up to 9 retries with the long schedule.
    assert_eq!(
        retry_delay(&o, 0, "DatabaseResumingException", ""),
        Some(Duration::from_secs(2))
    );
    assert_eq!(
        retry_delay(&o, 8, "", "Database is resuming after being auto-paused"),
        Some(Duration::from_secs(40))
    );
    assert_eq!(retry_delay(&o, 9, "DatabaseResumingException", ""), None);
    // Connection-ish errors (including BadRequestException): 2 retries.
    assert_eq!(
        retry_delay(&o, 0, "BadRequestException", "syntax"),
        Some(Duration::from_secs(2))
    );
    assert_eq!(
        retry_delay(&o, 1, "", "Communications link failure"),
        Some(Duration::from_secs(4))
    );
    assert_eq!(retry_delay(&o, 2, "BadRequestException", ""), None);
    assert_eq!(retry_delay(&o, 0, "ForbiddenException", "denied"), None);
    let disabled = RetryOptions {
        enabled: false,
        ..Default::default()
    };
    assert_eq!(
        retry_delay(&disabled, 0, "DatabaseResumingException", ""),
        None
    );
}

// ---------------------------------------------------------------------------
// Mock Data API
// ---------------------------------------------------------------------------

fn ok(body: Value) -> MockResponse {
    MockResponse::json(200, "application/json", body)
}

fn bad_request(message: &str) -> MockResponse {
    MockResponse::json(400, "application/json", json!({ "message": message }))
        .with_header("x-amzn-ErrorType", "BadRequestException")
}

fn string_field(v: &str) -> Value {
    json!({ "stringValue": v })
}

type Handler = Box<dyn Fn(&MockRequest) -> MockResponse + Send + Sync>;

async fn server(handler: Handler) -> MockServer {
    MockServer::start(move |req| handler(req)).await
}

fn path(req: &MockRequest) -> &str {
    req.target.as_str()
}

#[tokio::test]
async fn query_sends_named_parameters_and_hydrates_records() {
    let server = server(Box::new(|req| {
        assert_eq!(req.method, "POST");
        assert_eq!(path(req), "/Execute");
        ok(json!({
            "columnMetadata": [
                { "label": "status", "name": "status", "typeName": "VARCHAR" },
                { "label": "n", "typeName": "BIGINT" },
                { "label": "created", "typeName": "DATETIME" },
                { "label": "flag", "typeName": "BIT" }
            ],
            "records": [
                [ string_field("new"), { "longValue": 3 }, string_field("2020-01-01 00:00:00"), { "booleanValue": true } ],
                [ { "isNull": true }, { "longValue": 4 }, { "isNull": true }, { "booleanValue": false } ]
            ],
            "numberOfRecordsUpdated": 0
        }))
    }))
    .await;
    let driver = AuroraServerlessMySqlDriver::new(config(Some(&server.endpoint))).unwrap();
    let result = driver
        .query(
            "SELECT * FROM t WHERE status = ? AND n > ? AND ok = ?",
            &[json!("new"), json!(1), json!(true)],
            &QueryOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        result
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        vec!["status", "n", "created", "flag"]
    );
    assert_eq!(
        result.rows,
        vec![
            vec![
                json!("new"),
                json!(3),
                json!("2020-01-01T00:00:00.000Z"),
                json!(true)
            ],
            vec![Value::Null, json!(4), Value::Null, json!(false)],
        ]
    );

    let body = server.requests()[0].json();
    assert_eq!(
        body["sql"],
        "SELECT * FROM t WHERE status = :b0 AND n > :b1 AND ok = :b2"
    );
    assert_eq!(body["secretArn"], DUMMY_SECRET_ARN);
    assert_eq!(body["resourceArn"], DUMMY_RESOURCE_ARN);
    assert_eq!(body["database"], "mysql");
    assert_eq!(body["includeResultMetadata"], true);
    assert_eq!(
        body["parameters"],
        json!([
            { "name": "b0", "value": { "stringValue": "new" } },
            { "name": "b1", "value": { "longValue": 1 } },
            { "name": "b2", "value": { "booleanValue": true } }
        ])
    );
    let auth = server.requests()[0]
        .header("authorization")
        .unwrap()
        .to_string();
    assert!(auth.contains("/us-east-1/rds-data/aws4_request"), "{auth}");
}

#[tokio::test]
async fn statements_without_records_return_no_rows() {
    let server = server(Box::new(|_| {
        ok(json!({ "numberOfRecordsUpdated": 2, "generatedFields": [] }))
    }))
    .await;
    let driver = AuroraServerlessMySqlDriver::new(config(Some(&server.endpoint))).unwrap();
    let result = driver
        .query("DELETE FROM t", &[], &QueryOptions::default())
        .await
        .unwrap();
    assert!(result.rows.is_empty());
    assert!(server.requests()[0].json().get("parameters").is_none());
}

#[tokio::test]
async fn sql_errors_are_retried_like_data_api_client_then_raised() {
    let server = server(Box::new(|_| {
        bad_request("You have an error in your SQL syntax")
    }))
    .await;
    let driver = AuroraServerlessMySqlDriver::new(config(Some(&server.endpoint))).unwrap();
    let err = driver
        .query("SELEC 1", &[], &QueryOptions::default())
        .await
        .unwrap_err();
    match err {
        DriverError::Database { message, code } => {
            assert_eq!(message, "You have an error in your SQL syntax");
            assert_eq!(code.as_deref(), Some("BadRequestException"));
        }
        other => panic!("unexpected {other:?}"),
    }
    // 1 attempt + 2 connection-schedule retries (the SDK's own retries do not
    // apply to a 400).
    assert_eq!(server.requests().len(), 3);
}

#[tokio::test]
async fn resuming_cluster_is_waited_for() {
    let calls = Arc::new(Mutex::new(0));
    let calls2 = calls.clone();
    let server = server(Box::new(move |_| {
        let mut n = calls2.lock().unwrap();
        *n += 1;
        if *n <= 3 {
            MockResponse::json(400, "application/json", json!({ "message": "Database is resuming" }))
                .with_header("x-amzn-ErrorType", "DatabaseResumingException")
        } else {
            ok(json!({ "columnMetadata": [{ "label": "1", "typeName": "BIGINT" }], "records": [[{ "longValue": 1 }]] }))
        }
    }))
    .await;
    let driver = AuroraServerlessMySqlDriver::new(config(Some(&server.endpoint))).unwrap();
    driver.test_connection().await.unwrap();
    assert_eq!(*calls.lock().unwrap(), 4);
}

#[tokio::test]
async fn download_query_results_describes_a_temporary_table() {
    let server = server(Box::new(|req| {
        let body = req.json();
        match path(req) {
            "/BeginTransaction" => ok(json!({ "transactionId": "tx-1" })),
            "/CommitTransaction" => ok(json!({ "transactionStatus": "Transaction Committed" })),
            "/Execute" => {
                let sql = body["sql"].as_str().unwrap_or_default();
                if sql.starts_with("DESCRIBE") {
                    assert_eq!(body["transactionId"], "tx-1");
                    ok(json!({
                        "columnMetadata": [
                            { "label": "Field", "typeName": "VARCHAR" },
                            { "label": "Type", "typeName": "BLOB" }
                        ],
                        "records": [
                            [ string_field("amount"), { "blobValue": "aW50" } ],
                            [ string_field("status"), { "blobValue": "dmFyY2hhcg==" } ]
                        ]
                    }))
                } else if sql.starts_with("CREATE TEMPORARY TABLE")
                    || sql.starts_with("DROP TEMPORARY TABLE")
                {
                    assert_eq!(body["transactionId"], "tx-1");
                    ok(json!({ "numberOfRecordsUpdated": 0 }))
                } else {
                    ok(json!({
                        "columnMetadata": [
                            { "label": "status", "typeName": "VARCHAR" },
                            { "label": "amount", "typeName": "INT" }
                        ],
                        "records": [[ string_field("new"), { "longValue": 300 } ]]
                    }))
                }
            }
            other => panic!("unexpected {other}"),
        }
    }))
    .await;
    let driver = AuroraServerlessMySqlDriver::new(config(Some(&server.endpoint))).unwrap();
    let data = driver
        .download_query_results(
            "SELECT status, amount FROM t WHERE status = ?",
            &[json!("new")],
            &DownloadQueryResultsOptions::default(),
        )
        .await
        .unwrap();
    let DownloadedData::Memory(memory) = data else {
        panic!("memory expected")
    };
    assert_eq!(
        memory.columns,
        vec![Column::new("amount", "int"), Column::new("status", "text")]
    );
    assert_eq!(memory.rows, vec![vec![json!(300), json!("new")]]);

    let requests = server.requests();
    let paths: Vec<&str> = requests.iter().map(|r| r.target.as_str()).collect();
    assert_eq!(
        paths,
        vec![
            "/BeginTransaction",
            "/Execute",
            "/Execute",
            "/Execute",
            "/CommitTransaction",
            "/Execute"
        ]
    );
    let create = requests[1].json();
    let sql = create["sql"].as_str().unwrap();
    assert!(
        sql.starts_with("CREATE TEMPORARY TABLE `mysql`.t_"),
        "{sql}"
    );
    assert!(
        sql.ends_with(" AS SELECT status, amount FROM t WHERE status = :b0 LIMIT 0"),
        "{sql}"
    );
    assert_eq!(create["parameters"][0]["value"]["stringValue"], "new");
    assert_eq!(requests[0].json()["database"], "mysql");
}

#[tokio::test]
async fn failed_transaction_is_rolled_back() {
    let server = server(Box::new(|req| match path(req) {
        "/BeginTransaction" => ok(json!({ "transactionId": "tx-9" })),
        "/RollbackTransaction" => ok(json!({ "transactionStatus": "Rollback Complete" })),
        _ => MockResponse::json(
            400,
            "application/json",
            json!({ "message": "Table doesn't exist" }),
        )
        .with_header("x-amzn-ErrorType", "ForbiddenException"),
    }))
    .await;
    let driver = AuroraServerlessMySqlDriver::new(config(Some(&server.endpoint))).unwrap();
    let err = driver
        .download_query_results("SELECT 1", &[], &DownloadQueryResultsOptions::default())
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "Table doesn't exist");
    let requests = server.requests();
    let rollback = requests
        .iter()
        .find(|r| r.target == "/RollbackTransaction")
        .unwrap();
    assert_eq!(rollback.json()["transactionId"], "tx-9");
}

#[tokio::test]
async fn download_requires_a_database() {
    let mut c = config(Some("http://127.0.0.1:1"));
    c.database = None;
    let driver = AuroraServerlessMySqlDriver::new(c).unwrap();
    let err = driver
        .download_query_results("SELECT 1", &[], &DownloadQueryResultsOptions::default())
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Default database should be defined to be used for temporary tables during query results downloads"
    );
}

#[tokio::test]
async fn upload_batches_rows_and_converts_values() {
    let server = server(Box::new(|_| ok(json!({ "numberOfRecordsUpdated": 1 })))).await;
    let driver = AuroraServerlessMySqlDriver::new(config(Some(&server.endpoint))).unwrap();
    let columns = vec![Column::new("b", "boolean"), Column::new("ts", "timestamp")];
    let rows: Vec<Row> = (0..1001)
        .map(|_| vec![json!("true"), json!("2020-01-01T00:00:00.000Z")])
        .collect();
    driver
        .upload_table_with_indexes(
            "test.t",
            &columns,
            &QueryResult::new(columns.clone(), rows),
            &[IndexSql {
                sql: "CREATE INDEX i ON test.t (b)".to_string(),
                params: vec![],
            }],
            &[],
            &ExternalCreateTableOptions::default(),
        )
        .await
        .unwrap();
    let bodies: Vec<Value> = server.requests().iter().map(|r| r.json()).collect();
    assert_eq!(bodies.len(), 4); // CREATE, 2 INSERT batches, index
    assert_eq!(
        bodies[0]["sql"],
        "CREATE TABLE test.t (`b` boolean, `ts` timestamp)"
    );
    let insert = bodies[1]["sql"].as_str().unwrap();
    assert!(
        insert.starts_with(
            "INSERT INTO test.t\n            (`b`, `ts`)\n          VALUES (:b0, :b1), (:b2, :b3)"
        ),
        "{insert}"
    );
    assert_eq!(bodies[1]["parameters"].as_array().unwrap().len(), 2000);
    assert_eq!(
        bodies[1]["parameters"][0]["value"],
        json!({ "booleanValue": true })
    );
    assert_eq!(
        bodies[1]["parameters"][1]["value"],
        json!({ "stringValue": "2020-01-01T00:00:00.000" })
    );
    assert_eq!(bodies[2]["parameters"].as_array().unwrap().len(), 2);
    assert_eq!(bodies[3]["sql"], "CREATE INDEX i ON test.t (b)");
}

#[tokio::test]
async fn load_pre_aggregation_without_meta_lock() {
    let server = server(Box::new(|_| ok(json!({ "numberOfRecordsUpdated": 0 })))).await;
    let mut c = config(Some(&server.endpoint));
    c.load_pre_aggregation_without_meta_lock = true;
    let driver = AuroraServerlessMySqlDriver::new(c).unwrap();
    driver
        .load_pre_aggregation_into_table(
            "s.t",
            "CREATE TABLE s.t AS SELECT * FROM x WHERE a = ?",
            &[json!(1)],
            &QueryOptions::default(),
        )
        .await
        .unwrap();
    let sqls: Vec<String> = server
        .requests()
        .iter()
        .map(|r| r.json()["sql"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        sqls,
        vec![
            "CREATE TABLE s.t AS SELECT * FROM x WHERE a = :b0 LIMIT 0",
            "INSERT INTO s.t SELECT * FROM x WHERE a = :b0",
        ]
    );
}
