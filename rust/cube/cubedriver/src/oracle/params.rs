//! Query parameters: port of `OracleDriver.normalizeParams` and the value
//! binding node-oracledb applies to plain JS values.

use std::collections::HashMap;

use oracledb::{OracleNumber, ToDbValue};
use serde_json::Value;

use crate::error::{DriverError, Result};

/// Port of `OracleDriver.normalizeParams`.
///
/// Cube renders parameters as `?` (and `:"?"`); both are rewritten into named
/// binds `:cb_param_N`. Placeholders carrying the same value share one bind,
/// which keeps a repeated expression textually identical (Oracle otherwise
/// rejects e.g. a `CASE` in both `SELECT` and `GROUP BY` with ORA-00979).
///
/// Like Node, the rewrite is purely textual — a `?` inside a string literal
/// is a placeholder too — and happens only when there are values.
pub fn normalize_params(query: &str, values: &[Value]) -> (String, Vec<(String, Value)>) {
    if values.is_empty() {
        return (query.to_string(), Vec::new());
    }

    let mut binds: Vec<(String, Value)> = Vec::new();
    let mut value_to_name: HashMap<ValueKey, String> = HashMap::new();
    let mut idx = 0usize;
    let mut out = String::with_capacity(query.len() + values.len() * 12);

    let mut rest = query;
    loop {
        // `:"?"` must be matched as a whole before a lone `?`.
        let colon = rest.find(":\"?\"");
        let question = rest.find('?');
        let (pos, len) = match (colon, question) {
            (Some(c), Some(q)) if c < q => (c, 4),
            (_, Some(q)) => (q, 1),
            (Some(c), None) => (c, 4),
            (None, None) => break,
        };
        out.push_str(&rest[..pos]);
        rest = &rest[pos + len..];

        // JS: `values[idx]` is `undefined` past the end; node-oracledb then
        // binds NULL. The key distinguishes it from nothing else here.
        let value = values.get(idx).cloned().unwrap_or(Value::Null);
        idx += 1;
        let key = ValueKey::of(&value, idx);
        let name = match value_to_name.get(&key) {
            Some(name) => name.clone(),
            None => {
                let name = format!("cb_param_{}", binds.len());
                value_to_name.insert(key, name.clone());
                binds.push((name.clone(), value));
                name
            }
        };
        out.push(':');
        out.push_str(&name);
    }
    out.push_str(rest);

    (out, binds)
}

/// The identity JS' `Map` uses (SameValueZero): `1` and `'1'` differ, `1`
/// and `1.0` do not, `0` and `-0` do not, and objects/arrays are compared by
/// reference, so two of them never share a bind.
#[derive(Debug, PartialEq, Eq, Hash)]
enum ValueKey {
    Null,
    Bool(bool),
    Number(u64),
    String(String),
    Unique(usize),
}

impl ValueKey {
    fn of(value: &Value, position: usize) -> Self {
        match value {
            Value::Null => ValueKey::Null,
            Value::Bool(b) => ValueKey::Bool(*b),
            Value::Number(n) => {
                let f = n.as_f64().unwrap_or(f64::NAN);
                // `+ 0.0` folds -0 into 0.
                ValueKey::Number((f + 0.0).to_bits())
            }
            Value::String(s) => ValueKey::String(s.clone()),
            Value::Array(_) | Value::Object(_) => ValueKey::Unique(position),
        }
    }
}

/// A bind value owned for the duration of the statement.
#[derive(Debug, Clone)]
pub enum Bind {
    /// JS `null` / `undefined`: a NULL `VARCHAR2`.
    Null(Option<String>),
    /// JS `string`: `VARCHAR2`.
    Text(String),
    /// JS `number`: `NUMBER`.
    Number(OracleNumber),
    /// JS `boolean`: `BOOLEAN` (SQL-level booleans need Oracle 23ai, as with
    /// node-oracledb).
    Bool(bool),
}

impl Bind {
    /// Maps one JSON parameter the way node-oracledb maps the JS value.
    pub fn from_json(value: &Value) -> Result<Self> {
        Ok(match value {
            Value::Null => Bind::Null(None),
            Value::Bool(b) => Bind::Bool(*b),
            Value::String(s) => Bind::Text(s.clone()),
            Value::Number(n) => {
                // `OracleNumber` parses plain decimal text only; `f64`'s
                // `Display` never uses an exponent, `serde_json`'s does.
                let text = if let Some(i) = n.as_i64() {
                    i.to_string()
                } else if let Some(u) = n.as_u64() {
                    u.to_string()
                } else {
                    n.as_f64().unwrap_or_default().to_string()
                };
                Bind::Number(text.parse().map_err(|e: oracledb::Error| {
                    DriverError::Query(format!("Cannot bind number {n} for Oracle: {e}"))
                })?)
            }
            Value::Array(_) | Value::Object(_) => {
                return Err(DriverError::NotImplemented(format!(
                    "The Oracle driver cannot bind an array or object parameter: {value}"
                )))
            }
        })
    }

    /// The value as the `oracledb` crate consumes it.
    pub fn as_db_value(&self) -> &dyn ToDbValue {
        match self {
            Bind::Null(v) => v,
            Bind::Text(v) => v,
            Bind::Number(v) => v,
            Bind::Bool(v) => v,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn names(binds: &[(String, Value)]) -> Vec<(&str, Value)> {
        binds.iter().map(|(n, v)| (n.as_str(), v.clone())).collect()
    }

    // The cases below are the Node driver's `normalizeParams` contract.

    #[test]
    fn no_values_leaves_the_query_untouched() {
        let (sql, binds) = normalize_params("SELECT ? FROM dual", &[]);
        assert_eq!(sql, "SELECT ? FROM dual");
        assert!(binds.is_empty());
    }

    #[test]
    fn question_marks_become_named_binds() {
        let (sql, binds) = normalize_params(
            "SELECT * FROM t WHERE a = ? AND b = ?",
            &[json!(1), json!("x")],
        );
        assert_eq!(sql, "SELECT * FROM t WHERE a = :cb_param_0 AND b = :cb_param_1");
        assert_eq!(
            names(&binds),
            vec![("cb_param_0", json!(1)), ("cb_param_1", json!("x"))]
        );
    }

    #[test]
    fn quoted_question_mark_placeholder_is_matched_whole() {
        let (sql, binds) = normalize_params(
            "SELECT :\"?\" FROM t WHERE b = ?",
            &[json!("a"), json!("b")],
        );
        assert_eq!(sql, "SELECT :cb_param_0 FROM t WHERE b = :cb_param_1");
        assert_eq!(binds.len(), 2);
    }

    #[test]
    fn equal_values_share_one_bind() {
        let (sql, binds) = normalize_params(
            "SELECT CASE WHEN x = ? THEN 1 END FROM t GROUP BY CASE WHEN x = ? THEN 1 END",
            &[json!("v"), json!("v")],
        );
        assert_eq!(
            sql,
            "SELECT CASE WHEN x = :cb_param_0 THEN 1 END FROM t GROUP BY CASE WHEN x = :cb_param_0 THEN 1 END"
        );
        assert_eq!(names(&binds), vec![("cb_param_0", json!("v"))]);
    }

    #[test]
    fn same_value_zero_semantics() {
        // 1 and '1' are different keys; 1 and 1.0 are the same; nulls share.
        let (sql, binds) = normalize_params(
            "? ? ? ? ?",
            &[json!(1), json!("1"), json!(1.0), json!(null), json!(null)],
        );
        assert_eq!(sql, ":cb_param_0 :cb_param_1 :cb_param_0 :cb_param_2 :cb_param_2");
        assert_eq!(binds.len(), 3);

        // objects never share (JS compares them by reference)
        let (sql, _) = normalize_params("? ?", &[json!([1]), json!([1])]);
        assert_eq!(sql, ":cb_param_0 :cb_param_1");
    }

    #[test]
    fn question_marks_in_literals_are_replaced_like_node() {
        let (sql, _) = normalize_params("SELECT '?' FROM t WHERE a = ?", &[json!(1), json!(2)]);
        assert_eq!(sql, "SELECT ':cb_param_0' FROM t WHERE a = :cb_param_1");
    }

    #[test]
    fn values_are_bound_like_node_oracledb() {
        assert!(matches!(Bind::from_json(&json!(null)).unwrap(), Bind::Null(None)));
        assert!(matches!(Bind::from_json(&json!(true)).unwrap(), Bind::Bool(true)));
        match Bind::from_json(&json!(1.25)).unwrap() {
            Bind::Number(n) => assert_eq!(n.to_string(), "1.25"),
            other => panic!("{other:?}"),
        }
        match Bind::from_json(&json!(1e20)).unwrap() {
            Bind::Number(n) => assert_eq!(n.to_string(), "100000000000000000000"),
            other => panic!("{other:?}"),
        }
        match Bind::from_json(&json!(-42)).unwrap() {
            Bind::Number(n) => assert_eq!(n.to_string(), "-42"),
            other => panic!("{other:?}"),
        }
        let err = Bind::from_json(&json!({"a": 1})).unwrap_err();
        assert!(matches!(err, DriverError::NotImplemented(_)));
    }
}
