//! PostgreSQL driver: port of `@cubejs-backend/postgres-driver` on top of
//! `tokio-postgres`, `deadpool-postgres` and `rustls`.

mod decode;
mod numeric;
mod params;
mod tls;

use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use deadpool_postgres::{
    Hook, HookError, Manager, ManagerConfig, Object, Pool, PoolError, RecyclingMethod, Runtime,
    Timeouts,
};
use futures::{StreamExt, TryStreamExt};
use serde_json::Value;
use tokio_postgres::types::{Kind, Type};
use tokio_postgres::{NoTls, Statement};

use crate::config::DriverConfig;
use crate::driver::Driver;
use crate::error::{DriverError, Result};
use crate::types::{
    Column, DownloadQueryResultsOptions, DownloadedData, DriverCapabilities,
    ExternalCreateTableOptions, GenericType, IndexSql, QueryOptions, QueryResult, Row,
    StreamOptions, StreamTableData, TableMemoryData, TableStructure,
};

pub use decode::{format_date, format_timestamp, JsonCell};
pub use params::{check_values_limit, prepare_value, TextParam};
pub use tls::{client_config, CubeTls};

/// How a new pooled connection is prepared (`prepareConnection`), for the
/// PostgreSQL-derived databases that override it (CrateDB, Materialize,
/// QuestDB). The default is PostgreSQL's own: `SET TIME ZONE` plus
/// `SET statement_timeout`, with `application_name = cubejs`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConnectionSetup {
    /// Statements run on every new connection instead of the PostgreSQL ones.
    /// `Some(vec![])` runs nothing.
    pub statements: Option<Vec<String>>,
    /// `application_name` sent at start-up instead of `cubejs`.
    pub application_name: Option<String>,
}

/// Default `executionTimeout` when `CUBEJS_DB_QUERY_TIMEOUT` is unset: 10 minutes.
pub const DEFAULT_EXECUTION_TIMEOUT: Duration = Duration::from_millis(600_000);

/// Port of `createPoolName`.
pub fn create_pool_name(driver_name: &str, data_source: &str, pre_aggregations: bool) -> String {
    if pre_aggregations {
        format!("{driver_name}#{data_source}@preAggregations")
    } else {
        format!("{driver_name}#{data_source}")
    }
}

/// Configuration of [`PostgresDriver`] (`PostgresDriverConfiguration`).
#[derive(Debug, Clone)]
pub struct PostgresConfig {
    /// Connection settings and global driver knobs.
    pub driver: DriverConfig,
    /// `readOnly` (default `true`, see `getInitialConfiguration`).
    pub read_only: bool,
    /// `storeTimezone` (`SET TIME ZONE` on every connection, default `UTC`).
    pub store_timezone: String,
    /// `executionTimeout` → `SET statement_timeout` (from `CUBEJS_DB_QUERY_TIMEOUT`).
    pub execution_timeout: Duration,
    /// Overrides `CUBEJS_DB_MAX_POOL` (default 8).
    pub max_pool_size: Option<usize>,
    /// `acquireTimeoutMillis` (default 20 s).
    pub acquire_timeout: Duration,
    /// `idleTimeoutMillis` (default 30 s).
    pub idle_timeout: Duration,
    /// `evictionRunIntervalMillis` (default 10 s).
    pub eviction_run_interval: Duration,
}

impl PostgresConfig {
    /// Builds the Postgres configuration from a generic [`DriverConfig`].
    pub fn from_driver_config(driver: DriverConfig) -> Self {
        let execution_timeout = driver.data_source.query_timeout;
        Self {
            driver,
            read_only: true,
            store_timezone: "UTC".to_string(),
            execution_timeout,
            max_pool_size: None,
            acquire_timeout: Duration::from_millis(20_000),
            idle_timeout: Duration::from_millis(30_000),
            eviction_run_interval: Duration::from_millis(10_000),
        }
    }

    /// Reads the configuration of `data_source` from the environment.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Ok(Self::from_driver_config(DriverConfig::from_env(
            data_source,
        )?))
    }

    /// Reads the configuration from a connection URL
    /// (`postgres://user:pass@host:port/db`), for tests and tooling.
    pub fn from_url(url: &str) -> Self {
        let mut driver = DriverConfig::default();
        driver.data_source.url = Some(url.to_string());
        Self::from_driver_config(driver)
    }

    /// Effective pool size.
    pub fn pool_size(&self) -> usize {
        self.max_pool_size
            .filter(|v| *v > 0)
            .unwrap_or_else(|| self.driver.data_source.effective_max_pool_size())
    }

    /// Name of the pool (`postgres#<data source>[@preAggregations]`).
    pub fn pool_name(&self) -> String {
        create_pool_name(
            "postgres",
            &self.driver.data_source.data_source,
            self.driver.data_source.pre_aggregations,
        )
    }

    /// Translates the Cube configuration into a `tokio_postgres::Config`.
    pub fn pg_config(&self) -> Result<tokio_postgres::Config> {
        let ds = &self.driver.data_source;
        let mut cfg = match &ds.url {
            Some(url) => tokio_postgres::Config::from_str(url)
                .map_err(|e| DriverError::Config(format!("Invalid CUBEJS_DB_URL: {e}")))?,
            None => tokio_postgres::Config::new(),
        };

        if let Some(host) = ds.socket_path.as_deref().or(ds.host.as_deref()) {
            cfg.host(host);
        } else if cfg.get_hosts().is_empty() {
            cfg.host("localhost");
        }
        if let Some(port) = ds.port {
            cfg.port(port);
        } else if cfg.get_ports().is_empty() {
            cfg.port(5432);
        }
        if let Some(user) = &ds.user {
            cfg.user(user);
        } else if cfg.get_user().is_none() {
            if let Ok(user) = std::env::var("USER") {
                cfg.user(user);
            }
        }
        if let Some(password) = &ds.password {
            cfg.password(password);
        }
        if let Some(database) = &ds.database {
            cfg.dbname(database);
        }
        cfg.ssl_mode(if ds.ssl.is_some() {
            tokio_postgres::config::SslMode::Require
        } else {
            tokio_postgres::config::SslMode::Disable
        });
        cfg.application_name("cubejs");
        Ok(cfg)
    }
}

/// PostgreSQL driver.
pub struct PostgresDriver {
    config: PostgresConfig,
    pg_config: tokio_postgres::Config,
    pool: Pool,
    pool_name: String,
    tls: Option<CubeTls>,
    eviction_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl std::fmt::Debug for PostgresDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostgresDriver")
            .field("pool_name", &self.pool_name)
            .field("read_only", &self.config.read_only)
            .finish()
    }
}

impl PostgresDriver {
    /// Default per-driver concurrency (`getDefaultConcurrency`).
    pub const DEFAULT_CONCURRENCY: usize = 2;

    /// Creates the driver and its (lazy) connection pool. No connection is
    /// opened until the first query.
    pub fn new(config: PostgresConfig) -> Result<Self> {
        Self::new_with_setup(config, ConnectionSetup::default())
    }

    /// Like [`PostgresDriver::new`], with the connection preparation of a
    /// PostgreSQL-derived database (see [`ConnectionSetup`]).
    pub fn new_with_setup(config: PostgresConfig, setup: ConnectionSetup) -> Result<Self> {
        let mut pg_config = config.pg_config()?;
        if let Some(name) = &setup.application_name {
            pg_config.application_name(name);
        }
        let pool_name = config.pool_name();
        let tls = config
            .driver
            .data_source
            .ssl
            .as_ref()
            .map(CubeTls::new)
            .transpose()?;

        let manager_config = ManagerConfig {
            recycling_method: RecyclingMethod::Fast,
        };
        let manager = match &tls {
            Some(tls) => Manager::from_config(pg_config.clone(), tls.clone(), manager_config),
            None => Manager::from_config(pg_config.clone(), NoTls, manager_config),
        };

        let timezone = config.store_timezone.clone();
        let statement_timeout_ms = config.execution_timeout.as_millis() as u64;
        let setup_statements = setup.statements.clone();
        let pool = Pool::builder(manager)
            .max_size(config.pool_size())
            .timeouts(Timeouts {
                wait: Some(config.acquire_timeout),
                create: Some(config.acquire_timeout),
                recycle: Some(config.acquire_timeout),
            })
            .runtime(Runtime::Tokio1)
            .post_create(Hook::async_fn(move |client, _| {
                let timezone = timezone.clone();
                let setup_statements = setup_statements.clone();
                Box::pin(async move {
                    match setup_statements {
                        Some(statements) => {
                            for statement in &statements {
                                client
                                    .batch_execute(statement)
                                    .await
                                    .map_err(HookError::Backend)?;
                            }
                            Ok(())
                        }
                        None => prepare_connection(client, &timezone, statement_timeout_ms)
                            .await
                            .map_err(HookError::Backend),
                    }
                })
            }))
            .build()
            .map_err(|e| {
                DriverError::Config(format!("Unable to build the connection pool: {e}"))
            })?;

        let eviction_task = tokio::runtime::Handle::try_current().ok().map(|handle| {
            let pool = pool.clone();
            let interval = config.eviction_run_interval;
            let idle = config.idle_timeout;
            handle.spawn(async move {
                loop {
                    tokio::time::sleep(interval).await;
                    if pool.is_closed() {
                        break;
                    }
                    pool.retain(|_, metrics| metrics.last_used() < idle);
                }
            })
        });

        Ok(Self {
            config,
            pg_config,
            pool,
            pool_name,
            tls,
            eviction_task: Mutex::new(eviction_task),
        })
    }

    /// The driver configuration.
    pub fn postgres_config(&self) -> &PostgresConfig {
        &self.config
    }

    /// Name of the pool (`postgres#default`).
    pub fn pool_name(&self) -> &str {
        &self.pool_name
    }

    /// Number of open connections (busy + idle).
    pub fn pool_size(&self) -> usize {
        self.pool.status().size
    }

    /// Acquires a pooled connection and returns it immediately.
    ///
    /// Used by the Redshift driver, whose `testConnection` only checks that a
    /// connection can be established (querying even system tables is billed).
    pub async fn check_pool_connection(&self) -> Result<()> {
        let connection = self.acquire().await?;
        drop(connection);
        Ok(())
    }

    /// A pooled connection, returned to the pool when dropped. For the
    /// PostgreSQL-derived drivers that need their own statement handling
    /// (Materialize cursors, QuestDB's type mapping).
    pub async fn pooled_client(&self) -> Result<Object> {
        self.acquire().await
    }

    async fn acquire(&self) -> Result<Object> {
        self.pool.get().await.map_err(|e| self.pool_error(e))
    }

    fn pool_error(&self, e: PoolError) -> DriverError {
        match e {
            PoolError::Timeout(_) => DriverError::PoolTimeout(self.pool_name.clone()),
            PoolError::Backend(e) => self.connection_error(e),
            PoolError::PostCreateHook(HookError::Backend(e)) => self.connection_error(e),
            PoolError::PostCreateHook(HookError::Message(m)) => DriverError::Connection {
                pool_name: self.pool_name.clone(),
                message: m.to_string(),
            },
            PoolError::Closed => DriverError::Query(format!("Pool {} is closed", self.pool_name)),
            PoolError::NoRuntimeSpecified => {
                DriverError::Other("No async runtime available for the pool".to_string())
            }
        }
    }

    fn connection_error(&self, e: tokio_postgres::Error) -> DriverError {
        DriverError::Connection {
            pool_name: self.pool_name.clone(),
            message: match e.as_db_error() {
                Some(db) => db.message().to_string(),
                None => e.to_string(),
            },
        }
    }

    /// Opens a dedicated (non-pooled) connection, prepared like pooled ones.
    async fn connect_standalone(&self) -> Result<tokio_postgres::Client> {
        let connect = async {
            let client = match &self.tls {
                Some(tls) => {
                    let (client, connection) = self.pg_config.connect(tls.clone()).await?;
                    tokio::spawn(async move {
                        if let Err(e) = connection.await {
                            log::debug!("postgres connection closed: {e}");
                        }
                    });
                    client
                }
                None => {
                    let (client, connection) = self.pg_config.connect(NoTls).await?;
                    tokio::spawn(async move {
                        if let Err(e) = connection.await {
                            log::debug!("postgres connection closed: {e}");
                        }
                    });
                    client
                }
            };
            Ok::<_, tokio_postgres::Error>(client)
        };
        let client = tokio::time::timeout(self.config.driver.test_connection_timeout, connect)
            .await
            .map_err(|_| DriverError::Connection {
                pool_name: self.pool_name.clone(),
                message: format!(
                    "connection timed out after {:?}",
                    self.config.driver.test_connection_timeout
                ),
            })?
            .map_err(|e| self.connection_error(e))?;
        Ok(client)
    }

    /// `mapFields`: column metadata of a prepared statement.
    fn map_fields(&self, statement: &Statement) -> Result<Vec<Column>> {
        statement
            .columns()
            .iter()
            .map(|c| {
                let pg_type = postgres_type_name(c.type_());
                Ok(Column::new(
                    c.name(),
                    self.to_generic_type(&pg_type, None, None),
                ))
            })
            .collect()
    }

    fn convert_row(row: &tokio_postgres::Row, width: usize) -> Result<Row> {
        (0..width)
            .map(|i| {
                row.try_get::<usize, JsonCell>(i)
                    .map(|c| c.0)
                    .map_err(|e| DriverError::TypeDetection(e.to_string()))
            })
            .collect()
    }

    async fn query_response(&self, sql: &str, params: &[Value]) -> Result<QueryResult> {
        check_values_limit(params)?;
        let client = self.acquire().await?;
        let statement = client.prepare(sql).await?;
        let columns = self.map_fields(&statement)?;
        let width = columns.len();
        let text_params = params::to_text_params(params);
        let rows = client
            .query_raw(&statement, text_params.iter())
            .await?
            .map_err(DriverError::from)
            .and_then(|row| futures::future::ready(Self::convert_row(&row, width)))
            .try_collect::<Vec<Row>>()
            .await?;
        Ok(QueryResult::new(columns, rows))
    }
}

/// `prepareConnection`: session settings applied to every new connection.
async fn prepare_connection(
    client: &tokio_postgres::Client,
    timezone: &str,
    statement_timeout_ms: u64,
) -> std::result::Result<(), tokio_postgres::Error> {
    client
        .batch_execute(&format!(
            "SET TIME ZONE '{}'; SET statement_timeout TO {};",
            timezone.replace('\'', "''"),
            statement_timeout_ms
        ))
        .await
}

/// Port of `getPostgresTypeForField`: the PostgreSQL type name used for the
/// generic type mapping. Enums map to `varchar`, arrays to `text`.
pub fn postgres_type_name(ty: &Type) -> String {
    match ty.kind() {
        Kind::Enum(_) => "varchar".to_string(),
        Kind::Array(_) => "text".to_string(),
        Kind::Domain(inner) => postgres_type_name(inner),
        _ => ty.name().to_lowercase(),
    }
}

/// `PostgresToGenericType` + `GenericTypeToPostgres`.
fn postgres_to_generic(pg_type_lower: &str) -> Option<GenericType> {
    match pg_type_lower {
        // bpchar ("blank-padded char", the internal name of the character data type)
        "bpchar" => Some(GenericType::Other("varchar".to_string())),
        // External mapping
        "hll" => Some(GenericType::HllPostgres),
        _ => None,
    }
}

fn generic_to_postgres(generic: &GenericType) -> String {
    match generic {
        GenericType::String => "text".to_string(),
        GenericType::Double => "decimal".to_string(),
        GenericType::Int => "int8".to_string(),
        // Revert mapping for internal pre-aggregations
        GenericType::HllPostgres => "hll".to_string(),
        other => other.to_string(),
    }
}

#[async_trait]
impl Driver for PostgresDriver {
    fn config(&self) -> &DriverConfig {
        &self.config.driver
    }

    async fn test_connection(&self) -> Result<()> {
        let client = self.connect_standalone().await?;
        let result = client
            .query(
                "SELECT $1::int AS number",
                &[&TextParam(Some("1".to_string()))],
            )
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(e) => {
                let err = DriverError::from(e);
                let text = err.to_string();
                if text.contains("no pg_hba.conf entry for host") {
                    Err(DriverError::Query(format!(
                        "Please use CUBEJS_DB_SSL=true to connect: {text}"
                    )))
                } else {
                    Err(err)
                }
            }
        }
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
        format!("${}", index + 1)
    }

    fn to_generic_type(
        &self,
        db_type: &str,
        precision: Option<i64>,
        scale: Option<i64>,
    ) -> GenericType {
        postgres_to_generic(&db_type.to_lowercase()).unwrap_or_else(|| {
            crate::types::to_generic_type(
                db_type,
                precision,
                scale,
                self.config.driver.precise_decimal_in_cubestore,
            )
        })
    }

    #[allow(clippy::wrong_self_convention)]
    fn from_generic_type(&self, generic: &GenericType) -> String {
        generic_to_postgres(generic)
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

    async fn release(&self) -> Result<()> {
        if let Some(task) = self.eviction_task.lock().unwrap().take() {
            task.abort();
        }
        if !self.pool.is_closed() {
            self.pool.close();
        }
        Ok(())
    }

    fn primary_keys_query(&self, condition: Option<&str>) -> Option<String> {
        let q = |i: &str| self.quote_identifier(i);
        let cond = condition.map(|c| format!(" AND ({c})")).unwrap_or_default();
        Some(format!(
            "SELECT
      columns.table_schema as {},
      columns.table_name as {},
      columns.column_name as {}
    FROM information_schema.table_constraints tc
    JOIN information_schema.constraint_column_usage AS ccu USING (constraint_schema, constraint_name)
    JOIN information_schema.columns AS columns ON columns.table_schema = tc.constraint_schema
      AND tc.table_name = columns.table_name AND ccu.column_name = columns.column_name
    WHERE constraint_type = 'PRIMARY KEY' AND columns.table_schema NOT IN ({}){cond}",
            q("table_schema"),
            q("table_name"),
            q("column_name"),
            crate::sql::SYSTEM_SCHEMAS,
        ))
    }

    /// Foreign keys of the *referencing* tables.
    ///
    /// Diverges from `PostgresDriver.ts:201` on two counts, both deliberate.
    ///
    /// The joins carry the constraint's catalog and schema, not just its
    /// name. Postgres generates names like `orders_user_id_fkey` per schema,
    /// so two schemas holding a table of the same shape collide, and joining
    /// on the name alone returns their cross product: every column came back
    /// carrying every foreign key in the database.
    ///
    /// And There the referenced
    /// side (`constraint_column_usage`) carries the alias `columns`, which is
    /// also what `get_columns_for_specific_tables` builds its condition from,
    /// so asking for the columns of `orders` filtered on
    /// `target_table = orders` and silently returned no foreign keys — losing
    /// every join during incremental schema loading. Here `columns` is the
    /// referencing table, so the condition selects the tables the caller asked
    /// about, and the referenced side is aliased `target`.
    fn foreign_keys_query(&self, condition: Option<&str>) -> Option<String> {
        let q = |i: &str| self.quote_identifier(i);
        let cond = condition.map(|c| format!(" AND ({c})")).unwrap_or_default();
        Some(format!(
            "SELECT
        columns.table_schema as {},
        columns.table_name as {},
        kcu.column_name as {},
        target.table_name as {},
        target.column_name as {}
      FROM
        information_schema.table_constraints AS columns
      JOIN information_schema.key_column_usage AS kcu
        ON kcu.constraint_catalog = columns.constraint_catalog
       AND kcu.constraint_schema = columns.constraint_schema
       AND kcu.constraint_name = columns.constraint_name
      JOIN information_schema.constraint_column_usage AS target
        ON target.constraint_catalog = columns.constraint_catalog
       AND target.constraint_schema = columns.constraint_schema
       AND target.constraint_name = columns.constraint_name
      WHERE
         columns.constraint_type = 'FOREIGN KEY'
         AND {} NOT IN ({})
         {cond}
    ",
            q("table_schema"),
            q("table_name"),
            q("column_name"),
            q("target_table"),
            q("target_column"),
            self.column_name_for_schema_name(),
            crate::sql::SYSTEM_SCHEMAS,
        ))
    }

    async fn stream(
        &self,
        sql: &str,
        params: &[Value],
        _options: &StreamOptions,
    ) -> Result<StreamTableData> {
        check_values_limit(params)?;
        let client = self.acquire().await?;
        let statement = client.prepare(sql).await?;
        let columns = self.map_fields(&statement)?;
        let width = columns.len();
        let text_params = params::to_text_params(params);
        let row_stream = client.query_raw(&statement, text_params.iter()).await?;

        // The pooled connection travels with the stream and is returned to the
        // pool when the stream is dropped.
        let rows = futures::stream::try_unfold(
            (client, Box::pin(row_stream)),
            move |(client, mut row_stream)| async move {
                match row_stream.try_next().await {
                    Ok(Some(row)) => {
                        let converted = Self::convert_row(&row, width)?;
                        Ok(Some((converted, (client, row_stream))))
                    }
                    Ok(None) => Ok(None),
                    Err(e) => Err(DriverError::from(e)),
                }
            },
        )
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
            self.query_response(sql, params).await?,
        ))
    }

    async fn table_column_types(&self, table: &str) -> Result<TableStructure> {
        self.table_column_types_with_precision(table).await
    }

    async fn query_column_types(
        &self,
        sql: &str,
        params: &[Value],
        _options: &QueryOptions,
    ) -> Result<TableStructure> {
        check_values_limit(params)?;
        let client = self.acquire().await?;
        let statement = client.prepare(sql).await?;
        self.map_fields(&statement)
    }

    async fn create_table(&self, quoted_table_name: &str, columns: &[Column]) -> Result<()> {
        if quoted_table_name.len() > 63 {
            return Err(DriverError::Query(format!(
                "PostgreSQL can not work with table names longer than 63 symbols. \
                 Consider using the 'sqlAlias' attribute in your cube definition for {quoted_table_name}."
            )));
        }
        let create_sql = self.create_table_sql(quoted_table_name, columns);
        self.query_response(&create_sql, &[]).await.map_err(|e| {
            DriverError::Query(format!("Error during create table: {create_sql}: {e}"))
        })?;
        Ok(())
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

        if let Err(e) = self
            .upload_rows_into_existing_table(table, columns, table_data, indexes_sql)
            .await
        {
            if let Err(drop_err) = self.drop_table(table, &QueryOptions::default()).await {
                log::warn!("Unable to drop table {table} after failed upload: {drop_err}");
            }
            return Err(e);
        }
        Ok(())
    }
}

impl PostgresDriver {
    /// The `INSERT INTO … SELECT * FROM UNNEST (…)` upload of
    /// `uploadTableWithIndexes`, for an already created table.
    ///
    /// Split out so that the Redshift driver (which has its own `createTable`)
    /// can reuse it.
    pub async fn upload_rows_into_existing_table(
        &self,
        table: &str,
        columns: &[Column],
        table_data: &TableMemoryData,
        indexes_sql: &[IndexSql],
    ) -> Result<()> {
        {
            let insert = format!(
                "INSERT INTO {table}
      ({})
      SELECT * FROM UNNEST ({})",
                columns
                    .iter()
                    .map(|c| self.quote_identifier(&c.name))
                    .collect::<Vec<_>>()
                    .join(", "),
                columns
                    .iter()
                    .enumerate()
                    .map(|(i, c)| format!(
                        "{}::{}[]",
                        self.param(i),
                        self.from_generic_type(&c.type_)
                    ))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            let params: Vec<Value> = columns
                .iter()
                .map(|c| {
                    let idx = if table_data.columns.is_empty() {
                        columns.iter().position(|x| x.name == c.name)
                    } else {
                        table_data.column_index(&c.name)
                    };
                    Value::Array(
                        table_data
                            .rows
                            .iter()
                            .map(|row| idx.and_then(|i| row.get(i).cloned()).unwrap_or(Value::Null))
                            .collect(),
                    )
                })
                .collect();
            self.query_response(&insert, &params).await?;

            for index in indexes_sql {
                self.query_response(&index.sql, &index.params).await?;
            }
            Ok::<(), DriverError>(())
        }
    }
}

impl Drop for PostgresDriver {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.eviction_task.lock() {
            if let Some(task) = guard.take() {
                task.abort();
            }
        }
    }
}

// Keep the `Arc` import used for callers that want `Arc<dyn Driver>`.
#[allow(dead_code)]
type SharedDriver = Arc<dyn Driver>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_name() {
        assert_eq!(
            create_pool_name("postgres", "default", false),
            "postgres#default"
        );
        assert_eq!(
            create_pool_name("postgres", "default", true),
            "postgres#default@preAggregations"
        );
    }

    #[test]
    fn type_mapping() {
        assert_eq!(postgres_type_name(&Type::INT8), "int8");
        assert_eq!(postgres_type_name(&Type::TIMESTAMPTZ), "timestamptz");
        assert_eq!(postgres_type_name(&Type::INT4_ARRAY), "text");
        assert_eq!(postgres_type_name(&Type::BPCHAR), "bpchar");
        assert_eq!(
            postgres_to_generic("bpchar"),
            Some(GenericType::Other("varchar".into()))
        );
        assert_eq!(postgres_to_generic("hll"), Some(GenericType::HllPostgres));
        assert_eq!(generic_to_postgres(&GenericType::Int), "int8");
        assert_eq!(generic_to_postgres(&GenericType::Double), "decimal");
        assert_eq!(generic_to_postgres(&GenericType::String), "text");
        assert_eq!(generic_to_postgres(&GenericType::HllPostgres), "hll");
        assert_eq!(generic_to_postgres(&GenericType::Bigint), "bigint");
        assert_eq!(
            generic_to_postgres(&GenericType::Decimal(Some((10, 2)))),
            "decimal(10, 2)"
        );
    }

    #[tokio::test]
    async fn driver_type_mapping_and_sql() {
        let driver =
            PostgresDriver::new(PostgresConfig::from_driver_config(DriverConfig::default()))
                .unwrap();
        assert_eq!(driver.param(0), "$1");
        assert_eq!(driver.param(4), "$5");
        assert_eq!(
            driver.to_generic_type("int8", None, None),
            GenericType::Bigint
        );
        assert_eq!(
            driver.to_generic_type("timestamp", None, None),
            GenericType::Timestamp
        );
        assert_eq!(
            driver.to_generic_type("timestamptz", None, None),
            GenericType::Other("timestamptz".into())
        );
        assert_eq!(
            driver.to_generic_type("bpchar", None, None),
            GenericType::Other("varchar".into())
        );
        assert_eq!(
            driver.to_generic_type("numeric", Some(10), Some(2)),
            GenericType::Decimal(None)
        );
        assert!(driver.read_only());
        assert_eq!(
            driver.create_table_sql("t", &[Column::new("a", "int"), Column::new("b", "string")]),
            r#"CREATE TABLE t ("a" int8, "b" text)"#
        );
        assert!(driver
            .primary_keys_query(None)
            .unwrap()
            .contains("PRIMARY KEY"));
        assert!(driver
            .primary_keys_query(Some("x = 1"))
            .unwrap()
            .ends_with(" AND (x = 1)"));
        let fk = driver.foreign_keys_query(None).unwrap();
        assert!(fk.contains("columns.constraint_type = 'FOREIGN KEY'"));
        // The referencing table carries the `columns` alias, so a condition
        // built from `columns.table_name` selects the tables asked about; the
        // referenced side is `target`.
        assert!(fk.contains("information_schema.table_constraints AS columns"));
        assert!(fk.contains("columns.table_name as \"table_name\""));
        assert!(fk.contains("information_schema.constraint_column_usage AS target"));
        assert!(fk.contains("target.table_name as \"target_table\""));
        assert_eq!(driver.pool_name(), "postgres#default");
        driver.release().await.unwrap();
        // idempotent
        driver.release().await.unwrap();
    }

    #[tokio::test]
    async fn create_table_rejects_long_names() {
        let driver =
            PostgresDriver::new(PostgresConfig::from_driver_config(DriverConfig::default()))
                .unwrap();
        let name = "really-really-really-looooooooooooooooooooooooooooooooooooooooooooooooooooong-table-name";
        let err = driver
            .create_table(name, &[Column::new("id", "bigint")])
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "PostgreSQL can not work with table names longer than 63 symbols. Consider using the 'sqlAlias' attribute in your cube definition for {name}."
            )
        );
        driver.release().await.unwrap();
    }

    #[test]
    fn pg_config_from_parts_and_url() {
        use crate::config::SslConfig;
        let mut cfg = DriverConfig::default();
        cfg.data_source.host = Some("db".into());
        cfg.data_source.port = Some(5433);
        cfg.data_source.user = Some("u".into());
        cfg.data_source.password = Some("p".into());
        cfg.data_source.database = Some("d".into());
        cfg.data_source.ssl = Some(SslConfig::default());
        let pg = PostgresConfig::from_driver_config(cfg).pg_config().unwrap();
        assert_eq!(pg.get_hosts().len(), 1);
        assert_eq!(pg.get_ports(), &[5433]);
        assert_eq!(pg.get_user(), Some("u"));
        assert_eq!(pg.get_dbname(), Some("d"));
        assert!(matches!(
            pg.get_ssl_mode(),
            tokio_postgres::config::SslMode::Require
        ));

        let pg = PostgresConfig::from_url("postgres://a:b@h:1234/db")
            .pg_config()
            .unwrap();
        assert_eq!(pg.get_ports(), &[1234]);
        assert_eq!(pg.get_user(), Some("a"));
        assert_eq!(pg.get_dbname(), Some("db"));
        assert!(matches!(
            pg.get_ssl_mode(),
            tokio_postgres::config::SslMode::Disable
        ));
    }
}
