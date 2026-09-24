//! DuckDB value conversion: port of `transformRow` / `HydrationStream` on top
//! of the values `node-duckdb` hands to JavaScript.
//!
//! `node-duckdb` turns integers into `number`/`bigint`, `DECIMAL` into a
//! `number`, `DATE`/`TIMESTAMP*` into a `Date`, and `transformRow` then
//! stringifies every top-level number/bigint and turns every top-level `Date`
//! into `toISOString()`. The functions here produce the same JSON directly.

use ::duckdb::core::LogicalTypeId;
use ::duckdb::types::{TimeUnit, Value as DuckValue, ValueRef};
use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{json, Map, Value};

/// JSON parameter → DuckDB value.
pub fn to_duck_value(value: &Value) -> DuckValue {
    match value {
        Value::Null => DuckValue::Null,
        Value::Bool(b) => DuckValue::Boolean(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                DuckValue::BigInt(i)
            } else if let Some(u) = n.as_u64() {
                DuckValue::UBigInt(u)
            } else {
                DuckValue::Double(n.as_f64().unwrap_or(f64::NAN))
            }
        }
        Value::String(s) => DuckValue::Text(s.clone()),
        other => DuckValue::Text(other.to_string()),
    }
}

/// JavaScript `Number.prototype.toString()` for a finite or non-finite `f64`.
pub fn js_number_string(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    if f == 0.0 {
        return "0".to_string();
    }
    let abs = f.abs();
    if (1e-6..1e21).contains(&abs) {
        // Rust's `Display` is the shortest round-trip representation in
        // positional notation, which is what JS prints in this range.
        return format!("{f}");
    }
    // Exponential notation: `1e+21`, `1.5e-7`.
    let e = format!("{f:e}");
    match e.split_once('e') {
        Some((mantissa, exp)) if exp.starts_with('-') => format!("{mantissa}e{exp}"),
        Some((mantissa, exp)) => format!("{mantissa}e+{exp}"),
        None => e,
    }
}

/// Exact decimal string of `value / 10^scale`, without trailing zeros (the
/// JS `number` form of small decimals, without the precision loss).
pub fn decimal_string(value: i128, scale: u8) -> String {
    let negative = value < 0;
    let digits = value.unsigned_abs().to_string();
    let scale = usize::from(scale);
    let (int_part, frac_part) = if digits.len() > scale {
        let (i, f) = digits.split_at(digits.len() - scale);
        (i.to_string(), f.to_string())
    } else {
        ("0".to_string(), format!("{digits:0>scale$}"))
    };
    let frac = frac_part.trim_end_matches('0');
    let body = if frac.is_empty() {
        int_part
    } else {
        format!("{int_part}.{frac}")
    };
    if negative && body != "0" {
        format!("-{body}")
    } else {
        body
    }
}

/// `Date.prototype.toISOString()` of a microsecond timestamp (millisecond
/// precision, `Z` suffix).
pub fn iso_from_micros(micros: i64) -> String {
    let millis = micros.div_euclid(1000);
    match DateTime::<Utc>::from_timestamp_millis(millis) {
        Some(dt) => dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
        None => micros.to_string(),
    }
}

/// `toISOString()` of a `DATE` (days since the epoch, UTC midnight).
pub fn iso_from_days(days: i32) -> String {
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).expect("valid date");
    match epoch.checked_add_signed(chrono::Duration::days(i64::from(days))) {
        Some(d) => format!("{}T00:00:00.000Z", d.format("%Y-%m-%d")),
        None => days.to_string(),
    }
}

/// `TIME` as DuckDB prints it (`HH:MM:SS[.ffffff]`).
pub fn time_string(unit: TimeUnit, value: i64) -> String {
    let micros = unit.to_micros(value);
    let secs = micros.div_euclid(1_000_000);
    let frac = micros.rem_euclid(1_000_000);
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if frac == 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{h:02}:{m:02}:{s:02}.{frac:06}")
    }
}

/// Top-level cell: `transformRow` semantics (numbers become strings, dates
/// become ISO strings).
pub fn cell_to_json(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::TinyInt(i) => Value::String(i.to_string()),
        ValueRef::SmallInt(i) => Value::String(i.to_string()),
        ValueRef::Int(i) => Value::String(i.to_string()),
        ValueRef::BigInt(i) => Value::String(i.to_string()),
        ValueRef::HugeInt(i) => Value::String(i.to_string()),
        ValueRef::UHugeInt(i) => Value::String(i.to_string()),
        ValueRef::UTinyInt(i) => Value::String(i.to_string()),
        ValueRef::USmallInt(i) => Value::String(i.to_string()),
        ValueRef::UInt(i) => Value::String(i.to_string()),
        ValueRef::UBigInt(i) => Value::String(i.to_string()),
        ValueRef::Float(f) => Value::String(js_number_string(f64::from(f))),
        ValueRef::Double(f) => Value::String(js_number_string(f)),
        ValueRef::Decimal(d) => Value::String(decimal_string(d.value(), d.scale())),
        other => owned_to_json(&other.to_owned()),
    }
}

/// Nested (or non-numeric top-level) value: plain JSON, as `JSON.stringify`
/// renders what `node-duckdb` returns.
pub fn owned_to_json(value: &DuckValue) -> Value {
    match value {
        DuckValue::Null => Value::Null,
        DuckValue::Boolean(b) => Value::Bool(*b),
        DuckValue::TinyInt(i) => json!(i),
        DuckValue::SmallInt(i) => json!(i),
        DuckValue::Int(i) => json!(i),
        DuckValue::BigInt(i) => json!(i),
        DuckValue::HugeInt(i) => Value::String(i.to_string()),
        DuckValue::UHugeInt(i) => Value::String(i.to_string()),
        DuckValue::UTinyInt(i) => json!(i),
        DuckValue::USmallInt(i) => json!(i),
        DuckValue::UInt(i) => json!(i),
        DuckValue::UBigInt(i) => json!(i),
        DuckValue::Float(f) => float_json(f64::from(*f)),
        DuckValue::Double(f) => float_json(*f),
        DuckValue::Decimal(d) => {
            let s = decimal_string(d.value(), d.scale());
            s.parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number)
                .unwrap_or(Value::String(s))
        }
        DuckValue::Timestamp(unit, v) => Value::String(iso_from_micros(unit.to_micros(*v))),
        DuckValue::Text(s) => Value::String(s.clone()),
        DuckValue::Enum(s) => Value::String(s.clone()),
        DuckValue::Blob(b) | DuckValue::Geometry(b) => json!({ "type": "Buffer", "data": b }),
        DuckValue::Date32(d) => Value::String(iso_from_days(*d)),
        DuckValue::Time64(unit, v) => Value::String(time_string(*unit, *v)),
        DuckValue::Interval {
            months,
            days,
            nanos,
        } => json!({ "months": months, "days": days, "micros": nanos / 1000 }),
        DuckValue::List(items) | DuckValue::Array(items) => {
            Value::Array(items.iter().map(owned_to_json).collect())
        }
        DuckValue::Struct(fields) => {
            let mut map = Map::new();
            for (k, v) in fields.iter() {
                map.insert(k.clone(), owned_to_json(v));
            }
            Value::Object(map)
        }
        DuckValue::Map(entries) => {
            let string_keys = entries
                .iter()
                .all(|(k, _)| matches!(k, DuckValue::Text(_) | DuckValue::Enum(_)));
            if string_keys {
                let mut map = Map::new();
                for (k, v) in entries.iter() {
                    if let DuckValue::Text(k) | DuckValue::Enum(k) = k {
                        map.insert(k.clone(), owned_to_json(v));
                    }
                }
                Value::Object(map)
            } else {
                Value::Array(
                    entries
                        .iter()
                        .map(|(k, v)| json!({ "key": owned_to_json(k), "value": owned_to_json(v) }))
                        .collect(),
                )
            }
        }
        DuckValue::Union(inner) => owned_to_json(inner),
        #[allow(unreachable_patterns)]
        other => Value::String(format!("{other:?}")),
    }
}

fn float_json(f: f64) -> Value {
    serde_json::Number::from_f64(f)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

/// DuckDB type name of a result column (the spelling
/// `information_schema.columns.data_type` uses), for the generic type mapping.
pub fn logical_type_name(id: LogicalTypeId, decimal: Option<(u8, u8)>) -> String {
    match id {
        LogicalTypeId::Boolean => "boolean".to_string(),
        LogicalTypeId::Tinyint => "tinyint".to_string(),
        LogicalTypeId::Smallint => "smallint".to_string(),
        LogicalTypeId::Integer => "integer".to_string(),
        LogicalTypeId::Bigint => "bigint".to_string(),
        LogicalTypeId::UTinyint => "utinyint".to_string(),
        LogicalTypeId::USmallint => "usmallint".to_string(),
        LogicalTypeId::UInteger => "uinteger".to_string(),
        LogicalTypeId::UBigint => "ubigint".to_string(),
        LogicalTypeId::Hugeint => "hugeint".to_string(),
        LogicalTypeId::UHugeint => "uhugeint".to_string(),
        LogicalTypeId::Float => "float".to_string(),
        LogicalTypeId::Double => "double".to_string(),
        LogicalTypeId::Decimal => match decimal {
            Some((w, s)) => format!("decimal({w},{s})"),
            None => "decimal".to_string(),
        },
        LogicalTypeId::Date => "date".to_string(),
        LogicalTypeId::Timestamp
        | LogicalTypeId::TimestampS
        | LogicalTypeId::TimestampMs
        | LogicalTypeId::TimestampNs => "timestamp".to_string(),
        LogicalTypeId::TimestampTZ => "timestamp with time zone".to_string(),
        LogicalTypeId::Time | LogicalTypeId::TimeNs => "time".to_string(),
        LogicalTypeId::TimeTZ => "time with time zone".to_string(),
        LogicalTypeId::Interval => "interval".to_string(),
        LogicalTypeId::Varchar => "varchar".to_string(),
        LogicalTypeId::Blob => "blob".to_string(),
        LogicalTypeId::Uuid => "uuid".to_string(),
        LogicalTypeId::Enum => "enum".to_string(),
        LogicalTypeId::List | LogicalTypeId::Array => "list".to_string(),
        LogicalTypeId::Struct => "struct".to_string(),
        LogicalTypeId::Map => "map".to_string(),
        _ => "text".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_numbers() {
        assert_eq!(js_number_string(100.0), "100");
        assert_eq!(js_number_string(0.1), "0.1");
        assert_eq!(js_number_string(-2.5), "-2.5");
        assert_eq!(js_number_string(1e21), "1e+21");
        assert_eq!(js_number_string(1.5e-7), "1.5e-7");
        assert_eq!(js_number_string(123456789012.0), "123456789012");
        assert_eq!(js_number_string(f64::NAN), "NaN");
        assert_eq!(js_number_string(-0.0), "0");
    }

    #[test]
    fn decimals() {
        assert_eq!(decimal_string(100_000, 3), "100");
        assert_eq!(decimal_string(123_450, 3), "123.45");
        assert_eq!(decimal_string(-5, 3), "-0.005");
        assert_eq!(decimal_string(0, 2), "0");
        assert_eq!(decimal_string(42, 0), "42");
    }

    #[test]
    fn dates() {
        assert_eq!(
            iso_from_micros(1_577_840_461_111_110),
            "2020-01-01T01:01:01.111Z"
        );
        assert_eq!(iso_from_days(18262), "2020-01-01T00:00:00.000Z");
        assert_eq!(iso_from_micros(-1), "1969-12-31T23:59:59.999Z");
        assert_eq!(
            time_string(TimeUnit::Microsecond, 3_723_000_000),
            "01:02:03"
        );
        assert_eq!(
            time_string(TimeUnit::Microsecond, 3_723_500_000),
            "01:02:03.500000"
        );
    }

    #[test]
    fn params() {
        assert_eq!(to_duck_value(&json!(1)), DuckValue::BigInt(1));
        assert_eq!(to_duck_value(&json!(1.5)), DuckValue::Double(1.5));
        assert_eq!(to_duck_value(&json!("x")), DuckValue::Text("x".into()));
        assert_eq!(to_duck_value(&json!(null)), DuckValue::Null);
        assert_eq!(to_duck_value(&json!(true)), DuckValue::Boolean(true));
    }
}
