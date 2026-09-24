//! Per-column value converters for ClickHouse responses.
//!
//! Port of `packages/cubejs-clickhouse-driver/src/Transform.ts`: the JSON(Compact)
//! payload carries dates, date-times and numbers as ClickHouse formats them, and
//! the driver normalises them to the shapes the rest of Cube expects —
//! `YYYY-MM-DDTHH:MM:SS.mmm` for date/time values and strings for every numeric
//! type (so large integers and decimals keep their precision).

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde_json::Value;

/// `moment.HTML5_FMT.DATETIME_LOCAL_MS`.
const OUTPUT_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.3f";
const ZEROS: &str = "000";

/// The converter applied to every cell of one column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnConverter {
    /// `dateConverter`: append a zero time-of-day.
    Date,
    /// `DATE_TIME_CONVERTERS[precision]`: fast paths keyed on the exact width a
    /// `DateTime`/`DateTime64(p)` column produces.
    DateTime(u8),
    /// `dateTimeConverter`: always goes through the general formatter.
    DateTimeGeneric,
    /// `numberConverter`: stringify.
    Number,
}

const DATE_TIME64: &str = "DateTime64";
const DEFAULT_DATE_TIME64_PRECISION: i32 = 3;
const MAX_DATE_TIME64_PRECISION: i32 = 9;
const PRECISION_UNSUPPORTED: i32 = -1;

const WRAPPER_PREFIXES: &[&str] = &["Nullable(", "LowCardinality("];

/// Scalar names inside container arguments must not select a converter for the
/// container itself. `SimpleAggregateFunction` is excluded because it reads
/// back as its scalar argument type.
const NON_SCALAR_PREFIXES: &[&str] = &[
    "Array(",
    "Map(",
    "Tuple(",
    "Nested(",
    "Enum",
    "JSON",
    "AggregateFunction(",
];

/// `unwrapScalar`: strips the wrapper prefixes (but not their closing
/// parentheses, exactly as the TypeScript does).
fn unwrap_scalar(type_: &str) -> &str {
    let mut inner = type_;
    let mut stripped = true;
    while stripped {
        stripped = false;
        for prefix in WRAPPER_PREFIXES {
            if let Some(rest) = inner.strip_prefix(prefix) {
                inner = rest;
                stripped = true;
            }
        }
    }
    inner
}

/// `parseDateTimePrecision`.
fn parse_date_time_precision(inner: &str) -> i32 {
    let Some(at) = inner.find(DATE_TIME64) else {
        return 0;
    };
    let rest = &inner[at + DATE_TIME64.len()..];
    let Some(rest) = rest.strip_prefix('(') else {
        return DEFAULT_DATE_TIME64_PRECISION;
    };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return PRECISION_UNSUPPORTED;
    }
    match digits.parse::<i32>() {
        Ok(p) if p <= MAX_DATE_TIME64_PRECISION => p,
        _ => PRECISION_UNSUPPORTED,
    }
}

/// `getColumnConverter`.
pub fn column_converter(type_: &str) -> Option<ColumnConverter> {
    let inner = unwrap_scalar(type_);

    if NON_SCALAR_PREFIXES.iter().any(|p| inner.starts_with(p)) {
        return None;
    }

    if inner.contains("Date") {
        if !inner.contains("DateTime") {
            return Some(ColumnConverter::Date);
        }
        let precision = parse_date_time_precision(inner);
        return Some(if precision == PRECISION_UNSUPPORTED {
            ColumnConverter::DateTimeGeneric
        } else {
            ColumnConverter::DateTime(precision as u8)
        });
    }

    if inner.contains("Int") || inner.contains("Float") || inner.contains("Decimal") {
        return Some(ColumnConverter::Number);
    }

    None
}

/// Applies a converter to one value (`null` passes through untouched).
pub fn convert(converter: ColumnConverter, value: &Value) -> Value {
    if value.is_null() {
        return value.clone();
    }
    match converter {
        ColumnConverter::Date => match value.as_str() {
            Some(s) => Value::String(format!("{s}T00:00:00.000")),
            // JS template literal: `${value}T00:00:00.000` for anything.
            None => Value::String(format!("{}T00:00:00.000", js_string(value))),
        },
        ColumnConverter::Number => Value::String(match value {
            Value::String(s) => s.clone(),
            other => js_string(other),
        }),
        ColumnConverter::DateTimeGeneric => Value::String(format_date_time(value)),
        ColumnConverter::DateTime(precision) => Value::String(convert_date_time(precision, value)),
    }
}

fn convert_date_time(precision: u8, value: &Value) -> String {
    if let Some(s) = value.as_str() {
        let bytes = s.as_bytes();
        let fraction_dot = bytes.get(19) == Some(&b'.');
        match precision {
            0 if bytes.len() == 19 => {
                return format!("{}T{}.{ZEROS}", &s[0..10], &s[11..19]);
            }
            1 if bytes.len() == 21 && fraction_dot => {
                return format!("{}T{}.{}00", &s[0..10], &s[11..19], &s[20..21]);
            }
            2 if bytes.len() == 22 && fraction_dot => {
                return format!("{}T{}.{}0", &s[0..10], &s[11..19], &s[20..22]);
            }
            // `createTruncatingDateTimeConverter(width)` for precision 3..=9,
            // where width == precision + 20.
            p if (3..=9).contains(&p) && bytes.len() == p as usize + 20 && fraction_dot => {
                return format!("{}T{}", &s[0..10], &s[11..23]);
            }
            _ => {}
        }
    }
    format_date_time(value)
}

/// `hasCanonicalDateTimePrefix`.
fn has_canonical_date_time_prefix(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 19 {
        return false;
    }
    let separator = b[10];
    if b[4] != b'-'
        || b[7] != b'-'
        || (separator != b' ' && separator != b'T')
        || b[13] != b':'
        || b[16] != b':'
    {
        return false;
    }
    [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18]
        .iter()
        .all(|i| b[*i].is_ascii_digit())
}

/// `formatCanonicalDateTime`: a fast path that skips the general parser for the
/// `simple` and ISO shapes, which are already equivalent.
pub fn format_canonical_date_time(s: &str) -> Option<String> {
    if !has_canonical_date_time_prefix(s) {
        return None;
    }

    let b = s.as_bytes();
    let len = b.len();
    let mut millis = ZEROS.to_string();
    let mut tail = 19usize;

    if len > 19 && b[19] == b'.' {
        let mut end = 20;
        while end < len && b[end].is_ascii_digit() {
            end += 1;
        }
        let fraction_length = end - 20;
        if fraction_length == 0 {
            return None;
        }
        millis = if fraction_length >= 3 {
            s[20..23].to_string()
        } else {
            format!("{}{}", &s[20..end], &ZEROS[fraction_length..])
        };
        tail = end;
    }

    // Offsets require a timezone shift, which only the general parser does.
    if tail != len && !(tail == len - 1 && b[tail] == b'Z') {
        return None;
    }

    Some(format!("{}T{}.{millis}", &s[0..10], &s[11..19]))
}

/// `formatDateTime`: the canonical fast path, then the general parser.
pub fn format_date_time(value: &Value) -> String {
    if let Some(s) = value.as_str() {
        if let Some(formatted) = format_canonical_date_time(s) {
            return formatted;
        }
    }
    parse_to_utc(value)
        .map(|dt| dt.format(OUTPUT_FORMAT).to_string())
        // `moment(...).format()` on an unparsable input.
        .unwrap_or_else(|| "Invalid date".to_string())
}

/// Stand-in for `moment.utc(value)`: the shapes ClickHouse can produce for a
/// date/time column under any `date_time_output_format`.
fn parse_to_utc(value: &Value) -> Option<NaiveDateTime> {
    match value {
        // `unix_timestamp` output format, as a number or a numeric string.
        Value::Number(n) => n
            .as_i64()
            .and_then(|secs| DateTime::<Utc>::from_timestamp(secs, 0))
            .map(|dt| dt.naive_utc()),
        Value::String(s) => {
            let s = s.trim();
            if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
                return Some(dt.naive_utc());
            }
            for format in [
                "%Y-%m-%d %H:%M:%S%.f",
                "%Y-%m-%dT%H:%M:%S%.f",
                "%Y-%m-%d %H:%M:%S%.f%#z",
                "%Y-%m-%dT%H:%M:%S%.f%#z",
            ] {
                if let Ok(dt) = DateTime::parse_from_str(s, format) {
                    return Some(dt.naive_utc());
                }
                if let Ok(dt) = NaiveDateTime::parse_from_str(s, format) {
                    return Some(dt);
                }
            }
            if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
                return d.and_hms_opt(0, 0, 0);
            }
            s.parse::<i64>()
                .ok()
                .and_then(|secs| DateTime::<Utc>::from_timestamp(secs, 0))
                .map(|dt| dt.naive_utc())
        }
        _ => None,
    }
}

/// `String(value)` for a JSON scalar.
fn js_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn selects_converters_by_type() {
        assert_eq!(column_converter("Date"), Some(ColumnConverter::Date));
        assert_eq!(column_converter("Date32"), Some(ColumnConverter::Date));
        assert_eq!(
            column_converter("DateTime"),
            Some(ColumnConverter::DateTime(0))
        );
        assert_eq!(
            column_converter("DateTime('UTC')"),
            Some(ColumnConverter::DateTime(0))
        );
        assert_eq!(
            column_converter("DateTime64(3)"),
            Some(ColumnConverter::DateTime(3))
        );
        assert_eq!(
            column_converter("DateTime64(6, 'UTC')"),
            Some(ColumnConverter::DateTime(6))
        );
        // no explicit precision → the ClickHouse default of 3
        assert_eq!(
            column_converter("DateTime64"),
            Some(ColumnConverter::DateTime(3))
        );
        // out of range → the general formatter
        assert_eq!(
            column_converter("DateTime64(12)"),
            Some(ColumnConverter::DateTimeGeneric)
        );
        assert_eq!(
            column_converter("Nullable(DateTime64(3))"),
            Some(ColumnConverter::DateTime(3))
        );
        assert_eq!(column_converter("LowCardinality(Nullable(String))"), None);
        assert_eq!(column_converter("Int64"), Some(ColumnConverter::Number));
        assert_eq!(column_converter("UInt8"), Some(ColumnConverter::Number));
        assert_eq!(
            column_converter("Decimal(10, 2)"),
            Some(ColumnConverter::Number)
        );
        assert_eq!(column_converter("Float64"), Some(ColumnConverter::Number));
        assert_eq!(column_converter("String"), None);
        // containers keep their raw JSON
        assert_eq!(column_converter("Array(DateTime)"), None);
        assert_eq!(column_converter("Map(String, Int32)"), None);
        assert_eq!(column_converter("Tuple(Int32)"), None);
        assert_eq!(column_converter("Enum8('a' = 1)"), None);
        assert_eq!(column_converter("JSON"), None);
        assert_eq!(column_converter("AggregateFunction(sum, Int64)"), None);
        // ... while SimpleAggregateFunction reads back as its argument
        assert_eq!(
            column_converter("SimpleAggregateFunction(max, DateTime64(3))"),
            Some(ColumnConverter::DateTime(3))
        );
    }

    #[test]
    fn dates_get_a_zero_time() {
        assert_eq!(
            convert(ColumnConverter::Date, &json!("2020-01-01")),
            json!("2020-01-01T00:00:00.000")
        );
        assert_eq!(convert(ColumnConverter::Date, &Value::Null), Value::Null);
    }

    #[test]
    fn numbers_are_stringified() {
        assert_eq!(convert(ColumnConverter::Number, &json!(42)), json!("42"));
        assert_eq!(convert(ColumnConverter::Number, &json!(1.5)), json!("1.5"));
        assert_eq!(
            convert(ColumnConverter::Number, &json!("123.45")),
            json!("123.45")
        );
        assert_eq!(convert(ColumnConverter::Number, &Value::Null), Value::Null);
    }

    #[test]
    fn date_times_normalise_per_precision() {
        assert_eq!(
            convert(ColumnConverter::DateTime(0), &json!("2020-01-01 12:34:56")),
            json!("2020-01-01T12:34:56.000")
        );
        assert_eq!(
            convert(
                ColumnConverter::DateTime(1),
                &json!("2020-01-01 12:34:56.7")
            ),
            json!("2020-01-01T12:34:56.700")
        );
        assert_eq!(
            convert(
                ColumnConverter::DateTime(2),
                &json!("2020-01-01 12:34:56.78")
            ),
            json!("2020-01-01T12:34:56.780")
        );
        assert_eq!(
            convert(
                ColumnConverter::DateTime(3),
                &json!("2020-01-01 12:34:56.789")
            ),
            json!("2020-01-01T12:34:56.789")
        );
        // precision 6 truncates to milliseconds
        assert_eq!(
            convert(
                ColumnConverter::DateTime(6),
                &json!("2020-01-01 12:34:56.789123")
            ),
            json!("2020-01-01T12:34:56.789")
        );
    }

    #[test]
    fn date_times_fall_back_to_the_general_formatter() {
        // an ISO value on a precision-0 column (width does not match)
        assert_eq!(
            convert(
                ColumnConverter::DateTime(0),
                &json!("2020-01-01T12:34:56.789Z")
            ),
            json!("2020-01-01T12:34:56.789")
        );
        // an offset needs a real timezone shift
        assert_eq!(
            convert(
                ColumnConverter::DateTimeGeneric,
                &json!("2020-01-01T12:34:56+03:00")
            ),
            json!("2020-01-01T09:34:56.000")
        );
        // unix_timestamp output
        assert_eq!(
            convert(ColumnConverter::DateTimeGeneric, &json!(1_577_882_096)),
            json!("2020-01-01T12:34:56.000")
        );
        assert_eq!(
            convert(ColumnConverter::DateTimeGeneric, &json!("nonsense")),
            json!("Invalid date")
        );
    }

    #[test]
    fn canonical_fast_path() {
        assert_eq!(
            format_canonical_date_time("2020-01-01 12:34:56").as_deref(),
            Some("2020-01-01T12:34:56.000")
        );
        assert_eq!(
            format_canonical_date_time("2020-01-01T12:34:56.5Z").as_deref(),
            Some("2020-01-01T12:34:56.500")
        );
        assert_eq!(
            format_canonical_date_time("2020-01-01T12:34:56.123456").as_deref(),
            Some("2020-01-01T12:34:56.123")
        );
        // an offset is not canonical
        assert_eq!(
            format_canonical_date_time("2020-01-01 12:34:56+03:00"),
            None
        );
        assert_eq!(format_canonical_date_time("2020-01-01"), None);
        assert_eq!(format_canonical_date_time("not a date at all"), None);
    }
}
