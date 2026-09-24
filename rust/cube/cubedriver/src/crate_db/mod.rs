//! CrateDB driver: port of `@cubejs-backend/crate-driver`.
//!
//! In Node the driver is a subclass of `PostgresDriver`; here it wraps one and
//! overrides the same methods. Everything not listed below (configuration,
//! `$n` parameters, quoting, type mapping, introspection, streaming, uploads)
//! is the PostgreSQL behaviour.
//!
//! Overrides:
//!
//! * `prepareConnection`: CrateDB supports neither `SET TIME ZONE` nor
//!   `statement_timeout` (crate/crate#12356), so nothing is run on a new
//!   connection. (`loadUserDefinedTypes` has no counterpart: `tokio-postgres`
//!   resolves unknown type OIDs itself.)
//! * `loadPreAggregationIntoTable`: CrateDB rejects bound parameters in
//!   `CREATE TABLE AS`, so they are inlined as literals, and the new table is
//!   `REFRESH`ed because CrateDB is eventually consistent.

use async_trait::async_trait;
use serde_json::Value;

use crate::config::DriverConfig;
use crate::driver::Driver;
use crate::error::Result;
use crate::postgres::{ConnectionSetup, PostgresConfig, PostgresDriver};
use crate::types::{
    Column, ColumnInfo, DatabaseStructure, DownloadQueryResultsOptions, DownloadTableOptions,
    DownloadedData, DriverCapabilities, ExternalCreateTableOptions, GenericType, IndexSql,
    QueryOptions, QueryResult, SchemaName, SchemaTable, StreamOptions, StreamTableData,
    TableMemoryData, TableStructure,
};

/// `getDefaultConcurrency` (inherited from `PostgresDriver`).
pub const DEFAULT_CONCURRENCY: usize = PostgresDriver::DEFAULT_CONCURRENCY;

/// Configuration of [`CrateDriver`]: the PostgreSQL configuration, unchanged.
#[derive(Debug, Clone)]
pub struct CrateConfig {
    /// The PostgreSQL configuration the driver builds on.
    pub postgres: PostgresConfig,
}

impl CrateConfig {
    /// Builds the CrateDB configuration from a generic [`DriverConfig`].
    pub fn from_driver_config(driver: DriverConfig) -> Self {
        Self {
            postgres: PostgresConfig::from_driver_config(driver),
        }
    }

    /// Reads the configuration of `data_source` from the environment.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Ok(Self::from_driver_config(DriverConfig::from_env(
            data_source,
        )?))
    }

    /// Reads the configuration from a connection URL, for tests and tooling.
    pub fn from_url(url: &str) -> Self {
        Self {
            postgres: PostgresConfig::from_url(url),
        }
    }
}

/// CrateDB driver.
pub struct CrateDriver {
    inner: PostgresDriver,
}

impl std::fmt::Debug for CrateDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CrateDriver")
            .field("postgres", &self.inner)
            .finish()
    }
}

impl CrateDriver {
    /// Creates the driver and its (lazy) connection pool.
    pub fn new(config: CrateConfig) -> Result<Self> {
        let inner = PostgresDriver::new_with_setup(
            config.postgres,
            ConnectionSetup {
                // `prepareConnection` runs neither `SET TIME ZONE` nor
                // `SET statement_timeout`: CrateDB supports neither.
                statements: Some(Vec::new()),
                application_name: None,
            },
        )?;
        Ok(Self { inner })
    }

    /// Creates the driver from `CUBEJS_DB_*`.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Self::new(CrateConfig::from_env(data_source)?)
    }

    /// The wrapped PostgreSQL driver.
    pub fn postgres(&self) -> &PostgresDriver {
        &self.inner
    }

    /// Port of `CrateDriver.inlineParams`: replaces every `$<n>` with the
    /// literal of the `n`-th parameter; placeholders without a parameter are
    /// left untouched. Like the Node regex, it does not skip string literals.
    pub fn inline_params(sql: &str, params: &[Value]) -> String {
        if params.is_empty() {
            return sql.to_string();
        }
        let mut out = String::with_capacity(sql.len());
        let mut rest = sql;
        while let Some(pos) = rest.find('$') {
            out.push_str(&rest[..pos]);
            let after = &rest[pos + 1..];
            let digits = after
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(after.len());
            if digits == 0 {
                out.push('$');
                rest = after;
                continue;
            }
            let matched = &rest[pos..pos + 1 + digits];
            // `parseInt(index, 10) - 1`; an index beyond the parameters (or
            // `$0`) keeps the placeholder.
            let param = after[..digits]
                .parse::<usize>()
                .ok()
                .and_then(|n| n.checked_sub(1))
                .and_then(|i| params.get(i));
            match param {
                Some(value) => out.push_str(&Self::format_param(value)),
                None => out.push_str(matched),
            }
            rest = &after[digits..];
        }
        out.push_str(rest);
        out
    }

    /// Port of `CrateDriver.formatParam`.
    ///
    /// JSON has no `Date`: timestamps arrive as ISO strings, which the string
    /// branch quotes exactly like the Node `Date` branch does.
    pub fn format_param(value: &Value) -> String {
        match value {
            Value::Null => "NULL".to_string(),
            Value::Number(n) => n.to_string(),
            Value::Bool(true) => "TRUE".to_string(),
            Value::Bool(false) => "FALSE".to_string(),
            other => format!("'{}'", js_string(other).replace('\'', "''")),
        }
    }
}

/// `String(value)` for the non-primitive branch of `formatParam`.
fn js_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        // `String([1, null, 'a'])` is `1,,a`.
        Value::Array(items) => items.iter().map(js_string).collect::<Vec<_>>().join(","),
        Value::Object(_) => "[object Object]".to_string(),
    }
}

#[async_trait]
impl Driver for CrateDriver {
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

    async fn tables_schema(&self) -> Result<DatabaseStructure> {
        self.inner.tables_schema().await
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

    async fn load_pre_aggregation_into_table(
        &self,
        pre_aggregation_table_name: &str,
        load_sql: &str,
        params: &[Value],
        options: &QueryOptions,
    ) -> Result<QueryResult> {
        let result = self
            .inner
            .load_pre_aggregation_into_table(
                pre_aggregation_table_name,
                &Self::inline_params(load_sql, params),
                &[],
                options,
            )
            .await?;
        // CrateDB is eventually consistent: the rows written by the
        // `CREATE TABLE AS` are only visible to reads after a refresh.
        self.query(
            &format!("REFRESH TABLE {pre_aggregation_table_name}"),
            &[],
            &QueryOptions::default(),
        )
        .await?;
        Ok(result)
    }

    async fn stream(
        &self,
        sql: &str,
        params: &[Value],
        options: &StreamOptions,
    ) -> Result<StreamTableData> {
        self.inner.stream(sql, params, options).await
    }

    async fn download_query_results(
        &self,
        sql: &str,
        params: &[Value],
        options: &DownloadQueryResultsOptions,
    ) -> Result<DownloadedData> {
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

    async fn upload_table_with_indexes(
        &self,
        table: &str,
        columns: &[Column],
        table_data: &TableMemoryData,
        indexes_sql: &[IndexSql],
        unique_key_columns: &[String],
        external_options: &ExternalCreateTableOptions,
    ) -> Result<()> {
        self.inner
            .upload_table_with_indexes(
                table,
                columns,
                table_data,
                indexes_sql,
                unique_key_columns,
                external_options,
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Port of `test/CrateDriver.unit.test.ts`.

    #[test]
    #[allow(clippy::approx_constant)] // `3.14` as in the Node test
    fn format_param_formats_values_by_type() {
        // strings are quoted, single quotes doubled
        assert_eq!(CrateDriver::format_param(&json!("abc")), "'abc'");
        assert_eq!(CrateDriver::format_param(&json!("O'Brien")), "'O''Brien'");
        assert_eq!(CrateDriver::format_param(&json!("a'b'c")), "'a''b''c'");

        assert_eq!(CrateDriver::format_param(&json!(42)), "42");
        assert_eq!(CrateDriver::format_param(&json!(3.14)), "3.14");
        assert_eq!(CrateDriver::format_param(&json!(0)), "0");

        assert_eq!(CrateDriver::format_param(&json!(true)), "TRUE");
        assert_eq!(CrateDriver::format_param(&json!(false)), "FALSE");

        assert_eq!(CrateDriver::format_param(&Value::Null), "NULL");
        // A `Date` reaches the Rust driver as its ISO string.
        assert_eq!(
            CrateDriver::format_param(&json!("2020-01-01T00:00:00.000Z")),
            "'2020-01-01T00:00:00.000Z'"
        );
        // `String(value)` for arrays and objects
        assert_eq!(CrateDriver::format_param(&json!([1, "a"])), "'1,a'");
        assert_eq!(
            CrateDriver::format_param(&json!({"a": 1})),
            "'[object Object]'"
        );
    }

    #[test]
    fn inline_params_returns_the_sql_unchanged_without_params() {
        assert_eq!(CrateDriver::inline_params("SELECT 1", &[]), "SELECT 1");
        assert_eq!(CrateDriver::inline_params("SELECT $1", &[]), "SELECT $1");
    }

    #[test]
    fn inline_params_keeps_a_trailing_cast() {
        assert_eq!(
            CrateDriver::inline_params(
                "... WHERE d >= $1::timestamptz",
                &[json!("2020-01-01T00:00:00.000Z")]
            ),
            "... WHERE d >= '2020-01-01T00:00:00.000Z'::timestamptz"
        );
    }

    #[test]
    fn inline_params_by_position() {
        assert_eq!(
            CrateDriver::inline_params("a = $1 AND b = $2", &[json!("x"), json!(7)]),
            "a = 'x' AND b = 7"
        );
    }

    #[test]
    fn inline_params_handles_multi_digit_placeholders() {
        let params: Vec<Value> = (1..=10).map(|i| json!(i)).collect();
        assert_eq!(CrateDriver::inline_params("v = $10", &params), "v = 10");
    }

    #[test]
    fn inline_params_escapes_single_quotes() {
        assert_eq!(
            CrateDriver::inline_params("name = $1", &[json!("O'Brien")]),
            "name = 'O''Brien'"
        );
    }

    #[test]
    fn inline_params_leaves_unmatched_placeholders() {
        assert_eq!(
            CrateDriver::inline_params("a = $1 AND b = $2", &[json!("only")]),
            "a = 'only' AND b = $2"
        );
        assert_eq!(
            CrateDriver::inline_params("a = $0 AND b = $ AND c = $1$", &[json!(1)]),
            "a = $0 AND b = $ AND c = 1$"
        );
    }

    #[test]
    fn inline_params_for_partitioned_ctas() {
        let sql = "CREATE TABLE pa AS (SELECT sum(\"t\".v) FROM t AS \"t\" \
                   WHERE (\"t\".d >= $1::timestamptz AND \"t\".d <= $2::timestamptz) GROUP BY 1)";
        let out = CrateDriver::inline_params(
            sql,
            &[
                json!("2020-01-01T00:00:00.000Z"),
                json!("2020-01-31T23:59:59.999Z"),
            ],
        );
        assert!(!out.contains("$1") && !out.contains("$2"));
        assert!(out.contains("'2020-01-01T00:00:00.000Z'::timestamptz"));
        assert!(out.contains("'2020-01-31T23:59:59.999Z'::timestamptz"));
    }

    #[tokio::test]
    async fn inherits_postgres_behaviour() {
        let driver =
            CrateDriver::new(CrateConfig::from_driver_config(DriverConfig::default())).unwrap();
        assert_eq!(driver.param(0), "$1");
        assert_eq!(driver.quote_identifier("a"), "\"a\"");
        assert!(driver.read_only());
        assert!(driver.capabilities().incremental_schema_loading);
        assert_eq!(driver.from_generic_type(&GenericType::Int), "int8");
        assert_eq!(
            driver.wrap_query_with_limit("SELECT 1", 5),
            crate::sql::wrap_query_with_limit("SELECT 1", 5)
        );
        assert_eq!(DEFAULT_CONCURRENCY, 2);
        driver.release().await.unwrap();
    }
}
