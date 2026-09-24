//! Oracle driver: port of `@cubejs-backend/oracle-driver` on top of
//! [`oracledb`](https://github.com/oracle/rust-oracledb), Oracle's own
//! pure-Rust implementation of the Oracle Net (TNS/TTC) "thin" protocol.
//!
//! # Runtime requirements
//!
//! None beyond network access to the listener. The Node driver runs
//! node-oracledb 6 in its default thin mode (it never calls
//! `initOracleClient`), so it needs no Oracle Instant Client either; neither
//! does this port. There is no C library and no ODPI-C.
//!
//! `oracledb` is a pinned beta (`=26.0.0-beta.4`) maintained by Oracle.
//!
//! # Blocking client
//!
//! `oracledb` is synchronous. Every round trip runs on Tokio's blocking pool
//! (`spawn_blocking`), and connections live in a small pool of our own that
//! mirrors the Node driver's `generic-pool` settings: `min 0`, `max`
//! `CUBEJS_DB_MAX_POOL` or 50, a 20 s acquire timeout, a ping on borrow
//! (`testOnBorrow`) and a 30 s idle timeout.
//!
//! # TLS
//!
//! Like Node, the connect string built from the environment is the Easy
//! Connect `host:port/service`, i.e. plain TCP. A full connect string or
//! descriptor (`tcps://...`, `(DESCRIPTION=...)`) can be set on
//! [`OracleConfig::connection_string`], exactly as `connectionString` could
//! be passed to the Node driver's constructor; `oracledb` then speaks TLS
//! through `rustls`.

pub mod params;
pub mod types;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use oracledb::{Connection, ErrorKind, ToDbValue};
use serde_json::Value;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::config::DriverConfig;
use crate::driver::Driver;
use crate::error::{DriverError, Result};
use crate::types::{
    Column, ColumnInfo, DatabaseStructure, DownloadQueryResultsOptions, DownloadedData,
    DriverCapabilities, QueryOptions, QueryResult, Row, SchemaColumn, SchemaName, SchemaTable,
    TableStructure,
};

pub use params::{normalize_params, Bind};
pub use types::{db_type_name, oracle_to_generic};

/// Default listener port (`getEnv('dbPort') || 1521`).
pub const DEFAULT_PORT: u16 = 1521;
/// `getDefaultConcurrency`.
pub const DEFAULT_CONCURRENCY: usize = 2;
/// Default pool size (`maxPoolSize || getEnv('dbMaxPoolSize') || 50`).
pub const DEFAULT_MAX_POOL_SIZE: usize = 50;
/// `oracledb.maxRows`: node-oracledb stops fetching after this many rows.
pub const DEFAULT_MAX_ROWS: usize = 100_000;
/// `oracledb.prefetchRows`.
pub const PREFETCH_ROWS: u32 = 500;
/// Longest table name Oracle accepts (`createTable`).
pub const MAX_TABLE_NAME_LENGTH: usize = 128;

/// `OracleDriver.initConnection`: run once per new session so that the
/// ISO-ish date strings Cube binds convert implicitly to DATE/TIMESTAMP.
pub const INIT_SESSION_SQL: &str = "ALTER SESSION SET NLS_DATE_FORMAT = 'YYYY-MM-DD' \
NLS_TIMESTAMP_FORMAT = 'YYYY-MM-DD' NLS_TIMESTAMP_TZ_FORMAT = 'YYYY-MM-DD HH24:MI:SS.FF TZH:TZM'";

/// `OracleDriver.tablesSchema` query (verbatim).
pub const TABLES_SCHEMA_SQL: &str = r#"
      select tc.owner         "table_schema"
          , tc.table_name     "table_name"
          , tc.column_name    "column_name"
          , tc.data_type      "data_type"
          , c.constraint_type "key_type"
      from all_tab_columns tc
      left join all_cons_columns cc
        on (tc.owner, tc.table_name, tc.column_name)
        in ((cc.owner, cc.table_name, cc.column_name))
      left join all_constraints c
        on (tc.owner, tc.table_name, cc.constraint_name)
        in ((c.owner, c.table_name, c.constraint_name))
        and c.constraint_type
        in ('P','U')
      where tc.owner = user
    "#;

/// Configuration of [`OracleDriver`].
#[derive(Debug, Clone)]
pub struct OracleConfig {
    /// Connection settings and global driver knobs.
    pub driver: DriverConfig,
    /// `CUBEJS_DB_USER`.
    pub user: Option<String>,
    /// `CUBEJS_DB_PASS`.
    pub password: Option<String>,
    /// `CUBEJS_DB_NAME`: the service name of the Easy Connect string.
    pub database: Option<String>,
    /// `CUBEJS_DB_HOST`.
    pub host: Option<String>,
    /// `CUBEJS_DB_PORT` (default 1521).
    pub port: u16,
    /// `connectionString`: when set, used instead of `host:port/database`.
    /// Anything `oracledb` accepts: Easy Connect (Plus), `tcps://...`, a
    /// full `(DESCRIPTION=...)` descriptor.
    pub connection_string: Option<String>,
    /// `pool.max` (`CUBEJS_DB_MAX_POOL`, default 50).
    pub max_pool_size: usize,
    /// `pool.acquireTimeoutMillis` (20 s).
    pub acquire_timeout: Duration,
    /// `pool.idleTimeoutMillis` (30 s).
    pub idle_timeout: Duration,
    /// `oracledb.maxRows` (100 000). `0` means no limit.
    pub max_rows: usize,
    /// `readOnly()` (always `true` in Node).
    pub read_only: bool,
}

impl OracleConfig {
    /// Builds the Oracle configuration from a generic [`DriverConfig`].
    pub fn from_driver_config(driver: DriverConfig) -> Self {
        let ds = &driver.data_source;
        Self {
            user: ds.user.clone(),
            password: ds.password.clone(),
            database: ds.database.clone(),
            host: ds.host.clone(),
            port: ds.port.unwrap_or(DEFAULT_PORT),
            connection_string: None,
            max_pool_size: ds
                .max_pool_size
                .filter(|v| *v > 0)
                .unwrap_or(DEFAULT_MAX_POOL_SIZE),
            acquire_timeout: Duration::from_millis(20_000),
            idle_timeout: Duration::from_millis(30_000),
            max_rows: DEFAULT_MAX_ROWS,
            read_only: true,
            driver,
        }
    }

    /// Reads the configuration of `data_source` from the environment.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Ok(Self::from_driver_config(DriverConfig::from_env(
            data_source,
        )?))
    }

    /// Reads the configuration from an `oracle://user:pass@host:port/service`
    /// URL, for tests and tooling.
    pub fn from_url(url: &str) -> Result<Self> {
        let parsed = url::Url::parse(url)
            .map_err(|e| DriverError::Config(format!("Invalid Oracle URL: {e}")))?;
        let mut driver = DriverConfig::default();
        let ds = &mut driver.data_source;
        ds.host = parsed.host_str().map(|h| h.to_string());
        ds.port = Some(parsed.port().unwrap_or(DEFAULT_PORT));
        if !parsed.username().is_empty() {
            ds.user = Some(percent_decode(parsed.username()));
        }
        ds.password = parsed.password().map(percent_decode);
        let path = parsed.path().trim_start_matches('/');
        if !path.is_empty() {
            ds.database = Some(path.to_string());
        }
        Ok(Self::from_driver_config(driver))
    }

    /// `this.config.connectionString || `${host}:${port}/${db}``.
    pub fn effective_connection_string(&self) -> Result<String> {
        if let Some(cs) = self.connection_string.as_ref().filter(|s| !s.is_empty()) {
            return Ok(cs.clone());
        }
        // Node would build `undefined:1521/undefined` and fail on connect;
        // the missing variable is named up front instead.
        let host = self.host.as_ref().filter(|s| !s.is_empty()).ok_or_else(|| {
            DriverError::Config(
                "The Oracle driver needs CUBEJS_DB_HOST (or a connection string)".to_string(),
            )
        })?;
        let database = self
            .database
            .as_ref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                DriverError::Config(
                    "The Oracle driver needs CUBEJS_DB_NAME, the service name to connect to"
                        .to_string(),
                )
            })?;
        Ok(format!("{host}:{}/{database}", self.port))
    }

    /// Translates the Cube configuration into an `oracledb::Config`. Parses
    /// the connect string, so a malformed one fails here, at start-up.
    pub fn oracledb_config(&self) -> Result<oracledb::Config> {
        let connect_string = self.effective_connection_string()?;
        oracledb::Config::default()
            .set_credentials(
                self.user.as_deref().unwrap_or_default(),
                self.password.as_deref().unwrap_or_default(),
            )
            .set_driver_name("cube : rust-oracledb thin")
            .set_connect_string(&connect_string)
            .map_err(|e| {
                DriverError::Config(format!(
                    "Invalid Oracle connect string \"{connect_string}\": {e}"
                ))
            })
    }
}

/// Decodes the `%XX` escapes of a URL user info component.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 3 <= bytes.len() {
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

/// Maps an `oracledb` error. Server errors keep Oracle's own text
/// (`ORA-00942: table or view does not exist`), as node-oracledb does.
pub fn map_error(e: oracledb::Error) -> DriverError {
    match e.kind() {
        ErrorKind::DbError(db) => DriverError::Database {
            message: db.message().to_string(),
            code: Some(format!("ORA-{:05}", db.code())),
        },
        _ => DriverError::Query(e.to_string()),
    }
}

/// `rustls` panics in `ClientConfig::builder()` when both of its crypto
/// providers are compiled in and none was installed. `oracledb` calls it for
/// TCPS, and this workspace does compile both, so install `ring` (the one the
/// other drivers use) unless something already installed a default.
fn ensure_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Opens a session and runs [`INIT_SESSION_SQL`] (blocking).
fn connect_blocking(config: oracledb::Config) -> Result<Connection> {
    ensure_crypto_provider();
    let conn = oracledb::connect(config).map_err(|e| DriverError::Connection {
        pool_name: "oracle".to_string(),
        message: e.to_string(),
    })?;
    conn.execute(INIT_SESSION_SQL, &[]).map_err(map_error)?;
    Ok(conn)
}

/// Closes connections off the async threads (logoff is a round trip).
fn close_in_background(connections: Vec<Connection>) {
    if connections.is_empty() {
        return;
    }
    let close = move || {
        for mut conn in connections {
            let _ = conn.close();
        }
    };
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn_blocking(close);
        }
        Err(_) => close(),
    }
}

/// The connection pool (`Pool` from `@cubejs-backend/shared` over
/// `generic-pool`).
struct Pool {
    config: oracledb::Config,
    permits: Arc<Semaphore>,
    idle: Mutex<Vec<(Connection, Instant)>>,
    acquire_timeout: Duration,
    idle_timeout: Duration,
}

impl Pool {
    fn new(config: oracledb::Config, oracle: &OracleConfig) -> Self {
        Self {
            config,
            permits: Arc::new(Semaphore::new(oracle.max_pool_size.max(1))),
            idle: Mutex::new(Vec::new()),
            acquire_timeout: oracle.acquire_timeout,
            idle_timeout: oracle.idle_timeout,
        }
    }

    /// Takes a live idle connection, discarding the expired ones.
    fn take_idle(&self) -> Option<Connection> {
        let mut idle = self.idle.lock().unwrap();
        let mut expired = Vec::new();
        let mut found = None;
        while let Some((conn, since)) = idle.pop() {
            if since.elapsed() > self.idle_timeout {
                expired.push(conn);
            } else {
                found = Some(conn);
                break;
            }
        }
        drop(idle);
        close_in_background(expired);
        found
    }

    async fn acquire(self: &Arc<Self>) -> Result<Lease> {
        let pool_timeout = || DriverError::PoolTimeout("oracle".to_string());
        let started = Instant::now();
        let permit = tokio::time::timeout(self.acquire_timeout, self.permits.clone().acquire_owned())
            .await
            .map_err(|_| pool_timeout())?
            .map_err(|_| DriverError::Query("Oracle pool is closed".to_string()))?;

        let candidate = self.take_idle();
        let config = self.config.clone();
        // `testOnBorrow`: a pooled session is pinged before use and
        // replaced when the ping fails.
        let task = tokio::task::spawn_blocking(move || {
            if let Some(conn) = candidate {
                match conn.ping() {
                    Ok(()) => return Ok(conn),
                    Err(e) => {
                        log::warn!("Oracle pool: dropping a connection that failed its ping: {e}");
                        drop(conn);
                    }
                }
            }
            connect_blocking(config)
        });
        let remaining = self.acquire_timeout.saturating_sub(started.elapsed());
        let conn = tokio::time::timeout(remaining.max(Duration::from_millis(1)), task)
            .await
            .map_err(|_| pool_timeout())?
            .map_err(join_error)??;
        Ok(Lease {
            conn: Some(conn),
            pool: self.clone(),
            _permit: permit,
        })
    }

    fn close(&self) {
        let connections: Vec<Connection> =
            self.idle.lock().unwrap().drain(..).map(|(c, _)| c).collect();
        close_in_background(connections);
    }

    fn size(&self) -> usize {
        self.idle.lock().unwrap().len()
    }
}

fn join_error(e: tokio::task::JoinError) -> DriverError {
    if e.is_panic() {
        let message = e
            .into_panic()
            .downcast::<String>()
            .map(|s| *s)
            .or_else(|p| p.downcast::<&'static str>().map(|s| s.to_string()))
            .unwrap_or_else(|_| "unknown panic".to_string());
        if message == "not yet implemented" {
            // `oracledb` beta: e.g. a TIMESTAMP WITH TIME ZONE value stored
            // with a named region ('Europe/Paris') rather than an offset.
            return DriverError::NotImplemented(
                "The Oracle client cannot decode a value of this result yet (a TIMESTAMP WITH                  TIME ZONE using a named time zone region is the known case); convert it in                  the query, e.g. SYS_EXTRACT_UTC(col) or TO_CHAR(col)"
                    .to_string(),
            );
        }
        DriverError::Query(format!("The Oracle client failed unexpectedly: {message}"))
    } else {
        DriverError::Query("The Oracle client task was cancelled".to_string())
    }
}

/// A pooled connection; returned to the pool when dropped.
struct Lease {
    conn: Option<Connection>,
    pool: Arc<Pool>,
    _permit: OwnedSemaphorePermit,
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.pool.idle.lock().unwrap().push((conn, Instant::now()));
        }
    }
}

/// Runs `sql` on `conn` (blocking): queries return their rows (at most
/// `max_rows`, like `oracledb.maxRows`), anything else is executed and
/// committed.
fn execute_blocking(
    conn: &Connection,
    sql: &str,
    binds: &[(String, Bind)],
    max_rows: usize,
) -> Result<QueryResult> {
    let mut statement = conn
        .statement(sql)
        .map_err(map_error)?
        .prefetch_rows(PREFETCH_ROWS)
        .build()
        .map_err(map_error)?;
    let named: Vec<(&str, &dyn ToDbValue)> = binds
        .iter()
        .map(|(name, bind)| (name.as_str(), bind.as_db_value()))
        .collect();

    if !statement.is_query() {
        statement.execute_named(&named).map_err(map_error)?;
        // node-oracledb leaves `autoCommit` off; Oracle commits DDL
        // implicitly, and DML is committed here so that it is not left
        // pending on a pooled session.
        conn.commit().map_err(map_error)?;
        return Ok(QueryResult::default());
    }

    let cursor = statement.query_named(&named).map_err(map_error)?;
    let metadata = cursor.columns().to_vec();
    let columns: Vec<Column> = metadata
        .iter()
        .map(|m| Column::new(m.name(), oracle_to_generic(db_type_name(m.db_type()))))
        .collect();

    let mut rows: Vec<Row> = Vec::new();
    for row in cursor {
        if max_rows > 0 && rows.len() >= max_rows {
            log::warn!(
                "Oracle query result truncated to {max_rows} rows (oracledb.maxRows in the Node \
                 driver)"
            );
            break;
        }
        let row = row.map_err(map_error)?;
        let values = metadata
            .iter()
            .enumerate()
            .map(|(i, m)| types::cell_to_value(&row, i, m))
            .collect::<Result<Row>>()?;
        rows.push(values);
    }
    Ok(QueryResult::new(columns, rows))
}

/// Oracle driver.
pub struct OracleDriver {
    config: OracleConfig,
    pool: Arc<Pool>,
}

impl std::fmt::Debug for OracleDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OracleDriver")
            .field("host", &self.config.host)
            .field("database", &self.config.database)
            .finish()
    }
}

impl OracleDriver {
    /// Creates the driver and its (lazy) connection pool. Fails on a
    /// missing host/service or an invalid connect string.
    pub fn new(config: OracleConfig) -> Result<Self> {
        let oracledb_config = config.oracledb_config()?;
        let pool = Arc::new(Pool::new(oracledb_config, &config));
        Ok(Self { config, pool })
    }

    /// Creates the driver from `CUBEJS_DB_*`.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Self::new(OracleConfig::from_env(data_source)?)
    }

    /// The driver configuration.
    pub fn oracle_config(&self) -> &OracleConfig {
        &self.config
    }

    /// Number of idle pooled connections.
    pub fn pool_size(&self) -> usize {
        self.pool.size()
    }

    /// `withConnection`: runs `f` on a pooled connection on the blocking
    /// pool and gives the connection back afterwards.
    async fn with_connection<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
    {
        let mut lease = self.pool.acquire().await?;
        let conn = lease.conn.take().expect("a fresh lease holds a connection");
        let (conn, result) = tokio::task::spawn_blocking(move || {
            let result = f(&conn);
            (conn, result)
        })
        .await
        .map_err(join_error)?;
        lease.conn = Some(conn);
        result
    }

    async fn run(&self, sql: &str, params: &[Value]) -> Result<QueryResult> {
        let (sql, named) = normalize_params(sql, params);
        let binds = named
            .into_iter()
            .map(|(name, value)| Ok((name, Bind::from_json(&value)?)))
            .collect::<Result<Vec<_>>>()?;
        let max_rows = self.config.max_rows;
        self.with_connection(move |conn| execute_blocking(conn, &sql, &binds, max_rows))
            .await
    }

    fn not_supported(&self, what: &str) -> DriverError {
        DriverError::NotImplemented(format!(
            "{what} is not supported by the Oracle driver: it relies on information_schema, \
             which Oracle does not have (the Node driver does not implement it either)"
        ))
    }
}

/// Port of the `reduceCb` of `OracleDriver.tablesSchema`: schema → table →
/// columns, keeping the query's column order. The join can return a column
/// once per constraint it belongs to; such rows are merged (Node would list
/// the column twice), a `P`/`U` key on any of them marking it `primaryKey`.
pub fn tables_schema_from_rows(data: &QueryResult) -> DatabaseStructure {
    let mut result: DatabaseStructure = BTreeMap::new();
    for i in 0..data.len() {
        let (Some(schema), Some(table), Some(column)) = (
            data.get_string(i, "table_schema"),
            data.get_string(i, "table_name"),
            data.get_string(i, "column_name"),
        ) else {
            continue;
        };
        let data_type = data.get_string(i, "data_type").unwrap_or_default();
        let is_key = matches!(
            data.get_string(i, "key_type").as_deref(),
            Some("P") | Some("U")
        );
        let columns = result.entry(schema).or_default().entry(table).or_default();
        match columns.iter_mut().find(|c| c.name == column) {
            Some(existing) => {
                if is_key && existing.attributes.is_empty() {
                    existing.attributes = vec!["primaryKey".to_string()];
                }
            }
            None => columns.push(SchemaColumn {
                name: column,
                type_: data_type,
                attributes: if is_key {
                    vec!["primaryKey".to_string()]
                } else {
                    Vec::new()
                },
                foreign_keys: Vec::new(),
            }),
        }
    }
    result
}

#[async_trait]
impl Driver for OracleDriver {
    fn config(&self) -> &DriverConfig {
        &self.config.driver
    }

    async fn test_connection(&self) -> Result<()> {
        tokio::time::timeout(
            self.config.driver.test_connection_timeout,
            self.run("SELECT 1 FROM DUAL", &[]),
        )
        .await
        .map_err(|_| DriverError::Connection {
            pool_name: "oracle".to_string(),
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
        self.run(sql, params).await
    }

    fn read_only(&self) -> bool {
        self.config.read_only
    }

    fn capabilities(&self) -> DriverCapabilities {
        DriverCapabilities::default()
    }

    /// Oracle has no `LIMIT` and forbids `AS` for table aliases.
    fn wrap_query_with_limit(&self, query: &str, limit: u64) -> String {
        format!("SELECT * FROM ({query}) t WHERE ROWNUM <= {limit}")
    }

    async fn release(&self) -> Result<()> {
        self.pool.close();
        Ok(())
    }

    fn information_schema_query(&self) -> String {
        TABLES_SCHEMA_SQL.to_string()
    }

    async fn tables_schema(&self) -> Result<DatabaseStructure> {
        let data = self
            .query(TABLES_SCHEMA_SQL, &[], &QueryOptions::default())
            .await?;
        Ok(tables_schema_from_rows(&data))
    }

    async fn create_table(&self, quoted_table_name: &str, columns: &[Column]) -> Result<()> {
        if quoted_table_name.chars().count() > MAX_TABLE_NAME_LENGTH {
            return Err(DriverError::Query(format!(
                "Oracle can not work with table names longer than 128 symbols. Consider using \
                 the 'sqlAlias' attribute in your cube definition for {quoted_table_name}."
            )));
        }
        let create_sql = self.create_table_sql(quoted_table_name, columns);
        self.query(&create_sql, &[], &QueryOptions::default())
            .await
            .map_err(|e| {
                DriverError::Query(format!("Error during create table: {create_sql}: {e}"))
            })?;
        Ok(())
    }

    /// Rows plus the column types from the result-set metadata
    /// (`metaDataToColumnTypes`), in memory.
    async fn download_query_results(
        &self,
        sql: &str,
        params: &[Value],
        _options: &DownloadQueryResultsOptions,
    ) -> Result<DownloadedData> {
        Ok(DownloadedData::Memory(self.run(sql, params).await?))
    }

    async fn create_schema_if_not_exists(&self, _schema_name: &str) -> Result<()> {
        Err(self.not_supported("createSchemaIfNotExists"))
    }

    async fn get_schemas(&self) -> Result<Vec<SchemaName>> {
        Err(self.not_supported("getSchemas (incremental schema loading)"))
    }

    async fn get_tables_for_specific_schemas(
        &self,
        _schemas: &[SchemaName],
    ) -> Result<Vec<SchemaTable>> {
        Err(self.not_supported("getTablesForSpecificSchemas (incremental schema loading)"))
    }

    async fn get_columns_for_specific_tables(
        &self,
        _tables: &[SchemaTable],
    ) -> Result<Vec<ColumnInfo>> {
        Err(self.not_supported("getColumnsForSpecificTables (incremental schema loading)"))
    }

    async fn get_tables_query(&self, _schema_name: &str) -> Result<Vec<String>> {
        Err(self.not_supported("getTablesQuery"))
    }

    async fn table_column_types(&self, _table: &str) -> Result<TableStructure> {
        Err(self.not_supported("tableColumnTypes"))
    }

    async fn table_column_types_with_precision(&self, _table: &str) -> Result<TableStructure> {
        Err(self.not_supported("tableColumnTypes"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::GenericType;

    fn config() -> OracleConfig {
        let mut driver = DriverConfig::default();
        driver.data_source.host = Some("db.local".into());
        driver.data_source.database = Some("FREEPDB1".into());
        driver.data_source.user = Some("cube".into());
        driver.data_source.password = Some("pw".into());
        OracleConfig::from_driver_config(driver)
    }

    #[test]
    fn defaults_follow_the_node_driver() {
        let config = config();
        assert_eq!(config.port, 1521);
        assert_eq!(config.max_pool_size, 50);
        assert_eq!(config.acquire_timeout, Duration::from_secs(20));
        assert_eq!(config.idle_timeout, Duration::from_secs(30));
        assert_eq!(config.max_rows, 100_000);
        assert_eq!(DEFAULT_CONCURRENCY, 2);
        assert!(config.read_only);
    }

    #[test]
    fn connect_string_is_host_port_service() {
        let mut config = config();
        assert_eq!(
            config.effective_connection_string().unwrap(),
            "db.local:1521/FREEPDB1"
        );
        config.port = 1600;
        config.driver.data_source.max_pool_size = Some(3);
        assert_eq!(
            config.effective_connection_string().unwrap(),
            "db.local:1600/FREEPDB1"
        );
        config.connection_string = Some("tcps://adb.example.com:1522/svc_high".into());
        assert_eq!(
            config.effective_connection_string().unwrap(),
            "tcps://adb.example.com:1522/svc_high"
        );
        assert!(config.oracledb_config().is_ok());

        let mut driver = DriverConfig::default();
        driver.data_source.max_pool_size = Some(3);
        driver.data_source.port = Some(1600);
        let config = OracleConfig::from_driver_config(driver);
        assert_eq!(config.max_pool_size, 3);
        assert_eq!(config.port, 1600);
    }

    #[test]
    fn missing_host_or_service_is_a_named_error() {
        let mut config = config();
        config.host = None;
        let err = OracleDriver::new(config).unwrap_err();
        assert!(err.to_string().contains("CUBEJS_DB_HOST"), "{err}");

        let mut config = self::config();
        config.database = None;
        let err = OracleDriver::new(config).unwrap_err();
        assert!(err.to_string().contains("CUBEJS_DB_NAME"), "{err}");
    }

    #[test]
    fn invalid_connect_string_fails_at_start_up() {
        let mut config = config();
        config.connection_string = Some("(DESCRIPTION=(ADDRESS=".into());
        let err = OracleDriver::new(config).unwrap_err();
        assert!(matches!(err, DriverError::Config(_)), "{err}");
    }

    #[test]
    fn config_from_url() {
        let config =
            OracleConfig::from_url("oracle://cube:p%40ss@127.0.0.1:16901/FREEPDB1").unwrap();
        assert_eq!(config.host.as_deref(), Some("127.0.0.1"));
        assert_eq!(config.port, 16901);
        assert_eq!(config.user.as_deref(), Some("cube"));
        assert_eq!(config.password.as_deref(), Some("p@ss"));
        assert_eq!(config.database.as_deref(), Some("FREEPDB1"));
    }

    #[tokio::test]
    async fn sql_generation() {
        let driver = OracleDriver::new(config()).unwrap();
        assert!(driver.read_only());
        assert_eq!(driver.capabilities(), DriverCapabilities::default());
        assert_eq!(driver.param(0), "?");
        assert_eq!(driver.quote_identifier("a"), "\"a\"");
        assert_eq!(
            driver.wrap_query_with_limit("SELECT 1 FROM dual", 10),
            "SELECT * FROM (SELECT 1 FROM dual) t WHERE ROWNUM <= 10"
        );
        assert!(driver.information_schema_query().contains("from all_tab_columns tc"));

        let long_name = format!("s.{}", "x".repeat(130));
        let err = driver
            .create_table(&long_name, &[Column::new("a", "int")])
            .await
            .unwrap_err();
        assert!(err
            .to_string()
            .starts_with("Oracle can not work with table names longer than 128 symbols."));

        let err = driver.get_schemas().await.unwrap_err();
        assert!(matches!(err, DriverError::NotImplemented(_)));
        let err = driver.table_column_types("s.t").await.unwrap_err();
        assert!(matches!(err, DriverError::NotImplemented(_)));
        driver.release().await.unwrap();
        driver.release().await.unwrap();
    }

    #[test]
    fn tables_schema_reducer() {
        let text = |s: &str| Value::String(s.to_string());
        let data = QueryResult::new(
            ["table_schema", "table_name", "column_name", "data_type", "key_type"]
                .iter()
                .map(|c| Column::new(*c, GenericType::Text))
                .collect(),
            vec![
                vec![text("CUBE"), text("ORDERS"), text("ID"), text("NUMBER"), text("P")],
                vec![text("CUBE"), text("ORDERS"), text("ID"), text("NUMBER"), Value::Null],
                vec![text("CUBE"), text("ORDERS"), text("AMOUNT"), text("NUMBER"), Value::Null],
                vec![text("CUBE"), text("ACCOUNTS"), text("NAME"), text("VARCHAR2"), text("U")],
            ],
        );
        let structure = tables_schema_from_rows(&data);
        let tables: Vec<_> = structure["CUBE"].keys().cloned().collect();
        assert_eq!(tables, vec!["ACCOUNTS", "ORDERS"]);
        let orders = &structure["CUBE"]["ORDERS"];
        assert_eq!(orders.len(), 2);
        assert_eq!(orders[0].name, "ID");
        assert_eq!(orders[0].attributes, vec!["primaryKey".to_string()]);
        assert_eq!(orders[1].name, "AMOUNT");
        assert!(orders[1].attributes.is_empty());
        assert_eq!(
            structure["CUBE"]["ACCOUNTS"][0].attributes,
            vec!["primaryKey".to_string()]
        );
    }

    #[test]
    fn server_errors_keep_the_ora_code() {
        // Only the mapping of non-server errors can be built offline.
        let err = map_error(
            "not a number"
                .parse::<oracledb::OracleNumber>()
                .unwrap_err(),
        );
        assert!(matches!(err, DriverError::Query(_)));
    }
}
