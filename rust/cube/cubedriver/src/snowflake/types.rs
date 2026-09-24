//! Snowflake type mapping and value hydration.
//!
//! The Node driver relies on `snowflake-sdk` (`fetchAsString` for numbers and
//! semi-structured values, plus the hydrators in `type-parsers.ts`). The SQL
//! REST API already hands every value over as a string, so hydration here only
//! has to turn Snowflake's epoch-based temporal encodings into the same
//! `YYYY-MM-DDTHH:MM:SS.mmm` strings `formatUtcTimestamp` produces.

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use serde_json::Value;

use crate::types::GenericType;

/// Port of `SnowflakeToGenericType`.
pub fn snowflake_to_generic(db_type_lower: &str) -> Option<GenericType> {
    Some(match db_type_lower {
        // A limitation of the Node driver kept verbatim: objects are not
        // usable in Cube Store anyway.
        "object" => GenericType::Other("HLL_SNOWFLAKE".to_string()),
        "number" => GenericType::Decimal(None),
        // Snowflake reports DWH types like NUMBER(38, 15) as `fixed`.
        "fixed" => GenericType::Decimal(None),
        "timestamp_ntz" | "timestamp_ltz" | "timestamp_tz" => GenericType::Timestamp,
        _ => return None,
    })
}

/// Port of `SnowflakeDriver.toGenericType`.
///
/// Note that the Node override does *not* forward precision/scale to
/// `super.toGenericType`; it applies the precise-decimal rule itself.
pub fn to_generic_type(
    db_type: &str,
    precision: Option<i64>,
    scale: Option<i64>,
    precise_decimal: bool,
) -> GenericType {
    let generic = snowflake_to_generic(&db_type.to_lowercase())
        .unwrap_or_else(|| crate::types::to_generic_type(db_type, None, None, false));

    if let (GenericType::Decimal(None), Some(p), Some(s)) = (&generic, precision, scale) {
        if p > 0 && s > 0 && precise_decimal {
            return GenericType::Decimal(Some((p as u32, s as u32)));
        }
    }
    generic
}

/// Port of `getTypes`: numeric columns with scale 0 become `int`.
pub fn column_type(
    db_type: &str,
    precision: Option<i64>,
    scale: Option<i64>,
    precise_decimal: bool,
) -> GenericType {
    let is_number = matches!(db_type.to_lowercase().as_str(), "fixed" | "real" | "number");
    if is_number && scale == Some(0) {
        return GenericType::Int;
    }
    if is_number {
        return to_generic_type(db_type, precision, scale, precise_decimal);
    }
    to_generic_type(db_type, None, None, precise_decimal)
}

/// Port of `formatUtcTimestamp`.
pub fn format_utc_timestamp(value: DateTime<Utc>) -> String {
    value.format("%Y-%m-%dT%H:%M:%S%.3f").to_string()
}

/// Formats Snowflake's epoch encoding (`"1577881496.789000000"`, optionally
/// followed by a timezone offset in minutes) as a UTC timestamp string.
pub fn format_epoch_timestamp(raw: &str) -> Option<String> {
    // TIMESTAMP_TZ arrives as "<epoch> <offset minutes>"; the Node hydrator
    // formats the UTC instant, so the offset is not needed.
    let epoch = raw.split_whitespace().next()?;
    let (seconds, nanos) = match epoch.split_once('.') {
        Some((s, frac)) => {
            let mut frac = frac.to_string();
            frac.truncate(9);
            while frac.len() < 9 {
                frac.push('0');
            }
            (s.parse::<i64>().ok()?, frac.parse::<u32>().ok()?)
        }
        None => (epoch.parse::<i64>().ok()?, 0),
    };
    // A negative epoch borrows a second from the fraction.
    let (seconds, nanos) = if seconds < 0 && nanos > 0 {
        (seconds - 1, 1_000_000_000 - nanos)
    } else {
        (seconds, nanos)
    };
    match Utc.timestamp_opt(seconds, nanos) {
        chrono::LocalResult::Single(dt) => Some(format_utc_timestamp(dt)),
        _ => None,
    }
}

/// Formats Snowflake's DATE encoding (days since the epoch).
pub fn format_epoch_date(raw: &str) -> Option<String> {
    let days = raw.trim().parse::<i64>().ok()?;
    let date = NaiveDate::from_ymd_opt(1970, 1, 1)? + Duration::try_days(days)?;
    Some(format_utc_timestamp(date.and_hms_opt(0, 0, 0)?.and_utc()))
}

/// Formats Snowflake's TIME encoding (seconds since midnight).
pub fn format_epoch_time(raw: &str) -> Option<String> {
    let (seconds, nanos) = match raw.split_once('.') {
        Some((s, frac)) => {
            let mut frac = frac.to_string();
            frac.truncate(9);
            while frac.len() < 9 {
                frac.push('0');
            }
            (s.parse::<i64>().ok()?, frac.parse::<u32>().ok()?)
        }
        None => (raw.trim().parse::<i64>().ok()?, 0),
    };
    let millis = nanos / 1_000_000;
    Some(format!(
        "{:02}:{:02}:{:02}.{:03}",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60,
        millis
    ))
}

/// Hydrates one raw REST value for a column of type `db_type`.
pub fn hydrate(value: &Value, db_type: &str) -> Value {
    let Value::String(raw) = value else {
        return value.clone();
    };
    match db_type.to_lowercase().as_str() {
        "boolean" => Value::Bool(raw.eq_ignore_ascii_case("true") || raw == "1"),
        "date" => format_epoch_date(raw)
            .map(Value::String)
            .unwrap_or(value.clone()),
        "time" => format_epoch_time(raw)
            .map(Value::String)
            .unwrap_or(value.clone()),
        "timestamp_ntz" | "timestamp_ltz" | "timestamp_tz" => format_epoch_timestamp(raw)
            .map(Value::String)
            .unwrap_or(value.clone()),
        // `fixed`, `real`, `text`, `variant`, `object`, `array`, `binary`:
        // the Node driver keeps them as strings too (`fetchAsString`).
        _ => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_types() {
        assert_eq!(
            to_generic_type("FIXED", None, None, false),
            GenericType::Decimal(None)
        );
        assert_eq!(
            to_generic_type("NUMBER", Some(38), Some(15), true),
            GenericType::Decimal(Some((38, 15)))
        );
        assert_eq!(
            to_generic_type("NUMBER", Some(38), Some(15), false),
            GenericType::Decimal(None)
        );
        assert_eq!(
            to_generic_type("TIMESTAMP_NTZ", None, None, false),
            GenericType::Timestamp
        );
        assert_eq!(
            to_generic_type("OBJECT", None, None, false),
            GenericType::Other("HLL_SNOWFLAKE".into())
        );
        // TEXT is lower-cased into the base table, which maps `text`.
        assert_eq!(
            to_generic_type("TEXT", None, None, false),
            GenericType::Text
        );
        // `real` has no mapping at all, so it keeps its spelling (as in JS).
        assert_eq!(
            to_generic_type("real", None, None, false),
            GenericType::Other("real".into())
        );
        assert_eq!(
            to_generic_type("boolean", None, None, false),
            GenericType::Boolean
        );
    }

    #[test]
    fn column_types_follow_get_types() {
        // scale 0 numbers become `int`
        assert_eq!(
            column_type("fixed", Some(38), Some(0), false),
            GenericType::Int
        );
        assert_eq!(
            column_type("fixed", Some(38), Some(2), true),
            GenericType::Decimal(Some((38, 2)))
        );
        // `real` is a number for `getTypes`, but has no generic mapping.
        assert_eq!(
            column_type("real", Some(38), Some(2), false),
            GenericType::Other("real".into())
        );
        assert_eq!(
            column_type("real", Some(38), Some(0), false),
            GenericType::Int
        );
        assert_eq!(column_type("date", None, None, false), GenericType::Date);
    }

    #[test]
    fn temporal_hydration() {
        assert_eq!(
            format_epoch_timestamp("1577881496.789000000").as_deref(),
            Some("2020-01-01T12:24:56.789")
        );
        assert_eq!(
            format_epoch_timestamp("1577836800").as_deref(),
            Some("2020-01-01T00:00:00.000")
        );
        // TIMESTAMP_TZ carries the offset in a second field
        assert_eq!(
            format_epoch_timestamp("1577836800.000000000 1440").as_deref(),
            Some("2020-01-01T00:00:00.000")
        );
        assert_eq!(
            format_epoch_date("18262").as_deref(),
            Some("2020-01-01T00:00:00.000")
        );
        assert_eq!(
            format_epoch_date("0").as_deref(),
            Some("1970-01-01T00:00:00.000")
        );
        assert_eq!(
            format_epoch_time("45296.500000000").as_deref(),
            Some("12:34:56.500")
        );
    }

    #[test]
    fn values_are_hydrated_by_type() {
        assert_eq!(hydrate(&Value::from("true"), "boolean"), Value::Bool(true));
        assert_eq!(
            hydrate(&Value::from("false"), "BOOLEAN"),
            Value::Bool(false)
        );
        assert_eq!(
            hydrate(&Value::from("1577836800.000000000"), "timestamp_ntz"),
            Value::from("2020-01-01T00:00:00.000")
        );
        assert_eq!(
            hydrate(&Value::from("18262"), "date"),
            Value::from("2020-01-01T00:00:00.000")
        );
        // numbers stay strings, exactly like `fetchAsString`
        assert_eq!(
            hydrate(&Value::from("9007199254740993"), "fixed"),
            Value::from("9007199254740993")
        );
        assert_eq!(hydrate(&Value::Null, "fixed"), Value::Null);
        // unparseable values are passed through
        assert_eq!(hydrate(&Value::from("x"), "date"), Value::from("x"));
    }
}
