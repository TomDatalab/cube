//! Firebolt driver: port of `@cubejs-backend/firebolt-driver` over Firebolt's
//! HTTP API (`reqwest` with rustls).
//!
//! The Node driver uses `firebolt-sdk`; this port speaks the same endpoints:
//! an OAuth2 client-credentials token (or the v1 `/auth/v1/login` flow for
//! `user@domain` logins), the engine URL, and `POST <engine>/?database=…`
//! with `output_format=JSON`.
//!
//! Not supported yet: unload (the Node driver throws as well) and
//! `queryParameters` — values are interpolated client-side with the shared
//! `format_mysql` escaper, like the ClickHouse driver does.

pub mod types;

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::config::DriverConfig;
use crate::driver::Driver;
use crate::error::{DriverError, Result};
use crate::escape::format_mysql;
use crate::types::{
    Column, DownloadQueryResultsOptions, DownloadedData, GenericType, QueryOptions, QueryResult,
    Row, TableCsvData, TableStructure, UnloadOptions,
};

pub use types::{is_number_type, to_generic_type};

/// `getDefaultConcurrency`.
pub const DEFAULT_CONCURRENCY: usize = 10;
/// Default API endpoint (`CUBEJS_FIREBOLT_API_ENDPOINT`).
pub const DEFAULT_API_ENDPOINT: &str = "api.app.firebolt.io";
/// Identity provider of the v2 (service account) authentication.
pub const AUTH_ENDPOINT: &str = "https://id.app.firebolt.io/oauth/token";
/// OAuth audience of the v2 authentication.
pub const AUTH_AUDIENCE: &str = "https://api.firebolt.io";
/// `testConnectionTimeout` default: 2 minutes, so a stopped engine can start.
pub const TEST_CONNECTION_TIMEOUT: Duration = Duration::from_millis(120_000);

/// Configuration of [`FireboltDriver`] (`FireboltDriverConfiguration`).
#[derive(Debug, Clone)]
pub struct FireboltConfig {
    /// Connection settings and global driver knobs.
    pub driver: DriverConfig,
    /// `CUBEJS_DB_USER`: a service-account client id, or `user@domain`.
    pub username: Option<String>,
    /// `CUBEJS_DB_PASS`: the client secret or the password.
    pub password: Option<String>,
    /// `CUBEJS_DB_NAME`.
    pub database: Option<String>,
    /// `CUBEJS_FIREBOLT_ACCOUNT`.
    pub account: Option<String>,
    /// `CUBEJS_FIREBOLT_ENGINE_NAME`.
    pub engine_name: Option<String>,
    /// `CUBEJS_FIREBOLT_ENGINE_ENDPOINT` (deprecated, but avoids a lookup).
    pub engine_endpoint: Option<String>,
    /// `CUBEJS_FIREBOLT_API_ENDPOINT` (default `api.app.firebolt.io`).
    pub api_endpoint: String,
    /// `requestTimeout` (`CUBEJS_DB_QUERY_TIMEOUT`).
    pub request_timeout: Duration,
    /// `readOnly` (default `true`).
    pub read_only: bool,
}

impl FireboltConfig {
    /// Builds the Firebolt configuration from a generic [`DriverConfig`].
    pub fn from_driver_config(driver: DriverConfig) -> Self {
        let ds = &driver.data_source;
        Self {
            username: ds.user.clone(),
            password: ds.password.clone(),
            database: ds.database.clone(),
            account: None,
            engine_name: None,
            engine_endpoint: None,
            api_endpoint: DEFAULT_API_ENDPOINT.to_string(),
            request_timeout: ds.query_timeout,
            read_only: true,
            driver,
        }
    }

    /// Reads the configuration of `data_source` from the environment.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        let driver = DriverConfig::from_env(data_source)?;
        let mut config = Self::from_driver_config(driver);
        config.apply_env();
        Ok(config)
    }

    /// Reads the `CUBEJS_FIREBOLT_*` variables into an existing configuration.
    pub fn apply_env(&mut self) {
        self.account = env_var("CUBEJS_FIREBOLT_ACCOUNT");
        self.engine_name = env_var("CUBEJS_FIREBOLT_ENGINE_NAME");
        self.engine_endpoint = env_var("CUBEJS_FIREBOLT_ENGINE_ENDPOINT");
        if let Some(endpoint) = env_var("CUBEJS_FIREBOLT_API_ENDPOINT") {
            self.api_endpoint = endpoint;
        }
    }

    /// `username.includes('@')` decides between the two authentication flows.
    pub fn uses_password_auth(&self) -> bool {
        self.username.as_deref().is_some_and(|u| u.contains('@'))
    }
}

fn env_var(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// `POST /oauth/token` and `POST /auth/v1/login` share this response shape.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    expires_in: Option<u64>,
}

/// `GET /web/v3/account/<account>/engineUrl`.
#[derive(Debug, Deserialize)]
struct EngineUrlResponse {
    #[serde(rename = "engineUrl")]
    engine_url: String,
}

/// The `output_format=JSON` response envelope.
#[derive(Debug, Default, Deserialize)]
pub struct QueryResponse {
    #[serde(default)]
    pub meta: Vec<MetaColumn>,
    #[serde(default)]
    pub data: Vec<Value>,
    #[serde(default)]
    pub rows: Option<i64>,
}

/// One entry of `meta`.
#[derive(Debug, Clone, Deserialize)]
pub struct MetaColumn {
    pub name: String,
    #[serde(rename = "type")]
    pub type_: String,
}

/// Firebolt driver.
pub struct FireboltDriver {
    config: FireboltConfig,
    client: reqwest::Client,
    /// Cached access token and its expiry (seconds since the epoch).
    token: Mutex<Option<(String, u64)>>,
    /// Cached engine URL.
    engine_url: Mutex<Option<String>>,
}

impl std::fmt::Debug for FireboltDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FireboltDriver")
            .field("database", &self.config.database)
            .field("engine", &self.config.engine_name)
            .finish()
    }
}

impl FireboltDriver {
    /// Creates the driver. Nothing is sent until the first query.
    pub fn new(config: FireboltConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(format!("CubeDev_Cube/{}", env!("CARGO_PKG_VERSION")))
            .timeout(config.request_timeout + Duration::from_secs(30))
            .build()
            .map_err(|e| DriverError::Config(format!("Unable to build the HTTP client: {e}")))?;
        Ok(Self {
            config,
            client,
            token: Mutex::new(None),
            engine_url: Mutex::new(None),
        })
    }

    /// Creates the driver from `CUBEJS_DB_*` / `CUBEJS_FIREBOLT_*`.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Self::new(FireboltConfig::from_env(data_source)?)
    }

    /// The driver configuration.
    pub fn firebolt_config(&self) -> &FireboltConfig {
        &self.config
    }

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    /// A valid access token, refreshed shortly before it expires.
    async fn access_token(&self) -> Result<String> {
        let now = Self::now();
        let mut cached = self.token.lock().await;
        if let Some((token, expires_at)) = cached.as_ref() {
            if *expires_at > now {
                return Ok(token.clone());
            }
        }

        let username = self.config.username.clone().ok_or_else(|| {
            DriverError::Config("CUBEJS_DB_USER is not set for the Firebolt driver.".to_string())
        })?;
        let password = self.config.password.clone().unwrap_or_default();

        let request = if self.config.uses_password_auth() {
            // v1: user name / password login.
            self.client
                .post(format!(
                    "https://{}/auth/v1/login",
                    self.config.api_endpoint
                ))
                .json(&serde_json::json!({
                    "username": username,
                    "password": password,
                }))
        } else {
            // v2: service account (client id / client secret).
            self.client.post(AUTH_ENDPOINT).form(&[
                ("client_id", username.as_str()),
                ("client_secret", password.as_str()),
                ("grant_type", "client_credentials"),
                ("audience", AUTH_AUDIENCE),
            ])
        };

        let response = request.send().await.map_err(|e| DriverError::Connection {
            pool_name: "firebolt".to_string(),
            message: e.to_string(),
        })?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| DriverError::Query(e.to_string()))?;
        if !status.is_success() {
            return Err(DriverError::Config(format!(
                "Unable to authenticate against Firebolt ({status}): {}",
                body.trim()
            )));
        }
        let token: TokenResponse = serde_json::from_str(&body).map_err(|e| {
            DriverError::Config(format!("Unexpected Firebolt token response: {e}: {body}"))
        })?;
        let expires_in = token.expires_in.unwrap_or(3600).saturating_sub(60);
        *cached = Some((token.access_token.clone(), now + expires_in));
        Ok(token.access_token)
    }

    /// The URL queries are sent to (`engineEndpoint`, else the engine lookup).
    async fn engine_url(&self) -> Result<String> {
        if let Some(endpoint) = &self.config.engine_endpoint {
            return Ok(normalise_url(endpoint));
        }
        let mut cached = self.engine_url.lock().await;
        if let Some(url) = cached.as_ref() {
            return Ok(url.clone());
        }

        let account = self.config.account.clone().ok_or_else(|| {
            DriverError::Config(
                "CUBEJS_FIREBOLT_ACCOUNT is required to resolve the Firebolt engine URL \
                 (or set CUBEJS_FIREBOLT_ENGINE_ENDPOINT)."
                    .to_string(),
            )
        })?;

        // The system engine can be reached without knowing any engine URL.
        let system_engine: EngineUrlResponse = self
            .get_json(&format!(
                "https://{}/web/v3/account/{account}/engineUrl",
                self.config.api_endpoint
            ))
            .await?;
        let system_engine_url = normalise_url(&system_engine.engine_url);

        let url = match &self.config.engine_name {
            Some(engine_name) if !engine_name.is_empty() => {
                // Ask the system engine where the named engine listens.
                let response = self
                    .send_query(
                        &system_engine_url,
                        &format_mysql(
                            "SELECT url FROM information_schema.engines WHERE engine_name = ?",
                            &[Value::from(engine_name.clone())],
                        ),
                        None,
                    )
                    .await?;
                let url = response
                    .data
                    .first()
                    .and_then(|row| match row {
                        Value::Object(map) => map.get("url").cloned(),
                        Value::Array(values) => values.first().cloned(),
                        _ => None,
                    })
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
                    .ok_or_else(|| {
                        DriverError::Config(format!(
                            "Firebolt engine \"{engine_name}\" was not found."
                        ))
                    })?;
                normalise_url(&url)
            }
            _ => system_engine_url,
        };

        *cached = Some(url.clone());
        Ok(url)
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T> {
        let token = self.access_token().await?;
        let response = self
            .client
            .get(url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| DriverError::Connection {
                pool_name: "firebolt".to_string(),
                message: e.to_string(),
            })?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| DriverError::Query(e.to_string()))?;
        if !status.is_success() {
            return Err(DriverError::Database {
                message: body.trim().to_string(),
                code: Some(status.as_u16().to_string()),
            });
        }
        serde_json::from_str(&body)
            .map_err(|e| DriverError::Query(format!("Unexpected Firebolt response: {e}: {body}")))
    }

    /// Sends one statement to `engine_url`.
    async fn send_query(
        &self,
        engine_url: &str,
        sql: &str,
        database: Option<&str>,
    ) -> Result<QueryResponse> {
        let token = self.access_token().await?;
        let mut request = self
            .client
            .post(engine_url)
            .bearer_auth(token)
            .query(&[("output_format", "JSON")])
            .query(&[(
                "statement_timeout",
                self.config.request_timeout.as_secs().to_string(),
            )]);
        if let Some(database) = database {
            request = request.query(&[("database", database)]);
        }
        if let Some(account) = &self.config.account {
            request = request.query(&[("account_id", account)]);
        }

        let response =
            request
                .body(sql.to_string())
                .send()
                .await
                .map_err(|e| DriverError::Connection {
                    pool_name: engine_url.to_string(),
                    message: e.to_string(),
                })?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| DriverError::Query(e.to_string()))?;
        if !status.is_success() {
            return Err(DriverError::Database {
                message: body.trim().to_string(),
                code: Some(status.as_u16().to_string()),
            });
        }
        if body.trim().is_empty() {
            return Ok(QueryResponse::default());
        }
        serde_json::from_str(&body)
            .map_err(|e| DriverError::Query(format!("Unexpected Firebolt response: {e}: {body}")))
    }

    /// `queryResponse`: runs `sql` against the configured engine.
    async fn query_response(&self, sql: &str, params: &[Value]) -> Result<QueryResponse> {
        let engine_url = self.engine_url().await?;
        let statement = format_mysql(sql, params);
        self.send_query(&engine_url, &statement, self.config.database.as_deref())
            .await
    }

    /// Converts the `meta` + `data` envelope into a [`QueryResult`].
    fn build_result(&self, response: QueryResponse) -> QueryResult {
        let columns: Vec<Column> = response
            .meta
            .iter()
            .map(|c| Column::new(c.name.clone(), self.to_generic_type(&c.type_, None, None)))
            .collect();

        let rows: Vec<Row> = response
            .data
            .iter()
            .map(|row| match row {
                Value::Object(map) => response
                    .meta
                    .iter()
                    .map(|c| {
                        types::hydrate_value(map.get(&c.name).unwrap_or(&Value::Null), &c.type_)
                    })
                    .collect(),
                Value::Array(values) => response
                    .meta
                    .iter()
                    .enumerate()
                    .map(|(i, c)| {
                        types::hydrate_value(values.get(i).unwrap_or(&Value::Null), &c.type_)
                    })
                    .collect(),
                other => vec![other.clone()],
            })
            .collect();

        QueryResult::new(columns, rows)
    }
}

/// Makes sure an engine URL carries a scheme.
fn normalise_url(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else {
        format!("https://{url}")
    }
}

#[async_trait]
impl Driver for FireboltDriver {
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
        let response = self.query_response(sql, params).await?;
        Ok(self.build_result(response))
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

    fn test_connection_timeout(&self) -> Duration {
        TEST_CONNECTION_TIMEOUT
    }

    /// Firebolt only knows `CREATE DIMENSION TABLE`.
    fn create_table_sql(&self, quoted_table_name: &str, columns: &[Column]) -> String {
        let cols = columns
            .iter()
            .map(|c| {
                format!(
                    "{} {}",
                    self.quote_identifier(&c.name),
                    self.from_generic_type(&c.type_)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("CREATE DIMENSION TABLE {quoted_table_name} ({cols})")
    }

    /// Firebolt has no schemas, so the schema part is dropped.
    async fn drop_table(&self, table_name: &str, options: &QueryOptions) -> Result<()> {
        let name = match table_name.split_once('.') {
            Some((_, name)) => name,
            None => table_name,
        };
        self.query(&format!("DROP TABLE {name}"), &[], options)
            .await?;
        Ok(())
    }

    async fn create_schema_if_not_exists(&self, _schema_name: &str) -> Result<()> {
        // no-op
        Ok(())
    }

    async fn get_tables_query(&self, _schema_name: &str) -> Result<Vec<String>> {
        let result = self
            .query("SHOW TABLES", &[], &QueryOptions::default())
            .await?;
        Ok((0..result.len())
            .filter_map(|i| result.get_string(i, "table_name"))
            .collect())
    }

    async fn table_column_types(&self, table: &str) -> Result<TableStructure> {
        let result = self
            .query(&format!("DESCRIBE {table}"), &[], &QueryOptions::default())
            .await?;
        Ok((0..result.len())
            .filter_map(|i| {
                let name = result.get_string(i, "column_name")?;
                let data_type = result.get_string(i, "data_type")?;
                Some(Column::new(
                    name,
                    self.to_generic_type(&data_type, None, None),
                ))
            })
            .collect())
    }

    async fn download_query_results(
        &self,
        sql: &str,
        params: &[Value],
        _options: &DownloadQueryResultsOptions,
    ) -> Result<DownloadedData> {
        let response = self.query_response(sql, params).await?;
        Ok(DownloadedData::Memory(self.build_result(response)))
    }

    async fn is_unload_supported(&self, _options: &UnloadOptions) -> Result<bool> {
        Ok(false)
    }

    async fn unload(&self, _table: &str, _options: &UnloadOptions) -> Result<TableCsvData> {
        Err(DriverError::NotImplemented(
            "Unload is not supported".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn driver() -> FireboltDriver {
        let mut config = FireboltConfig::from_driver_config(DriverConfig::default());
        config.username = Some("client-id".to_string());
        config.password = Some("secret".to_string());
        config.database = Some("cube".to_string());
        config.engine_endpoint = Some("my-engine.eu-west-1.app.firebolt.io".to_string());
        FireboltDriver::new(config).unwrap()
    }

    #[test]
    fn auth_flow_selection() {
        let mut config = FireboltConfig::from_driver_config(DriverConfig::default());
        config.username = Some("client-id".into());
        assert!(!config.uses_password_auth());
        config.username = Some("someone@example.com".into());
        assert!(config.uses_password_auth());
    }

    #[test]
    fn engine_urls_get_a_scheme() {
        assert_eq!(
            normalise_url("my-engine.app.firebolt.io/"),
            "https://my-engine.app.firebolt.io"
        );
        assert_eq!(
            normalise_url("https://my-engine.app.firebolt.io"),
            "https://my-engine.app.firebolt.io"
        );
    }

    #[tokio::test]
    async fn sql_contract() {
        let driver = driver();
        assert_eq!(driver.param(0), "?");
        assert_eq!(driver.quote_identifier("a"), "\"a\"");
        assert!(driver.read_only());
        assert_eq!(driver.test_connection_timeout(), TEST_CONNECTION_TIMEOUT);
        assert_eq!(
            driver.create_table_sql("t", &[Column::new("a", "int"), Column::new("b", "string")]),
            r#"CREATE DIMENSION TABLE t ("a" int, "b" string)"#
        );
        assert!(!driver
            .is_unload_supported(&UnloadOptions::default())
            .await
            .unwrap());
        let err = driver
            .unload("t", &UnloadOptions::default())
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "Unload is not supported");
        // schemas are a no-op
        driver.create_schema_if_not_exists("x").await.unwrap();
    }

    #[test]
    fn results_are_built_from_meta() {
        let driver = driver();
        let response: QueryResponse = serde_json::from_value(serde_json::json!({
            "meta": [
                { "name": "id", "type": "long" },
                { "name": "price", "type": "numeric(10, 2)" },
                { "name": "name", "type": "text" },
                { "name": "nothing", "type": "int null" }
            ],
            "data": [
                { "id": 1, "price": 1.25, "name": "a", "nothing": null }
            ],
            "rows": 1
        }))
        .unwrap();
        let result = driver.build_result(response);
        assert_eq!(
            result.columns,
            vec![
                Column::new("id", "bigint"),
                // `numeric(p, s)` is passed through unchanged, as in JS
                Column::new("price", "numeric(10, 2)"),
                Column::new("name", "text"),
                Column::new("nothing", "int null"),
            ]
        );
        // numbers are stringified (`getHydratedValue`)
        assert_eq!(
            result.rows,
            vec![vec![
                Value::from("1"),
                Value::from("1.25"),
                Value::from("a"),
                Value::Null
            ]]
        );
    }
}
