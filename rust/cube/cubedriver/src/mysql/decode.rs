//! Decoding of MySQL text-protocol values into JSON.
//!
//! The Node driver configures `mysql2` with `dateStrings: true` and
//! `decimalNumbers: false`, so dates, times and decimals come back as strings
//! while the integer and floating point types come back as numbers. The text
//! protocol hands every cell over as bytes, so the column type decides.

use base64::Engine as _;
use mysql_async::consts::ColumnType;
use mysql_async::{Column, Value as MyValue};
use serde_json::Value;

use crate::mysql::types::is_unsigned;

/// `character_set` of a binary (as opposed to textual) column.
const BINARY_CHARSET: u16 = 63;

/// Converts one cell of a result row.
pub fn decode_value(column: &Column, value: &MyValue) -> Value {
    let bytes = match value {
        MyValue::NULL => return Value::Null,
        MyValue::Bytes(b) => b.as_slice(),
        // The text protocol only ever yields NULL or Bytes; the binary
        // protocol variants are converted directly for completeness.
        MyValue::Int(i) => return Value::from(*i),
        MyValue::UInt(u) => return Value::from(*u),
        MyValue::Float(f) => return f64::from(*f).into(),
        MyValue::Double(d) => return (*d).into(),
        MyValue::Date(..) | MyValue::Time(..) => return Value::String(value.as_sql(true)),
    };

    match column.column_type() {
        ColumnType::MYSQL_TYPE_TINY
        | ColumnType::MYSQL_TYPE_SHORT
        | ColumnType::MYSQL_TYPE_LONG
        | ColumnType::MYSQL_TYPE_INT24
        | ColumnType::MYSQL_TYPE_LONGLONG
        | ColumnType::MYSQL_TYPE_YEAR => decode_integer(bytes, is_unsigned(column.flags())),
        ColumnType::MYSQL_TYPE_FLOAT | ColumnType::MYSQL_TYPE_DOUBLE => decode_float(bytes),
        // `decimalNumbers: false` keeps decimals exact, as strings.
        ColumnType::MYSQL_TYPE_DECIMAL | ColumnType::MYSQL_TYPE_NEWDECIMAL => decode_string(bytes),
        // `dateStrings: true`: the server's own rendering, verbatim. The
        // temporal types report the binary character set, so they have to be
        // taken before the charset check below.
        ColumnType::MYSQL_TYPE_DATE
        | ColumnType::MYSQL_TYPE_NEWDATE
        | ColumnType::MYSQL_TYPE_DATETIME
        | ColumnType::MYSQL_TYPE_DATETIME2
        | ColumnType::MYSQL_TYPE_TIMESTAMP
        | ColumnType::MYSQL_TYPE_TIMESTAMP2
        | ColumnType::MYSQL_TYPE_TIME
        | ColumnType::MYSQL_TYPE_TIME2 => decode_string(bytes),
        ColumnType::MYSQL_TYPE_JSON => serde_json::from_slice(bytes).unwrap_or_else(|_| {
            // Invalid JSON cannot happen for a JSON column, but a lossy string
            // beats losing the row.
            decode_string(bytes)
        }),
        ColumnType::MYSQL_TYPE_BIT | ColumnType::MYSQL_TYPE_GEOMETRY => decode_binary(bytes),
        _ => {
            if column.character_set() == BINARY_CHARSET {
                decode_binary(bytes)
            } else {
                decode_string(bytes)
            }
        }
    }
}

fn decode_integer(bytes: &[u8], unsigned: bool) -> Value {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return decode_binary(bytes);
    };
    if unsigned {
        if let Ok(v) = text.parse::<u64>() {
            return Value::from(v);
        }
    }
    match text.parse::<i64>() {
        Ok(v) => Value::from(v),
        // Out of range for i64/u64: keep the exact digits rather than rounding.
        Err(_) => Value::String(text.to_string()),
    }
}

fn decode_float(bytes: &[u8]) -> Value {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return decode_binary(bytes);
    };
    match text.parse::<f64>() {
        Ok(v) => serde_json::Number::from_f64(v)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Err(_) => Value::String(text.to_string()),
    }
}

fn decode_string(bytes: &[u8]) -> Value {
    Value::String(String::from_utf8_lossy(bytes).into_owned())
}

fn decode_binary(bytes: &[u8]) -> Value {
    Value::String(base64::engine::general_purpose::STANDARD.encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mysql_async::consts::ColumnFlags;

    /// Builds a `Column` with just the fields the decoder reads.
    fn column(column_type: ColumnType, flags: ColumnFlags, character_set: u16) -> Column {
        Column::new(column_type)
            .with_name(b"c")
            .with_flags(flags)
            .with_character_set(character_set)
    }

    fn utf8(column_type: ColumnType) -> Column {
        column(column_type, ColumnFlags::empty(), 45)
    }

    #[test]
    fn integers_become_numbers() {
        let c = utf8(ColumnType::MYSQL_TYPE_LONGLONG);
        assert_eq!(
            decode_value(&c, &MyValue::Bytes(b"42".to_vec())),
            Value::from(42)
        );
        assert_eq!(
            decode_value(&c, &MyValue::Bytes(b"-42".to_vec())),
            Value::from(-42)
        );
        // exceeding i64 keeps the exact digits
        assert_eq!(
            decode_value(&c, &MyValue::Bytes(b"99999999999999999999".to_vec())),
            Value::from("99999999999999999999")
        );
        let unsigned = column(
            ColumnType::MYSQL_TYPE_LONGLONG,
            ColumnFlags::UNSIGNED_FLAG,
            45,
        );
        assert_eq!(
            decode_value(&unsigned, &MyValue::Bytes(b"18446744073709551615".to_vec())),
            Value::from(18446744073709551615u64)
        );
    }

    #[test]
    fn decimals_and_dates_stay_strings() {
        // temporal columns report the binary charset but are still rendered
        // by the server as text
        assert_eq!(
            decode_value(
                &column(ColumnType::MYSQL_TYPE_DATE, ColumnFlags::empty(), 63),
                &MyValue::Bytes(b"2020-01-01".to_vec())
            ),
            Value::from("2020-01-01")
        );
        assert_eq!(
            decode_value(
                &utf8(ColumnType::MYSQL_TYPE_NEWDECIMAL),
                &MyValue::Bytes(b"123.45".to_vec())
            ),
            Value::from("123.45")
        );
        assert_eq!(
            decode_value(
                &utf8(ColumnType::MYSQL_TYPE_DATETIME),
                &MyValue::Bytes(b"2020-01-01 00:00:00".to_vec())
            ),
            Value::from("2020-01-01 00:00:00")
        );
        assert_eq!(
            decode_value(
                &utf8(ColumnType::MYSQL_TYPE_DATE),
                &MyValue::Bytes(b"2020-01-01".to_vec())
            ),
            Value::from("2020-01-01")
        );
    }

    #[test]
    fn floats_become_numbers() {
        assert_eq!(
            decode_value(
                &utf8(ColumnType::MYSQL_TYPE_DOUBLE),
                &MyValue::Bytes(b"1.5".to_vec())
            ),
            Value::from(1.5)
        );
    }

    #[test]
    fn json_is_parsed() {
        assert_eq!(
            decode_value(
                &utf8(ColumnType::MYSQL_TYPE_JSON),
                &MyValue::Bytes(br#"{"a":1}"#.to_vec())
            ),
            serde_json::json!({"a": 1})
        );
    }

    #[test]
    fn binary_columns_are_base64() {
        let c = column(ColumnType::MYSQL_TYPE_BLOB, ColumnFlags::empty(), 63);
        assert_eq!(
            decode_value(&c, &MyValue::Bytes(vec![0, 1, 2])),
            Value::from("AAEC")
        );
        // ... while a text blob stays a string
        let c = column(ColumnType::MYSQL_TYPE_BLOB, ColumnFlags::empty(), 45);
        assert_eq!(
            decode_value(&c, &MyValue::Bytes(b"hi".to_vec())),
            Value::from("hi")
        );
    }

    #[test]
    fn null_is_null() {
        assert_eq!(
            decode_value(&utf8(ColumnType::MYSQL_TYPE_LONG), &MyValue::NULL),
            Value::Null
        );
    }
}
