//! The [`Driver`] trait: Rust counterpart of `DriverInterface` + `BaseDriver`.
//!
//! Methods with a body are the `BaseDriver` defaults; drivers override the
//! ones they specialise (exactly like subclasses of `BaseDriver` do).

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde_json::Value;

use crate::config::DriverConfig;
use crate::error::{DriverError, Result};
use crate::sql;
use crate::type_detection::detect_types_from_tabular;
use crate::types::{
    Column, ColumnInfo, DatabaseStructure, DownloadQueryResultsOptions, DownloadTableOptions,
    DownloadedData, DriverCapabilities, ExternalCreateTableOptions, ForeignKey, ForeignKeyInfo,
    GenericType, IndexSql, PrimaryKeyInfo, QueryOptions, QueryResult, SchemaColumn, SchemaName,
    SchemaTable, StreamOptions, StreamTableData, TableCsvData, TableMemoryData, TableName,
    TableStructure, UnloadOptions,
};

/// A database driver. Mirrors `DriverInterface` from
/// `@cubejs-backend/base-driver`; see the module docs for the default-method
/// convention.
#[async_trait]
pub trait Driver: Send + Sync {
    // ------------------------------------------------------------------
    // Required
    // ------------------------------------------------------------------

    /// Driver-wide configuration (used by the default implementations).
    fn config(&self) -> &DriverConfig;

    /// Checks that the database is reachable with the configured credentials.
    async fn test_connection(&self) -> Result<()>;

    /// Executes `sql` with positional `params` and returns all rows.
    async fn query(
        &self,
        sql: &str,
        params: &[Value],
        options: &QueryOptions,
    ) -> Result<QueryResult>;

    // ------------------------------------------------------------------
    // Simple defaults
    // ------------------------------------------------------------------

    /// Placeholder for the parameter at `index` (`?` in the base driver).
    fn param(&self, _index: usize) -> String {
        "?".to_string()
    }

    /// `"identifier"`.
    fn quote_identifier(&self, identifier: &str) -> String {
        sql::quote_identifier(identifier)
    }

    /// Maps a database type to Cube's generic type
    /// (`BaseDriver.toGenericType`).
    fn to_generic_type(
        &self,
        db_type: &str,
        precision: Option<i64>,
        scale: Option<i64>,
    ) -> GenericType {
        crate::types::to_generic_type(
            db_type,
            precision,
            scale,
            self.config().precise_decimal_in_cubestore,
        )
    }

    /// Maps a generic type to the database type used in `CREATE TABLE`.
    #[allow(clippy::wrong_self_convention)]
    fn from_generic_type(&self, generic: &GenericType) -> String {
        generic.to_string()
    }

    /// `true` when the driver may only read (no pre-aggregation tables).
    fn read_only(&self) -> bool {
        false
    }

    /// Driver capabilities advertised to the orchestrator.
    fn capabilities(&self) -> DriverCapabilities {
        DriverCapabilities::default()
    }

    /// Current time in milliseconds since the Unix epoch (`Date.now()`).
    fn now_timestamp(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    /// `SELECT * FROM (<query>) AS t LIMIT <limit>`.
    fn wrap_query_with_limit(&self, query: &str, limit: u64) -> String {
        sql::wrap_query_with_limit(query, limit)
    }

    /// `testConnectionTimeout` option.
    fn test_connection_timeout(&self) -> Duration {
        self.config().test_connection_timeout
    }

    /// Releases pooled connections. Must be idempotent.
    async fn release(&self) -> Result<()> {
        Ok(())
    }

    // ------------------------------------------------------------------
    // Query text hooks
    // ------------------------------------------------------------------

    fn information_schema_query(&self) -> String {
        sql::information_schema_query(&|i| self.quote_identifier(i))
    }

    fn get_schemas_query(&self) -> String {
        sql::get_schemas_query(&|i| self.quote_identifier(i))
    }

    fn get_tables_for_specific_schemas_query(&self, schemas_placeholders: &str) -> String {
        sql::get_tables_for_specific_schemas_query(
            &|i| self.quote_identifier(i),
            schemas_placeholders,
        )
    }

    fn get_columns_for_specific_tables_query(&self, condition_string: &str) -> String {
        sql::get_columns_for_specific_tables_query(&|i| self.quote_identifier(i), condition_string)
    }

    /// Query returning `table_schema, table_name, column_name` of primary keys
    /// (optionally filtered by `condition`). `None` when unsupported.
    fn primary_keys_query(&self, _condition: Option<&str>) -> Option<String> {
        None
    }

    /// Query returning foreign keys (see [`ForeignKeyInfo`]). `None` when unsupported.
    fn foreign_keys_query(&self, _condition: Option<&str>) -> Option<String> {
        None
    }

    fn column_name_for_schema_name(&self) -> String {
        "columns.table_schema".to_string()
    }

    fn column_name_for_table_name(&self) -> String {
        "columns.table_name".to_string()
    }

    /// Converts an uploaded value for the target column type
    /// (`BaseDriver.toColumnValue`, identity by default).
    fn to_column_value(&self, value: &Value, _generic_type: &GenericType) -> Value {
        value.clone()
    }

    /// `BaseDriver.createTableSql`.
    fn create_table_sql(&self, quoted_table_name: &str, columns: &[Column]) -> String {
        sql::create_table_sql(
            quoted_table_name,
            columns,
            &|i| self.quote_identifier(i),
            &|t| self.from_generic_type(t),
        )
    }

    // ------------------------------------------------------------------
    // Streaming / unload (optional in `DriverInterface`)
    // ------------------------------------------------------------------

    /// Streams the rows of `sql`.
    async fn stream(
        &self,
        _sql: &str,
        _params: &[Value],
        _options: &StreamOptions,
    ) -> Result<StreamTableData> {
        Err(DriverError::NotImplemented(
            "Driver's .stream() method is not implemented yet.".to_string(),
        ))
    }

    /// Unloads `table` to an export bucket.
    async fn unload(&self, _table: &str, _options: &UnloadOptions) -> Result<TableCsvData> {
        Err(DriverError::NotImplemented(
            "Driver's .unload() method is not implemented.".to_string(),
        ))
    }

    /// Unloads the result of `sql` to an export bucket.
    async fn unload_from_query(
        &self,
        _sql: &str,
        _params: &[Value],
        _options: &UnloadOptions,
    ) -> Result<TableCsvData> {
        Err(DriverError::NotImplemented(
            "Driver's .unloadFromQuery() method is not implemented.".to_string(),
        ))
    }

    /// Whether the export bucket feature is configured.
    async fn is_unload_supported(&self, _options: &UnloadOptions) -> Result<bool> {
        Ok(false)
    }

    // ------------------------------------------------------------------
    // Introspection
    // ------------------------------------------------------------------

    /// Primary keys; failures are logged and yield an empty list
    /// (the model generator then falls back to heuristics).
    async fn primary_keys(&self, condition: Option<&str>, params: &[Value]) -> Vec<PrimaryKeyInfo> {
        let Some(query) = self.primary_keys_query(condition) else {
            return Vec::new();
        };
        match self.query(&query, params, &QueryOptions::default()).await {
            Ok(result) => (0..result.len())
                .filter_map(|i| {
                    Some(PrimaryKeyInfo {
                        table_schema: result.get_string(i, "table_schema")?,
                        table_name: result.get_string(i, "table_name")?,
                        column_name: result.get_string(i, "column_name")?,
                    })
                })
                .collect(),
            Err(e) => {
                log::warn!(
                    "Primary Keys Query failed. Primary Keys will be defined by heuristics: {e}"
                );
                Vec::new()
            }
        }
    }

    /// Foreign keys; failures are logged and yield an empty list.
    async fn foreign_keys(&self, condition: Option<&str>, params: &[Value]) -> Vec<ForeignKeyInfo> {
        let Some(query) = self.foreign_keys_query(condition) else {
            return Vec::new();
        };
        match self.query(&query, params, &QueryOptions::default()).await {
            Ok(result) => (0..result.len())
                .filter_map(|i| {
                    Some(ForeignKeyInfo {
                        table_schema: result.get_string(i, "table_schema")?,
                        table_name: result.get_string(i, "table_name")?,
                        column_name: result.get_string(i, "column_name")?,
                        target_table: result.get_string(i, "target_table")?,
                        target_column: result.get_string(i, "target_column")?,
                    })
                })
                .collect(),
            Err(e) => {
                log::warn!("Foreign Keys Query failed. Joins will be defined by heuristics: {e}");
                Vec::new()
            }
        }
    }

    /// Whole database structure: schema → table → columns.
    async fn tables_schema(&self) -> Result<DatabaseStructure> {
        let query = self.information_schema_query();
        let data = self.query(&query, &[], &QueryOptions::default()).await?;
        Ok(information_columns_to_structure(&data))
    }

    /// `tablesSchema` enriched with primary and foreign keys.
    async fn tables_schema_v2(&self) -> Result<DatabaseStructure> {
        let mut structure = self.tables_schema().await?;
        let (primary_keys, foreign_keys) =
            futures::join!(self.primary_keys(None, &[]), self.foreign_keys(None, &[]));

        for pk in primary_keys {
            if let Some(columns) = structure
                .get_mut(&pk.table_schema)
                .and_then(|s| s.get_mut(&pk.table_name))
            {
                for c in columns.iter_mut().filter(|c| c.name == pk.column_name) {
                    c.attributes = vec!["primaryKey".to_string()];
                }
            }
        }

        for fk in foreign_keys {
            if let Some(columns) = structure
                .get_mut(&fk.table_schema)
                .and_then(|s| s.get_mut(&fk.table_name))
            {
                for c in columns.iter_mut().filter(|c| c.name == fk.column_name) {
                    c.foreign_keys.push(ForeignKey {
                        target_table: fk.target_table.clone(),
                        target_column: fk.target_column.clone(),
                    });
                }
            }
        }

        Ok(structure)
    }

    /// `CREATE SCHEMA IF NOT EXISTS` unless the schema already exists.
    async fn create_schema_if_not_exists(&self, schema_name: &str) -> Result<()> {
        let existing = self
            .query(
                &format!(
                    "SELECT schema_name FROM information_schema.schemata WHERE schema_name = {}",
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

    /// All user schemas.
    async fn get_schemas(&self) -> Result<Vec<SchemaName>> {
        let query = self.get_schemas_query();
        let result = self.query(&query, &[], &QueryOptions::default()).await?;
        Ok((0..result.len())
            .filter_map(|i| {
                Some(SchemaName {
                    schema_name: result.get_string(i, "schema_name")?,
                })
            })
            .collect())
    }

    /// Tables of the given schemas.
    async fn get_tables_for_specific_schemas(
        &self,
        schemas: &[SchemaName],
    ) -> Result<Vec<SchemaTable>> {
        let placeholders = (0..schemas.len())
            .map(|i| self.param(i))
            .collect::<Vec<_>>()
            .join(", ");
        let params: Vec<Value> = schemas
            .iter()
            .map(|s| Value::from(s.schema_name.as_str()))
            .collect();
        let query = self.get_tables_for_specific_schemas_query(&placeholders);
        let result = self
            .query(&query, &params, &QueryOptions::default())
            .await?;
        Ok((0..result.len())
            .filter_map(|i| {
                Some(SchemaTable {
                    schema_name: result.get_string(i, "schema_name")?,
                    table_name: result.get_string(i, "table_name")?,
                })
            })
            .collect())
    }

    /// Columns (with keys) of the given tables.
    async fn get_columns_for_specific_tables(
        &self,
        tables: &[SchemaTable],
    ) -> Result<Vec<ColumnInfo>> {
        let (condition, parameters) = sql::columns_for_specific_tables_condition(
            tables,
            &|i| self.param(i),
            &self.column_name_for_schema_name(),
            &self.column_name_for_table_name(),
        );
        let params: Vec<Value> = parameters.into_iter().map(Value::from).collect();
        let query = self.get_columns_for_specific_tables_query(&condition);

        let (primary_keys, foreign_keys) = futures::join!(
            self.primary_keys(Some(&condition), &params),
            self.foreign_keys(Some(&condition), &params)
        );

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

        for column in columns.iter_mut() {
            if primary_keys.iter().any(|pk| {
                pk.table_schema == column.schema_name
                    && pk.table_name == column.table_name
                    && pk.column_name == column.column_name
            }) {
                column.attributes = vec!["primaryKey".to_string()];
            }
            column.foreign_keys = foreign_keys
                .iter()
                .filter(|fk| {
                    fk.table_schema == column.schema_name
                        && fk.table_name == column.table_name
                        && fk.column_name == column.column_name
                })
                .map(|fk| ForeignKey {
                    target_table: fk.target_table.clone(),
                    target_column: fk.target_column.clone(),
                })
                .collect();
        }

        Ok(columns)
    }

    /// Table names of `schema_name`.
    async fn get_tables_query(&self, schema_name: &str) -> Result<Vec<String>> {
        let result = self
            .query(
                &format!(
                    "SELECT table_name FROM information_schema.tables WHERE table_schema = {}",
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

    // ------------------------------------------------------------------
    // Pre-aggregations
    // ------------------------------------------------------------------

    /// Builds a pre-aggregation table by running `load_sql`.
    async fn load_pre_aggregation_into_table(
        &self,
        _pre_aggregation_table_name: &str,
        load_sql: &str,
        params: &[Value],
        options: &QueryOptions,
    ) -> Result<QueryResult> {
        self.query(load_sql, params, options).await
    }

    /// `DROP TABLE <table_name>`.
    async fn drop_table(&self, table_name: &str, options: &QueryOptions) -> Result<()> {
        self.query(&format!("DROP TABLE {table_name}"), &[], options)
            .await?;
        Ok(())
    }

    /// Downloads the result of `sql` (read-only pre-aggregations).
    async fn download_query_results(
        &self,
        sql: &str,
        params: &[Value],
        _options: &DownloadQueryResultsOptions,
    ) -> Result<DownloadedData> {
        let mut result = self.query(sql, params, &QueryOptions::default()).await?;
        result.columns = detect_types_from_tabular(&result)?;
        Ok(DownloadedData::Memory(result))
    }

    /// Downloads a whole table.
    async fn download_table(
        &self,
        table: &str,
        _options: &DownloadTableOptions,
    ) -> Result<TableMemoryData> {
        self.query(
            &format!("SELECT * FROM {table}"),
            &[],
            &QueryOptions::default(),
        )
        .await
    }

    /// Creates `table` and inserts `table_data` into it.
    async fn upload_table(
        &self,
        table: &str,
        columns: &[Column],
        table_data: &TableMemoryData,
    ) -> Result<()> {
        self.upload_table_with_indexes(
            table,
            columns,
            table_data,
            &[],
            &[],
            &ExternalCreateTableOptions::default(),
        )
        .await
    }

    /// Creates `table`, inserts `table_data` row by row and creates indexes.
    /// The table is dropped again when anything fails.
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

        let insert = sql::insert_row_sql(table, columns, &|i| self.quote_identifier(i), &|i| {
            self.param(i)
        });
        let upload = async {
            for row in &table_data.rows {
                let params: Vec<Value> = columns
                    .iter()
                    .map(|c| {
                        self.to_column_value(&column_value(table_data, columns, row, c), &c.type_)
                    })
                    .collect();
                self.query(&insert, &params, &QueryOptions::default())
                    .await?;
            }
            for index in indexes_sql {
                self.query(&index.sql, &index.params, &QueryOptions::default())
                    .await?;
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

    /// Uploads whatever `download_query_results` / `download_table` produced,
    /// including an export-bucket CSV download.
    ///
    /// This is the entry point the pre-aggregation build uses: the source
    /// driver may hand over rows *or* CSV files (`externalCaps.csvImport &&
    /// isUnloadSupported` in `PreAggregations.ts`). External drivers that can
    /// import CSV natively — Cube Store does, through `CREATE TABLE …
    /// LOCATION` — override this; the default materialises the download into
    /// memory (downloading and parsing the files if needed) and falls back to
    /// [`Driver::upload_table_with_indexes`].
    ///
    /// The download is taken by value: a [`DownloadedData::Stream`] can only
    /// be consumed once, and `&DownloadedData` is not `Send` because of it.
    async fn upload_downloaded_table_with_indexes(
        &self,
        table: &str,
        columns: &[Column],
        table_data: DownloadedData,
        indexes_sql: &[IndexSql],
        unique_key_columns: &[String],
        external_options: &ExternalCreateTableOptions,
    ) -> Result<()> {
        // `into_memory` collects a stream and downloads + parses CSV files.
        let memory = table_data.into_memory().await?;
        self.upload_table_with_indexes(
            table,
            columns,
            &memory,
            indexes_sql,
            unique_key_columns,
            external_options,
        )
        .await
    }

    /// [`Driver::upload_downloaded_table_with_indexes`] without indexes.
    async fn upload_downloaded_table(
        &self,
        table: &str,
        columns: &[Column],
        table_data: DownloadedData,
    ) -> Result<()> {
        self.upload_downloaded_table_with_indexes(
            table,
            columns,
            table_data,
            &[],
            &[],
            &ExternalCreateTableOptions::default(),
        )
        .await
    }

    /// Generic column types of `table` (`schema.table`).
    async fn table_column_types(&self, table: &str) -> Result<TableStructure> {
        self.table_column_types_impl(table, false).await
    }

    /// Like [`Driver::table_column_types`] but honours numeric precision/scale.
    async fn table_column_types_with_precision(&self, table: &str) -> Result<TableStructure> {
        self.table_column_types_impl(table, true).await
    }

    /// Shared implementation of the two `table_column_types*` methods.
    async fn table_column_types_impl(
        &self,
        table: &str,
        with_precision: bool,
    ) -> Result<TableStructure> {
        let TableName { schema, name } = TableName::split(table);
        // JS: `const [schema, name] = table.split('.')` keeps only the 2nd part.
        let name = name.split('.').next().unwrap_or("").to_string();
        let query = sql::table_column_types_query(
            &|i| self.quote_identifier(i),
            &|i| self.param(i),
            self.config().fetch_columns_by_ordinal_position,
            with_precision,
        );
        let result = self
            .query(
                &query,
                &[Value::from(name), Value::from(schema)],
                &QueryOptions::default(),
            )
            .await?;
        Ok((0..result.len())
            .filter_map(|i| {
                let name = result.get_string(i, "column_name")?;
                let data_type = result.get_string(i, "data_type")?;
                let (precision, scale) = if with_precision {
                    (
                        result.get_i64(i, "numeric_precision"),
                        result.get_i64(i, "numeric_scale"),
                    )
                } else {
                    (None, None)
                };
                Some(Column::new(
                    name,
                    self.to_generic_type(&data_type, precision, scale),
                ))
            })
            .collect())
    }

    /// Column types of an arbitrary query (empty when the driver cannot tell).
    async fn query_column_types(
        &self,
        _sql: &str,
        _params: &[Value],
        _options: &QueryOptions,
    ) -> Result<TableStructure> {
        Ok(Vec::new())
    }

    /// `CREATE TABLE` with the generic column types mapped by
    /// [`Driver::from_generic_type`].
    async fn create_table(&self, quoted_table_name: &str, columns: &[Column]) -> Result<()> {
        let create_sql = self.create_table_sql(quoted_table_name, columns);
        self.query(&create_sql, &[], &QueryOptions::default())
            .await
            .map_err(|e| {
                DriverError::Query(format!("Error during create table: {create_sql}: {e}"))
            })?;
        Ok(())
    }
}

/// Looks up the value of column `c` in `row`, by name when `table_data`
/// carries column metadata, positionally otherwise.
fn column_value(
    table_data: &TableMemoryData,
    columns: &[Column],
    row: &[Value],
    c: &Column,
) -> Value {
    let idx = if table_data.columns.is_empty() {
        columns.iter().position(|x| x.name == c.name)
    } else {
        table_data.column_index(&c.name)
    };
    idx.and_then(|i| row.get(i).cloned()).unwrap_or(Value::Null)
}

/// Port of `informationColumnsSchemaSorter` + `informationColumnsSchemaReducer`.
pub fn information_columns_to_structure(data: &QueryResult) -> DatabaseStructure {
    let mut rows: Vec<(String, String, String, String, bool)> = (0..data.len())
        .filter_map(|i| {
            Some((
                data.get_string(i, "table_schema")?,
                data.get_string(i, "table_name")?,
                data.get_string(i, "column_name")?,
                data.get_string(i, "data_type")?,
                data.get_string(i, "key_type").is_some(),
            ))
        })
        .collect();
    rows.sort_by(|a, b| {
        let ka = format!("{}.{}.{}", a.0, a.1, a.2);
        let kb = format!("{}.{}.{}", b.0, b.1, b.2);
        ka.cmp(&kb)
    });

    let mut result: DatabaseStructure = BTreeMap::new();
    for (schema, table, column, data_type, is_key) in rows {
        result
            .entry(schema)
            .or_default()
            .entry(table)
            .or_default()
            .push(SchemaColumn {
                name: column,
                type_: data_type,
                attributes: if is_key {
                    vec!["primaryKey".to_string()]
                } else {
                    Vec::new()
                },
                foreign_keys: Vec::new(),
            });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Column;
    use std::sync::Mutex;

    /// Minimal driver recording the queries it receives and replying with
    /// canned results, to exercise the default implementations.
    struct FakeDriver {
        config: DriverConfig,
        queries: Mutex<Vec<(String, Vec<Value>)>>,
        responses: Mutex<Vec<QueryResult>>,
    }

    impl FakeDriver {
        fn new(responses: Vec<QueryResult>) -> Self {
            Self {
                config: DriverConfig::default(),
                queries: Mutex::new(Vec::new()),
                responses: Mutex::new(responses),
            }
        }

        fn queries(&self) -> Vec<(String, Vec<Value>)> {
            self.queries.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Driver for FakeDriver {
        fn config(&self) -> &DriverConfig {
            &self.config
        }

        async fn test_connection(&self) -> Result<()> {
            Ok(())
        }

        async fn query(
            &self,
            sql: &str,
            params: &[Value],
            _options: &QueryOptions,
        ) -> Result<QueryResult> {
            self.queries
                .lock()
                .unwrap()
                .push((sql.to_string(), params.to_vec()));
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                Ok(QueryResult::default())
            } else {
                Ok(responses.remove(0))
            }
        }
    }

    fn result(columns: &[&str], rows: Vec<Vec<&str>>) -> QueryResult {
        QueryResult::new(
            columns.iter().map(|c| Column::new(*c, "text")).collect(),
            rows.into_iter()
                .map(|r| r.into_iter().map(Value::from).collect())
                .collect(),
        )
    }

    #[tokio::test]
    async fn tables_schema_groups_and_sorts() {
        let driver = FakeDriver::new(vec![result(
            &["column_name", "table_name", "table_schema", "data_type"],
            vec![
                vec!["id", "orders", "public", "integer"],
                vec!["amount", "orders", "public", "numeric"],
                vec!["name", "users", "public", "character varying"],
                vec!["x", "t", "other", "text"],
            ],
        )]);
        let structure = driver.tables_schema().await.unwrap();
        let schemas: Vec<_> = structure.keys().cloned().collect();
        assert_eq!(schemas, vec!["other", "public"]);
        let orders = &structure["public"]["orders"];
        assert_eq!(orders.len(), 2);
        assert_eq!(orders[0].name, "amount");
        assert_eq!(orders[1].name, "id");
        assert_eq!(orders[1].type_, "integer");
        assert!(driver.queries()[0].0.contains("information_schema.columns"));
    }

    #[tokio::test]
    async fn create_schema_if_not_exists_checks_first() {
        let driver = FakeDriver::new(vec![QueryResult::default()]);
        driver.create_schema_if_not_exists("stb").await.unwrap();
        let queries = driver.queries();
        assert_eq!(queries.len(), 2);
        assert!(queries[0]
            .0
            .contains("information_schema.schemata WHERE schema_name = ?"));
        assert_eq!(queries[0].1, vec![Value::from("stb")]);
        assert_eq!(queries[1].0, "CREATE SCHEMA IF NOT EXISTS stb");

        let driver = FakeDriver::new(vec![result(&["schema_name"], vec![vec!["stb"]])]);
        driver.create_schema_if_not_exists("stb").await.unwrap();
        assert_eq!(driver.queries().len(), 1);
    }

    #[tokio::test]
    async fn get_columns_for_specific_tables_builds_condition() {
        let driver = FakeDriver::new(vec![result(
            &["column_name", "table_name", "schema_name", "data_type"],
            vec![vec!["id", "orders", "public", "integer"]],
        )]);
        let tables = vec![SchemaTable {
            schema_name: "public".into(),
            table_name: "orders".into(),
        }];
        let columns = driver
            .get_columns_for_specific_tables(&tables)
            .await
            .unwrap();
        assert_eq!(columns.len(), 1);
        assert_eq!(columns[0].column_name, "id");
        assert!(columns[0].attributes.is_empty());
        let queries = driver.queries();
        // no primary/foreign keys queries by default, only the columns query
        assert_eq!(queries.len(), 1);
        assert!(queries[0]
            .0
            .contains("(columns.table_schema = ? AND columns.table_name IN (?))"));
        assert_eq!(
            queries[0].1,
            vec![Value::from("public"), Value::from("orders")]
        );
    }

    #[tokio::test]
    async fn upload_table_inserts_rows_and_drops_on_failure() {
        let driver = FakeDriver::new(vec![]);
        let columns = vec![Column::new("id", "bigint"), Column::new("name", "text")];
        let data = QueryResult::new(
            vec![Column::new("name", "text"), Column::new("id", "bigint")],
            vec![vec![Value::from("a"), Value::from(1)]],
        );
        driver
            .upload_table("test.t", &columns, &data)
            .await
            .unwrap();
        let queries = driver.queries();
        assert_eq!(
            queries[0].0,
            r#"CREATE TABLE test.t ("id" bigint, "name" text)"#
        );
        assert!(queries[1].0.contains("INSERT INTO test.t"));
        // values are matched by column name, not position
        assert_eq!(queries[1].1, vec![Value::from(1), Value::from("a")]);
    }

    #[tokio::test]
    async fn table_column_types_maps_generic_types() {
        let driver = FakeDriver::new(vec![result(
            &["column_name", "table_name", "table_schema", "data_type"],
            vec![
                vec!["id", "t", "s", "integer"],
                vec!["ts", "t", "s", "timestamp without time zone"],
                vec!["u", "t", "s", "uuid"],
            ],
        )]);
        let types = driver.table_column_types("s.t").await.unwrap();
        assert_eq!(
            types,
            vec![
                Column::new("id", "int"),
                Column::new("ts", "timestamp"),
                Column::new("u", "uuid"),
            ]
        );
        let (sql, params) = &driver.queries()[0];
        assert!(sql.contains("ORDER BY columns.ordinal_position"));
        assert_eq!(params, &vec![Value::from("t"), Value::from("s")]);
    }

    #[tokio::test]
    async fn download_query_results_detects_types() {
        let driver = FakeDriver::new(vec![result(&["a", "b"], vec![vec!["1", "2020-01-01"]])]);
        let data = driver
            .download_query_results("SELECT 1", &[], &DownloadQueryResultsOptions::default())
            .await
            .unwrap();
        let DownloadedData::Memory(m) = data else {
            panic!("expected memory data");
        };
        assert_eq!(
            m.columns,
            vec![Column::new("a", "int"), Column::new("b", "date")]
        );
    }
}
