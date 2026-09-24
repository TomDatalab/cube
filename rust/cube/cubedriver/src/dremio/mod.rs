//! Dremio driver: port of `@cubejs-backend/dremio-driver` over Dremio's REST
//! API with `reqwest` (rustls).
//!
//! Two deployments are supported, exactly like the Node driver:
//!
//! * **Dremio Software** (`CUBEJS_DB_HOST` / `CUBEJS_DB_PORT` /
//!   `CUBEJS_DB_SSL`): `http[s]://<host>:<port>/api/v3/...`, authenticated
//!   either with a personal access token (`CUBEJS_DB_DREMIO_AUTH_TOKEN`) or with
//!   `CUBEJS_DB_USER` / `CUBEJS_DB_PASS` through `POST /apiv2/login`
//!   (`Authorization: _dremio<token>`).
//! * **Dremio Cloud** (`CUBEJS_DB_URL`, e.g.
//!   `https://api.dremio.cloud/v0/projects/<id>`): the URL is used as the API
//!   root (no `/api/v3` suffix) and `CUBEJS_DB_DREMIO_AUTH_TOKEN` is required.
//!
//! A query is a job: `POST /sql` submits it, `GET /job/<id>` is polled until
//! the job completes (the interval grows by 200 ms per attempt, capped at
//! `CUBEJS_DB_POLL_MAX_INTERVAL`, for at most `CUBEJS_DB_POLL_TIMEOUT` or
//! `CUBEJS_DB_QUERY_TIMEOUT`), and the rows are fetched in pages of 500
//! (`GET /job/<id>/results?offset=&limit=`, Dremio's maximum page size).
//!
//! Parameters are interpolated client-side with the ANSI escaper
//! (`formatAnsi`): quotes are doubled, backslashes are data.
//!
//! Differences from the Node driver, all additive:
//!
//! * the result carries column names and types, taken from the `schema` that
//!   Dremio returns with the results (the Node driver returns bare objects);
//!   columns Dremio omits from a row because they are `NULL` become `null`;
//! * an empty result still fetches the first page so that its columns are
//!   known;
//! * HTTP errors carry Dremio's `errorMessage` instead of axios'
//!   "Request failed with status code N" (which is used when there is none).
//!
//! The SQL dialect (`DremioQuery`) belongs to the planner, not to this crate.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures::future::BoxFuture;
use futures::{FutureExt, StreamExt, TryStreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::config::{data_sources, env_key, DriverConfig, EnvSource, ProcessEnv};
use crate::driver::{information_columns_to_structure, Driver};
use crate::error::{DriverError, Result};
use crate::escape::format_ansi;
use crate::types::{Column, DatabaseStructure, GenericType, QueryOptions, QueryResult, Row};

/// `getDefaultConcurrency`.
pub const DEFAULT_CONCURRENCY: usize = 2;
/// Maximum page size of `GET /job/<id>/results` (`DREMIO_JOB_LIMIT`).
pub const DREMIO_JOB_LIMIT: u64 = 500;
/// Default host (`CUBEJS_DB_HOST`).
pub const DEFAULT_HOST: &str = "localhost";
/// Default port (`CUBEJS_DB_PORT`).
pub const DEFAULT_PORT: u16 = 9047;
/// How many result pages are fetched at the same time. The Node driver fires
/// every page request at once; a bound keeps huge results from opening
/// thousands of connections while returning the same rows in the same order.
const PAGE_FETCH_CONCURRENCY: usize = 8;

/// `applyParams`: client-side interpolation with the ANSI escaper.
pub fn apply_params(query: &str, params: &[Value]) -> String {
    format_ansi(query, params)
}

/// Configuration of [`DremioDriver`].
#[derive(Debug, Clone)]
pub struct DremioConfig {
    /// Connection settings and global driver knobs.
    pub driver: DriverConfig,
    /// `CUBEJS_DB_URL`: Dremio Cloud API root. When set, the auth token is required.
    pub db_url: Option<String>,
    /// `CUBEJS_DB_DREMIO_AUTH_TOKEN`: personal access token (`Bearer`).
    pub auth_token: Option<String>,
    /// `CUBEJS_DB_HOST` (default `localhost`).
    pub host: String,
    /// `CUBEJS_DB_PORT` (default `9047`).
    pub port: u16,
    /// `CUBEJS_DB_USER`.
    pub user: Option<String>,
    /// `CUBEJS_DB_PASS`.
    pub password: Option<String>,
    /// `CUBEJS_DB_NAME`: the space / source introspected by `tables_schema`.
    pub database: Option<String>,
    /// `CUBEJS_DB_SSL=true`: `https` instead of `http` (Dremio Software only).
    pub ssl: bool,
    /// `CUBEJS_DB_POLL_TIMEOUT`, else `CUBEJS_DB_QUERY_TIMEOUT`.
    pub poll_timeout: Duration,
    /// `CUBEJS_DB_POLL_MAX_INTERVAL` (default 5 s).
    pub poll_max_interval: Duration,
}

impl DremioConfig {
    /// Builds the configuration from a generic [`DriverConfig`]. The
    /// Dremio-specific variables are read by [`DremioConfig::apply_env`].
    pub fn from_driver_config(driver: DriverConfig) -> Self {
        let ds = &driver.data_source;
        Self {
            db_url: ds.url.clone().filter(|u| !u.is_empty()),
            auth_token: None,
            host: ds
                .host
                .clone()
                .filter(|h| !h.is_empty())
                .unwrap_or_else(|| DEFAULT_HOST.to_string()),
            port: ds.port.filter(|p| *p != 0).unwrap_or(DEFAULT_PORT),
            user: ds.user.clone(),
            password: ds.password.clone(),
            database: ds.database.clone().filter(|d| !d.is_empty()),
            // `DataSourceConfig::ssl` is also set by
            // `CUBEJS_DB_SSL_REJECT_UNAUTHORIZED` alone; `apply_env` reads the
            // exact `CUBEJS_DB_SSL` flag the Node driver looks at.
            ssl: ds.ssl.is_some(),
            poll_timeout: ds.poll_timeout.unwrap_or(ds.query_timeout),
            poll_max_interval: ds.poll_max_interval,
            driver,
        }
    }

    /// Reads the configuration of `data_source` from the environment.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        let driver = DriverConfig::from_env(data_source)?;
        let mut config = Self::from_driver_config(driver);
        config.apply_env()?;
        Ok(config)
    }

    /// Reads `CUBEJS_DB_DREMIO_AUTH_TOKEN` and `CUBEJS_DB_SSL` (data source
    /// and pre-aggregation aware) from the process environment.
    pub fn apply_env(&mut self) -> Result<()> {
        self.apply_env_source(&ProcessEnv)
    }

    /// [`DremioConfig::apply_env`] against an arbitrary [`EnvSource`].
    pub fn apply_env_source(&mut self, env: &dyn EnvSource) -> Result<()> {
        let declared = data_sources(env);
        let ds = self.driver.data_source.data_source.clone();
        let pre_aggregations = self.driver.data_source.pre_aggregations;
        let key = |origin: &str| env_key(origin, &declared, Some(&ds), pre_aggregations);

        if let Some(token) = env.get(&key("CUBEJS_DB_DREMIO_AUTH_TOKEN")?) {
            if !token.is_empty() {
                self.auth_token = Some(token);
            }
        }
        let ssl_key = key("CUBEJS_DB_SSL")?;
        if let Some(ssl) = env.get(&ssl_key).filter(|v| !v.is_empty()) {
            self.ssl = match ssl.to_lowercase().as_str() {
                "true" => true,
                "false" => false,
                _ => {
                    return Err(DriverError::Config(format!(
                        "The {ssl_key} must be either 'true' or 'false'."
                    )))
                }
            };
        }
        Ok(())
    }

    /// API root and version prefix: `(<CUBEJS_DB_URL>, "")` for Dremio Cloud,
    /// `(http[s]://<host>:<port>, "/api/v3")` otherwise.
    pub fn endpoint(&self) -> (String, &'static str) {
        match &self.db_url {
            Some(url) => (url.trim_end_matches('/').to_string(), ""),
            None => {
                let protocol = if self.ssl { "https" } else { "http" };
                (
                    format!("{protocol}://{}:{}", self.host, self.port),
                    "/api/v3",
                )
            }
        }
    }

    fn validate(&self) -> Result<()> {
        if self.db_url.is_some() && self.auth_token.as_deref().unwrap_or("").is_empty() {
            return Err(DriverError::Config("dremioAuthToken is blank".to_string()));
        }
        Ok(())
    }
}

/// `POST /apiv2/login` response.
#[derive(Debug, Deserialize)]
struct LoginResponse {
    token: String,
    /// Expiry in milliseconds since the epoch.
    #[serde(default)]
    expires: i64,
}

/// `GET /job/<id>`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobStatus {
    pub job_state: String,
    #[serde(default)]
    pub row_count: Option<u64>,
    #[serde(default)]
    pub error_message: Option<String>,
}

/// `GET /job/<id>/results`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobResults {
    #[serde(default)]
    pub row_count: Option<u64>,
    #[serde(default)]
    pub schema: Vec<SchemaField>,
    #[serde(default)]
    pub rows: Vec<serde_json::Map<String, Value>>,
}

/// One entry of the results' `schema`.
#[derive(Debug, Clone, Deserialize)]
pub struct SchemaField {
    pub name: String,
    #[serde(rename = "type", default)]
    pub type_: Option<SchemaType>,
}

/// `schema[].type`.
#[derive(Debug, Clone, Deserialize)]
pub struct SchemaType {
    pub name: String,
    #[serde(default)]
    pub precision: Option<i64>,
    #[serde(default)]
    pub scale: Option<i64>,
}

/// `GET /catalog/by-path/<path>` (only what `refreshTablesSchema` reads).
#[derive(Debug, Deserialize)]
struct CatalogEntry {
    #[serde(default)]
    children: Option<Vec<CatalogChild>>,
}

#[derive(Debug, Deserialize)]
struct CatalogChild {
    #[serde(default)]
    path: Vec<String>,
}

/// Dremio driver.
pub struct DremioDriver {
    config: DremioConfig,
    client: reqwest::Client,
    /// Cached `/apiv2/login` token and its expiry (ms since the epoch).
    auth_token: Mutex<Option<(String, i64)>>,
}

impl std::fmt::Debug for DremioDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DremioDriver")
            .field("endpoint", &self.config.endpoint())
            .finish()
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl DremioDriver {
    /// Creates the driver. Fails like the Node constructor when
    /// `CUBEJS_DB_URL` is set without `CUBEJS_DB_DREMIO_AUTH_TOKEN`.
    pub fn new(config: DremioConfig) -> Result<Self> {
        config.validate()?;
        let client = reqwest::Client::builder()
            .user_agent(format!("CubeDev_Cube/{}", env!("CARGO_PKG_VERSION")))
            // Each request is short (jobs are polled); bound it by the poll
            // timeout so a stuck connection cannot hang a query forever.
            .timeout(config.poll_timeout.max(Duration::from_secs(30)))
            .build()
            .map_err(|e| DriverError::Config(format!("Unable to build the HTTP client: {e}")))?;
        Ok(Self {
            config,
            client,
            auth_token: Mutex::new(None),
        })
    }

    /// Creates the driver from `CUBEJS_DB_*`.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Self::new(DremioConfig::from_env(data_source)?)
    }

    /// The driver configuration.
    pub fn dremio_config(&self) -> &DremioConfig {
        &self.config
    }

    fn api_url(&self, path: &str) -> String {
        let (url, api_version) = self.config.endpoint();
        format!("{url}{api_version}{path}")
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<Value> {
        let response = request.send().await.map_err(|e| DriverError::Connection {
            pool_name: "dremio".to_string(),
            message: e.to_string(),
        })?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| DriverError::Query(e.to_string()))?;
        if !status.is_success() {
            return Err(dremio_http_error(status, &text));
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text)
            .map_err(|e| DriverError::Query(format!("Unexpected Dremio response: {e}: {text}")))
    }

    /// `getToken`: the value of the `Authorization` header.
    ///
    /// With a personal access token, every call validates it with
    /// `GET <api>/catalog` (as the Node driver does); otherwise the
    /// `/apiv2/login` token is cached until it expires.
    pub async fn get_token(&self) -> Result<String> {
        if let Some(token) = self.config.auth_token.as_deref().filter(|t| !t.is_empty()) {
            let bearer = format!("Bearer {token}");
            self.send(
                self.client
                    .get(self.api_url("/catalog"))
                    .header("Authorization", &bearer),
            )
            .await?;
            return Ok(bearer);
        }

        let mut cached = self.auth_token.lock().await;
        if let Some((token, expires)) = cached.as_ref() {
            if *expires > now_ms() {
                return Ok(format!("_dremio{token}"));
            }
        }

        let (url, _) = self.config.endpoint();
        let data = self
            .send(self.client.post(format!("{url}/apiv2/login")).json(&json!({
                "userName": self.config.user,
                "password": self.config.password,
            })))
            .await?;
        let login: LoginResponse = serde_json::from_value(data)
            .map_err(|e| DriverError::Query(format!("Unexpected Dremio login response: {e}")))?;
        let header = format!("_dremio{}", login.token);
        *cached = Some((login.token, login.expires));
        Ok(header)
    }

    /// `restDremioQuery`.
    async fn rest_dremio_query(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let token = self.get_token().await?;
        let mut request = self
            .client
            .request(method, self.api_url(path))
            .header("Authorization", token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        self.send(request).await
    }

    /// `getJobStatus`: `Some` once the job completed, `None` while it runs.
    pub async fn get_job_status(&self, job_id: &str) -> Result<Option<JobStatus>> {
        let data = self
            .rest_dremio_query(reqwest::Method::GET, &format!("/job/{job_id}"), None)
            .await?;
        let status: JobStatus = serde_json::from_value(data)
            .map_err(|e| DriverError::Query(format!("Unexpected Dremio job status: {e}")))?;
        job_state_outcome(job_id, status)
    }

    /// `getJobResults`.
    pub async fn get_job_results(
        &self,
        job_id: &str,
        limit: u64,
        offset: u64,
    ) -> Result<JobResults> {
        let data = self
            .rest_dremio_query(
                reqwest::Method::GET,
                &format!("/job/{job_id}/results?offset={offset}&limit={limit}"),
                None,
            )
            .await?;
        serde_json::from_value(data)
            .map_err(|e| DriverError::Query(format!("Unexpected Dremio job results: {e}")))
    }

    /// `executeQuery`: submits the SQL and returns the job id.
    pub async fn execute_query(&self, sql: &str) -> Result<String> {
        let data = self
            .rest_dremio_query(reqwest::Method::POST, "/sql", Some(json!({ "sql": sql })))
            .await?;
        data.get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| DriverError::Query(format!("Dremio did not return a job id: {data}")))
    }

    /// Builds the positional result out of the pages.
    fn build_result(&self, pages: Vec<JobResults>) -> QueryResult {
        let schema = pages
            .iter()
            .find(|p| !p.schema.is_empty())
            .map(|p| p.schema.clone())
            .unwrap_or_default();
        let columns: Vec<Column> = if schema.is_empty() {
            pages
                .iter()
                .flat_map(|p| p.rows.first())
                .next()
                .map(|row| row.keys().map(|k| Column::new(k.clone(), "text")).collect())
                .unwrap_or_default()
        } else {
            schema
                .iter()
                .map(|f| {
                    let generic = match &f.type_ {
                        Some(t) => {
                            self.to_generic_type(&t.name.to_lowercase(), t.precision, t.scale)
                        }
                        None => GenericType::Text,
                    };
                    Column::new(f.name.clone(), generic)
                })
                .collect()
        };
        let rows: Vec<Row> = pages
            .into_iter()
            .flat_map(|p| p.rows)
            .map(|mut row| {
                columns
                    .iter()
                    .map(|c| row.remove(&c.name).unwrap_or(Value::Null))
                    .collect()
            })
            .collect();
        QueryResult::new(columns, rows)
    }

    /// `refreshTablesSchema`: walks the catalog under `path` so that
    /// `INFORMATION_SCHEMA` lists every table (Dremio only lists the ones it
    /// has already seen otherwise).
    pub fn refresh_tables_schema<'a>(&'a self, path: String) -> BoxFuture<'a, Result<()>> {
        async move {
            let data = self
                .rest_dremio_query(
                    reqwest::Method::GET,
                    &format!("/catalog/by-path/{path}"),
                    None,
                )
                .await?;
            let entry: Option<CatalogEntry> = serde_json::from_value(data).ok();
            let Some(children) = entry.and_then(|e| e.children) else {
                return Ok(());
            };
            futures::future::try_join_all(
                children
                    .into_iter()
                    .map(|child| self.refresh_tables_schema(child.path.join("/"))),
            )
            .await?;
            Ok(())
        }
        .boxed()
    }
}

/// Maps a job state to the `getJobStatus` outcome.
fn job_state_outcome(job_id: &str, status: JobStatus) -> Result<Option<JobStatus>> {
    match status.job_state.as_str() {
        "FAILED" => Err(DriverError::Database {
            message: status.error_message.unwrap_or_default(),
            code: None,
        }),
        "CANCELED" => Err(DriverError::Query(format!(
            "Job {job_id} has been canceled"
        ))),
        "COMPLETED" => Ok(Some(status)),
        _ => Ok(None),
    }
}

/// Unwraps Dremio's `{ "errorMessage": … }` error body.
fn dremio_http_error(status: reqwest::StatusCode, body: &str) -> DriverError {
    let message = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v.get("errorMessage")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| format!("Request failed with status code {}", status.as_u16()));
    DriverError::Database {
        message,
        code: Some(status.as_u16().to_string()),
    }
}

/// `pausePromise(Math.min(pollMaxInterval, 200 * i))`.
fn poll_delay(attempt: u32, max: Duration) -> Duration {
    Duration::from_millis(200 * attempt as u64).min(max)
}

/// The page offsets `query` requests for `row_count` rows. An empty result
/// still fetches the first page, for its schema.
fn page_offsets(row_count: u64) -> Vec<u64> {
    let mut offsets: Vec<u64> = (0..row_count).step_by(DREMIO_JOB_LIMIT as usize).collect();
    if offsets.is_empty() {
        offsets.push(0);
    }
    offsets
}

#[async_trait]
impl Driver for DremioDriver {
    fn config(&self) -> &DriverConfig {
        &self.config.driver
    }

    /// `testConnection` = `getToken` (validates the PAT or logs in).
    async fn test_connection(&self) -> Result<()> {
        self.get_token().await.map(|_| ())
    }

    async fn query(
        &self,
        sql: &str,
        params: &[Value],
        _options: &QueryOptions,
    ) -> Result<QueryResult> {
        let query_string = apply_params(sql, params);

        self.get_token().await?;
        let job_id = self.execute_query(&query_string).await?;

        let started = Instant::now();
        let mut attempt: u32 = 0;
        while started.elapsed() <= self.config.poll_timeout {
            if let Some(job) = self.get_job_status(&job_id).await? {
                let pages: Vec<JobResults> =
                    futures::stream::iter(page_offsets(job.row_count.unwrap_or(0)))
                        .map(|offset| self.get_job_results(&job_id, DREMIO_JOB_LIMIT, offset))
                        .buffered(PAGE_FETCH_CONCURRENCY)
                        .try_collect()
                        .await?;
                return Ok(self.build_result(pages));
            }

            tokio::time::sleep(poll_delay(attempt, self.config.poll_max_interval)).await;
            attempt += 1;
        }

        Err(DriverError::Query(format!(
            "DremioQuery job timeout reached {}ms",
            self.config.poll_timeout.as_millis()
        )))
    }

    /// `informationSchemaQuery`: the base query without Dremio's system schemas.
    fn information_schema_query(&self) -> String {
        let query = format!(
            "{} AND columns.table_schema NOT IN ('INFORMATION_SCHEMA', 'sys.cache')",
            crate::sql::information_schema_query(&|i| self.quote_identifier(i))
        );
        log::debug!("{query}");
        query
    }

    /// `tablesSchema`: refreshes the catalog of `CUBEJS_DB_NAME` first.
    async fn tables_schema(&self) -> Result<DatabaseStructure> {
        let Some(database) = self.config.database.clone() else {
            return Err(DriverError::Config(
                "CUBEJS_DB_NAME can`t be empty.".to_string(),
            ));
        };
        self.refresh_tables_schema(database).await?;
        let query = self.information_schema_query();
        let data = self.query(&query, &[], &QueryOptions::default()).await?;
        Ok(information_columns_to_structure(&data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn config(pairs: &[(&str, &str)]) -> Result<DremioConfig> {
        let env = env(pairs);
        let driver = DriverConfig::from_env_source(&env, None, false)?;
        let mut config = DremioConfig::from_driver_config(driver);
        config.apply_env_source(&env)?;
        Ok(config)
    }

    #[test]
    fn software_defaults() {
        let c = config(&[]).unwrap();
        assert_eq!(
            c.endpoint(),
            ("http://localhost:9047".to_string(), "/api/v3")
        );
        assert_eq!(c.poll_timeout, Duration::from_secs(600));
        assert_eq!(c.poll_max_interval, Duration::from_secs(5));
        DremioDriver::new(c).unwrap();
    }

    #[test]
    fn software_host_port_ssl() {
        let c = config(&[
            ("CUBEJS_DB_HOST", "dremio.local"),
            ("CUBEJS_DB_PORT", "443"),
            ("CUBEJS_DB_SSL", "true"),
            ("CUBEJS_DB_POLL_TIMEOUT", "30s"),
            ("CUBEJS_DB_POLL_MAX_INTERVAL", "2"),
        ])
        .unwrap();
        assert_eq!(
            c.endpoint(),
            ("https://dremio.local:443".to_string(), "/api/v3")
        );
        assert_eq!(c.poll_timeout, Duration::from_secs(30));
        assert_eq!(c.poll_max_interval, Duration::from_secs(2));

        let err = config(&[("CUBEJS_DB_SSL", "yes")]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "The CUBEJS_DB_SSL must be either 'true' or 'false'."
        );
    }

    #[test]
    fn poll_timeout_falls_back_to_query_timeout() {
        let c = config(&[("CUBEJS_DB_QUERY_TIMEOUT", "2m")]).unwrap();
        assert_eq!(c.poll_timeout, Duration::from_secs(120));
    }

    #[test]
    fn cloud_requires_a_token() {
        let c = config(&[("CUBEJS_DB_URL", "https://api.dremio.cloud/v0/projects/p1/")]).unwrap();
        let err = DremioDriver::new(c).unwrap_err();
        assert_eq!(err.to_string(), "dremioAuthToken is blank");

        let c = config(&[
            ("CUBEJS_DB_URL", "https://api.dremio.cloud/v0/projects/p1"),
            ("CUBEJS_DB_DREMIO_AUTH_TOKEN", "pat"),
        ])
        .unwrap();
        assert_eq!(
            c.endpoint(),
            ("https://api.dremio.cloud/v0/projects/p1".to_string(), "")
        );
        assert_eq!(c.auth_token.as_deref(), Some("pat"));
        let driver = DremioDriver::new(c).unwrap();
        assert_eq!(
            driver.api_url("/sql"),
            "https://api.dremio.cloud/v0/projects/p1/sql"
        );
    }

    #[test]
    fn data_source_specific_token() {
        let c = {
            let env = env(&[
                ("CUBEJS_DATASOURCES", "default,lake"),
                ("CUBEJS_DS_LAKE_DB_DREMIO_AUTH_TOKEN", "lake-pat"),
                ("CUBEJS_DB_DREMIO_AUTH_TOKEN", "default-pat"),
            ]);
            let driver = DriverConfig::from_env_source(&env, Some("lake"), false).unwrap();
            let mut c = DremioConfig::from_driver_config(driver);
            c.apply_env_source(&env).unwrap();
            c
        };
        assert_eq!(c.auth_token.as_deref(), Some("lake-pat"));
    }

    #[test]
    fn sql_contract() {
        let driver = DremioDriver::new(config(&[]).unwrap()).unwrap();
        assert_eq!(driver.quote_identifier("a"), "\"a\"");
        assert_eq!(driver.param(0), "?");
        assert!(!driver.read_only());
        assert_eq!(
            driver.wrap_query_with_limit("SELECT 1", 10),
            "SELECT * FROM (SELECT 1) AS t LIMIT 10"
        );
        let q = driver.information_schema_query();
        assert!(q.contains("FROM information_schema.columns"));
        assert!(q.ends_with(" AND columns.table_schema NOT IN ('INFORMATION_SCHEMA', 'sys.cache')"));
    }

    #[tokio::test]
    async fn tables_schema_requires_a_database() {
        let driver = DremioDriver::new(config(&[]).unwrap()).unwrap();
        let err = driver.tables_schema().await.unwrap_err();
        assert_eq!(err.to_string(), "CUBEJS_DB_NAME can`t be empty.");
    }

    #[test]
    fn job_states() {
        let status = |state: &str| JobStatus {
            job_state: state.to_string(),
            row_count: Some(3),
            error_message: Some("boom".to_string()),
        };
        assert!(job_state_outcome("j", status("RUNNING")).unwrap().is_none());
        assert!(job_state_outcome("j", status("ENQUEUED"))
            .unwrap()
            .is_none());
        assert_eq!(
            job_state_outcome("j", status("COMPLETED"))
                .unwrap()
                .unwrap()
                .row_count,
            Some(3)
        );
        assert_eq!(
            job_state_outcome("j", status("FAILED"))
                .unwrap_err()
                .to_string(),
            "boom"
        );
        assert_eq!(
            job_state_outcome("j1", status("CANCELED"))
                .unwrap_err()
                .to_string(),
            "Job j1 has been canceled"
        );
    }

    #[test]
    fn polling_and_paging() {
        let max = Duration::from_secs(1);
        assert_eq!(poll_delay(0, max), Duration::ZERO);
        assert_eq!(poll_delay(3, max), Duration::from_millis(600));
        assert_eq!(poll_delay(50, max), max);

        assert_eq!(page_offsets(0), vec![0]);
        assert_eq!(page_offsets(4), vec![0]);
        assert_eq!(page_offsets(500), vec![0]);
        assert_eq!(page_offsets(501), vec![0, 500]);
        assert_eq!(page_offsets(1200), vec![0, 500, 1000]);
    }

    #[test]
    fn results_follow_the_schema() {
        let driver = DremioDriver::new(config(&[]).unwrap()).unwrap();
        let page: JobResults = serde_json::from_value(json!({
            "rowCount": 2,
            "schema": [
                { "name": "id", "type": { "name": "INTEGER" } },
                { "name": "amount", "type": { "name": "DECIMAL", "precision": 10, "scale": 2 } },
                { "name": "status", "type": { "name": "VARCHAR" } },
                { "name": "ts", "type": { "name": "TIMESTAMP" } },
            ],
            // Dremio leaves NULL columns out of the row object.
            "rows": [
                { "status": "new", "id": 1, "amount": 1.5, "ts": "2020-01-01 00:00:00.000" },
                { "id": 2, "amount": 2 },
            ],
        }))
        .unwrap();
        let second: JobResults = serde_json::from_value(json!({
            "rows": [ { "id": 3, "status": "x" } ],
        }))
        .unwrap();
        let result = driver.build_result(vec![page, second]);
        assert_eq!(
            result.columns,
            vec![
                Column::new("id", "int"),
                Column::new("amount", "decimal"),
                Column::new("status", "text"),
                Column::new("ts", "timestamp"),
            ]
        );
        assert_eq!(
            result.rows,
            vec![
                vec![
                    json!(1),
                    json!(1.5),
                    json!("new"),
                    json!("2020-01-01 00:00:00.000")
                ],
                vec![json!(2), json!(2), Value::Null, Value::Null],
                vec![json!(3), Value::Null, json!("x"), Value::Null],
            ]
        );
    }

    #[test]
    fn http_errors_are_unwrapped() {
        let err = dremio_http_error(
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"errorMessage":"Table 'x' not found","moreInfo":""}"#,
        );
        assert_eq!(err.to_string(), "Table 'x' not found");
        let err = dremio_http_error(reqwest::StatusCode::UNAUTHORIZED, "");
        assert_eq!(err.to_string(), "Request failed with status code 401");
    }

    // Port of `test/unit/params-escaping.test.ts`.
    mod params_escaping {
        use super::super::apply_params;
        use serde_json::json;

        #[test]
        fn preserves_like_escape_sequences_emitted_by_the_schema_compiler() {
            let sql = apply_params(
                r"SELECT * FROM orders WHERE LOWER(name) LIKE '%' || LOWER(?) || '%' ESCAPE '\'",
                &[json!(r"new\_order\%")],
            );
            assert_eq!(
                sql,
                r"SELECT * FROM orders WHERE LOWER(name) LIKE '%' || LOWER('new\_order\%') || '%' ESCAPE '\'"
            );
        }

        #[test]
        fn does_not_double_literal_backslashes_in_like_parameters() {
            let sql = apply_params(
                r"SELECT * FROM orders WHERE LOWER(name) LIKE '%' || LOWER(?) || '%' ESCAPE '\'",
                &[json!(r"folder\\name")],
            );
            assert_eq!(
                sql,
                r"SELECT * FROM orders WHERE LOWER(name) LIKE '%' || LOWER('folder\\name') || '%' ESCAPE '\'"
            );
        }

        #[test]
        fn doubles_quotes_so_a_value_cannot_break_out_of_the_literal() {
            let sql = apply_params(
                "SELECT * FROM orders WHERE name = ?",
                &[json!("o'reilly'); DROP TABLE orders; --")],
            );
            assert_eq!(
                sql,
                "SELECT * FROM orders WHERE name = 'o''reilly''); DROP TABLE orders; --'"
            );
        }

        #[test]
        fn keeps_the_literal_closed_for_a_backslash_then_quote_payload() {
            let sql = apply_params(
                "SELECT * FROM orders WHERE name = ?",
                &[json!(r"foo\' OR 1=1 --")],
            );
            assert_eq!(sql, r"SELECT * FROM orders WHERE name = 'foo\'' OR 1=1 --'");
        }

        #[test]
        fn keeps_the_literal_closed_for_a_value_ending_in_a_backslash() {
            let sql = apply_params(
                "SELECT * FROM orders WHERE name = ? AND status = ?",
                &[json!(r"payload\"), json!("new")],
            );
            assert_eq!(
                sql,
                r"SELECT * FROM orders WHERE name = 'payload\' AND status = 'new'"
            );
        }

        #[test]
        fn keeps_a_literal_percent_sign_in_an_equality_parameter_verbatim() {
            let sql = apply_params(
                "SELECT * FROM orders WHERE discount_label = ?",
                &[json!("100% cotton")],
            );
            assert_eq!(
                sql,
                "SELECT * FROM orders WHERE discount_label = '100% cotton'"
            );
        }

        #[test]
        fn escapes_every_element_of_an_array_parameter() {
            let sql = apply_params(
                "SELECT * FROM orders WHERE status IN (?)",
                &[json!(["it's", "b"])],
            );
            assert_eq!(sql, "SELECT * FROM orders WHERE status IN ('it''s', 'b')");
        }

        #[test]
        fn substitutes_multiple_placeholders_in_order() {
            let sql = apply_params(
                r"SELECT * FROM orders WHERE LOWER(name) LIKE '%' || LOWER(?) || '%' ESCAPE '\' AND status = ? AND amount > ?",
                &[json!(r"pending\_review"), json!("new"), json!(100)],
            );
            assert_eq!(
                sql,
                r"SELECT * FROM orders WHERE LOWER(name) LIKE '%' || LOWER('pending\_review') || '%' ESCAPE '\' AND status = 'new' AND amount > 100"
            );
        }
    }
}
