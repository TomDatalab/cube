//! Port of `named-numeric-formats.ts`.

use serde_json::{json, Value};

/// `NAMED_NUMERIC_FORMATS`.
pub fn resolve_named_numeric_format(value: &str) -> Option<&'static str> {
    Some(match value {
        "number_0" => ",.0f",
        "number_1" => ",.1~f",
        "number_2" => ",.2~f",
        "number_3" => ",.3~f",
        "number_4" => ",.4~f",
        "number_5" => ",.5~f",
        "number_6" => ",.6~f",
        "percent_0" => ".0%",
        "percent_1" => ".1~%",
        "percent_2" => ".2~%",
        "percent_3" => ".3~%",
        "percent_4" => ".4~%",
        "percent_5" => ".5~%",
        "percent_6" => ".6~%",
        "currency_0" => "$,.0f",
        "currency_1" => "$,.1~f",
        "currency_2" => "$,.2~f",
        "currency_3" => "$,.3~f",
        "currency_4" => "$,.4~f",
        "currency_5" => "$,.5~f",
        "currency_6" => "$,.6~f",
        "decimal" => ",.2~f",
        "decimal_0" => ",.0f",
        "decimal_1" => ",.1~f",
        "decimal_2" => ",.2~f",
        "decimal_3" => ",.3~f",
        "decimal_4" => ",.4~f",
        "decimal_5" => ",.5~f",
        "decimal_6" => ",.6~f",
        "abbr" => ".2~s",
        "abbr_0" => ".0~s",
        "abbr_1" => ".1~s",
        "abbr_2" => ".2~s",
        "abbr_3" => ".3~s",
        "abbr_4" => ".4~s",
        "abbr_5" => ".5~s",
        "abbr_6" => ".6~s",
        "id" => ".0f",
        "accounting" => "(,.2~f",
        "accounting_0" => "(,.0f",
        "accounting_1" => "(,.1~f",
        "accounting_2" => "(,.2~f",
        "accounting_3" => "(,.3~f",
        "accounting_4" => "(,.4~f",
        "accounting_5" => "(,.5~f",
        "accounting_6" => "(,.6~f",
        _ => return None,
    })
}

/// `STANDARD_FORMAT_SPECIFIERS`.
pub fn standard_format_specifier(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "percent" => ("percent", ".2~%"),
        "currency" => ("currency", "$,.2~f"),
        "number" => ("number", ",.2~f"),
        "abbr" => ("abbr", ".2~s"),
        "accounting" => ("accounting", "(,.2~f"),
        "id" => ("id", ".0f"),
        _ => return None,
    })
}

/// `DEFAULT_FORMAT_SPECIFIER`.
pub const DEFAULT_FORMAT_SPECIFIER: (&str, &str) = ("number", ",.2~f");

const EXCLUDED_MEASURE_TYPES: &[&str] = &["string", "boolean", "time"];

/// `CubeToMetaTransformer.transformMeasureFormat`.
pub fn transform_measure_format(format: Option<&Value>) -> Option<Value> {
    let format = format?;
    let Some(name) = format.as_str() else {
        // Already an object format — passed through unchanged.
        return Some(format.clone());
    };
    if name.is_empty() {
        return None;
    }
    if let Some(resolved) = resolve_named_numeric_format(name) {
        return Some(json!({ "type": "custom-numeric", "value": resolved, "alias": name }));
    }
    if matches!(name, "percent" | "currency" | "number") {
        return Some(Value::String(name.to_string()));
    }
    Some(json!({ "type": "custom-numeric", "value": name }))
}

/// `CubeToMetaTransformer.transformDimensionFormat`.
pub fn transform_dimension_format(
    format: Option<&Value>,
    dimension_type: Option<&str>,
) -> Option<Value> {
    let format = format?;
    let Some(name) = format.as_str() else {
        return Some(format.clone());
    };
    if name.is_empty() {
        return None;
    }
    if let Some(resolved) = resolve_named_numeric_format(name) {
        return Some(json!({ "type": "custom-numeric", "value": resolved, "alias": name }));
    }
    if matches!(name, "imageUrl" | "currency" | "percent" | "number" | "id") {
        return Some(Value::String(name.to_string()));
    }
    match dimension_type {
        Some("time") => Some(json!({ "type": "custom-time", "value": name })),
        Some("number") => Some(json!({ "type": "custom-numeric", "value": name })),
        _ => Some(Value::String(name.to_string())),
    }
}

/// `CubeToMetaTransformer.resolveFormatDescription`.
pub fn resolve_format_description(
    format: Option<&Value>,
    member_type: &str,
    is_measure: bool,
    currency: Option<&str>,
) -> Option<Value> {
    if is_measure {
        if EXCLUDED_MEASURE_TYPES.contains(&member_type) {
            return None;
        }
    } else if member_type != "number" {
        return None;
    }

    let (name, specifier) = match format {
        Some(Value::Object(obj))
            if obj.get("type").and_then(Value::as_str) == Some("custom-numeric") =>
        {
            let name = obj
                .get("alias")
                .and_then(Value::as_str)
                .unwrap_or("custom")
                .to_string();
            let specifier = obj
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            (name, specifier)
        }
        Some(Value::String(s)) => match standard_format_specifier(s) {
            Some((name, specifier)) => (name.to_string(), specifier.to_string()),
            None => (
                DEFAULT_FORMAT_SPECIFIER.0.to_string(),
                DEFAULT_FORMAT_SPECIFIER.1.to_string(),
            ),
        },
        _ => (
            DEFAULT_FORMAT_SPECIFIER.0.to_string(),
            DEFAULT_FORMAT_SPECIFIER.1.to_string(),
        ),
    };

    let mut desc = serde_json::Map::new();
    desc.insert("name".to_string(), Value::String(name));
    desc.insert("specifier".to_string(), Value::String(specifier));
    if let Some(currency) = currency {
        desc.insert("currency".to_string(), Value::String(currency.to_string()));
    }
    Some(Value::Object(desc))
}
