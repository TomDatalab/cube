//! Query parameters.
//!
//! The Node.js driver (via `pg`) sends every parameter in the *text* format
//! and lets the server infer the type from the statement. [`TextParam`] does
//! the same: it accepts any PostgreSQL type and encodes the JSON value the way
//! `pg`'s `prepareValue` does (arrays as `{...}` literals, objects as JSON).

use std::error::Error;

use bytes::BytesMut;
use serde_json::Value;
use tokio_postgres::types::{to_sql_checked, Format, IsNull, ToSql, Type};

/// The 65535-parameter limit of the PostgreSQL bind message.
pub const MAX_PARAMS: usize = 65_535;

/// Port of `PostgresDriver.checkValuesLimit`.
pub fn check_values_limit(values: &[Value]) -> crate::error::Result<()> {
    if values.len() > MAX_PARAMS {
        return Err(crate::error::DriverError::Query(format!(
            "PostgreSQL protocol does not support more than 65535 parameters, but {} passed",
            values.len()
        )));
    }
    Ok(())
}

/// A parameter sent in text format, whatever the inferred server type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextParam(pub Option<String>);

impl TextParam {
    /// Converts a JSON value into its `pg`-compatible text form.
    pub fn from_json(value: &Value) -> Self {
        TextParam(prepare_value(value))
    }
}

impl From<&Value> for TextParam {
    fn from(value: &Value) -> Self {
        TextParam::from_json(value)
    }
}

impl ToSql for TextParam {
    fn to_sql(
        &self,
        _ty: &Type,
        out: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn Error + Sync + Send>> {
        match &self.0 {
            None => Ok(IsNull::Yes),
            Some(s) => {
                out.extend_from_slice(s.as_bytes());
                Ok(IsNull::No)
            }
        }
    }

    fn accepts(_ty: &Type) -> bool {
        true
    }

    fn encode_format(&self, _ty: &Type) -> Format {
        Format::Text
    }

    to_sql_checked!();
}

/// Port of `pg`'s `prepareValue`: `None` is SQL `NULL`.
pub fn prepare_value(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(s.clone()),
        Value::Array(items) => Some(array_string(items)),
        Value::Object(_) => Some(value.to_string()),
    }
}

/// Port of `pg`'s `arrayString`.
fn array_string(items: &[Value]) -> String {
    let mut result = String::from("{");
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            result.push(',');
        }
        match item {
            Value::Null => result.push_str("NULL"),
            Value::Array(inner) => result.push_str(&array_string(inner)),
            other => {
                let repr = prepare_value(other).unwrap_or_default();
                result.push_str(&escape_element(&repr));
            }
        }
    }
    result.push('}');
    result
}

fn escape_element(element: &str) -> String {
    let escaped = element.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Converts Cube parameters to the wire representation.
pub fn to_text_params(values: &[Value]) -> Vec<TextParam> {
    values.iter().map(TextParam::from_json).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scalars() {
        assert_eq!(prepare_value(&Value::Null), None);
        assert_eq!(prepare_value(&json!(true)).as_deref(), Some("true"));
        assert_eq!(prepare_value(&json!(1.5)).as_deref(), Some("1.5"));
        assert_eq!(prepare_value(&json!("a")).as_deref(), Some("a"));
        assert_eq!(
            prepare_value(&json!({"a": 1})).as_deref(),
            Some(r#"{"a":1}"#)
        );
    }

    #[test]
    fn arrays_are_pg_literals() {
        assert_eq!(
            prepare_value(&json!([1, "two", null, true])).as_deref(),
            Some(r#"{"1","two",NULL,"true"}"#)
        );
        assert_eq!(
            prepare_value(&json!([["a", "b"], ["c"]])).as_deref(),
            Some(r#"{{"a","b"},{"c"}}"#)
        );
        assert_eq!(
            prepare_value(&json!(["say \"hi\"", "back\\slash"])).as_deref(),
            Some(r#"{"say \"hi\"","back\\slash"}"#)
        );
        assert_eq!(prepare_value(&json!([])).as_deref(), Some("{}"));
    }

    #[test]
    fn text_param_encodes_bytes() {
        let mut buf = BytesMut::new();
        let p = TextParam::from_json(&json!("2020-01-01"));
        assert!(matches!(
            p.to_sql(&Type::TIMESTAMP, &mut buf).unwrap(),
            IsNull::No
        ));
        assert_eq!(&buf[..], b"2020-01-01");
        assert!(matches!(p.encode_format(&Type::INT4), Format::Text));
        let p = TextParam::from_json(&Value::Null);
        assert!(matches!(
            p.to_sql(&Type::INT4, &mut buf).unwrap(),
            IsNull::Yes
        ));
        assert!(<TextParam as ToSql>::accepts(&Type::INT4_ARRAY));
    }

    #[test]
    fn values_limit() {
        let ok: Vec<Value> = vec![json!("x"); 65_535];
        assert!(check_values_limit(&ok).is_ok());
        let too_many: Vec<Value> = vec![json!("x"); 65_536];
        assert_eq!(
            check_values_limit(&too_many).unwrap_err().to_string(),
            "PostgreSQL protocol does not support more than 65535 parameters, but 65536 passed"
        );
    }
}
