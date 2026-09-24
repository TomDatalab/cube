//! Port of the parts of `data-api-client` (v2.4.1) that the Node driver
//! relies on: named-parameter formatting, result hydration and the retry
//! policy for paused / unreachable Aurora clusters.
//!
//! `data-api-client` is initialised by the Node driver without an `engine`,
//! so it runs with its default engine, `pg`: that only matters for JSON
//! object parameters (the placeholder gets a `::jsonb` cast) and `CAST`
//! hints, and is reproduced as is.

use std::collections::HashMap;
use std::time::Duration;

use aws_sdk_rdsdata::primitives::Blob;
use aws_sdk_rdsdata::types::{ArrayValue, Field, SqlParameter, TypeHint};
use base64::Engine as _;
use chrono::{DateTime, NaiveDate, NaiveDateTime, SecondsFormat, Utc};
use serde_json::Value;

use crate::error::{DriverError, Result};

/// `positionBindings`: `?` → `:b0`, `:b1`, …; an escaped `\?` becomes a
/// literal `?`.
pub fn position_bindings(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len() + 8);
    let mut count = 0usize;
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'?') => {
                chars.next();
                out.push('?');
            }
            '?' => {
                out.push_str(&format!(":b{count}"));
                count += 1;
            }
            other => out.push(other),
        }
    }
    out
}

/// Kind of a `:name` / `::name` token found in the SQL (`getSqlParams`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlParamKind {
    /// `:name`, a value placeholder (`n_ph`).
    Placeholder,
    /// `::name`, an identifier placeholder (`n_id`).
    Identifier,
}

fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// `getSqlParams`: every `:{1,2}\w+` token; a later token with the same label
/// overrides an earlier one. `::x` right after `:name` is a cast, not an
/// identifier placeholder.
pub fn sql_params(sql: &str) -> HashMap<String, SqlParamKind> {
    let bytes = sql.as_bytes();
    let mut result = HashMap::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b':' {
            i += 1;
            continue;
        }
        let double = i + 1 < bytes.len() && bytes[i + 1] == b':';
        let word_start = if double { i + 2 } else { i + 1 };
        let mut end = word_start;
        while end < bytes.len() && is_word(bytes[end]) {
            end += 1;
        }
        if end == word_start {
            // `::` not followed by a word: the regex retries from the next `:`.
            i += 1;
            continue;
        }
        let label = sql[word_start..end].to_string();
        if double {
            let before = &sql[i.saturating_sub(20)..i];
            if !ends_with_named_param(before) {
                result.insert(label, SqlParamKind::Identifier);
            }
        } else {
            result.insert(label, SqlParamKind::Placeholder);
        }
        i = end;
    }
    result
}

/// `/:\w+$/`.
fn ends_with_named_param(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut j = bytes.len();
    while j > 0 && is_word(bytes[j - 1]) {
        j -= 1;
    }
    j < bytes.len() && j > 0 && bytes[j - 1] == b':'
}

const SUPPORTED_TYPES: &[&str] = &[
    "arrayValue",
    "blobValue",
    "booleanValue",
    "doubleValue",
    "isNull",
    "longValue",
    "stringValue",
    "structValue",
];

/// `{ "<type>": value }` with a single supported key: passed through as is.
fn as_raw_field(value: &Value) -> Option<(&str, &Value)> {
    match value {
        Value::Object(map) if map.len() == 1 => {
            let (k, v) = map.iter().next()?;
            SUPPORTED_TYPES
                .contains(&k.as_str())
                .then_some((k.as_str(), v))
        }
        _ => None,
    }
}

fn raw_field(name: &str, kind: &str, value: &Value) -> Result<Field> {
    let invalid = || DriverError::Query(format!("'{name}' is an invalid type"));
    Ok(match kind {
        "stringValue" => Field::StringValue(value.as_str().ok_or_else(invalid)?.to_string()),
        "booleanValue" => Field::BooleanValue(value.as_bool().ok_or_else(invalid)?),
        "longValue" => Field::LongValue(value.as_i64().ok_or_else(invalid)?),
        "doubleValue" => Field::DoubleValue(value.as_f64().ok_or_else(invalid)?),
        "isNull" => Field::IsNull(value.as_bool().unwrap_or(true)),
        "blobValue" => Field::BlobValue(Blob::new(
            base64::engine::general_purpose::STANDARD
                .decode(value.as_str().ok_or_else(invalid)?)
                .map_err(|_| invalid())?,
        )),
        _ => return Err(invalid()),
    })
}

/// `formatParam` + `getType` / `getTypeHint`: a JSON parameter value to the
/// Data API's `Field`.
///
/// * strings → `stringValue`, booleans → `booleanValue`, `null` → `isNull`;
/// * numbers: integral values (as JavaScript sees them) → `longValue`,
///   others → `doubleValue`;
/// * `{ "<fieldType>": v }` → that field, verbatim;
/// * other objects → `stringValue` of their JSON with the `JSON` type hint;
/// * arrays → error `'<name>' is an invalid type`.
pub fn format_param(name: &str, value: &Value) -> Result<SqlParameter> {
    let (field, hint) = match value {
        Value::Null => (Field::IsNull(true), None),
        Value::Bool(b) => (Field::BooleanValue(*b), None),
        Value::String(s) => (Field::StringValue(s.clone()), None),
        Value::Number(n) => (number_field(n), None),
        Value::Array(_) => return Err(DriverError::Query(format!("'{name}' is an invalid type"))),
        Value::Object(_) => match as_raw_field(value) {
            Some((kind, v)) => (raw_field(name, kind, v)?, None),
            None => (Field::StringValue(value.to_string()), Some(TypeHint::Json)),
        },
    };
    Ok(SqlParameter::builder()
        .name(name)
        .value(field)
        .set_type_hint(hint)
        .build())
}

/// `parseInt(val.toString()) === val` decides between long and double.
fn number_field(n: &serde_json::Number) -> Field {
    if let Some(i) = n.as_i64() {
        return Field::LongValue(i);
    }
    let f = n.as_f64().unwrap_or(f64::NAN);
    // JS prints integral numbers below 1e21 without exponent, so they parse back.
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e21 && f.abs() <= i64::MAX as f64 {
        Field::LongValue(f as i64)
    } else {
        Field::DoubleValue(f)
    }
}

/// Replaces every `:name` followed by a word boundary (`/:name\b/g`).
fn replace_placeholder(sql: &str, name: &str, replacement: &str) -> String {
    let needle = format!(":{name}");
    let mut out = String::with_capacity(sql.len());
    let mut rest = sql;
    while let Some(pos) = rest.find(&needle) {
        let after = &rest[pos + needle.len()..];
        let boundary = after
            .as_bytes()
            .first()
            .map(|c| !is_word(*c))
            .unwrap_or(true);
        out.push_str(&rest[..pos]);
        if boundary {
            out.push_str(replacement);
        } else {
            out.push_str(&needle);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// The Node driver's `query`: positional `?` become `:bN`, the values become
/// `bN` named parameters. Parameters without a placeholder in the SQL are not
/// sent (`processParams` only keeps names found by `getSqlParams`).
pub fn build_statement(sql: &str, values: &[Value]) -> Result<(String, Vec<SqlParameter>)> {
    let mut sql = position_bindings(sql);
    let found = sql_params(&sql);
    let mut parameters = Vec::new();
    for (i, value) in values.iter().enumerate() {
        let name = format!("b{i}");
        match found.get(&name) {
            Some(SqlParamKind::Placeholder) => {
                if value.is_object() && as_raw_field(value).is_none() {
                    // engine `pg`: plain objects are cast to jsonb.
                    sql = replace_placeholder(&sql, &name, &format!(":{name}::jsonb"));
                }
                parameters.push(format_param(&name, value)?);
            }
            Some(SqlParamKind::Identifier) => {
                return Err(DriverError::NotImplemented(format!(
                    "Identifier placeholder ::{name} is not supported by the Aurora Serverless \
                     MySQL driver"
                )));
            }
            None => {}
        }
    }
    Ok((sql, parameters))
}

/// `formatFromTimeStamp(value, treatAsLocalDate)` rendered the way
/// `JSON.stringify(Date)` does (`toISOString`), `null` for an invalid date.
pub fn format_timestamp(value: &str) -> Value {
    fn iso(dt: DateTime<Utc>) -> Value {
        Value::String(dt.to_rfc3339_opts(SecondsFormat::Millis, true))
    }
    // `/^\d{4}-\d{2}-\d{2}(\s\d{2}:\d{2}:\d{2}(\.\d+)?)?$/` → parsed as UTC.
    if let Ok(d) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        if value.len() == 10 {
            return iso(d.and_hms_opt(0, 0, 0).unwrap_or_default().and_utc());
        }
    }
    let bytes = value.as_bytes();
    if bytes.len() >= 19 && bytes[10].is_ascii_whitespace() {
        let normalized = format!("{}T{}", &value[..10], &value[11..]);
        if let Ok(dt) = NaiveDateTime::parse_from_str(&normalized, "%Y-%m-%dT%H:%M:%S%.f") {
            // JS Dates have millisecond precision.
            let ms = dt.and_utc().timestamp_millis();
            if let Some(dt) = DateTime::<Utc>::from_timestamp_millis(ms) {
                return iso(dt);
            }
        }
    }
    // `new Date(value)`: ISO 8601 with an offset still parses.
    match DateTime::parse_from_rfc3339(value) {
        Ok(dt) => iso(dt.with_timezone(&Utc)),
        Err(_) => Value::Null,
    }
}

fn array_to_json(array: &ArrayValue) -> Value {
    fn opt<T: Into<Value> + Clone>(v: &[Option<T>]) -> Value {
        Value::Array(
            v.iter()
                .map(|x| x.clone().map(Into::into).unwrap_or(Value::Null))
                .collect(),
        )
    }
    match array {
        ArrayValue::StringValues(v) => opt(v),
        ArrayValue::LongValues(v) => opt(v),
        ArrayValue::DoubleValues(v) => opt(v),
        ArrayValue::BooleanValues(v) => opt(v),
        ArrayValue::ArrayValues(v) => Value::Array(
            v.iter()
                .map(|x| {
                    x.as_ref()
                        .map(array_to_json)
                        .unwrap_or(Value::Array(vec![]))
                })
                .collect(),
        ),
        _ => Value::Array(vec![]),
    }
}

/// `formatRecordValue`: one result cell. Dates are deserialised
/// (`deserializeDate` defaults to `true`) and rendered as ISO strings, JSON
/// columns are parsed, `YEAR` becomes a number, blobs become base64 (the
/// convention of the MySQL driver).
pub fn format_record_value(
    field: &Field,
    type_name: Option<&str>,
    blob_as_text: bool,
) -> Result<Value> {
    let raw = match field {
        Field::IsNull(true) => return Ok(Value::Null),
        Field::IsNull(false) => Value::Null,
        Field::StringValue(s) => Value::String(s.clone()),
        Field::LongValue(v) => Value::from(*v),
        Field::DoubleValue(v) => Value::from(*v),
        Field::BooleanValue(v) => Value::Bool(*v),
        Field::BlobValue(b) if blob_as_text => {
            return Ok(Value::String(
                String::from_utf8_lossy(b.as_ref()).to_string(),
            ))
        }
        Field::BlobValue(b) => {
            return Ok(Value::String(
                base64::engine::general_purpose::STANDARD.encode(b.as_ref()),
            ))
        }
        Field::ArrayValue(a) => array_to_json(a),
        _ => Value::Null,
    };
    let Some(type_name) = type_name else {
        return Ok(raw);
    };
    let upper = type_name.to_uppercase();
    if [
        "DATE",
        "DATETIME",
        "TIMESTAMP",
        "TIMESTAMPTZ",
        "TIMESTAMP WITH TIME ZONE",
    ]
    .contains(&upper.as_str())
    {
        return Ok(match &raw {
            Value::String(s) => format_timestamp(s),
            other => other.clone(),
        });
    }
    if upper == "JSON" || upper == "JSONB" {
        return match &raw {
            Value::String(s) => serde_json::from_str(s).map_err(|e| {
                DriverError::TypeDetection(format!("Unable to parse {type_name} value: {e}"))
            }),
            other => Ok(other.clone()),
        };
    }
    if type_name == "YEAR" {
        if let Value::String(s) = &raw {
            let digits: String = s
                .trim_start()
                .chars()
                .enumerate()
                .take_while(|(i, c)| c.is_ascii_digit() || (*i == 0 && (*c == '-' || *c == '+')))
                .map(|(_, c)| c)
                .collect();
            return Ok(digits
                .parse::<i64>()
                .map(Value::from)
                .unwrap_or(Value::Null));
        }
    }
    Ok(raw)
}

/// Retry policy of `data-api-client` (`retry.ts`).
#[derive(Debug, Clone)]
pub struct RetryOptions {
    pub enabled: bool,
    pub max_retries: usize,
    /// Extra error codes retried with the "resuming" schedule.
    pub retryable_errors: Vec<String>,
    /// Multiplier applied to every delay (1.0 in production; tests shrink it).
    pub delay_scale: f64,
}

impl Default for RetryOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            max_retries: 9,
            retryable_errors: Vec::new(),
            delay_scale: 1.0,
        }
    }
}

/// Back-off while an auto-paused cluster resumes.
pub const RESUMING_RETRY_DELAYS_MS: &[u64] = &[
    0, 2000, 5000, 10000, 15000, 20000, 25000, 30000, 35000, 40000,
];
/// Back-off after a connection-level failure.
pub const CONNECTION_RETRY_DELAYS_MS: &[u64] = &[0, 2000, 4000];
const CONNECTION_ERROR_PATTERNS: &[&str] = &[
    "Communications link failure",
    "Connection is not available",
    "currently unavailable",
    "Database cluster is not available",
    "Can't connect to",
    "Connection timed out",
];

/// `isDatabaseResuming`.
pub fn is_database_resuming(code: &str, message: &str) -> bool {
    code == "DatabaseResumingException" || message.contains("is resuming after being auto-paused")
}

/// `isConnectionError`. Note that `BadRequestException` — which is also what
/// the Data API returns for SQL errors — is retried, as in the Node client.
pub fn is_connection_error(code: &str, message: &str) -> bool {
    code == "BadRequestException"
        || code == "StatementTimeoutException"
        || CONNECTION_ERROR_PATTERNS
            .iter()
            .any(|p| message.contains(p))
}

/// Pause before attempt `attempt` (1-based retries), `None` when the error
/// must not be retried any more (`withRetry`).
pub fn retry_delay(
    options: &RetryOptions,
    attempt: usize,
    code: &str,
    message: &str,
) -> Option<Duration> {
    if !options.enabled {
        return None;
    }
    let (delays, max_attempts) = if is_database_resuming(code, message) {
        (
            RESUMING_RETRY_DELAYS_MS,
            options.max_retries.min(RESUMING_RETRY_DELAYS_MS.len() - 1),
        )
    } else if is_connection_error(code, message) {
        (
            CONNECTION_RETRY_DELAYS_MS,
            CONNECTION_RETRY_DELAYS_MS.len() - 1,
        )
    } else if options.retryable_errors.iter().any(|e| e == code) {
        (
            RESUMING_RETRY_DELAYS_MS,
            options.max_retries.min(RESUMING_RETRY_DELAYS_MS.len() - 1),
        )
    } else {
        return None;
    };
    // `attempt` is the number of failures so far; retry while below the max.
    if attempt >= max_attempts {
        return None;
    }
    let next = attempt + 1;
    let ms = delays.get(next).or(delays.last()).copied().unwrap_or(0);
    Some(Duration::from_millis(
        (ms as f64 * options.delay_scale) as u64,
    ))
}
