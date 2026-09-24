//! Vertica driver: port of `@cubejs-backend/vertica-driver`.
//!
//! The Node driver sits on `vertica-nodejs` (a fork of `node-postgres`); this
//! one on a small protocol client of its own ([`wire`]), because
//! `tokio-postgres` cannot talk to Vertica (see the module docs of [`wire`]).
//!
//! Ported behaviour:
//!
//! * `CUBEJS_DB_HOST`, `_PORT` (default 5433), `_NAME`, `_USER`, `_PASS`,
//!   `CUBEJS_DB_SSL*`; pool size `CUBEJS_DB_MAX_POOL`, else `maxPoolSize`,
//!   else 8 (the environment wins, as in Node);
//! * `SET TIMEZONE TO 'UTC'` on every new connection;
//! * `?` parameters (interpolated client-side with ANSI escaping), `"`
//!   quoting, `readOnly`, `BaseDriver` capabilities and `wrapQueryWithLimit`;
//! * `v_catalog` introspection (`informationSchemaQuery`, `getTablesQuery`,
//!   `tableColumnTypes`) and `CREATE SCHEMA IF NOT EXISTS`;
//! * `toGenericType` with Vertica's own table (unknown types become `text`).
//!
//! Values come back as `vertica-nodejs` returns them: integers and floats as
//! numbers, booleans as booleans, everything else (dates, timestamps,
//! numerics, …) as the server's text.
//!
//! Like Node, there is no `stream` (the `BaseDriver` default refuses it) and
//! no unload. The dialect (`VerticaQuery`) belongs to the schema compiler.

pub mod wire;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::config::DriverConfig;
use crate::driver::Driver;
use crate::error::{DriverError, Result};
use crate::escape::format_ansi;
use crate::types::{
    Column, GenericType, QueryOptions, QueryResult, Row, TableName, TableStructure,
};

use wire::{ConnectOptions, Connection, Field};

/// Default port of Vertica.
pub const DEFAULT_PORT: u16 = 5433;
/// Pool size when neither `CUBEJS_DB_MAX_POOL` nor `maxPoolSize` is set.
pub const DEFAULT_MAX_POOL_SIZE: usize = 8;
/// `getDefaultConcurrency` (`BaseDriver`'s).
pub const DEFAULT_CONCURRENCY: usize = 2;
/// `idleTimeoutMillis` of the `pg`-style pool (10 s).
pub const IDLE_TIMEOUT: Duration = Duration::from_millis(10_000);
/// Statement run on every new connection (`connectListener`).
pub const CONNECT_STATEMENT: &str = "SET TIMEZONE TO 'UTC'";

/// `VerticaTypeToGenericType`.
fn vertica_to_generic(vertica_type: &str) -> Option<&'static str> {
    Some(match vertica_type {
        "boolean" => "boolean",
        "int" => "bigint",
        "float" => "double",
        "date" => "date",
        "timestamp" => "timestamp",
        "timestamptz" => "timestamp",
        "numeric" => "decimal",
        _ => return None,
    })
}

/// `VerticaDriver.toGenericType`: lower-cased, the first `(p,s)` removed,
/// looked up in `VerticaTypeToGenericType`, `text` otherwise.
pub fn to_generic_type(
    column_type: &str,
    precision: Option<i64>,
    scale: Option<i64>,
    precise_decimal: bool,
) -> GenericType {
    let lower = column_type.to_lowercase();
    // `.replace(/\([0-9,]+\)/, '')`: the first parenthesised run of digits/commas.
    let mut stripped = lower.clone();
    let mut search = 0;
    while let Some(open) = lower[search..].find('(').map(|i| i + search) {
        match lower[open + 1..].find(')') {
            Some(len)
                if len > 0
                    && lower[open + 1..open + 1 + len]
                        .chars()
                        .all(|c| c.is_ascii_digit() || c == ',') =>
            {
                stripped = format!("{}{}", &lower[..open], &lower[open + 2 + len..]);
                break;
            }
            _ => search = open + 1,
        }
    }
    let generic = vertica_to_generic(&stripped).unwrap_or("text");
    if generic == "decimal" && precise_decimal {
        if let (Some(p), Some(s)) = (precision, scale) {
            if p > 0 && s > 0 {
                return GenericType::Decimal(Some((p as u32, s as u32)));
            }
        }
    }
    GenericType::parse(generic)
}

/// Vertica's type name for a result column OID (for [`to_generic_type`]).
pub fn type_name_for_oid(oid: u32) -> &'static str {
    match oid {
        5 => "boolean",
        6 => "int",
        7 => "float",
        8 => "char",
        9 => "varchar",
        10 => "date",
        11 => "time",
        12 => "timestamp",
        13 => "timestamptz",
        14 => "interval",
        15 => "timetz",
        16 => "numeric",
        17 => "varbinary",
        20 => "uuid",
        114 => "interval year to month",
        115 => "long varchar",
        116 => "long varbinary",
        117 => "binary",
        _ => "unknown",
    }
}

/// Decodes a text value the way `vertica-nodejs` does.
pub fn decode_value(oid: u32, text: Option<String>) -> Value {
    let Some(text) = text else {
        return Value::Null;
    };
    match oid {
        5 => match text.as_str() {
            "t" | "true" | "1" | "TRUE" | "y" | "yes" | "on" => Value::Bool(true),
            "f" | "false" | "0" | "FALSE" | "n" | "no" | "off" => Value::Bool(false),
            _ => Value::String(text),
        },
        6 => text
            .parse::<i64>()
            .map(Value::from)
            .unwrap_or(Value::String(text)),
        7 => match text.parse::<f64>() {
            // `NaN` / `Infinity` have no JSON form.
            Ok(v) => serde_json::Number::from_f64(v)
                .map(Value::Number)
                .unwrap_or(Value::Null),
            Err(_) => Value::String(text),
        },
        _ => Value::String(text),
    }
}

/// Configuration of [`VerticaDriver`].
#[derive(Debug, Clone)]
pub struct VerticaConfig {
    /// Connection settings.
    pub driver: DriverConfig,
    /// `maxPoolSize` option (below `CUBEJS_DB_MAX_POOL`).
    pub max_pool_size: Option<usize>,
}

impl VerticaConfig {
    /// Builds the configuration from a generic [`DriverConfig`].
    pub fn from_driver_config(driver: DriverConfig) -> Self {
        Self {
            driver,
            max_pool_size: None,
        }
    }

    /// Reads the configuration of `data_source` from the environment.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Ok(Self::from_driver_config(DriverConfig::from_env(
            data_source,
        )?))
    }

    /// Reads the configuration from `vertica://user:pass@host:port/db`, for
    /// tests and tooling.
    pub fn from_url(url: &str) -> Result<Self> {
        let parsed = url::Url::parse(url)
            .map_err(|e| DriverError::Config(format!("Invalid Vertica URL: {e}")))?;
        let mut driver = DriverConfig::default();
        let ds = &mut driver.data_source;
        ds.host = parsed.host_str().map(str::to_string);
        ds.port = parsed.port();
        if !parsed.username().is_empty() {
            ds.user = Some(percent_decode(parsed.username()));
        }
        ds.password = parsed.password().map(percent_decode);
        let db = parsed.path().trim_start_matches('/');
        if !db.is_empty() {
            ds.database = Some(percent_decode(db));
        }
        Ok(Self::from_driver_config(driver))
    }

    /// `CUBEJS_DB_MAX_POOL || maxPoolSize || 8`.
    pub fn pool_size(&self) -> usize {
        self.driver
            .data_source
            .max_pool_size
            .filter(|v| *v > 0)
            .or(self.max_pool_size.filter(|v| *v > 0))
            .unwrap_or(DEFAULT_MAX_POOL_SIZE)
    }

    /// The connection parameters.
    pub fn connect_options(&self) -> ConnectOptions {
        let ds = &self.driver.data_source;
        ConnectOptions {
            host: ds
                .host
                .clone()
                .filter(|h| !h.is_empty())
                .unwrap_or_else(|| "localhost".to_string()),
            port: ds.port.unwrap_or(DEFAULT_PORT),
            user: ds
                .user
                .clone()
                .or_else(|| std::env::var("USER").ok())
                .unwrap_or_default(),
            password: ds.password.clone(),
            database: ds.database.clone(),
            ssl: ds.ssl.clone(),
        }
    }
}

fn percent_decode(s: &str) -> String {
    url::form_urlencoded::parse(format!("x={}", s.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_else(|| s.to_string())
}

struct Idle {
    conn: Connection,
    since: Instant,
}

struct PoolInner {
    options: ConnectOptions,
    semaphore: Arc<Semaphore>,
    idle: Mutex<Vec<Idle>>,
}

/// A connection borrowed from the pool; returned on drop unless broken.
struct Pooled {
    conn: Option<Connection>,
    pool: Arc<PoolInner>,
    _permit: OwnedSemaphorePermit,
}

impl Drop for Pooled {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            if !conn.broken && !self.pool.semaphore.is_closed() {
                if let Ok(mut idle) = self.pool.idle.lock() {
                    idle.push(Idle {
                        conn,
                        since: Instant::now(),
                    });
                }
            }
        }
    }
}

impl Pooled {
    fn conn(&mut self) -> &mut Connection {
        self.conn.as_mut().expect("pooled connection")
    }
}

/// Vertica driver.
pub struct VerticaDriver {
    config: VerticaConfig,
    pool: Arc<PoolInner>,
}

impl std::fmt::Debug for VerticaDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerticaDriver")
            .field("host", &self.pool.options.host)
            .field("port", &self.pool.options.port)
            .field("pool_size", &self.config.pool_size())
            .finish()
    }
}

impl VerticaDriver {
    /// Creates the driver. No connection is opened until the first query.
    pub fn new(config: VerticaConfig) -> Result<Self> {
        if let Some(ssl) = &config.driver.data_source.ssl {
            // Fail on unusable TLS material now rather than at first query.
            crate::postgres::client_config(ssl)?;
        }
        let pool = Arc::new(PoolInner {
            options: config.connect_options(),
            semaphore: Arc::new(Semaphore::new(config.pool_size())),
            idle: Mutex::new(Vec::new()),
        });
        Ok(Self { config, pool })
    }

    /// Creates the driver from `CUBEJS_DB_*`.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Self::new(VerticaConfig::from_env(data_source)?)
    }

    /// The driver configuration.
    pub fn vertica_config(&self) -> &VerticaConfig {
        &self.config
    }

    async fn acquire(&self) -> Result<Pooled> {
        let permit = self
            .pool
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| DriverError::Query("The Vertica pool is closed".to_string()))?;
        let reused = {
            let mut idle = self.pool.idle.lock().unwrap();
            // `idleTimeoutMillis`: idle connections older than 10 s are closed.
            idle.retain(|i| i.since.elapsed() < IDLE_TIMEOUT);
            idle.pop().map(|i| i.conn)
        };
        let conn = match reused {
            Some(conn) => conn,
            None => {
                let mut conn = Connection::connect(&self.pool.options).await?;
                conn.simple_query(CONNECT_STATEMENT).await?;
                conn
            }
        };
        Ok(Pooled {
            conn: Some(conn),
            pool: self.pool.clone(),
            _permit: permit,
        })
    }

    fn map_fields(&self, fields: &[Field]) -> Vec<Column> {
        fields
            .iter()
            .map(|f| {
                Column::new(
                    f.name.clone(),
                    self.to_generic_type(type_name_for_oid(f.type_oid), None, None),
                )
            })
            .collect()
    }
}

#[async_trait]
impl Driver for VerticaDriver {
    fn config(&self) -> &DriverConfig {
        &self.config.driver
    }

    async fn test_connection(&self) -> Result<()> {
        self.query("SELECT 1 AS n", &[], &QueryOptions::default())
            .await?;
        Ok(())
    }

    /// `pool.query(query, values)`: the rows of the last result set.
    async fn query(
        &self,
        sql: &str,
        params: &[Value],
        _options: &QueryOptions,
    ) -> Result<QueryResult> {
        let sql = format_ansi(sql, params);
        let mut conn = self.acquire().await?;
        let results = conn.conn().simple_query(&sql).await?;
        let Some(result) = results.into_iter().rev().find(|r| !r.fields.is_empty()) else {
            return Ok(QueryResult::default());
        };
        let columns = self.map_fields(&result.fields);
        let rows = result
            .rows
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .zip(result.fields.iter())
                    .map(|(v, f)| decode_value(f.type_oid, v))
                    .collect::<Row>()
            })
            .collect();
        Ok(QueryResult::new(columns, rows))
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
        true
    }

    async fn release(&self) -> Result<()> {
        self.pool.semaphore.close();
        let idle: Vec<Idle> = std::mem::take(&mut *self.pool.idle.lock().unwrap());
        for i in idle {
            i.conn.close().await;
        }
        Ok(())
    }

    fn information_schema_query(&self) -> String {
        "
      SELECT
        column_name,
        table_name,
        table_schema,
        data_type
      FROM v_catalog.columns;
    "
        .to_string()
    }

    async fn create_schema_if_not_exists(&self, schema_name: &str) -> Result<()> {
        self.query(
            &format!("CREATE SCHEMA IF NOT EXISTS {schema_name};"),
            &[],
            &QueryOptions::default(),
        )
        .await?;
        Ok(())
    }

    async fn get_tables_query(&self, schema_name: &str) -> Result<Vec<String>> {
        let result = self
            .query(
                &format!(
                    "SELECT table_name FROM v_catalog.tables WHERE table_schema = {}",
                    self.param(0)
                ),
                &[Value::from(schema_name)],
                &QueryOptions::default(),
            )
            .await?;
        Ok((0..result.len())
            .filter_map(|i| result.get_string(i, "table_name"))
            .collect())
    }

    async fn table_column_types(&self, table: &str) -> Result<TableStructure> {
        let TableName { schema, name } = TableName::split(table);
        // JS: `const [schema, name] = table.split('.')`.
        let name = name.split('.').next().unwrap_or("").to_string();
        let result = self
            .query(
                &format!(
                    "SELECT
        column_name,
        data_type,
        numeric_precision,
        numeric_scale
      FROM v_catalog.columns
      WHERE table_name = {}
        AND table_schema = {}",
                    self.param(0),
                    self.param(1)
                ),
                &[Value::from(name), Value::from(schema)],
                &QueryOptions::default(),
            )
            .await?;
        Ok((0..result.len())
            .filter_map(|i| {
                let column = result.get_string(i, "column_name")?;
                let data_type = result.get_string(i, "data_type")?;
                Some(Column::new(
                    column,
                    self.to_generic_type(
                        &data_type,
                        result.get_i64(i, "numeric_precision"),
                        result.get_i64(i, "numeric_scale"),
                    ),
                ))
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_types() {
        let g = |t: &str| to_generic_type(t, None, None, false);
        assert_eq!(g("int"), GenericType::Bigint);
        assert_eq!(g("Integer"), GenericType::Text);
        assert_eq!(g("date"), GenericType::Date);
        assert_eq!(g("timestamp"), GenericType::Timestamp);
        assert_eq!(g("timestamptz"), GenericType::Timestamp);
        assert_eq!(g("numeric(10,2)"), GenericType::Decimal(None));
        assert_eq!(g("varchar(64)"), GenericType::Text);
        assert_eq!(g("char(2)"), GenericType::Text);
        assert_eq!(g("set[int8]"), GenericType::Text);
        assert_eq!(g("array[int8]"), GenericType::Text);
        assert_eq!(g("boolean"), GenericType::Boolean);
        assert_eq!(g("float"), GenericType::Double);
        assert_eq!(
            to_generic_type("numeric(10,2)", Some(10), Some(2), true),
            GenericType::Decimal(Some((10, 2)))
        );
        assert_eq!(
            to_generic_type("numeric(10,0)", Some(10), Some(0), true),
            GenericType::Decimal(None)
        );
    }

    #[test]
    fn values_like_vertica_nodejs() {
        assert_eq!(decode_value(6, Some("1".into())), Value::from(1));
        assert_eq!(decode_value(7, Some("1.5".into())), Value::from(1.5));
        assert_eq!(decode_value(7, Some("NaN".into())), Value::Null);
        assert_eq!(decode_value(5, Some("t".into())), Value::Bool(true));
        assert_eq!(decode_value(5, Some("f".into())), Value::Bool(false));
        assert_eq!(decode_value(16, Some("1.01".into())), Value::from("1.01"));
        assert_eq!(
            decode_value(12, Some("2020-01-01 00:00:00".into())),
            Value::from("2020-01-01 00:00:00")
        );
        assert_eq!(decode_value(9, None), Value::Null);
    }

    #[test]
    fn config() {
        let c = VerticaConfig::from_url("vertica://dbadmin:p%40ss@h:5434/test").unwrap();
        let o = c.connect_options();
        assert_eq!(o.host, "h");
        assert_eq!(o.port, 5434);
        assert_eq!(o.user, "dbadmin");
        assert_eq!(o.password.as_deref(), Some("p@ss"));
        assert_eq!(o.database.as_deref(), Some("test"));

        let mut c = VerticaConfig::from_driver_config(DriverConfig::default());
        assert_eq!(c.connect_options().port, 5433);
        assert_eq!(c.connect_options().host, "localhost");
        assert_eq!(c.pool_size(), 8);
        c.max_pool_size = Some(3);
        assert_eq!(c.pool_size(), 3);
        // `CUBEJS_DB_MAX_POOL` wins over the option
        c.driver.data_source.max_pool_size = Some(5);
        assert_eq!(c.pool_size(), 5);
    }

    #[tokio::test]
    async fn driver_contract() {
        let d =
            VerticaDriver::new(VerticaConfig::from_driver_config(DriverConfig::default())).unwrap();
        assert!(d.read_only());
        assert_eq!(d.param(0), "?");
        assert_eq!(d.quote_identifier("a"), "\"a\"");
        assert!(!d.capabilities().incremental_schema_loading);
        assert!(d
            .information_schema_query()
            .contains("FROM v_catalog.columns;"));
        assert_eq!(d.from_generic_type(&GenericType::Int), "int");
        let err = d
            .stream("SELECT 1", &[], &Default::default())
            .await
            .unwrap_err();
        assert!(matches!(err, DriverError::NotImplemented(_)));
        d.release().await.unwrap();
        // idempotent, and closed afterwards
        d.release().await.unwrap();
        let err = d
            .query("SELECT 1", &[], &QueryOptions::default())
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "The Vertica pool is closed");
    }
}
