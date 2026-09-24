//! MS SQL driver: port of `@cubejs-backend/mssql-driver` on top of
//! `tiberius` (a pure-Rust TDS implementation).
//!
//! # TLS
//!
//! Built with `tiberius`' `rustls` feature, so `CUBEJS_DB_SSL=true` encrypts
//! the connection and Azure SQL, which requires encryption, works. That
//! feature brings `rustls-native-certs` and with it `openssl-probe`, whose
//! name is the only OpenSSL thing about it: the crate has no dependencies, no
//! build script, and does nothing but look up the system certificate paths.
//! Nothing links OpenSSL.
//!
//! `CUBEJS_DB_SSL_REJECT_UNAUTHORIZED` and `CUBEJS_DB_SSL_CA` have the same
//! meaning as in the other drivers. Client certificates
//! (`CUBEJS_DB_SSL_CERT` / `_KEY`) are refused: `tiberius` offers no way to
//! present one.

pub mod types;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::Value;
use tiberius::{AuthMethod, Client, ColumnData, Config, EncryptionLevel, IntoSql, Query, ToSql};
use tokio::net::TcpStream;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

use crate::config::DriverConfig;
use crate::driver::Driver;
use crate::error::{DriverError, Result};
use crate::types::{
    Column, DownloadQueryResultsOptions, DownloadedData, DriverCapabilities, GenericType,
    QueryOptions, QueryResult, Row, StreamOptions, StreamTableData, TableStructure,
};

pub use types::{generic_to_mssql, mssql_to_generic, to_generic_type};

/// Default TDS port.
pub const DEFAULT_PORT: u16 = 1433;
/// `getDefaultConcurrency`.
pub const DEFAULT_CONCURRENCY: usize = 2;
/// Default pool size (`pool.max`).
pub const DEFAULT_MAX_POOL_SIZE: usize = 8;

type TdsClient = Client<Compat<TcpStream>>;

/// Configuration of [`MsSqlDriver`] (`MSSqlDriverConfiguration`).
#[derive(Debug, Clone)]
pub struct MsSqlConfig {
    /// Connection settings and global driver knobs.
    pub driver: DriverConfig,
    /// `CUBEJS_DB_HOST` (`server`).
    pub server: String,
    /// `CUBEJS_DB_PORT` (default 1433).
    pub port: u16,
    /// `CUBEJS_DB_NAME`.
    pub database: Option<String>,
    /// `CUBEJS_DB_USER`.
    pub user: Option<String>,
    /// `CUBEJS_DB_PASS`.
    pub password: Option<String>,
    /// `CUBEJS_DB_DOMAIN` (Windows authentication).
    pub domain: Option<String>,
    /// `requestTimeout` (`CUBEJS_DB_QUERY_TIMEOUT`).
    pub request_timeout: Duration,
    /// `pool.max` (default 8).
    pub max_pool_size: usize,
    /// `pool.acquireTimeoutMillis` (default 20 s).
    pub acquire_timeout: Duration,
    /// `readOnly` (default `true`).
    pub read_only: bool,
    /// Holds an inline `CUBEJS_DB_SSL_CA` on disk, because tiberius reads the
    /// certificate from a path. Dropped with the configuration.
    ca_file: Arc<Mutex<Option<Arc<tempfile::NamedTempFile>>>>,
}

impl MsSqlConfig {
    /// Builds the MS SQL configuration from a generic [`DriverConfig`].
    pub fn from_driver_config(driver: DriverConfig) -> Self {
        let ds = &driver.data_source;
        Self {
            server: ds.host.clone().unwrap_or_default(),
            port: ds.port.unwrap_or(DEFAULT_PORT),
            database: ds.database.clone(),
            user: ds.user.clone(),
            password: ds.password.clone(),
            domain: None,
            request_timeout: ds.query_timeout,
            max_pool_size: ds
                .max_pool_size
                .filter(|v| *v > 0)
                .unwrap_or(DEFAULT_MAX_POOL_SIZE),
            acquire_timeout: Duration::from_millis(20_000),
            read_only: true,
            ca_file: Arc::new(Mutex::new(None)),
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

    /// Reads the MS SQL specific variables into an existing configuration.
    pub fn apply_env(&mut self) {
        self.domain = std::env::var("CUBEJS_DB_DOMAIN")
            .ok()
            .filter(|v| !v.is_empty());
    }

    /// Reads the configuration from a `mssql://user:pass@host:port/db` URL,
    /// for tests and tooling.
    pub fn from_url(url: &str) -> Result<Self> {
        let parsed = url::Url::parse(url)
            .map_err(|e| DriverError::Config(format!("Invalid MS SQL URL: {e}")))?;
        let mut driver = DriverConfig::default();
        driver.data_source.host = parsed.host_str().map(|h| h.to_string());
        driver.data_source.port = Some(parsed.port().unwrap_or(DEFAULT_PORT));
        if !parsed.username().is_empty() {
            let user = percent_decode(parsed.username());
            driver.data_source.user = Some(user);
        }
        driver.data_source.password = parsed.password().map(percent_decode);
        let path = parsed.path().trim_start_matches('/');
        if !path.is_empty() {
            driver.data_source.database = Some(path.to_string());
        }
        Ok(Self::from_driver_config(driver))
    }

    /// Translates the Cube configuration into a `tiberius::Config`.
    pub fn tiberius_config(&self) -> Result<Config> {
        let mut config = Config::new();
        config.host(if self.server.is_empty() {
            "localhost"
        } else {
            &self.server
        });
        config.port(self.port);
        if let Some(database) = &self.database {
            config.database(database);
        }
        let user = self.user.clone().unwrap_or_default();
        let password = self.password.clone().unwrap_or_default();
        if self.domain.as_ref().is_some_and(|d| !d.is_empty()) {
            // `AuthMethod::windows` only exists on Windows; on Unix tiberius
            // needs the `integrated-auth-gssapi` feature, which links the
            // system GSSAPI library (a C dependency this build excludes).
            return Err(DriverError::Config(
                "CUBEJS_DB_DOMAIN (Windows authentication) is not supported by the Rust MS SQL \
                 driver: it would need tiberius' GSSAPI integration, which links a C library. \
                 Use SQL Server authentication (CUBEJS_DB_USER / CUBEJS_DB_PASS) instead."
                    .to_string(),
            ));
        }
        config.authentication(AuthMethod::sql_server(user, password));
        self.apply_encryption(&mut config)?;
        config.application_name("cubejs");
        Ok(config)
    }

    /// Maps `CUBEJS_DB_SSL*` onto tiberius' encryption and trust settings.
    ///
    /// Node's driver passes `encrypt: getEnv('dbSsl')` to `mssql`, so an unset
    /// `CUBEJS_DB_SSL` means an unencrypted connection.
    fn apply_encryption(&self, config: &mut Config) -> Result<()> {
        let Some(ssl) = &self.driver.data_source.ssl else {
            config.encryption(EncryptionLevel::NotSupported);
            return Ok(());
        };

        if ssl.cert.is_some() || ssl.key.is_some() {
            return Err(DriverError::Config(
                "CUBEJS_DB_SSL_CERT / CUBEJS_DB_SSL_KEY (client certificates) are not supported \
                 by the Rust MS SQL driver: tiberius offers no way to present one."
                    .to_string(),
            ));
        }
        if ssl.passphrase.is_some() {
            return Err(DriverError::Config(
                "CUBEJS_DB_SSL_PASSPHRASE (encrypted private keys) is not supported by the Rust \
                 MS SQL driver."
                    .to_string(),
            ));
        }
        if ssl.ciphers.is_some() {
            log::warn!(
                "CUBEJS_DB_SSL_CIPHERS is ignored: rustls does not accept OpenSSL cipher strings"
            );
        }
        if ssl.servername.is_some() {
            log::warn!(
                "CUBEJS_DB_SSL_SERVERNAME is ignored by the MS SQL driver: tiberius verifies \
                 against the host it connects to"
            );
        }

        config.encryption(EncryptionLevel::Required);

        // `rejectUnauthorized` defaults to false in Cube, which is what makes
        // a self-signed development server work out of the box.
        if !ssl.reject_unauthorized {
            config.trust_cert();
            return Ok(());
        }

        // An explicit CA is validated in addition to the system trust store.
        // tiberius reads it from a file, so inline PEM is materialised into
        // one that lives as long as this configuration.
        if let Some(ca) = &ssl.ca {
            config.trust_cert_ca(self.ca_path(ca)?);
        }

        Ok(())
    }

    /// The path tiberius reads the CA from. `CUBEJS_DB_SSL_CA` may be a path
    /// or the certificate itself, and only the first can be handed over
    /// directly.
    fn ca_path(&self, ca: &str) -> Result<String> {
        let as_path = std::path::Path::new(ca);
        if as_path.is_file() {
            return Ok(ca.to_string());
        }

        if !ca.contains("BEGIN CERTIFICATE") {
            return Err(DriverError::Config(format!(
                "CUBEJS_DB_SSL_CA is neither a readable file nor a PEM certificate: {ca}"
            )));
        }

        let mut file = self.ca_file.lock().map_err(|_| {
            DriverError::Config("The CA certificate file lock was poisoned".to_string())
        })?;

        if file.is_none() {
            let temp = tempfile::Builder::new()
                .prefix("cube-mssql-ca-")
                .suffix(".pem")
                .tempfile()
                .map_err(|e| {
                    DriverError::Config(format!("Failed to materialise CUBEJS_DB_SSL_CA: {e}"))
                })?;
            std::fs::write(temp.path(), ca).map_err(|e| {
                DriverError::Config(format!("Failed to write CUBEJS_DB_SSL_CA: {e}"))
            })?;
            *file = Some(Arc::new(temp));
        }

        Ok(file
            .as_ref()
            .expect("just written")
            .path()
            .display()
            .to_string())
    }
}

/// Decodes the `%XX` escapes of a URL user info component.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&input[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A tiny connection pool (`mssql`'s `ConnectionPool` equivalent).
struct Pool {
    config: Config,
    permits: Arc<Semaphore>,
    idle: Mutex<Vec<TdsClient>>,
    acquire_timeout: Duration,
}

impl Pool {
    fn new(config: Config, max_size: usize, acquire_timeout: Duration) -> Self {
        Self {
            config,
            permits: Arc::new(Semaphore::new(max_size.max(1))),
            idle: Mutex::new(Vec::new()),
            acquire_timeout,
        }
    }

    async fn acquire(self: &Arc<Self>) -> Result<PooledClient> {
        let permit =
            tokio::time::timeout(self.acquire_timeout, self.permits.clone().acquire_owned())
                .await
                .map_err(|_| DriverError::PoolTimeout("mssql".to_string()))?
                .map_err(|_| DriverError::Query("MS SQL pool is closed".to_string()))?;

        let pooled = self.idle.lock().unwrap().pop();
        let client = match pooled {
            Some(client) => client,
            None => connect(self.config.clone()).await?,
        };
        Ok(PooledClient {
            client: Some(client),
            pool: self.clone(),
            _permit: permit,
        })
    }

    fn close(&self) {
        self.idle.lock().unwrap().clear();
    }

    fn size(&self) -> usize {
        self.idle.lock().unwrap().len()
    }
}

async fn connect(config: Config) -> Result<TdsClient> {
    let addr = config.get_addr().to_string();
    let tcp = TcpStream::connect(config.get_addr())
        .await
        .map_err(|e| DriverError::Connection {
            pool_name: addr.clone(),
            message: e.to_string(),
        })?;
    tcp.set_nodelay(true).ok();
    Client::connect(config, tcp.compat_write())
        .await
        .map_err(|e| DriverError::Connection {
            pool_name: addr,
            message: tds_message(&e),
        })
}

/// A pooled connection; returned to the pool when dropped.
struct PooledClient {
    client: Option<TdsClient>,
    pool: Arc<Pool>,
    _permit: OwnedSemaphorePermit,
}

impl PooledClient {
    fn client(&mut self) -> &mut TdsClient {
        self.client.as_mut().expect("client is taken")
    }

    /// Takes the connection out of the pool permanently (streaming).
    fn into_inner(mut self) -> TdsClient {
        self.client.take().expect("client is taken")
    }
}

impl Drop for PooledClient {
    fn drop(&mut self) {
        if let Some(client) = self.client.take() {
            self.pool.idle.lock().unwrap().push(client);
        }
    }
}

/// A query parameter that can be bound to a TDS statement.
#[derive(Debug, Clone, PartialEq)]
pub enum Param {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
}

impl Param {
    /// Maps one JSON parameter (`request.input(...)` in the Node driver).
    pub fn from_json(value: &Value) -> Self {
        match value {
            Value::Null => Param::Null,
            Value::Bool(b) => Param::Bool(*b),
            Value::Number(n) if n.is_f64() => Param::Float(n.as_f64().unwrap_or_default()),
            Value::Number(n) => Param::Int(n.as_i64().unwrap_or_default()),
            Value::String(s) => Param::Text(s.clone()),
            other => Param::Text(other.to_string()),
        }
    }
}

impl<'a> IntoSql<'a> for &'a Param {
    fn into_sql(self) -> ColumnData<'a> {
        ToSql::to_sql(self)
    }
}

impl ToSql for Param {
    fn to_sql(&self) -> ColumnData<'_> {
        match self {
            Param::Null => ColumnData::String(None),
            Param::Bool(b) => ColumnData::Bit(Some(*b)),
            Param::Int(i) => ColumnData::I64(Some(*i)),
            Param::Float(f) => ColumnData::F64(Some(*f)),
            Param::Text(s) => ColumnData::String(Some(s.as_str().into())),
        }
    }
}

/// The SQL actually sent to the server: placeholders are only rewritten when
/// the statement carries parameters (a parameterless statement is a plain
/// batch, where `@_1` can only be literal text).
fn prepare_sql(sql: &str, params: &[Value]) -> String {
    if params.is_empty() {
        sql.to_string()
    } else {
        rewrite_parameter_placeholders(sql)
    }
}

/// Rewrites the `@_1`-style placeholders Cube generates into the `@P1` ones
/// `tiberius` declares for `sp_executesql`.
///
/// String literals (`'…'`), quoted (`"…"`) and bracketed (`[…]`) identifiers
/// are left untouched.
pub fn rewrite_parameter_placeholders(sql: &str) -> String {
    let bytes: Vec<char> = sql.chars().collect();
    let mut out = String::with_capacity(sql.len());
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            '\'' | '"' => {
                let quote = c;
                out.push(c);
                i += 1;
                while i < bytes.len() {
                    out.push(bytes[i]);
                    if bytes[i] == quote {
                        // A doubled quote is an escaped one.
                        if bytes.get(i + 1) == Some(&quote) {
                            out.push(quote);
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            '[' => {
                out.push(c);
                i += 1;
                while i < bytes.len() {
                    out.push(bytes[i]);
                    if bytes[i] == ']' {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            '@' if bytes.get(i + 1) == Some(&'_')
                && bytes.get(i + 2).is_some_and(|c| c.is_ascii_digit()) =>
            {
                out.push('@');
                out.push('P');
                i += 2;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// MS SQL driver.
pub struct MsSqlDriver {
    config: MsSqlConfig,
    pool: Arc<Pool>,
}

impl std::fmt::Debug for MsSqlDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MsSqlDriver")
            .field("server", &self.config.server)
            .field("database", &self.config.database)
            .finish()
    }
}

impl MsSqlDriver {
    /// Creates the driver and its (lazy) connection pool.
    pub fn new(config: MsSqlConfig) -> Result<Self> {
        let tiberius_config = config.tiberius_config()?;
        let pool = Arc::new(Pool::new(
            tiberius_config,
            config.max_pool_size,
            config.acquire_timeout,
        ));
        Ok(Self { config, pool })
    }

    /// Creates the driver from `CUBEJS_DB_*`.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Self::new(MsSqlConfig::from_env(data_source)?)
    }

    /// The driver configuration.
    pub fn mssql_config(&self) -> &MsSqlConfig {
        &self.config
    }

    /// Number of idle pooled connections.
    pub fn pool_size(&self) -> usize {
        self.pool.size()
    }

    fn build_query<'a>(sql: &'a str, params: &'a [Param]) -> Query<'a> {
        let mut query = Query::new(sql);
        for param in params {
            query.bind(param);
        }
        query
    }

    /// `mapFields` for a result set.
    fn map_fields(&self, columns: &[tiberius::Column]) -> Vec<Column> {
        columns
            .iter()
            .map(|c| {
                Column::new(
                    c.name(),
                    self.to_generic_type(types::column_type_name(c.column_type()), None, None),
                )
            })
            .collect()
    }

    async fn query_response(&self, sql: &str, params: &[Value]) -> Result<QueryResult> {
        let sql = prepare_sql(sql, params);
        let bound: Vec<Param> = params.iter().map(Param::from_json).collect();
        let mut client = self.pool.acquire().await?;

        let run = async {
            // Statements without parameters go out as a plain SQL batch, like
            // `mssql` does: `sp_executesql` refuses `CREATE SCHEMA`, which has
            // to be the first statement of its batch.
            let stream = if bound.is_empty() {
                client.client().simple_query(sql.as_str()).await
            } else {
                Self::build_query(&sql, &bound).query(client.client()).await
            }
            .map_err(|e| DriverError::Database {
                message: tds_message(&e),
                code: tds_code(&e),
            })?;
            let rows = stream
                .into_first_result()
                .await
                .map_err(|e| DriverError::Database {
                    message: tds_message(&e),
                    code: tds_code(&e),
                })?;

            let columns = match rows.first() {
                Some(row) => self.map_fields(row.columns()),
                None => Vec::new(),
            };
            let converted: Result<Vec<Row>> = rows
                .iter()
                .map(|row| {
                    let column_types: Vec<_> =
                        row.columns().iter().map(|c| c.column_type()).collect();
                    column_types
                        .iter()
                        .enumerate()
                        .map(|(i, t)| types::cell_to_value(row, i, *t))
                        .collect()
                })
                .collect();
            Ok(QueryResult::new(columns, converted?))
        };

        tokio::time::timeout(self.config.request_timeout, run)
            .await
            .map_err(|_| {
                DriverError::Query(format!(
                    "MS SQL request timeout reached {}ms",
                    self.config.request_timeout.as_millis()
                ))
            })?
    }
}

/// Human readable message of a TDS error.
fn tds_message(error: &tiberius::error::Error) -> String {
    match error {
        tiberius::error::Error::Server(token) => token.message().to_string(),
        other => other.to_string(),
    }
}

fn tds_code(error: &tiberius::error::Error) -> Option<String> {
    match error {
        tiberius::error::Error::Server(token) => Some(token.code().to_string()),
        _ => None,
    }
}

#[async_trait]
impl Driver for MsSqlDriver {
    fn config(&self) -> &DriverConfig {
        &self.config.driver
    }

    async fn test_connection(&self) -> Result<()> {
        tokio::time::timeout(
            self.config.driver.test_connection_timeout,
            self.query("SELECT 1 as number", &[], &QueryOptions::default()),
        )
        .await
        .map_err(|_| DriverError::Connection {
            pool_name: self.config.server.clone(),
            message: format!(
                "connection timed out after {:?}",
                self.config.driver.test_connection_timeout
            ),
        })??;
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

    fn param(&self, index: usize) -> String {
        format!("@_{}", index + 1)
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

    #[allow(clippy::wrong_self_convention)]
    fn from_generic_type(&self, generic: &GenericType) -> String {
        types::generic_to_mssql(generic)
    }

    fn read_only(&self) -> bool {
        self.config.read_only
    }

    fn capabilities(&self) -> DriverCapabilities {
        DriverCapabilities {
            incremental_schema_loading: true,
            ..Default::default()
        }
    }

    fn wrap_query_with_limit(&self, query: &str, limit: u64) -> String {
        format!("SELECT TOP {limit} * FROM ({query}) AS t")
    }

    /// Fixes "The multipart identifier \"columns.data_type\" could not be bound".
    fn information_schema_query(&self) -> String {
        format!(
            "
      SELECT column_name as {},
        table_name as {},
        table_schema as {},
        data_type as {}
      FROM INFORMATION_SCHEMA.COLUMNS
      WHERE table_schema NOT IN ('information_schema', 'sys')
    ",
            self.quote_identifier("column_name"),
            self.quote_identifier("table_name"),
            self.quote_identifier("table_schema"),
            self.quote_identifier("data_type"),
        )
    }

    async fn table_column_types(&self, table: &str) -> Result<TableStructure> {
        let name = crate::types::TableName::split(table);
        let table_name = name.name.split('.').next().unwrap_or("").to_string();
        let sql = format!(
            "SELECT column_name as {},
             table_name as {},
             table_schema as {},
             data_type  as {},
             numeric_precision AS {},
             numeric_scale AS {}
      FROM INFORMATION_SCHEMA.COLUMNS
      WHERE table_name = {} AND table_schema = {}",
            self.quote_identifier("column_name"),
            self.quote_identifier("table_name"),
            self.quote_identifier("table_schema"),
            self.quote_identifier("data_type"),
            self.quote_identifier("numeric_precision"),
            self.quote_identifier("numeric_scale"),
            self.param(0),
            self.param(1),
        );
        let result = self
            .query(
                &sql,
                &[Value::from(table_name), Value::from(name.schema)],
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

    async fn get_tables_query(&self, schema_name: &str) -> Result<Vec<String>> {
        let result = self
            .query(
                &format!(
                    "SELECT table_name FROM INFORMATION_SCHEMA.TABLES WHERE table_schema = {}",
                    self.param(0)
                ),
                &[Value::from(schema_name)],
                &QueryOptions::default(),
            )
            .await?;
        Ok((0..result.len())
            .filter_map(|i| {
                result
                    .get_string(i, "table_name")
                    .or_else(|| result.get_string(i, "TABLE_NAME"))
            })
            .collect())
    }

    async fn create_schema_if_not_exists(&self, schema_name: &str) -> Result<()> {
        let existing = self
            .query(
                &format!(
                    "SELECT schema_name FROM INFORMATION_SCHEMA.SCHEMATA WHERE schema_name = {}",
                    self.param(0)
                ),
                &[Value::from(schema_name)],
                &QueryOptions::default(),
            )
            .await?;
        if existing.is_empty() {
            self.query(
                &format!("CREATE SCHEMA {schema_name}"),
                &[],
                &QueryOptions::default(),
            )
            .await?;
        }
        Ok(())
    }

    async fn stream(
        &self,
        sql: &str,
        params: &[Value],
        options: &StreamOptions,
    ) -> Result<StreamTableData> {
        let sql = prepare_sql(sql, params);
        let bound: Vec<Param> = params.iter().map(Param::from_json).collect();
        // The connection travels with the stream: it is dropped (and the
        // permit released) when the task finishes.
        let client = self.pool.acquire().await?.into_inner();
        let (columns_tx, columns_rx) = tokio::sync::oneshot::channel::<Result<Vec<Column>>>();
        let (rows_tx, rows_rx) =
            tokio::sync::mpsc::channel::<Result<Row>>(options.high_water_mark.clamp(1, 16_000));

        let precise = self.config.driver.precise_decimal_in_cubestore;
        tokio::spawn(async move {
            let mut client = client;
            let stream = if bound.is_empty() {
                client.simple_query(sql.as_str()).await
            } else {
                Self::build_query(&sql, &bound).query(&mut client).await
            };
            let mut stream = match stream {
                Ok(stream) => stream,
                Err(e) => {
                    let _ = columns_tx.send(Err(DriverError::Database {
                        message: tds_message(&e),
                        code: tds_code(&e),
                    }));
                    return;
                }
            };

            let mut columns_sent = false;
            let mut columns_tx = Some(columns_tx);
            while let Some(item) = stream.next().await {
                match item {
                    Ok(tiberius::QueryItem::Metadata(metadata)) => {
                        let columns: Vec<Column> = metadata
                            .columns()
                            .iter()
                            .map(|c| {
                                Column::new(
                                    c.name(),
                                    types::to_generic_type(
                                        types::column_type_name(c.column_type()),
                                        None,
                                        None,
                                        precise,
                                    ),
                                )
                            })
                            .collect();
                        if let Some(tx) = columns_tx.take() {
                            let _ = tx.send(Ok(columns));
                            columns_sent = true;
                        }
                    }
                    Ok(tiberius::QueryItem::Row(row)) => {
                        let types: Vec<_> = row.columns().iter().map(|c| c.column_type()).collect();
                        let converted: Result<Row> = types
                            .iter()
                            .enumerate()
                            .map(|(i, t)| types::cell_to_value(&row, i, *t))
                            .collect();
                        if rows_tx.send(converted).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let error = DriverError::Database {
                            message: tds_message(&e),
                            code: tds_code(&e),
                        };
                        match columns_tx.take() {
                            Some(tx) => {
                                let _ = tx.send(Err(error));
                            }
                            None => {
                                let _ = rows_tx.send(Err(error)).await;
                            }
                        }
                        return;
                    }
                }
            }
            if !columns_sent {
                if let Some(tx) = columns_tx.take() {
                    let _ = tx.send(Ok(Vec::new()));
                }
            }
        });

        let columns = columns_rx
            .await
            .map_err(|_| DriverError::Query("MS SQL stream ended unexpectedly".to_string()))??;
        let rows = tokio_stream_rows(rows_rx);
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
        Ok(DownloadedData::Memory(
            self.query_response(sql, params).await?,
        ))
    }

    async fn release(&self) -> Result<()> {
        self.pool.close();
        Ok(())
    }
}

/// Wraps the row channel into the stream shape the trait expects.
fn tokio_stream_rows(
    rx: tokio::sync::mpsc::Receiver<Result<Row>>,
) -> futures::stream::BoxStream<'static, Result<Row>> {
    futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    })
    .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn driver() -> MsSqlDriver {
        let mut config = MsSqlConfig::from_driver_config(DriverConfig::default());
        config.server = "db.local".to_string();
        config.database = Some("cube".to_string());
        MsSqlDriver::new(config).unwrap()
    }

    #[test]
    fn placeholders_are_rewritten() {
        assert_eq!(
            rewrite_parameter_placeholders("SELECT * FROM t WHERE a = @_1 AND b = @_12"),
            "SELECT * FROM t WHERE a = @P1 AND b = @P12"
        );
        // literals and identifiers are untouched
        assert_eq!(
            rewrite_parameter_placeholders("SELECT '@_1' AS [@_2], \"@_3\" FROM t WHERE x = @_4"),
            "SELECT '@_1' AS [@_2], \"@_3\" FROM t WHERE x = @P4"
        );
        assert_eq!(
            rewrite_parameter_placeholders("SELECT 'it''s @_1' , @_2"),
            "SELECT 'it''s @_1' , @P2"
        );
        // named variables that are not Cube placeholders stay as they are
        assert_eq!(
            rewrite_parameter_placeholders("DECLARE @x int; SELECT @x, @_1"),
            "DECLARE @x int; SELECT @x, @P1"
        );
    }

    #[test]
    fn an_unencrypted_connection_is_the_default() {
        // Node passes `encrypt: getEnv('dbSsl')`, which is false unless set.
        let config = MsSqlConfig::from_driver_config(DriverConfig::default());
        assert!(config.tiberius_config().is_ok());
        assert!(MsSqlDriver::new(config).is_ok());
    }

    #[test]
    fn ssl_turns_on_encryption() {
        let mut driver_config = DriverConfig::default();
        driver_config.data_source.ssl = Some(crate::config::SslConfig::default());
        let config = MsSqlConfig::from_driver_config(driver_config);

        // The configuration builds and the driver connects over TLS; Azure
        // SQL, which mandates encryption, therefore works.
        assert!(config.tiberius_config().is_ok());
        assert!(MsSqlDriver::new(config).is_ok());
    }

    #[test]
    fn an_inline_ca_is_materialised_for_tiberius() {
        const CA: &str = "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n";

        let mut driver_config = DriverConfig::default();
        driver_config.data_source.ssl = Some(crate::config::SslConfig {
            ca: Some(CA.to_string()),
            reject_unauthorized: true,
            ..crate::config::SslConfig::default()
        });
        let config = MsSqlConfig::from_driver_config(driver_config);

        // tiberius reads the CA from a path, so inline PEM lands in a file
        // that holds the same bytes.
        let path = config.ca_path(CA).expect("materialised");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), CA);
        // The same configuration reuses the file rather than writing a new one.
        assert_eq!(config.ca_path(CA).unwrap(), path);
        assert!(config.tiberius_config().is_ok());
    }

    #[test]
    fn a_ca_path_is_passed_through() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "-----BEGIN CERTIFICATE-----\n").unwrap();
        let path = file.path().display().to_string();

        let config = MsSqlConfig::from_driver_config(DriverConfig::default());
        assert_eq!(config.ca_path(&path).unwrap(), path);
    }

    #[test]
    fn a_ca_that_is_neither_a_file_nor_a_certificate_is_refused() {
        let config = MsSqlConfig::from_driver_config(DriverConfig::default());
        let err = config.ca_path("/no/such/file").unwrap_err();
        assert!(err.to_string().contains("CUBEJS_DB_SSL_CA"), "{err}");
    }

    #[test]
    fn client_certificates_are_refused() {
        for ssl in [
            crate::config::SslConfig {
                cert: Some("cert".into()),
                ..crate::config::SslConfig::default()
            },
            crate::config::SslConfig {
                key: Some("key".into()),
                ..crate::config::SslConfig::default()
            },
            crate::config::SslConfig {
                passphrase: Some("secret".into()),
                ..crate::config::SslConfig::default()
            },
        ] {
            let mut driver_config = DriverConfig::default();
            driver_config.data_source.ssl = Some(ssl);
            let config = MsSqlConfig::from_driver_config(driver_config);
            let err = config.tiberius_config().unwrap_err();
            assert!(err.to_string().contains("CUBEJS_DB_SSL"), "{err}");
        }
    }

    #[test]
    fn tiberius_configuration() {
        let mut driver_config = DriverConfig::default();
        driver_config.data_source.host = Some("db.local".into());
        driver_config.data_source.port = Some(1444);
        driver_config.data_source.user = Some("sa".into());
        driver_config.data_source.password = Some("pw".into());
        driver_config.data_source.database = Some("cube".into());
        let config = MsSqlConfig::from_driver_config(driver_config);
        assert_eq!(config.port, 1444);
        let tds = config.tiberius_config().unwrap();
        assert_eq!(tds.get_addr(), "db.local:1444");

        let mut with_domain = config.clone();
        with_domain.domain = Some("CORP".into());
        let err = with_domain.tiberius_config().unwrap_err();
        assert!(err.to_string().contains("CUBEJS_DB_DOMAIN"));
    }

    #[test]
    fn config_from_url() {
        let config =
            MsSqlConfig::from_url("mssql://sa:Cube_test_123%21@127.0.0.1:21433/master").unwrap();
        assert_eq!(config.server, "127.0.0.1");
        assert_eq!(config.port, 21433);
        assert_eq!(config.user.as_deref(), Some("sa"));
        assert_eq!(config.password.as_deref(), Some("Cube_test_123!"));
        assert_eq!(config.database.as_deref(), Some("master"));
    }

    #[test]
    fn parameters_are_mapped() {
        assert_eq!(Param::from_json(&Value::Null), Param::Null);
        assert_eq!(Param::from_json(&Value::from(7)), Param::Int(7));
        assert_eq!(Param::from_json(&Value::from(1.5)), Param::Float(1.5));
        assert_eq!(Param::from_json(&Value::Bool(true)), Param::Bool(true));
        assert_eq!(
            Param::from_json(&Value::from("x")),
            Param::Text("x".to_string())
        );
        assert!(matches!(Param::Null.to_sql(), ColumnData::String(None)));
        assert!(matches!(Param::Int(3).to_sql(), ColumnData::I64(Some(3))));
    }

    #[tokio::test]
    async fn sql_and_type_mapping() {
        let driver = driver();
        assert_eq!(driver.param(0), "@_1");
        assert_eq!(driver.param(4), "@_5");
        assert_eq!(driver.quote_identifier("a"), "\"a\"");
        assert!(driver.read_only());
        assert!(driver.capabilities().incremental_schema_loading);
        assert_eq!(
            driver.wrap_query_with_limit("SELECT 1", 10),
            "SELECT TOP 10 * FROM (SELECT 1) AS t"
        );
        assert_eq!(
            driver.create_table_sql(
                "s.t",
                &[
                    Column::new("a", "string"),
                    Column::new("b", "boolean"),
                    Column::new("c", "timestamp"),
                    Column::new("d", "uuid"),
                ]
            ),
            r#"CREATE TABLE s.t ("a" nvarchar(max), "b" bit, "c" datetime2, "d" uniqueidentifier)"#
        );
        assert_eq!(
            driver.to_generic_type("bit", None, None),
            GenericType::Boolean
        );

        let q = driver.information_schema_query();
        assert!(q.contains("FROM INFORMATION_SCHEMA.COLUMNS"));
        assert!(q.contains("WHERE table_schema NOT IN ('information_schema', 'sys')"));
        assert!(!q.contains("columns.data_type"));

        driver.release().await.unwrap();
    }
}
