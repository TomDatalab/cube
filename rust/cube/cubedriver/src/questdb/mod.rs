//! QuestDB driver: port of `@cubejs-backend/questdb-driver`.
//!
//! QuestDB speaks the PostgreSQL wire protocol, and the Node driver talks to
//! it through a plain `pg` pool rather than through `PostgresDriver`, so this
//! driver only borrows the connection pool, parameter encoding and value
//! decoding of [`PostgresDriver`] and otherwise follows `QuestDriver.ts`:
//!
//! * no session setup on new connections (no `SET TIME ZONE`),
//! * pool size `CUBEJS_DB_MAX_POOL` or 4,
//! * `BaseDriver` type mapping, from the `pg-types` builtin OID names; an OID
//!   outside that list fails `downloadQueryResults` like in Node,
//! * no schemas: `createSchemaIfNotExists` is a no-op and `tablesSchema`
//!   reports every table under the empty schema name `''`,
//! * `SHOW TABLES` / `SHOW COLUMNS FROM '<table>'` for introspection,
//! * uploads insert row by row into a string-literal table name, then `COMMIT`.
//!
//! The QuestDB SQL dialect (`QuestQuery`) belongs to the schema compiler, not
//! to the driver.

use async_trait::async_trait;
use futures::TryStreamExt;
use serde_json::Value;

use crate::config::DriverConfig;
use crate::driver::Driver;
use crate::error::{DriverError, Result};
use crate::escape::{escape_string, ANSI};
use crate::postgres::{
    check_values_limit, ConnectionSetup, JsonCell, PostgresConfig, PostgresDriver, TextParam,
};
use crate::types::{
    Column, DatabaseStructure, DownloadQueryResultsOptions, DownloadedData,
    ExternalCreateTableOptions, IndexSql, QueryOptions, QueryResult, Row, TableMemoryData,
    TableStructure,
};

/// `getDefaultConcurrency`.
pub const DEFAULT_CONCURRENCY: usize = 2;
/// Pool size when neither `maxPoolSize` nor `CUBEJS_DB_MAX_POOL` is set.
pub const DEFAULT_MAX_POOL_SIZE: usize = 4;

/// `NativeTypeToQuestType`: `R.invertObj(pg.types.builtins)`, lower-cased.
pub fn native_type_to_quest_type(oid: u32) -> Option<&'static str> {
    Some(match oid {
        16 => "bool",
        17 => "bytea",
        18 => "char",
        20 => "int8",
        21 => "int2",
        23 => "int4",
        24 => "regproc",
        25 => "text",
        26 => "oid",
        27 => "tid",
        28 => "xid",
        29 => "cid",
        114 => "json",
        142 => "xml",
        194 => "pg_node_tree",
        210 => "smgr",
        602 => "path",
        604 => "polygon",
        650 => "cidr",
        700 => "float4",
        701 => "float8",
        702 => "abstime",
        703 => "reltime",
        704 => "tinterval",
        718 => "circle",
        774 => "macaddr8",
        790 => "money",
        829 => "macaddr",
        869 => "inet",
        1033 => "aclitem",
        1042 => "bpchar",
        1043 => "varchar",
        1082 => "date",
        1083 => "time",
        1114 => "timestamp",
        1184 => "timestamptz",
        1186 => "interval",
        1266 => "timetz",
        1560 => "bit",
        1562 => "varbit",
        1700 => "numeric",
        1790 => "refcursor",
        2202 => "regprocedure",
        2203 => "regoper",
        2204 => "regoperator",
        2205 => "regclass",
        2206 => "regtype",
        2950 => "uuid",
        2970 => "txid_snapshot",
        3220 => "pg_lsn",
        3361 => "pg_ndistinct",
        3402 => "pg_dependencies",
        3614 => "tsvector",
        3615 => "tsquery",
        3642 => "gtsvector",
        3734 => "regconfig",
        3769 => "regdictionary",
        3802 => "jsonb",
        4089 => "regnamespace",
        4096 => "regrole",
        _ => return None,
    })
}

/// Configuration of [`QuestDriver`] (`QuestDriverConfiguration`).
#[derive(Debug, Clone)]
pub struct QuestConfig {
    /// Connection settings; `read_only` defaults to `true`.
    pub postgres: PostgresConfig,
}

impl QuestConfig {
    /// Builds the configuration from a generic [`DriverConfig`].
    pub fn from_driver_config(driver: DriverConfig) -> Self {
        let mut postgres = PostgresConfig::from_driver_config(driver);
        // `config.maxPoolSize || getEnv('dbMaxPoolSize') || 4`
        postgres.max_pool_size = Some(
            postgres
                .driver
                .data_source
                .max_pool_size
                .filter(|v| *v > 0)
                .unwrap_or(DEFAULT_MAX_POOL_SIZE),
        );
        // `readOnly: true` (`getInitialConfiguration`).
        postgres.read_only = true;
        Self { postgres }
    }

    /// Reads the configuration of `data_source` from the environment.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Ok(Self::from_driver_config(DriverConfig::from_env(
            data_source,
        )?))
    }

    /// Reads the configuration from a connection URL, for tests and tooling.
    pub fn from_url(url: &str) -> Self {
        let mut driver = DriverConfig::default();
        driver.data_source.url = Some(url.to_string());
        Self::from_driver_config(driver)
    }
}

/// QuestDB driver.
pub struct QuestDriver {
    config: QuestConfig,
    inner: PostgresDriver,
}

impl std::fmt::Debug for QuestDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuestDriver")
            .field("pool", &self.inner)
            .finish()
    }
}

impl QuestDriver {
    /// Creates the driver and its (lazy) connection pool.
    pub fn new(config: QuestConfig) -> Result<Self> {
        let inner = PostgresDriver::new_with_setup(
            config.postgres.clone(),
            ConnectionSetup {
                // A plain `pg` pool: nothing is run on new connections.
                statements: Some(Vec::new()),
                application_name: None,
            },
        )?;
        Ok(Self { config, inner })
    }

    /// Creates the driver from `CUBEJS_DB_*`.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Self::new(QuestConfig::from_env(data_source)?)
    }

    /// The driver configuration.
    pub fn quest_config(&self) -> &QuestConfig {
        &self.config
    }

    /// `mapFields`: fails on an OID `pg-types` does not know, like Node.
    pub fn map_fields(&self, columns: &[tokio_postgres::Column]) -> Result<Vec<Column>> {
        columns
            .iter()
            .map(|c| {
                let oid = c.type_().oid();
                let quest_type = native_type_to_quest_type(oid).ok_or_else(|| {
                    DriverError::TypeDetection(format!(
                        "Unable to detect type for field \"{}\" with dataTypeID: {oid}",
                        c.name()
                    ))
                })?;
                Ok(Column::new(
                    c.name(),
                    self.to_generic_type(quest_type, None, None),
                ))
            })
            .collect()
    }

    /// `queryResponse`: rows and the prepared statement's columns. With
    /// `strict_types`, an unknown OID fails (`downloadQueryResults`);
    /// otherwise its column carries the server's type name (`query`, which
    /// in Node does not map the fields at all).
    async fn query_response(
        &self,
        sql: &str,
        params: &[Value],
        strict_types: bool,
    ) -> Result<QueryResult> {
        check_values_limit(params)?;
        let client = self.inner.pooled_client().await?;
        let statement = client.prepare(sql).await?;
        let columns = if strict_types {
            self.map_fields(statement.columns())?
        } else {
            statement
                .columns()
                .iter()
                .map(|c| {
                    let name = native_type_to_quest_type(c.type_().oid())
                        .map(str::to_string)
                        .unwrap_or_else(|| c.type_().name().to_string());
                    Column::new(c.name(), self.to_generic_type(&name, None, None))
                })
                .collect()
        };
        let width = columns.len();
        let text_params: Vec<TextParam> = params.iter().map(TextParam::from_json).collect();
        let rows = client
            .query_raw(&statement, text_params.iter())
            .await?
            .map_err(DriverError::from)
            .and_then(|row| {
                futures::future::ready(
                    (0..width)
                        .map(|i| {
                            row.try_get::<usize, JsonCell>(i)
                                .map(|c| c.0)
                                .map_err(|e| DriverError::TypeDetection(e.to_string()))
                        })
                        .collect::<Result<Row>>(),
                )
            })
            .try_collect::<Vec<Row>>()
            .await?;
        Ok(QueryResult::new(columns, rows))
    }

    /// `escapeStringLiteral` (ANSI: quotes doubled, backslashes kept).
    pub fn escape_string_literal(value: &str) -> String {
        escape_string(ANSI, value)
    }
}

#[async_trait]
impl Driver for QuestDriver {
    fn config(&self) -> &DriverConfig {
        self.inner.config()
    }

    /// `SELECT $1 AS number` with `['1']` on a pooled connection.
    async fn test_connection(&self) -> Result<()> {
        self.query(
            "SELECT $1 AS number",
            &[Value::from("1")],
            &QueryOptions::default(),
        )
        .await?;
        Ok(())
    }

    async fn query(
        &self,
        sql: &str,
        params: &[Value],
        _options: &QueryOptions,
    ) -> Result<QueryResult> {
        self.query_response(sql, params, false).await
    }

    fn param(&self, index: usize) -> String {
        format!("${}", index + 1)
    }

    fn read_only(&self) -> bool {
        self.config.postgres.read_only
    }

    async fn release(&self) -> Result<()> {
        self.inner.release().await
    }

    async fn download_query_results(
        &self,
        sql: &str,
        params: &[Value],
        _options: &DownloadQueryResultsOptions,
    ) -> Result<DownloadedData> {
        Ok(DownloadedData::Memory(
            self.query_response(sql, params, true).await?,
        ))
    }

    /// No-op: QuestDB has no schemas.
    async fn create_schema_if_not_exists(&self, _schema_name: &str) -> Result<()> {
        Ok(())
    }

    /// Every table under the empty schema name, so that no `schema.` prefix
    /// ends up in generated queries. System tables (`sys.*`) are skipped.
    async fn tables_schema(&self) -> Result<DatabaseStructure> {
        let tables = self.get_tables_query("").await?;
        let mut tables_of_schema = std::collections::BTreeMap::new();
        let columns = futures::future::try_join_all(
            tables
                .iter()
                .filter(|t| !t.starts_with("sys."))
                .map(|t| async move {
                    Ok::<_, DriverError>((t.clone(), self.table_column_types(t).await?))
                }),
        )
        .await?;
        for (table, columns) in columns {
            tables_of_schema.insert(
                table,
                columns
                    .into_iter()
                    .map(|c| crate::types::SchemaColumn {
                        name: c.name,
                        type_: c.type_.to_string(),
                        attributes: Vec::new(),
                        foreign_keys: Vec::new(),
                    })
                    .collect(),
            );
        }
        let mut structure = DatabaseStructure::new();
        structure.insert(String::new(), tables_of_schema);
        Ok(structure)
    }

    /// `SHOW TABLES`.
    async fn get_tables_query(&self, _schema_name: &str) -> Result<Vec<String>> {
        let result = self
            .query("SHOW TABLES", &[], &QueryOptions::default())
            .await?;
        Ok((0..result.len())
            .filter_map(|i| {
                result
                    .get_string(i, "table_name")
                    .or_else(|| result.get_string(i, "TABLE_NAME"))
            })
            .collect())
    }

    /// `SHOW COLUMNS FROM '<table>'`, mapped with `BaseDriver.toGenericType`.
    async fn table_column_types(&self, table: &str) -> Result<TableStructure> {
        let result = self
            .query(
                &format!("SHOW COLUMNS FROM {}", Self::escape_string_literal(table)),
                &[],
                &QueryOptions::default(),
            )
            .await?;
        Ok((0..result.len())
            .filter_map(|i| {
                let name = result.get_string(i, "column")?;
                let db_type = result.get_string(i, "type")?;
                Some(Column::new(
                    name,
                    self.to_generic_type(&db_type, None, None),
                ))
            })
            .collect())
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

        let insert = format!(
            "INSERT INTO {}
        ({})
        VALUES ({})",
            Self::escape_string_literal(table),
            columns
                .iter()
                .map(|c| self.quote_identifier(&c.name))
                .collect::<Vec<_>>()
                .join(", "),
            (0..columns.len())
                .map(|i| self.param(i))
                .collect::<Vec<_>>()
                .join(", "),
        );
        let upload = async {
            for row in &table_data.rows {
                let params: Vec<Value> = columns
                    .iter()
                    .map(|c| {
                        let value = table_data
                            .column_index(&c.name)
                            .and_then(|i| row.get(i).cloned())
                            .unwrap_or(Value::Null);
                        self.to_column_value(&value, &c.type_)
                    })
                    .collect();
                self.query(&insert, &params, &QueryOptions::default())
                    .await?;
            }
            // Make the rows visible to later queries.
            self.query("COMMIT", &[], &QueryOptions::default()).await?;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::GenericType;

    fn driver() -> QuestDriver {
        QuestDriver::new(QuestConfig::from_driver_config(DriverConfig::default())).unwrap()
    }

    #[tokio::test]
    async fn defaults() {
        let d = driver();
        assert!(d.read_only());
        assert_eq!(d.param(0), "$1");
        assert_eq!(d.param(9), "$10");
        assert_eq!(d.quote_identifier("a"), "\"a\"");
        assert_eq!(d.quest_config().postgres.pool_size(), 4);
        // `BaseDriver` capabilities: no incremental schema loading.
        assert!(!d.capabilities().incremental_schema_loading);
        assert_eq!(DEFAULT_CONCURRENCY, 2);
        d.release().await.unwrap();

        let mut config = DriverConfig::default();
        config.data_source.max_pool_size = Some(12);
        assert_eq!(
            QuestConfig::from_driver_config(config).postgres.pool_size(),
            12
        );
    }

    #[tokio::test]
    async fn type_mapping_is_base_drivers() {
        let d = driver();
        // `SHOW COLUMNS` type names
        assert_eq!(
            d.to_generic_type("LONG", None, None),
            GenericType::Other("LONG".into())
        );
        assert_eq!(d.to_generic_type("DATE", None, None), GenericType::Date);
        // `timestamp` itself has no `DbTypeToGenericType` entry: passed through
        assert_eq!(
            d.to_generic_type("TIMESTAMP", None, None),
            GenericType::Other("TIMESTAMP".into())
        );
        // OID names, through `BaseDriver.toGenericType`
        assert_eq!(d.to_generic_type("int8", None, None), GenericType::Bigint);
        assert_eq!(d.to_generic_type("float8", None, None), GenericType::Double);
        // Not PostgreSQL's `bpchar → varchar`
        assert_eq!(
            d.to_generic_type("bpchar", None, None),
            GenericType::Other("bpchar".into())
        );
        // `fromGenericType` is the identity
        assert_eq!(d.from_generic_type(&GenericType::Int), "int");
        assert_eq!(
            d.create_table_sql("t", &[Column::new("id", "long")]),
            "CREATE TABLE t (\"id\" long)"
        );
        d.release().await.unwrap();
    }

    #[test]
    fn native_types() {
        assert_eq!(native_type_to_quest_type(20), Some("int8"));
        assert_eq!(native_type_to_quest_type(1114), Some("timestamp"));
        assert_eq!(native_type_to_quest_type(1043), Some("varchar"));
        // arrays are not `pg-types` builtins
        assert_eq!(native_type_to_quest_type(1016), None);
    }

    #[test]
    fn string_literals() {
        assert_eq!(QuestDriver::escape_string_literal("t"), "'t'");
        assert_eq!(QuestDriver::escape_string_literal("o'k\\"), "'o''k\\'");
    }
}
