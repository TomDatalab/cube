//! Row/column model and Cube's "generic" database types.
//!
//! # Row model
//!
//! Rows are positional: [`Row`] is a `Vec<serde_json::Value>` and the column
//! names/types live once per result set in [`QueryResult::columns`].
//! `serde_json::Value` is used as the cell type (rather than a bespoke enum)
//! because
//!
//! * the Node.js drivers already return JSON-shaped values (numerics and
//!   `int8` as strings, timestamps as ISO strings, `json`/`jsonb` and arrays as
//!   nested values, `null`), and every consumer (API gateway result transform,
//!   pre-aggregation upload, CubeStore inserts) serialises rows to JSON;
//! * query parameters in Cube already arrive as JSON (`unknown[]`), so a single
//!   value type covers both directions;
//! * it is the lingua franca between the crates of the Rust backend, avoiding
//!   a conversion layer at every crate boundary.
//!
//! `Value::Null` is the SQL `NULL`, so there is no extra `Option` wrapper.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;

/// Cube's generic database types (`GenericDataBaseType` in TypeScript).
///
/// Unknown / driver specific type names are preserved in [`GenericType::Other`]
/// so that the mapping is lossless (`BaseDriver.toGenericType` passes unknown
/// types through unchanged).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GenericType {
    Text,
    String,
    Int,
    Bigint,
    /// `decimal` or `decimal(precision, scale)` when
    /// `CUBEJS_DB_PRECISE_DECIMAL_IN_CUBESTORE` is enabled.
    Decimal(Option<(u32, u32)>),
    Double,
    Float,
    Boolean,
    Timestamp,
    Date,
    /// HyperLogLog sketch produced by the `postgresql-hll` extension.
    HllPostgres,
    /// Any other (driver specific) type name.
    Other(std::string::String),
}

impl GenericType {
    /// Parses a generic type name (never fails: unknown names become [`GenericType::Other`]).
    pub fn parse(s: &str) -> Self {
        match s {
            "text" => GenericType::Text,
            "string" => GenericType::String,
            "int" => GenericType::Int,
            "bigint" => GenericType::Bigint,
            "decimal" => GenericType::Decimal(None),
            "double" => GenericType::Double,
            "float" => GenericType::Float,
            "boolean" => GenericType::Boolean,
            "timestamp" => GenericType::Timestamp,
            "date" => GenericType::Date,
            "HLL_POSTGRES" => GenericType::HllPostgres,
            _ => {
                if let Some(inner) = s.strip_prefix("decimal(").and_then(|r| r.strip_suffix(')')) {
                    let mut parts = inner.split(',').map(|p| p.trim().parse::<u32>());
                    if let (Some(Ok(p)), Some(Ok(sc)), None) =
                        (parts.next(), parts.next(), parts.next())
                    {
                        return GenericType::Decimal(Some((p, sc)));
                    }
                }
                GenericType::Other(s.to_string())
            }
        }
    }

    /// `true` for `decimal` and `decimal(p, s)`.
    pub fn is_decimal(&self) -> bool {
        matches!(self, GenericType::Decimal(_))
    }
}

impl fmt::Display for GenericType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GenericType::Text => f.write_str("text"),
            GenericType::String => f.write_str("string"),
            GenericType::Int => f.write_str("int"),
            GenericType::Bigint => f.write_str("bigint"),
            GenericType::Decimal(None) => f.write_str("decimal"),
            GenericType::Decimal(Some((p, s))) => write!(f, "decimal({p}, {s})"),
            GenericType::Double => f.write_str("double"),
            GenericType::Float => f.write_str("float"),
            GenericType::Boolean => f.write_str("boolean"),
            GenericType::Timestamp => f.write_str("timestamp"),
            GenericType::Date => f.write_str("date"),
            GenericType::HllPostgres => f.write_str("HLL_POSTGRES"),
            GenericType::Other(s) => f.write_str(s),
        }
    }
}

impl FromStr for GenericType {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(GenericType::parse(s))
    }
}

impl From<&str> for GenericType {
    fn from(s: &str) -> Self {
        GenericType::parse(s)
    }
}

impl Serialize for GenericType {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for GenericType {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let s = std::string::String::deserialize(deserializer)?;
        Ok(GenericType::parse(&s))
    }
}

/// Port of `DbTypeToGenericType` from `BaseDriver.ts`.
///
/// Returns `None` when the (lower-cased) database type has no mapping.
pub fn db_type_to_generic(db_type_lower: &str) -> Option<GenericType> {
    Some(match db_type_lower {
        "timestamp without time zone" => GenericType::Timestamp,
        "character varying" => GenericType::Text,
        "varchar" => GenericType::Text,
        "integer" => GenericType::Int,
        "nvarchar" => GenericType::Text,
        "text" => GenericType::Text,
        "string" => GenericType::Text,
        "boolean" => GenericType::Boolean,
        "bigint" => GenericType::Bigint,
        "time" => GenericType::String,
        "datetime" => GenericType::Timestamp,
        "date" => GenericType::Date,
        "enum" => GenericType::Text,
        "double precision" => GenericType::Double,
        // PostgreSQL aliases, but maybe another databases support it
        "numeric" => GenericType::Decimal(None),
        "int8" => GenericType::Bigint,
        "int4" => GenericType::Int,
        "int2" => GenericType::Int,
        "bool" => GenericType::Boolean,
        "float4" => GenericType::Float,
        "float8" => GenericType::Double,
        _ => return None,
    })
}

/// Port of `BaseDriver.toGenericType`.
///
/// `precise_decimal` is `CUBEJS_DB_PRECISE_DECIMAL_IN_CUBESTORE`.
pub fn to_generic_type(
    db_type: &str,
    precision: Option<i64>,
    scale: Option<i64>,
    precise_decimal: bool,
) -> GenericType {
    // JS returns the original `columnType` string when the table has no entry,
    // so a name that already *is* a generic type keeps its canonical meaning
    // (`timestamp`, `date`, `decimal(10, 2)`, ...). `parse` falls back to
    // `Other` for everything else, preserving the original spelling.
    let generic =
        db_type_to_generic(&db_type.to_lowercase()).unwrap_or_else(|| GenericType::parse(db_type));

    // JS: `genericType === 'decimal' && precision && scale && getEnv(...)`
    // (`0` is falsy, hence the `> 0` checks).
    if let (GenericType::Decimal(None), Some(p), Some(s)) = (&generic, precision, scale) {
        if p > 0 && s > 0 && precise_decimal {
            return GenericType::Decimal(Some((p as u32, s as u32)));
        }
    }

    generic
}

/// A result-set / table column: name plus generic type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub name: std::string::String,
    #[serde(rename = "type")]
    pub type_: GenericType,
}

impl Column {
    pub fn new(name: impl Into<std::string::String>, type_: impl Into<GenericType>) -> Self {
        Self {
            name: name.into(),
            type_: type_.into(),
        }
    }
}

/// `TableStructure` in TypeScript.
pub type TableStructure = Vec<Column>;

/// A positional row. See the module documentation for the rationale.
pub type Row = Vec<Value>;

/// A fully materialised result set.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct QueryResult {
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
}

impl QueryResult {
    pub fn new(columns: Vec<Column>, rows: Vec<Row>) -> Self {
        Self { columns, rows }
    }

    /// Index of the first column named `name`.
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name)
    }

    /// Cell at `(row, column name)`; `None` when the column does not exist.
    pub fn get(&self, row: usize, column: &str) -> Option<&Value> {
        let idx = self.column_index(column)?;
        self.rows.get(row)?.get(idx)
    }

    /// String cell at `(row, column name)`. Non-string JSON scalars are
    /// stringified (mirrors JavaScript's implicit coercion).
    pub fn get_string(&self, row: usize, column: &str) -> Option<std::string::String> {
        value_to_string(self.get(row, column)?)
    }

    /// Integer cell at `(row, column name)`.
    pub fn get_i64(&self, row: usize, column: &str) -> Option<i64> {
        match self.get(row, column)? {
            Value::Number(n) => n.as_i64(),
            Value::String(s) => s.parse().ok(),
            _ => None,
        }
    }

    /// Number of rows.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// `true` when the result set has no rows.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Converts rows into JSON objects keyed by column name
    /// (the `Record<string, unknown>` shape of the Node.js drivers).
    pub fn to_json_rows(&self) -> Vec<serde_json::Map<std::string::String, Value>> {
        self.rows
            .iter()
            .map(|row| {
                self.columns
                    .iter()
                    .zip(row.iter())
                    .map(|(c, v)| (c.name.clone(), v.clone()))
                    .collect()
            })
            .collect()
    }
}

/// Converts a JSON scalar to its string form (`null` → `None`).
pub fn value_to_string(v: &Value) -> Option<std::string::String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        other => Some(other.to_string()),
    }
}

/// Fully qualified `schema.table` name (port of `TableName` in `utils.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TableName {
    pub schema: std::string::String,
    pub name: std::string::String,
}

impl TableName {
    pub fn new(
        schema: impl Into<std::string::String>,
        name: impl Into<std::string::String>,
    ) -> Self {
        Self {
            schema: schema.into(),
            name: name.into(),
        }
    }

    /// Splits at the first `.`: `"a.b.c"` → schema `a`, name `b.c`.
    pub fn split(table: &str) -> Self {
        match table.split_once('.') {
            Some((schema, name)) => Self::new(schema, name),
            None => Self::new(table, ""),
        }
    }

    pub fn join(&self) -> std::string::String {
        format!("{}.{}", self.schema, self.name)
    }
}

/// `QuerySchemasResult`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaName {
    pub schema_name: std::string::String,
}

/// `QueryTablesResult`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaTable {
    pub schema_name: std::string::String,
    pub table_name: std::string::String,
}

/// `ForeignKey`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForeignKey {
    pub target_table: std::string::String,
    pub target_column: std::string::String,
}

/// `QueryColumnsResult`: one row of `getColumnsForSpecificTables`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnInfo {
    pub schema_name: std::string::String,
    pub table_name: std::string::String,
    pub column_name: std::string::String,
    /// Raw database type (`information_schema.columns.data_type`).
    pub data_type: std::string::String,
    /// `["primaryKey"]` when the column is part of the primary key.
    #[serde(default)]
    pub attributes: Vec<std::string::String>,
    #[serde(default)]
    pub foreign_keys: Vec<ForeignKey>,
}

/// `TableColumn` as used by `tablesSchema` (raw database type + attributes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaColumn {
    pub name: std::string::String,
    /// Raw database type (`information_schema.columns.data_type`).
    #[serde(rename = "type")]
    pub type_: std::string::String,
    #[serde(default)]
    pub attributes: Vec<std::string::String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub foreign_keys: Vec<ForeignKey>,
}

/// `DatabaseStructure`: schema → table → columns.
pub type DatabaseStructure =
    BTreeMap<std::string::String, BTreeMap<std::string::String, Vec<SchemaColumn>>>;

/// `PrimaryKeysQueryResult`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrimaryKeyInfo {
    pub table_schema: std::string::String,
    pub table_name: std::string::String,
    pub column_name: std::string::String,
}

/// `ForeignKeysQueryResult`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForeignKeyInfo {
    pub table_schema: std::string::String,
    pub table_name: std::string::String,
    pub column_name: std::string::String,
    pub target_table: std::string::String,
    pub target_column: std::string::String,
}

/// `InlineTable` (inline tables shipped with a query, used by some drivers).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InlineTable {
    pub name: std::string::String,
    pub columns: TableStructure,
    /// Rows in CSV format.
    pub csv_rows: std::string::String,
}

/// `QueryOptions`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct QueryOptions {
    #[serde(default)]
    pub inline_tables: Vec<InlineTable>,
    #[serde(default)]
    pub request_id: Option<std::string::String>,
    /// Any additional driver specific options (`[key: string]: any`).
    #[serde(default, flatten)]
    pub extra: serde_json::Map<std::string::String, Value>,
}

impl QueryOptions {
    /// Sets the `sendParameters` switch honoured by the Cube Store driver
    /// (see `CubeStoreDriver::send_parameters`). No other driver reads it.
    pub fn with_send_parameters(mut self, send_parameters: bool) -> Self {
        self.extra
            .insert("sendParameters".to_string(), Value::Bool(send_parameters));
        self
    }

    /// Sets the `requestId` carried into the driver's tracing object.
    pub fn with_request_id(mut self, request_id: impl Into<std::string::String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }
}

/// `StreamOptions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamOptions {
    /// Buffer size hint (rows) for the row stream.
    pub high_water_mark: usize,
    #[serde(default)]
    pub request_id: Option<std::string::String>,
}

impl Default for StreamOptions {
    fn default() -> Self {
        Self {
            high_water_mark: 16_000,
            request_id: None,
        }
    }
}

/// `DownloadQueryResultsOptions`
/// (`StreamOptions & ExternalDriverCompatibilities & StreamingSourceOptions`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadQueryResultsOptions {
    #[serde(flatten)]
    pub stream: StreamOptions,
    /// The external driver (CubeStore) accepts CSV imports.
    #[serde(default)]
    pub csv_import: bool,
    /// The external driver (CubeStore) accepts streamed row imports.
    #[serde(default)]
    pub stream_import: bool,
    #[serde(default)]
    pub stream_offset: bool,
    #[serde(default)]
    pub output_column_types: Option<TableStructure>,
}

/// `ExternalDriverCompatibilities & StreamingSourceOptions` for `downloadTable`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadTableOptions {
    #[serde(default)]
    pub csv_import: bool,
    #[serde(default)]
    pub stream_import: bool,
    #[serde(default)]
    pub stream_offset: bool,
    #[serde(default)]
    pub output_column_types: Option<TableStructure>,
}

/// `UnloadOptions`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UnloadOptions {
    pub max_file_size: u64,
    #[serde(default)]
    pub query: Option<UnloadQuery>,
    #[serde(default)]
    pub request_id: Option<std::string::String>,
}

/// `UnloadQuery`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UnloadQuery {
    pub sql: std::string::String,
    #[serde(default)]
    pub params: Vec<Value>,
}

/// `TableMemoryData`: rows held in memory (positional, see [`QueryResult`]).
pub type TableMemoryData = QueryResult;

/// `TableCSVData`: pointers to unloaded CSV files (export bucket drivers).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableCsvData {
    /// Temporary (signed) URLs of the unloaded CSV files.
    pub csv_file: Vec<std::string::String>,
    #[serde(default)]
    pub types: Option<TableStructure>,
    #[serde(default)]
    pub csv_no_header: bool,
    #[serde(default)]
    pub csv_delimiter: Option<std::string::String>,
    #[serde(default)]
    pub csv_disable_quoting: bool,
    #[serde(default)]
    pub export_bucket_csv_escape_symbol: Option<std::string::String>,
}

/// `StreamTableData`: a row stream plus the column types.
///
/// The underlying connection is released when the stream is dropped
/// (the Node.js `release()` callback has no explicit counterpart).
pub struct StreamTableData {
    pub columns: Vec<Column>,
    pub rows: BoxStream<'static, Result<Row>>,
}

impl fmt::Debug for StreamTableData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamTableData")
            .field("columns", &self.columns)
            .field("rows", &"<stream>")
            .finish()
    }
}

/// `DownloadTableData` / `DownloadQueryResultsResult`.
#[derive(Debug)]
pub enum DownloadedData {
    Memory(TableMemoryData),
    Csv(TableCsvData),
    Stream(StreamTableData),
}

impl DownloadedData {
    /// Column types when known.
    pub fn columns(&self) -> Option<&[Column]> {
        match self {
            DownloadedData::Memory(m) => Some(&m.columns),
            DownloadedData::Csv(c) => c.types.as_deref(),
            DownloadedData::Stream(s) => Some(&s.columns),
        }
    }

    /// Collects a stream / memory result into a [`TableMemoryData`].
    pub async fn into_memory(self) -> Result<TableMemoryData> {
        use futures::TryStreamExt;
        match self {
            DownloadedData::Memory(m) => Ok(m),
            DownloadedData::Stream(s) => {
                let rows = s.rows.try_collect::<Vec<Row>>().await?;
                Ok(QueryResult::new(s.columns, rows))
            }
            // The unloaded files are fetched and parsed; see [`crate::csv_import`].
            DownloadedData::Csv(csv) => crate::csv_import::csv_to_memory(&csv).await,
        }
    }
}

/// `IndexesSQL` entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexSql {
    pub sql: std::string::String,
    #[serde(default)]
    pub params: Vec<Value>,
}

/// `CreateTableIndex`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateTableIndex {
    pub index_name: std::string::String,
    #[serde(rename = "type")]
    pub type_: std::string::String,
    pub columns: Vec<std::string::String>,
}

/// `ExternalCreateTableOptions`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalCreateTableOptions {
    #[serde(default)]
    pub aggregations_columns: Vec<std::string::String>,
    #[serde(default)]
    pub create_table_indexes: Vec<CreateTableIndex>,
    #[serde(default)]
    pub seal_at: Option<std::string::String>,
}

/// `DriverCapabilities`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriverCapabilities {
    #[serde(default)]
    pub csv_import: bool,
    #[serde(default)]
    pub stream_import: bool,
    #[serde(default)]
    pub unload_without_temp_table: bool,
    #[serde(default)]
    pub streaming_source: bool,
    #[serde(default)]
    pub incremental_schema_loading: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_type_roundtrip() {
        for s in [
            "text",
            "string",
            "int",
            "bigint",
            "decimal",
            "decimal(10, 2)",
            "double",
            "float",
            "boolean",
            "timestamp",
            "date",
            "HLL_POSTGRES",
            "uuid",
            "timestamptz",
        ] {
            assert_eq!(GenericType::parse(s).to_string(), s);
        }
        assert_eq!(
            GenericType::parse("decimal(10,2)"),
            GenericType::Decimal(Some((10, 2)))
        );
        assert_eq!(
            GenericType::parse("varchar"),
            GenericType::Other("varchar".into())
        );
    }

    #[test]
    fn generic_type_serde() {
        let c = Column::new("a", "decimal(10, 2)");
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(json, r#"{"name":"a","type":"decimal(10, 2)"}"#);
        let back: Column = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn db_type_mapping() {
        assert_eq!(
            to_generic_type("integer", None, None, false),
            GenericType::Int
        );
        assert_eq!(
            to_generic_type("INTEGER", None, None, false),
            GenericType::Int
        );
        assert_eq!(
            to_generic_type("character varying", None, None, false),
            GenericType::Text
        );
        assert_eq!(
            to_generic_type("timestamp without time zone", None, None, false),
            GenericType::Timestamp
        );
        assert_eq!(
            to_generic_type("int8", None, None, false),
            GenericType::Bigint
        );
        assert_eq!(
            to_generic_type("float4", None, None, false),
            GenericType::Float
        );
        assert_eq!(
            to_generic_type("float8", None, None, false),
            GenericType::Double
        );
        assert_eq!(
            to_generic_type("time", None, None, false),
            GenericType::String
        );
        assert_eq!(
            to_generic_type("timestamptz", None, None, false),
            GenericType::Other("timestamptz".into())
        );
        assert_eq!(
            to_generic_type("numeric", Some(10), Some(2), false),
            GenericType::Decimal(None)
        );
        assert_eq!(
            to_generic_type("numeric", Some(10), Some(2), true),
            GenericType::Decimal(Some((10, 2)))
        );
        // scale 0 is falsy in JS
        assert_eq!(
            to_generic_type("numeric", Some(10), Some(0), true),
            GenericType::Decimal(None)
        );
    }

    #[test]
    fn query_result_accessors() {
        let r = QueryResult::new(
            vec![Column::new("a", "int"), Column::new("b", "text")],
            vec![
                vec![Value::from(1), Value::from("x")],
                vec![Value::Null, Value::Null],
            ],
        );
        assert_eq!(r.get_i64(0, "a"), Some(1));
        assert_eq!(r.get_string(0, "b").as_deref(), Some("x"));
        assert_eq!(r.get_string(1, "b"), None);
        assert_eq!(r.get(0, "zzz"), None);
        let json = r.to_json_rows();
        assert_eq!(json[0]["a"], Value::from(1));
        assert_eq!(json[1]["b"], Value::Null);
    }

    #[test]
    fn table_name_split() {
        let t = TableName::split("public.orders");
        assert_eq!(t.schema, "public");
        assert_eq!(t.name, "orders");
        assert_eq!(t.join(), "public.orders");
        let t = TableName::split("a.b.c");
        assert_eq!(t.schema, "a");
        assert_eq!(t.name, "b.c");
    }
}
