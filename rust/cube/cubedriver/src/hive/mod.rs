//! Hive driver (`CUBEJS_DB_TYPE=hive`): port of `@cubejs-backend/hive-driver`
//! on a hand-written HiveServer2 `TCLIService` client ([`tcli`]) serialised
//! with the `thrift` crate's binary protocol, over an async TCP transport
//! with SASL `PLAIN` or `NOSASL` ([`transport`]). It also serves the Spark
//! Thrift Server, which speaks the same protocol.
//!
//! Ported from the Node.js driver (which used `jshs2`):
//!
//! * configuration: `CUBEJS_DB_HOST`, `CUBEJS_DB_PORT` (10000),
//!   `CUBEJS_DB_NAME` (`default`), `CUBEJS_DB_USER` (`anonymous`),
//!   `CUBEJS_DB_PASS`, `CUBEJS_DB_HIVE_TYPE`, `CUBEJS_DB_HIVE_VER` (2.1.1),
//!   `CUBEJS_DB_HIVE_THRIFT_VER`, `CUBEJS_DB_HIVE_CDH_VER`,
//!   `CUBEJS_DB_MAX_POOL` (8), all data-source aware; SASL `PLAIN` with the
//!   `cube.js` authzid by default;
//! * execution: asynchronous `ExecuteStatement`, `GetOperationStatus` polled
//!   every 500 ms, `GetResultSetMetadata`, then `FetchResults` in blocks of
//!   5120 rows until an empty block; client-side `?` interpolation with
//!   `sqlstring`'s backslash escaping; `_u1.`-style prefixes stripped from column names;
//!   `bigint` values as strings (`i64ToString`);
//! * `tablesSchema` (`show tables in <db>` + `describe <db>.<table>`),
//!   back-tick quoting, default concurrency 2, pool of 8 sessions with a 20 s
//!   acquire timeout and a 30 s idle timeout.
//!
//! Differences:
//!
//! * `CUBEJS_DB_HIVE_AUTH` (`PLAIN` | `NOSASL`) is new: the Node.js driver
//!   only accepted `auth: 'NOSASL'` as a constructor option, i.e. from a
//!   `driverFactory` in `cube.js`, which the Rust server does not load.
//! * Only real SQL `NULL`s become `null`; `jshs2` rendered them as the string
//!   `'NULL'` and the driver then also turned genuine `'NULL'` strings into
//!   `null`.
//! * Operations are closed (`CloseOperation`) once fetched; `jshs2` leaked them
//!   until the session ended.
//! * Kerberos (`GSSAPI`) and the HTTP transport were not supported by the
//!   Node.js driver either and are rejected with a configuration error.

pub mod tcli;
pub mod transport;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::config::{data_sources, env_key, DriverConfig, EnvSource, ProcessEnv};
use crate::driver::Driver;
use crate::error::{DriverError, Result};
use crate::escape;
use crate::types::{Column, DatabaseStructure, QueryOptions, QueryResult, SchemaColumn};

use tcli::{
    operation_state, ExecuteStatementResp, FetchResultsResp, GetOperationStatusResp,
    GetResultSetMetadataResp, OpenSessionResp, OperationHandle, Request, SessionHandle, StatusResp,
};
pub use transport::Auth;

/// `getDefaultConcurrency`.
pub const DEFAULT_CONCURRENCY: usize = 2;
/// Default HiveServer2 port.
pub const DEFAULT_PORT: u16 = 10000;
/// `maxRows` of `jshs2`'s `Configuration`.
pub const DEFAULT_MAX_ROWS: i64 = 5120;
/// `authZid`.
pub const DEFAULT_AUTHZID: &str = "cube.js";

/// Authentication mechanism (`auth` option).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AuthMechanism {
    #[default]
    Plain,
    NoSasl,
}

impl AuthMechanism {
    pub fn parse(value: &str) -> Result<Self> {
        match value.to_ascii_uppercase().as_str() {
            "PLAIN" | "NONE" | "LDAP" | "CUSTOM" => Ok(AuthMechanism::Plain),
            "NOSASL" => Ok(AuthMechanism::NoSasl),
            other => Err(DriverError::Config(format!(
                "Unsupported Hive authentication mechanism \"{other}\": the Rust Hive driver \
                 supports PLAIN (hive.server2.authentication=NONE/LDAP/CUSTOM) and NOSASL; \
                 Kerberos (GSSAPI) is not supported."
            ))),
        }
    }
}

/// `hiveType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HiveType {
    #[default]
    Hive,
    Cdh,
}

/// Configuration of [`HiveDriver`].
#[derive(Debug, Clone)]
pub struct HiveConfig {
    /// Connection settings and global driver knobs.
    pub driver: DriverConfig,
    /// `CUBEJS_DB_HOST`.
    pub host: Option<String>,
    /// `CUBEJS_DB_PORT` (default 10000).
    pub port: u16,
    /// `CUBEJS_DB_NAME` (default `default`), used by `tablesSchema`.
    pub db_name: String,
    /// `CUBEJS_DB_USER` (default `anonymous`).
    pub username: String,
    /// `CUBEJS_DB_PASS` (default empty; `None` is sent in the SASL payload).
    pub password: String,
    /// `auth` (`CUBEJS_DB_HIVE_AUTH`, default `PLAIN`).
    pub auth: AuthMechanism,
    /// `authZid` (default `cube.js`).
    pub authzid: String,
    /// Connect / handshake timeout (`timeout`, default 10 s).
    pub timeout: Duration,
    /// `CUBEJS_DB_HIVE_TYPE` (`CDH` or Hive).
    pub hive_type: HiveType,
    /// `CUBEJS_DB_HIVE_VER` (default `2.1.1`): selects the protocol version
    /// requested in `OpenSession`.
    pub hive_ver: String,
    /// `CUBEJS_DB_HIVE_THRIFT_VER` (default `0.9.3`, informational: the
    /// binary protocol is the same in every Thrift version).
    pub thrift_ver: String,
    /// `CUBEJS_DB_HIVE_CDH_VER` (informational).
    pub cdh_ver: Option<String>,
    /// `maxRows` per `FetchResults` (5120).
    pub max_rows: i64,
    /// Pool size (`CUBEJS_DB_MAX_POOL`, default 8).
    pub max_pool_size: usize,
    /// `acquireTimeoutMillis` (20 s).
    pub acquire_timeout: Duration,
    /// `idleTimeoutMillis` (30 s).
    pub idle_timeout: Duration,
    /// Pause between `GetOperationStatus` calls (500 ms).
    pub poll_interval: Duration,
}

impl Default for HiveConfig {
    fn default() -> Self {
        Self::from_driver_config(DriverConfig::default())
    }
}

/// Data-source aware `CUBEJS_*` lookups (`keyByDataSource`).
fn ds_get(env: &dyn EnvSource, driver: &DriverConfig, origin: &str) -> Result<Option<String>> {
    let key = env_key(
        origin,
        &data_sources(env),
        Some(&driver.data_source.data_source),
        driver.data_source.pre_aggregations,
    )?;
    Ok(env.get(&key).filter(|v| !v.is_empty()))
}

impl HiveConfig {
    /// Builds the configuration from a generic [`DriverConfig`].
    pub fn from_driver_config(driver: DriverConfig) -> Self {
        let ds = &driver.data_source;
        Self {
            host: ds.host.clone(),
            port: ds.port.unwrap_or(DEFAULT_PORT),
            db_name: ds.database.clone().unwrap_or_else(|| "default".to_string()),
            username: ds.user.clone().unwrap_or_else(|| "anonymous".to_string()),
            password: ds.password.clone().unwrap_or_default(),
            auth: AuthMechanism::Plain,
            authzid: DEFAULT_AUTHZID.to_string(),
            timeout: Duration::from_millis(10_000),
            hive_type: HiveType::Hive,
            hive_ver: "2.1.1".to_string(),
            thrift_ver: "0.9.3".to_string(),
            cdh_ver: None,
            max_rows: DEFAULT_MAX_ROWS,
            max_pool_size: ds.effective_max_pool_size(),
            acquire_timeout: Duration::from_millis(20_000),
            idle_timeout: Duration::from_millis(30_000),
            poll_interval: Duration::from_millis(500),
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

    /// Reads the `CUBEJS_DB_HIVE_*` variables from the process environment.
    pub fn apply_env(&mut self) -> Result<()> {
        self.apply_env_source(&ProcessEnv)
    }

    /// Reads the `CUBEJS_DB_HIVE_*` variables from `env`.
    pub fn apply_env_source(&mut self, env: &dyn EnvSource) -> Result<()> {
        let driver = self.driver.clone();
        let get = |origin: &str| ds_get(env, &driver, origin);
        self.hive_type = match get("CUBEJS_DB_HIVE_TYPE")?.as_deref() {
            Some("CDH") => HiveType::Cdh,
            _ => HiveType::Hive,
        };
        if let Some(ver) = get("CUBEJS_DB_HIVE_VER")? {
            self.hive_ver = ver;
        }
        if let Some(ver) = get("CUBEJS_DB_HIVE_THRIFT_VER")? {
            self.thrift_ver = ver;
        }
        self.cdh_ver = get("CUBEJS_DB_HIVE_CDH_VER")?;
        if let Some(auth) = get("CUBEJS_DB_HIVE_AUTH")? {
            self.auth = AuthMechanism::parse(&auth)?;
        }
        Ok(())
    }

    /// `client_protocol` sent in `OpenSession`, from `CUBEJS_DB_HIVE_VER`
    /// (the server answers with the lower of its version and this one).
    pub fn client_protocol(&self) -> i32 {
        let mut parts = self
            .hive_ver
            .split('.')
            .map(|p| p.parse::<u32>().unwrap_or(0));
        let (major, minor) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
        if (major, minor) >= (2, 2) {
            tcli::PROTOCOL_V10
        } else {
            tcli::PROTOCOL_V9
        }
    }

    fn transport_auth(&self) -> Auth {
        match self.auth {
            AuthMechanism::NoSasl => Auth::NoSasl,
            AuthMechanism::Plain => Auth::Plain {
                authzid: self.authzid.clone(),
                username: self.username.clone(),
                // TSaslTransport: `password || 'None'`.
                password: if self.password.is_empty() {
                    "None".to_string()
                } else {
                    self.password.clone()
                },
            },
        }
    }
}

/// Strips `_u1.`-style prefixes (`/^_u(.+?)\./`) from a column name.
pub fn clean_column_name(name: &str) -> String {
    if let Some(rest) = name.strip_prefix("_u") {
        let mut chars = rest.char_indices();
        if let Some((_, first)) = chars.next() {
            let skip = first.len_utf8();
            if let Some(dot) = rest[skip..].find('.') {
                return rest[skip + dot + 1..].to_string();
            }
        }
    }
    name.to_string()
}

/// Error for a failed `TStatus`.
fn status_error(status: &tcli::Status, default_message: &str) -> DriverError {
    DriverError::Database {
        message: status.message(default_message),
        code: status.sql_state.clone(),
    }
}

/// An open HiveServer2 session on its own connection.
pub struct Session {
    connection: transport::Connection,
    handle: SessionHandle,
    /// `serverProtocolVersion`.
    pub server_protocol_version: i32,
}

impl Session {
    /// `HiveConnection.connect`: socket, SASL, `OpenSession`.
    pub async fn open(config: &HiveConfig) -> Result<Self> {
        let host = config.host.as_deref().ok_or_else(|| {
            DriverError::Config("CUBEJS_DB_HOST is required for the Hive driver.".to_string())
        })?;
        let mut connection = transport::Connection::connect(
            host,
            config.port,
            &config.transport_auth(),
            config.timeout,
        )
        .await?;
        let configuration = BTreeMap::new();
        let response: OpenSessionResp = connection
            .call(&Request::OpenSession {
                client_protocol: config.client_protocol(),
                username: &config.username,
                password: &config.password,
                configuration: &configuration,
            })
            .await?;
        if response.status.is_error() {
            return Err(DriverError::Connection {
                pool_name: "hive".to_string(),
                message: response
                    .status
                    .message("ExecuteStatement operation fail,... !!"),
            });
        }
        let handle = response
            .session_handle
            .ok_or_else(|| DriverError::Connection {
                pool_name: "hive".to_string(),
                message: "OpenSession returned no session handle".to_string(),
            })?;
        Ok(Self {
            connection,
            handle,
            server_protocol_version: response.server_protocol_version,
        })
    }

    /// `CloseSession` + socket shutdown; errors are ignored.
    pub async fn close(mut self) {
        let _: Result<StatusResp> = self
            .connection
            .call(&Request::CloseSession {
                session: &self.handle,
            })
            .await;
        self.connection.shutdown().await;
    }

    /// `handleQuery` for already interpolated SQL.
    pub async fn execute(&mut self, sql: &str, config: &HiveConfig) -> Result<QueryResult> {
        let response: ExecuteStatementResp = self
            .connection
            .call(&Request::ExecuteStatement {
                session: &self.handle,
                statement: sql,
                run_async: true,
            })
            .await?;
        if response.status.is_error() {
            return Err(status_error(
                &response.status,
                "ExecuteStatement operation fail,... !!",
            ));
        }
        let operation = response.operation_handle.ok_or_else(|| {
            DriverError::Query("ExecuteStatement returned no operation handle".to_string())
        })?;

        let result = self.fetch_operation(&operation, config).await;
        // Close the operation whatever happened; a failure here does not
        // change the outcome of the query.
        let _: Result<StatusResp> = self
            .connection
            .call(&Request::CloseOperation {
                operation: &operation,
            })
            .await;
        result
    }

    async fn fetch_operation(
        &mut self,
        operation: &OperationHandle,
        config: &HiveConfig,
    ) -> Result<QueryResult> {
        loop {
            let status: GetOperationStatusResp = self
                .connection
                .call(&Request::GetOperationStatus { operation })
                .await?;
            let state = status.operation_state.unwrap_or(operation_state::UNKNOWN);
            if status.status.is_error() || state == operation_state::ERROR {
                let message = match status.error_message.as_deref().filter(|m| !m.is_empty()) {
                    Some(m) => m.to_string(),
                    None => status.status.message("ExecuteStatement operation fail"),
                };
                return Err(DriverError::Database {
                    message,
                    code: status.sql_state.or(status.status.sql_state),
                });
            }
            match state {
                operation_state::FINISHED => break,
                operation_state::CANCELED => {
                    return Err(DriverError::Query(
                        "Hive operation was canceled".to_string(),
                    ))
                }
                operation_state::CLOSED => {
                    return Err(DriverError::Query("Hive operation was closed".to_string()))
                }
                operation_state::TIMEDOUT => {
                    return Err(DriverError::Query("Hive operation timed out".to_string()))
                }
                _ => tokio::time::sleep(config.poll_interval).await,
            }
        }

        if !operation.has_result_set {
            return Ok(QueryResult::default());
        }

        let metadata: GetResultSetMetadataResp = self
            .connection
            .call(&Request::GetResultSetMetadata { operation })
            .await?;
        if metadata.status.is_error() {
            return Err(status_error(
                &metadata.status,
                "ExecuteStatement operation fail,... !!",
            ));
        }
        let schema = metadata.columns.unwrap_or_default();
        let columns = schema
            .iter()
            .map(|c| {
                Column::new(
                    clean_column_name(&c.column_name),
                    hive_to_generic_type(&c.type_name, config.driver.precise_decimal_in_cubestore),
                )
            })
            .collect();

        let mut rows = Vec::new();
        loop {
            let fetched: FetchResultsResp = self
                .connection
                .call(&Request::FetchResults {
                    operation,
                    max_rows: config.max_rows,
                })
                .await?;
            if fetched.status.is_error() {
                return Err(status_error(
                    &fetched.status,
                    "ExecuteStatement operation fail,... !!",
                ));
            }
            let block = fetched.results.unwrap_or_default();
            if block.is_empty() {
                break;
            }
            rows.extend(block.into_rows());
        }
        Ok(QueryResult::new(columns, rows))
    }
}

/// Generic type of a result-set column (`TTypeId` name).
pub fn hive_to_generic_type(type_name: &str, precise_decimal: bool) -> crate::types::GenericType {
    use crate::types::GenericType;
    match type_name {
        "tinyint" | "smallint" | "int" => GenericType::Int,
        "float" => GenericType::Float,
        "char" | "varchar" | "string" => GenericType::Text,
        _ => crate::types::to_generic_type(type_name, None, None, precise_decimal),
    }
}

struct Idle {
    session: Session,
    since: Instant,
}

/// Minimal session pool (`generic-pool` settings of the Node.js driver).
struct Pool {
    idle: StdMutex<Vec<Idle>>,
    permits: Arc<Semaphore>,
}

struct Lease {
    session: Session,
    _permit: OwnedSemaphorePermit,
}

impl Pool {
    fn new(size: usize) -> Self {
        Self {
            idle: StdMutex::new(Vec::new()),
            permits: Arc::new(Semaphore::new(size.max(1))),
        }
    }

    async fn acquire(&self, config: &HiveConfig) -> Result<Lease> {
        let permit =
            tokio::time::timeout(config.acquire_timeout, self.permits.clone().acquire_owned())
                .await
                .map_err(|_| DriverError::PoolTimeout("hive".to_string()))?
                .map_err(|_| DriverError::PoolTimeout("hive".to_string()))?;

        loop {
            let idle = self.idle.lock().unwrap().pop();
            match idle {
                Some(idle) if idle.since.elapsed() < config.idle_timeout => {
                    return Ok(Lease {
                        session: idle.session,
                        _permit: permit,
                    })
                }
                Some(expired) => {
                    tokio::spawn(expired.session.close());
                }
                None => break,
            }
        }
        let session = Session::open(config).await?;
        Ok(Lease {
            session,
            _permit: permit,
        })
    }

    fn release(&self, lease: Lease) {
        self.idle.lock().unwrap().push(Idle {
            session: lease.session,
            since: Instant::now(),
        });
    }

    fn drain(&self) -> Vec<Session> {
        self.idle
            .lock()
            .unwrap()
            .drain(..)
            .map(|i| i.session)
            .collect()
    }
}

/// Hive / Spark Thrift Server driver.
pub struct HiveDriver {
    config: HiveConfig,
    pool: Pool,
}

impl std::fmt::Debug for HiveDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HiveDriver")
            .field("host", &self.config.host)
            .field("port", &self.config.port)
            .field("db_name", &self.config.db_name)
            .finish()
    }
}

impl HiveDriver {
    /// Creates the driver. No connection is opened until the first query.
    pub fn new(config: HiveConfig) -> Result<Self> {
        let pool = Pool::new(config.max_pool_size);
        Ok(Self { config, pool })
    }

    /// Creates the driver from `CUBEJS_DB_*` / `CUBEJS_DB_HIVE_*`.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Self::new(HiveConfig::from_env(data_source)?)
    }

    /// The driver configuration.
    pub fn hive_config(&self) -> &HiveConfig {
        &self.config
    }

    /// `SqlString.format(query, values)`. `sqlstring` escapes quotes and
    /// backslashes with a backslash, which is what HiveQL understands (a
    /// doubled `''` is *not* an escape in Hive), i.e. the Spark rules.
    pub fn prepare_sql(&self, sql: &str, params: &[Value]) -> String {
        escape::format(escape::Dialect::Spark, sql, params)
    }

    /// Runs `sql` on a pooled session. A session whose error came from the
    /// server (a failed statement) goes back to the pool; one with a
    /// transport failure is dropped.
    async fn handle_query(&self, sql: &str) -> Result<QueryResult> {
        let mut lease = self.pool.acquire(&self.config).await?;
        let result = lease.session.execute(sql, &self.config).await;
        match &result {
            Ok(_) | Err(DriverError::Database { .. }) => self.pool.release(lease),
            // Dropping the connection closes the socket; HiveServer2 then
            // discards the session.
            Err(_) => drop(lease),
        }
        result
    }
}

#[async_trait]
impl Driver for HiveDriver {
    fn config(&self) -> &DriverConfig {
        &self.config.driver
    }

    /// Opens a dedicated session, runs `SELECT 1` and closes it.
    async fn test_connection(&self) -> Result<()> {
        let mut session = Session::open(&self.config).await?;
        let result = session.execute("SELECT 1", &self.config).await;
        session.close().await;
        result.map(|_| ())
    }

    async fn query(
        &self,
        sql: &str,
        params: &[Value],
        _options: &QueryOptions,
    ) -> Result<QueryResult> {
        self.handle_query(&self.prepare_sql(sql, params)).await
    }

    fn quote_identifier(&self, identifier: &str) -> String {
        format!("`{identifier}`")
    }

    async fn release(&self) -> Result<()> {
        for session in self.pool.drain() {
            session.close().await;
        }
        Ok(())
    }

    async fn tables_schema(&self) -> Result<DatabaseStructure> {
        let db = &self.config.db_name;
        let tables = self.handle_query(&format!("show tables in {db}")).await?;
        let names: Vec<String> = (0..tables.len())
            .filter_map(|i| {
                tables
                    .get_string(i, "tab_name")
                    .or_else(|| tables.get_string(i, "tableName"))
            })
            .collect();
        let described = futures::future::try_join_all(names.iter().map(|table| async move {
            let columns = self.handle_query(&format!("describe {db}.{table}")).await?;
            let columns = (0..columns.len())
                .map(|i| SchemaColumn {
                    name: columns.get_string(i, "col_name").unwrap_or_default(),
                    type_: columns.get_string(i, "data_type").unwrap_or_default(),
                    attributes: Vec::new(),
                    foreign_keys: Vec::new(),
                })
                .collect::<Vec<_>>();
            Ok::<_, DriverError>((table.clone(), columns))
        }))
        .await?;
        let mut structure = DatabaseStructure::new();
        structure.insert(db.clone(), described.into_iter().collect());
        Ok(structure)
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

    fn config_from(pairs: &[(&str, &str)]) -> Result<HiveConfig> {
        let env = env(pairs);
        let driver = DriverConfig::from_env_source(&env, None, false)?;
        let mut config = HiveConfig::from_driver_config(driver);
        config.apply_env_source(&env)?;
        Ok(config)
    }

    #[test]
    fn defaults() {
        let config = config_from(&[("CUBEJS_DB_HOST", "hive")]).unwrap();
        assert_eq!(config.host.as_deref(), Some("hive"));
        assert_eq!(config.port, 10000);
        assert_eq!(config.db_name, "default");
        assert_eq!(config.username, "anonymous");
        assert_eq!(config.password, "");
        assert_eq!(config.auth, AuthMechanism::Plain);
        assert_eq!(config.authzid, "cube.js");
        assert_eq!(config.hive_type, HiveType::Hive);
        assert_eq!(config.hive_ver, "2.1.1");
        assert_eq!(config.thrift_ver, "0.9.3");
        assert_eq!(config.max_rows, 5120);
        assert_eq!(config.max_pool_size, 8);
        assert_eq!(config.timeout, Duration::from_secs(10));
        assert_eq!(config.acquire_timeout, Duration::from_secs(20));
        assert_eq!(config.client_protocol(), tcli::PROTOCOL_V9);
        assert_eq!(
            config.transport_auth(),
            Auth::Plain {
                authzid: "cube.js".into(),
                username: "anonymous".into(),
                password: "None".into()
            }
        );
        assert_eq!(DEFAULT_CONCURRENCY, 2);
    }

    #[test]
    fn env_overrides() {
        let config = config_from(&[
            ("CUBEJS_DB_HOST", "hive"),
            ("CUBEJS_DB_PORT", "10001"),
            ("CUBEJS_DB_NAME", "sales"),
            ("CUBEJS_DB_USER", "u"),
            ("CUBEJS_DB_PASS", "p"),
            ("CUBEJS_DB_MAX_POOL", "3"),
            ("CUBEJS_DB_HIVE_TYPE", "CDH"),
            ("CUBEJS_DB_HIVE_VER", "3.1.3"),
            ("CUBEJS_DB_HIVE_THRIFT_VER", "0.13.0"),
            ("CUBEJS_DB_HIVE_CDH_VER", "6.3"),
            ("CUBEJS_DB_HIVE_AUTH", "nosasl"),
        ])
        .unwrap();
        assert_eq!(config.port, 10001);
        assert_eq!(config.db_name, "sales");
        assert_eq!(config.username, "u");
        assert_eq!(config.password, "p");
        assert_eq!(config.max_pool_size, 3);
        assert_eq!(config.hive_type, HiveType::Cdh);
        assert_eq!(config.client_protocol(), tcli::PROTOCOL_V10);
        assert_eq!(config.thrift_ver, "0.13.0");
        assert_eq!(config.cdh_ver.as_deref(), Some("6.3"));
        assert_eq!(config.auth, AuthMechanism::NoSasl);
        assert_eq!(config.transport_auth(), Auth::NoSasl);

        let err = config_from(&[("CUBEJS_DB_HIVE_AUTH", "KERBEROS")]).unwrap_err();
        assert!(err
            .to_string()
            .contains("Kerberos (GSSAPI) is not supported"));

        // data-source aware
        let env = env(&[
            ("CUBEJS_DATASOURCES", "default,warehouse"),
            ("CUBEJS_DS_WAREHOUSE_DB_HOST", "hive2"),
            ("CUBEJS_DS_WAREHOUSE_DB_HIVE_VER", "2.3.4"),
        ]);
        let driver = DriverConfig::from_env_source(&env, Some("warehouse"), false).unwrap();
        let mut config = HiveConfig::from_driver_config(driver);
        config.apply_env_source(&env).unwrap();
        assert_eq!(config.host.as_deref(), Some("hive2"));
        assert_eq!(config.hive_ver, "2.3.4");
    }

    #[test]
    fn column_names_and_sql() {
        assert_eq!(clean_column_name("_u1.id"), "id");
        assert_eq!(clean_column_name("_u12.name"), "name");
        assert_eq!(clean_column_name("t.id"), "t.id");
        assert_eq!(clean_column_name("_u.x"), "_u.x");
        assert_eq!(clean_column_name("id"), "id");

        let driver = HiveDriver::new(config_from(&[("CUBEJS_DB_HOST", "h")]).unwrap()).unwrap();
        assert_eq!(driver.quote_identifier("a"), "`a`");
        assert_eq!(driver.param(0), "?");
        assert!(!driver.read_only());
        assert_eq!(
            driver.capabilities(),
            crate::types::DriverCapabilities::default()
        );
        assert_eq!(
            driver.prepare_sql(
                "SELECT * FROM t WHERE a = ? AND b = ?",
                &[Value::from("it's"), Value::from(1)]
            ),
            "SELECT * FROM t WHERE a = 'it\\'s' AND b = 1"
        );
        assert_eq!(
            driver.wrap_query_with_limit("SELECT 1", 5),
            "SELECT * FROM (SELECT 1) AS t LIMIT 5"
        );
    }

    #[test]
    fn result_types() {
        use crate::types::GenericType;
        assert_eq!(hive_to_generic_type("int", false), GenericType::Int);
        assert_eq!(hive_to_generic_type("smallint", false), GenericType::Int);
        assert_eq!(hive_to_generic_type("bigint", false), GenericType::Bigint);
        assert_eq!(hive_to_generic_type("string", false), GenericType::Text);
        assert_eq!(hive_to_generic_type("varchar", false), GenericType::Text);
        assert_eq!(hive_to_generic_type("double", false), GenericType::Double);
        assert_eq!(
            hive_to_generic_type("decimal", false),
            GenericType::Decimal(None)
        );
        assert_eq!(
            hive_to_generic_type("timestamp", false),
            GenericType::Timestamp
        );
        assert_eq!(hive_to_generic_type("date", false), GenericType::Date);
        assert_eq!(hive_to_generic_type("boolean", false), GenericType::Boolean);
    }

    #[tokio::test]
    async fn missing_host_is_a_config_error() {
        let driver = HiveDriver::new(HiveConfig::default()).unwrap();
        let err = driver
            .query("SELECT 1", &[], &QueryOptions::default())
            .await
            .unwrap_err();
        assert!(matches!(err, DriverError::Config(_)), "{err}");
    }
}
