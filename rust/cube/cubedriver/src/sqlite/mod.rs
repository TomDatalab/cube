//! SQLite driver: port of `@cubejs-backend/sqlite-driver` on `rusqlite`.
//!
//! # Native engine
//!
//! SQLite is an embedded C library, so there is no wire protocol to speak in
//! pure Rust. The `sqlite` Cargo feature enables `rusqlite` with its `bundled`
//! feature, which compiles the SQLite amalgamation into the binary with the C
//! compiler. This is the same trade-off the Node driver makes: the `sqlite3`
//! npm package links the same C engine through a native addon. No JavaScript
//! and no system `libsqlite3` are involved.
//!
//! # Behaviour (same as the Node driver)
//!
//! * `CUBEJS_DB_NAME` is the database file (`:memory:` for an in-memory
//!   database). It is required: the Node driver hands it to
//!   `new sqlite3.Database(...)`, which rejects a missing file name.
//! * One connection per driver (Node: one `sqlite3.Database`). SQLite
//!   serialises writers anyway and an in-memory database only exists inside
//!   its connection, so there is no pool: calls are serialised by a mutex and
//!   run on Tokio's blocking pool.
//! * Parameters are positional `?`; identifiers are quoted with `"`.
//! * Rows keep SQLite's storage classes, like `node-sqlite3`: `INTEGER` is a
//!   JSON number, `REAL` a JSON number, `TEXT` a string, `NULL` null and a
//!   `BLOB` the JSON form of a Node `Buffer` (`{"type":"Buffer","data":[..]}`).
//! * `tablesSchema` reads `sqlite_master` + `pragma_table_info` and reports
//!   every table under the `main` schema.
//! * `createSchemaIfNotExists` attaches a database named after the schema;
//!   `getTablesQuery` lists the tables of an attached database.
//! * `getDefaultConcurrency` is 2 ([`DEFAULT_CONCURRENCY`]).
//! * Everything else is `BaseDriver`'s default (`information_schema` based
//!   introspection, which SQLite does not have, fails exactly as in Node).
//!
//! # Deliberate differences
//!
//! * `tableColumnsQuery` interpolates the table name into a string literal;
//!   the port escapes `'` so that a table name cannot break out of it.
//! * After [`Driver::release`] every call fails with
//!   `SQLITE_MISUSE: Database is closed` (the message `node-sqlite3` gives).

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rusqlite::types::{Value as SqlValue, ValueRef};
use serde_json::{json, Value};

use crate::config::DriverConfig;
use crate::driver::Driver;
use crate::error::{DriverError, Result};
use crate::types::{Column, DatabaseStructure, QueryOptions, QueryResult, SchemaColumn};

/// `getDefaultConcurrency`.
pub const DEFAULT_CONCURRENCY: usize = 2;

/// Pool name used in connection errors.
const POOL_NAME: &str = "sqlite";

/// Message of every call made after [`Driver::release`].
pub const DATABASE_CLOSED: &str = "SQLITE_MISUSE: Database is closed";

/// Configuration of [`SqliteDriver`].
#[derive(Debug, Clone)]
pub struct SqliteConfig {
    /// Global driver knobs.
    pub driver: DriverConfig,
    /// `CUBEJS_DB_NAME`: database file, or `:memory:`.
    pub database: Option<String>,
}

impl SqliteConfig {
    /// Builds the configuration from a generic [`DriverConfig`]
    /// (`CUBEJS_DB_NAME`).
    pub fn from_driver_config(driver: DriverConfig) -> Self {
        let database = driver
            .data_source
            .database
            .clone()
            .filter(|d| !d.is_empty());
        Self { driver, database }
    }

    /// Reads the configuration of `data_source` from the environment.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Ok(Self::from_driver_config(DriverConfig::from_env(
            data_source,
        )?))
    }

    /// Configuration for `database` (a file path or `:memory:`).
    pub fn with_database(database: impl Into<String>) -> Self {
        Self {
            driver: DriverConfig::default(),
            database: Some(database.into()),
        }
    }
}

/// SQLite driver.
pub struct SqliteDriver {
    config: SqliteConfig,
    connection: Arc<Mutex<Option<rusqlite::Connection>>>,
}

impl std::fmt::Debug for SqliteDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteDriver")
            .field("database", &self.config.database)
            .finish()
    }
}

impl SqliteDriver {
    /// Opens the database named by `config.database`.
    pub fn new(config: SqliteConfig) -> Result<Self> {
        let database = config.database.clone().ok_or_else(|| {
            DriverError::Config(
                "The SQLite driver needs a database file: set CUBEJS_DB_NAME \
                 (a file path, or :memory: for an in-memory database)"
                    .to_string(),
            )
        })?;
        let connection =
            rusqlite::Connection::open(&database).map_err(|e| DriverError::Connection {
                pool_name: POOL_NAME.to_string(),
                message: format!("{database}: {e}"),
            })?;
        Ok(Self::with_connection(config, connection))
    }

    /// Wraps an already open connection (the Node driver's `config.db`).
    pub fn with_connection(config: SqliteConfig, connection: rusqlite::Connection) -> Self {
        Self {
            config,
            connection: Arc::new(Mutex::new(Some(connection))),
        }
    }

    /// Creates the driver from `CUBEJS_DB_*`.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Self::new(SqliteConfig::from_env(data_source)?)
    }

    /// The driver configuration.
    pub fn sqlite_config(&self) -> &SqliteConfig {
        &self.config
    }

    /// `getDefaultConcurrency`.
    pub fn default_concurrency() -> usize {
        DEFAULT_CONCURRENCY
    }

    /// `informationSchemaQuery` (SQLite override).
    pub fn sqlite_information_schema_query() -> String {
        "
      SELECT name
      FROM sqlite_master
      WHERE type='table'
      AND name!='sqlite_sequence'
      ORDER BY name
   "
        .to_string()
    }

    /// `tableColumnsQuery`.
    pub fn table_columns_query(table_name: &str) -> String {
        format!(
            "
      SELECT name, type
      FROM pragma_table_info('{}')
    ",
            table_name.replace('\'', "''")
        )
    }

    /// The `ATTACH` statement of `createSchemaIfNotExists`.
    pub fn attach_database_sql(schema_name: &str) -> String {
        format!("ATTACH DATABASE {schema_name} AS {schema_name}")
    }

    /// Runs `f` on the connection in Tokio's blocking pool.
    async fn with_conn<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&rusqlite::Connection) -> Result<T> + Send + 'static,
    {
        let connection = self.connection.clone();
        tokio::task::spawn_blocking(move || {
            let guard = connection
                .lock()
                .map_err(|_| DriverError::Other("SQLite connection mutex poisoned".to_string()))?;
            match guard.as_ref() {
                Some(conn) => f(conn),
                None => Err(DriverError::Connection {
                    pool_name: POOL_NAME.to_string(),
                    message: DATABASE_CLOSED.to_string(),
                }),
            }
        })
        .await
        .map_err(|e| DriverError::Other(format!("SQLite worker failed: {e}")))?
    }

    /// Names of the attached databases (`PRAGMA database_list`).
    async fn attached_databases(&self) -> Result<Vec<String>> {
        let list = self
            .query("PRAGMA database_list", &[], &QueryOptions::default())
            .await?;
        Ok((0..list.len())
            .filter_map(|i| list.get_string(i, "name"))
            .collect())
    }
}

/// Converts a JSON parameter to an SQLite value (`node-sqlite3` binding
/// rules: booleans are integers, objects are bound as text).
pub fn to_sql_value(value: &Value) -> SqlValue {
    match value {
        Value::Null => SqlValue::Null,
        Value::Bool(b) => SqlValue::Integer(i64::from(*b)),
        Value::Number(n) => match n.as_i64() {
            Some(i) => SqlValue::Integer(i),
            None => SqlValue::Real(n.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(s) => SqlValue::Text(s.clone()),
        other => SqlValue::Text(other.to_string()),
    }
}

/// Converts an SQLite value to JSON (see the module docs).
pub fn from_sql_value(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::from(i),
        ValueRef::Real(f) => serde_json::Number::from_f64(f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        ValueRef::Text(t) => Value::String(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => json!({ "type": "Buffer", "data": b }),
    }
}

/// Runs one statement and collects its rows. The column type is the declared
/// type when there is one (`text` for expressions).
fn run_query(conn: &rusqlite::Connection, sql: &str, params: &[Value]) -> Result<RawResult> {
    let mut stmt = conn.prepare(sql).map_err(sqlite_error)?;
    let columns: Vec<(String, Option<String>)> = stmt
        .columns()
        .iter()
        .map(|c| (c.name().to_string(), c.decl_type().map(str::to_string)))
        .collect();
    let bound: Vec<SqlValue> = params.iter().map(to_sql_value).collect();
    let mut rows = stmt
        .query(rusqlite::params_from_iter(bound.iter()))
        .map_err(sqlite_error)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(sqlite_error)? {
        let mut values = Vec::with_capacity(columns.len());
        for i in 0..columns.len() {
            values.push(from_sql_value(row.get_ref(i).map_err(sqlite_error)?));
        }
        out.push(values);
    }
    Ok(RawResult { columns, rows: out })
}

struct RawResult {
    columns: Vec<(String, Option<String>)>,
    rows: Vec<Vec<Value>>,
}

fn sqlite_error(e: rusqlite::Error) -> DriverError {
    let code = match &e {
        rusqlite::Error::SqliteFailure(err, _) => Some(format!("{:?}", err.code)),
        _ => None,
    };
    DriverError::Database {
        message: e.to_string(),
        code,
    }
}

#[async_trait]
impl Driver for SqliteDriver {
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
        let sql = sql.to_string();
        let params = params.to_vec();
        let raw = self
            .with_conn(move |conn| run_query(conn, &sql, &params))
            .await?;
        let columns = raw
            .columns
            .into_iter()
            .map(|(name, decl)| {
                let generic = self.to_generic_type(decl.as_deref().unwrap_or("text"), None, None);
                Column::new(name, generic)
            })
            .collect();
        Ok(QueryResult::new(columns, raw.rows))
    }

    async fn release(&self) -> Result<()> {
        let connection = self.connection.clone();
        tokio::task::spawn_blocking(move || {
            let taken = connection.lock().ok().and_then(|mut g| g.take());
            match taken {
                Some(conn) => conn.close().map_err(|(_, e)| sqlite_error(e)),
                None => Ok(()),
            }
        })
        .await
        .map_err(|e| DriverError::Other(format!("SQLite worker failed: {e}")))?
    }

    fn information_schema_query(&self) -> String {
        Self::sqlite_information_schema_query()
    }

    /// `{ main: { <table>: [{ name, type }] } }`.
    async fn tables_schema(&self) -> Result<DatabaseStructure> {
        let tables = self
            .query(
                &self.information_schema_query(),
                &[],
                &QueryOptions::default(),
            )
            .await?;
        let mut main = std::collections::BTreeMap::new();
        for i in 0..tables.len() {
            let Some(table) = tables.get_string(i, "name") else {
                continue;
            };
            let columns = self
                .query(
                    &Self::table_columns_query(&table),
                    &[],
                    &QueryOptions::default(),
                )
                .await?;
            let columns = (0..columns.len())
                .filter_map(|j| {
                    Some(SchemaColumn {
                        name: columns.get_string(j, "name")?,
                        type_: columns.get_string(j, "type").unwrap_or_default(),
                        attributes: Vec::new(),
                        foreign_keys: Vec::new(),
                    })
                })
                .collect();
            main.insert(table, columns);
        }
        let mut structure = DatabaseStructure::new();
        structure.insert("main".to_string(), main);
        Ok(structure)
    }

    async fn create_schema_if_not_exists(&self, schema_name: &str) -> Result<()> {
        if !self
            .attached_databases()
            .await?
            .iter()
            .any(|s| s == schema_name)
        {
            self.query(
                &Self::attach_database_sql(schema_name),
                &[],
                &QueryOptions::default(),
            )
            .await?;
        }
        Ok(())
    }

    async fn get_tables_query(&self, schema_name: &str) -> Result<Vec<String>> {
        if !self
            .attached_databases()
            .await?
            .iter()
            .any(|s| s == schema_name)
        {
            return Ok(Vec::new());
        }
        let result = self
            .query(
                &format!(
                    "SELECT name as table_name FROM {schema_name}.sqlite_master WHERE type='table' ORDER BY name"
                ),
                &[],
                &QueryOptions::default(),
            )
            .await?;
        Ok((0..result.len())
            .filter_map(|i| result.get_string(i, "table_name"))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_database_name() {
        let err = SqliteDriver::new(SqliteConfig::from_driver_config(DriverConfig::default()))
            .expect_err("must fail");
        assert!(matches!(err, DriverError::Config(_)));
        assert!(err.to_string().contains("CUBEJS_DB_NAME"));
    }

    #[test]
    fn reads_cubejs_db_name() {
        let env = [("CUBEJS_DB_NAME", "/tmp/x.db")];
        let config = DriverConfig::from_env_source(&env, None, false).unwrap();
        assert_eq!(
            SqliteConfig::from_driver_config(config).database.as_deref(),
            Some("/tmp/x.db")
        );
    }

    #[test]
    fn value_conversions() {
        assert_eq!(to_sql_value(&json!(true)), SqlValue::Integer(1));
        assert_eq!(to_sql_value(&json!(3)), SqlValue::Integer(3));
        assert_eq!(to_sql_value(&json!(1.5)), SqlValue::Real(1.5));
        assert_eq!(to_sql_value(&json!("a")), SqlValue::Text("a".into()));
        assert_eq!(to_sql_value(&Value::Null), SqlValue::Null);
        assert_eq!(
            from_sql_value(ValueRef::Blob(&[1, 2])),
            json!({"type": "Buffer", "data": [1, 2]})
        );
        assert_eq!(from_sql_value(ValueRef::Integer(7)), json!(7));
    }

    #[test]
    fn query_texts() {
        assert!(SqliteDriver::table_columns_query("o'x").contains("pragma_table_info('o''x')"));
        assert_eq!(
            SqliteDriver::attach_database_sql("s"),
            "ATTACH DATABASE s AS s"
        );
        assert!(SqliteDriver::sqlite_information_schema_query().contains("name!='sqlite_sequence'"));
    }
}
