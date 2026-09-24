//! Decoding of PostgreSQL binary result values into `serde_json::Value`.
//!
//! The Node.js driver receives results in the *text* protocol and post-processes
//! a handful of types (`pg-types` parsers plus Cube's own date/timestamp
//! parsers). `tokio-postgres` always requests the *binary* format, so this
//! module decodes the binary representation and renders each value exactly the
//! way the Node.js driver would have returned it:
//!
//! | PostgreSQL type            | JSON value                                    |
//! |----------------------------|-----------------------------------------------|
//! | `bool`                     | boolean                                       |
//! | `int2`, `int4`, `oid`      | number                                        |
//! | `int8`                     | string (as `pg-types` does)                   |
//! | `float4`, `float8`         | number (`NaN`/`±Infinity` become `null`)      |
//! | `numeric`                  | string keeping the scale (`"1.00"`)           |
//! | `date`                     | `"YYYY-MM-DDT00:00:00.000"`                   |
//! | `timestamp`, `timestamptz` | `"YYYY-MM-DDTHH:MM:SS.mmm"` (tz shifted to UTC)|
//! | `json`, `jsonb`            | parsed JSON                                   |
//! | text-like, enums, uuid     | string                                        |
//! | arrays                     | JSON array of decoded elements                |
//! | `hll`                      | base64 of the sketch bytes                    |
//! | `bytea`                    | `"\x…"` hex string (PostgreSQL text form)     |
//! | composite                  | PostgreSQL record text form `"(a,b)"`         |

use std::error::Error;
use std::net::IpAddr;

use base64::Engine;
use chrono::{Duration as ChronoDuration, NaiveDate, NaiveDateTime, Timelike};
use fallible_iterator::FallibleIterator;
use postgres_protocol::types as pt;
use serde_json::Value;
use tokio_postgres::types::{FromSql, Kind, Type};

use super::numeric::numeric_to_string;

type BoxError = Box<dyn Error + Sync + Send>;

/// A result cell decoded to JSON. Accepts every PostgreSQL type.
#[derive(Debug, Clone, PartialEq)]
pub struct JsonCell(pub Value);

impl<'a> FromSql<'a> for JsonCell {
    fn from_sql(ty: &Type, raw: &'a [u8]) -> Result<Self, BoxError> {
        decode_value(ty, raw).map(JsonCell)
    }

    fn from_sql_null(_ty: &Type) -> Result<Self, BoxError> {
        Ok(JsonCell(Value::Null))
    }

    fn accepts(_ty: &Type) -> bool {
        true
    }
}

/// Decodes a non-null binary value of type `ty`.
pub fn decode_value(ty: &Type, raw: &[u8]) -> Result<Value, BoxError> {
    match ty.kind() {
        Kind::Array(elem) => decode_array(elem, raw),
        Kind::Enum(_) => text(raw),
        Kind::Domain(inner) => decode_value(inner, raw),
        Kind::Composite(fields) => {
            let field_types: Vec<Type> = fields.iter().map(|f| f.type_().clone()).collect();
            decode_composite(&field_types, raw).map(Value::String)
        }
        _ => decode_simple(ty, raw),
    }
}

fn text(raw: &[u8]) -> Result<Value, BoxError> {
    Ok(Value::String(pt::text_from_sql(raw)?.to_string()))
}

fn float(v: f64) -> Value {
    serde_json::Number::from_f64(v)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

fn decode_simple(ty: &Type, raw: &[u8]) -> Result<Value, BoxError> {
    Ok(match *ty {
        Type::BOOL => Value::Bool(pt::bool_from_sql(raw)?),
        Type::INT2 => Value::from(pt::int2_from_sql(raw)?),
        Type::INT4 => Value::from(pt::int4_from_sql(raw)?),
        Type::OID => Value::from(pt::oid_from_sql(raw)?),
        Type::INT8 => Value::String(pt::int8_from_sql(raw)?.to_string()),
        Type::FLOAT4 => float(pt::float4_from_sql(raw)? as f64),
        Type::FLOAT8 => float(pt::float8_from_sql(raw)?),
        Type::NUMERIC => Value::String(numeric_to_string(raw)?),
        Type::TEXT
        | Type::VARCHAR
        | Type::BPCHAR
        | Type::NAME
        | Type::UNKNOWN
        | Type::XML
        | Type::CHAR
        | Type::CSTRING => return text(raw),
        Type::JSON => serde_json::from_slice(raw)?,
        Type::JSONB => {
            let Some((&version, body)) = raw.split_first() else {
                return Err("invalid jsonb: empty value".into());
            };
            if version != 1 {
                return Err(format!("unsupported jsonb version {version}").into());
            }
            serde_json::from_slice(body)?
        }
        Type::DATE => Value::String(format_date(pt::date_from_sql(raw)?)),
        Type::TIMESTAMP | Type::TIMESTAMPTZ => {
            Value::String(format_timestamp(pt::timestamp_from_sql(raw)?))
        }
        Type::TIME => Value::String(format_time(pt::time_from_sql(raw)?)),
        Type::TIMETZ => Value::String(format_timetz(raw)?),
        Type::INTERVAL => Value::String(format_interval(raw)?),
        Type::UUID => Value::String(format_uuid(pt::uuid_from_sql(raw)?)),
        Type::BYTEA => Value::String(format!("\\x{}", hex(pt::bytea_from_sql(raw)))),
        Type::MONEY => Value::String(format_money(pt::int8_from_sql(raw)?)),
        Type::INET | Type::CIDR => {
            let inet = pt::inet_from_sql(raw)?;
            Value::String(format_inet(inet.addr(), inet.netmask(), *ty == Type::CIDR))
        }
        Type::MACADDR => {
            let mac = pt::macaddr_from_sql(raw)?;
            Value::String(
                mac.iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(":"),
            )
        }
        Type::MACADDR8 => Value::String(
            raw.iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(":"),
        ),
        Type::BIT | Type::VARBIT => {
            let bits = pt::varbit_from_sql(raw)?;
            let mut s = String::with_capacity(bits.len());
            for i in 0..bits.len() {
                let byte = bits.bytes()[i / 8];
                s.push(if byte & (0x80 >> (i % 8)) != 0 {
                    '1'
                } else {
                    '0'
                });
            }
            Value::String(s)
        }
        _ => {
            if ty.name() == "hll" {
                // Cube uses base64 as the exchange format of HLL sketches.
                Value::String(base64::engine::general_purpose::STANDARD.encode(raw))
            } else if let Ok(s) = std::str::from_utf8(raw) {
                if s.chars().all(|c| !c.is_control() || c.is_whitespace()) {
                    Value::String(s.to_string())
                } else {
                    Value::String(format!("\\x{}", hex(raw)))
                }
            } else {
                Value::String(format!("\\x{}", hex(raw)))
            }
        }
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn decode_array(elem: &Type, raw: &[u8]) -> Result<Value, BoxError> {
    let array = pt::array_from_sql(raw)?;
    let dims: Vec<usize> = array
        .dimensions()
        .map(|d| Ok(d.len.max(0) as usize))
        .collect()?;
    let values: Vec<Value> = array
        .values()
        .map(|v| match v {
            None => Ok(Value::Null),
            Some(bytes) => decode_value(elem, bytes),
        })
        .collect()?;

    if dims.len() <= 1 {
        return Ok(Value::Array(values));
    }

    fn nest(values: &[Value], dims: &[usize]) -> Value {
        if dims.len() == 1 {
            return Value::Array(values.to_vec());
        }
        let chunk = dims[1..].iter().product::<usize>().max(1);
        Value::Array(values.chunks(chunk).map(|c| nest(c, &dims[1..])).collect())
    }
    Ok(nest(&values, &dims))
}

/// Renders a composite value in PostgreSQL's record text form: `(1,"a b",)`.
fn decode_composite(field_types: &[Type], raw: &[u8]) -> Result<String, BoxError> {
    let mut buf = raw;
    let nfields = read_i32(&mut buf)?;
    let mut parts = Vec::with_capacity(nfields.max(0) as usize);
    for i in 0..nfields.max(0) as usize {
        let _oid = read_i32(&mut buf)?;
        let len = read_i32(&mut buf)?;
        if len < 0 {
            parts.push(String::new());
            continue;
        }
        let len = len as usize;
        if buf.len() < len {
            return Err("invalid composite: field truncated".into());
        }
        let (bytes, rest) = buf.split_at(len);
        buf = rest;
        let value = match field_types.get(i) {
            Some(ty) => decode_value(ty, bytes)?,
            None => text(bytes)?,
        };
        let s = crate::types::value_to_string(&value).unwrap_or_default();
        let needs_quotes = s.is_empty()
            || s.chars()
                .any(|c| matches!(c, '(' | ')' | ',' | '"' | '\\') || c.is_whitespace());
        if needs_quotes {
            parts.push(format!(
                "\"{}\"",
                s.replace('\\', "\\\\").replace('"', "\"\"")
            ));
        } else {
            parts.push(s);
        }
    }
    Ok(format!("({})", parts.join(",")))
}

fn read_i32(buf: &mut &[u8]) -> Result<i32, BoxError> {
    if buf.len() < 4 {
        return Err("unexpected end of buffer".into());
    }
    let v = i32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
    *buf = &buf[4..];
    Ok(v)
}

fn read_i64(buf: &mut &[u8]) -> Result<i64, BoxError> {
    if buf.len() < 8 {
        return Err("unexpected end of buffer".into());
    }
    let mut b = [0u8; 8];
    b.copy_from_slice(&buf[..8]);
    *buf = &buf[8..];
    Ok(i64::from_be_bytes(b))
}

fn pg_epoch() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2000, 1, 1)
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .expect("valid epoch")
}

/// Port of `dateTypeParser`: `YYYY-MM-DD` → `YYYY-MM-DDT00:00:00.000`.
pub fn format_date(days: i32) -> String {
    match days {
        i32::MAX => return "infinity".to_string(),
        i32::MIN => return "-infinity".to_string(),
        _ => {}
    }
    match pg_epoch()
        .date()
        .checked_add_signed(ChronoDuration::days(days as i64))
    {
        Some(d) => format!("{}T00:00:00.000", format_ymd(&d)),
        None => format!("{days} days"),
    }
}

/// Port of `timestampTypeParser` / `timestampTzTypeParser`: microseconds since
/// 2000-01-01 (UTC) → `YYYY-MM-DDTHH:MM:SS.mmm`, fractional part truncated.
pub fn format_timestamp(micros: i64) -> String {
    match micros {
        i64::MAX => return "infinity".to_string(),
        i64::MIN => return "-infinity".to_string(),
        _ => {}
    }
    match pg_epoch().checked_add_signed(ChronoDuration::microseconds(micros)) {
        Some(dt) => format!(
            "{}T{:02}:{:02}:{:02}.{:03}",
            format_ymd(&dt.date()),
            dt.hour(),
            dt.minute(),
            dt.second(),
            dt.nanosecond() / 1_000_000
        ),
        None => format!("{micros} us"),
    }
}

fn format_ymd(d: &NaiveDate) -> String {
    use chrono::Datelike;
    format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
}

/// `HH:MM:SS[.ffffff]` with trailing fractional zeros trimmed (PostgreSQL text form).
pub fn format_time(micros: i64) -> String {
    let total_secs = micros.div_euclid(1_000_000);
    let frac = micros.rem_euclid(1_000_000);
    let h = total_secs / 3600;
    let m = (total_secs % 3600) / 60;
    let s = total_secs % 60;
    let mut out = format!("{h:02}:{m:02}:{s:02}");
    if frac != 0 {
        let f = format!("{frac:06}");
        out.push('.');
        out.push_str(f.trim_end_matches('0'));
    }
    out
}

fn format_timetz(raw: &[u8]) -> Result<String, BoxError> {
    let mut buf = raw;
    let micros = read_i64(&mut buf)?;
    // seconds west of UTC (positive = west)
    let zone = read_i32(&mut buf)?;
    let sign = if zone > 0 { '-' } else { '+' };
    let abs = zone.unsigned_abs();
    let (zh, zm, zs) = (abs / 3600, (abs % 3600) / 60, abs % 60);
    let mut out = format!("{}{}{:02}", format_time(micros), sign, zh);
    if zm != 0 || zs != 0 {
        out.push_str(&format!(":{zm:02}"));
        if zs != 0 {
            out.push_str(&format!(":{zs:02}"));
        }
    }
    Ok(out)
}

/// PostgreSQL `IntervalStyle = postgres` rendering: `1 year 2 mons 3 days 04:05:06.789`.
fn format_interval(raw: &[u8]) -> Result<String, BoxError> {
    let mut buf = raw;
    let micros = read_i64(&mut buf)?;
    let days = read_i32(&mut buf)?;
    let months = read_i32(&mut buf)?;

    let years = months / 12;
    let mons = months % 12;
    let mut parts: Vec<String> = Vec::new();
    let plural = |n: i32, unit: &str| {
        if n.abs() == 1 {
            format!("{n} {unit}")
        } else {
            format!("{n} {unit}s")
        }
    };
    if years != 0 {
        parts.push(plural(years, "year"));
    }
    if mons != 0 {
        parts.push(plural(mons, "mon"));
    }
    if days != 0 {
        parts.push(plural(days, "day"));
    }
    if micros != 0 || parts.is_empty() {
        let sign = if micros < 0 { "-" } else { "" };
        parts.push(format!("{sign}{}", format_time(micros.abs())));
    }
    Ok(parts.join(" "))
}

fn format_uuid(bytes: [u8; 16]) -> String {
    let h = hex(&bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// `money` in the `C` locale (`$1,234.56`).
fn format_money(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let abs = cents.unsigned_abs();
    let dollars = (abs / 100).to_string();
    let mut grouped = String::new();
    for (i, c) in dollars.chars().enumerate() {
        if i > 0 && (dollars.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("{sign}${grouped}.{:02}", abs % 100)
}

fn format_inet(addr: IpAddr, netmask: u8, always_mask: bool) -> String {
    let full = match addr {
        IpAddr::V4(_) => 32,
        IpAddr::V6(_) => 128,
    };
    if always_mask || netmask != full {
        format!("{addr}/{netmask}")
    } else {
        addr.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;
    use tokio_postgres::types::ToSql;

    fn encode<T: ToSql>(v: T, ty: &Type) -> Vec<u8> {
        let mut buf = BytesMut::new();
        v.to_sql(ty, &mut buf).unwrap();
        buf.to_vec()
    }

    #[test]
    fn scalars() {
        assert_eq!(
            decode_value(&Type::BOOL, &encode(true, &Type::BOOL)).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            decode_value(&Type::INT4, &encode(7i32, &Type::INT4)).unwrap(),
            Value::from(7)
        );
        assert_eq!(
            decode_value(&Type::INT8, &encode(9_007_199_254_740_993i64, &Type::INT8)).unwrap(),
            Value::from("9007199254740993")
        );
        assert_eq!(
            decode_value(&Type::FLOAT8, &encode(1.5f64, &Type::FLOAT8)).unwrap(),
            Value::from(1.5)
        );
        assert_eq!(
            decode_value(&Type::FLOAT8, &encode(f64::NAN, &Type::FLOAT8)).unwrap(),
            Value::Null
        );
        assert_eq!(
            decode_value(&Type::TEXT, b"hello").unwrap(),
            Value::from("hello")
        );
        assert_eq!(
            decode_value(&Type::JSON, br#"{"a":[1,2]}"#).unwrap(),
            serde_json::json!({"a": [1, 2]})
        );
        let mut jsonb = vec![1u8];
        jsonb.extend_from_slice(br#"{"b":true}"#);
        assert_eq!(
            decode_value(&Type::JSONB, &jsonb).unwrap(),
            serde_json::json!({"b": true})
        );
        assert_eq!(
            decode_value(&Type::BYTEA, b"\x01\xff").unwrap(),
            Value::from("\\x01ff")
        );
        assert_eq!(
            decode_value(
                &Type::UUID,
                &[
                    0xa0, 0xee, 0xbc, 0x99, 0x9c, 0x0b, 0x4e, 0xf8, 0xbb, 0x6d, 0x6b, 0xb9, 0xbd,
                    0x38, 0x0a, 0x11
                ]
            )
            .unwrap(),
            Value::from("a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11")
        );
    }

    #[test]
    fn dates_and_timestamps() {
        // 2020-01-01 is 7305 days after 2000-01-01
        assert_eq!(format_date(7305), "2020-01-01T00:00:00.000");
        assert_eq!(format_date(0), "2000-01-01T00:00:00.000");
        assert_eq!(format_date(-1), "1999-12-31T00:00:00.000");
        assert_eq!(format_date(i32::MAX), "infinity");

        let day = 86_400_000_000i64;
        assert_eq!(format_timestamp(7305 * day), "2020-01-01T00:00:00.000");
        // 2020-01-01 12:34:56.123456 -> ms truncated
        assert_eq!(
            format_timestamp(7305 * day + 45_296_123_456),
            "2020-01-01T12:34:56.123"
        );
        assert_eq!(
            format_timestamp(7305 * day + 45_296_500_000),
            "2020-01-01T12:34:56.500"
        );
        // 2019-12-31 22:00:00 (timestamptz '2020-01-01 00:00:00+02')
        assert_eq!(
            format_timestamp(7305 * day - 2 * 3_600_000_000),
            "2019-12-31T22:00:00.000"
        );
        // year 500 keeps its leading zero (pad4)
        let d500 = NaiveDate::from_ymd_opt(500, 6, 15).unwrap();
        let days = (d500 - NaiveDate::from_ymd_opt(2000, 1, 1).unwrap()).num_days();
        assert_eq!(
            format_timestamp(days * day + 12 * 3_600_000_000),
            "0500-06-15T12:00:00.000"
        );
        assert_eq!(format_timestamp(i64::MIN), "-infinity");
    }

    #[test]
    fn time_and_interval() {
        assert_eq!(format_time(45_296_000_000), "12:34:56");
        assert_eq!(format_time(45_296_500_000), "12:34:56.5");
        assert_eq!(format_time(45_296_123_456), "12:34:56.123456");

        let mut raw = Vec::new();
        raw.extend_from_slice(&45_296_000_000i64.to_be_bytes());
        raw.extend_from_slice(&(-7200i32).to_be_bytes());
        assert_eq!(format_timetz(&raw).unwrap(), "12:34:56+02");

        let interval = |micros: i64, days: i32, months: i32| {
            let mut raw = Vec::new();
            raw.extend_from_slice(&micros.to_be_bytes());
            raw.extend_from_slice(&days.to_be_bytes());
            raw.extend_from_slice(&months.to_be_bytes());
            format_interval(&raw).unwrap()
        };
        assert_eq!(interval(0, 0, 0), "00:00:00");
        assert_eq!(
            interval(3_600_000_000, 1, 14),
            "1 year 2 mons 1 day 01:00:00"
        );
        assert_eq!(interval(-3_600_000_000, 2, 0), "2 days -01:00:00");
        assert_eq!(interval(500_000, 0, 24), "2 years 00:00:00.5");
    }

    #[test]
    fn arrays() {
        let raw = encode(vec![1i32, 2, 3], &Type::INT4_ARRAY);
        assert_eq!(
            decode_value(&Type::INT4_ARRAY, &raw).unwrap(),
            serde_json::json!([1, 2, 3])
        );
        let raw = encode(vec![Some("a"), None], &Type::TEXT_ARRAY);
        assert_eq!(
            decode_value(&Type::TEXT_ARRAY, &raw).unwrap(),
            serde_json::json!(["a", null])
        );
        let raw = encode(Vec::<i32>::new(), &Type::INT4_ARRAY);
        assert_eq!(
            decode_value(&Type::INT4_ARRAY, &raw).unwrap(),
            serde_json::json!([])
        );
    }

    #[test]
    fn composite_text_form() {
        // (1, 'a b', NULL) with field types int4, text, text
        let mut raw = Vec::new();
        raw.extend_from_slice(&3i32.to_be_bytes());
        raw.extend_from_slice(&23i32.to_be_bytes());
        raw.extend_from_slice(&4i32.to_be_bytes());
        raw.extend_from_slice(&1i32.to_be_bytes());
        raw.extend_from_slice(&25i32.to_be_bytes());
        raw.extend_from_slice(&3i32.to_be_bytes());
        raw.extend_from_slice(b"a b");
        raw.extend_from_slice(&25i32.to_be_bytes());
        raw.extend_from_slice(&(-1i32).to_be_bytes());
        assert_eq!(
            decode_composite(&[Type::INT4, Type::TEXT, Type::TEXT], &raw).unwrap(),
            "(1,\"a b\",)"
        );
    }

    #[test]
    fn misc_formats() {
        assert_eq!(format_money(123_456), "$1,234.56");
        assert_eq!(format_money(-5), "-$0.05");
        assert_eq!(
            format_inet("10.0.0.1".parse().unwrap(), 32, false),
            "10.0.0.1"
        );
        assert_eq!(
            format_inet("10.0.0.0".parse().unwrap(), 8, false),
            "10.0.0.0/8"
        );
        assert_eq!(
            format_inet("10.0.0.1".parse().unwrap(), 32, true),
            "10.0.0.1/32"
        );
        assert_eq!(
            decode_value(&Type::VARBIT, &[0, 0, 0, 5, 0b1010_1000]).unwrap(),
            Value::from("10101")
        );
    }
}
