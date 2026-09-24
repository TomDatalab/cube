//! DuckDB driver: port of `@cubejs-backend/duckdb-driver` on `duckdb-rs`.
//!
//! # Native engine
//!
//! DuckDB is an embedded C++ engine with no network protocol, so a pure-Rust
//! client is impossible. The `duckdb` Cargo feature enables the `duckdb`
//! crate with its `bundled` feature, which compiles DuckDB's amalgamation into
//! the binary with the C++ compiler. The Node driver does the same thing: the
//! `duckdb` npm package links the same C++ engine as a native addon. No
//! JavaScript is involved and no system `libduckdb` is needed.
//!
//! # Configuration (same names and defaults as Node)
//!
//! | Variable | Effect |
//! |---|---|
//! | `CUBEJS_DB_DUCKDB_DATABASE_PATH` | database file; wins over MotherDuck |
//! | `CUBEJS_DB_DUCKDB_MOTHERDUCK_TOKEN` | opens `md:?motherduck_token=…&custom_user_agent=Cube/<v>` |
//! | neither | `:memory:` |
//! | `CUBEJS_DB_DUCKDB_SCHEMA` | `SET schema`, and restricts introspection to `table_catalog = <schema>` |
//! | `CUBEJS_DB_DUCKDB_MEMORY_LIMIT` | `SET memory_limit` |
//! | `CUBEJS_DB_DUCKDB_S3_REGION`, `_S3_ENDPOINT`, `_S3_ACCESS_KEY_ID`, `_S3_SECRET_ACCESS_KEY`, `_S3_USE_SSL`, `_S3_URL_STYLE`, `_S3_SESSION_TOKEN` | `SET s3_*` |
//! | `CUBEJS_DB_DUCKDB_S3_USE_CREDENTIAL_CHAIN` | `CREATE SECRET (TYPE S3, PROVIDER 'CREDENTIAL_CHAIN')` |
//! | `CUBEJS_DB_DUCKDB_EXTENSIONS` | comma list; `INSTALL x` + `LOAD x` |
//! | `CUBEJS_DB_DUCKDB_COMMUNITY_EXTENSIONS` | comma list; `INSTALL x FROM community` + `LOAD x` |
//!
//! All of them honour `CUBEJS_DS_<NAME>_…` and the pre-aggregations prefix
//! (`keyByDataSource`). `initSql` is a programmatic option only, as in Node
//! ([`DuckDbConfig::init_sql`]).
//!
//! Initialisation is lazy (first query) and retried when it fails, like
//! `getInitiatedState`. A failed `SET` or `initSql` is logged and skipped; a
//! failed credential-chain secret, `INSTALL` or `LOAD` fails initialisation
//! with a [`DriverError::Connection`] naming the extension. `INSTALL` and the
//! MotherDuck extension download from DuckDB's extension repository at run
//! time, exactly as the Node driver does, so they need network access then.
//!
//! # Queries and values
//!
//! * `query` runs on one long-lived connection (Node's `defaultConnection`),
//!   serialised by a mutex on Tokio's blocking pool.
//! * `stream` opens a fresh connection to the same database per stream (as
//!   Node does, because a stream on the shared connection can break) and
//!   feeds rows through a bounded channel sized by `highWaterMark`.
//! * Rows follow `transformRow`: every top-level number is a string, `DATE`
//!   and `TIMESTAMP*` are `toISOString()` strings (millisecond precision, `Z`).
//! * Parameters are positional `?`, identifiers are quoted with `"`.
//! * Type mapping: `date` → `timestamp` (Cube Store has no `DATE`), otherwise
//!   `BaseDriver.toGenericType` on the lower-cased type.
//! * `readOnly()` is `false`; no capabilities, no unload (as in Node).
//!
//! # Deliberate differences
//!
//! * `DECIMAL` values: `node-duckdb` converts them to a JS `number` before
//!   `transformRow` stringifies it. The port prints the exact decimal without
//!   trailing zeros instead, which is the same text for every value a double
//!   represents exactly and does not lose digits beyond 15 significant ones.
//! * The `SET key='value'` statements escape `'` in the value.
//! * `stream` reports the column types DuckDB knows (Node returns
//!   `types: undefined`); they go through the same generic type mapping.
//! * Empty entries of the extension lists (`"a,,b"`) are skipped instead of
//!   running `INSTALL` with an empty name.

pub mod convert;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use crate::config::{data_sources, env_key, DriverConfig, EnvSource, ProcessEnv};
use crate::driver::Driver;
use crate::error::{DriverError, Result};
use crate::types::{
    Column, GenericType, QueryOptions, QueryResult, Row, StreamOptions, StreamTableData,
};

use convert::{cell_to_json, logical_type_name, to_duck_value};

/// Pool name used in connection errors.
const POOL_NAME: &str = "duckdb";

/// Version sent in MotherDuck's `custom_user_agent` (`Cube/<version>`).
pub const CUBE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Configuration of [`DuckDbDriver`] (`DuckDBDriverConfiguration` + the env
/// variables the Node driver reads through `getEnv`).
#[derive(Debug, Clone, Default)]
pub struct DuckDbConfig {
    /// Global driver knobs (data source name, pre-aggregations flag).
    pub driver: DriverConfig,
    /// `databasePath` / `CUBEJS_DB_DUCKDB_DATABASE_PATH`.
    pub database_path: Option<String>,
    /// `motherDuckToken` / `CUBEJS_DB_DUCKDB_MOTHERDUCK_TOKEN`.
    pub mother_duck_token: Option<String>,
    /// `schema` / `CUBEJS_DB_DUCKDB_SCHEMA`.
    pub schema: Option<String>,
    /// `initSql` (programmatic only).
    pub init_sql: Option<String>,
    /// `CUBEJS_DB_DUCKDB_S3_REGION`.
    pub s3_region: Option<String>,
    /// `CUBEJS_DB_DUCKDB_S3_ENDPOINT`.
    pub s3_endpoint: Option<String>,
    /// `CUBEJS_DB_DUCKDB_S3_ACCESS_KEY_ID`.
    pub s3_access_key_id: Option<String>,
    /// `CUBEJS_DB_DUCKDB_S3_SECRET_ACCESS_KEY`.
    pub s3_secret_access_key: Option<String>,
    /// `CUBEJS_DB_DUCKDB_MEMORY_LIMIT`.
    pub memory_limit: Option<String>,
    /// `CUBEJS_DB_DUCKDB_S3_USE_SSL`.
    pub s3_use_ssl: Option<String>,
    /// `CUBEJS_DB_DUCKDB_S3_URL_STYLE`.
    pub s3_url_style: Option<String>,
    /// `CUBEJS_DB_DUCKDB_S3_SESSION_TOKEN`.
    pub s3_session_token: Option<String>,
    /// `duckdbS3UseCredentialChain` / `CUBEJS_DB_DUCKDB_S3_USE_CREDENTIAL_CHAIN`.
    pub s3_use_credential_chain: Option<bool>,
    /// `CUBEJS_DB_DUCKDB_EXTENSIONS`.
    pub extensions: Option<Vec<String>>,
    /// `CUBEJS_DB_DUCKDB_COMMUNITY_EXTENSIONS`.
    pub community_extensions: Option<Vec<String>>,
}

impl DuckDbConfig {
    /// Wraps a generic [`DriverConfig`]; call [`DuckDbConfig::apply_env`] to
    /// read the `CUBEJS_DB_DUCKDB_*` variables.
    pub fn from_driver_config(driver: DriverConfig) -> Self {
        Self {
            driver,
            ..Default::default()
        }
    }

    /// Reads the configuration of `data_source` from the environment.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        let mut config = Self::from_driver_config(DriverConfig::from_env(data_source)?);
        config.apply_env()?;
        Ok(config)
    }

    /// Fills every unset field from the process environment.
    pub fn apply_env(&mut self) -> Result<()> {
        self.apply_env_source(&ProcessEnv)
    }

    /// Fills every unset field from `env` (explicit values win, like
    /// `this.config.x || getEnv(...)`).
    pub fn apply_env_source(&mut self, env: &dyn EnvSource) -> Result<()> {
        let declared = data_sources(env);
        let ds = self.driver.data_source.data_source.clone();
        let pre_aggs = self.driver.data_source.pre_aggregations;
        let read = |origin: &str| -> Result<Option<String>> {
            let key = env_key(origin, &declared, Some(&ds), pre_aggs)?;
            Ok(env.get(&key).filter(|v| !v.is_empty()))
        };
        let list = |origin: &str| -> Result<Option<Vec<String>>> {
            Ok(read(origin)?.map(|v| {
                v.split(',')
                    .map(|e| e.trim().to_string())
                    .filter(|e| !e.is_empty())
                    .collect()
            }))
        };

        macro_rules! fill {
            ($field:ident, $key:literal) => {
                if self.$field.is_none() {
                    self.$field = read($key)?;
                }
            };
        }
        fill!(database_path, "CUBEJS_DB_DUCKDB_DATABASE_PATH");
        fill!(mother_duck_token, "CUBEJS_DB_DUCKDB_MOTHERDUCK_TOKEN");
        fill!(schema, "CUBEJS_DB_DUCKDB_SCHEMA");
        fill!(s3_region, "CUBEJS_DB_DUCKDB_S3_REGION");
        fill!(s3_endpoint, "CUBEJS_DB_DUCKDB_S3_ENDPOINT");
        fill!(s3_access_key_id, "CUBEJS_DB_DUCKDB_S3_ACCESS_KEY_ID");
        fill!(
            s3_secret_access_key,
            "CUBEJS_DB_DUCKDB_S3_SECRET_ACCESS_KEY"
        );
        fill!(memory_limit, "CUBEJS_DB_DUCKDB_MEMORY_LIMIT");
        fill!(s3_use_ssl, "CUBEJS_DB_DUCKDB_S3_USE_SSL");
        fill!(s3_url_style, "CUBEJS_DB_DUCKDB_S3_URL_STYLE");
        fill!(s3_session_token, "CUBEJS_DB_DUCKDB_S3_SESSION_TOKEN");

        if self.extensions.is_none() {
            self.extensions = list("CUBEJS_DB_DUCKDB_EXTENSIONS")?;
        }
        if self.community_extensions.is_none() {
            self.community_extensions = list("CUBEJS_DB_DUCKDB_COMMUNITY_EXTENSIONS")?;
        }
        if self.s3_use_credential_chain.is_none() {
            if let Some(v) = read("CUBEJS_DB_DUCKDB_S3_USE_CREDENTIAL_CHAIN")? {
                self.s3_use_credential_chain = Some(match v.to_lowercase().as_str() {
                    "true" => true,
                    "false" => false,
                    _ => {
                        // The Node message names the key without the
                        // pre-aggregations prefix.
                        let key = env_key(
                            "CUBEJS_DB_DUCKDB_S3_USE_CREDENTIAL_CHAIN",
                            &declared,
                            Some(&ds),
                            false,
                        )?;
                        return Err(DriverError::Config(format!(
                            "The {key} must be either 'true' or 'false'."
                        )));
                    }
                });
            }
        }
        Ok(())
    }

    /// The database URL handed to DuckDB: the path, else MotherDuck, else
    /// `:memory:`.
    pub fn database_url(&self) -> String {
        if let Some(path) = self.database_path.as_deref().filter(|p| !p.is_empty()) {
            path.to_string()
        } else if let Some(token) = self.mother_duck_token.as_deref().filter(|t| !t.is_empty()) {
            format!("md:?motherduck_token={token}&custom_user_agent=Cube/{CUBE_VERSION}")
        } else {
            ":memory:".to_string()
        }
    }

    /// The `SET key='value'` statements of `init`, in Node's order.
    pub fn settings(&self) -> Vec<(&'static str, String)> {
        [
            ("s3_region", &self.s3_region),
            ("s3_endpoint", &self.s3_endpoint),
            ("s3_access_key_id", &self.s3_access_key_id),
            ("s3_secret_access_key", &self.s3_secret_access_key),
            ("memory_limit", &self.memory_limit),
            ("schema", &self.schema),
            ("s3_use_ssl", &self.s3_use_ssl),
            ("s3_url_style", &self.s3_url_style),
            ("s3_session_token", &self.s3_session_token),
        ]
        .into_iter()
        .filter_map(|(k, v)| v.clone().filter(|v| !v.is_empty()).map(|v| (k, v)))
        .collect()
    }
}

/// The initialised database: Node's `defaultConnection`.
struct DuckDbState {
    connection: Mutex<::duckdb::Connection>,
}

/// DuckDB driver.
pub struct DuckDbDriver {
    config: DuckDbConfig,
    state: tokio::sync::Mutex<Option<Arc<DuckDbState>>>,
}

impl std::fmt::Debug for DuckDbDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DuckDbDriver")
            .field("database_path", &self.config.database_path)
            .field("motherduck", &self.config.mother_duck_token.is_some())
            .field("schema", &self.config.schema)
            .finish()
    }
}

fn duck_error(e: ::duckdb::Error) -> DriverError {
    DriverError::Database {
        message: e.to_string(),
        code: None,
    }
}

fn init_error(message: String) -> DriverError {
    DriverError::Connection {
        pool_name: POOL_NAME.to_string(),
        message,
    }
}

fn worker_error(e: tokio::task::JoinError) -> DriverError {
    DriverError::Other(format!("DuckDB worker failed: {e}"))
}

/// Opens the database and runs Node's `init` sequence (blocking).
fn init_database(config: &DuckDbConfig) -> Result<::duckdb::Connection> {
    let url = config.database_url();
    let token = config
        .mother_duck_token
        .as_deref()
        .filter(|t| !t.is_empty());
    let mut flags = ::duckdb::Config::default();
    if token.is_some() {
        flags = flags
            .custom_user_agent(&format!("Cube/{CUBE_VERSION}"))
            .map_err(|e| init_error(e.to_string()))?;
    }
    let conn = ::duckdb::Connection::open_with_flags(&url, flags).map_err(|e| {
        // Never log the MotherDuck token.
        let shown = if token.is_some() { "md:" } else { url.as_str() };
        init_error(format!("Unable to open DuckDB database {shown}: {e}"))
    })?;

    for (key, value) in config.settings() {
        let sql = format!("SET {key}='{}'", value.replace('\'', "''"));
        if let Err(e) = conn.execute_batch(&sql) {
            log::error!("DuckDB - error on configuration, key: {key}: {e}");
        }
    }

    if config.s3_use_credential_chain == Some(true) {
        conn.execute_batch("CREATE SECRET (TYPE S3, PROVIDER 'CREDENTIAL_CHAIN')")
            .map_err(|e| {
                init_error(format!(
                    "DuckDB - error on creating S3 credential chain secret: {e}"
                ))
            })?;
    }

    let official = config.extensions.clone().unwrap_or_default();
    install_extensions(&conn, &official, "")?;
    load_extensions(&conn, &official)?;
    // @see https://duckdb.org/community_extensions/
    let community = config.community_extensions.clone().unwrap_or_default();
    install_extensions(&conn, &community, "community")?;
    load_extensions(&conn, &community)?;

    if let Some(init_sql) = config.init_sql.as_deref().filter(|s| !s.is_empty()) {
        if let Err(e) = conn.execute_batch(init_sql) {
            log::error!("DuckDB - error on init sql (skipping): {e}");
        }
    }

    Ok(conn)
}

fn install_extensions(
    conn: &::duckdb::Connection,
    extensions: &[String],
    repository: &str,
) -> Result<()> {
    let from = if repository.is_empty() {
        String::new()
    } else {
        format!(" FROM {repository}")
    };
    for extension in extensions {
        conn.execute_batch(&format!("INSTALL {extension}{from}"))
            .map_err(|e| init_error(format!("DuckDB - error on installing {extension}: {e}")))?;
    }
    Ok(())
}

fn load_extensions(conn: &::duckdb::Connection, extensions: &[String]) -> Result<()> {
    for extension in extensions {
        conn.execute_batch(&format!("LOAD {extension}"))
            .map_err(|e| init_error(format!("DuckDB - error on loading {extension}: {e}")))?;
    }
    Ok(())
}

/// Column name + DuckDB type name of an executed statement.
fn statement_columns(stmt: &::duckdb::Statement<'_>) -> Vec<(String, String)> {
    let names = stmt.column_names();
    names
        .into_iter()
        .enumerate()
        .map(|(i, name)| {
            let lt = stmt.column_logical_type(i);
            let id = lt.id();
            let decimal = if id == ::duckdb::core::LogicalTypeId::Decimal {
                Some((lt.decimal_width(), lt.decimal_scale()))
            } else {
                None
            };
            (name, logical_type_name(id, decimal))
        })
        .collect()
}

type RawColumns = Vec<(String, String)>;

/// Runs `sql` and collects every row (blocking).
fn run_query(
    conn: &::duckdb::Connection,
    sql: &str,
    params: &[Value],
) -> Result<(RawColumns, Vec<Row>)> {
    let mut stmt = conn.prepare(sql).map_err(duck_error)?;
    let bound: Vec<::duckdb::types::Value> = params.iter().map(to_duck_value).collect();
    let mut rows = stmt
        .query(::duckdb::params_from_iter(bound.iter()))
        .map_err(duck_error)?;
    let columns = rows.as_ref().map(statement_columns).unwrap_or_default();
    let width = columns.len();
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(duck_error)? {
        let mut values = Vec::with_capacity(width);
        for i in 0..width {
            values.push(cell_to_json(row.get_ref(i).map_err(duck_error)?));
        }
        out.push(values);
    }
    Ok((columns, out))
}

impl DuckDbDriver {
    /// Creates the driver. The database is opened lazily, on first use.
    pub fn new(config: DuckDbConfig) -> Result<Self> {
        Ok(Self {
            config,
            state: tokio::sync::Mutex::new(None),
        })
    }

    /// Creates the driver from `CUBEJS_DB_*` / `CUBEJS_DB_DUCKDB_*`.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Self::new(DuckDbConfig::from_env(data_source)?)
    }

    /// The driver configuration.
    pub fn duckdb_config(&self) -> &DuckDbConfig {
        &self.config
    }

    /// `getInitiatedState`: initialises once; a failure is not cached.
    async fn state(&self) -> Result<Arc<DuckDbState>> {
        let mut guard = self.state.lock().await;
        if let Some(state) = guard.as_ref() {
            return Ok(state.clone());
        }
        let config = self.config.clone();
        let conn = tokio::task::spawn_blocking(move || init_database(&config))
            .await
            .map_err(worker_error)??;
        let state = Arc::new(DuckDbState {
            connection: Mutex::new(conn),
        });
        *guard = Some(state.clone());
        Ok(state)
    }

    fn columns(&self, raw: RawColumns) -> Vec<Column> {
        raw.into_iter()
            .map(|(name, type_name)| {
                Column::new(name, self.to_generic_type(&type_name, None, None))
            })
            .collect()
    }
}

/// `DuckDBDriver.toGenericType`.
pub fn duckdb_to_generic_type(
    column_type: &str,
    precision: Option<i64>,
    scale: Option<i64>,
    precise_decimal: bool,
) -> GenericType {
    let lower = column_type.trim().to_lowercase();
    let (mut precision, mut scale) = (precision, scale);
    // `numeric(p, s)`: the precision/scale are taken from the type name
    // (the resulting name has no entry in the base table, so it is kept).
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
    if column_type.to_lowercase() == "date" {
        // DATE_TRUNC returns DATE, but Cube Store has no DATE type; the value
        // is converted to an ISO timestamp anyway.
        return GenericType::Timestamp;
    }
    crate::types::to_generic_type(
        &column_type.to_lowercase(),
        precision,
        scale,
        precise_decimal,
    )
}

#[async_trait]
impl Driver for DuckDbDriver {
    fn config(&self) -> &DriverConfig {
        &self.config.driver
    }

    async fn test_connection(&self) -> Result<()> {
        self.query("SELECT 1", &[], &QueryOptions::default())
            .await
            .map(|_| ())
    }

    async fn query(
        &self,
        sql: &str,
        params: &[Value],
        _options: &QueryOptions,
    ) -> Result<QueryResult> {
        let state = self.state().await?;
        let sql = sql.to_string();
        let params = params.to_vec();
        let (columns, rows) = tokio::task::spawn_blocking(move || {
            let conn = state
                .connection
                .lock()
                .map_err(|_| DriverError::Other("DuckDB connection mutex poisoned".to_string()))?;
            run_query(&conn, &sql, &params)
        })
        .await
        .map_err(worker_error)??;
        Ok(QueryResult::new(self.columns(columns), rows))
    }

    fn to_generic_type(
        &self,
        db_type: &str,
        precision: Option<i64>,
        scale: Option<i64>,
    ) -> GenericType {
        duckdb_to_generic_type(
            db_type,
            precision,
            scale,
            self.config().precise_decimal_in_cubestore,
        )
    }

    fn read_only(&self) -> bool {
        false
    }

    fn information_schema_query(&self) -> String {
        let base = crate::sql::information_schema_query(&|i| self.quote_identifier(i));
        match self.config.schema.as_deref().filter(|s| !s.is_empty()) {
            Some(schema) => format!("{base} AND table_catalog = '{schema}'"),
            None => base,
        }
    }

    fn get_schemas_query(&self) -> String {
        match self.config.schema.as_deref().filter(|s| !s.is_empty()) {
            Some(schema) => format!(
                "
        SELECT table_schema as {}
        FROM information_schema.tables
        WHERE table_catalog = '{schema}'
        GROUP BY table_schema
      ",
                self.quote_identifier("schema_name")
            ),
            None => crate::sql::get_schemas_query(&|i| self.quote_identifier(i)),
        }
    }

    async fn stream(
        &self,
        sql: &str,
        params: &[Value],
        options: &StreamOptions,
    ) -> Result<StreamTableData> {
        let state = self.state().await?;
        let sql = sql.to_string();
        let params = params.to_vec();
        let (columns_tx, columns_rx) = oneshot::channel::<Result<RawColumns>>();
        let (rows_tx, rows_rx) = mpsc::channel::<Result<Row>>(options.high_water_mark.max(1));

        tokio::task::spawn_blocking(move || {
            // A new connection per stream; dropped (closed) when done.
            let conn = match state.connection.lock() {
                Ok(c) => c.try_clone().map_err(duck_error),
                Err(_) => Err(DriverError::Other(
                    "DuckDB connection mutex poisoned".to_string(),
                )),
            };
            drop(state);
            let conn = match conn {
                Ok(c) => c,
                Err(e) => {
                    let _ = columns_tx.send(Err(e));
                    return;
                }
            };
            let bound: Vec<::duckdb::types::Value> = params.iter().map(to_duck_value).collect();
            let mut stmt = match conn.prepare(&sql) {
                Ok(s) => s,
                Err(e) => {
                    let _ = columns_tx.send(Err(duck_error(e)));
                    return;
                }
            };
            let mut rows = match stmt.query(::duckdb::params_from_iter(bound.iter())) {
                Ok(r) => r,
                Err(e) => {
                    let _ = columns_tx.send(Err(duck_error(e)));
                    return;
                }
            };
            let columns = rows.as_ref().map(statement_columns).unwrap_or_default();
            let width = columns.len();
            if columns_tx.send(Ok(columns)).is_err() {
                return;
            }
            loop {
                let item = match rows.next() {
                    Ok(Some(row)) => (0..width)
                        .map(|i| row.get_ref(i).map(cell_to_json).map_err(duck_error))
                        .collect::<Result<Row>>(),
                    Ok(None) => break,
                    Err(e) => Err(duck_error(e)),
                };
                let failed = item.is_err();
                if rows_tx.blocking_send(item).is_err() || failed {
                    break;
                }
            }
        });

        let columns = columns_rx
            .await
            .map_err(|_| DriverError::Other("DuckDB stream worker stopped".to_string()))??;
        let rows = futures::stream::unfold(rows_rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        })
        .boxed();
        Ok(StreamTableData {
            columns: self.columns(columns),
            rows,
        })
    }

    async fn release(&self) -> Result<()> {
        let taken = self.state.lock().await.take();
        if let Some(state) = taken {
            tokio::task::spawn_blocking(move || drop(state))
                .await
                .map_err(worker_error)?;
        }
        Ok(())
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

    #[test]
    fn database_url_resolution() {
        let mut c = DuckDbConfig::default();
        assert_eq!(c.database_url(), ":memory:");
        c.mother_duck_token = Some("tok".into());
        assert_eq!(
            c.database_url(),
            format!("md:?motherduck_token=tok&custom_user_agent=Cube/{CUBE_VERSION}")
        );
        c.database_path = Some("/data/x.duckdb".into());
        assert_eq!(c.database_url(), "/data/x.duckdb");
    }

    #[test]
    fn reads_env_with_data_source_prefix() {
        let e = env(&[
            ("CUBEJS_DATASOURCES", "default,other"),
            ("CUBEJS_DS_OTHER_DB_DUCKDB_DATABASE_PATH", "/o.duckdb"),
            ("CUBEJS_DS_OTHER_DB_DUCKDB_EXTENSIONS", "httpfs, json"),
            ("CUBEJS_DS_OTHER_DB_DUCKDB_MEMORY_LIMIT", "1GB"),
            ("CUBEJS_DS_OTHER_DB_DUCKDB_S3_USE_CREDENTIAL_CHAIN", "TRUE"),
            ("CUBEJS_DB_DUCKDB_DATABASE_PATH", "/default.duckdb"),
        ]);
        let driver = DriverConfig::from_env_source(&e, Some("other"), false).unwrap();
        let mut c = DuckDbConfig::from_driver_config(driver);
        c.apply_env_source(&e).unwrap();
        assert_eq!(c.database_path.as_deref(), Some("/o.duckdb"));
        assert_eq!(
            c.extensions,
            Some(vec!["httpfs".to_string(), "json".to_string()])
        );
        assert_eq!(c.s3_use_credential_chain, Some(true));
        assert_eq!(c.settings(), vec![("memory_limit", "1GB".to_string())]);
    }

    #[test]
    fn explicit_values_win_over_env() {
        let e = env(&[("CUBEJS_DB_DUCKDB_SCHEMA", "env_schema")]);
        let mut c = DuckDbConfig {
            schema: Some("explicit".into()),
            ..Default::default()
        };
        c.apply_env_source(&e).unwrap();
        assert_eq!(c.schema.as_deref(), Some("explicit"));
    }

    #[test]
    fn credential_chain_must_be_boolean() {
        let e = env(&[("CUBEJS_DB_DUCKDB_S3_USE_CREDENTIAL_CHAIN", "yes")]);
        let mut c = DuckDbConfig::default();
        let err = c.apply_env_source(&e).unwrap_err();
        assert_eq!(
            err.to_string(),
            "The CUBEJS_DB_DUCKDB_S3_USE_CREDENTIAL_CHAIN must be either 'true' or 'false'."
        );
    }

    #[test]
    fn settings_order_and_escaping_input() {
        let c = DuckDbConfig {
            s3_region: Some("eu".into()),
            schema: Some("db".into()),
            s3_session_token: Some("t".into()),
            ..Default::default()
        };
        let keys: Vec<_> = c.settings().into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, vec!["s3_region", "schema", "s3_session_token"]);
    }

    #[test]
    fn generic_types() {
        assert_eq!(
            duckdb_to_generic_type("DATE", None, None, false),
            GenericType::Timestamp
        );
        assert_eq!(
            duckdb_to_generic_type("BIGINT", None, None, false),
            GenericType::Bigint
        );
        assert_eq!(
            duckdb_to_generic_type("TIMESTAMP", None, None, false),
            GenericType::Timestamp
        );
        assert_eq!(
            duckdb_to_generic_type("DECIMAL(18,3)", None, None, false),
            GenericType::Decimal(Some((18, 3)))
        );
        assert_eq!(
            duckdb_to_generic_type("VARCHAR", None, None, false),
            GenericType::Text
        );
        assert_eq!(
            duckdb_to_generic_type("numeric(10, 2)", None, None, true),
            GenericType::Other("numeric(10, 2)".into())
        );
    }
}
