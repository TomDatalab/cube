//! Conversion of a Cube Store response into the crate's [`QueryResult`].
//!
//! Replaces `parseCubestoreResultMessage` from `@cubejs-backend/native`: the
//! Arrow branch mirrors `cubeorchestrator::query_message_parser` (timestamps as
//! `%Y-%m-%dT%H:%M:%S%.3f`, decimals as strings), the legacy branch takes the
//! stringified cells straight off the wire.

use cubestore_ws_transport::arrow::array::{
    Array, ArrayRef, BinaryArray, BooleanArray, Date32Array, Date64Array, Decimal128Array,
    Decimal256Array, Float16Array, Float32Array, Float64Array, Int16Array, Int32Array, Int64Array,
    Int8Array, LargeBinaryArray, LargeStringArray, StringArray, StringViewArray,
    TimestampMicrosecondArray, TimestampMillisecondArray, TimestampNanosecondArray,
    TimestampSecondArray, UInt16Array, UInt32Array, UInt64Array, UInt8Array,
};
use cubestore_ws_transport::arrow::datatypes::{DataType, TimeUnit};
use cubestore_ws_transport::{QueryResult as TransportResult, ResultData};
use serde_json::Value;

use crate::error::{DriverError, Result};
use crate::types::{Column, GenericType, QueryResult, Row};

/// `%Y-%m-%dT%H:%M:%S%.3f`, the timestamp shape the rest of Cube expects.
const TIMESTAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.3f";

/// Arrow type → Cube generic type.
pub fn arrow_to_generic_type(data_type: &DataType) -> GenericType {
    match data_type {
        DataType::Boolean => GenericType::Boolean,
        DataType::Int8 | DataType::Int16 | DataType::Int32 => GenericType::Int,
        DataType::UInt8 | DataType::UInt16 | DataType::UInt32 => GenericType::Int,
        DataType::Int64 | DataType::UInt64 => GenericType::Bigint,
        DataType::Float16 | DataType::Float32 => GenericType::Float,
        DataType::Float64 => GenericType::Double,
        DataType::Decimal128(_, _) | DataType::Decimal256(_, _) => GenericType::Decimal(None),
        DataType::Date32 | DataType::Date64 => GenericType::Date,
        DataType::Timestamp(_, _) => GenericType::Timestamp,
        DataType::Time32(_) | DataType::Time64(_) => GenericType::String,
        _ => GenericType::Text,
    }
}

/// Converts a transport-level result into a [`QueryResult`].
pub fn to_query_result(result: &TransportResult) -> Result<QueryResult> {
    match &result.data {
        ResultData::Completed => Ok(QueryResult::default()),
        ResultData::Legacy { columns, rows } => {
            let cols: Vec<Column> = columns
                .iter()
                .map(|name| Column::new(name.clone(), GenericType::Text))
                .collect();
            let rows: Vec<Row> = rows
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|cell| match cell {
                            Some(v) => Value::String(v.clone()),
                            None => Value::Null,
                        })
                        .collect()
                })
                .collect();
            Ok(QueryResult::new(cols, rows))
        }
        ResultData::Arrow { schema, batches } => {
            let cols: Vec<Column> = schema
                .fields()
                .iter()
                .map(|f| Column::new(f.name().clone(), arrow_to_generic_type(f.data_type())))
                .collect();

            let total: usize = batches.iter().map(|b| b.num_rows()).sum();
            let mut rows: Vec<Row> = Vec::with_capacity(total);

            for batch in batches {
                // Column-major decode, then transpose: downcasting once per
                // column instead of once per cell.
                let mut columns: Vec<Vec<Value>> = Vec::with_capacity(batch.num_columns());
                for array in batch.columns() {
                    columns.push(array_to_values(array)?);
                }
                for i in 0..batch.num_rows() {
                    rows.push(columns.iter().map(|c| c[i].clone()).collect());
                }
            }

            Ok(QueryResult::new(cols, rows))
        }
    }
}

macro_rules! numeric_values {
    ($array:expr, $ty:ty, $len:expr) => {{
        let a = downcast::<$ty>($array)?;
        (0..$len)
            .map(|i| {
                if a.is_null(i) {
                    Value::Null
                } else {
                    Value::from(a.value(i))
                }
            })
            .collect()
    }};
}

macro_rules! string_values {
    ($array:expr, $ty:ty, $len:expr) => {{
        let a = downcast::<$ty>($array)?;
        (0..$len)
            .map(|i| {
                if a.is_null(i) {
                    Value::Null
                } else {
                    Value::String(a.value(i).to_string())
                }
            })
            .collect()
    }};
}

macro_rules! datetime_values {
    ($array:expr, $ty:ty, $len:expr) => {{
        let a = downcast::<$ty>($array)?;
        (0..$len)
            .map(|i| match (a.is_null(i), a.value_as_datetime(i)) {
                (false, Some(dt)) => Value::String(dt.format(TIMESTAMP_FORMAT).to_string()),
                _ => Value::Null,
            })
            .collect()
    }};
}

macro_rules! decimal_values {
    ($array:expr, $ty:ty, $len:expr) => {{
        let a = downcast::<$ty>($array)?;
        let scale = a.scale().max(0) as u32;
        (0..$len)
            .map(|i| {
                if a.is_null(i) {
                    Value::Null
                } else {
                    Value::String(decimal_to_string(a.value(i), scale))
                }
            })
            .collect()
    }};
}

macro_rules! binary_values {
    ($array:expr, $ty:ty, $len:expr) => {{
        use base64::Engine as _;
        let a = downcast::<$ty>($array)?;
        (0..$len)
            .map(|i| {
                if a.is_null(i) {
                    Value::Null
                } else {
                    Value::String(base64::engine::general_purpose::STANDARD.encode(a.value(i)))
                }
            })
            .collect()
    }};
}

fn downcast<T: 'static>(array: &ArrayRef) -> Result<&T> {
    array.as_any().downcast_ref::<T>().ok_or_else(|| {
        DriverError::TypeDetection(format!(
            "Unexpected Arrow array for type {:?}",
            array.data_type()
        ))
    })
}

fn array_to_values(array: &ArrayRef) -> Result<Vec<Value>> {
    let len = array.len();
    let values: Vec<Value> = match array.data_type() {
        DataType::Null => vec![Value::Null; len],
        DataType::Boolean => numeric_values!(array, BooleanArray, len),
        DataType::Int8 => numeric_values!(array, Int8Array, len),
        DataType::Int16 => numeric_values!(array, Int16Array, len),
        DataType::Int32 => numeric_values!(array, Int32Array, len),
        DataType::Int64 => numeric_values!(array, Int64Array, len),
        DataType::UInt8 => numeric_values!(array, UInt8Array, len),
        DataType::UInt16 => numeric_values!(array, UInt16Array, len),
        DataType::UInt32 => numeric_values!(array, UInt32Array, len),
        DataType::UInt64 => numeric_values!(array, UInt64Array, len),
        DataType::Float32 => numeric_values!(array, Float32Array, len),
        DataType::Float64 => numeric_values!(array, Float64Array, len),
        DataType::Float16 => {
            let a = downcast::<Float16Array>(array)?;
            (0..len)
                .map(|i| {
                    if a.is_null(i) {
                        Value::Null
                    } else {
                        Value::from(a.value(i).to_f64())
                    }
                })
                .collect()
        }
        DataType::Utf8 => string_values!(array, StringArray, len),
        DataType::LargeUtf8 => string_values!(array, LargeStringArray, len),
        DataType::Utf8View => string_values!(array, StringViewArray, len),
        DataType::Binary => binary_values!(array, BinaryArray, len),
        DataType::LargeBinary => binary_values!(array, LargeBinaryArray, len),
        DataType::Date32 => datetime_values!(array, Date32Array, len),
        DataType::Date64 => datetime_values!(array, Date64Array, len),
        DataType::Timestamp(TimeUnit::Second, _) => {
            datetime_values!(array, TimestampSecondArray, len)
        }
        DataType::Timestamp(TimeUnit::Millisecond, _) => {
            datetime_values!(array, TimestampMillisecondArray, len)
        }
        DataType::Timestamp(TimeUnit::Microsecond, _) => {
            datetime_values!(array, TimestampMicrosecondArray, len)
        }
        DataType::Timestamp(TimeUnit::Nanosecond, _) => {
            datetime_values!(array, TimestampNanosecondArray, len)
        }
        DataType::Decimal128(_, _) => decimal_values!(array, Decimal128Array, len),
        DataType::Decimal256(_, _) => decimal_values!(array, Decimal256Array, len),
        other => {
            return Err(DriverError::TypeDetection(format!(
                "Unsupported Arrow type in Cube Store response: {other:?}"
            )))
        }
    };
    Ok(values)
}

/// Renders a decimal mantissa with `scale` fractional digits
/// (`cubeorchestrator::query_message_parser::decimal_to_string`).
fn decimal_to_string<T: std::fmt::Display>(mantissa: T, scale: u32) -> String {
    let raw = mantissa.to_string();
    if scale == 0 {
        return raw;
    }

    let scale = scale as usize;
    let (sign, digits) = match raw.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", raw.as_str()),
    };

    let (int_part, frac) = if digits.len() > scale {
        let (int_part, frac) = digits.split_at(digits.len() - scale);
        (int_part, frac.to_string())
    } else {
        let pad = "0".repeat(scale - digits.len());
        ("0", format!("{pad}{digits}"))
    };

    let frac = frac.trim_end_matches('0');
    if frac.is_empty() {
        format!("{sign}{int_part}")
    } else {
        format!("{sign}{int_part}.{frac}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cubestore_ws_transport::arrow::array::RecordBatch;
    use cubestore_ws_transport::arrow::datatypes::{Field, Schema};
    use std::sync::Arc;

    #[test]
    fn decimal_rendering() {
        assert_eq!(decimal_to_string(12345i128, 2), "123.45");
        assert_eq!(decimal_to_string(-12345i128, 2), "-123.45");
        assert_eq!(decimal_to_string(5i128, 3), "0.005");
        assert_eq!(decimal_to_string(1200i128, 2), "12");
        assert_eq!(decimal_to_string(42i128, 0), "42");
    }

    #[test]
    fn legacy_rows_become_strings() {
        let result = TransportResult {
            data: ResultData::Legacy {
                columns: vec!["a".into(), "b".into()],
                rows: vec![vec![Some("1".into()), None]],
            },
        };
        let converted = to_query_result(&result).unwrap();
        assert_eq!(
            converted.columns,
            vec![Column::new("a", "text"), Column::new("b", "text")]
        );
        assert_eq!(converted.rows, vec![vec![Value::from("1"), Value::Null]]);
    }

    #[test]
    fn completed_is_an_empty_result() {
        let result = TransportResult {
            data: ResultData::Completed,
        };
        let converted = to_query_result(&result).unwrap();
        assert!(converted.is_empty());
        assert!(converted.columns.is_empty());
    }

    #[test]
    fn arrow_batches_are_converted() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("i", DataType::Int64, true),
            Field::new("s", DataType::Utf8, true),
            Field::new("t", DataType::Timestamp(TimeUnit::Millisecond, None), true),
            Field::new("d", DataType::Decimal128(10, 2), true),
            Field::new("b", DataType::Boolean, true),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![Some(7), None])),
                Arc::new(StringArray::from(vec![Some("x"), None])),
                Arc::new(TimestampMillisecondArray::from(vec![Some(1_500), None])),
                Arc::new(
                    Decimal128Array::from(vec![Some(12345i128), None])
                        .with_precision_and_scale(10, 2)
                        .unwrap(),
                ),
                Arc::new(BooleanArray::from(vec![Some(true), None])),
            ],
        )
        .unwrap();

        let converted = to_query_result(&TransportResult {
            data: ResultData::Arrow {
                schema,
                batches: vec![batch],
            },
        })
        .unwrap();

        assert_eq!(
            converted.columns,
            vec![
                Column::new("i", "bigint"),
                Column::new("s", "text"),
                Column::new("t", "timestamp"),
                Column::new("d", "decimal"),
                Column::new("b", "boolean"),
            ]
        );
        assert_eq!(
            converted.rows[0],
            vec![
                Value::from(7),
                Value::from("x"),
                Value::from("1970-01-01T00:00:01.500"),
                Value::from("123.45"),
                Value::from(true),
            ]
        );
        assert!(converted.rows[1].iter().all(|v| v.is_null()));
    }
}
