//! Materialize driver: port of `@cubejs-backend/materialize-driver`.
//!
//! In Node the driver is a subclass of `PostgresDriver`; here it wraps one and
//! overrides the same methods. Everything not listed below (`$n` parameters,
//! quoting, type mapping, `readOnly`, introspection of keys) is the
//! PostgreSQL behaviour.
//!
//! Overrides:
//!
//! * SSL is **on by default**: `CUBEJS_DB_SSL=false` disables it,
//!   `CUBEJS_DB_SSL=true` verifies the server certificate, and leaving it unset
//!   connects with TLS and verification (Node's `ssl: true`).
//! * `application_name` is `cubejs-materialize-driver`.
//! * `prepareConnection`: `SET TIME ZONE` (no `statement_timeout`, which
//!   Materialize does not support) and `SET CLUSTER TO` from
//!   `CUBEJS_DB_MATERIALIZE_CLUSTER`.
//! * `createSchemaIfNotExists` goes through `SHOW SCHEMAS`.
//! * `uploadTableWithIndexes` inserts row by row (`BaseDriver`'s), since
//!   Materialize has no `UNNEST` of array parameters.
//! * `tablesSchema` only lists sources, tables and materialized views, with
//!   a filter that depends on `mz_version()`.
//! * `stream` reads through a cursor (`DECLARE` / `FETCH 1000 … WITH
//!   (TIMEOUT …)`) inside a transaction.

use async_trait::async_trait;
use futures::{StreamExt, TryStreamExt};
use serde_json::Value;

use crate::config::{DriverConfig, SslConfig};
use crate::driver::{information_columns_to_structure, Driver};
use crate::error::{DriverError, Result};
use crate::postgres::{
    check_values_limit, postgres_type_name, ConnectionSetup, JsonCell, PostgresConfig,
    PostgresDriver, TextParam,
};
use crate::types::{
    Column, ColumnInfo, DatabaseStructure, DownloadQueryResultsOptions, DownloadTableOptions,
    DownloadedData, DriverCapabilities, GenericType, QueryOptions, QueryResult, Row, SchemaName,
    SchemaTable, StreamOptions, StreamTableData, TableMemoryData, TableStructure,
};

/// `getDefaultConcurrency`.
pub const DEFAULT_CONCURRENCY: usize = 2;
/// `application_name` set on every connection.
pub const APPLICATION_NAME: &str = "cubejs-materialize-driver";
/// Name of the cursor `stream` declares.
pub const CURSOR_ID: &str = "mz_cursor";
/// Rows per `FETCH` of `stream`.
pub const FETCH_SIZE: usize = 1000;

/// Configuration of [`MaterializeDriver`].
#[derive(Debug, Clone)]
pub struct MaterializeConfig {
    /// The PostgreSQL configuration the driver builds on.
    pub postgres: PostgresConfig,
    /// `CUBEJS_DB_MATERIALIZE_CLUSTER` (`SET CLUSTER TO …` on every connection).
    pub cluster: Option<String>,
    /// `application_name` (default `cubejs-materialize-driver`).
    pub application_name: String,
}

impl MaterializeConfig {
    /// Builds the configuration from a generic [`DriverConfig`].
    ///
    /// SSL is turned on unless the configuration already has it; call
    /// [`MaterializeConfig::apply_env`] to honour an explicit
    /// `CUBEJS_DB_SSL=false`.
    pub fn from_driver_config(driver: DriverConfig) -> Self {
        let mut postgres = PostgresConfig::from_driver_config(driver);
        if postgres.driver.data_source.ssl.is_none() {
            // `options.ssl = true`: TLS, and Node verifies certificates by default.
            postgres.driver.data_source.ssl = Some(SslConfig {
                reject_unauthorized: true,
                ..Default::default()
            });
        }
        Self {
            postgres,
            cluster: None,
            application_name: APPLICATION_NAME.to_string(),
        }
    }

    /// Reads the configuration of `data_source` from the environment.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        let mut config = Self::from_driver_config(DriverConfig::from_env(data_source)?);
        config.apply_env();
        Ok(config)
    }

    /// Reads the Materialize specific variables. Like the Node driver, these
    /// are read from `process.env` directly, not per data source.
    pub fn apply_env(&mut self) {
        self.apply_env_values(
            std::env::var("CUBEJS_DB_SSL").ok().as_deref(),
            std::env::var("CUBEJS_DB_MATERIALIZE_CLUSTER")
                .ok()
                .as_deref(),
        );
    }

    /// [`MaterializeConfig::apply_env`] with explicit values (for tests).
    pub fn apply_env_values(&mut self, ssl: Option<&str>, cluster: Option<&str>) {
        let ssl_config = &mut self.postgres.driver.data_source.ssl;
        match ssl {
            Some("false") => *ssl_config = None,
            // `{ rejectUnauthorized: true }`; a configured CA or client
            // certificate is kept (Node drops it here).
            Some("true") => {
                ssl_config
                    .get_or_insert_with(SslConfig::default)
                    .reject_unauthorized = true;
            }
            _ => {}
        }
        self.cluster = cluster.filter(|c| !c.is_empty()).map(str::to_string);
    }

    /// Reads the configuration from a connection URL, for tests and tooling.
    /// SSL follows the URL's `sslmode` (disabled when absent).
    pub fn from_url(url: &str) -> Self {
        let mut config = Self::from_driver_config(DriverConfig::default());
        config.postgres.driver.data_source.url = Some(url.to_string());
        config.postgres.driver.data_source.ssl = None;
        config
    }

    /// Statements of `prepareConnection`.
    pub fn session_statements(&self) -> Vec<String> {
        let mut statements = vec![format!(
            "SET TIME ZONE '{}'",
            self.postgres.store_timezone.replace('\'', "''")
        )];
        if let Some(cluster) = &self.cluster {
            statements.push(format!("SET CLUSTER TO {cluster}"));
        }
        statements
    }
}

/// Materialize driver.
pub struct MaterializeDriver {
    config: MaterializeConfig,
    inner: PostgresDriver,
}

impl std::fmt::Debug for MaterializeDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MaterializeDriver")
            .field("postgres", &self.inner)
            .field("cluster", &self.config.cluster)
            .finish()
    }
}

impl MaterializeDriver {
    /// Creates the driver and its (lazy) connection pool.
    pub fn new(config: MaterializeConfig) -> Result<Self> {
        let inner = PostgresDriver::new_with_setup(
            config.postgres.clone(),
            ConnectionSetup {
                statements: Some(config.session_statements()),
                application_name: Some(config.application_name.clone()),
            },
        )?;
        Ok(Self { config, inner })
    }

    /// Creates the driver from `CUBEJS_DB_*`.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Self::new(MaterializeConfig::from_env(data_source)?)
    }

    /// The driver configuration.
    pub fn materialize_config(&self) -> &MaterializeConfig {
        &self.config
    }

    /// The wrapped PostgreSQL driver.
    pub fn postgres(&self) -> &PostgresDriver {
        &self.inner
    }

    /// `getMaterializeVersion`: `v0.24.3-alpha.5 (65778f520)` → `v0.24.3-alpha.5`.
    pub async fn get_materialize_version(&self) -> Result<String> {
        let result = self
            .query(
                "SELECT mz_version() as version;",
                &[],
                &QueryOptions::default(),
            )
            .await?;
        let version = result
            .get_string(0, "version")
            .ok_or_else(|| DriverError::Query("mz_version() returned no version".to_string()))?;
        Ok(version.split(' ').next().unwrap_or_default().to_string())
    }

    /// `informationSchemaQueryWithFilter`: only sources, tables and
    /// materialized views are queryable (and, before v0.27.0-alpha, only
    /// materialized sources and views).
    pub fn information_schema_query_with_filter(&self, version: &str) -> Result<String> {
        let materialization_filter = if semver_lt(version, "v0.27.0-alpha")? {
            "
        table_name IN (
          SELECT name
          FROM mz_catalog.mz_sources
          WHERE mz_internal.mz_is_materialized(id)
          UNION
          SELECT name
          FROM mz_catalog.mz_views
          WHERE mz_internal.mz_is_materialized(id)
          UNION
          SELECT t.name
          FROM mz_catalog.mz_tables t
        )"
        } else {
            "
        table_name IN (
          SELECT name
          FROM mz_catalog.mz_sources
          UNION
          SELECT name
          FROM mz_catalog.mz_tables t
          UNION
          SELECT name
          FROM mz_catalog.mz_materialized_views t
        )
        "
        };
        Ok(format!(
            "{} AND {materialization_filter}",
            self.information_schema_query()
        ))
    }

    fn map_fields(&self, statement: &tokio_postgres::Statement) -> Vec<Column> {
        statement
            .columns()
            .iter()
            .map(|c| {
                Column::new(
                    c.name(),
                    self.to_generic_type(&postgres_type_name(c.type_()), None, None),
                )
            })
            .collect()
    }

    /// `FETCH … WITH (TIMEOUT='<executionTimeout> milliseconds')`.
    pub fn fetch_statement(&self) -> String {
        format!(
            "FETCH {FETCH_SIZE} {CURSOR_ID} WITH (TIMEOUT='{} milliseconds');",
            self.config.postgres.execution_timeout.as_millis()
        )
    }
}

/// `semver.lt(a, b)` for Materialize versions (`v0.24.3-alpha.5`).
///
/// Invalid versions fail like `semver` does (`TypeError: Invalid Version`).
pub fn semver_lt(a: &str, b: &str) -> Result<bool> {
    Ok(parse_semver(a)? < parse_semver(b)?)
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum PreId {
    Num(u64),
    Alpha(String),
}

#[derive(Debug, PartialEq, Eq)]
struct SemVer {
    core: (u64, u64, u64),
    pre: Vec<PreId>,
}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        self.core
            .cmp(&other.core)
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                // A release ranks above its pre-releases.
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                // Numeric identifiers rank below alphanumeric ones, and a
                // shorter list of equal identifiers ranks first.
                (false, false) => self.pre.cmp(&other.pre),
            })
    }
}

fn parse_semver(version: &str) -> Result<SemVer> {
    let invalid = || DriverError::Query(format!("Invalid Version: {version}"));
    let v = version.trim();
    let v = v.strip_prefix(['v', '=']).unwrap_or(v);
    let v = v.split('+').next().unwrap_or(v);
    let (core, pre) = match v.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (v, None),
    };
    let nums: Vec<u64> = core
        .split('.')
        .map(|p| p.parse::<u64>().map_err(|_| invalid()))
        .collect::<Result<_>>()?;
    let [major, minor, patch] = nums[..] else {
        return Err(invalid());
    };
    let pre = match pre {
        None => Vec::new(),
        Some(pre) => pre
            .split('.')
            .map(|id| {
                if id.is_empty() {
                    Err(invalid())
                } else if id.chars().all(|c| c.is_ascii_digit()) {
                    id.parse::<u64>().map(PreId::Num).map_err(|_| invalid())
                } else {
                    Ok(PreId::Alpha(id.to_string()))
                }
            })
            .collect::<Result<_>>()?,
    };
    Ok(SemVer {
        core: (major, minor, patch),
        pre,
    })
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

#[async_trait]
impl Driver for MaterializeDriver {
    fn config(&self) -> &DriverConfig {
        self.inner.config()
    }

    async fn test_connection(&self) -> Result<()> {
        self.inner.test_connection().await
    }

    async fn query(
        &self,
        sql: &str,
        params: &[Value],
        options: &QueryOptions,
    ) -> Result<QueryResult> {
        self.inner.query(sql, params, options).await
    }

    fn param(&self, index: usize) -> String {
        self.inner.param(index)
    }

    fn quote_identifier(&self, identifier: &str) -> String {
        self.inner.quote_identifier(identifier)
    }

    fn to_generic_type(
        &self,
        db_type: &str,
        precision: Option<i64>,
        scale: Option<i64>,
    ) -> GenericType {
        self.inner.to_generic_type(db_type, precision, scale)
    }

    #[allow(clippy::wrong_self_convention)]
    fn from_generic_type(&self, generic: &GenericType) -> String {
        self.inner.from_generic_type(generic)
    }

    fn read_only(&self) -> bool {
        self.inner.read_only()
    }

    fn capabilities(&self) -> DriverCapabilities {
        self.inner.capabilities()
    }

    async fn release(&self) -> Result<()> {
        self.inner.release().await
    }

    fn primary_keys_query(&self, condition: Option<&str>) -> Option<String> {
        self.inner.primary_keys_query(condition)
    }

    fn foreign_keys_query(&self, condition: Option<&str>) -> Option<String> {
        self.inner.foreign_keys_query(condition)
    }

    async fn create_schema_if_not_exists(&self, schema_name: &str) -> Result<()> {
        let schemas = self
            .query(
                &format!("SHOW SCHEMAS WHERE name = '{schema_name}'"),
                &[],
                &QueryOptions::default(),
            )
            .await?;
        if schemas.is_empty() {
            self.query(
                &format!("CREATE SCHEMA IF NOT EXISTS {schema_name}"),
                &[],
                &QueryOptions::default(),
            )
            .await?;
        }
        Ok(())
    }

    async fn tables_schema(&self) -> Result<DatabaseStructure> {
        let version = self.get_materialize_version().await?;
        let query = self.information_schema_query_with_filter(&version)?;
        let data = self.query(&query, &[], &QueryOptions::default()).await?;
        Ok(information_columns_to_structure(&data))
    }

    async fn get_schemas(&self) -> Result<Vec<SchemaName>> {
        self.inner.get_schemas().await
    }

    async fn get_tables_for_specific_schemas(
        &self,
        schemas: &[SchemaName],
    ) -> Result<Vec<SchemaTable>> {
        self.inner.get_tables_for_specific_schemas(schemas).await
    }

    async fn get_columns_for_specific_tables(
        &self,
        tables: &[SchemaTable],
    ) -> Result<Vec<ColumnInfo>> {
        self.inner.get_columns_for_specific_tables(tables).await
    }

    /// `BEGIN; DECLARE mz_cursor CURSOR FOR …; FETCH 0` for the column types,
    /// then `FETCH 1000` until the cursor is drained; `COMMIT` when the stream
    /// ends or is dropped half-way (the connection then returns to the pool).
    async fn stream(
        &self,
        sql: &str,
        params: &[Value],
        _options: &StreamOptions,
    ) -> Result<StreamTableData> {
        check_values_limit(params)?;
        let client = self.inner.pooled_client().await?;

        let text_params: Vec<TextParam> = params.iter().map(TextParam::from_json).collect();
        let prepared = async {
            client.batch_execute("BEGIN;").await?;
            client
                .execute_raw(
                    &format!("DECLARE {CURSOR_ID} CURSOR FOR {sql}"),
                    text_params.iter(),
                )
                .await?;
            let described = client
                .prepare(&format!("FETCH 0 FROM {CURSOR_ID};"))
                .await?;
            let fetch = client.prepare(&self.fetch_statement()).await?;
            Ok::<_, tokio_postgres::Error>((described, fetch))
        }
        .await;
        let (described, fetch) = match prepared {
            Ok(v) => v,
            Err(e) => {
                let _ = client.batch_execute("COMMIT;").await;
                return Err(e.into());
            }
        };
        let columns = self.map_fields(&described);
        let width = columns.len();

        // The pooled connection travels with the stream. A `COMMIT` guard
        // closes the transaction (and the cursor) whichever way it ends.
        struct Guard {
            client: Option<deadpool_postgres::Object>,
        }
        impl Drop for Guard {
            fn drop(&mut self) {
                if let Some(client) = self.client.take() {
                    if let Ok(handle) = tokio::runtime::Handle::try_current() {
                        handle.spawn(async move {
                            let _ = client.batch_execute("COMMIT;").await;
                        });
                    }
                }
            }
        }
        let guard = Guard {
            client: Some(client),
        };

        let rows = futures::stream::try_unfold(
            (guard, fetch, false),
            move |(guard, fetch, done)| async move {
                if done {
                    return Ok(None);
                }
                let client = guard.client.as_ref().expect("stream connection");
                let batch: Vec<tokio_postgres::Row> = client
                    .query_raw(&fetch, std::iter::empty::<TextParam>())
                    .await?
                    .try_collect()
                    .await?;
                let converted = batch
                    .iter()
                    .map(|row| convert_row(row, width))
                    .collect::<Result<Vec<Row>>>()?;
                let finished = converted.is_empty();
                Ok::<_, DriverError>(Some((converted, (guard, fetch, finished))))
            },
        )
        .map_ok(|batch| futures::stream::iter(batch.into_iter().map(Ok)))
        .try_flatten()
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
        self.inner
            .download_query_results(sql, params, options)
            .await
    }

    async fn download_table(
        &self,
        table: &str,
        options: &DownloadTableOptions,
    ) -> Result<TableMemoryData> {
        self.inner.download_table(table, options).await
    }

    async fn table_column_types(&self, table: &str) -> Result<TableStructure> {
        self.inner.table_column_types(table).await
    }

    async fn query_column_types(
        &self,
        sql: &str,
        params: &[Value],
        options: &QueryOptions,
    ) -> Result<TableStructure> {
        self.inner.query_column_types(sql, params, options).await
    }

    async fn create_table(&self, quoted_table_name: &str, columns: &[Column]) -> Result<()> {
        self.inner.create_table(quoted_table_name, columns).await
    }

    // `uploadTableWithIndexes` is `BaseDriver`'s row-by-row insert: the
    // trait's default, which is used by not overriding it here.
}

#[cfg(test)]
mod tests {
    use super::*;

    fn driver() -> MaterializeDriver {
        let mut config = MaterializeConfig::from_driver_config(DriverConfig::default());
        config.postgres.driver.data_source.ssl = None;
        MaterializeDriver::new(config).unwrap()
    }

    #[test]
    fn ssl_is_on_by_default() {
        let config = MaterializeConfig::from_driver_config(DriverConfig::default());
        let ssl = config.postgres.driver.data_source.ssl.clone().unwrap();
        assert!(ssl.reject_unauthorized);

        let mut config = MaterializeConfig::from_driver_config(DriverConfig::default());
        config.apply_env_values(Some("false"), None);
        assert!(config.postgres.driver.data_source.ssl.is_none());

        let mut driver = DriverConfig::default();
        driver.data_source.ssl = Some(SslConfig {
            ca: Some("-----BEGIN CERTIFICATE-----".into()),
            ..Default::default()
        });
        let mut config = MaterializeConfig::from_driver_config(driver);
        config.apply_env_values(Some("true"), Some("quickstart"));
        let ssl = config.postgres.driver.data_source.ssl.clone().unwrap();
        assert!(ssl.reject_unauthorized);
        assert!(ssl.ca.is_some());
        assert_eq!(config.cluster.as_deref(), Some("quickstart"));
    }

    #[test]
    fn session_statements() {
        let mut config = MaterializeConfig::from_driver_config(DriverConfig::default());
        assert_eq!(config.session_statements(), vec!["SET TIME ZONE 'UTC'"]);
        config.apply_env_values(None, Some("analytics"));
        assert_eq!(
            config.session_statements(),
            vec!["SET TIME ZONE 'UTC'", "SET CLUSTER TO analytics"]
        );
        assert_eq!(config.application_name, "cubejs-materialize-driver");
        let pg = config.postgres.pg_config().unwrap();
        assert!(matches!(
            pg.get_ssl_mode(),
            tokio_postgres::config::SslMode::Require
        ));
    }

    #[test]
    fn semver_comparison() {
        assert!(semver_lt("v0.24.3-alpha.5", "v0.27.0-alpha").unwrap());
        assert!(semver_lt("v0.26.9", "v0.27.0-alpha").unwrap());
        assert!(!semver_lt("v0.27.0-alpha", "v0.27.0-alpha").unwrap());
        assert!(!semver_lt("v0.27.0-alpha.1", "v0.27.0-alpha").unwrap());
        assert!(!semver_lt("v0.27.0", "v0.27.0-alpha").unwrap());
        assert!(!semver_lt("v0.88.0", "v0.27.0-alpha").unwrap());
        assert!(!semver_lt("v26.1.0", "v0.27.0-alpha").unwrap());
        assert!(semver_lt("v0.27.0-alpha.2", "v0.27.0-alpha.10").unwrap());
        assert!(semver_lt("v0.27.0-1", "v0.27.0-alpha").unwrap());
        let err = semver_lt("dev", "v0.27.0-alpha").unwrap_err();
        assert_eq!(err.to_string(), "Invalid Version: dev");
    }

    #[tokio::test]
    async fn information_schema_filter_depends_on_version() {
        let d = driver();
        let old = d.information_schema_query_with_filter("v0.24.3").unwrap();
        assert!(old.contains("FROM information_schema.columns"));
        assert!(old.contains(" AND \n        table_name IN ("));
        assert!(old.contains("mz_internal.mz_is_materialized(id)"));
        assert!(!old.contains("mz_materialized_views"));

        let new = d.information_schema_query_with_filter("v0.88.0").unwrap();
        assert!(new.contains("FROM mz_catalog.mz_materialized_views t"));
        assert!(!new.contains("mz_is_materialized"));
        d.release().await.unwrap();
    }

    #[tokio::test]
    async fn inherits_postgres_behaviour() {
        let d = driver();
        assert_eq!(d.param(1), "$2");
        assert!(d.read_only());
        assert!(d.capabilities().incremental_schema_loading);
        assert_eq!(
            d.fetch_statement(),
            "FETCH 1000 mz_cursor WITH (TIMEOUT='600000 milliseconds');"
        );
        assert_eq!(DEFAULT_CONCURRENCY, 2);
        d.release().await.unwrap();
    }
}
