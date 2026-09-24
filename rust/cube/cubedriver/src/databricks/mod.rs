//! Databricks driver (`CUBEJS_DB_TYPE=databricks-jdbc`): port of
//! `@cubejs-backend/databricks-jdbc-driver` on top of the Databricks SQL
//! Statement Execution REST API (`reqwest` with rustls) instead of the
//! Databricks JDBC jar, which would need a JVM.
//!
//! What stays identical to the Node.js driver:
//!
//! * configuration: `CUBEJS_DB_DATABRICKS_URL` (a JDBC URL, `jdbc:spark://`
//!   is still rewritten with a deprecation warning), `CUBEJS_DB_DATABRICKS_TOKEN`
//!   (or `PWD=` inside the URL), `CUBEJS_DB_DATABRICKS_OAUTH_CLIENT_ID` /
//!   `_SECRET` (OAuth M2M, exchanged at `/oidc/v1/token`),
//!   `CUBEJS_DB_DATABRICKS_CATALOG`, `CUBEJS_DB_NAME`, the export bucket
//!   variables and `CUBEJS_DB_POLL_MAX_INTERVAL`, all data-source aware;
//! * the SQL: `SHOW DATABASES` / `SHOW TABLES IN` / `DESCRIBE` /
//!   `DESCRIBE QUERY` introspection, back-tick quoting, catalog prefixing of
//!   the pre-aggregation schema, client-side parameter interpolation with the
//!   Spark escaping rules (`format('spark', ...)` in the JDBC driver);
//! * type mapping (`binary` → `hll_datasketches`, `decimal(10,0)` → `bigint`,
//!   `numeric(p,s)` precision), `readOnly` (true without an export bucket),
//!   capabilities and the default concurrency (10).
//!
//! Differences, all deliberate:
//!
//! * `httpPath` must name a SQL warehouse: the Statement Execution API does
//!   not run on all-purpose clusters (a named configuration error says so).
//! * Values come from the `JSON_ARRAY` format rather than JDBC getters:
//!   booleans and 8/16/32-bit integers are JSON booleans/numbers as before,
//!   `bigint` stays a string as before, but `decimal` is kept as an exact
//!   string (JDBC's `getDouble` rounded it) and timestamps use the crate-wide
//!   `YYYY-MM-DDTHH:MM:SS.sss` form instead of Java's `Timestamp.toString()`.
//! * Export-bucket unload is not implemented (see [`DatabricksDriver::unload`]):
//!   the `INSERT OVERWRITE DIRECTORY` SQL is ported, listing and signing the
//!   files in S3 / GCS / Azure is not. With an export bucket configured,
//!   `is_unload_supported` fails with a named error instead of quietly
//!   building without it.

pub mod statement;
pub mod url;

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::Value;
use tokio::sync::{Mutex, Semaphore};

use crate::config::DriverConfig;
use crate::config::{data_sources, env_key, EnvSource, ProcessEnv};
use crate::driver::Driver;
use crate::error::{DriverError, Result};
use crate::escape::{self, Dialect};
use crate::type_detection::detect_types_from_tabular;
use crate::types::{
    Column, ColumnInfo, DatabaseStructure, DownloadQueryResultsOptions, DownloadedData,
    DriverCapabilities, GenericType, QueryOptions, QueryResult, Row, SchemaColumn, SchemaName,
    SchemaTable, StreamOptions, StreamTableData, TableCsvData, TableStructure, UnloadOptions,
};

pub use statement::{Credentials, Disposition, StatementClient, StatementResponse};
pub use url::ParsedJdbcUrl;

/// `getDefaultConcurrency`.
pub const DEFAULT_CONCURRENCY: usize = 10;
/// `testConnectionTimeout` default of the JDBC driver (60 s).
pub const DEFAULT_TEST_CONNECTION_TIMEOUT: Duration = Duration::from_secs(60);
/// `SUPPORTED_BUCKET_TYPES`.
pub const SUPPORTED_BUCKET_TYPES: &[&str] = &["s3", "gcs", "azure"];
/// How long a statement request blocks before it switches to polling.
pub const WAIT_TIMEOUT: &str = "30s";

/// Configuration of [`DatabricksDriver`] (`DatabricksDriverConfiguration`).
#[derive(Debug, Clone, Default)]
pub struct DatabricksConfig {
    /// Connection settings and global driver knobs.
    pub driver: DriverConfig,
    /// `CUBEJS_DB_DATABRICKS_URL` (JDBC form).
    pub url: Option<String>,
    /// `CUBEJS_DB_DATABRICKS_TOKEN`.
    pub token: Option<String>,
    /// `CUBEJS_DB_DATABRICKS_OAUTH_CLIENT_ID`.
    pub oauth_client_id: Option<String>,
    /// `CUBEJS_DB_DATABRICKS_OAUTH_CLIENT_SECRET`.
    pub oauth_client_secret: Option<String>,
    /// `CUBEJS_DB_DATABRICKS_CATALOG`.
    pub catalog: Option<String>,
    /// `CUBEJS_DB_NAME` (restricts `tablesSchema` to one schema).
    pub database: Option<String>,
    /// `readOnly`; `None` means "true when no export bucket is configured".
    pub read_only: Option<bool>,
    /// `CUBEJS_DB_EXPORT_BUCKET_TYPE` (`s3`, `gcs` or `azure`).
    pub bucket_type: Option<String>,
    /// `CUBEJS_DB_EXPORT_BUCKET`.
    pub export_bucket: Option<String>,
    /// `CUBEJS_DB_EXPORT_BUCKET_MOUNT_DIR`.
    pub export_bucket_mount_dir: Option<String>,
    /// `CUBEJS_DB_EXPORT_BUCKET_CSV_ESCAPE_SYMBOL`.
    pub export_bucket_csv_escape_symbol: Option<String>,
    /// `CUBEJS_DB_EXPORT_BUCKET_AWS_KEY`.
    pub aws_key: Option<String>,
    /// `CUBEJS_DB_EXPORT_BUCKET_AWS_SECRET`.
    pub aws_secret: Option<String>,
    /// `CUBEJS_DB_EXPORT_BUCKET_AWS_REGION`.
    pub aws_region: Option<String>,
    /// `CUBEJS_DB_EXPORT_BUCKET_AZURE_KEY`.
    pub azure_key: Option<String>,
    /// `CUBEJS_DB_EXPORT_BUCKET_AZURE_TENANT_ID`.
    pub azure_tenant_id: Option<String>,
    /// `CUBEJS_DB_EXPORT_BUCKET_AZURE_CLIENT_ID`.
    pub azure_client_id: Option<String>,
    /// `CUBEJS_DB_EXPORT_BUCKET_AZURE_CLIENT_SECRET`.
    pub azure_client_secret: Option<String>,
    /// `CUBEJS_DB_EXPORT_GCS_CREDENTIALS` (base64 JSON, decoded).
    pub gcs_credentials: Option<Value>,
    /// `pollInterval` (`CUBEJS_DB_POLL_MAX_INTERVAL`, default 5 s).
    pub poll_interval: Duration,
    /// Statement timeout (`CUBEJS_DB_QUERY_TIMEOUT`, default 10 min — the
    /// JDBC driver's fixed `setQueryTimeout(600)`).
    pub execution_timeout: Duration,
    /// Pre-aggregation schema (`CUBEJS_PRE_AGGREGATIONS_SCHEMA`, else
    /// `dev_pre_aggregations` / `prod_pre_aggregations`), used to prefix the
    /// catalog. Resolved from the environment when `None`.
    pub pre_aggregations_schema: Option<String>,
    /// Overrides `https://<host>` (tests, proxies).
    pub base_url: Option<String>,
}

/// Data-source aware `CUBEJS_*` lookups (`keyByDataSource`).
struct DsEnv<'a> {
    env: &'a dyn EnvSource,
    declared: Vec<String>,
    data_source: String,
    pre_aggregations: bool,
}

impl<'a> DsEnv<'a> {
    fn new(env: &'a dyn EnvSource, driver: &DriverConfig) -> Self {
        Self {
            env,
            declared: data_sources(env),
            data_source: driver.data_source.data_source.clone(),
            pre_aggregations: driver.data_source.pre_aggregations,
        }
    }

    fn key(&self, origin: &str, pre_aggregations: bool) -> Result<String> {
        env_key(
            origin,
            &self.declared,
            Some(&self.data_source),
            pre_aggregations,
        )
    }

    fn get(&self, origin: &str) -> Result<Option<String>> {
        let key = self.key(origin, self.pre_aggregations)?;
        Ok(self.env.get(&key).filter(|v| !v.is_empty()))
    }
}

impl DatabricksConfig {
    /// Builds the configuration from a generic [`DriverConfig`].
    pub fn from_driver_config(mut driver: DriverConfig) -> Self {
        if driver.test_connection_timeout == crate::config::DEFAULT_TEST_CONNECTION_TIMEOUT {
            driver.test_connection_timeout = DEFAULT_TEST_CONNECTION_TIMEOUT;
        }
        let ds = &driver.data_source;
        Self {
            database: ds.database.clone(),
            poll_interval: ds.poll_max_interval,
            execution_timeout: ds.query_timeout,
            export_bucket_csv_escape_symbol: ds.export_bucket_csv_escape_symbol.clone(),
            driver: driver.clone(),
            ..Default::default()
        }
    }

    /// Reads the configuration of `data_source` from the environment.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        let driver = DriverConfig::from_env(data_source)?;
        let mut config = Self::from_driver_config(driver);
        config.apply_env()?;
        Ok(config)
    }

    /// Reads the Databricks and export-bucket variables from the process environment.
    pub fn apply_env(&mut self) -> Result<()> {
        self.apply_env_source(&ProcessEnv)
    }

    /// Reads the Databricks and export-bucket variables from `env`.
    pub fn apply_env_source(&mut self, env: &dyn EnvSource) -> Result<()> {
        let e = DsEnv::new(env, &self.driver);

        // `databricksUrl` throws when missing; the `jdbcUrl` fallback of the
        // Node.js driver is therefore unreachable and not ported.
        self.url = e.get("CUBEJS_DB_DATABRICKS_URL")?;
        if self.url.is_none() {
            return Err(DriverError::Config(format!(
                "The {} is required and missing.",
                e.key("CUBEJS_DB_DATABRICKS_URL", false)?
            )));
        }
        self.token = e.get("CUBEJS_DB_DATABRICKS_TOKEN")?;
        self.oauth_client_id = e.get("CUBEJS_DB_DATABRICKS_OAUTH_CLIENT_ID")?;
        self.oauth_client_secret = e.get("CUBEJS_DB_DATABRICKS_OAUTH_CLIENT_SECRET")?;
        self.catalog = e.get("CUBEJS_DB_DATABRICKS_CATALOG")?;

        self.bucket_type = e.get("CUBEJS_DB_EXPORT_BUCKET_TYPE")?;
        if let Some(bucket_type) = &self.bucket_type {
            if !SUPPORTED_BUCKET_TYPES.contains(&bucket_type.as_str()) {
                return Err(DriverError::Config(format!(
                    "The {} must be one of the [{}].",
                    e.key("CUBEJS_DB_EXPORT_BUCKET_TYPE", false)?,
                    SUPPORTED_BUCKET_TYPES.join(", ")
                )));
            }
        }
        self.export_bucket = e.get("CUBEJS_DB_EXPORT_BUCKET")?;
        self.export_bucket_mount_dir = e.get("CUBEJS_DB_EXPORT_BUCKET_MOUNT_DIR")?;
        self.aws_key = e.get("CUBEJS_DB_EXPORT_BUCKET_AWS_KEY")?;
        self.aws_secret = e.get("CUBEJS_DB_EXPORT_BUCKET_AWS_SECRET")?;
        self.aws_region = e.get("CUBEJS_DB_EXPORT_BUCKET_AWS_REGION")?;
        self.azure_key = e.get("CUBEJS_DB_EXPORT_BUCKET_AZURE_KEY")?;
        self.azure_tenant_id = e.get("CUBEJS_DB_EXPORT_BUCKET_AZURE_TENANT_ID")?;
        self.azure_client_id = e.get("CUBEJS_DB_EXPORT_BUCKET_AZURE_CLIENT_ID")?;
        self.azure_client_secret = e.get("CUBEJS_DB_EXPORT_BUCKET_AZURE_CLIENT_SECRET")?;
        if let Some(encoded) = e.get("CUBEJS_DB_EXPORT_GCS_CREDENTIALS")? {
            use base64::Engine;
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(encoded.trim())
                .map_err(|err| {
                    DriverError::Config(format!(
                        "CUBEJS_DB_EXPORT_GCS_CREDENTIALS must be base64 encoded JSON: {err}"
                    ))
                })?;
            self.gcs_credentials = Some(serde_json::from_slice(&decoded).map_err(|err| {
                DriverError::Config(format!(
                    "CUBEJS_DB_EXPORT_GCS_CREDENTIALS must be base64 encoded JSON: {err}"
                ))
            })?);
        }

        if self.pre_aggregations_schema.is_none() {
            self.pre_aggregations_schema = Some(pre_aggregations_schema_from_env(env));
        }
        Ok(())
    }

    /// `readOnly`: explicit, otherwise `true` without an export bucket.
    pub fn is_read_only(&self) -> bool {
        self.read_only.unwrap_or(self.export_bucket.is_none())
    }
}

/// `getPreAggrSchemaName`.
pub fn pre_aggregations_schema_from_env(env: &dyn EnvSource) -> String {
    if let Some(schema) = env
        .get("CUBEJS_PRE_AGGREGATIONS_SCHEMA")
        .filter(|s| !s.is_empty())
    {
        return schema;
    }
    let dev_mode = env.get("NODE_ENV").as_deref() != Some("production")
        || env
            .get("CUBEJS_DEV_MODE")
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
    if dev_mode {
        "dev_pre_aggregations".to_string()
    } else {
        "prod_pre_aggregations".to_string()
    }
}

/// `query.replace(new RegExp(`(?<=\s)${schema}\.(?=[^\s]+)`, 'g'), `${catalog}.${schema}.`)`.
pub fn prefix_schema_with_catalog(query: &str, schema: &str, catalog: &str) -> String {
    if schema.is_empty() {
        return query.to_string();
    }
    let needle = format!("{schema}.");
    let replacement = format!("{catalog}.{schema}.");
    let mut result = String::with_capacity(query.len());
    let mut last = 0;
    let mut search_from = 0;
    while let Some(pos) = query[search_from..].find(&needle) {
        let start = search_from + pos;
        let end = start + needle.len();
        let preceded_by_space = query[..start]
            .chars()
            .next_back()
            .map(char::is_whitespace)
            .unwrap_or(false);
        let followed_by_non_space = query[end..]
            .chars()
            .next()
            .map(|c| !c.is_whitespace())
            .unwrap_or(false);
        if preceded_by_space && followed_by_non_space {
            result.push_str(&query[last..start]);
            result.push_str(&replacement);
            last = end;
        }
        search_from = end;
    }
    result.push_str(&query[last..]);
    result
}

/// `quoteIdentifier`: back-ticks, unless already back-ticked.
pub fn quote_identifier(identifier: &str) -> String {
    if identifier.len() >= 2 && identifier.starts_with('`') && identifier.ends_with('`') {
        identifier.to_string()
    } else {
        format!("`{identifier}`")
    }
}

/// `DatabricksToGenericType` + the `numeric(p, s)` handling of `toGenericType`.
pub fn to_generic_type(
    column_type: &str,
    precision: Option<i64>,
    scale: Option<i64>,
    precise_decimal: bool,
) -> GenericType {
    let lower = column_type.trim().to_lowercase();
    let (mut precision, mut scale) = (precision, scale);
    if let Some(inner) = lower
        .strip_prefix("numeric")
        .map(str::trim_start)
        .and_then(|r| r.strip_prefix('('))
        .and_then(|r| r.strip_suffix(')'))
    {
        let parts: Vec<_> = inner.split(',').map(|p| p.trim().parse::<i64>()).collect();
        if let [Ok(p), Ok(s)] = parts.as_slice() {
            precision = Some(*p);
            scale = Some(*s);
        }
    }
    match column_type.to_lowercase().as_str() {
        "binary" => GenericType::Other("hll_datasketches".to_string()),
        "decimal(10,0)" => GenericType::Bigint,
        _ => crate::types::to_generic_type(column_type, precision, scale, precise_decimal),
    }
}

/// `generateTableColumnsForExport`: `binary` sketches are exported as base64.
pub fn generate_table_columns_for_export(columns: &[Column]) -> String {
    columns
        .iter()
        .map(|c| match &c.type_ {
            GenericType::Other(t) if t == "hll_datasketches" => format!("base64({})", c.name),
            _ => c.name.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Normalises a `JSON_ARRAY` timestamp (`2020-01-01T10:00:00.123Z`) to
/// `2020-01-01T10:00:00.123`.
fn normalize_timestamp(raw: &str) -> String {
    use chrono::{DateTime, NaiveDateTime};
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return dt.naive_utc().format("%Y-%m-%dT%H:%M:%S%.3f").to_string();
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(raw, fmt) {
            return dt.format("%Y-%m-%dT%H:%M:%S%.3f").to_string();
        }
    }
    raw.to_string()
}

/// Hydrates one `JSON_ARRAY` cell against the manifest `type_name`.
pub fn hydrate(value: &Value, type_name: &str) -> Value {
    let Value::String(s) = value else {
        return value.clone();
    };
    match type_name {
        "BOOLEAN" => match s.as_str() {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ => value.clone(),
        },
        "BYTE" | "SHORT" | "INT" => s
            .parse::<i64>()
            .map(Value::from)
            .unwrap_or_else(|_| value.clone()),
        "FLOAT" | "DOUBLE" => s
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .unwrap_or_else(|| value.clone()),
        "TIMESTAMP" | "TIMESTAMP_NTZ" => Value::String(normalize_timestamp(s)),
        _ => value.clone(),
    }
}

fn hydrate_row(row: &[Value], type_names: &[String]) -> Row {
    type_names
        .iter()
        .enumerate()
        .map(|(i, t)| row.get(i).map(|v| hydrate(v, t)).unwrap_or(Value::Null))
        .collect()
}

/// Resolves the credentials of every request (static token or OAuth M2M).
#[derive(Debug)]
pub struct AuthProvider {
    method: AuthMethod,
    token_url: String,
    http: reqwest::Client,
    cached: Mutex<Option<(String, Instant)>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AuthMethod {
    Static(Credentials),
    OAuth {
        client_id: String,
        client_secret: String,
    },
}

impl AuthProvider {
    /// Current credentials, refreshing the OAuth access token when needed
    /// (`getValidAccessToken`: refreshed one minute before it expires).
    pub async fn credentials(&self) -> Result<Credentials> {
        match &self.method {
            AuthMethod::Static(credentials) => Ok(credentials.clone()),
            AuthMethod::OAuth {
                client_id,
                client_secret,
            } => {
                let mut cached = self.cached.lock().await;
                if let Some((token, expires)) = cached.as_ref() {
                    if Instant::now() < *expires {
                        return Ok(Credentials::Bearer(token.clone()));
                    }
                }
                let (token, expires_in) = self.fetch_access_token(client_id, client_secret).await?;
                let expires = Instant::now()
                    + Duration::from_secs(expires_in).saturating_sub(Duration::from_secs(60));
                *cached = Some((token.clone(), expires));
                Ok(Credentials::Bearer(token))
            }
        }
    }

    /// `fetchAccessToken`: client credentials grant, `scope=all-apis`.
    async fn fetch_access_token(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> Result<(String, u64)> {
        #[derive(serde::Deserialize)]
        struct TokenResponse {
            access_token: String,
            #[serde(default)]
            expires_in: Option<u64>,
        }
        let response = self
            .http
            .post(&self.token_url)
            .basic_auth(client_id, Some(client_secret))
            .form(&[("grant_type", "client_credentials"), ("scope", "all-apis")])
            .send()
            .await
            .map_err(|e| DriverError::Connection {
                pool_name: "databricks".to_string(),
                message: format!("Failed to get access token: {e}"),
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(DriverError::Connection {
                pool_name: "databricks".to_string(),
                message: format!(
                    "Failed to get access token: {}",
                    status.canonical_reason().unwrap_or(status.as_str())
                ),
            });
        }
        let token: TokenResponse = response
            .json()
            .await
            .map_err(|e| DriverError::Query(format!("Failed to get access token: {e}")))?;
        Ok((token.access_token, token.expires_in.unwrap_or(3600)))
    }
}

/// Databricks driver.
pub struct DatabricksDriver {
    config: DatabricksConfig,
    parsed: ParsedJdbcUrl,
    client: StatementClient,
    auth: Arc<AuthProvider>,
    pool: Arc<Semaphore>,
}

impl std::fmt::Debug for DatabricksDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DatabricksDriver")
            .field("host", &self.parsed.host)
            .field("warehouse_id", &self.parsed.warehouse_id)
            .field("catalog", &self.config.catalog)
            .finish()
    }
}

impl DatabricksDriver {
    /// Validates the configuration and creates the driver. Nothing is sent
    /// until the first query.
    pub fn new(config: DatabricksConfig) -> Result<Self> {
        let raw_url = config.url.clone().ok_or_else(|| {
            DriverError::Config("The CUBEJS_DB_DATABRICKS_URL is required and missing.".to_string())
        })?;
        let (url, spark_protocol) = url::normalize_spark_protocol(&raw_url);
        let (uid, pwd, _cleaned) = url::extract_and_remove_uid_pwd(&url);
        let password = config
            .token
            .clone()
            .filter(|t| !t.is_empty())
            .or_else(|| Some(pwd.clone()).filter(|p| !p.is_empty()));

        let method = match (&config.oauth_client_id, &config.oauth_client_secret) {
            (Some(_), None) => {
                return Err(DriverError::Config(
                    "Invalid credentials: No OAuth Client Secret provided".to_string(),
                ))
            }
            (None, Some(_)) => {
                return Err(DriverError::Config(
                    "Invalid credentials: No OAuth Client ID provided".to_string(),
                ))
            }
            // OAuth has an advantage over UID+PWD.
            (Some(id), Some(secret)) => AuthMethod::OAuth {
                client_id: id.clone(),
                client_secret: secret.clone(),
            },
            (None, None) => match password {
                None => return Err(DriverError::Config("No credentials provided".to_string())),
                Some(password) if uid == "token" => {
                    AuthMethod::Static(Credentials::Bearer(password))
                }
                Some(password) => AuthMethod::Static(Credentials::Basic {
                    user: uid,
                    password,
                }),
            },
        };

        let parsed = url::parse_jdbc_url(&url)?;

        // `showDeprecations`.
        if !pwd.is_empty() {
            log::warn!(
                "PWD Parameter Deprecation in connection string: PWD parameter is deprecated and \
                 will be ignored in future releases. Please migrate to the \
                 CUBEJS_DB_DATABRICKS_TOKEN environment variable."
            );
        }
        if spark_protocol {
            log::warn!(
                "jdbc:spark protocol deprecation: The `jdbc:spark` protocol is deprecated and \
                 will be ignored in future releases. Please migrate your \
                 CUBEJS_DB_DATABRICKS_URL environment variable to the `jdbc:databricks` protocol."
            );
        }

        let http = reqwest::Client::builder()
            .user_agent("CubeDev_Cube")
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|e| DriverError::Config(format!("Unable to build the HTTP client: {e}")))?;
        let base_url = config
            .base_url
            .clone()
            .map(|u| u.trim_end_matches('/').to_string())
            .unwrap_or_else(|| format!("https://{}", parsed.host));

        let auth = Arc::new(AuthProvider {
            method,
            token_url: format!("{base_url}/oidc/v1/token"),
            http: http.clone(),
            cached: Mutex::new(None),
        });
        let pool = Arc::new(Semaphore::new(
            config.driver.data_source.effective_max_pool_size(),
        ));

        let mut config = config;
        if config.pre_aggregations_schema.is_none() {
            config.pre_aggregations_schema = Some(pre_aggregations_schema_from_env(&ProcessEnv));
        }

        Ok(Self {
            client: StatementClient::new(http, base_url),
            config,
            parsed,
            auth,
            pool,
        })
    }

    /// Creates the driver from `CUBEJS_DB_*` / `CUBEJS_DB_DATABRICKS_*`.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Self::new(DatabricksConfig::from_env(data_source)?)
    }

    /// The driver configuration.
    pub fn databricks_config(&self) -> &DatabricksConfig {
        &self.config
    }

    /// Host and warehouse parsed from the JDBC URL.
    pub fn connection_properties(&self) -> &ParsedJdbcUrl {
        &self.parsed
    }

    fn pre_aggregations_schema(&self) -> &str {
        self.config
            .pre_aggregations_schema
            .as_deref()
            .unwrap_or("dev_pre_aggregations")
    }

    /// `query()`'s catalog prefixing followed by the JDBC driver's
    /// client-side parameter interpolation (`format('spark', ...)`).
    pub fn prepare_sql(&self, sql: &str, params: &[Value]) -> String {
        let sql = match &self.config.catalog {
            Some(catalog) => {
                prefix_schema_with_catalog(sql, self.pre_aggregations_schema(), catalog)
            }
            None => sql.to_string(),
        };
        escape::format(Dialect::Spark, &sql, params)
    }

    /// `getSchemaFullName`.
    fn schema_full_name(&self, schema: &str) -> String {
        match &self.config.catalog {
            Some(catalog) => format!("{}.{}", quote_identifier(catalog), quote_identifier(schema)),
            None => quote_identifier(schema),
        }
    }

    fn request(&self, sql: String, disposition: Disposition) -> statement::StatementRequest {
        statement::StatementRequest {
            statement: sql,
            warehouse_id: self.parsed.warehouse_id.clone(),
            catalog: self.parsed.catalog.clone(),
            schema: self.parsed.schema.clone(),
            disposition,
            format: "JSON_ARRAY",
            wait_timeout: WAIT_TIMEOUT.to_string(),
            on_wait_timeout: "CONTINUE",
        }
    }

    /// Runs already prepared SQL and returns the terminal response.
    async fn execute_prepared(
        &self,
        sql: String,
        disposition: Disposition,
    ) -> Result<StatementResponse> {
        let _permit = self
            .pool
            .acquire()
            .await
            .map_err(|_| DriverError::PoolTimeout("databricks".to_string()))?;
        let credentials = self.auth.credentials().await?;
        self.client
            .execute(
                &self.request(sql, disposition),
                &credentials,
                self.config.poll_interval,
                self.config.execution_timeout,
            )
            .await
    }

    fn columns(&self, response: &StatementResponse) -> (Vec<Column>, Vec<String>) {
        let manifest = response.manifest.clone().unwrap_or_default();
        let columns = manifest
            .schema
            .columns
            .iter()
            .map(|c| {
                Column::new(
                    c.name.clone(),
                    self.to_generic_type(&c.type_text(), c.type_precision, c.type_scale),
                )
            })
            .collect();
        let type_names = manifest
            .schema
            .columns
            .iter()
            .map(|c| c.type_name())
            .collect();
        (columns, type_names)
    }

    /// Runs `sql` (already prepared) and materialises every chunk.
    async fn run(&self, sql: String, disposition: Disposition) -> Result<QueryResult> {
        let response = self.execute_prepared(sql, disposition).await?;
        let (columns, type_names) = self.columns(&response);
        let mut rows: Vec<Row> = Vec::new();
        let mut next = None;
        if let Some(result) = &response.result {
            for row in self.client.chunk_rows(result).await? {
                rows.push(hydrate_row(&row, &type_names));
            }
            next = result.next_chunk();
        }
        while let Some(index) = next {
            let credentials = self.auth.credentials().await?;
            let chunk = self
                .client
                .get_chunk(&response.statement_id, index, &credentials)
                .await?;
            for row in self.client.chunk_rows(&chunk).await? {
                rows.push(hydrate_row(&row, &type_names));
            }
            next = chunk.next_chunk();
        }
        Ok(QueryResult::new(columns, rows))
    }

    /// `SHOW DATABASES [IN catalog]`.
    fn show_databases_sql(&self) -> String {
        format!(
            "SHOW DATABASES{}",
            self.config
                .catalog
                .as_ref()
                .map(|c| format!(" IN {}", quote_identifier(c)))
                .unwrap_or_default()
        )
    }

    async fn show_databases(&self) -> Result<Vec<String>> {
        let result = self
            .query(&self.show_databases_sql(), &[], &QueryOptions::default())
            .await?;
        Ok((0..result.len())
            .filter_map(|i| {
                result
                    .get_string(i, "databaseName")
                    .or_else(|| result.get_string(i, "namespace"))
            })
            .collect())
    }

    /// `SHOW TABLES IN <schema>` as `(database, tableName)` pairs.
    async fn show_tables(&self, schema: &str) -> Result<Vec<(String, String)>> {
        let result = self
            .query(
                &format!("SHOW TABLES IN {}", self.schema_full_name(schema)),
                &[],
                &QueryOptions::default(),
            )
            .await?;
        Ok((0..result.len())
            .filter_map(|i| {
                Some((
                    result.get_string(i, "database").unwrap_or_default(),
                    result.get_string(i, "tableName")?,
                ))
            })
            .collect())
    }

    /// `getTables`.
    async fn get_tables(&self) -> Result<Vec<(String, String)>> {
        if let Some(database) = &self.config.database {
            return self.show_tables(database).await;
        }
        let databases = self.show_databases().await?;
        let tables =
            futures::future::try_join_all(databases.iter().map(|d| self.show_tables(d))).await?;
        Ok(tables.into_iter().flatten().collect())
    }

    /// `tableColumnTypes`' fully qualified, quoted name.
    pub fn table_full_name(&self, table: &str) -> String {
        let parts: Vec<&str> = table.split('.').collect();
        match (parts.as_slice(), &self.config.catalog) {
            ([catalog, schema, name], _) => format!(
                "{}.{}.{}",
                quote_identifier(catalog),
                quote_identifier(schema),
                quote_identifier(name)
            ),
            ([schema, name], Some(catalog)) => format!(
                "{}.{}.{}",
                quote_identifier(catalog),
                quote_identifier(schema),
                quote_identifier(name)
            ),
            ([schema, name, ..], None) => {
                format!("{}.{}", quote_identifier(schema), quote_identifier(name))
            }
            _ => quote_identifier(table),
        }
    }

    /// Parses `DESCRIBE` / `DESCRIBE QUERY` output, stopping at the first
    /// empty `col_name` (Databricks appends extra sections after it).
    fn describe_to_columns(&self, result: &QueryResult) -> TableStructure {
        let mut columns = Vec::new();
        for i in 0..result.len() {
            let name = result.get_string(i, "col_name").unwrap_or_default();
            if name.is_empty() {
                break;
            }
            let data_type = result.get_string(i, "data_type").unwrap_or_default();
            columns.push(Column::new(
                name,
                self.to_generic_type(&data_type, None, None),
            ));
        }
        columns
    }

    /// Fully qualified (catalog-prefixed) unquoted table name used by unload.
    pub fn unload_table_full_name(&self, table: &str) -> String {
        match &self.config.catalog {
            Some(catalog) => format!("{catalog}.{table}"),
            None => table.to_string(),
        }
    }

    /// `createExternalTableFromSql` statement (before parameter interpolation).
    pub fn unload_from_sql_statement(
        &self,
        table_full_name: &str,
        sql: &str,
        columns: &[Column],
    ) -> String {
        let select = if columns
            .iter()
            .any(|c| c.type_ == GenericType::Other("hll_datasketches".to_string()))
        {
            format!(
                "SELECT {} FROM ({sql})",
                generate_table_columns_for_export(columns)
            )
        } else {
            sql.to_string()
        };
        format!(
            "\n        INSERT OVERWRITE DIRECTORY '{}/{table_full_name}'\n        USING CSV\n        OPTIONS (escape '\"')\n        {select}\n      ",
            self.export_directory()
        )
    }

    /// `createExternalTableFromTable` statement.
    pub fn unload_from_table_statement(&self, table_full_name: &str, columns: &[Column]) -> String {
        format!(
            "\n        INSERT OVERWRITE DIRECTORY '{}/{table_full_name}'\n        USING CSV\n        OPTIONS (escape '\"')\n        SELECT {} FROM {table_full_name}\n      ",
            self.export_directory(),
            generate_table_columns_for_export(columns)
        )
    }

    fn export_directory(&self) -> String {
        self.config
            .export_bucket_mount_dir
            .clone()
            .or_else(|| self.config.export_bucket.clone())
            .unwrap_or_default()
    }

    fn unload_not_implemented(&self) -> DriverError {
        DriverError::NotImplemented(format!(
            "Databricks export bucket unload ({} bucket {}) is not implemented in the Rust \
             driver: listing and signing the unloaded CSV files needs a cloud storage client. \
             Unset CUBEJS_DB_EXPORT_BUCKET to build pre-aggregations by downloading query \
             results instead.",
            self.config.bucket_type.as_deref().unwrap_or("<unset>"),
            self.config.export_bucket.as_deref().unwrap_or("<unset>")
        ))
    }
}

#[async_trait]
impl Driver for DatabricksDriver {
    fn config(&self) -> &DriverConfig {
        &self.config.driver
    }

    /// Checks the SQL warehouse through the REST API, like the Node.js driver.
    async fn test_connection(&self) -> Result<()> {
        let credentials = self.auth.credentials().await?;
        let data = self
            .client
            .get_warehouse(&self.parsed.warehouse_id, &credentials)
            .await?;
        let state = data.get("state").and_then(Value::as_str).unwrap_or("");
        if state == "DELETING" || state == "DELETED" {
            return Err(DriverError::Connection {
                pool_name: "databricks".to_string(),
                message: format!("Warehouse is being deleted (current state: {state})"),
            });
        }
        // There is also DEGRADED status, but it doesn't mean that the warehouse is not working.
        let health = data.get("health");
        if health.and_then(|h| h.get("status")).and_then(Value::as_str) == Some("FAILED") {
            let field = |k: &str| {
                health
                    .and_then(|h| h.get(k))
                    .and_then(Value::as_str)
                    .unwrap_or("undefined")
                    .to_string()
            };
            return Err(DriverError::Connection {
                pool_name: "databricks".to_string(),
                message: format!(
                    "Warehouse is unhealthy: {}. Details: {}",
                    field("summary"),
                    field("details")
                ),
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
        self.run(self.prepare_sql(sql, params), Disposition::Inline)
            .await
    }

    fn quote_identifier(&self, identifier: &str) -> String {
        quote_identifier(identifier)
    }

    fn to_generic_type(
        &self,
        db_type: &str,
        precision: Option<i64>,
        scale: Option<i64>,
    ) -> GenericType {
        to_generic_type(
            db_type,
            precision,
            scale,
            self.config.driver.precise_decimal_in_cubestore,
        )
    }

    fn read_only(&self) -> bool {
        self.config.is_read_only()
    }

    fn capabilities(&self) -> DriverCapabilities {
        DriverCapabilities {
            unload_without_temp_table: true,
            incremental_schema_loading: true,
            ..Default::default()
        }
    }

    async fn stream(
        &self,
        sql: &str,
        params: &[Value],
        _options: &StreamOptions,
    ) -> Result<StreamTableData> {
        let response = self
            .execute_prepared(self.prepare_sql(sql, params), Disposition::ExternalLinks)
            .await?;
        let (columns, type_names) = self.columns(&response);
        let client = self.client.clone();
        let auth = self.auth.clone();
        let statement_id = response.statement_id.clone();

        enum Next {
            Data(statement::ResultData),
            Chunk(i64),
            Done,
        }
        let first = match response.result {
            Some(result) => Next::Data(result),
            None => Next::Done,
        };
        let chunks = futures::stream::unfold(first, move |state| {
            let client = client.clone();
            let auth = auth.clone();
            let statement_id = statement_id.clone();
            let type_names = type_names.clone();
            async move {
                let data = match state {
                    Next::Done => return None,
                    Next::Data(data) => data,
                    Next::Chunk(index) => {
                        let fetched = async {
                            let credentials = auth.credentials().await?;
                            client.get_chunk(&statement_id, index, &credentials).await
                        }
                        .await;
                        match fetched {
                            Ok(data) => data,
                            Err(e) => return Some((vec![Err(e)], Next::Done)),
                        }
                    }
                };
                let next = match data.next_chunk() {
                    Some(i) => Next::Chunk(i),
                    None => Next::Done,
                };
                match client.chunk_rows(&data).await {
                    Ok(rows) => Some((
                        rows.iter()
                            .map(|r| Ok(hydrate_row(r, &type_names)))
                            .collect(),
                        next,
                    )),
                    Err(e) => Some((vec![Err(e)], Next::Done)),
                }
            }
        });
        let rows = chunks.flat_map(futures::stream::iter).boxed();
        Ok(StreamTableData { columns, rows })
    }

    async fn download_query_results(
        &self,
        sql: &str,
        params: &[Value],
        options: &DownloadQueryResultsOptions,
    ) -> Result<DownloadedData> {
        if options.stream_import {
            return Ok(DownloadedData::Stream(
                self.stream(sql, params, &options.stream).await?,
            ));
        }
        // `BaseDriver.downloadQueryResults`, fetched through external links so
        // results above the 25 MiB inline limit work.
        let mut result = self
            .run(self.prepare_sql(sql, params), Disposition::ExternalLinks)
            .await?;
        result.columns = detect_types_from_tabular(&result)?;
        Ok(DownloadedData::Memory(result))
    }

    async fn load_pre_aggregation_into_table(
        &self,
        pre_aggregation_table_name: &str,
        load_sql: &str,
        params: &[Value],
        options: &QueryOptions,
    ) -> Result<QueryResult> {
        let sql = match &self.config.catalog {
            Some(catalog) => {
                let schema = pre_aggregation_table_name
                    .split('.')
                    .next()
                    .unwrap_or_default();
                prefix_schema_with_catalog(load_sql, schema, catalog)
            }
            None => load_sql.to_string(),
        };
        self.query(&sql, params, options).await
    }

    async fn drop_table(&self, table_name: &str, options: &QueryOptions) -> Result<()> {
        let full = match &self.config.catalog {
            Some(catalog) => format!("{catalog}.{table_name}"),
            None => table_name.to_string(),
        };
        self.query(&format!("DROP TABLE {full}"), &[], options)
            .await?;
        Ok(())
    }

    async fn create_schema_if_not_exists(&self, schema_name: &str) -> Result<()> {
        self.query(
            &format!(
                "CREATE SCHEMA IF NOT EXISTS {}",
                self.schema_full_name(schema_name)
            ),
            &[],
            &QueryOptions::default(),
        )
        .await?;
        Ok(())
    }

    async fn get_tables_query(&self, schema_name: &str) -> Result<Vec<String>> {
        Ok(self
            .show_tables(schema_name)
            .await?
            .into_iter()
            .map(|(_, t)| t)
            .collect())
    }

    async fn tables_schema(&self) -> Result<DatabaseStructure> {
        let tables = self.get_tables().await?;
        let described =
            futures::future::try_join_all(tables.iter().map(|(database, table)| async move {
                let columns = self
                    .table_column_types(&format!("{database}.{table}"))
                    .await?;
                Ok::<_, DriverError>((database.clone(), table.clone(), columns))
            }))
            .await?;
        let mut metadata = DatabaseStructure::new();
        for (database, table, columns) in described {
            metadata.entry(database).or_default().insert(
                table,
                columns
                    .into_iter()
                    .map(|c| SchemaColumn {
                        name: c.name,
                        type_: c.type_.to_string(),
                        attributes: Vec::new(),
                        foreign_keys: Vec::new(),
                    })
                    .collect(),
            );
        }
        Ok(metadata)
    }

    async fn get_schemas(&self) -> Result<Vec<SchemaName>> {
        Ok(self
            .show_databases()
            .await?
            .into_iter()
            .map(|schema_name| SchemaName { schema_name })
            .collect())
    }

    async fn get_tables_for_specific_schemas(
        &self,
        schemas: &[SchemaName],
    ) -> Result<Vec<SchemaTable>> {
        let tables =
            futures::future::try_join_all(schemas.iter().map(|s| self.show_tables(&s.schema_name)))
                .await?;
        Ok(tables
            .into_iter()
            .flatten()
            .map(|(schema_name, table_name)| SchemaTable {
                schema_name,
                table_name,
            })
            .collect())
    }

    async fn get_columns_for_specific_tables(
        &self,
        tables: &[SchemaTable],
    ) -> Result<Vec<ColumnInfo>> {
        let columns = futures::future::try_join_all(tables.iter().map(|t| async move {
            let full = match &self.config.catalog {
                Some(catalog) => format!("{catalog}.{}.{}", t.schema_name, t.table_name),
                None => format!("{}.{}", t.schema_name, t.table_name),
            };
            let types = self.table_column_types(&full).await?;
            Ok::<_, DriverError>(
                types
                    .into_iter()
                    .map(|c| ColumnInfo {
                        schema_name: t.schema_name.clone(),
                        table_name: t.table_name.clone(),
                        column_name: c.name,
                        data_type: c.type_.to_string(),
                        attributes: Vec::new(),
                        foreign_keys: Vec::new(),
                    })
                    .collect::<Vec<_>>(),
            )
        }))
        .await?;
        Ok(columns.into_iter().flatten().collect())
    }

    async fn table_column_types_impl(
        &self,
        table: &str,
        _with_precision: bool,
    ) -> Result<TableStructure> {
        let result = self
            .query(
                &format!("DESCRIBE {}", self.table_full_name(table)),
                &[],
                &QueryOptions::default(),
            )
            .await?;
        Ok(self.describe_to_columns(&result))
    }

    async fn query_column_types(
        &self,
        sql: &str,
        params: &[Value],
        _options: &QueryOptions,
    ) -> Result<TableStructure> {
        let result = self
            .query(
                &format!("DESCRIBE QUERY {sql}"),
                params,
                &QueryOptions::default(),
            )
            .await?;
        Ok(self.describe_to_columns(&result))
    }

    async fn is_unload_supported(&self, _options: &UnloadOptions) -> Result<bool> {
        if self.config.export_bucket.is_none() {
            return Ok(false);
        }
        Err(self.unload_not_implemented())
    }

    async fn unload(&self, _table: &str, _options: &UnloadOptions) -> Result<TableCsvData> {
        let bucket_type = self.config.bucket_type.as_deref().unwrap_or("");
        if !SUPPORTED_BUCKET_TYPES.contains(&bucket_type) {
            return Err(DriverError::Config(format!(
                "Unsupported export bucket type: {}",
                self.config.bucket_type.as_deref().unwrap_or("undefined")
            )));
        }
        Err(self.unload_not_implemented())
    }

    async fn unload_from_query(
        &self,
        _sql: &str,
        _params: &[Value],
        options: &UnloadOptions,
    ) -> Result<TableCsvData> {
        self.unload("", options).await
    }
}

#[cfg(test)]
mod tests;
