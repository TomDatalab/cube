//! Pinot driver: port of `@cubejs-backend/pinot-driver` over the broker's
//! `POST /query/sql` endpoint (`reqwest` with rustls).
//!
//! Kept from Node: the URL rules (`CUBEJS_DB_HOST` may carry the scheme,
//! otherwise `CUBEJS_DB_SSL` picks `https`), bearer-token / basic auth and
//! the `database` header, the `queryOptions` string (multi-stage engine,
//! `enableNullHandling`, `timeoutMs`), client-side ANSI interpolation, the
//! type mapping, `readOnly`, identity quoting and the error messages.
//!
//! Differences: `CUBEJS_DB_SSL_CA` is added to the trusted roots (node-fetch
//! ignored the SSL options entirely; certificates are verified either way), an
//! unset `CUBEJS_DB_PINOT_NULL_HANDLING` is sent as `false` rather than the
//! literal `undefined` (which Pinot parsed as `false`), and a missing
//! `CUBEJS_DB_PORT` is a configuration error instead of a request to
//! `host:undefined`.
//!
//! `PinotQuery` (the SQL dialect) belongs to the planner, not to this crate.

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::config::{data_sources, env_key, DriverConfig, EnvSource, ProcessEnv};
use crate::driver::Driver;
use crate::error::{DriverError, Result};
use crate::escape::format_ansi;
use crate::types::{
    Column, DownloadQueryResultsOptions, DownloadedData, GenericType, QueryOptions, QueryResult,
};

/// `getDefaultConcurrency`.
pub const DEFAULT_CONCURRENCY: usize = 10;

/// `PinotTypeToGenericType`.
pub fn pinot_type_to_generic(column_type: &str) -> Option<GenericType> {
    Some(match column_type.to_lowercase().as_str() {
        "string" => GenericType::Text,
        "int" => GenericType::Int,
        "long" => GenericType::Bigint,
        "float" => GenericType::Double,
        "double" => GenericType::Double,
        "big_decimal" => GenericType::Decimal(None),
        "boolean" => GenericType::Boolean,
        "timestamp" => GenericType::Timestamp,
        "json" => GenericType::Text,
        "bytes" => GenericType::Text,
        _ => return None,
    })
}

/// Configuration of [`PinotDriver`] (`PinotDriverConfiguration`).
#[derive(Debug, Clone)]
pub struct PinotConfig {
    /// Connection settings and global driver knobs.
    pub driver: DriverConfig,
    /// `CUBEJS_DB_HOST`, with or without `http(s)://`.
    pub host: String,
    /// `CUBEJS_DB_PORT`.
    pub port: Option<u16>,
    /// `CUBEJS_DB_USER`.
    pub user: Option<String>,
    /// `CUBEJS_DB_NAME`: sent as the `database` header with a token.
    pub database: Option<String>,
    /// `CUBEJS_DB_PASS`: basic auth when set.
    pub password: Option<String>,
    /// `CUBEJS_DB_PINOT_AUTH_TOKEN`.
    pub auth_token: Option<String>,
    /// `CUBEJS_DB_SSL`: `https` when the host has no scheme.
    pub use_ssl: bool,
    /// `CUBEJS_DB_PINOT_NULL_HANDLING`.
    pub null_handling: Option<bool>,
    /// `CUBEJS_DB_QUERY_TIMEOUT` (sent as `timeoutMs`).
    pub query_timeout: Duration,
}

fn get(env: &dyn EnvSource, driver: &DriverConfig, origin: &str) -> Result<Option<String>> {
    let key = env_key(
        origin,
        &data_sources(env),
        Some(&driver.data_source.data_source),
        driver.data_source.pre_aggregations,
    )?;
    Ok(env.get(&key).filter(|v| !v.is_empty()))
}

fn get_bool(env: &dyn EnvSource, driver: &DriverConfig, origin: &str) -> Result<Option<bool>> {
    match get(env, driver, origin)? {
        None => Ok(None),
        Some(v) => match v.to_lowercase().as_str() {
            "true" => Ok(Some(true)),
            "false" => Ok(Some(false)),
            _ => {
                let key = env_key(
                    origin,
                    &data_sources(env),
                    Some(&driver.data_source.data_source),
                    false,
                )?;
                Err(DriverError::Config(format!(
                    "The {key} must be either 'true' or 'false'."
                )))
            }
        },
    }
}

impl PinotConfig {
    /// Builds the configuration from the generic `CUBEJS_DB_*` settings.
    pub fn from_driver_config(driver: DriverConfig) -> Self {
        let ds = &driver.data_source;
        Self {
            host: ds.host.clone().unwrap_or_default(),
            port: ds.port,
            user: ds.user.clone(),
            database: ds.database.clone(),
            password: ds.password.clone(),
            auth_token: None,
            use_ssl: false,
            null_handling: None,
            query_timeout: ds.query_timeout,
            driver,
        }
    }

    /// Reads the Pinot specific variables from the process environment.
    pub fn apply_env(&mut self) -> Result<()> {
        self.apply_env_source(&ProcessEnv)
    }

    /// Reads `CUBEJS_DB_PINOT_*` and `CUBEJS_DB_SSL`.
    pub fn apply_env_source(&mut self, env: &dyn EnvSource) -> Result<()> {
        self.auth_token = get(env, &self.driver, "CUBEJS_DB_PINOT_AUTH_TOKEN")?;
        self.null_handling = get_bool(env, &self.driver, "CUBEJS_DB_PINOT_NULL_HANDLING")?;
        self.use_ssl = get_bool(env, &self.driver, "CUBEJS_DB_SSL")?.unwrap_or(false);
        Ok(())
    }

    /// Reads everything from the process environment.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        let mut config = Self::from_driver_config(DriverConfig::from_env(data_source)?);
        config.apply_env()?;
        Ok(config)
    }

    /// `<scheme>://<host>:<port>/query/sql`.
    pub fn url(&self) -> Result<String> {
        let port = self.port.ok_or_else(|| {
            DriverError::Config("CUBEJS_DB_PORT is required for the Pinot driver".to_string())
        })?;
        let lower = self.host.to_lowercase();
        let host = if lower.starts_with("http://") || lower.starts_with("https://") {
            self.host.clone()
        } else {
            format!(
                "{}://{}",
                if self.use_ssl { "https" } else { "http" },
                self.host
            )
        };
        Ok(format!("{host}:{port}/query/sql"))
    }

    /// `authorizationHeaders()`.
    pub fn authorization_headers(&self) -> Vec<(&'static str, String)> {
        if let Some(token) = &self.auth_token {
            let mut headers = vec![("Authorization", format!("Bearer {token}"))];
            if let Some(database) = &self.database {
                headers.push(("database", database.clone()));
            }
            return headers;
        }
        match &self.password {
            None => Vec::new(),
            Some(password) => {
                use base64::Engine as _;
                let credentials = format!("{}:{password}", self.user.as_deref().unwrap_or(""));
                vec![(
                    "Authorization",
                    format!(
                        "Basic {}",
                        base64::engine::general_purpose::STANDARD.encode(credentials)
                    ),
                )]
            }
        }
    }

    /// The `queryOptions` string.
    pub fn query_options(&self) -> String {
        format!(
            "useMultistageEngine=true;enableNullHandling={};timeoutMs={}",
            self.null_handling.unwrap_or(false),
            self.query_timeout.as_secs() * 1000
        )
    }
}

/// `resultTable` of a broker response.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultTable {
    #[serde(default)]
    pub data_schema: DataSchema,
    #[serde(default)]
    pub rows: Vec<Vec<Value>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataSchema {
    #[serde(default)]
    pub column_data_types: Vec<String>,
    #[serde(default)]
    pub column_names: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PinotResponse {
    #[serde(default)]
    exceptions: Vec<Value>,
    #[serde(default)]
    result_table: Option<ResultTable>,
}

/// Pinot driver.
pub struct PinotDriver {
    config: PinotConfig,
    url: String,
    client: reqwest::Client,
}

impl std::fmt::Debug for PinotDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PinotDriver")
            .field("url", &self.url)
            .finish()
    }
}

impl PinotDriver {
    /// Creates the driver. Nothing is sent until the first query.
    pub fn new(config: PinotConfig) -> Result<Self> {
        let url = config.url()?;
        let mut builder = reqwest::Client::builder()
            .user_agent(format!("CubeDev_Cube/{}", env!("CARGO_PKG_VERSION")))
            // The broker enforces `timeoutMs`; this only guards a dead socket.
            .timeout(config.query_timeout + Duration::from_secs(30));
        if let Some(ca) = config
            .driver
            .data_source
            .ssl
            .as_ref()
            .and_then(|s| s.ca.as_ref())
        {
            for cert in reqwest::Certificate::from_pem_bundle(ca.as_bytes()).map_err(|e| {
                DriverError::Config(format!("Invalid CUBEJS_DB_SSL_CA for Pinot: {e}"))
            })? {
                builder = builder.add_root_certificate(cert);
            }
        }
        let client = builder
            .build()
            .map_err(|e| DriverError::Config(format!("Unable to build the HTTP client: {e}")))?;
        Ok(Self {
            config,
            url,
            client,
        })
    }

    /// Creates the driver from `CUBEJS_DB_*`.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Self::new(PinotConfig::from_env(data_source)?)
    }

    /// The driver configuration.
    pub fn pinot_config(&self) -> &PinotConfig {
        &self.config
    }

    /// The broker SQL endpoint.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// `prepareQueryWithParams`: ANSI client-side interpolation.
    pub fn prepare_query_with_params(&self, query: &str, values: &[Value]) -> String {
        format_ansi(query, values)
    }

    /// The JSON body posted to the broker.
    pub fn request_body(&self, sql: &str) -> Value {
        json!({ "sql": sql, "queryOptions": self.config.query_options() })
    }

    /// `request()`: posts `sql` and returns the result table.
    pub async fn request(&self, sql: &str) -> Result<ResultTable> {
        let mut request = self
            .client
            .post(&self.url)
            .header("Content-Type", "application/json");
        for (name, value) in self.config.authorization_headers() {
            request = request.header(name, value);
        }
        let response = request
            .body(self.request_body(sql).to_string())
            .send()
            .await
            .map_err(|e| DriverError::Query(e.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(DriverError::Query(if status.as_u16() == 401 {
                "Unauthorized request".to_string()
            } else {
                "Unexpected error".to_string()
            }));
        }
        let text = response
            .text()
            .await
            .map_err(|e| DriverError::Query(e.to_string()))?;
        let parsed: PinotResponse = serde_json::from_str(&text)
            .map_err(|e| DriverError::Query(format!("Unexpected Pinot response: {e}")))?;
        if let Some(exception) = parsed.exceptions.first() {
            let message = exception
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| exception.to_string());
            return Err(DriverError::Database {
                message,
                code: exception.get("errorCode").map(|c| match c {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                }),
            });
        }
        parsed
            .result_table
            .ok_or_else(|| DriverError::Query("Pinot response has no resultTable".to_string()))
    }

    fn to_result(&self, table: ResultTable) -> QueryResult {
        let columns = table
            .data_schema
            .column_names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let db_type = table
                    .data_schema
                    .column_data_types
                    .get(i)
                    .map(String::as_str)
                    .unwrap_or("string");
                Column::new(name.clone(), self.to_generic_type(db_type, None, None))
            })
            .collect();
        QueryResult::new(columns, table.rows)
    }
}

#[async_trait]
impl Driver for PinotDriver {
    fn config(&self) -> &DriverConfig {
        &self.config.driver
    }

    /// `select 1` must return a row.
    async fn test_connection(&self) -> Result<()> {
        let table = self.request("select 1").await?;
        if table.rows.is_empty() {
            return Err(DriverError::Connection {
                pool_name: "pinot".to_string(),
                message: "Unable to connect to your Pinot instance".to_string(),
            });
        }
        Ok(())
    }

    async fn query(
        &self,
        sql: &str,
        params: &[Value],
        _options: &QueryOptions,
    ) -> Result<QueryResult> {
        let table = self
            .request(&self.prepare_query_with_params(sql, params))
            .await?;
        Ok(self.to_result(table))
    }

    fn read_only(&self) -> bool {
        true
    }

    /// Pinot identifiers are not quoted.
    fn quote_identifier(&self, identifier: &str) -> String {
        identifier.to_string()
    }

    fn to_generic_type(
        &self,
        db_type: &str,
        precision: Option<i64>,
        scale: Option<i64>,
    ) -> GenericType {
        pinot_type_to_generic(db_type).unwrap_or_else(|| {
            crate::types::to_generic_type(
                db_type,
                precision,
                scale,
                self.config().precise_decimal_in_cubestore,
            )
        })
    }

    /// Rows plus the broker's column types (no type detection).
    async fn download_query_results(
        &self,
        sql: &str,
        params: &[Value],
        _options: &DownloadQueryResultsOptions,
    ) -> Result<DownloadedData> {
        Ok(DownloadedData::Memory(
            self.query(sql, params, &QueryOptions::default()).await?,
        ))
    }
}

// Shared with the Presto tests; loaded twice when both features are on.
#[cfg(test)]
#[allow(clippy::duplicate_mod)]
#[path = "../prestodb/mock_http.rs"]
mod mock_http;

#[cfg(test)]
mod tests {
    use super::mock_http::{new_log, start};
    use super::*;
    use std::collections::HashMap;

    fn config(host: &str, port: Option<u16>) -> PinotConfig {
        let mut config = PinotConfig::from_driver_config(DriverConfig::default());
        config.host = host.to_string();
        config.port = port;
        config
    }

    fn driver() -> PinotDriver {
        PinotDriver::new(config("localhost", Some(8099))).unwrap()
    }

    const LIKE: &str =
        r"SELECT * FROM orders WHERE LOWER(name) LIKE CONCAT('%', LOWER(?), '%') ESCAPE '\'";

    // --- params-escaping.test.ts ---

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
    fn doubles_quotes_so_a_value_cannot_break_out_of_the_literal() {
        assert_eq!(
            driver().prepare_query_with_params(
                "SELECT * FROM orders WHERE name = ?",
                &[json!("o'reilly'); DROP TABLE orders; --")]
            ),
            "SELECT * FROM orders WHERE name = 'o''reilly''); DROP TABLE orders; --'"
        );
    }

    #[test]
    fn keeps_the_literal_closed_for_a_quote_payload() {
        assert_eq!(
            driver().prepare_query_with_params(
                "SELECT * FROM orders WHERE name = ? AND status = ?",
                &[json!("a'"), json!("new")]
            ),
            "SELECT * FROM orders WHERE name = 'a''' AND status = 'new'"
        );
    }

    #[test]
    fn keeps_the_literal_closed_for_a_value_ending_in_a_backslash() {
        assert_eq!(
            driver().prepare_query_with_params(
                "SELECT * FROM orders WHERE name = ? AND status = ?",
                &[json!("payload\\"), json!("new")]
            ),
            r"SELECT * FROM orders WHERE name = 'payload\' AND status = 'new'"
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
    fn escapes_every_element_of_an_array_parameter() {
        assert_eq!(
            driver().prepare_query_with_params(
                "SELECT * FROM orders WHERE status IN (?)",
                &[json!(["it's", "b"])]
            ),
            "SELECT * FROM orders WHERE status IN ('it''s', 'b')"
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

    // --- configuration ---

    #[test]
    fn url_rules() {
        assert_eq!(
            config("broker", Some(8099)).url().unwrap(),
            "http://broker:8099/query/sql"
        );
        let mut c = config("broker", Some(443));
        c.use_ssl = true;
        assert_eq!(c.url().unwrap(), "https://broker:443/query/sql");
        // An explicit scheme wins over CUBEJS_DB_SSL.
        assert_eq!(
            config("HTTPS://broker", Some(8099)).url().unwrap(),
            "HTTPS://broker:8099/query/sql"
        );
        assert!(config("broker", None).url().is_err());
    }

    #[test]
    fn env_configuration() {
        let env: HashMap<String, String> = [
            ("CUBEJS_DB_HOST", "broker"),
            ("CUBEJS_DB_PORT", "8099"),
            ("CUBEJS_DB_NAME", "db1"),
            ("CUBEJS_DB_PINOT_AUTH_TOKEN", "tok"),
            ("CUBEJS_DB_PINOT_NULL_HANDLING", "true"),
            ("CUBEJS_DB_SSL", "true"),
            ("CUBEJS_DB_QUERY_TIMEOUT", "30s"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let driver = DriverConfig::from_env_source(&env, None, false).unwrap();
        let mut config = PinotConfig::from_driver_config(driver);
        config.apply_env_source(&env).unwrap();
        assert_eq!(config.url().unwrap(), "https://broker:8099/query/sql");
        assert_eq!(
            config.authorization_headers(),
            vec![
                ("Authorization", "Bearer tok".to_string()),
                ("database", "db1".to_string())
            ]
        );
        assert_eq!(
            config.query_options(),
            "useMultistageEngine=true;enableNullHandling=true;timeoutMs=30000"
        );

        let env: HashMap<String, String> = [("CUBEJS_DB_PINOT_NULL_HANDLING", "maybe")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let driver = DriverConfig::from_env_source(&env, None, false).unwrap();
        let err = PinotConfig::from_driver_config(driver)
            .apply_env_source(&env)
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "The CUBEJS_DB_PINOT_NULL_HANDLING must be either 'true' or 'false'."
        );
    }

    #[test]
    fn basic_auth_and_defaults() {
        let mut c = config("b", Some(1));
        assert!(c.authorization_headers().is_empty());
        c.user = Some("u".into());
        c.password = Some("p".into());
        c.database = Some("ignored-without-token".into());
        assert_eq!(
            c.authorization_headers(),
            vec![("Authorization", "Basic dTpw".to_string())]
        );
        assert_eq!(
            c.query_options(),
            "useMultistageEngine=true;enableNullHandling=false;timeoutMs=600000"
        );
    }

    #[test]
    fn driver_contract() {
        let driver = driver();
        assert!(driver.read_only());
        assert_eq!(driver.quote_identifier("a.b"), "a.b");
        assert_eq!(driver.param(0), "?");
        for (pinot, generic) in [
            ("STRING", "text"),
            ("INT", "int"),
            ("LONG", "bigint"),
            ("FLOAT", "double"),
            ("DOUBLE", "double"),
            ("BIG_DECIMAL", "decimal"),
            ("BOOLEAN", "boolean"),
            ("TIMESTAMP", "timestamp"),
            ("JSON", "text"),
            ("BYTES", "text"),
            ("varchar", "text"),
            ("INT_ARRAY", "INT_ARRAY"),
        ] {
            assert_eq!(
                driver.to_generic_type(pinot, None, None).to_string(),
                generic
            );
        }
        assert_eq!(DEFAULT_CONCURRENCY, 10);
    }

    // --- HTTP ---

    #[tokio::test]
    async fn queries_the_broker() {
        let server = start(new_log(), |_| {
            (
                200,
                json!({
                    "exceptions": [],
                    "resultTable": {
                        "dataSchema": { "columnDataTypes": ["STRING", "LONG"], "columnNames": ["name", "cnt"] },
                        "rows": [["a", 1], ["b", 2]]
                    }
                })
                .to_string(),
            )
        })
        .await;
        let mut config = config("127.0.0.1", Some(server.port()));
        config.password = Some("p".into());
        config.user = Some("u".into());
        let driver = PinotDriver::new(config).unwrap();
        let result = driver
            .query(
                "SELECT name, count(*) AS cnt FROM t WHERE name = ?",
                &[json!("x")],
                &QueryOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            result.columns,
            vec![Column::new("name", "text"), Column::new("cnt", "bigint")]
        );
        assert_eq!(
            result.rows,
            vec![vec![json!("a"), json!(1)], vec![json!("b"), json!(2)]]
        );

        let request = &server.requests()[0];
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/query/sql");
        assert_eq!(request.header("Content-Type"), Some("application/json"));
        assert_eq!(request.header("Authorization"), Some("Basic dTpw"));
        let body: Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(
            body,
            json!({
                "sql": "SELECT name, count(*) AS cnt FROM t WHERE name = 'x'",
                "queryOptions": "useMultistageEngine=true;enableNullHandling=false;timeoutMs=600000"
            })
        );

        let DownloadedData::Memory(data) = driver
            .download_query_results("SELECT 1", &[], &DownloadQueryResultsOptions::default())
            .await
            .unwrap()
        else {
            panic!("expected memory data");
        };
        assert_eq!(data.columns[1], Column::new("cnt", "bigint"));
        driver.test_connection().await.unwrap();
    }

    #[tokio::test]
    async fn errors() {
        let server = start(new_log(), |_| {
            (
                200,
                json!({ "exceptions": [{ "errorCode": 150, "message": "SQLParsingError: bad" }] })
                    .to_string(),
            )
        })
        .await;
        let driver = PinotDriver::new(config("127.0.0.1", Some(server.port()))).unwrap();
        let err = driver
            .query("SELEC", &[], &QueryOptions::default())
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "SQLParsingError: bad");

        let server = start(new_log(), |_| (401, String::new())).await;
        let driver = PinotDriver::new(config("127.0.0.1", Some(server.port()))).unwrap();
        let err = driver
            .query("SELECT 1", &[], &QueryOptions::default())
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "Unauthorized request");

        let server = start(new_log(), |_| (500, String::new())).await;
        let driver = PinotDriver::new(config("127.0.0.1", Some(server.port()))).unwrap();
        let err = driver
            .query("SELECT 1", &[], &QueryOptions::default())
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "Unexpected error");

        let server = start(new_log(), |_| {
            (
                200,
                json!({ "resultTable": { "dataSchema": { "columnDataTypes": [], "columnNames": [] }, "rows": [] } })
                    .to_string(),
            )
        })
        .await;
        let driver = PinotDriver::new(config("127.0.0.1", Some(server.port()))).unwrap();
        let err = driver.test_connection().await.unwrap_err();
        assert!(err
            .to_string()
            .ends_with("Unable to connect to your Pinot instance"));
    }
}
