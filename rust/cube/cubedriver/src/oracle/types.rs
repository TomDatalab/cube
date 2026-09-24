//! Oracle type mapping and value conversion.
//!
//! Result-set columns are described by `oracledb::Metadata`; the Node driver
//! works from node-oracledb's `metaData[].dbTypeName`, so [`db_type_name`]
//! first recovers that exact string and every other mapping is keyed by it.

use base64::Engine;
use oracledb::{
    DbType, JsonValue, Metadata, OracleNumber, OracleTimestamp, Row, Vector, VectorData,
};
use serde_json::Value;

use crate::error::{DriverError, Result};
use crate::types::GenericType;

/// node-oracledb's `dbTypeName` for a column type (`"NUMBER"`, `"VARCHAR2"`,
/// `"TIMESTAMP WITH TIME ZONE"`, ...).
pub fn db_type_name(db_type: &DbType) -> &'static str {
    const TYPES: &[(&DbType, &str)] = &[
        (oracledb::DB_TYPE_BFILE, "BFILE"),
        (oracledb::DB_TYPE_BINARY_DOUBLE, "BINARY_DOUBLE"),
        (oracledb::DB_TYPE_BINARY_FLOAT, "BINARY_FLOAT"),
        (oracledb::DB_TYPE_BINARY_INTEGER, "BINARY_INTEGER"),
        (oracledb::DB_TYPE_BLOB, "BLOB"),
        (oracledb::DB_TYPE_BOOLEAN, "BOOLEAN"),
        (oracledb::DB_TYPE_CHAR, "CHAR"),
        (oracledb::DB_TYPE_CLOB, "CLOB"),
        (oracledb::DB_TYPE_CURSOR, "CURSOR"),
        (oracledb::DB_TYPE_DATE, "DATE"),
        (oracledb::DB_TYPE_INTERVAL_DS, "INTERVAL DAY TO SECOND"),
        (oracledb::DB_TYPE_INTERVAL_YM, "INTERVAL YEAR TO MONTH"),
        (oracledb::DB_TYPE_JSON, "JSON"),
        (oracledb::DB_TYPE_LONG, "LONG"),
        (oracledb::DB_TYPE_LONG_NVARCHAR, "LONG"),
        (oracledb::DB_TYPE_LONG_RAW, "LONG RAW"),
        (oracledb::DB_TYPE_NCHAR, "NCHAR"),
        (oracledb::DB_TYPE_NCLOB, "NCLOB"),
        (oracledb::DB_TYPE_NUMBER, "NUMBER"),
        (oracledb::DB_TYPE_NVARCHAR, "NVARCHAR2"),
        (oracledb::DB_TYPE_OBJECT, "OBJECT"),
        (oracledb::DB_TYPE_RAW, "RAW"),
        (oracledb::DB_TYPE_ROWID, "ROWID"),
        (oracledb::DB_TYPE_TIMESTAMP, "TIMESTAMP"),
        (
            oracledb::DB_TYPE_TIMESTAMP_LTZ,
            "TIMESTAMP WITH LOCAL TIME ZONE",
        ),
        (oracledb::DB_TYPE_TIMESTAMP_TZ, "TIMESTAMP WITH TIME ZONE"),
        (oracledb::DB_TYPE_UROWID, "UROWID"),
        (oracledb::DB_TYPE_VARCHAR, "VARCHAR2"),
        (oracledb::DB_TYPE_VECTOR, "VECTOR"),
        (oracledb::DB_TYPE_XMLTYPE, "XMLTYPE"),
    ];
    TYPES
        .iter()
        .find(|(t, _)| *t == db_type)
        .map(|(_, name)| *name)
        .unwrap_or("UNKNOWN")
}

/// Port of `OracleTypeToGenericType` + `metaDataToColumnTypes`: the generic
/// type of a result-set column, from its `dbTypeName`.
///
/// Exactly like Node, `NUMBER` is always `decimal` (whatever its scale), the
/// whole `TIMESTAMP*` family and `DATE` are `timestamp`, and anything not in
/// the table is `text`.
pub fn oracle_to_generic(db_type_name: &str) -> GenericType {
    let name = db_type_name.to_lowercase();
    if name.starts_with("timestamp") {
        return GenericType::Timestamp;
    }
    match name.as_str() {
        "varchar2" | "nvarchar2" | "char" | "nchar" | "clob" | "nclob" | "long" => {
            GenericType::Text
        }
        "binary_float" => GenericType::Float,
        "binary_double" => GenericType::Double,
        "date" => GenericType::Timestamp,
        "number" => GenericType::Decimal(None),
        _ => GenericType::Text,
    }
}

/// Formats a date/time value the way node-oracledb's JS `Date` serialises
/// (`toJSON()`, i.e. UTC with millisecond precision and a trailing `Z`).
///
/// `DATE` and `TIMESTAMP` carry no zone: node-oracledb reads them as the
/// process' local time, which is UTC in Cube's images, so the wall-clock
/// value is emitted unchanged. For `TIMESTAMP WITH TIME ZONE` Oracle sends
/// the instant normalised to UTC (the offset travels alongside and is only
/// informative), so the fields are already the UTC instant.
pub fn format_timestamp(ts: &OracleTimestamp) -> Result<String> {
    let naive =
        chrono::NaiveDate::from_ymd_opt(ts.year().into(), ts.month().into(), ts.day().into())
            .and_then(|d| {
                d.and_hms_nano_opt(
                    ts.hour().into(),
                    ts.minute().into(),
                    ts.second().into(),
                    ts.nanoseconds(),
                )
            })
            .ok_or_else(|| DriverError::TypeDetection(format!("Invalid Oracle timestamp: {ts}")))?;
    Ok(naive.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
}

/// JS `String(number)` for the special floating point values.
fn float_to_string(v: f64) -> String {
    if v.is_nan() {
        "NaN".to_string()
    } else if v.is_infinite() {
        if v > 0.0 { "Infinity" } else { "-Infinity" }.to_string()
    } else {
        v.to_string()
    }
}

fn not_supported(type_name: &str) -> DriverError {
    DriverError::NotImplemented(format!(
        "The Oracle driver cannot fetch columns of type {type_name}; cast the column to a \
         supported type (e.g. TO_CHAR) in the query"
    ))
}

fn fetch_error(e: oracledb::Error) -> DriverError {
    DriverError::TypeDetection(e.to_string())
}

/// Converts the cell at `index` into JSON.
///
/// * `NUMBER` / `BINARY_*`: decimal string (exact for `NUMBER`), the same
///   representation the other Rust drivers give numerics.
/// * `DATE` / `TIMESTAMP*`: `YYYY-MM-DDTHH:MM:SS.mmmZ` (see [`format_timestamp`]).
/// * character types, `CLOB`/`NCLOB`/`LONG`, `ROWID`: string.
/// * `RAW`/`BLOB`/`LONG RAW`: base64 string.
/// * `BOOLEAN`: JSON boolean. `JSON`: the JSON value. Dense `VECTOR`: array.
/// * `INTERVAL`s: Oracle's text form.
/// * `CURSOR`, `OBJECT`, `XMLTYPE`, `BFILE`, sparse `VECTOR`:
///   [`DriverError::NotImplemented`].
pub fn cell_to_value(row: &Row, index: usize, meta: &Metadata) -> Result<Value> {
    let db_type = meta.db_type();
    let name = db_type_name(db_type);

    let value = match name {
        "NUMBER" | "BINARY_INTEGER" => row
            .get::<Option<OracleNumber>>(index)
            .map_err(fetch_error)?
            .map(|n| Value::String(n.to_string())),
        "BINARY_DOUBLE" => row
            .get::<Option<f64>>(index)
            .map_err(fetch_error)?
            .map(|v| Value::String(float_to_string(v))),
        // `f32::to_string` keeps the shortest representation of the f32
        // (0.1, not 0.10000000149011612).
        "BINARY_FLOAT" => row
            .get::<Option<f32>>(index)
            .map_err(fetch_error)?
            .map(|v| {
                Value::String(if v.is_finite() {
                    v.to_string()
                } else {
                    float_to_string(f64::from(v))
                })
            }),
        "DATE" | "TIMESTAMP" | "TIMESTAMP WITH LOCAL TIME ZONE" | "TIMESTAMP WITH TIME ZONE" => row
            .get::<Option<OracleTimestamp>>(index)
            .map_err(fetch_error)?
            .map(|ts| format_timestamp(&ts).map(Value::String))
            .transpose()?,
        "BOOLEAN" => row
            .get::<Option<bool>>(index)
            .map_err(fetch_error)?
            .map(Value::Bool),
        "RAW" | "BLOB" | "LONG RAW" => row
            .get::<Option<Vec<u8>>>(index)
            .map_err(fetch_error)?
            .map(|v| Value::String(base64::engine::general_purpose::STANDARD.encode(v))),
        "INTERVAL DAY TO SECOND" => row
            .get::<Option<oracledb::OracleIntervalDS>>(index)
            .map_err(fetch_error)?
            .map(|v| Value::String(v.to_string())),
        "INTERVAL YEAR TO MONTH" => row
            .get::<Option<oracledb::OracleIntervalYM>>(index)
            .map_err(fetch_error)?
            .map(|v| Value::String(v.to_string())),
        "JSON" => row
            .get::<Option<JsonValue>>(index)
            .map_err(fetch_error)?
            .map(|v| json_to_value(&v))
            .transpose()?,
        "VECTOR" => row
            .get::<Option<Vector>>(index)
            .map_err(fetch_error)?
            .map(|v| vector_to_value(&v))
            .transpose()?,
        "CURSOR" | "OBJECT" | "XMLTYPE" | "BFILE" | "UNKNOWN" => {
            return Err(not_supported(&format!(
                "{} (column \"{}\")",
                meta.data_type(),
                meta.name()
            )))
        }
        // VARCHAR2, NVARCHAR2, CHAR, NCHAR, LONG, CLOB, NCLOB, ROWID, UROWID.
        _ => row
            .get::<Option<String>>(index)
            .map_err(fetch_error)?
            .map(Value::String),
    };

    Ok(value.unwrap_or(Value::Null))
}

fn vector_data_to_value(data: &VectorData) -> Value {
    match data {
        VectorData::Float32(v) => Value::Array(
            v.iter()
                .map(|x| {
                    serde_json::Number::from_f64(f64::from(*x)).map_or(Value::Null, Value::Number)
                })
                .collect(),
        ),
        VectorData::Float64(v) => Value::Array(
            v.iter()
                .map(|x| serde_json::Number::from_f64(*x).map_or(Value::Null, Value::Number))
                .collect(),
        ),
        VectorData::Int8(v) => Value::Array(v.iter().map(|x| Value::from(*x)).collect()),
        VectorData::Binary(v) => Value::Array(v.iter().map(|x| Value::from(*x)).collect()),
    }
}

fn vector_to_value(vector: &Vector) -> Result<Value> {
    match vector {
        Vector::Dense(data) => Ok(vector_data_to_value(data)),
        #[allow(unreachable_patterns)]
        _ => Err(not_supported("VECTOR (sparse)")),
    }
}

/// Converts an Oracle `JSON` value into `serde_json`.
pub fn json_to_value(v: &JsonValue) -> Result<Value> {
    Ok(match v {
        JsonValue::Null => Value::Null,
        JsonValue::Boolean(b) => Value::Bool(*b),
        JsonValue::String(s) => Value::String(s.clone()),
        JsonValue::Number(n) => {
            let s = n.to_string();
            serde_json::from_str::<serde_json::Number>(&s)
                .map(Value::Number)
                .unwrap_or(Value::String(s))
        }
        JsonValue::BinaryDouble(f) => {
            serde_json::Number::from_f64(*f).map_or(Value::Null, Value::Number)
        }
        JsonValue::BinaryFloat(f) => {
            serde_json::Number::from_f64(f64::from(*f)).map_or(Value::Null, Value::Number)
        }
        JsonValue::Timestamp(ts) => Value::String(format_timestamp(ts)?),
        JsonValue::IntervalDS(i) => Value::String(i.to_string()),
        JsonValue::IntervalYM(i) => Value::String(i.to_string()),
        JsonValue::Raw(b) | JsonValue::JsonId(b) => {
            Value::String(base64::engine::general_purpose::STANDARD.encode(b))
        }
        JsonValue::JsonArray(items) => {
            Value::Array(items.iter().map(json_to_value).collect::<Result<_>>()?)
        }
        JsonValue::JsonObject(map) => {
            // A HashMap has no order; sort the keys for a stable output.
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = serde_json::Map::new();
            for k in keys {
                out.insert(k.clone(), json_to_value(&map[k])?);
            }
            Value::Object(out)
        }
        JsonValue::Vector(v) => vector_to_value(v)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_type_names_match_node_oracledb() {
        assert_eq!(db_type_name(oracledb::DB_TYPE_NUMBER), "NUMBER");
        assert_eq!(db_type_name(oracledb::DB_TYPE_VARCHAR), "VARCHAR2");
        assert_eq!(db_type_name(oracledb::DB_TYPE_NVARCHAR), "NVARCHAR2");
        assert_eq!(
            db_type_name(oracledb::DB_TYPE_TIMESTAMP_TZ),
            "TIMESTAMP WITH TIME ZONE"
        );
        assert_eq!(db_type_name(oracledb::DB_TYPE_UNKNOWN), "UNKNOWN");
    }

    // Port of the Node driver's `metaDataToColumnTypes` behaviour.
    #[test]
    fn meta_data_to_column_types() {
        let cases = [
            ("VARCHAR2", GenericType::Text),
            ("NVARCHAR2", GenericType::Text),
            ("CHAR", GenericType::Text),
            ("NCHAR", GenericType::Text),
            ("CLOB", GenericType::Text),
            ("NCLOB", GenericType::Text),
            ("LONG", GenericType::Text),
            ("BINARY_FLOAT", GenericType::Float),
            ("BINARY_DOUBLE", GenericType::Double),
            ("DATE", GenericType::Timestamp),
            ("NUMBER", GenericType::Decimal(None)),
            ("TIMESTAMP", GenericType::Timestamp),
            ("TIMESTAMP WITH TIME ZONE", GenericType::Timestamp),
            ("TIMESTAMP WITH LOCAL TIME ZONE", GenericType::Timestamp),
            ("RAW", GenericType::Text),
            ("BLOB", GenericType::Text),
            ("", GenericType::Text),
            ("number", GenericType::Decimal(None)),
        ];
        for (name, expected) in cases {
            assert_eq!(oracle_to_generic(name), expected, "{name}");
        }
    }

    #[test]
    fn timestamps_serialise_like_js_dates() {
        let ts = OracleTimestamp::new_timestamp(2020, 1, 2, 3, 4, 5, 123_456_789);
        assert_eq!(format_timestamp(&ts).unwrap(), "2020-01-02T03:04:05.123Z");
        let date = OracleTimestamp::new_date(2020, 1, 2);
        assert_eq!(format_timestamp(&date).unwrap(), "2020-01-02T00:00:00.000Z");
        // The fields of a TIMESTAMP WITH TIME ZONE are the UTC instant.
        let tz = OracleTimestamp::new_timestamp_tz(2020, 1, 1, 22, 30, 0, 0, 2, 30);
        assert_eq!(format_timestamp(&tz).unwrap(), "2020-01-01T22:30:00.000Z");
    }

    #[test]
    fn floats_stringify_like_js() {
        assert_eq!(float_to_string(1.5), "1.5");
        assert_eq!(float_to_string(f64::NAN), "NaN");
        assert_eq!(float_to_string(f64::NEG_INFINITY), "-Infinity");
    }

    #[test]
    fn json_values_convert() {
        let mut map = std::collections::HashMap::new();
        map.insert("b".to_string(), JsonValue::Number("1.50".parse().unwrap()));
        map.insert(
            "a".to_string(),
            JsonValue::JsonArray(vec![JsonValue::Boolean(true), JsonValue::Null]),
        );
        let v = json_to_value(&JsonValue::JsonObject(map)).unwrap();
        assert_eq!(v, serde_json::json!({"a": [true, null], "b": 1.5}));
    }
}
