//! Unit tests of the Databricks driver.
//!
//! No Databricks workspace is available to the test suite, so the Statement
//! Execution API is exercised against an in-process HTTP server replaying the
//! response shapes documented at
//! <https://docs.databricks.com/api/workspace/statementexecution>.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;

// ----------------------------------------------------------------------
// A minimal HTTP/1.1 server
// ----------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Request {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: String,
}

impl Request {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

type Handler = Arc<dyn Fn(&Request, &str) -> (u16, String) + Send + Sync>;

struct MockServer {
    base_url: String,
    requests: Arc<StdMutex<Vec<Request>>>,
}

impl MockServer {
    /// `handler` receives the request and the server's own base URL.
    async fn start(
        handler: impl Fn(&Request, &str) -> (u16, String) + Send + Sync + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(StdMutex::new(Vec::new()));
        let handler: Handler = Arc::new(handler);
        let (reqs, base) = (requests.clone(), base_url.clone());
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let (reqs, handler, base) = (reqs.clone(), handler.clone(), base.clone());
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 4096];
                    let header_end = loop {
                        let n = socket.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            break pos + 4;
                        }
                    };
                    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
                    let mut lines = head.split("\r\n");
                    let mut first = lines.next().unwrap_or("").split(' ');
                    let method = first.next().unwrap_or("").to_string();
                    let path = first.next().unwrap_or("").to_string();
                    let headers: HashMap<String, String> = lines
                        .filter_map(|l| l.split_once(':'))
                        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
                        .collect();
                    let len: usize = headers
                        .get("content-length")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                    while buf.len() < header_end + len {
                        let n = socket.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    let body = String::from_utf8_lossy(&buf[header_end..]).to_string();
                    let request = Request {
                        method,
                        path,
                        headers,
                        body,
                    };
                    let (status, response) = handler(&request, &base);
                    reqs.lock().unwrap().push(request);
                    let out = format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                        response.len()
                    );
                    let _ = socket.write_all(out.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Self { base_url, requests }
    }

    fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }

    fn statements(&self) -> Vec<String> {
        self.requests()
            .iter()
            .filter(|r| r.method == "POST" && r.path == "/api/2.0/sql/statements")
            .map(|r| r.json()["statement"].as_str().unwrap_or("").to_string())
            .collect()
    }
}

const URL: &str = "jdbc:databricks://adb-123456789.10.azuredatabricks.net:443/default;transportMode=http;ssl=1;httpPath=/sql/1.0/warehouses/wh123;AuthMech=3";

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn config_from(pairs: &[(&str, &str)]) -> Result<DatabricksConfig> {
    let env = env(pairs);
    let driver = DriverConfig::from_env_source(&env, None, false)?;
    let mut config = DatabricksConfig::from_driver_config(driver);
    config.apply_env_source(&env)?;
    Ok(config)
}

fn base_config() -> DatabricksConfig {
    config_from(&[
        ("CUBEJS_DB_DATABRICKS_URL", URL),
        ("CUBEJS_DB_DATABRICKS_TOKEN", "dapi-token"),
        ("NODE_ENV", "production"),
    ])
    .unwrap()
}

fn driver_for(server: &MockServer, mut config: DatabricksConfig) -> DatabricksDriver {
    config.base_url = Some(server.base_url.clone());
    DatabricksDriver::new(config).unwrap()
}

fn manifest(columns: Value) -> Value {
    json!({ "format": "JSON_ARRAY", "schema": { "column_count": columns.as_array().map(|c| c.len()), "columns": columns } })
}

fn succeeded(id: &str, columns: Value, result: Value) -> String {
    json!({
        "statement_id": id,
        "status": { "state": "SUCCEEDED" },
        "manifest": manifest(columns),
        "result": result,
    })
    .to_string()
}

// ----------------------------------------------------------------------
// Configuration
// ----------------------------------------------------------------------

#[test]
fn url_is_required() {
    let err = config_from(&[]).unwrap_err();
    assert_eq!(
        err.to_string(),
        "The CUBEJS_DB_DATABRICKS_URL is required and missing."
    );
}

#[test]
fn env_is_data_source_aware() {
    let config = config_from(&[
        ("CUBEJS_DATASOURCES", "default,lake"),
        ("CUBEJS_DB_DATABRICKS_URL", "jdbc:databricks://wrong"),
        ("CUBEJS_DS_LAKE_DB_DATABRICKS_URL", URL),
        ("CUBEJS_DS_LAKE_DB_DATABRICKS_TOKEN", "t"),
        ("CUBEJS_DS_LAKE_DB_DATABRICKS_CATALOG", "main"),
    ]);
    // the default data source is read when none is given
    assert_eq!(
        config.unwrap().url.as_deref(),
        Some("jdbc:databricks://wrong")
    );

    let env = env(&[
        ("CUBEJS_DATASOURCES", "default,lake"),
        ("CUBEJS_DS_LAKE_DB_DATABRICKS_URL", URL),
        ("CUBEJS_DS_LAKE_DB_DATABRICKS_TOKEN", "t"),
        ("CUBEJS_DS_LAKE_DB_DATABRICKS_CATALOG", "main"),
    ]);
    let driver = DriverConfig::from_env_source(&env, Some("lake"), false).unwrap();
    let mut config = DatabricksConfig::from_driver_config(driver);
    config.apply_env_source(&env).unwrap();
    assert_eq!(config.url.as_deref(), Some(URL));
    assert_eq!(config.token.as_deref(), Some("t"));
    assert_eq!(config.catalog.as_deref(), Some("main"));

    let env = env_missing_lake();
    let driver = DriverConfig::from_env_source(&env, Some("lake"), false).unwrap();
    let mut config = DatabricksConfig::from_driver_config(driver);
    let err = config.apply_env_source(&env).unwrap_err();
    assert_eq!(
        err.to_string(),
        "The CUBEJS_DS_LAKE_DB_DATABRICKS_URL is required and missing."
    );
}

fn env_missing_lake() -> HashMap<String, String> {
    env(&[("CUBEJS_DATASOURCES", "default,lake")])
}

#[test]
fn export_bucket_settings() {
    use base64::Engine;
    let gcs = base64::engine::general_purpose::STANDARD.encode(r#"{"project_id":"p"}"#);
    let config = config_from(&[
        ("CUBEJS_DB_DATABRICKS_URL", URL),
        ("CUBEJS_DB_DATABRICKS_TOKEN", "t"),
        ("CUBEJS_DB_EXPORT_BUCKET_TYPE", "s3"),
        ("CUBEJS_DB_EXPORT_BUCKET", "s3://cube-export"),
        ("CUBEJS_DB_EXPORT_BUCKET_MOUNT_DIR", "dbfs:/mnt/export"),
        ("CUBEJS_DB_EXPORT_BUCKET_AWS_KEY", "AKIA"),
        ("CUBEJS_DB_EXPORT_BUCKET_AWS_SECRET", "secret"),
        ("CUBEJS_DB_EXPORT_BUCKET_AWS_REGION", "us-east-1"),
        ("CUBEJS_DB_EXPORT_BUCKET_AZURE_KEY", "ak"),
        ("CUBEJS_DB_EXPORT_BUCKET_AZURE_TENANT_ID", "tenant"),
        ("CUBEJS_DB_EXPORT_BUCKET_AZURE_CLIENT_ID", "client"),
        ("CUBEJS_DB_EXPORT_BUCKET_AZURE_CLIENT_SECRET", "cs"),
        ("CUBEJS_DB_EXPORT_BUCKET_CSV_ESCAPE_SYMBOL", "\\"),
        ("CUBEJS_DB_EXPORT_GCS_CREDENTIALS", &gcs),
        ("CUBEJS_DB_POLL_MAX_INTERVAL", "2s"),
        ("CUBEJS_DB_NAME", "sales"),
    ])
    .unwrap();
    assert_eq!(config.bucket_type.as_deref(), Some("s3"));
    assert_eq!(config.export_bucket.as_deref(), Some("s3://cube-export"));
    assert_eq!(
        config.export_bucket_mount_dir.as_deref(),
        Some("dbfs:/mnt/export")
    );
    assert_eq!(config.aws_key.as_deref(), Some("AKIA"));
    assert_eq!(config.aws_secret.as_deref(), Some("secret"));
    assert_eq!(config.aws_region.as_deref(), Some("us-east-1"));
    assert_eq!(config.azure_key.as_deref(), Some("ak"));
    assert_eq!(config.azure_tenant_id.as_deref(), Some("tenant"));
    assert_eq!(config.azure_client_id.as_deref(), Some("client"));
    assert_eq!(config.azure_client_secret.as_deref(), Some("cs"));
    assert_eq!(
        config.export_bucket_csv_escape_symbol.as_deref(),
        Some("\\")
    );
    assert_eq!(config.gcs_credentials, Some(json!({"project_id": "p"})));
    assert_eq!(config.poll_interval, Duration::from_secs(2));
    assert_eq!(config.database.as_deref(), Some("sales"));
    // an export bucket makes the driver writable
    assert!(!config.is_read_only());

    let err = config_from(&[
        ("CUBEJS_DB_DATABRICKS_URL", URL),
        ("CUBEJS_DB_EXPORT_BUCKET_TYPE", "gcp"),
    ])
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "The CUBEJS_DB_EXPORT_BUCKET_TYPE must be one of the [s3, gcs, azure]."
    );
}

#[test]
fn pre_aggregations_schema_resolution() {
    assert_eq!(
        pre_aggregations_schema_from_env(&env(&[("NODE_ENV", "production")])),
        "prod_pre_aggregations"
    );
    assert_eq!(
        pre_aggregations_schema_from_env(&env(&[])),
        "dev_pre_aggregations"
    );
    assert_eq!(
        pre_aggregations_schema_from_env(&env(&[
            ("NODE_ENV", "production"),
            ("CUBEJS_DEV_MODE", "true")
        ])),
        "dev_pre_aggregations"
    );
    assert_eq!(
        pre_aggregations_schema_from_env(&env(&[("CUBEJS_PRE_AGGREGATIONS_SCHEMA", "pa")])),
        "pa"
    );
}

#[test]
fn credentials_validation() {
    let mut config = base_config();
    config.token = None;
    let err = DatabricksDriver::new(config.clone()).unwrap_err();
    assert_eq!(err.to_string(), "No credentials provided");

    config.oauth_client_id = Some("id".into());
    let err = DatabricksDriver::new(config.clone()).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Invalid credentials: No OAuth Client Secret provided"
    );

    config.oauth_client_id = None;
    config.oauth_client_secret = Some("secret".into());
    let err = DatabricksDriver::new(config.clone()).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Invalid credentials: No OAuth Client ID provided"
    );

    // PWD inside the URL still works (deprecated)
    let mut config = base_config();
    config.token = None;
    config.url = Some(format!("{URL};UID=token;PWD=from-url"));
    let driver = DatabricksDriver::new(config).unwrap();
    assert_eq!(
        driver.auth.method,
        AuthMethod::Static(Credentials::Bearer("from-url".into()))
    );

    // the token variable wins over PWD
    let mut config = base_config();
    config.url = Some(format!("{URL};UID=token;PWD=from-url"));
    let driver = DatabricksDriver::new(config).unwrap();
    assert_eq!(
        driver.auth.method,
        AuthMethod::Static(Credentials::Bearer("dapi-token".into()))
    );
}

/// The Node.js unit test uses a URL without `httpPath`; the REST driver
/// needs the warehouse, so it reports it.
#[test]
fn node_test_url_without_http_path() {
    let config = config_from(&[
        (
            "CUBEJS_DB_DATABRICKS_URL",
            "jdbc:databricks://adb-123456789.10.azuredatabricks.net:443",
        ),
        ("CUBEJS_DB_DATABRICKS_TOKEN", "token"),
    ])
    .unwrap();
    let err = DatabricksDriver::new(config).unwrap_err();
    assert_eq!(err.to_string(), "Missing httpPath in JDBC URL");
}

#[test]
fn spark_url_is_accepted() {
    let mut config = base_config();
    config.url = Some(URL.replace("jdbc:databricks://", "jdbc:spark://"));
    let driver = DatabricksDriver::new(config).unwrap();
    assert_eq!(driver.connection_properties().warehouse_id, "wh123");
    assert_eq!(
        driver.connection_properties().host,
        "adb-123456789.10.azuredatabricks.net"
    );
}

// ----------------------------------------------------------------------
// SQL and types
// ----------------------------------------------------------------------

#[test]
fn sql_quoting_and_types() {
    let driver = DatabricksDriver::new(base_config()).unwrap();
    assert_eq!(driver.quote_identifier("a"), "`a`");
    assert_eq!(driver.quote_identifier("`a`"), "`a`");
    assert_eq!(driver.param(0), "?");
    assert!(driver.read_only());
    let caps = driver.capabilities();
    assert!(caps.unload_without_temp_table);
    assert!(caps.incremental_schema_loading);
    assert_eq!(driver.test_connection_timeout(), Duration::from_secs(60));
    assert_eq!(DEFAULT_CONCURRENCY, 10);
    assert_eq!(
        driver.wrap_query_with_limit("SELECT 1", 10),
        "SELECT * FROM (SELECT 1) AS t LIMIT 10"
    );

    assert_eq!(
        driver.to_generic_type("binary", None, None),
        GenericType::Other("hll_datasketches".into())
    );
    assert_eq!(
        driver.to_generic_type("decimal(10,0)", None, None),
        GenericType::Bigint
    );
    assert_eq!(
        driver.to_generic_type("string", None, None),
        GenericType::Text
    );
    assert_eq!(driver.to_generic_type("int", None, None), GenericType::Int);
    assert_eq!(
        driver.to_generic_type("bigint", None, None),
        GenericType::Bigint
    );
    assert_eq!(
        driver.to_generic_type("timestamp", None, None),
        GenericType::Timestamp
    );
    assert_eq!(
        driver.to_generic_type("boolean", None, None),
        GenericType::Boolean
    );
    assert_eq!(
        to_generic_type("numeric(10, 2)", None, None, false),
        GenericType::Other("numeric(10, 2)".into())
    );
    assert_eq!(
        to_generic_type("decimal(12,2)", None, None, false),
        GenericType::Decimal(Some((12, 2)))
    );
}

#[test]
fn parameters_are_inlined_with_spark_escaping() {
    let driver = DatabricksDriver::new(base_config()).unwrap();
    assert_eq!(
        driver.prepare_sql(
            "SELECT * FROM t WHERE a = ? AND b = ? AND c = ?",
            &[json!("it's \\ x"), json!(5), json!(null)]
        ),
        "SELECT * FROM t WHERE a = 'it\\'s \\\\ x' AND b = 5 AND c = NULL"
    );
}

#[test]
fn catalog_prefixes_the_pre_aggregation_schema() {
    let mut config = base_config();
    config.catalog = Some("main".into());
    let driver = DatabricksDriver::new(config).unwrap();
    assert_eq!(
        driver.prepare_sql(
            "SELECT * FROM prod_pre_aggregations.orders_main JOIN x.prod_pre_aggregations.y ON 1 WHERE s = 'prod_pre_aggregations. '",
            &[]
        ),
        "SELECT * FROM main.prod_pre_aggregations.orders_main JOIN x.prod_pre_aggregations.y ON 1 WHERE s = 'prod_pre_aggregations. '"
    );
    assert_eq!(
        prefix_schema_with_catalog("CREATE TABLE s.t AS SELECT 1\nFROM\ts.u", "s", "c"),
        "CREATE TABLE c.s.t AS SELECT 1\nFROM\tc.s.u"
    );
    assert_eq!(driver.schema_full_name("sales"), "`main`.`sales`");
    assert_eq!(driver.show_databases_sql(), "SHOW DATABASES IN `main`");
    assert_eq!(driver.table_full_name("a.b"), "`main`.`a`.`b`");
    assert_eq!(driver.table_full_name("c.a.b"), "`c`.`a`.`b`");

    let driver = DatabricksDriver::new(base_config()).unwrap();
    assert_eq!(driver.table_full_name("a.b"), "`a`.`b`");
    assert_eq!(driver.show_databases_sql(), "SHOW DATABASES");
}

#[test]
fn unload_statements() {
    let mut config = base_config();
    config.export_bucket = Some("s3://bucket".into());
    config.catalog = Some("main".into());
    let driver = DatabricksDriver::new(config).unwrap();
    let full = driver.unload_table_full_name("stb.orders");
    assert_eq!(full, "main.stb.orders");
    let columns = vec![
        Column::new("id", "int"),
        Column::new("sketch", "hll_datasketches"),
    ];
    assert_eq!(
        generate_table_columns_for_export(&columns),
        "id, base64(sketch)"
    );
    let sql = driver.unload_from_sql_statement(&full, "SELECT 1", &columns);
    assert!(sql.contains("INSERT OVERWRITE DIRECTORY 's3://bucket/main.stb.orders'"));
    assert!(sql.contains("USING CSV"));
    assert!(sql.contains("OPTIONS (escape '\"')"));
    assert!(sql.contains("SELECT id, base64(sketch) FROM (SELECT 1)"));
    let sql = driver.unload_from_table_statement(&full, &columns[..1]);
    assert!(sql.contains("SELECT id FROM main.stb.orders"));
}

/// Port of the Node.js unload tests: the signed-URL collection is not
/// implemented, so every bucket type fails with a named error.
#[tokio::test]
async fn unload_is_a_named_error() {
    for (bucket_type, bucket) in [
        ("azure", "wasbs://cube-export@mock.blob.core.windows.net"),
        ("s3", "s3://cube-export"),
        ("gcs", "gs://cube-export"),
    ] {
        let config = config_from(&[
            ("CUBEJS_DB_DATABRICKS_URL", URL),
            ("CUBEJS_DB_DATABRICKS_TOKEN", "token"),
            ("CUBEJS_DB_EXPORT_BUCKET_TYPE", bucket_type),
            ("CUBEJS_DB_EXPORT_BUCKET", bucket),
        ])
        .unwrap();
        let driver = DatabricksDriver::new(config).unwrap();
        assert!(!driver.read_only());
        let options = UnloadOptions {
            max_file_size: 3,
            query: Some(crate::types::UnloadQuery {
                sql: "SELECT * FROM product".into(),
                params: vec![json!(1)],
            }),
            request_id: None,
        };
        let err = driver.unload("product", &options).await.unwrap_err();
        assert!(matches!(err, DriverError::NotImplemented(_)), "{err}");
        assert!(err.to_string().contains(bucket_type));
        let err = driver.is_unload_supported(&options).await.unwrap_err();
        assert!(matches!(err, DriverError::NotImplemented(_)));
    }

    let mut config = base_config();
    config.export_bucket = Some("s3://x".into());
    let driver = DatabricksDriver::new(config).unwrap();
    let err = driver
        .unload("t", &UnloadOptions::default())
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "Unsupported export bucket type: undefined");

    let driver = DatabricksDriver::new(base_config()).unwrap();
    assert!(!driver
        .is_unload_supported(&UnloadOptions::default())
        .await
        .unwrap());
}

#[test]
fn values_are_hydrated_by_type() {
    assert_eq!(hydrate(&json!("true"), "BOOLEAN"), json!(true));
    assert_eq!(hydrate(&json!("42"), "INT"), json!(42));
    assert_eq!(hydrate(&json!("7"), "SHORT"), json!(7));
    assert_eq!(
        hydrate(&json!("9007199254740993"), "LONG"),
        json!("9007199254740993")
    );
    assert_eq!(hydrate(&json!("1.5"), "DOUBLE"), json!(1.5));
    assert_eq!(hydrate(&json!("NaN"), "DOUBLE"), json!("NaN"));
    assert_eq!(hydrate(&json!("12.30"), "DECIMAL"), json!("12.30"));
    assert_eq!(
        hydrate(&json!("2020-01-01T10:00:00.123Z"), "TIMESTAMP"),
        json!("2020-01-01T10:00:00.123")
    );
    assert_eq!(
        hydrate(&json!("2020-01-01T10:00:00Z"), "TIMESTAMP"),
        json!("2020-01-01T10:00:00.000")
    );
    assert_eq!(
        hydrate(&json!("2020-01-01T10:00:00"), "TIMESTAMP_NTZ"),
        json!("2020-01-01T10:00:00.000")
    );
    assert_eq!(hydrate(&json!("2020-01-01"), "DATE"), json!("2020-01-01"));
    assert_eq!(hydrate(&Value::Null, "INT"), Value::Null);
}

// ----------------------------------------------------------------------
// Statement Execution API (mock server)
// ----------------------------------------------------------------------

#[tokio::test]
async fn inline_query_with_chunks() {
    let server = MockServer::start(|req, _| match (req.method.as_str(), req.path.as_str()) {
        ("POST", "/api/2.0/sql/statements") => (
            200,
            succeeded(
                "st-1",
                json!([
                    { "name": "id", "type_text": "INT", "type_name": "INT", "position": 0 },
                    { "name": "name", "type_text": "STRING", "type_name": "STRING", "position": 1 },
                    { "name": "amount", "type_text": "DECIMAL(10,2)", "type_name": "DECIMAL", "position": 2, "type_precision": 10, "type_scale": 2 },
                    { "name": "ok", "type_text": "BOOLEAN", "type_name": "BOOLEAN", "position": 3 },
                    { "name": "ts", "type_text": "TIMESTAMP", "type_name": "TIMESTAMP", "position": 4 }
                ]),
                json!({
                    "chunk_index": 0, "row_offset": 0, "row_count": 1,
                    "data_array": [["1", "a", "1.50", "true", "2020-01-01T00:00:00.000Z"]],
                    "next_chunk_index": 1,
                    "next_chunk_internal_link": "/api/2.0/sql/statements/st-1/result/chunks/1"
                }),
            ),
        ),
        ("GET", "/api/2.0/sql/statements/st-1/result/chunks/1") => (
            200,
            json!({
                "chunk_index": 1, "row_offset": 1, "row_count": 1,
                "data_array": [["2", null, null, "false", null]]
            })
            .to_string(),
        ),
        _ => (404, json!({"error_code": "NOT_FOUND", "message": "no"}).to_string()),
    })
    .await;

    let mut config = base_config();
    config.url = Some(format!("{URL};ConnCatalog=main"));
    let driver = driver_for(&server, config);
    let result = driver
        .query(
            "SELECT * FROM t WHERE x = ?",
            &[json!("v")],
            &QueryOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        result.columns,
        vec![
            Column::new("id", "int"),
            Column::new("name", "text"),
            Column::new("amount", "decimal(10, 2)"),
            Column::new("ok", "boolean"),
            Column::new("ts", "timestamp"),
        ]
    );
    assert_eq!(
        result.rows,
        vec![
            vec![
                json!(1),
                json!("a"),
                json!("1.50"),
                json!(true),
                json!("2020-01-01T00:00:00.000")
            ],
            vec![
                json!(2),
                Value::Null,
                Value::Null,
                json!(false),
                Value::Null
            ],
        ]
    );

    let requests = server.requests();
    let submit = requests[0].json();
    assert_eq!(submit["statement"], "SELECT * FROM t WHERE x = 'v'");
    assert_eq!(submit["warehouse_id"], "wh123");
    assert_eq!(submit["catalog"], "main");
    assert_eq!(submit["schema"], "default");
    assert_eq!(submit["disposition"], "INLINE");
    assert_eq!(submit["format"], "JSON_ARRAY");
    assert_eq!(submit["on_wait_timeout"], "CONTINUE");
    assert_eq!(
        requests[0].headers.get("authorization").map(String::as_str),
        Some("Bearer dapi-token")
    );
    assert_eq!(
        requests[0].headers.get("user-agent").map(String::as_str),
        Some("CubeDev_Cube")
    );
}

#[tokio::test]
async fn pending_statements_are_polled() {
    let polls = Arc::new(StdMutex::new(0));
    let p = polls.clone();
    let server = MockServer::start(
        move |req, _| match (req.method.as_str(), req.path.as_str()) {
            ("POST", "/api/2.0/sql/statements") => (
                200,
                json!({ "statement_id": "st-2", "status": { "state": "PENDING" } }).to_string(),
            ),
            ("GET", "/api/2.0/sql/statements/st-2") => {
                let mut n = p.lock().unwrap();
                *n += 1;
                if *n < 3 {
                    (
                        200,
                        json!({ "statement_id": "st-2", "status": { "state": "RUNNING" } })
                            .to_string(),
                    )
                } else {
                    (
                        200,
                        succeeded(
                            "st-2",
                            json!([{ "name": "1", "type_text": "INT", "type_name": "INT" }]),
                            json!({ "chunk_index": 0, "row_count": 1, "data_array": [["1"]] }),
                        ),
                    )
                }
            }
            _ => (404, "{}".to_string()),
        },
    )
    .await;
    let driver = driver_for(&server, base_config());
    let result = driver
        .query("SELECT 1", &[], &QueryOptions::default())
        .await
        .unwrap();
    assert_eq!(result.rows, vec![vec![json!(1)]]);
    assert_eq!(*polls.lock().unwrap(), 3);
}

#[tokio::test]
async fn timed_out_statements_are_cancelled() {
    let server = MockServer::start(|req, _| match (req.method.as_str(), req.path.as_str()) {
        ("POST", "/api/2.0/sql/statements") | ("GET", "/api/2.0/sql/statements/st-3") => (
            200,
            json!({ "statement_id": "st-3", "status": { "state": "RUNNING" } }).to_string(),
        ),
        ("POST", "/api/2.0/sql/statements/st-3/cancel") => (200, "{}".to_string()),
        _ => (404, "{}".to_string()),
    })
    .await;
    let mut config = base_config();
    config.execution_timeout = Duration::from_millis(300);
    let driver = driver_for(&server, config);
    let err = driver
        .query("SELECT sleep()", &[], &QueryOptions::default())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("timeout"), "{err}");
    assert!(server
        .requests()
        .iter()
        .any(|r| r.path == "/api/2.0/sql/statements/st-3/cancel"));
}

#[tokio::test]
async fn failures_are_reported() {
    let server = MockServer::start(|req, _| match req.path.as_str() {
        "/api/2.0/sql/statements" => {
            let statement = req.json()["statement"].as_str().unwrap_or("").to_string();
            if statement.contains("bad") {
                (
                    200,
                    json!({
                        "statement_id": "st-4",
                        "status": { "state": "FAILED", "error": {
                            "error_code": "BAD_REQUEST",
                            "message": "[TABLE_OR_VIEW_NOT_FOUND] The table or view `bad` cannot be found."
                        }}
                    })
                    .to_string(),
                )
            } else {
                (
                    403,
                    json!({ "error_code": "PERMISSION_DENIED", "message": "Invalid access token." }).to_string(),
                )
            }
        }
        _ => (404, "{}".to_string()),
    })
    .await;
    let driver = driver_for(&server, base_config());
    let err = driver
        .query("SELECT * FROM bad", &[], &QueryOptions::default())
        .await
        .unwrap_err();
    match err {
        DriverError::Database { message, code } => {
            assert!(message.contains("TABLE_OR_VIEW_NOT_FOUND"));
            assert_eq!(code.as_deref(), Some("BAD_REQUEST"));
        }
        other => panic!("unexpected {other:?}"),
    }
    let err = driver
        .query("SELECT 1", &[], &QueryOptions::default())
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "Invalid access token.");
}

#[tokio::test]
async fn external_links_download_and_stream() {
    let server = MockServer::start(|req, base| match (req.method.as_str(), req.path.as_str()) {
        ("POST", "/api/2.0/sql/statements") => (
            200,
            succeeded(
                "st-5",
                json!([
                    { "name": "id", "type_text": "BIGINT", "type_name": "LONG" },
                    { "name": "v", "type_text": "DOUBLE", "type_name": "DOUBLE" }
                ]),
                json!({
                    "external_links": [{
                        "chunk_index": 0, "row_offset": 0, "row_count": 2,
                        "external_link": format!("{base}/ext/0?sig=abc"),
                        "http_headers": { "x-ms-blob-type": "BlockBlob" },
                        "next_chunk_index": 1,
                        "next_chunk_internal_link": "/api/2.0/sql/statements/st-5/result/chunks/1"
                    }]
                }),
            ),
        ),
        ("GET", "/api/2.0/sql/statements/st-5/result/chunks/1") => (
            200,
            json!({ "external_links": [{
                "chunk_index": 1, "row_offset": 2, "row_count": 1,
                "external_link": format!("{base}/ext/1?sig=def")
            }]})
            .to_string(),
        ),
        ("GET", "/ext/0?sig=abc") => (200, json!([["1", "1.5"], ["2", null]]).to_string()),
        ("GET", "/ext/1?sig=def") => (200, json!([["3", "2.5"]]).to_string()),
        _ => (404, "{}".to_string()),
    })
    .await;
    let driver = driver_for(&server, base_config());

    let data = driver
        .download_query_results(
            "SELECT * FROM big",
            &[],
            &DownloadQueryResultsOptions::default(),
        )
        .await
        .unwrap();
    let DownloadedData::Memory(memory) = data else {
        panic!("expected memory data")
    };
    assert_eq!(memory.rows.len(), 3);
    assert_eq!(memory.rows[0], vec![json!("1"), json!(1.5)]);
    assert_eq!(server.requests()[0].json()["disposition"], "EXTERNAL_LINKS");

    // pre-signed links are fetched without the workspace credentials
    let ext: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|r| r.path.starts_with("/ext/"))
        .collect();
    assert_eq!(ext.len(), 2);
    assert!(ext.iter().all(|r| !r.headers.contains_key("authorization")));
    assert_eq!(
        ext[0].headers.get("x-ms-blob-type").map(String::as_str),
        Some("BlockBlob")
    );

    let options = DownloadQueryResultsOptions {
        stream_import: true,
        ..Default::default()
    };
    let DownloadedData::Stream(stream) = driver
        .download_query_results("SELECT * FROM big", &[], &options)
        .await
        .unwrap()
    else {
        panic!("expected a stream")
    };
    assert_eq!(
        stream.columns,
        vec![Column::new("id", "bigint"), Column::new("v", "double")]
    );
    use futures::TryStreamExt;
    let rows: Vec<Row> = stream.rows.try_collect().await.unwrap();
    assert_eq!(
        rows,
        vec![
            vec![json!("1"), json!(1.5)],
            vec![json!("2"), Value::Null],
            vec![json!("3"), json!(2.5)],
        ]
    );
}

#[tokio::test]
async fn oauth_m2m_tokens_are_exchanged_and_cached() {
    let server = MockServer::start(|req, _| match (req.method.as_str(), req.path.as_str()) {
        ("POST", "/oidc/v1/token") => (
            200,
            json!({ "access_token": "oauth-at", "token_type": "Bearer", "expires_in": 3600 })
                .to_string(),
        ),
        ("POST", "/api/2.0/sql/statements") => (
            200,
            succeeded("st-6", json!([]), json!({ "data_array": [] })),
        ),
        _ => (404, "{}".to_string()),
    })
    .await;
    let mut config = base_config();
    config.token = None;
    config.oauth_client_id = Some("client".into());
    config.oauth_client_secret = Some("secret".into());
    let driver = driver_for(&server, config);
    driver
        .query("SELECT 1", &[], &QueryOptions::default())
        .await
        .unwrap();
    driver
        .query("SELECT 2", &[], &QueryOptions::default())
        .await
        .unwrap();

    let requests = server.requests();
    let token_requests: Vec<_> = requests
        .iter()
        .filter(|r| r.path == "/oidc/v1/token")
        .collect();
    assert_eq!(token_requests.len(), 1);
    use base64::Engine;
    let basic = base64::engine::general_purpose::STANDARD.encode("client:secret");
    assert_eq!(
        token_requests[0]
            .headers
            .get("authorization")
            .map(String::as_str),
        Some(format!("Basic {basic}").as_str())
    );
    assert!(token_requests[0]
        .body
        .contains("grant_type=client_credentials"));
    assert!(token_requests[0].body.contains("scope=all-apis"));
    assert!(requests
        .iter()
        .filter(|r| r.path == "/api/2.0/sql/statements")
        .all(|r| r.headers.get("authorization").map(String::as_str) == Some("Bearer oauth-at")));
}

#[tokio::test]
async fn test_connection_checks_the_warehouse() {
    let state = Arc::new(StdMutex::new(
        json!({ "state": "RUNNING", "health": { "status": "HEALTHY" } }),
    ));
    let s = state.clone();
    let server = MockServer::start(move |req, _| match req.path.as_str() {
        "/api/2.0/sql/warehouses/wh123" => (200, s.lock().unwrap().to_string()),
        _ => (404, "{}".to_string()),
    })
    .await;
    let driver = driver_for(&server, base_config());
    driver.test_connection().await.unwrap();

    *state.lock().unwrap() = json!({ "state": "DELETED" });
    let err = driver.test_connection().await.unwrap_err();
    assert!(err
        .to_string()
        .contains("Warehouse is being deleted (current state: DELETED)"));

    *state.lock().unwrap() = json!({ "state": "RUNNING", "health": { "status": "FAILED", "summary": "boom", "details": "disk" } });
    let err = driver.test_connection().await.unwrap_err();
    assert!(err
        .to_string()
        .contains("Warehouse is unhealthy: boom. Details: disk"));
}

fn describe_result(columns: &[(&str, &str)]) -> Value {
    let mut rows: Vec<Value> = columns.iter().map(|(n, t)| json!([n, t, null])).collect();
    rows.push(json!(["", "", ""]));
    rows.push(json!(["# Partition Information", "", ""]));
    json!({ "data_array": rows })
}

#[tokio::test]
async fn introspection() {
    let server = MockServer::start(|req, _| {
        let statement = req.json()["statement"].as_str().unwrap_or("").to_string();
        let string_col = |n: &str| json!({ "name": n, "type_text": "STRING", "type_name": "STRING" });
        let body = match statement.as_str() {
            "SHOW DATABASES IN `main`" => succeeded(
                "s",
                json!([string_col("databaseName")]),
                json!({ "data_array": [["sales"], ["hr"]] }),
            ),
            "SHOW TABLES IN `main`.`sales`" => succeeded(
                "s",
                json!([string_col("database"), string_col("tableName"), { "name": "isTemporary", "type_text": "BOOLEAN", "type_name": "BOOLEAN" }]),
                json!({ "data_array": [["sales", "orders", "false"]] }),
            ),
            "SHOW TABLES IN `main`.`hr`" => succeeded(
                "s",
                json!([string_col("database"), string_col("tableName"), { "name": "isTemporary", "type_text": "BOOLEAN", "type_name": "BOOLEAN" }]),
                json!({ "data_array": [["hr", "people", "false"]] }),
            ),
            "DESCRIBE `main`.`sales`.`orders`" => succeeded(
                "s",
                json!([string_col("col_name"), string_col("data_type"), string_col("comment")]),
                describe_result(&[("id", "bigint"), ("amount", "decimal(10,2)"), ("sketch", "binary")]),
            ),
            "DESCRIBE `main`.`hr`.`people`" => succeeded(
                "s",
                json!([string_col("col_name"), string_col("data_type"), string_col("comment")]),
                describe_result(&[("name", "string")]),
            ),
            "DESCRIBE QUERY SELECT * FROM x WHERE a = 1" => succeeded(
                "s",
                json!([string_col("col_name"), string_col("data_type"), string_col("comment")]),
                describe_result(&[("a", "int"), ("b", "timestamp")]),
            ),
            _ => succeeded("s", json!([]), json!({ "data_array": [] })),
        };
        (200, body)
    })
    .await;
    let mut config = base_config();
    config.catalog = Some("main".into());
    let driver = driver_for(&server, config);

    let structure = driver.tables_schema().await.unwrap();
    let orders = &structure["sales"]["orders"];
    assert_eq!(
        orders
            .iter()
            .map(|c| (c.name.as_str(), c.type_.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("id", "bigint"),
            ("amount", "decimal(10, 2)"),
            ("sketch", "hll_datasketches")
        ]
    );
    assert_eq!(structure["hr"]["people"][0].type_, "text");

    let schemas = driver.get_schemas().await.unwrap();
    assert_eq!(
        schemas,
        vec![
            SchemaName {
                schema_name: "sales".into()
            },
            SchemaName {
                schema_name: "hr".into()
            }
        ]
    );
    let tables = driver
        .get_tables_for_specific_schemas(&schemas)
        .await
        .unwrap();
    assert_eq!(tables.len(), 2);
    assert_eq!(tables[0].table_name, "orders");
    assert_eq!(tables[0].schema_name, "sales");

    let columns = driver
        .get_columns_for_specific_tables(&tables[..1])
        .await
        .unwrap();
    assert_eq!(columns.len(), 3);
    assert_eq!(columns[0].column_name, "id");
    assert_eq!(columns[0].data_type, "bigint");
    assert_eq!(columns[0].schema_name, "sales");

    assert_eq!(
        driver.get_tables_query("sales").await.unwrap(),
        vec!["orders".to_string()]
    );

    let types = driver
        .query_column_types(
            "SELECT * FROM x WHERE a = ?",
            &[json!(1)],
            &QueryOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        types,
        vec![Column::new("a", "int"), Column::new("b", "timestamp")]
    );

    driver.create_schema_if_not_exists("stb").await.unwrap();
    driver
        .drop_table("prod_pre_aggregations.t1", &QueryOptions::default())
        .await
        .unwrap();
    driver
        .load_pre_aggregation_into_table(
            "stb.t2",
            "CREATE TABLE stb.t2 AS SELECT * FROM prod_pre_aggregations.src",
            &[],
            &QueryOptions::default(),
        )
        .await
        .unwrap();
    let statements = server.statements();
    assert!(statements.contains(&"CREATE SCHEMA IF NOT EXISTS `main`.`stb`".to_string()));
    assert!(statements.contains(&"DROP TABLE main.prod_pre_aggregations.t1".to_string()));
    assert!(statements.contains(
        &"CREATE TABLE main.stb.t2 AS SELECT * FROM main.prod_pre_aggregations.src".to_string()
    ));
}

#[tokio::test]
async fn database_restricts_tables_schema() {
    let server = MockServer::start(|req, _| {
        let statement = req.json()["statement"].as_str().unwrap_or("").to_string();
        let string_col =
            |n: &str| json!({ "name": n, "type_text": "STRING", "type_name": "STRING" });
        let body = match statement.as_str() {
            "SHOW TABLES IN `sales`" => succeeded(
                "s",
                json!([string_col("database"), string_col("tableName")]),
                json!({ "data_array": [["sales", "orders"]] }),
            ),
            "DESCRIBE `sales`.`orders`" => succeeded(
                "s",
                json!([string_col("col_name"), string_col("data_type")]),
                json!({ "data_array": [["id", "int"]] }),
            ),
            other => panic!("unexpected statement {other}"),
        };
        (200, body)
    })
    .await;
    let mut config = base_config();
    config.database = Some("sales".into());
    let driver = driver_for(&server, config);
    let structure = driver.tables_schema().await.unwrap();
    assert_eq!(structure["sales"]["orders"][0].name, "id");
    assert_eq!(structure["sales"]["orders"][0].type_, "int");
}
