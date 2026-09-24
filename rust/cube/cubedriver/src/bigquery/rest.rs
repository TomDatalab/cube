//! The slice of the BigQuery REST API the driver needs, plus the row
//! hydration that reproduces `HydrationStream.transformRow`.

use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{DriverError, Result};
use crate::types::{Column, GenericType, Row};

/// Base URL of the BigQuery REST API.
pub const API_BASE: &str = "https://bigquery.googleapis.com/bigquery/v2";

/// `BigQueryToGenericType`: the types the base table does not know about.
pub fn bigquery_to_generic(db_type_lower: &str) -> Option<GenericType> {
    match db_type_lower {
        "bignumeric" | "bigdecimal" | "decimal" => Some(GenericType::Decimal(None)),
        _ => None,
    }
}

/// Port of `BigQueryDriver.toGenericType`.
pub fn to_generic_type(
    db_type: &str,
    precision: Option<i64>,
    scale: Option<i64>,
    precise_decimal: bool,
) -> GenericType {
    // JS: `BigQueryToGenericType[columnType.toLowerCase()] || columnType`,
    // then `super.toGenericType(mappedType, precision, scale)`.
    match bigquery_to_generic(&db_type.to_lowercase()) {
        Some(mapped) => {
            crate::types::to_generic_type(&mapped.to_string(), precision, scale, precise_decimal)
        }
        None => crate::types::to_generic_type(db_type, precision, scale, precise_decimal),
    }
}

/// Port of `BigQueryDriver.quoteIdentifier`.
pub fn quote_identifier(identifier: &str) -> String {
    identifier
        .split('.')
        .map(|name| {
            if !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            {
                name.to_string()
            } else {
                // Verbatim from the Node driver: the *whole* identifier is
                // quoted, not the current part.
                format!("`{identifier}`")
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// One entry of a table / result-set schema.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TableFieldSchema {
    #[serde(default)]
    pub name: String,
    #[serde(rename = "type", default)]
    pub type_: String,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub fields: Option<Vec<TableFieldSchema>>,
}

impl TableFieldSchema {
    fn is_repeated(&self) -> bool {
        self.mode.as_deref() == Some("REPEATED")
    }

    fn is_record(&self) -> bool {
        matches!(self.type_.as_str(), "RECORD" | "STRUCT")
    }
}

/// A result-set / table schema.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct TableSchema {
    #[serde(default)]
    pub fields: Vec<TableFieldSchema>,
}

/// `jobs.getQueryResults` / `jobs.query` response.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryResultsResponse {
    #[serde(default)]
    pub schema: Option<TableSchema>,
    #[serde(default)]
    pub rows: Vec<TableRow>,
    #[serde(default)]
    pub page_token: Option<String>,
    #[serde(default)]
    pub job_complete: Option<bool>,
    #[serde(default)]
    pub total_rows: Option<String>,
}

/// One row of the tabular REST representation (`{ "f": [{ "v": ... }] }`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TableRow {
    #[serde(default)]
    pub f: Vec<TableCell>,
}

/// One cell.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TableCell {
    #[serde(default)]
    pub v: Value,
}

/// A job reference.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JobReference {
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub job_id: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
}

/// The parts of a job resource the driver reads.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    #[serde(default)]
    pub job_reference: Option<JobReference>,
    #[serde(default)]
    pub status: Option<JobStatus>,
}

/// `job.status`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobStatus {
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub error_result: Option<ErrorProto>,
}

/// `ErrorProto`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ErrorProto {
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

/// `datasets.list` response.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatasetList {
    #[serde(default)]
    pub datasets: Vec<DatasetListItem>,
    #[serde(default)]
    pub next_page_token: Option<String>,
}

/// One entry of `datasets.list`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatasetListItem {
    #[serde(default)]
    pub dataset_reference: Option<DatasetReference>,
}

/// `DatasetReference`.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DatasetReference {
    #[serde(default)]
    pub dataset_id: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
}

/// `tables.list` response.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableList {
    #[serde(default)]
    pub tables: Vec<TableListItem>,
    #[serde(default)]
    pub next_page_token: Option<String>,
}

/// One entry of `tables.list`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableListItem {
    #[serde(default)]
    pub table_reference: Option<TableReference>,
}

/// `TableReference`.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TableReference {
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub dataset_id: Option<String>,
    #[serde(default)]
    pub table_id: Option<String>,
}

/// `tables.get` response (only the schema is used).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TableMetadata {
    #[serde(default)]
    pub schema: Option<TableSchema>,
}

/// A positional query parameter (`parameterMode: 'positional'`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QueryParameter {
    pub parameter_type: QueryParameterType,
    pub parameter_value: QueryParameterValue,
}

/// `QueryParameterType`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QueryParameterType {
    #[serde(rename = "type")]
    pub type_: String,
}

/// `QueryParameterValue`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QueryParameterValue {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// Maps a JSON parameter onto a positional BigQuery query parameter.
///
/// `@google-cloud/bigquery` infers the parameter type from the JavaScript
/// value; this reproduces the mapping for the JSON value space Cube uses.
pub fn to_query_parameter(value: &Value) -> QueryParameter {
    let (type_, value) = match value {
        Value::Null => ("STRING", None),
        Value::Bool(b) => ("BOOL", Some(b.to_string())),
        Value::Number(n) if n.is_f64() => ("FLOAT64", Some(n.to_string())),
        Value::Number(n) => ("INT64", Some(n.to_string())),
        Value::String(s) => ("STRING", Some(s.clone())),
        other => ("STRING", Some(other.to_string())),
    };
    QueryParameter {
        parameter_type: QueryParameterType {
            type_: type_.to_string(),
        },
        parameter_value: QueryParameterValue { value },
    }
}

/// `toGenericType` of the driver, passed into the schema converters.
pub type ToGenericType<'a> = &'a dyn Fn(&str, Option<i64>, Option<i64>) -> GenericType;

/// Column list of a result-set schema.
pub fn schema_to_columns(schema: &TableSchema, to_generic: ToGenericType<'_>) -> Vec<Column> {
    schema
        .fields
        .iter()
        .map(|f| Column::new(f.name.clone(), to_generic(&f.type_, None, None)))
        .collect()
}

/// Column types of a *table* schema (`tableColumnTypes`).
///
/// BigQuery `NUMERIC` is always `(38, 9)` and `BIGNUMERIC` `(76, 38)`, which
/// the Node driver hardcodes as well.
pub fn table_schema_to_columns(schema: &TableSchema, to_generic: ToGenericType<'_>) -> Vec<Column> {
    schema
        .fields
        .iter()
        .map(|f| {
            let type_ = match f.type_.as_str() {
                "NUMERIC" | "DECIMAL" => to_generic(&f.type_, Some(38), Some(9)),
                "BIGNUMERIC" | "BIGDECIMAL" => to_generic(&f.type_, Some(76), Some(38)),
                _ => to_generic(&f.type_, None, None),
            };
            Column::new(f.name.clone(), type_)
        })
        .collect()
}

/// Converts the REST representation of a row into a positional [`Row`].
pub fn convert_row(row: &TableRow, fields: &[TableFieldSchema]) -> Row {
    fields
        .iter()
        .enumerate()
        .map(|(i, field)| match row.f.get(i) {
            Some(cell) => convert_cell(&cell.v, field),
            None => Value::Null,
        })
        .collect()
}

/// Converts one cell, mirroring what `@google-cloud/bigquery` hands to
/// `transformRow`: numbers and big numerics arrive as strings, wrapped
/// temporal values as their `.value`, bytes as base64.
fn convert_cell(value: &Value, field: &TableFieldSchema) -> Value {
    if field.is_repeated() {
        // Repeated fields arrive as `[{ "v": ... }, ...]`.
        if let Value::Array(items) = value {
            let scalar = TableFieldSchema {
                mode: None,
                ..field.clone()
            };
            return Value::Array(
                items
                    .iter()
                    .map(|item| convert_cell(item.get("v").unwrap_or(&Value::Null), &scalar))
                    .collect(),
            );
        }
        return Value::Null;
    }

    match value {
        Value::Null => Value::Null,
        Value::Object(map) if field.is_record() => {
            let empty = Vec::new();
            let sub_fields = field.fields.as_ref().unwrap_or(&empty);
            let cells = map.get("f").and_then(|f| f.as_array());
            let mut object = serde_json::Map::new();
            for (i, sub) in sub_fields.iter().enumerate() {
                let cell = cells
                    .and_then(|c| c.get(i))
                    .and_then(|c| c.get("v"))
                    .unwrap_or(&Value::Null);
                object.insert(sub.name.clone(), convert_cell(cell, sub));
            }
            Value::Object(object)
        }
        Value::String(s) => match field.type_.as_str() {
            "BOOLEAN" | "BOOL" => Value::Bool(s.eq_ignore_ascii_case("true")),
            "TIMESTAMP" => Value::String(format_timestamp(s)),
            _ => Value::String(s.clone()),
        },
        Value::Bool(b) => Value::Bool(*b),
        // Anything unexpected is kept as-is (`transformRow` passes it through).
        other => other.clone(),
    }
}

/// Formats an epoch-seconds timestamp the way `BigQueryTimestamp.value` does
/// (`2020-01-01T12:34:56.789Z`).
pub fn format_timestamp(epoch_seconds: &str) -> String {
    let Ok(seconds) = epoch_seconds.parse::<f64>() else {
        return epoch_seconds.to_string();
    };
    let micros = (seconds * 1_000_000.0).round() as i64;
    match Utc.timestamp_micros(micros) {
        chrono::LocalResult::Single(dt) => dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
        _ => epoch_seconds.to_string(),
    }
}

/// Turns a non-2xx REST response body into a [`DriverError`].
pub fn api_error(status: reqwest::StatusCode, body: &str) -> DriverError {
    #[derive(Deserialize)]
    struct ErrorEnvelope {
        error: Option<ErrorBody>,
    }
    #[derive(Deserialize)]
    struct ErrorBody {
        message: Option<String>,
    }

    let message = serde_json::from_str::<ErrorEnvelope>(body)
        .ok()
        .and_then(|e| e.error)
        .and_then(|e| e.message)
        .unwrap_or_else(|| body.trim().to_string());
    DriverError::Database {
        message,
        code: Some(status.as_u16().to_string()),
    }
}

/// Parses a JSON REST response.
pub fn parse_json<T: serde::de::DeserializeOwned>(body: &str) -> Result<T> {
    serde_json::from_str(body)
        .map_err(|e| DriverError::Query(format!("Unexpected BigQuery response: {e}: {body}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(name: &str, type_: &str) -> TableFieldSchema {
        TableFieldSchema {
            name: name.to_string(),
            type_: type_.to_string(),
            mode: None,
            fields: None,
        }
    }

    #[test]
    fn quote_identifier_matches_node() {
        assert_eq!(quote_identifier("orders"), "orders");
        assert_eq!(quote_identifier("a.b"), "a.b");
        assert_eq!(quote_identifier("Orders"), "`Orders`");
        // the Node implementation quotes the whole identifier per part
        assert_eq!(quote_identifier("a.B"), "a.`a.B`");
        assert_eq!(quote_identifier("with space"), "`with space`");
    }

    #[test]
    fn type_mapping() {
        assert_eq!(
            to_generic_type("BIGNUMERIC", None, None, false),
            GenericType::Decimal(None)
        );
        assert_eq!(
            to_generic_type("NUMERIC", Some(38), Some(9), true),
            GenericType::Decimal(Some((38, 9)))
        );
        assert_eq!(
            to_generic_type("BIGNUMERIC", Some(76), Some(38), true),
            GenericType::Decimal(Some((76, 38)))
        );
        assert_eq!(
            to_generic_type("INTEGER", None, None, false),
            GenericType::Int
        );
        assert_eq!(
            to_generic_type("BOOLEAN", None, None, false),
            GenericType::Boolean
        );
        // Unmapped names keep their spelling, including their case (JS returns
        // `columnType` unchanged when the lookup misses).
        assert_eq!(
            to_generic_type("TIMESTAMP", None, None, false),
            GenericType::Other("TIMESTAMP".into())
        );
        assert_eq!(
            to_generic_type("timestamp", None, None, false),
            GenericType::Timestamp
        );
        // unknown types keep their spelling, as in JS
        assert_eq!(
            to_generic_type("INT64", None, None, false),
            GenericType::Other("INT64".into())
        );
        assert_eq!(
            to_generic_type("FLOAT", None, None, false),
            GenericType::Other("FLOAT".into())
        );
    }

    #[test]
    fn timestamps_are_iso_formatted() {
        assert_eq!(format_timestamp("1577836800.0"), "2020-01-01T00:00:00.000Z");
        assert_eq!(
            format_timestamp("1577881496.789"),
            "2020-01-01T12:24:56.789Z"
        );
        assert_eq!(format_timestamp("not-a-number"), "not-a-number");
    }

    #[test]
    fn rows_are_hydrated() {
        let fields = vec![
            field("i", "INTEGER"),
            field("f", "FLOAT"),
            field("b", "BOOLEAN"),
            field("t", "TIMESTAMP"),
            field("d", "DATE"),
            field("n", "STRING"),
        ];
        let row: TableRow = serde_json::from_value(serde_json::json!({
            "f": [
                { "v": "9007199254740993" },
                { "v": "1.5" },
                { "v": "true" },
                { "v": "1577836800.0" },
                { "v": "2020-01-01" },
                { "v": null }
            ]
        }))
        .unwrap();
        let converted = convert_row(&row, &fields);
        assert_eq!(converted[0], Value::from("9007199254740993"));
        assert_eq!(converted[1], Value::from("1.5"));
        assert_eq!(converted[2], Value::Bool(true));
        assert_eq!(converted[3], Value::from("2020-01-01T00:00:00.000Z"));
        assert_eq!(converted[4], Value::from("2020-01-01"));
        assert_eq!(converted[5], Value::Null);
    }

    #[test]
    fn repeated_and_record_fields() {
        let fields = vec![
            TableFieldSchema {
                name: "tags".into(),
                type_: "STRING".into(),
                mode: Some("REPEATED".into()),
                fields: None,
            },
            TableFieldSchema {
                name: "nested".into(),
                type_: "RECORD".into(),
                mode: None,
                fields: Some(vec![field("id", "INTEGER"), field("ok", "BOOLEAN")]),
            },
        ];
        let row: TableRow = serde_json::from_value(serde_json::json!({
            "f": [
                { "v": [{ "v": "a" }, { "v": "b" }] },
                { "v": { "f": [{ "v": "7" }, { "v": "false" }] } }
            ]
        }))
        .unwrap();
        let converted = convert_row(&row, &fields);
        assert_eq!(converted[0], serde_json::json!(["a", "b"]));
        assert_eq!(converted[1], serde_json::json!({ "id": "7", "ok": false }));
    }

    #[test]
    fn query_parameters() {
        let p = to_query_parameter(&Value::from("x"));
        assert_eq!(p.parameter_type.type_, "STRING");
        assert_eq!(p.parameter_value.value.as_deref(), Some("x"));
        assert_eq!(
            to_query_parameter(&Value::from(7)).parameter_type.type_,
            "INT64"
        );
        assert_eq!(
            to_query_parameter(&Value::from(1.5)).parameter_type.type_,
            "FLOAT64"
        );
        assert_eq!(
            to_query_parameter(&Value::Bool(true)).parameter_type.type_,
            "BOOL"
        );
        let null = to_query_parameter(&Value::Null);
        assert_eq!(null.parameter_type.type_, "STRING");
        assert_eq!(null.parameter_value.value, None);
        assert_eq!(
            serde_json::to_string(&null).unwrap(),
            r#"{"parameterType":{"type":"STRING"},"parameterValue":{}}"#
        );
    }

    #[test]
    fn errors_are_unwrapped() {
        let err = api_error(
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"error":{"code":400,"message":"Syntax error"}}"#,
        );
        assert_eq!(err.to_string(), "Syntax error");
        let err = api_error(reqwest::StatusCode::BAD_REQUEST, "boom");
        assert_eq!(err.to_string(), "boom");
    }
}
