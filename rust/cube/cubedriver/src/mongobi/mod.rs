//! MongoDB BI Connector driver: port of `@cubejs-backend/mongobi-driver`.
//!
//! `mongosqld` speaks the MySQL wire protocol (as MySQL 5.7), so this driver
//! uses `mysql_async` like [`crate::mysql`], with the MongoBI driver's own
//! behaviour:
//!
//! * no connection attributes at handshake (`flags: ['-CONNECT_ATTRS']`,
//!   which `mongosqld` rejects; `mysql_async` never sends them);
//! * `SET time_zone = '<storeTimezone>'` (default `+00:00`) before queries;
//! * client-side `?` interpolation with MySQL escaping (`mysql2`'s `query`);
//! * backtick quoting, `readOnly` (the BI Connector cannot create tables),
//!   `BaseDriver` type mapping and capabilities;
//! * result types from `getNativeTypeName`, which fails on a MySQL type it
//!   does not list (`downloadQueryResults` and `stream`);
//! * `informationSchemaQuery` restricted to `CUBEJS_DB_NAME`;
//! * `stream` on a dedicated connection.
//!
//! Values: `DATETIME` comes back as the server's text (the Node `typeCast`),
//! `DATE` / `TIMESTAMP` as the ISO string a JS `Date` serialises to, decimals
//! as strings and numbers as numbers (`mysql2` defaults).
//!
//! Not ported: the `SHOW PROCESSLIST` / `KILL` cancellation of `withConnection`,
//! since the driver trait has no cancellation hook (dropping the query future
//! closes the connection instead).

use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use mysql_async::consts::ColumnType;
use mysql_async::prelude::Queryable;
use mysql_async::{Conn, Opts, OptsBuilder, Pool, PoolConstraints, PoolOpts, SslOpts};
use serde_json::Value;

use crate::config::DriverConfig;
use crate::driver::Driver;
use crate::error::{DriverError, Result};
use crate::escape::format_mysql;
use crate::postgres::create_pool_name;
use crate::types::{
    Column, DownloadQueryResultsOptions, DownloadedData, QueryOptions, QueryResult, Row,
    StreamOptions, StreamTableData,
};

/// `getDefaultConcurrency`.
pub const DEFAULT_CONCURRENCY: usize = 2;

/// `getNativeTypeName` (`MySqlNativeToMySqlType` of `MySQLType.ts`).
pub fn native_type_name(column_type: ColumnType) -> Result<&'static str> {
    Ok(match column_type {
        ColumnType::MYSQL_TYPE_DECIMAL | ColumnType::MYSQL_TYPE_NEWDECIMAL => "decimal",
        ColumnType::MYSQL_TYPE_TINY => "tinyint",
        ColumnType::MYSQL_TYPE_SHORT => "smallint",
        ColumnType::MYSQL_TYPE_LONG => "int",
        ColumnType::MYSQL_TYPE_INT24 => "mediumint",
        ColumnType::MYSQL_TYPE_LONGLONG => "bigint",
        ColumnType::MYSQL_TYPE_NEWDATE => "datetime",
        ColumnType::MYSQL_TYPE_TIMESTAMP => "timestamp",
        ColumnType::MYSQL_TYPE_DATETIME => "datetime",
        ColumnType::MYSQL_TYPE_TIME => "time",
        ColumnType::MYSQL_TYPE_TINY_BLOB => "tinytext",
        ColumnType::MYSQL_TYPE_MEDIUM_BLOB => "mediumtext",
        ColumnType::MYSQL_TYPE_LONG_BLOB => "longtext",
        ColumnType::MYSQL_TYPE_BLOB => "text",
        ColumnType::MYSQL_TYPE_VAR_STRING => "varchar",
        ColumnType::MYSQL_TYPE_STRING => "binary",
        ColumnType::MYSQL_TYPE_FLOAT => "float",
        ColumnType::MYSQL_TYPE_DOUBLE => "double",
        other => {
            return Err(DriverError::TypeDetection(format!(
                "Unsupported mapping for data type: {}",
                other as u8
            )))
        }
    })
}

/// Configuration of [`MongoBiDriver`] (`MongoBIDriverConfiguration`).
#[derive(Debug, Clone)]
pub struct MongoBiConfig {
    /// Connection settings and global driver knobs.
    pub driver: DriverConfig,
    /// `storeTimezone` (`SET time_zone`, default `+00:00`).
    pub store_timezone: String,
    /// `maxPoolSize` option (above `CUBEJS_DB_MAX_POOL`; default 8).
    pub max_pool_size: Option<usize>,
    /// `idleTimeoutMillis` (30 s).
    pub idle_timeout: Duration,
}

impl MongoBiConfig {
    /// Builds the configuration from a generic [`DriverConfig`].
    pub fn from_driver_config(driver: DriverConfig) -> Self {
        Self {
            driver,
            store_timezone: "+00:00".to_string(),
            max_pool_size: None,
            idle_timeout: Duration::from_millis(30_000),
        }
    }

    /// Reads the configuration of `data_source` from the environment.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Ok(Self::from_driver_config(DriverConfig::from_env(
            data_source,
        )?))
    }

    /// Reads the configuration from `mysql://user:pass@host:port/db`, for
    /// tests and tooling. The fields are filled in too, because
    /// `informationSchemaQuery` is built from the database name.
    pub fn from_url(url: &str) -> Self {
        let mut driver = DriverConfig::default();
        driver.data_source.url = Some(url.to_string());
        if let Ok(opts) = Opts::from_url(url) {
            driver.data_source.host = Some(opts.ip_or_hostname().to_string());
            driver.data_source.port = Some(opts.tcp_port());
            driver.data_source.user = opts.user().map(str::to_string);
            driver.data_source.password = opts.pass().map(str::to_string);
            driver.data_source.database = opts.db_name().map(str::to_string);
        }
        Self::from_driver_config(driver)
    }

    /// `config.maxPoolSize || getEnv('dbMaxPoolSize') || 8`.
    pub fn pool_size(&self) -> usize {
        self.max_pool_size
            .filter(|v| *v > 0)
            .unwrap_or_else(|| self.driver.data_source.effective_max_pool_size())
    }

    /// Name of the pool (`mongobi#<data source>[@preAggregations]`).
    pub fn pool_name(&self) -> String {
        create_pool_name(
            "mongobi",
            &self.driver.data_source.data_source,
            self.driver.data_source.pre_aggregations,
        )
    }

    /// `prepareConnection`.
    pub fn time_zone_statement(&self) -> String {
        format!(
            "SET time_zone = '{}'",
            self.store_timezone.replace('\'', "''")
        )
    }

    /// Translates the configuration into `mysql_async` options.
    pub fn opts(&self) -> Result<Opts> {
        let ds = &self.driver.data_source;
        let mut builder = match &ds.url {
            Some(url) => OptsBuilder::from_opts(
                Opts::from_url(url)
                    .map_err(|e| DriverError::Config(format!("Invalid CUBEJS_DB_URL: {e}")))?,
            ),
            None => OptsBuilder::default(),
        };
        if let Some(host) = &ds.host {
            builder = builder.ip_or_hostname(host.clone());
        }
        if let Some(port) = ds.port {
            builder = builder.tcp_port(port);
        }
        if ds.user.is_some() {
            builder = builder.user(ds.user.clone());
        }
        if ds.password.is_some() {
            builder = builder.pass(ds.password.clone());
        }
        if ds.database.is_some() {
            builder = builder.db_name(ds.database.clone());
        }
        if let Some(ssl) = &ds.ssl {
            let mut opts = SslOpts::default()
                .with_danger_accept_invalid_certs(!ssl.reject_unauthorized)
                .with_danger_skip_domain_validation(!ssl.reject_unauthorized);
            if let Some(ca) = &ssl.ca {
                opts = opts.with_root_certs(vec![ca.clone().into_bytes().into()]);
            }
            builder = builder.ssl_opts(opts);
        }
        let max = self.pool_size().max(1);
        let constraints = PoolConstraints::new(0, max)
            .ok_or_else(|| DriverError::Config(format!("Invalid pool constraints: max {max}")))?;
        builder = builder
            .pool_opts(
                PoolOpts::default()
                    .with_constraints(constraints)
                    .with_inactive_connection_ttl(self.idle_timeout),
            )
            // The Node driver sets the time zone before every query; once per
            // connection is equivalent, the session keeps it.
            .init(vec![self.time_zone_statement()]);
        Ok(Opts::from(builder))
    }
}

/// MongoDB BI Connector driver.
pub struct MongoBiDriver {
    config: MongoBiConfig,
    opts: Opts,
    pool: Pool,
    pool_name: String,
}

impl std::fmt::Debug for MongoBiDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MongoBiDriver")
            .field("pool_name", &self.pool_name)
            .finish()
    }
}

/// Converts a cell like `mysql2` without `dateStrings`: temporal types other
/// than `DATETIME` become the ISO string of the JS `Date` (the session time
/// zone is UTC, as is the Cube process).
fn decode_value(column: &mysql_async::Column, value: &mysql_async::Value) -> Value {
    let decoded = crate::mysql::decode_value(column, value);
    match (column.column_type(), &decoded) {
        (ColumnType::MYSQL_TYPE_DATE | ColumnType::MYSQL_TYPE_NEWDATE, Value::String(s)) => {
            iso_date_time(&format!("{s} 00:00:00")).map_or(decoded, Value::String)
        }
        (
            ColumnType::MYSQL_TYPE_TIMESTAMP | ColumnType::MYSQL_TYPE_TIMESTAMP2,
            Value::String(s),
        ) => iso_date_time(s).map_or(decoded, Value::String),
        _ => decoded,
    }
}

/// `2020-01-01 10:00:00[.ffffff]` → `2020-01-01T10:00:00.000Z`.
fn iso_date_time(s: &str) -> Option<String> {
    let parsed = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f").ok()?;
    Some(parsed.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
}

fn convert_row(columns: &[mysql_async::Column], row: mysql_async::Row) -> Row {
    let row = row.unwrap();
    columns
        .iter()
        .enumerate()
        .map(|(i, c)| match row.get(i) {
            Some(v) => decode_value(c, v),
            None => Value::Null,
        })
        .collect()
}

impl MongoBiDriver {
    /// Creates the driver and its (lazy) connection pool.
    pub fn new(config: MongoBiConfig) -> Result<Self> {
        let opts = config.opts()?;
        let pool_name = config.pool_name();
        Ok(Self {
            pool: Pool::new(opts.clone()),
            config,
            opts,
            pool_name,
        })
    }

    /// Creates the driver from `CUBEJS_DB_*`.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Self::new(MongoBiConfig::from_env(data_source)?)
    }

    /// The driver configuration.
    pub fn mongobi_config(&self) -> &MongoBiConfig {
        &self.config
    }

    /// Name of the pool (`mongobi#default`).
    pub fn pool_name(&self) -> &str {
        &self.pool_name
    }

    fn mysql_error(&self, e: mysql_async::Error) -> DriverError {
        match &e {
            mysql_async::Error::Server(server) => DriverError::Database {
                message: server.message.clone(),
                code: Some(server.code.to_string()),
            },
            mysql_async::Error::Driver(_) | mysql_async::Error::Io(_) => DriverError::Connection {
                pool_name: self.pool_name.clone(),
                message: e.to_string(),
            },
            _ => DriverError::Query(e.to_string()),
        }
    }

    async fn connect_standalone(&self) -> Result<Conn> {
        tokio::time::timeout(
            self.config.driver.test_connection_timeout,
            Conn::new(self.opts.clone()),
        )
        .await
        .map_err(|_| DriverError::Connection {
            pool_name: self.pool_name.clone(),
            message: format!(
                "connection timed out after {:?}",
                self.config.driver.test_connection_timeout
            ),
        })?
        .map_err(|e| self.mysql_error(e))
    }

    /// Fields mapped through `getNativeTypeName` and `BaseDriver.toGenericType`.
    fn map_fields(&self, columns: &[mysql_async::Column], strict: bool) -> Result<Vec<Column>> {
        columns
            .iter()
            .map(|c| {
                let name = match native_type_name(c.column_type()) {
                    Ok(name) => name,
                    Err(e) if strict => return Err(e),
                    Err(_) => crate::mysql::native_to_mysql_type(c.column_type()),
                };
                Ok(Column::new(
                    c.name_str().to_string(),
                    self.to_generic_type(name, None, None),
                ))
            })
            .collect()
    }

    async fn query_response(
        &self,
        sql: &str,
        params: &[Value],
        strict: bool,
    ) -> Result<QueryResult> {
        let formatted = format_mysql(sql, params);
        let mut conn = self
            .pool
            .get_conn()
            .await
            .map_err(|e| self.mysql_error(e))?;
        let mut result = conn
            .query_iter(formatted)
            .await
            .map_err(|e| self.mysql_error(e))?;
        let mysql_columns = result.columns().map(|c| c.to_vec()).unwrap_or_default();
        let columns = self.map_fields(&mysql_columns, strict)?;
        let rows: Vec<Row> = result
            .map(|row| convert_row(&mysql_columns, row))
            .await
            .map_err(|e| self.mysql_error(e))?;
        Ok(QueryResult::new(columns, rows))
    }
}

#[async_trait]
impl Driver for MongoBiDriver {
    fn config(&self) -> &DriverConfig {
        &self.config.driver
    }

    /// `SELECT 1` on a fresh connection.
    async fn test_connection(&self) -> Result<()> {
        let mut conn = self.connect_standalone().await?;
        let result = conn
            .query_drop("SELECT 1")
            .await
            .map_err(|e| self.mysql_error(e));
        let _ = conn.disconnect().await;
        result
    }

    async fn query(
        &self,
        sql: &str,
        params: &[Value],
        _options: &QueryOptions,
    ) -> Result<QueryResult> {
        self.query_response(sql, params, false).await
    }

    fn quote_identifier(&self, identifier: &str) -> String {
        format!("`{identifier}`")
    }

    /// The BI Connector does not support table creation.
    fn read_only(&self) -> bool {
        true
    }

    fn information_schema_query(&self) -> String {
        let base = crate::sql::information_schema_query(&|i| self.quote_identifier(i));
        match self
            .config
            .driver
            .data_source
            .database
            .as_deref()
            .filter(|d| !d.is_empty())
        {
            Some(db) => format!("{base} AND columns.table_schema = '{db}'"),
            None => base,
        }
    }

    async fn release(&self) -> Result<()> {
        self.pool
            .clone()
            .disconnect()
            .await
            .map_err(|e| self.mysql_error(e))
    }

    async fn stream(
        &self,
        sql: &str,
        params: &[Value],
        options: &StreamOptions,
    ) -> Result<StreamTableData> {
        let formatted = format_mysql(sql, params);
        let mut conn = self.connect_standalone().await?;
        // `prepareConnection` on the dedicated connection (the init
        // statement already ran; kept for parity with Node).
        conn.query_drop(self.config.time_zone_statement())
            .await
            .map_err(|e| self.mysql_error(e))?;

        let (columns_tx, columns_rx) = tokio::sync::oneshot::channel();
        let (rows_tx, rows_rx) =
            tokio::sync::mpsc::channel::<Result<Row>>(options.high_water_mark.max(1));
        let pool_name = self.pool_name.clone();
        let precise_decimal = self.config.driver.precise_decimal_in_cubestore;

        tokio::spawn(async move {
            let mut result = match conn.query_iter(formatted).await {
                Ok(r) => r,
                Err(e) => {
                    let err = match &e {
                        mysql_async::Error::Server(s) => DriverError::Database {
                            message: s.message.clone(),
                            code: Some(s.code.to_string()),
                        },
                        _ => DriverError::Connection {
                            pool_name,
                            message: e.to_string(),
                        },
                    };
                    let _ = columns_tx.send(Err(err));
                    let _ = conn.disconnect().await;
                    return;
                }
            };
            let mysql_columns = result.columns().map(|c| c.to_vec()).unwrap_or_default();
            let columns: Result<Vec<Column>> = mysql_columns
                .iter()
                .map(|c| {
                    Ok(Column::new(
                        c.name_str().to_string(),
                        crate::types::to_generic_type(
                            native_type_name(c.column_type())?,
                            None,
                            None,
                            precise_decimal,
                        ),
                    ))
                })
                .collect();
            let failed = columns.is_err();
            if columns_tx.send(columns).is_err() || failed {
                drop(result);
                let _ = conn.disconnect().await;
                return;
            }
            loop {
                match result.next().await {
                    Ok(Some(row)) => {
                        if rows_tx
                            .send(Ok(convert_row(&mysql_columns, row)))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        let _ = rows_tx.send(Err(DriverError::Query(e.to_string()))).await;
                        break;
                    }
                }
            }
            drop(result);
            let _ = conn.disconnect().await;
        });

        let columns = columns_rx.await.map_err(|_| {
            DriverError::Query("MongoBI stream ended before the result set header".to_string())
        })??;
        let rows = futures::stream::unfold(rows_rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        })
        .boxed();
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
            self.query_response(sql, params, true).await?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::GenericType;

    fn driver() -> MongoBiDriver {
        let mut config = DriverConfig::default();
        config.data_source.database = Some("test".into());
        MongoBiDriver::new(MongoBiConfig::from_driver_config(config)).unwrap()
    }

    #[test]
    fn native_types() {
        assert_eq!(
            native_type_name(ColumnType::MYSQL_TYPE_STRING).unwrap(),
            "binary"
        );
        assert_eq!(
            native_type_name(ColumnType::MYSQL_TYPE_DOUBLE).unwrap(),
            "double"
        );
        assert_eq!(
            native_type_name(ColumnType::MYSQL_TYPE_LONGLONG).unwrap(),
            "bigint"
        );
        assert_eq!(
            native_type_name(ColumnType::MYSQL_TYPE_DATE)
                .unwrap_err()
                .to_string(),
            "Unsupported mapping for data type: 10"
        );
        assert_eq!(
            native_type_name(ColumnType::MYSQL_TYPE_JSON)
                .unwrap_err()
                .to_string(),
            "Unsupported mapping for data type: 245"
        );
    }

    #[tokio::test]
    async fn contract() {
        let d = driver();
        assert_eq!(d.pool_name(), "mongobi#default");
        assert!(d.read_only());
        assert_eq!(d.param(0), "?");
        assert_eq!(d.quote_identifier("a"), "`a`");
        assert!(!d.capabilities().incremental_schema_loading);
        assert!(d
            .information_schema_query()
            .ends_with(" AND columns.table_schema = 'test'"));
        assert_eq!(
            d.mongobi_config().time_zone_statement(),
            "SET time_zone = '+00:00'"
        );
        // `BaseDriver.toGenericType`, not MySQL's
        assert_eq!(d.to_generic_type("bigint", None, None), GenericType::Bigint);
        assert_eq!(
            d.to_generic_type("datetime", None, None),
            GenericType::Timestamp
        );
        assert_eq!(
            d.to_generic_type("binary", None, None),
            GenericType::Other("binary".into())
        );
        assert_eq!(d.mongobi_config().pool_size(), 8);
        assert_eq!(DEFAULT_CONCURRENCY, 2);

        let d2 =
            MongoBiDriver::new(MongoBiConfig::from_driver_config(DriverConfig::default())).unwrap();
        assert!(!d2
            .information_schema_query()
            .contains("columns.table_schema = '"));
    }

    #[test]
    fn js_dates() {
        assert_eq!(
            iso_date_time("1998-08-02 00:00:00").as_deref(),
            Some("1998-08-02T00:00:00.000Z")
        );
        assert_eq!(
            iso_date_time("2020-01-01 10:00:00.123456").as_deref(),
            Some("2020-01-01T10:00:00.123Z")
        );
        assert_eq!(iso_date_time("0000-00-00 00:00:00"), None);
    }

    #[test]
    fn opts() {
        let config = MongoBiConfig::from_url("mysql://u:p@h:3307/test");
        assert_eq!(config.driver.data_source.database.as_deref(), Some("test"));
        let opts = config.opts().unwrap();
        assert_eq!(opts.tcp_port(), 3307);
        assert_eq!(opts.init(), &["SET time_zone = '+00:00'".to_string()]);
    }
}
