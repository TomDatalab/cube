//! Redshift driver: port of `@cubejs-backend/redshift-driver`.
//!
//! In Node the driver is a subclass of `PostgresDriver`; here it wraps one and
//! overrides the same methods. Everything not listed below (parameters,
//! quoting, type mapping, streaming, uploads) is the PostgreSQL behaviour.
//!
//! Not supported yet: IAM authentication (`CUBEJS_DB_REDSHIFT_CLUSTER_IDENTIFIER`,
//! which needs the AWS SDK) and `UNLOAD` to S3.

use async_trait::async_trait;
use serde_json::Value;

use crate::config::DriverConfig;
use crate::driver::{information_columns_to_structure, Driver};
use crate::error::{DriverError, Result};
use crate::postgres::{PostgresConfig, PostgresDriver};
use crate::sql;
use crate::types::{
    Column, ColumnInfo, DatabaseStructure, DownloadQueryResultsOptions, DownloadTableOptions,
    DownloadedData, DriverCapabilities, ExternalCreateTableOptions, GenericType, IndexSql,
    QueryOptions, QueryResult, SchemaColumn, SchemaName, SchemaTable, StreamOptions,
    StreamTableData, TableCsvData, TableMemoryData, TableStructure, UnloadOptions,
};

/// `getDefaultConcurrency`.
pub const DEFAULT_CONCURRENCY: usize = 5;

/// Schemas Redshift hides from introspection (`IGNORED_SCHEMAS`).
pub const IGNORED_SCHEMAS: &[&str] = &[
    "pg_catalog",
    "pg_internal",
    "information_schema",
    "mysql",
    "performance_schema",
    "sys",
    "INFORMATION_SCHEMA",
];

/// Redshift breaks after 32767 bound parameters (`checkValuesLimit`).
pub const MAX_PARAMETERS: usize = 32_768;

/// `IGNORED_SCHEMAS.map(s => `'${s}'`).join(',')`.
fn ignored_schemas_list() -> String {
    IGNORED_SCHEMAS
        .iter()
        .map(|s| format!("'{s}'"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Configuration of [`RedshiftDriver`] (`RedshiftDriverConfiguration`).
#[derive(Debug, Clone)]
pub struct RedshiftConfig {
    /// The PostgreSQL configuration the driver builds on.
    pub postgres: PostgresConfig,
    /// `CUBEJS_DB_EXPORT_BUCKET` (unload is not implemented yet).
    pub export_bucket: Option<String>,
    /// `CUBEJS_DB_REDSHIFT_CLUSTER_IDENTIFIER` (IAM auth, unsupported).
    pub cluster_identifier: Option<String>,
}

impl RedshiftConfig {
    /// Builds the Redshift configuration from a generic [`DriverConfig`].
    pub fn from_driver_config(driver: DriverConfig) -> Self {
        let mut postgres = PostgresConfig::from_driver_config(driver);
        // `getInitialConfiguration`: Redshift is not read-only, UNLOAD needs
        // to be able to create tables.
        postgres.read_only = false;
        Self {
            postgres,
            export_bucket: None,
            cluster_identifier: None,
        }
    }

    /// Reads the configuration of `data_source` from the environment.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        let driver = DriverConfig::from_env(data_source)?;
        let mut config = Self::from_driver_config(driver);
        config.apply_env();
        Ok(config)
    }

    /// Reads the Redshift specific variables into an existing configuration.
    pub fn apply_env(&mut self) {
        self.export_bucket = env_var("CUBEJS_DB_EXPORT_BUCKET");
        self.cluster_identifier = env_var("CUBEJS_DB_REDSHIFT_CLUSTER_IDENTIFIER");
    }

    /// Reads the configuration from a connection URL, for tests and tooling.
    ///
    /// The database name is also lifted out of the URL, because the `SHOW …`
    /// statements need it explicitly.
    pub fn from_url(url: &str) -> Self {
        let mut driver = DriverConfig::default();
        driver.data_source.url = Some(url.to_string());
        if let Ok(parsed) = url::Url::parse(url) {
            let path = parsed.path().trim_start_matches('/');
            if !path.is_empty() {
                driver.data_source.database = Some(path.to_string());
            }
        }
        Self::from_driver_config(driver)
    }

    /// Database name used by the `SHOW …` statements.
    pub fn database(&self) -> Option<String> {
        self.postgres.driver.data_source.database.clone()
    }
}

fn env_var(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// Redshift driver.
pub struct RedshiftDriver {
    config: RedshiftConfig,
    inner: PostgresDriver,
}

impl std::fmt::Debug for RedshiftDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedshiftDriver")
            .field("postgres", &self.inner)
            .finish()
    }
}

impl RedshiftDriver {
    /// Creates the driver and its (lazy) connection pool.
    pub fn new(config: RedshiftConfig) -> Result<Self> {
        let has_password = config
            .postgres
            .driver
            .data_source
            .password
            .as_ref()
            .is_some_and(|p| !p.is_empty());
        if config
            .cluster_identifier
            .as_ref()
            .is_some_and(|c| !c.is_empty())
            && !has_password
        {
            return Err(DriverError::Config(
                "Redshift IAM authentication (CUBEJS_DB_REDSHIFT_CLUSTER_IDENTIFIER without \
                 CUBEJS_DB_PASS) is not supported by the Rust driver yet: it needs the AWS SDK. \
                 Configure CUBEJS_DB_USER / CUBEJS_DB_PASS instead."
                    .to_string(),
            ));
        }
        let inner = PostgresDriver::new(config.postgres.clone())?;
        Ok(Self { config, inner })
    }

    /// Creates the driver from `CUBEJS_DB_*`.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Self::new(RedshiftConfig::from_env(data_source)?)
    }

    /// The driver configuration.
    pub fn redshift_config(&self) -> &RedshiftConfig {
        &self.config
    }

    /// The wrapped PostgreSQL driver.
    pub fn postgres(&self) -> &PostgresDriver {
        &self.inner
    }

    /// Port of `RedshiftDriver.checkValuesLimit`.
    pub fn check_values_limit(params: &[Value]) -> Result<()> {
        if params.len() >= MAX_PARAMETERS {
            return Err(DriverError::Query(format!(
                "Redshift server does not support more than 32767 parameters, but {} passed",
                params.len()
            )));
        }
        Ok(())
    }

    fn database(&self) -> Result<String> {
        self.config
            .database()
            .filter(|d| !d.is_empty())
            .ok_or_else(|| {
                DriverError::Config(
                "CUBEJS_DB_NAME is required by the Redshift driver (SHOW SCHEMAS FROM DATABASE)."
                    .to_string(),
            )
            })
    }

    /// `SHOW TABLES FROM SCHEMA <db>.<schema>`.
    async fn tables_for_external_schema(&self, schema_name: &str) -> Result<Vec<String>> {
        let result = self
            .query(
                &format!("SHOW TABLES FROM SCHEMA {}.{schema_name}", self.database()?),
                &[],
                &QueryOptions::default(),
            )
            .await?;
        Ok((0..result.len())
            .filter_map(|i| result.get_string(i, "table_name"))
            .collect())
    }

    /// `SHOW COLUMNS FROM TABLE <db>.<schema>.<table>`.
    async fn columns_for_external_table(
        &self,
        schema_name: &str,
        table_name: &str,
    ) -> Result<Vec<ColumnInfo>> {
        let result = self
            .query(
                &format!(
                    "SHOW COLUMNS FROM TABLE {}.{schema_name}.{table_name}",
                    self.database()?
                ),
                &[],
                &QueryOptions::default(),
            )
            .await?;
        Ok((0..result.len())
            .filter_map(|i| {
                Some(ColumnInfo {
                    schema_name: result
                        .get_string(i, "schema_name")
                        .unwrap_or_else(|| schema_name.to_string()),
                    table_name: result
                        .get_string(i, "table_name")
                        .unwrap_or_else(|| table_name.to_string()),
                    column_name: result.get_string(i, "column_name")?,
                    data_type: result.get_string(i, "data_type")?,
                    attributes: Vec::new(),
                    foreign_keys: Vec::new(),
                })
            })
            .collect())
    }
}

#[async_trait]
impl Driver for RedshiftDriver {
    fn config(&self) -> &DriverConfig {
        self.inner.config()
    }

    /// Redshift has no cheap connection check, and querying even system tables
    /// is billed, so only the pool connection is exercised.
    async fn test_connection(&self) -> Result<()> {
        self.inner.check_pool_connection().await
    }

    async fn query(
        &self,
        sql: &str,
        params: &[Value],
        options: &QueryOptions,
    ) -> Result<QueryResult> {
        Self::check_values_limit(params)?;
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
        DriverCapabilities {
            incremental_schema_loading: true,
            ..Default::default()
        }
    }

    async fn release(&self) -> Result<()> {
        self.inner.release().await
    }

    /// Redshift does not expose primary keys through `information_schema`.
    fn primary_keys_query(&self, _condition: Option<&str>) -> Option<String> {
        None
    }

    /// Redshift does not expose foreign keys through `information_schema`.
    fn foreign_keys_query(&self, _condition: Option<&str>) -> Option<String> {
        None
    }

    fn information_schema_query(&self) -> String {
        format!(
            "
      SELECT columns.column_name as {},
             columns.table_name as {},
             columns.table_schema as {},
             columns.data_type as {}
      FROM information_schema.columns
      WHERE columns.table_schema NOT IN ({})
   ",
            self.quote_identifier("column_name"),
            self.quote_identifier("table_name"),
            self.quote_identifier("table_schema"),
            self.quote_identifier("data_type"),
            ignored_schemas_list(),
        )
    }

    fn get_schemas_query(&self) -> String {
        format!(
            "
      SELECT table_schema as {}
      FROM information_schema.tables
      WHERE table_schema NOT IN ({})
      GROUP BY table_schema
    ",
            self.quote_identifier("schema_name"),
            ignored_schemas_list(),
        )
    }

    /// Schemas not owned by the current user are missing from
    /// `information_schema`, so the check goes through `pg_namespace`.
    async fn create_schema_if_not_exists(&self, schema_name: &str) -> Result<()> {
        let existing = self
            .query(
                &format!(
                    "SELECT nspname FROM pg_namespace where nspname = {}",
                    self.param(0)
                ),
                &[Value::from(schema_name)],
                &QueryOptions::default(),
            )
            .await?;
        if existing.is_empty() {
            self.query(
                &format!("CREATE SCHEMA IF NOT EXISTS {schema_name}"),
                &[],
                &QueryOptions::default(),
            )
            .await?;
        }
        Ok(())
    }

    /// External (Spectrum) tables are invisible to `information_schema`, so
    /// they are collected separately.
    async fn tables_schema(&self) -> Result<DatabaseStructure> {
        let query = self.information_schema_query();
        let data = self.query(&query, &[], &QueryOptions::default()).await?;
        let mut structure = information_columns_to_structure(&data);

        let all_schemas = self.get_schemas().await?;
        let external: Vec<String> = all_schemas
            .into_iter()
            .map(|s| s.schema_name)
            .filter(|s| !structure.contains_key(s))
            .collect();

        for schema in external {
            let tables = self.tables_for_external_schema(&schema).await?;
            let entry = structure.entry(schema.clone()).or_default();
            for table in tables {
                let columns = self.columns_for_external_table(&schema, &table).await?;
                entry.insert(
                    table,
                    columns
                        .into_iter()
                        .map(|c| SchemaColumn {
                            name: c.column_name,
                            type_: c.data_type,
                            attributes: Vec::new(),
                            foreign_keys: Vec::new(),
                        })
                        .collect(),
                );
            }
        }

        Ok(structure)
    }

    /// `SHOW SCHEMAS FROM DATABASE <db>` also returns external schemas.
    async fn get_schemas(&self) -> Result<Vec<SchemaName>> {
        let result = self
            .query(
                &format!("SHOW SCHEMAS FROM DATABASE {}", self.database()?),
                &[],
                &QueryOptions::default(),
            )
            .await?;
        Ok((0..result.len())
            .filter_map(|i| result.get_string(i, "schema_name"))
            .filter(|s| !IGNORED_SCHEMAS.contains(&s.as_str()))
            .map(|schema_name| SchemaName { schema_name })
            .collect())
    }

    async fn get_tables_for_specific_schemas(
        &self,
        schemas: &[SchemaName],
    ) -> Result<Vec<SchemaTable>> {
        let mut tables = self.inner.get_tables_for_specific_schemas(schemas).await?;

        // External schemas are not described by `information_schema.tables`.
        let missing: Vec<&SchemaName> = schemas
            .iter()
            .filter(|s| !tables.iter().any(|t| t.schema_name == s.schema_name))
            .collect();
        for schema in missing {
            for table_name in self.tables_for_external_schema(&schema.schema_name).await? {
                tables.push(SchemaTable {
                    schema_name: schema.schema_name.clone(),
                    table_name,
                });
            }
        }
        Ok(tables)
    }

    async fn get_columns_for_specific_tables(
        &self,
        tables: &[SchemaTable],
    ) -> Result<Vec<ColumnInfo>> {
        // The base implementation without the primary/foreign key queries,
        // which Redshift does not support.
        let (condition, parameters) = sql::columns_for_specific_tables_condition(
            tables,
            &|i| self.param(i),
            &self.column_name_for_schema_name(),
            &self.column_name_for_table_name(),
        );
        let params: Vec<Value> = parameters.into_iter().map(Value::from).collect();
        let query = self.get_columns_for_specific_tables_query(&condition);
        let result = self
            .query(&query, &params, &QueryOptions::default())
            .await?;
        let mut columns: Vec<ColumnInfo> = (0..result.len())
            .filter_map(|i| {
                Some(ColumnInfo {
                    schema_name: result.get_string(i, "schema_name")?,
                    table_name: result.get_string(i, "table_name")?,
                    column_name: result.get_string(i, "column_name")?,
                    data_type: result.get_string(i, "data_type")?,
                    attributes: Vec::new(),
                    foreign_keys: Vec::new(),
                })
            })
            .collect();

        let missing: Vec<&SchemaTable> = tables
            .iter()
            .filter(|t| {
                !columns
                    .iter()
                    .any(|c| c.schema_name == t.schema_name && c.table_name == t.table_name)
            })
            .collect();
        for table in missing {
            columns.extend(
                self.columns_for_external_table(&table.schema_name, &table.table_name)
                    .await?,
            );
        }
        Ok(columns)
    }

    /// Falls back to `SHOW COLUMNS` for external (Spectrum) tables.
    async fn table_column_types(&self, table: &str) -> Result<TableStructure> {
        let columns = self.inner.table_column_types(table).await?;
        if !columns.is_empty() {
            return Ok(columns);
        }
        let name = crate::types::TableName::split(table);
        let table_name = name.name.split('.').next().unwrap_or("");
        Ok(self
            .columns_for_external_table(&name.schema, table_name)
            .await?
            .into_iter()
            .map(|c| {
                Column::new(
                    c.column_name,
                    self.to_generic_type(&c.data_type, None, None),
                )
            })
            .collect())
    }

    async fn query_column_types(
        &self,
        sql: &str,
        params: &[Value],
        options: &QueryOptions,
    ) -> Result<TableStructure> {
        Self::check_values_limit(params)?;
        self.inner.query_column_types(sql, params, options).await
    }

    /// Redshift allows longer table names than PostgreSQL.
    async fn create_table(&self, quoted_table_name: &str, columns: &[Column]) -> Result<()> {
        if quoted_table_name.len() > 127 {
            return Err(DriverError::Query(format!(
                "Redshift can not work with table names longer than 127 symbols. \
                 Consider using the 'sqlAlias' attribute in your cube definition for {quoted_table_name}."
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

    async fn upload_table_with_indexes(
        &self,
        table: &str,
        columns: &[Column],
        table_data: &TableMemoryData,
        indexes_sql: &[IndexSql],
        unique_key_columns: &[String],
        external_options: &ExternalCreateTableOptions,
    ) -> Result<()> {
        self.create_table(table, columns).await?;
        // The PostgreSQL implementation creates the table itself, so the rows
        // are uploaded through it with the table already in place.
        let result = self
            .inner
            .upload_rows_into_existing_table(table, columns, table_data, indexes_sql)
            .await;
        let _ = (unique_key_columns, external_options);
        if let Err(e) = result {
            if let Err(drop_err) = self.drop_table(table, &QueryOptions::default()).await {
                log::warn!("Unable to drop table {table} after failed upload: {drop_err}");
            }
            return Err(e);
        }
        Ok(())
    }

    async fn stream(
        &self,
        sql: &str,
        params: &[Value],
        options: &StreamOptions,
    ) -> Result<StreamTableData> {
        Self::check_values_limit(params)?;
        self.inner.stream(sql, params, options).await
    }

    async fn download_query_results(
        &self,
        sql: &str,
        params: &[Value],
        options: &DownloadQueryResultsOptions,
    ) -> Result<DownloadedData> {
        Self::check_values_limit(params)?;
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

    async fn is_unload_supported(&self, _options: &UnloadOptions) -> Result<bool> {
        // `UNLOAD … TO 's3://…'` needs the AWS SDK to list and sign the files.
        Ok(false)
    }

    async fn unload(&self, _table: &str, _options: &UnloadOptions) -> Result<TableCsvData> {
        Err(DriverError::NotImplemented(
            "Redshift UNLOAD to S3 is not implemented in the Rust driver yet.".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redshift() -> RedshiftDriver {
        let mut config = RedshiftConfig::from_driver_config(DriverConfig::default());
        config.postgres.driver.data_source.database = Some("dev".to_string());
        RedshiftDriver::new(config).unwrap()
    }

    #[tokio::test]
    async fn inherits_postgres_behaviour() {
        let driver = redshift();
        assert_eq!(driver.param(0), "$1");
        assert_eq!(driver.quote_identifier("a"), "\"a\"");
        assert_eq!(
            driver.to_generic_type("bpchar", None, None),
            GenericType::Other("varchar".into())
        );
        assert_eq!(driver.from_generic_type(&GenericType::Int), "int8");
        assert_eq!(
            driver.create_table_sql("t", &[Column::new("a", "int")]),
            r#"CREATE TABLE t ("a" int8)"#
        );
        // `getInitialConfiguration` turns the PostgreSQL read-only default off
        assert!(!driver.read_only());
        assert!(driver.capabilities().incremental_schema_loading);
        assert!(!driver
            .is_unload_supported(&UnloadOptions::default())
            .await
            .unwrap());
        driver.release().await.unwrap();
    }

    #[test]
    fn introspection_queries_use_the_redshift_schema_list() {
        let driver = redshift();
        let q = driver.information_schema_query();
        assert!(q.contains(
            "WHERE columns.table_schema NOT IN ('pg_catalog','pg_internal','information_schema','mysql','performance_schema','sys','INFORMATION_SCHEMA')"
        ));
        let q = driver.get_schemas_query();
        assert!(q.contains("'pg_internal'"));
        assert!(q.contains("GROUP BY table_schema"));
        // keys are not supported
        assert!(driver.primary_keys_query(None).is_none());
        assert!(driver.foreign_keys_query(None).is_none());
    }

    #[tokio::test]
    async fn parameter_limit_is_enforced() {
        let driver = redshift();
        let params: Vec<Value> = vec![Value::from("x"); 32_768];
        let err = driver
            .query("SELECT 1", &params, &QueryOptions::default())
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Redshift server does not support more than 32767 parameters, but 32768 passed"
        );
        assert!(RedshiftDriver::check_values_limit(&vec![Value::from("x"); 32_767]).is_ok());
        driver.release().await.unwrap();
    }

    #[tokio::test]
    async fn table_names_may_be_longer_than_in_postgres() {
        let driver = redshift();
        let name = "a".repeat(100);
        // 100 characters are fine for Redshift (PostgreSQL would reject them);
        // the query then fails because nothing is listening.
        let err = driver
            .create_table(&name, &[Column::new("id", "bigint")])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Error during create table"));

        let name = "a".repeat(128);
        let err = driver
            .create_table(&name, &[Column::new("id", "bigint")])
            .await
            .unwrap_err();
        assert!(err
            .to_string()
            .starts_with("Redshift can not work with table names longer than 127 symbols."));
        driver.release().await.unwrap();
    }

    #[test]
    fn iam_authentication_is_reported() {
        let mut config = RedshiftConfig::from_driver_config(DriverConfig::default());
        config.cluster_identifier = Some("my-cluster".to_string());
        let err = RedshiftDriver::new(config).unwrap_err();
        assert!(err.to_string().contains("IAM authentication"));
    }
}
