//! Firebolt type mapping and value hydration.

use serde_json::Value;

use crate::types::GenericType;

/// Port of `FireboltTypeToGeneric`.
pub fn firebolt_to_generic(db_type: &str) -> Option<GenericType> {
    match db_type {
        "long" => Some(GenericType::Bigint),
        _ => None,
    }
}

/// Port of `FireboltDriver.toGenericType`.
///
/// The `(nullable|array)(…)` branch of the Node implementation can never
/// return (it re-tests `columnType` instead of the inner type), and the
/// `numeric(p, s)` branch only fills in precision/scale, which the base
/// implementation ignores for a type name it does not know. Both quirks are
/// reproduced verbatim so that the generic types match the Node driver.
pub fn to_generic_type(
    db_type: &str,
    precision: Option<i64>,
    scale: Option<i64>,
    precise_decimal: bool,
) -> GenericType {
    if let Some(generic) = firebolt_to_generic(db_type) {
        return generic;
    }

    let (mut precision, mut scale) = (precision, scale);
    if let Some((p, s)) = parse_numeric(db_type) {
        precision = Some(p);
        scale = Some(s);
    }

    crate::types::to_generic_type(db_type, precision, scale, precise_decimal)
}

/// `/^numeric\s*\(\s*(\d+)\s*,\s*(\d+)\s*\)$/i`.
fn parse_numeric(db_type: &str) -> Option<(i64, i64)> {
    let lowered = db_type.trim().to_lowercase();
    let rest = lowered.strip_prefix("numeric")?.trim_start();
    let inner = rest.strip_prefix('(')?.strip_suffix(')')?;
    let (p, s) = inner.split_once(',')?;
    Some((p.trim().parse().ok()?, s.trim().parse().ok()?))
}

/// The type names `firebolt-sdk`'s `isNumberType` treats as numeric.
const NUMBER_TYPES: &[&str] = &[
    "int",
    "integer",
    "bigint",
    "long",
    "short",
    "tinyint",
    "smallint",
    "float",
    "real",
    "double",
    "double precision",
    "decimal",
    "numeric",
];

/// Port of `isNumberType`: `nullable(...)`, `array(...)` and a trailing
/// `null` are unwrapped before the name is looked up.
pub fn is_number_type(db_type: &str) -> bool {
    let mut type_name = db_type.trim().to_lowercase();
    loop {
        type_name = type_name.trim().trim_end_matches("null").trim().to_string();
        let unwrapped = type_name
            .strip_prefix("nullable(")
            .or_else(|| type_name.strip_prefix("array("))
            .and_then(|rest| rest.strip_suffix(')'));
        match unwrapped {
            Some(inner) => type_name = inner.trim().to_string(),
            None => break,
        }
    }
    // `decimal(38, 9)` / `numeric(10, 2)` keep their base name.
    let base = match type_name.split_once('(') {
        Some((base, _)) => base.trim(),
        None => type_name.as_str(),
    };
    NUMBER_TYPES.contains(&base)
}

/// Port of `getHydratedValue`: numeric values are returned as strings.
pub fn hydrate_value(value: &Value, db_type: &str) -> Value {
    if value.is_null() || !is_number_type(db_type) {
        return value.clone();
    }
    match value {
        Value::Number(n) => Value::String(n.to_string()),
        Value::String(s) => Value::String(s.clone()),
        other => Value::String(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_types() {
        assert_eq!(
            to_generic_type("long", None, None, false),
            GenericType::Bigint
        );
        assert_eq!(
            to_generic_type("text", None, None, false),
            GenericType::Text
        );
        assert_eq!(
            to_generic_type("integer", None, None, false),
            GenericType::Int
        );
        assert_eq!(
            to_generic_type("boolean", None, None, false),
            GenericType::Boolean
        );
        // `numeric(p, s)` has no entry in the base table, so (as in JS) the
        // type name is passed through unchanged.
        assert_eq!(
            to_generic_type("numeric(10, 2)", None, None, true),
            GenericType::Other("numeric(10, 2)".into())
        );
        assert_eq!(
            to_generic_type("numeric", Some(10), Some(2), true),
            GenericType::Decimal(Some((10, 2)))
        );
    }

    #[test]
    fn numeric_parsing() {
        assert_eq!(parse_numeric("numeric(10, 2)"), Some((10, 2)));
        assert_eq!(parse_numeric("NUMERIC (38,9)"), Some((38, 9)));
        assert_eq!(parse_numeric("decimal(10, 2)"), None);
        assert_eq!(parse_numeric("numeric"), None);
    }

    #[test]
    fn number_types() {
        assert!(is_number_type("int"));
        assert!(is_number_type("BIGINT"));
        assert!(is_number_type("nullable(int)"));
        assert!(is_number_type("array(double)"));
        assert!(is_number_type("numeric(10, 2)"));
        assert!(is_number_type("int null"));
        assert!(!is_number_type("text"));
        assert!(!is_number_type("boolean"));
        assert!(!is_number_type("timestamp"));
    }

    #[test]
    fn hydration() {
        assert_eq!(hydrate_value(&Value::from(1), "int"), Value::from("1"));
        assert_eq!(
            hydrate_value(&Value::from(1.25), "numeric(10, 2)"),
            Value::from("1.25")
        );
        assert_eq!(hydrate_value(&Value::Null, "int"), Value::Null);
        assert_eq!(hydrate_value(&Value::from("a"), "text"), Value::from("a"));
        assert_eq!(
            hydrate_value(&Value::Bool(true), "boolean"),
            Value::Bool(true)
        );
    }
}
