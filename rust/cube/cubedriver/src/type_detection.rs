//! Port of `packages/cubejs-base-driver/src/type-detection.ts`:
//! infers generic column types from the values of a result set
//! (used by `downloadQueryResults` in read-only mode).

use serde_json::Value;

use crate::error::{DriverError, Result};
use crate::types::{Column, GenericType, QueryResult};

const DB_INT_MAX: i128 = 2_147_483_647;
const DB_INT_MIN: i128 = -2_147_483_648;
const DB_BIG_INT_MAX: i128 = 9_223_372_036_854_775_807;
const DB_BIG_INT_MIN: i128 = -9_223_372_036_854_775_808;

/// Rows inspected at most while looking for a non-null sample per column.
const DB_TYPE_DETECTION_MAX_ROWS: usize = 100;

/// JavaScript `v.toString()` for the values the matchers see.
fn js_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Array(items) => items
            .iter()
            .map(|i| match i {
                Value::Null => String::new(),
                other => js_string(other),
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_string(),
        Value::Null => "null".to_string(),
    }
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `^\d\d\d\d-\d\d-\d\dT\d\d:\d\d:\d\d`
fn matches_timestamp(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 19 {
        return false;
    }
    let pattern = b"dddd-dd-ddTdd:dd:dd";
    pattern.iter().zip(b.iter()).all(|(p, c)| match p {
        b'd' => c.is_ascii_digit(),
        other => other == c,
    })
}

/// `^\d\d\d\d-\d\d-\d\d$`
fn matches_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b"dddd-dd-dd".iter().zip(b.iter()).all(|(p, c)| match p {
            b'd' => c.is_ascii_digit(),
            other => other == c,
        })
}

/// `^-?\d+$`
fn matches_integer(s: &str) -> bool {
    let digits = s.strip_prefix('-').unwrap_or(s);
    is_digits(digits)
}

/// `^-?\d+(\.\d+)?$`
fn matches_decimal(s: &str) -> bool {
    let body = s.strip_prefix('-').unwrap_or(s);
    match body.split_once('.') {
        Some((int, frac)) => is_digits(int) && is_digits(frac),
        None => is_digits(body),
    }
}

fn integer_in_range(v: &Value, min: i128, max: i128) -> bool {
    if let Value::Number(n) = v {
        if let Some(i) = n.as_i64() {
            return (i as i128) <= max && (i as i128) >= min;
        }
        if let Some(u) = n.as_u64() {
            return (u as i128) <= max && (u as i128) >= min;
        }
        // Number.isInteger(1.0) is true in JS; serde keeps "1.0" as f64
        if let Some(f) = n.as_f64() {
            if f.fract() == 0.0 && f.is_finite() {
                let i = f as i128;
                return i <= max && i >= min;
            }
        }
    }
    let s = js_string(v);
    if matches_integer(&s) {
        return s
            .parse::<i128>()
            .map(|i| i <= max && i >= min)
            .unwrap_or(false);
    }
    false
}

type Matcher = fn(&Value) -> bool;

/// Ordered from most to least specific (`DbTypeValueMatcher`).
fn matchers() -> [(GenericType, Matcher); 8] {
    [
        (GenericType::Timestamp, |v| matches_timestamp(&js_string(v))),
        (GenericType::Date, |v| matches_date(&js_string(v))),
        (GenericType::Int, |v| {
            integer_in_range(v, DB_INT_MIN, DB_INT_MAX)
        }),
        (GenericType::Bigint, |v| {
            integer_in_range(v, DB_BIG_INT_MIN, DB_BIG_INT_MAX)
        }),
        (GenericType::Decimal(None), |v| {
            matches_decimal(&js_string(v))
        }),
        (GenericType::Boolean, |v| match v {
            Value::Bool(_) => true,
            other => {
                let s = js_string(other).to_lowercase();
                s == "true" || s == "false"
            }
        }),
        (GenericType::String, |v| match v {
            // `v.length < 256`: only strings and arrays have a length in JS
            Value::String(s) => s.chars().count() < 256,
            Value::Array(a) => a.len() < 256,
            _ => false,
        }),
        (GenericType::Text, |_| true),
    ]
}

/// Port of `detectTypesFromTabular`.
pub fn detect_types_from_tabular(result: &QueryResult) -> Result<Vec<Column>> {
    if result.rows.is_empty() {
        return Err(DriverError::TypeDetection(
            "Unable to detect column types for pre-aggregation on empty values in readOnly mode."
                .to_string(),
        ));
    }

    let n = result.columns.len();
    let mut samples: Vec<Vec<&Value>> = vec![Vec::new(); n];
    let mut unresolved = n;

    for row in result.rows.iter().take(DB_TYPE_DETECTION_MAX_ROWS) {
        for (i, sample) in samples.iter_mut().enumerate() {
            if let Some(v) = row.get(i) {
                if !v.is_null() {
                    if sample.is_empty() {
                        unresolved -= 1;
                    }
                    sample.push(v);
                }
            }
        }
        if unresolved == 0 {
            break;
        }
    }

    let matchers = matchers();
    Ok(result
        .columns
        .iter()
        .zip(samples.iter())
        .map(|(col, values)| {
            let type_ = if values.is_empty() {
                // A column that is NULL across every inspected row has no detectable type.
                GenericType::Text
            } else {
                matchers
                    .iter()
                    .find(|(_, m)| values.iter().all(|v| m(v)))
                    .map(|(t, _)| t.clone())
                    .unwrap_or(GenericType::Text)
            };
            Column::new(col.name.clone(), type_)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn detect(columns: &[&str], rows: Vec<Vec<Value>>) -> Vec<String> {
        let result = QueryResult::new(
            columns.iter().map(|c| Column::new(*c, "text")).collect(),
            rows,
        );
        detect_types_from_tabular(&result)
            .unwrap()
            .into_iter()
            .map(|c| c.type_.to_string())
            .collect()
    }

    #[test]
    fn detects_common_types() {
        let types = detect(
            &["ts", "d", "i", "bi", "dec", "b", "s", "t", "n"],
            vec![vec![
                json!("2020-01-01T00:00:00.000"),
                json!("2020-01-01"),
                json!(1),
                json!("9223372036854775807"),
                json!("1.5"),
                json!("true"),
                json!("hello"),
                json!("x".repeat(300)),
                Value::Null,
            ]],
        );
        assert_eq!(
            types,
            vec![
                "timestamp",
                "date",
                "int",
                "bigint",
                "decimal",
                "boolean",
                "string",
                "text",
                "text"
            ]
        );
    }

    #[test]
    fn scans_further_rows_for_nulls() {
        let types = detect(
            &["a", "b"],
            vec![
                vec![Value::Null, json!("x")],
                vec![json!("2147483648"), Value::Null],
            ],
        );
        assert_eq!(types, vec!["bigint", "string"]);
    }

    #[test]
    fn sampling_stops_once_every_column_has_a_value() {
        // `detectTypesFromTabular` breaks out of the scan as soon as
        // `unresolvedFields` is empty, so a single column is typed from its
        // first non-null value and later rows never widen it.
        let types = detect(
            &["a"],
            vec![vec![json!("1")], vec![json!("1.5")], vec![json!("x")]],
        );
        assert_eq!(types, vec!["int"]);
        let types = detect(&["a"], vec![vec![json!(-5)]]);
        assert_eq!(types, vec!["int"]);
    }

    #[test]
    fn mixed_values_fall_back() {
        // A column that stays null keeps the scan alive, so the other column
        // accumulates several values and a mixed set falls back to a wider type.
        let types = detect(
            &["a", "b"],
            vec![
                vec![json!("1"), Value::Null],
                vec![json!("1.5"), Value::Null],
                vec![json!("x"), json!("y")],
            ],
        );
        assert_eq!(types, vec!["string", "string"]);

        let types = detect(
            &["a", "b"],
            vec![
                vec![json!("1"), Value::Null],
                vec![json!("1.5"), json!("y")],
            ],
        );
        assert_eq!(types, vec!["decimal", "string"]);

        let types = detect(
            &["a", "b"],
            vec![
                vec![json!(true), Value::Null],
                vec![json!("false"), json!("y")],
            ],
        );
        assert_eq!(types, vec!["boolean", "string"]);
    }

    #[test]
    fn empty_rows_is_an_error() {
        let result = QueryResult::new(vec![Column::new("a", "text")], vec![]);
        let err = detect_types_from_tabular(&result).unwrap_err();
        assert!(err.to_string().contains("empty values in readOnly mode"));
    }
}
