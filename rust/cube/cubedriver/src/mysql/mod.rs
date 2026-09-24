//! MySQL driver: port of `@cubejs-backend/mysql-driver` on top of
//! `mysql_async` (rustls, never OpenSSL).
//!
//! Like `mysql2`'s `connection.query(sql, values)`, query parameters are
//! interpolated client-side with the MySQL escaping dialect (see
//! [`crate::escape`]) rather than sent as prepared-statement parameters, so the
//! SQL that reaches the server is the same text the Node driver produces.

mod decode;
pub mod types;

use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use mysql_async::prelude::Queryable;
use mysql_async::{Conn, Opts, OptsBuilder, Pool, PoolConstraints, PoolOpts, SslOpts};
use serde_json::Value;

use crate::config::DriverConfig;
use crate::driver::Driver;
use crate::error::{DriverError, Result};
use crate::escape::format_mysql;
use crate::postgres::create_pool_name;
use crate::types::{
    Column, DownloadQueryResultsOptions, DownloadedData, DriverCapabilities,
    ExternalCreateTableOptions, GenericType, IndexSql, QueryOptions, QueryResult, Row,
    StreamOptions, StreamTableData, TableMemoryData, TableStructure,
};

pub use decode::decode_value;
pub use types::{generic_to_mysql, mysql_to_generic, native_to_mysql_type};

/// Default port (`CUBEJS_DB_PORT`).
pub const DEFAULT_PORT: u16 = 3306;
/// Rows inserted per `INSERT` statement by `uploadTableWithIndexes`.
pub const INSERT_BATCH_SIZE: usize = 1000;
/// MySQL's identifier length limit.
pub const MAX_TABLE_NAME_LENGTH: usize = 64;

/// Configuration of [`MySqlDriver`] (`MySqlDriverConfiguration`).
#[derive(Debug, Clone)]
pub struct MySqlConfig {
    /// Connection settings and global driver knobs.
    pub driver: DriverConfig,
    /// `readOnly` (default `true`).
    pub read_only: bool,
    /// `storeTimezone` (`SET time_zone`, default `+00:00`).
    pub store_timezone: String,
    /// `loadPreAggregationWithoutMetaLock`.
    pub load_pre_aggregation_without_meta_lock: bool,
    /// Overrides `CUBEJS_DB_MAX_POOL` (default 8).
    pub max_pool_size: Option<usize>,
    /// `acquireTimeoutMillis` (default 20 s).
    pub acquire_timeout: Duration,
    /// `idleTimeoutMillis` (default 30 s).
    pub idle_timeout: Duration,
}

impl MySqlConfig {
    /// Builds the MySQL configuration from a generic [`DriverConfig`].
    pub fn from_driver_config(driver: DriverConfig) -> Self {
        Self {
            driver,
            read_only: true,
            store_timezone: "+00:00".to_string(),
            load_pre_aggregation_without_meta_lock: false,
            max_pool_size: None,
            acquire_timeout: Duration::from_millis(20_000),
            idle_timeout: Duration::from_millis(30_000),
        }
    }

    /// Reads the configuration of `data_source` from the environment.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Ok(Self::from_driver_config(DriverConfig::from_env(
            data_source,
        )?))
    }

    /// Reads the configuration from a connection URL
    /// (`mysql://user:pass@host:port/db`), for tests and tooling.
    ///
    /// The individual fields are filled in as well, because `informationSchemaQuery`
    /// and the pool name are built from them rather than from the URL. A URL
    /// that does not parse is kept as-is and reported by [`MySqlConfig::opts`].
    pub fn from_url(url: &str) -> Self {
        let mut driver = DriverConfig::default();
        driver.data_source.url = Some(url.to_string());
        if let Ok(opts) = Opts::from_url(url) {
            driver.data_source.host = Some(opts.ip_or_hostname().to_string());
            driver.data_source.port = Some(opts.tcp_port());
            driver.data_source.user = opts.user().map(|u| u.to_string());
            driver.data_source.password = opts.pass().map(|p| p.to_string());
            driver.data_source.database = opts.db_name().map(|d| d.to_string());
        }
        Self::from_driver_config(driver)
    }

    /// Effective pool size.
    pub fn pool_size(&self) -> usize {
        self.max_pool_size
            .filter(|v| *v > 0)
            .unwrap_or_else(|| self.driver.data_source.effective_max_pool_size())
    }

    /// Name of the pool (`mysql#<data source>[@preAggregations]`).
    pub fn pool_name(&self) -> String {
        create_pool_name(
            "mysql",
            &self.driver.data_source.data_source,
            self.driver.data_source.pre_aggregations,
        )
    }

    /// The statement run on every new connection (`setTimeZone`).
    ///
    /// The Node driver issues it before each query; running it once per
    /// connection is equivalent — the session setting survives for the life of
    /// the connection — and saves a round trip per query.
    pub fn init_statement(&self) -> String {
        format!(
            "SET time_zone = '{}'",
            self.store_timezone.replace('\'', "''")
        )
    }

    fn ssl_opts(&self) -> Option<SslOpts> {
        let ssl = self.driver.data_source.ssl.as_ref()?;
        let mut opts = SslOpts::default()
            .with_danger_accept_invalid_certs(!ssl.reject_unauthorized)
            .with_danger_skip_domain_validation(!ssl.reject_unauthorized);
        if let Some(ca) = &ssl.ca {
            opts = opts.with_root_certs(vec![ca.clone().into_bytes().into()]);
        }
        Some(opts)
    }

    /// Translates the Cube configuration into `mysql_async` options.
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
        if let Some(socket_path) = &ds.socket_path {
            builder = builder
                .socket(Some(socket_path.clone()))
                .prefer_socket(true);
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
        if let Some(ssl) = self.ssl_opts() {
            builder = builder.ssl_opts(ssl);
        }

        let max = self.pool_size().max(1);
        let min = ds.min_pool_size.unwrap_or(0).min(max);
        let constraints = PoolConstraints::new(min, max).ok_or_else(|| {
            DriverError::Config(format!("Invalid pool constraints: min {min} > max {max}"))
        })?;
        builder = builder
            .pool_opts(
                PoolOpts::default()
                    .with_constraints(constraints)
                    .with_inactive_connection_ttl(self.idle_timeout),
            )
            .init(vec![self.init_statement()]);

        Ok(Opts::from(builder))
    }
}

/// MySQL driver.
pub struct MySqlDriver {
    config: MySqlConfig,
    opts: Opts,
    pool: Pool,
    pool_name: String,
}

impl std::fmt::Debug for MySqlDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MySqlDriver")
            .field("pool_name", &self.pool_name)
            .field("read_only", &self.config.read_only)
            .finish()
    }
}

impl MySqlDriver {
    /// Default per-driver concurrency (`getDefaultConcurrency`).
    pub const DEFAULT_CONCURRENCY: usize = 2;

    /// Creates the driver and its (lazy) connection pool.
    pub fn new(config: MySqlConfig) -> Result<Self> {
        let opts = config.opts()?;
        let pool_name = config.pool_name();
        Ok(Self {
            pool: Pool::new(opts.clone()),
            config,
            opts,
            pool_name,
        })
    }

    /// The driver configuration.
    pub fn mysql_config(&self) -> &MySqlConfig {
        &self.config
    }

    /// Name of the pool (`mysql#default`).
    pub fn pool_name(&self) -> &str {
        &self.pool_name
    }

    fn database(&self) -> &str {
        self.config
            .driver
            .data_source
            .database
            .as_deref()
            .unwrap_or("")
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

    /// Opens a dedicated (non-pooled) connection, as `testConnection` and
    /// `stream` do in the Node driver.
    async fn connect_standalone(&self) -> Result<Conn> {
        let connect = Conn::new(self.opts.clone());
        tokio::time::timeout(self.config.driver.test_connection_timeout, connect)
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

    /// `mapFieldsToGenericTypes`.
    fn map_fields(&self, columns: &[mysql_async::Column]) -> Vec<Column> {
        columns
            .iter()
            .map(|c| {
                let db_type = native_to_mysql_type(c.column_type());
                Column::new(
                    c.name_str().to_string(),
                    self.to_generic_type(db_type, None, None),
                )
            })
            .collect()
    }

    /// Runs `sql` with the values already interpolated.
    async fn query_response(&self, sql: &str, params: &[Value]) -> Result<QueryResult> {
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
        let columns = self.map_fields(&mysql_columns);

        let rows: Vec<Row> = result
            .map(|row| convert_row(&mysql_columns, row))
            .await
            .map_err(|e| self.mysql_error(e))?;

        Ok(QueryResult::new(columns, rows))
    }
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

#[async_trait]
impl Driver for MySqlDriver {
    fn config(&self) -> &DriverConfig {
        &self.config.driver
    }

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
        self.query_response(sql, params).await
    }

    fn quote_identifier(&self, identifier: &str) -> String {
        format!("`{identifier}`")
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
        generic_to_mysql(generic)
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

    fn information_schema_query(&self) -> String {
        format!(
            "{} AND columns.table_schema = '{}'",
            crate::sql::information_schema_query(&|i| self.quote_identifier(i)),
            self.database()
        )
    }

    fn primary_keys_query(&self, condition: Option<&str>) -> Option<String> {
        let q = |i: &str| self.quote_identifier(i);
        let cond = condition.map(|c| format!(" AND ({c})")).unwrap_or_default();
        Some(format!(
            "SELECT
      TABLE_SCHEMA as {},
      TABLE_NAME as {},
      COLUMN_NAME as {}
  FROM
      information_schema.KEY_COLUMN_USAGE
  WHERE
      CONSTRAINT_NAME = 'PRIMARY'
      AND TABLE_SCHEMA NOT IN ('information_schema', 'mysql', 'performance_schema', 'sys')
      {cond}
  ORDER BY
      TABLE_SCHEMA,
      TABLE_NAME,
      ORDINAL_POSITION;",
            q("table_schema"),
            q("table_name"),
            q("column_name"),
        ))
    }

    /// Foreign keys of the *referencing* tables.
    ///
    /// Diverges from `MySqlDriver.ts:216` on purpose, which had three faults:
    /// it joined `key_column_usage` to itself, so `target_table` repeated the
    /// referencing table instead of the referenced one; it filtered
    /// `columns.table_name` against a list of *schema* names, which never
    /// matches; and the alias `columns` pointed at the referenced side while
    /// `get_columns_for_specific_tables` builds its condition from
    /// `columns.table_name`, so the filtered call returned nothing. MySQL
    /// records the referenced side on the same row, so no self-join is needed.
    fn foreign_keys_query(&self, condition: Option<&str>) -> Option<String> {
        let q = |i: &str| self.quote_identifier(i);
        let cond = condition.map(|c| format!(" AND ({c})")).unwrap_or_default();
        Some(format!(
            "SELECT
        columns.table_schema as {},
        columns.table_name as {},
        columns.column_name as {},
        columns.referenced_table_name as {},
        columns.referenced_column_name as {}
    FROM
        information_schema.key_column_usage AS columns
    WHERE
        columns.referenced_table_name IS NOT NULL
        AND columns.table_schema NOT IN ('information_schema', 'mysql', 'performance_schema', 'sys'){cond};",
            q("table_schema"),
            q("table_name"),
            q("column_name"),
            q("target_table"),
            q("target_column"),
        ))
    }

    fn to_column_value(&self, value: &Value, generic_type: &GenericType) -> Value {
        match (generic_type, value) {
            (GenericType::Timestamp, Value::String(s)) => Value::String(s.replacen('Z', "", 1)),
            (GenericType::Boolean, Value::String(s)) => match s.to_lowercase().as_str() {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                _ => value.clone(),
            },
            _ => value.clone(),
        }
    }

    async fn release(&self) -> Result<()> {
        // `Pool::disconnect` consumes the pool, so a clone is drained instead;
        // it shares the same inner pool, which makes this idempotent.
        self.pool
            .clone()
            .disconnect()
            .await
            .map_err(|e| self.mysql_error(e))
    }

    async fn create_table(&self, quoted_table_name: &str, columns: &[Column]) -> Result<()> {
        if quoted_table_name.len() > MAX_TABLE_NAME_LENGTH {
            return Err(DriverError::Query(format!(
                "MySQL can not work with table names longer than 64 symbols. \
                 Consider using the 'sqlAlias' attribute in your cube definition for {quoted_table_name}."
            )));
        }
        let create_sql = self.create_table_sql(quoted_table_name, columns);
        self.query_response(&create_sql, &[]).await.map_err(|e| {
            DriverError::Query(format!("Error during create table: {create_sql}: {e}"))
        })?;
        Ok(())
    }

    async fn table_column_types(&self, table: &str) -> Result<TableStructure> {
        self.table_column_types_with_precision(table).await
    }

    async fn load_pre_aggregation_into_table(
        &self,
        _pre_aggregation_table_name: &str,
        load_sql: &str,
        params: &[Value],
        options: &QueryOptions,
    ) -> Result<QueryResult> {
        if self.config.load_pre_aggregation_without_meta_lock {
            self.query(&format!("{load_sql} LIMIT 0"), params, options)
                .await?;
            // `loadSql.replace(/^CREATE TABLE (\S+) AS/i, 'INSERT INTO $1')`
            let insert_sql = replace_create_table_with_insert(load_sql);
            return self.query(&insert_sql, params, options).await;
        }
        self.query(load_sql, params, options).await
    }

    async fn stream(
        &self,
        sql: &str,
        params: &[Value],
        options: &StreamOptions,
    ) -> Result<StreamTableData> {
        let formatted = format_mysql(sql, params);
        let mut conn = self.connect_standalone().await?;

        // The columns are only known once the result set header has arrived, so
        // the reader task hands them back before streaming the rows.
        let (columns_tx, columns_rx) = tokio::sync::oneshot::channel();
        let (rows_tx, rows_rx) =
            tokio::sync::mpsc::channel::<Result<Row>>(options.high_water_mark.max(1));
        let pool_name = self.pool_name.clone();
        let precise_decimal = self.config.driver.precise_decimal_in_cubestore;

        tokio::spawn(async move {
            let mut result = match conn.query_iter(formatted).await {
                Ok(r) => r,
                Err(e) => {
                    let _ = columns_tx.send(Err(DriverError::Connection {
                        pool_name,
                        message: e.to_string(),
                    }));
                    let _ = conn.disconnect().await;
                    return;
                }
            };

            let mysql_columns = result.columns().map(|c| c.to_vec()).unwrap_or_default();
            let columns: Vec<Column> = mysql_columns
                .iter()
                .map(|c| {
                    Column::new(
                        c.name_str().to_string(),
                        types::to_generic_type(
                            native_to_mysql_type(c.column_type()),
                            None,
                            None,
                            precise_decimal,
                        ),
                    )
                })
                .collect();
            if columns_tx.send(Ok(columns)).is_err() {
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
            DriverError::Query("MySQL stream ended before the result set header".to_string())
        })??;

        Ok(StreamTableData {
            columns,
            rows: tokio_stream_to_boxed(rows_rx),
        })
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

    async fn upload_table_with_indexes(
        &self,
        table: &str,
        columns: &[Column],
        table_data: &TableMemoryData,
        indexes_sql: &[IndexSql],
        _unique_key_columns: &[String],
        _external_options: &ExternalCreateTableOptions,
    ) -> Result<()> {
        self.create_table(table, columns).await?;

        let quoted_columns = columns
            .iter()
            .map(|c| self.quote_identifier(&c.name))
            .collect::<Vec<_>>()
            .join(", ");

        let upload = async {
            for chunk in table_data.rows.chunks(INSERT_BATCH_SIZE) {
                let placeholders = (0..chunk.len())
                    .map(|i| {
                        format!(
                            "({})",
                            columns
                                .iter()
                                .enumerate()
                                .map(|(p, _)| self.param(p + i * columns.len()))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let mut params: Vec<Value> = Vec::with_capacity(chunk.len() * columns.len());
                for row in chunk {
                    for c in columns {
                        let value = table_data
                            .column_index(&c.name)
                            .and_then(|idx| row.get(idx).cloned())
                            .unwrap_or(Value::Null);
                        params.push(self.to_column_value(&value, &c.type_));
                    }
                }

                self.query_response(
                    &format!(
                        "INSERT INTO {table}
        ({quoted_columns})
        VALUES {placeholders}"
                    ),
                    &params,
                )
                .await?;
            }

            for index in indexes_sql {
                self.query_response(&index.sql, &index.params).await?;
            }
            Ok::<(), DriverError>(())
        };

        if let Err(e) = upload.await {
            if let Err(drop_err) = self.drop_table(table, &QueryOptions::default()).await {
                log::warn!("Unable to drop table {table} after failed upload: {drop_err}");
            }
            return Err(e);
        }
        Ok(())
    }
}

fn tokio_stream_to_boxed(
    rx: tokio::sync::mpsc::Receiver<Result<Row>>,
) -> futures::stream::BoxStream<'static, Result<Row>> {
    futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    })
    .boxed()
}

/// `loadSql.replace(/^CREATE TABLE (\S+) AS/i, 'INSERT INTO $1')`.
pub fn replace_create_table_with_insert(load_sql: &str) -> String {
    const PREFIX: &str = "CREATE TABLE ";
    if load_sql.len() < PREFIX.len() || !load_sql[..PREFIX.len()].eq_ignore_ascii_case(PREFIX) {
        return load_sql.to_string();
    }
    let rest = &load_sql[PREFIX.len()..];
    // `(\S+) AS`: the table name is the first run of non-whitespace.
    let name_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let (name, tail) = rest.split_at(name_end);
    let Some(after_space) = tail.strip_prefix(' ') else {
        return load_sql.to_string();
    };
    if after_space.len() < 2 || !after_space[..2].eq_ignore_ascii_case("AS") {
        return load_sql.to_string();
    }
    format!("INSERT INTO {name}{}", &after_space[2..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn driver() -> MySqlDriver {
        let mut config = DriverConfig::default();
        config.data_source.database = Some("test".into());
        MySqlDriver::new(MySqlConfig::from_driver_config(config)).unwrap()
    }

    #[test]
    fn pool_name_and_defaults() {
        let d = driver();
        assert_eq!(d.pool_name(), "mysql#default");
        assert!(d.read_only());
        assert!(d.capabilities().incremental_schema_loading);
        assert_eq!(d.param(0), "?");
        assert_eq!(d.quote_identifier("a"), "`a`");
        assert_eq!(
            d.mysql_config().init_statement(),
            "SET time_zone = '+00:00'"
        );
    }

    #[test]
    fn opts_from_parts_and_url() {
        let mut config = DriverConfig::default();
        config.data_source.host = Some("db".into());
        config.data_source.port = Some(3307);
        config.data_source.user = Some("u".into());
        config.data_source.password = Some("p".into());
        config.data_source.database = Some("d".into());
        let opts = MySqlConfig::from_driver_config(config).opts().unwrap();
        assert_eq!(opts.ip_or_hostname(), "db");
        assert_eq!(opts.tcp_port(), 3307);
        assert_eq!(opts.user(), Some("u"));
        assert_eq!(opts.db_name(), Some("d"));
        assert!(opts.ssl_opts().is_none());
        assert_eq!(opts.init(), &["SET time_zone = '+00:00'".to_string()]);

        let config = MySqlConfig::from_url("mysql://a:b@h:1234/db");
        // the fields are filled in too, so `informationSchemaQuery` works
        assert_eq!(config.driver.data_source.database.as_deref(), Some("db"));
        assert_eq!(config.driver.data_source.host.as_deref(), Some("h"));
        let opts = config.opts().unwrap();
        assert_eq!(opts.ip_or_hostname(), "h");
        assert_eq!(opts.tcp_port(), 1234);
        assert_eq!(opts.user(), Some("a"));
        assert_eq!(opts.db_name(), Some("db"));

        let err = MySqlConfig::from_url("not-a-url").opts().unwrap_err();
        assert!(err.to_string().contains("Invalid CUBEJS_DB_URL"));
    }

    #[test]
    fn ssl_options_follow_reject_unauthorized() {
        use crate::config::SslConfig;
        let mut config = DriverConfig::default();
        config.data_source.ssl = Some(SslConfig::default());
        let opts = MySqlConfig::from_driver_config(config).opts().unwrap();
        let ssl = opts.ssl_opts().expect("ssl enabled");
        assert!(ssl.accept_invalid_certs());

        let mut config = DriverConfig::default();
        config.data_source.ssl = Some(SslConfig {
            reject_unauthorized: true,
            ..Default::default()
        });
        let opts = MySqlConfig::from_driver_config(config).opts().unwrap();
        assert!(!opts.ssl_opts().unwrap().accept_invalid_certs());
    }

    #[test]
    fn sql_contract() {
        let d = driver();
        let q = d.information_schema_query();
        assert!(q.contains("FROM information_schema.columns"));
        assert!(q.ends_with(" AND columns.table_schema = 'test'"));

        let pk = d.primary_keys_query(None).unwrap();
        assert!(pk.contains("information_schema.KEY_COLUMN_USAGE"));
        assert!(pk.contains("CONSTRAINT_NAME = 'PRIMARY'"));
        assert!(pk.contains("TABLE_SCHEMA as `table_schema`"));
        assert!(pk.trim_end().ends_with("ORDINAL_POSITION;"));
        assert!(d
            .primary_keys_query(Some("x = 1"))
            .unwrap()
            .contains(" AND (x = 1)"));

        let fk = d.foreign_keys_query(None).unwrap();
        // The referencing table carries the `columns` alias, so a condition
        // built from `columns.table_name` selects the tables asked about.
        assert!(fk.contains("information_schema.key_column_usage AS columns"));
        assert!(fk.contains("columns.table_name as `table_name`"));
        // The referenced side comes from the row itself, with no self-join.
        assert!(fk.contains("columns.referenced_table_name as `target_table`"));
        assert!(fk.contains("columns.referenced_column_name as `target_column`"));
        assert!(fk.contains("columns.referenced_table_name IS NOT NULL"));
        // The system filter compares schema names against schema names.
        assert!(fk.contains("columns.table_schema NOT IN ('information_schema'"));
        assert!(d
            .foreign_keys_query(Some("x = 1"))
            .unwrap()
            .contains("'sys') AND (x = 1);"));

        assert_eq!(
            d.create_table_sql("t", &[Column::new("a", "int"), Column::new("b", "string")]),
            "CREATE TABLE t (`a` int, `b` varchar(255) CHARACTER SET utf8mb4)"
        );
    }

    #[test]
    fn column_values_are_coerced() {
        let d = driver();
        assert_eq!(
            d.to_column_value(
                &Value::from("2020-01-01T00:00:00.000Z"),
                &GenericType::Timestamp
            ),
            Value::from("2020-01-01T00:00:00.000")
        );
        assert_eq!(
            d.to_column_value(&Value::from("TRUE"), &GenericType::Boolean),
            Value::Bool(true)
        );
        assert_eq!(
            d.to_column_value(&Value::from("nope"), &GenericType::Boolean),
            Value::from("nope")
        );
    }

    #[tokio::test]
    async fn create_table_rejects_long_names() {
        let d = driver();
        let name = "a".repeat(65);
        let err = d
            .create_table(&name, &[Column::new("id", "bigint")])
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "MySQL can not work with table names longer than 64 symbols. Consider using the 'sqlAlias' attribute in your cube definition for {name}."
            )
        );
        d.release().await.unwrap();
    }

    #[test]
    fn create_table_to_insert_rewrite() {
        assert_eq!(
            replace_create_table_with_insert("CREATE TABLE stb.t AS SELECT 1"),
            "INSERT INTO stb.t SELECT 1"
        );
        assert_eq!(
            replace_create_table_with_insert("create table stb.t as SELECT 1"),
            "INSERT INTO stb.t SELECT 1"
        );
        // not a CREATE TABLE ... AS: left alone
        assert_eq!(replace_create_table_with_insert("SELECT 1"), "SELECT 1");
        assert_eq!(
            replace_create_table_with_insert("CREATE TABLE t (a int)"),
            "CREATE TABLE t (a int)"
        );
    }
}
