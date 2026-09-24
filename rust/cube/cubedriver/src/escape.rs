//! Client-side SQL value escaping and `?` placeholder interpolation.
//!
//! Port of `packages/cubejs-backend-shared/src/sql-escape.ts` (itself a
//! drop-in replacement for `sqlstring`'s `escape` / `format`). Drivers that do
//! not send bound parameters over the wire — the CubeStore driver when the
//! server is too old for `sendableParameters`, the ClickHouse driver over HTTP,
//! and the MySQL driver (`mysql2`'s `connection.query` interpolates
//! client-side) — inline the values with these helpers, so the SQL text they
//! produce is byte-identical to the Node.js drivers'.

use serde_json::Value;

/// Escaping rules of a SQL dialect (`SqlDialectEscapeRules`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DialectRules {
    /// Character that delimits string literals.
    pub string_quote_char: char,
    /// A quote inside a literal is escaped by doubling it (`''`).
    pub double_quote_to_escape: bool,
    /// The backslash is an escape character (MySQL), so it must be doubled.
    pub escape_backslash: bool,
    /// Character that delimits identifiers.
    pub identifier_quote_char: char,
}

/// Standard SQL (Presto, Trino, Athena, Dremio, ksqlDB, Pinot, ...).
pub const ANSI: DialectRules = DialectRules {
    string_quote_char: '\'',
    double_quote_to_escape: true,
    escape_backslash: false,
    identifier_quote_char: '"',
};

/// MySQL / MariaDB (and CubeStore, which speaks the MySQL dialect).
pub const MYSQL: DialectRules = DialectRules {
    string_quote_char: '\'',
    double_quote_to_escape: true,
    escape_backslash: true,
    identifier_quote_char: '`',
};

/// Spark SQL / Hive / Databricks: backslash escapes only.
pub const SPARK: DialectRules = DialectRules {
    string_quote_char: '\'',
    double_quote_to_escape: false,
    escape_backslash: true,
    identifier_quote_char: '`',
};

/// `EscapeDialect` from `sql-escape.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Dialect {
    #[default]
    Ansi,
    MySql,
    Spark,
}

impl Dialect {
    pub fn rules(self) -> DialectRules {
        match self {
            Dialect::Ansi => ANSI,
            Dialect::MySql => MYSQL,
            Dialect::Spark => SPARK,
        }
    }
}

/// `SqlEscaper.escapeString`.
pub fn escape_string(rules: DialectRules, value: &str) -> String {
    let q = rules.string_quote_char;
    // The backslash MUST be doubled first so we do not double the backslashes
    // introduced when escaping quotes.
    let mut escaped = if rules.escape_backslash {
        value.replace('\\', "\\\\")
    } else {
        value.to_string()
    };
    escaped = if rules.double_quote_to_escape {
        escaped.replace(q, &format!("{q}{q}"))
    } else {
        escaped.replace(q, &format!("\\{q}"))
    };
    format!("{q}{escaped}{q}")
}

/// `SqlEscaper.escapeIdentifier`.
pub fn escape_identifier(rules: DialectRules, identifier: &str) -> String {
    let q = rules.identifier_quote_char;
    format!("{q}{}{q}", identifier.replace(q, &format!("{q}{q}")))
}

/// `SqlEscaper.escapeValue`.
pub fn escape_value(rules: DialectRules, value: &Value) -> String {
    escape_value_internal(rules, value, false)
}

fn escape_value_internal(rules: DialectRules, value: &Value, stringify_objects: bool) -> String {
    match value {
        Value::Null => "NULL".to_string(),
        Value::Bool(true) => "TRUE".to_string(),
        Value::Bool(false) => "FALSE".to_string(),
        // `String(value)` in JS: `serde_json` already prints integers without a
        // fractional part and floats in the shortest round-tripping form.
        Value::Number(n) => n.to_string(),
        Value::String(s) => escape_string(rules, s),
        Value::Array(items) => items
            .iter()
            .map(|v| match v {
                Value::Array(_) => format!("({})", escape_value_internal(rules, v, true)),
                other => escape_value_internal(rules, other, true),
            })
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(map) => {
            if stringify_objects {
                // JS: `escapeString(String(value))`, and `String({})` is
                // `[object Object]` — reproduced verbatim so an accidental
                // object never turns into injectable SQL.
                escape_string(rules, "[object Object]")
            } else {
                map.iter()
                    .map(|(k, v)| {
                        format!(
                            "{} = {}",
                            escape_identifier(rules, k),
                            escape_value_internal(rules, v, true)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        }
    }
}

/// `SqlEscaper.format`: substitutes positional placeholders in `sql`.
///
/// * `?` is replaced by an escaped value,
/// * `??` by an escaped identifier,
/// * longer runs of `?` are left untouched and consume no value.
pub fn format(dialect: Dialect, sql: &str, values: &[Value]) -> String {
    let rules = dialect.rules();
    if values.is_empty() {
        return sql.to_string();
    }

    let bytes = sql.as_bytes();
    let mut result = String::with_capacity(sql.len());
    let mut chunk_index = 0usize;
    let mut values_index = 0usize;
    let mut i = 0usize;
    let mut replaced = false;

    while i < bytes.len() && values_index < values.len() {
        if bytes[i] != b'?' {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i] == b'?' {
            i += 1;
        }
        let len = i - start;
        if len <= 2 {
            let rendered = if len == 2 {
                escape_identifier(rules, &value_as_js_string(&values[values_index]))
            } else {
                escape_value(rules, &values[values_index])
            };
            result.push_str(&sql[chunk_index..start]);
            result.push_str(&rendered);
            chunk_index = i;
            values_index += 1;
            replaced = true;
        }
    }

    if !replaced {
        return sql.to_string();
    }
    if chunk_index < sql.len() {
        result.push_str(&sql[chunk_index..]);
    }
    result
}

/// `String(value)` for the `??` identifier placeholder.
fn value_as_js_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Array(_) | Value::Object(_) => value.to_string(),
    }
}

/// `formatMySql`: MySQL / MariaDB (and CubeStore) interpolation.
pub fn format_mysql(sql: &str, values: &[Value]) -> String {
    format(Dialect::MySql, sql, values)
}

/// `formatAnsi`: standard-SQL interpolation.
pub fn format_ansi(sql: &str, values: &[Value]) -> String {
    format(Dialect::Ansi, sql, values)
}

/// `escape` from `sqlstring`, MySQL dialect — used by
/// `CubeStoreDriver.createTableSqlWithOptions` for `select_statement` and
/// `source_table`.
pub fn escape_mysql_string(value: &str) -> String {
    escape_string(MYSQL, value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn escapes_strings_per_dialect() {
        assert_eq!(escape_string(MYSQL, "a'b"), "'a''b'");
        assert_eq!(escape_string(MYSQL, "a\\b"), "'a\\\\b'");
        assert_eq!(escape_string(ANSI, "a\\b"), "'a\\b'");
        assert_eq!(escape_string(ANSI, "a'b"), "'a''b'");
        assert_eq!(escape_string(SPARK, "a'b"), "'a\\'b'");
    }

    #[test]
    fn escapes_identifiers() {
        assert_eq!(escape_identifier(MYSQL, "a`b"), "`a``b`");
        assert_eq!(escape_identifier(ANSI, "a\"b"), "\"a\"\"b\"");
    }

    #[test]
    fn escapes_values() {
        assert_eq!(escape_value(MYSQL, &Value::Null), "NULL");
        assert_eq!(escape_value(MYSQL, &json!(true)), "TRUE");
        assert_eq!(escape_value(MYSQL, &json!(false)), "FALSE");
        assert_eq!(escape_value(MYSQL, &json!(42)), "42");
        assert_eq!(escape_value(MYSQL, &json!(1.5)), "1.5");
        assert_eq!(escape_value(MYSQL, &json!("x")), "'x'");
        assert_eq!(escape_value(MYSQL, &json!([1, "a", null])), "1, 'a', NULL");
        assert_eq!(escape_value(MYSQL, &json!([[1, 2], [3]])), "(1, 2), (3)");
        assert_eq!(escape_value(MYSQL, &json!({"a": 1})), "`a` = 1");
    }

    #[test]
    fn formats_placeholders() {
        assert_eq!(
            format_mysql(
                "SELECT * FROM t WHERE a = ? AND b = ?",
                &[json!(1), json!("x")]
            ),
            "SELECT * FROM t WHERE a = 1 AND b = 'x'"
        );
        // `??` is an identifier
        assert_eq!(
            format_mysql("SELECT ?? FROM t", &[json!("col")]),
            "SELECT `col` FROM t"
        );
        // more placeholders than values: the rest is left verbatim
        assert_eq!(format_mysql("SELECT ?, ?", &[json!(1)]), "SELECT 1, ?");
        // no values at all: untouched
        assert_eq!(format_mysql("SELECT ?", &[]), "SELECT ?");
        // runs longer than 2 are not placeholders and consume no value
        assert_eq!(format_mysql("SELECT ???, ?", &[json!(1)]), "SELECT ???, 1");
        // injection is escaped
        assert_eq!(
            format_mysql("SELECT ?", &[json!("a' OR 1=1 --")]),
            "SELECT 'a'' OR 1=1 --'"
        );
    }

    #[test]
    fn format_is_utf8_safe() {
        assert_eq!(
            format_mysql("SELECT 'héllo', ?", &[json!("wörld")]),
            "SELECT 'héllo', 'wörld'"
        );
    }
}
