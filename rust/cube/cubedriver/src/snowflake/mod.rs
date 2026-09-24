//! Snowflake driver: port of `@cubejs-backend/snowflake-driver` on top of the
//! Snowflake SQL REST API (`reqwest` with rustls).
//!
//! The Node driver uses `snowflake-sdk`; this port speaks the public SQL API
//! (`POST /api/v2/statements`) instead, which is the same protocol without the
//! SDK. Consequences worth knowing:
//!
//! * Authentication is key-pair (`KEYPAIR_JWT`) or OAuth — the SQL API does
//!   not accept a user name and password, so `CUBEJS_DB_PASS` alone is
//!   rejected with a configuration error.
//! * Encrypted private keys (`CUBEJS_DB_SNOWFLAKE_PRIVATE_KEY_PASS`) are not
//!   supported yet.
//! * Export-bucket unloads (`COPY INTO 's3://…'`) are not implemented; the
//!   generated SQL is there but collecting the files needs a cloud storage
//!   client.
//! * Result column names are lower-cased, matching `getTypes` in the Node
//!   driver (rows are positional here, so nothing is matched by name).

pub mod auth;
pub mod types;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use tokio::sync::Mutex;

use crate::config::DriverConfig;
use crate::driver::Driver;
use crate::error::{DriverError, Result};
use crate::types::{
    Column, DownloadQueryResultsOptions, DownloadedData, DriverCapabilities, GenericType,
    QueryOptions, QueryResult, Row, TableCsvData, TableStructure, UnloadOptions,
};

pub use auth::Authentication;

/// `getDefaultConcurrency`.
pub const DEFAULT_CONCURRENCY: usize = 8;
/// Default path of the OAuth token file inside Snowpark Container Services.
pub const DEFAULT_OAUTH_TOKEN_PATH: &str = "/snowflake/session/token";

/// Configuration of [`SnowflakeDriver`] (`SnowflakeDriverOptions`).
#[derive(Debug, Clone, Default)]
pub struct SnowflakeConfig {
    /// Connection settings and global driver knobs.
    pub driver: DriverConfig,
    /// `CUBEJS_DB_SNOWFLAKE_ACCOUNT`.
    pub account: Option<String>,
    /// `CUBEJS_DB_SNOWFLAKE_REGION`.
    pub region: Option<String>,
    /// `CUBEJS_DB_SNOWFLAKE_HOST` (overrides the account based host).
    pub host: Option<String>,
    /// `CUBEJS_DB_SNOWFLAKE_WAREHOUSE`.
    pub warehouse: Option<String>,
    /// `CUBEJS_DB_SNOWFLAKE_ROLE`.
    pub role: Option<String>,
    /// `CUBEJS_DB_NAME`.
    pub database: Option<String>,
    /// `CUBEJS_DB_SCHEMA`.
    pub schema: Option<String>,
    /// `CUBEJS_DB_USER`.
    pub username: Option<String>,
    /// `CUBEJS_DB_PASS` (unusable with the SQL API, see the module docs).
    pub password: Option<String>,
    /// `CUBEJS_DB_SNOWFLAKE_AUTHENTICATOR`.
    pub authenticator: Option<String>,
    /// `CUBEJS_DB_SNOWFLAKE_OAUTH_TOKEN`.
    pub oauth_token: Option<String>,
    /// `CUBEJS_DB_SNOWFLAKE_OAUTH_TOKEN_PATH`.
    pub oauth_token_path: Option<String>,
    /// `CUBEJS_DB_SNOWFLAKE_PRIVATE_KEY`.
    pub private_key: Option<String>,
    /// `CUBEJS_DB_SNOWFLAKE_PRIVATE_KEY_PATH`.
    pub private_key_path: Option<String>,
    /// `CUBEJS_DB_SNOWFLAKE_PRIVATE_KEY_PASS`.
    pub private_key_pass: Option<String>,
    /// `CUBEJS_DB_SNOWFLAKE_CLIENT_SESSION_KEEP_ALIVE`.
    pub client_session_keep_alive: bool,
    /// `CUBEJS_DB_SNOWFLAKE_QUOTED_IDENTIFIERS_IGNORE_CASE`.
    pub ident_ignore_case: bool,
    /// `executionTimeout` (`CUBEJS_DB_QUERY_TIMEOUT`).
    pub execution_timeout: Duration,
    /// `CUBEJS_DB_EXPORT_BUCKET` (unload is not implemented yet).
    pub export_bucket: Option<String>,
    /// `readOnly` (default `false`).
    pub read_only: bool,
}

impl SnowflakeConfig {
    /// Builds the Snowflake configuration from a generic [`DriverConfig`].
    pub fn from_driver_config(driver: DriverConfig) -> Self {
        let ds = &driver.data_source;
        Self {
            database: ds.database.clone(),
            schema: ds.schema.clone(),
            username: ds.user.clone(),
            password: ds.password.clone(),
            execution_timeout: ds.query_timeout,
            ..Self {
                driver: driver.clone(),
                ..Default::default()
            }
        }
    }

    /// Reads the configuration of `data_source` from the environment.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        let driver = DriverConfig::from_env(data_source)?;
        let mut config = Self::from_driver_config(driver);
        config.apply_env()?;
        Ok(config)
    }

    /// Reads the `CUBEJS_DB_SNOWFLAKE_*` variables into an existing configuration.
    pub fn apply_env(&mut self) -> Result<()> {
        let config = self;
        config.account = env_var("CUBEJS_DB_SNOWFLAKE_ACCOUNT");
        config.region = env_var("CUBEJS_DB_SNOWFLAKE_REGION");
        config.host = env_var("CUBEJS_DB_SNOWFLAKE_HOST");
        config.warehouse = env_var("CUBEJS_DB_SNOWFLAKE_WAREHOUSE");
        config.role = env_var("CUBEJS_DB_SNOWFLAKE_ROLE");
        config.authenticator = env_var("CUBEJS_DB_SNOWFLAKE_AUTHENTICATOR");
        config.oauth_token = env_var("CUBEJS_DB_SNOWFLAKE_OAUTH_TOKEN");
        config.oauth_token_path = env_var("CUBEJS_DB_SNOWFLAKE_OAUTH_TOKEN_PATH");
        config.private_key = env_var("CUBEJS_DB_SNOWFLAKE_PRIVATE_KEY");
        config.private_key_path = env_var("CUBEJS_DB_SNOWFLAKE_PRIVATE_KEY_PATH");
        config.private_key_pass = env_var("CUBEJS_DB_SNOWFLAKE_PRIVATE_KEY_PASS");
        config.client_session_keep_alive =
            env_bool("CUBEJS_DB_SNOWFLAKE_CLIENT_SESSION_KEEP_ALIVE")?;
        config.ident_ignore_case = env_bool("CUBEJS_DB_SNOWFLAKE_QUOTED_IDENTIFIERS_IGNORE_CASE")?;
        config.export_bucket = env_var("CUBEJS_DB_EXPORT_BUCKET");
        Ok(())
    }

    /// `https://<account>[.<region>].snowflakecomputing.com`.
    pub fn base_url(&self) -> Result<String> {
        if let Some(host) = &self.host {
            let host = host.trim_end_matches('/');
            return Ok(if host.starts_with("http") {
                host.to_string()
            } else {
                format!("https://{host}")
            });
        }
        let account = self
            .account
            .as_deref()
            .filter(|a| !a.is_empty())
            .ok_or_else(|| {
                DriverError::Config(
                    "CUBEJS_DB_SNOWFLAKE_ACCOUNT is not set for the Snowflake driver.".to_string(),
                )
            })?;
        let host = match &self.region {
            Some(region) if !region.is_empty() && !account.contains('.') => {
                format!("{account}.{region}")
            }
            _ => account.to_string(),
        };
        Ok(format!("https://{host}.snowflakecomputing.com"))
    }

    /// Resolves how the driver authenticates (`prepareConnectOptions`).
    pub fn authentication(&self) -> Result<Authentication> {
        let authenticator = self
            .authenticator
            .as_deref()
            .map(|a| a.to_uppercase())
            .unwrap_or_default();

        if authenticator == "OAUTH" {
            let token = match &self.oauth_token {
                Some(token) if !token.is_empty() => token.clone(),
                _ => {
                    let path = self
                        .oauth_token_path
                        .clone()
                        .unwrap_or_else(|| DEFAULT_OAUTH_TOKEN_PATH.to_string());
                    std::fs::read_to_string(&path)
                        .map_err(|_| {
                            DriverError::Config(format!(
                                "File {path} provided by CUBEJS_DB_SNOWFLAKE_OAUTH_TOKEN_PATH \
                                 does not exist."
                            ))
                        })?
                        .trim()
                        .to_string()
                }
            };
            return Ok(Authentication::OAuth { token });
        }

        if let Some(private_key) = self.resolve_private_key()? {
            return Ok(Authentication::KeyPair { private_key });
        }

        Err(DriverError::Config(
            "The Rust Snowflake driver talks to the SQL REST API, which only accepts key-pair \
             (CUBEJS_DB_SNOWFLAKE_PRIVATE_KEY / _PATH) or OAuth \
             (CUBEJS_DB_SNOWFLAKE_AUTHENTICATOR=OAUTH) authentication. \
             A user name and password cannot be used."
                .to_string(),
        ))
    }

    /// The private key, read from `CUBEJS_DB_SNOWFLAKE_PRIVATE_KEY` or its path.
    pub fn resolve_private_key(&self) -> Result<Option<String>> {
        let key = match (&self.private_key, &self.private_key_path) {
            (Some(key), _) if !key.is_empty() => Some(key.clone()),
            (_, Some(path)) if !path.is_empty() => {
                Some(std::fs::read_to_string(path).map_err(|e| {
                    DriverError::Config(format!(
                        "Unable to read CUBEJS_DB_SNOWFLAKE_PRIVATE_KEY_PATH \"{path}\": {e}"
                    ))
                })?)
            }
            _ => None,
        };
        if let Some(key) = &key {
            if key.contains("BEGIN ENCRYPTED PRIVATE KEY") {
                if self.private_key_pass.is_none() {
                    return Err(DriverError::Config(
                        "Snowflake encrypted private key provided, but no passphrase was given."
                            .to_string(),
                    ));
                }
                return Err(DriverError::Config(
                    "Encrypted Snowflake private keys are not supported by the Rust driver yet: \
                     decrypt the key with `openssl pkcs8 -in key.p8 -out key.pem` first."
                        .to_string(),
                ));
            }
        }
        Ok(key)
    }
}

fn env_var(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn env_bool(key: &str) -> Result<bool> {
    match env_var(key) {
        Some(v) => match v.to_ascii_lowercase().as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(DriverError::Config(format!(
                "The {key} must be either 'true' or 'false'."
            ))),
        },
        None => Ok(false),
    }
}

/// `resultSetMetaData.rowType` entry.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RowType {
    #[serde(default)]
    pub name: String,
    #[serde(rename = "type", default)]
    pub type_: String,
    #[serde(default)]
    pub precision: Option<i64>,
    #[serde(default)]
    pub scale: Option<i64>,
}

/// `resultSetMetaData`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultSetMetaData {
    #[serde(default)]
    pub num_rows: Option<i64>,
    #[serde(default)]
    pub row_type: Vec<RowType>,
    #[serde(default)]
    pub partition_info: Vec<Value>,
}

/// A `POST /api/v2/statements` response.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementResponse {
    #[serde(default)]
    pub result_set_meta_data: Option<ResultSetMetaData>,
    #[serde(default)]
    pub data: Vec<Vec<Value>>,
    #[serde(default)]
    pub statement_handle: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
}

/// Snowflake driver.
pub struct SnowflakeDriver {
    config: SnowflakeConfig,
    client: reqwest::Client,
    base_url: String,
    /// Cached key-pair assertion (`(token, expires at)`).
    jwt: Mutex<Option<(String, u64)>>,
}

impl std::fmt::Debug for SnowflakeDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SnowflakeDriver")
            .field("base_url", &self.base_url)
            .field("database", &self.config.database)
            .field("warehouse", &self.config.warehouse)
            .finish()
    }
}

impl SnowflakeDriver {
    /// Creates the driver. Nothing is sent until the first query.
    pub fn new(config: SnowflakeConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(format!("CubeDev_Cube/{}", env!("CARGO_PKG_VERSION")))
            .timeout(config.execution_timeout + Duration::from_secs(30))
            .build()
            .map_err(|e| DriverError::Config(format!("Unable to build the HTTP client: {e}")))?;
        // A driver without an account can still be constructed (like every
        // other driver here); the error surfaces on first use.
        let base_url = config.base_url().unwrap_or_default();
        Ok(Self {
            config,
            client,
            base_url,
            jwt: Mutex::new(None),
        })
    }

    /// Creates the driver from `CUBEJS_DB_*` / `CUBEJS_DB_SNOWFLAKE_*`.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Self::new(SnowflakeConfig::from_env(data_source)?)
    }

    /// The driver configuration.
    pub fn snowflake_config(&self) -> &SnowflakeConfig {
        &self.config
    }

    /// Port of `buildAlterSessionSql`. The SQL API carries these as request
    /// parameters, but the statement is kept for parity (and for callers that
    /// want to pin a session explicitly).
    pub fn build_alter_session_sql(&self) -> String {
        let mut assignments = vec![
            "TIMEZONE = 'UTC'".to_string(),
            format!(
                "STATEMENT_TIMEOUT_IN_SECONDS = {}",
                self.config.execution_timeout.as_secs()
            ),
        ];
        assignments.push(format!(
            "QUOTED_IDENTIFIERS_IGNORE_CASE = {}",
            self.config.ident_ignore_case
        ));
        format!("ALTER SESSION SET {}", assignments.join(", "))
    }

    /// Session parameters sent with every statement.
    fn session_parameters(&self) -> Map<String, Value> {
        let mut parameters = Map::new();
        parameters.insert("TIMEZONE".to_string(), Value::from("UTC"));
        parameters.insert(
            "STATEMENT_TIMEOUT_IN_SECONDS".to_string(),
            Value::from(self.config.execution_timeout.as_secs()),
        );
        parameters.insert(
            "QUOTED_IDENTIFIERS_IGNORE_CASE".to_string(),
            Value::from(self.config.ident_ignore_case),
        );
        parameters.insert(
            "CLIENT_SESSION_KEEP_ALIVE".to_string(),
            Value::from(self.config.client_session_keep_alive),
        );
        parameters
    }

    /// The bearer token for one request.
    async fn token(&self) -> Result<(String, &'static str)> {
        let authentication = self.config.authentication()?;
        match authentication {
            Authentication::OAuth { token } => Ok((token, "OAUTH")),
            Authentication::KeyPair { private_key } => {
                let now = auth_now();
                let mut cached = self.jwt.lock().await;
                if let Some((token, expires_at)) = cached.as_ref() {
                    if *expires_at > now + 60 {
                        return Ok((token.clone(), "KEYPAIR_JWT"));
                    }
                }
                let account = self.config.account.clone().unwrap_or_default();
                let user = self.config.username.clone().ok_or_else(|| {
                    DriverError::Config(
                        "CUBEJS_DB_USER is required for Snowflake key-pair authentication."
                            .to_string(),
                    )
                })?;
                let token = auth::build_jwt(&account, &user, &private_key, now)?;
                *cached = Some((token.clone(), now + auth::JWT_LIFETIME_SECONDS));
                Ok((token, "KEYPAIR_JWT"))
            }
        }
    }

    fn base(&self) -> Result<&str> {
        if self.base_url.is_empty() {
            self.config.base_url()?;
        }
        Ok(&self.base_url)
    }

    async fn send<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<(u16, T)> {
        let (token, token_type) = self.token().await?;
        let response = request
            .bearer_auth(token)
            .header("X-Snowflake-Authorization-Token-Type", token_type)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| DriverError::Connection {
                pool_name: "snowflake".to_string(),
                message: e.to_string(),
            })?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| DriverError::Query(e.to_string()))?;
        if !status.is_success() && status.as_u16() != 202 {
            return Err(snowflake_error(status, &body));
        }
        let parsed = serde_json::from_str(&body).map_err(|e| {
            DriverError::Query(format!("Unexpected Snowflake response: {e}: {body}"))
        })?;
        Ok((status.as_u16(), parsed))
    }

    /// Submits a statement and waits for it to finish.
    async fn execute(&self, sql: &str, params: &[Value]) -> Result<StatementResponse> {
        let mut body = json!({
            "statement": sql,
            "timeout": self.config.execution_timeout.as_secs(),
            "parameters": Value::Object(self.session_parameters()),
        });
        if let Some(database) = &self.config.database {
            body["database"] = Value::String(database.clone());
        }
        if let Some(schema) = &self.config.schema {
            body["schema"] = Value::String(schema.clone());
        }
        if let Some(warehouse) = &self.config.warehouse {
            body["warehouse"] = Value::String(warehouse.clone());
        }
        if let Some(role) = &self.config.role {
            body["role"] = Value::String(role.clone());
        }
        if !params.is_empty() {
            body["bindings"] = Value::Object(bindings(params));
        }

        let url = format!("{}/api/v2/statements", self.base()?);
        let (status, response): (u16, StatementResponse) =
            self.send(self.client.post(&url).json(&body)).await?;
        if status != 202 {
            return Ok(response);
        }

        // Asynchronous execution: poll the statement handle.
        let handle = response.statement_handle.ok_or_else(|| {
            DriverError::Query("Snowflake accepted the statement without a handle".to_string())
        })?;
        let started = Instant::now();
        let mut i = 0u32;
        loop {
            let pause = Duration::from_millis(200 * u64::from(i))
                .min(self.config.driver.data_source.poll_max_interval);
            tokio::time::sleep(pause).await;
            i += 1;

            let url = format!("{}/api/v2/statements/{handle}", self.base()?);
            let (status, response): (u16, StatementResponse) =
                self.send(self.client.get(&url)).await?;
            if status != 202 {
                return Ok(response);
            }
            if started.elapsed() > self.config.execution_timeout {
                return Err(DriverError::Query(format!(
                    "Snowflake statement timeout reached {}ms",
                    self.config.execution_timeout.as_millis()
                )));
            }
        }
    }

    /// Fetches the remaining partitions of a result set.
    async fn fetch_partition(&self, handle: &str, partition: usize) -> Result<Vec<Vec<Value>>> {
        let url = format!("{}/api/v2/statements/{handle}", self.base()?);
        let (_, response): (u16, StatementResponse) = self
            .send(
                self.client
                    .get(&url)
                    .query(&[("partition", partition.to_string())]),
            )
            .await?;
        Ok(response.data)
    }

    /// Runs `sql` and materialises every partition.
    async fn query_response(&self, sql: &str, params: &[Value]) -> Result<QueryResult> {
        let response = self.execute(sql, params).await?;
        let metadata = response.result_set_meta_data.clone().unwrap_or_default();
        let columns = self.columns(&metadata.row_type);

        let mut rows: Vec<Row> = response
            .data
            .iter()
            .map(|row| hydrate_row(row, &metadata.row_type))
            .collect();

        if metadata.partition_info.len() > 1 {
            if let Some(handle) = &response.statement_handle {
                for partition in 1..metadata.partition_info.len() {
                    let data = self.fetch_partition(handle, partition).await?;
                    rows.extend(data.iter().map(|row| hydrate_row(row, &metadata.row_type)));
                }
            }
        }

        Ok(QueryResult::new(columns, rows))
    }

    /// `getTypes`: column names are lower-cased, numbers with scale 0 are `int`.
    fn columns(&self, row_type: &[RowType]) -> Vec<Column> {
        row_type
            .iter()
            .map(|c| {
                Column::new(
                    c.name.to_lowercase(),
                    types::column_type(
                        &c.type_,
                        c.precision,
                        c.scale,
                        self.config.driver.precise_decimal_in_cubestore,
                    ),
                )
            })
            .collect()
    }
}

fn auth_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `bindings`: positional parameters, 1-based, as `{ "1": { type, value } }`.
pub fn bindings(params: &[Value]) -> Map<String, Value> {
    params
        .iter()
        .enumerate()
        .map(|(i, value)| {
            let (type_, value) = match value {
                Value::Null => ("TEXT", Value::Null),
                Value::Bool(b) => ("BOOLEAN", Value::String(b.to_string())),
                Value::Number(n) if n.is_f64() => ("REAL", Value::String(n.to_string())),
                Value::Number(n) => ("FIXED", Value::String(n.to_string())),
                Value::String(s) => ("TEXT", Value::String(s.clone())),
                other => ("TEXT", Value::String(other.to_string())),
            };
            (
                (i + 1).to_string(),
                json!({ "type": type_, "value": value }),
            )
        })
        .collect()
}

/// Hydrates one row of the `data` matrix.
fn hydrate_row(row: &[Value], row_type: &[RowType]) -> Row {
    row_type
        .iter()
        .enumerate()
        .map(|(i, column)| match row.get(i) {
            Some(value) => types::hydrate(value, &column.type_),
            None => Value::Null,
        })
        .collect()
}

/// Unwraps the `{ "message": ..., "code": ... }` error envelope.
fn snowflake_error(status: reqwest::StatusCode, body: &str) -> DriverError {
    #[derive(Deserialize)]
    struct ErrorBody {
        message: Option<String>,
        code: Option<String>,
    }
    match serde_json::from_str::<ErrorBody>(body) {
        Ok(error) if error.message.is_some() => DriverError::Database {
            message: error.message.unwrap_or_default(),
            code: error.code.or_else(|| Some(status.as_u16().to_string())),
        },
        _ => DriverError::Database {
            message: body.trim().to_string(),
            code: Some(status.as_u16().to_string()),
        },
    }
}

#[async_trait]
impl Driver for SnowflakeDriver {
    fn config(&self) -> &DriverConfig {
        &self.config.driver
    }

    async fn test_connection(&self) -> Result<()> {
        self.query("SELECT 1", &[], &QueryOptions::default())
            .await?;
        Ok(())
    }

    async fn query(
        &self,
        sql: &str,
        params: &[Value],
        _options: &QueryOptions,
    ) -> Result<QueryResult> {
        self.query_response(sql, params).await
    }

    fn to_generic_type(
        &self,
        db_type: &str,
        precision: Option<i64>,
        scale: Option<i64>,
    ) -> GenericType {
        types::to_generic_type(
            db_type,
            precision,
            scale,
            self.config.driver.precise_decimal_in_cubestore,
        )
    }

    fn read_only(&self) -> bool {
        self.config.read_only
    }

    fn capabilities(&self) -> DriverCapabilities {
        DriverCapabilities {
            unload_without_temp_table: true,
            incremental_schema_loading: true,
            ..Default::default()
        }
    }

    fn information_schema_query(&self) -> String {
        "
        SELECT COLUMNS.COLUMN_NAME as \"column_name\",
               COLUMNS.TABLE_NAME as \"table_name\",
               COLUMNS.TABLE_SCHEMA as \"table_schema\",
               CASE WHEN COLUMNS.NUMERIC_SCALE = 0 AND COLUMNS.DATA_TYPE = 'NUMBER' THEN 'int' ELSE COLUMNS.DATA_TYPE END as \"data_type\"
        FROM INFORMATION_SCHEMA.COLUMNS
        WHERE COLUMNS.TABLE_SCHEMA NOT IN ('INFORMATION_SCHEMA')
     "
        .to_string()
    }

    async fn table_column_types(&self, table: &str) -> Result<TableStructure> {
        let name = crate::types::TableName::split(table);
        let table_name = name.name.split('.').next().unwrap_or("").to_uppercase();
        let schema = name.schema.to_uppercase();
        let result = self
            .query(
                &format!(
                    "SELECT COLUMNS.COLUMN_NAME,
        CASE
          WHEN
            COLUMNS.NUMERIC_SCALE = 0 AND
            COLUMNS.DATA_TYPE = 'NUMBER'
          THEN 'int'
          ELSE COLUMNS.DATA_TYPE
        END as DATA_TYPE,
        COLUMNS.NUMERIC_PRECISION,
        COLUMNS.NUMERIC_SCALE
      FROM INFORMATION_SCHEMA.COLUMNS
      WHERE
        TABLE_NAME = {} AND
        TABLE_SCHEMA = {}
      ORDER BY ORDINAL_POSITION",
                    self.param(0),
                    self.param(1)
                ),
                &[Value::from(table_name), Value::from(schema)],
                &QueryOptions::default(),
            )
            .await?;

        Ok((0..result.len())
            .filter_map(|i| {
                let name = result.get_string(i, "column_name")?;
                let data_type = result.get_string(i, "data_type")?;
                Some(Column::new(
                    name,
                    self.to_generic_type(
                        &data_type,
                        result.get_i64(i, "numeric_precision"),
                        result.get_i64(i, "numeric_scale"),
                    ),
                ))
            })
            .collect())
    }

    async fn query_column_types(
        &self,
        sql: &str,
        params: &[Value],
        _options: &QueryOptions,
    ) -> Result<TableStructure> {
        let response = self.execute(&format!("{sql} LIMIT 0"), params).await?;
        let metadata = response.result_set_meta_data.unwrap_or_default();
        Ok(self.columns(&metadata.row_type))
    }

    async fn get_tables_query(&self, schema_name: &str) -> Result<Vec<String>> {
        let result = self
            .query(
                &format!(
                    "SELECT table_name FROM information_schema.tables WHERE table_schema = {}",
                    self.param(0)
                ),
                &[Value::from(schema_name.to_uppercase())],
                &QueryOptions::default(),
            )
            .await?;
        Ok((0..result.len())
            .filter_map(|i| result.get_string(i, "table_name").map(|t| t.to_lowercase()))
            .collect())
    }

    async fn download_query_results(
        &self,
        sql: &str,
        params: &[Value],
        _options: &DownloadQueryResultsOptions,
    ) -> Result<DownloadedData> {
        Ok(DownloadedData::Memory(
            self.query_response(sql, params).await?,
        ))
    }

    async fn is_unload_supported(&self, _options: &UnloadOptions) -> Result<bool> {
        // `COPY INTO '<bucket>'` needs a cloud storage client to collect and
        // sign the resulting files.
        Ok(false)
    }

    async fn unload(&self, _table: &str, _options: &UnloadOptions) -> Result<TableCsvData> {
        Err(DriverError::NotImplemented(
            "Snowflake unload to an export bucket is not implemented in the Rust driver yet."
                .to_string(),
        ))
    }
}

/// Sorted view of the session parameters, for assertions and debugging.
impl SnowflakeDriver {
    pub fn session_parameters_map(&self) -> BTreeMap<String, Value> {
        self.session_parameters().into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_KEY: &str = include_str!("../../test/fixtures/rsa_test_key.pem");

    fn config() -> SnowflakeConfig {
        let mut config = SnowflakeConfig::from_driver_config(DriverConfig::default());
        config.account = Some("myorg-account1".to_string());
        config.username = Some("cube".to_string());
        config.private_key = Some(TEST_KEY.to_string());
        config.database = Some("TEST_DB".to_string());
        config.warehouse = Some("TEST_WH".to_string());
        config
    }

    #[test]
    fn base_urls() {
        let mut config = config();
        assert_eq!(
            config.base_url().unwrap(),
            "https://myorg-account1.snowflakecomputing.com"
        );
        config.region = Some("eu-central-1".to_string());
        assert_eq!(
            config.base_url().unwrap(),
            "https://myorg-account1.eu-central-1.snowflakecomputing.com"
        );
        config.host = Some("my.host.example.com".to_string());
        assert_eq!(config.base_url().unwrap(), "https://my.host.example.com");

        let mut empty = SnowflakeConfig::from_driver_config(DriverConfig::default());
        empty.account = None;
        let err = empty.base_url().unwrap_err();
        assert!(err.to_string().contains("CUBEJS_DB_SNOWFLAKE_ACCOUNT"));
    }

    #[test]
    fn authentication_resolution() {
        let config = config();
        assert!(matches!(
            config.authentication().unwrap(),
            Authentication::KeyPair { .. }
        ));

        let mut oauth = config.clone();
        oauth.authenticator = Some("oauth".to_string());
        oauth.oauth_token = Some("token".to_string());
        assert_eq!(
            oauth.authentication().unwrap(),
            Authentication::OAuth {
                token: "token".to_string()
            }
        );

        let mut password = SnowflakeConfig::from_driver_config(DriverConfig::default());
        password.account = Some("a".to_string());
        password.username = Some("u".to_string());
        password.password = Some("p".to_string());
        let err = password.authentication().unwrap_err();
        assert!(err.to_string().contains("key-pair"));
        assert!(err.to_string().contains("password cannot be used"));
    }

    #[test]
    fn encrypted_keys_are_reported() {
        let mut config = config();
        config.private_key = Some(
            "-----BEGIN ENCRYPTED PRIVATE KEY-----\nAA\n-----END ENCRYPTED PRIVATE KEY-----".into(),
        );
        let err = config.resolve_private_key().unwrap_err();
        assert!(err.to_string().contains("no passphrase was given"));

        config.private_key_pass = Some("pass".into());
        let err = config.resolve_private_key().unwrap_err();
        assert!(err.to_string().contains("not supported"));
    }

    #[tokio::test]
    async fn sql_and_type_mapping() {
        let driver = SnowflakeDriver::new(config()).unwrap();
        assert_eq!(driver.param(0), "?");
        assert_eq!(driver.quote_identifier("a"), "\"a\"");
        assert!(!driver.read_only());
        assert!(driver.capabilities().unload_without_temp_table);
        assert!(driver.capabilities().incremental_schema_loading);

        let q = driver.information_schema_query();
        assert!(q.contains("COLUMNS.COLUMN_NAME as \"column_name\""));
        assert!(q.contains(
            "CASE WHEN COLUMNS.NUMERIC_SCALE = 0 AND COLUMNS.DATA_TYPE = 'NUMBER' THEN 'int' ELSE COLUMNS.DATA_TYPE END as \"data_type\""
        ));
        assert!(q.contains("WHERE COLUMNS.TABLE_SCHEMA NOT IN ('INFORMATION_SCHEMA')"));

        assert_eq!(
            driver.to_generic_type("TIMESTAMP_LTZ", None, None),
            GenericType::Timestamp
        );
        assert_eq!(
            driver.to_generic_type("OBJECT", None, None),
            GenericType::Other("HLL_SNOWFLAKE".into())
        );
        assert_eq!(
            driver.build_alter_session_sql(),
            "ALTER SESSION SET TIMEZONE = 'UTC', STATEMENT_TIMEOUT_IN_SECONDS = 600, QUOTED_IDENTIFIERS_IGNORE_CASE = false"
        );
        assert_eq!(
            driver.session_parameters_map()["STATEMENT_TIMEOUT_IN_SECONDS"],
            Value::from(600)
        );
        assert!(!driver
            .is_unload_supported(&UnloadOptions::default())
            .await
            .unwrap());
    }

    #[test]
    fn bindings_are_positional_and_typed() {
        let map = bindings(&[
            Value::from("x"),
            Value::from(7),
            Value::from(1.5),
            Value::Bool(true),
            Value::Null,
        ]);
        assert_eq!(map["1"], json!({ "type": "TEXT", "value": "x" }));
        assert_eq!(map["2"], json!({ "type": "FIXED", "value": "7" }));
        assert_eq!(map["3"], json!({ "type": "REAL", "value": "1.5" }));
        assert_eq!(map["4"], json!({ "type": "BOOLEAN", "value": "true" }));
        assert_eq!(map["5"], json!({ "type": "TEXT", "value": null }));
    }

    #[test]
    fn rows_are_hydrated_against_the_row_type() {
        let row_type: Vec<RowType> = serde_json::from_value(json!([
            { "name": "ID", "type": "fixed", "precision": 38, "scale": 0 },
            { "name": "AMOUNT", "type": "fixed", "precision": 10, "scale": 2 },
            { "name": "CREATED_AT", "type": "timestamp_ntz", "precision": 0, "scale": 9 },
            { "name": "OK", "type": "boolean" }
        ]))
        .unwrap();
        let driver = SnowflakeDriver::new(config()).unwrap();
        let columns = driver.columns(&row_type);
        assert_eq!(
            columns,
            vec![
                Column::new("id", "int"),
                Column::new("amount", "decimal"),
                Column::new("created_at", "timestamp"),
                Column::new("ok", "boolean"),
            ]
        );

        let row = hydrate_row(
            &[
                Value::from("1"),
                Value::from("1.25"),
                Value::from("1577836800.000000000"),
                Value::from("true"),
            ],
            &row_type,
        );
        assert_eq!(
            row,
            vec![
                Value::from("1"),
                Value::from("1.25"),
                Value::from("2020-01-01T00:00:00.000"),
                Value::Bool(true),
            ]
        );
    }

    #[test]
    fn errors_are_unwrapped() {
        let err = snowflake_error(
            reqwest::StatusCode::UNPROCESSABLE_ENTITY,
            r#"{"code":"000904","message":"SQL compilation error"}"#,
        );
        assert_eq!(err.to_string(), "SQL compilation error");
        let err = snowflake_error(reqwest::StatusCode::BAD_GATEWAY, "oops");
        assert_eq!(err.to_string(), "oops");
    }
}
