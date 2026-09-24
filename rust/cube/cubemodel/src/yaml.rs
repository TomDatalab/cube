//! YAML -> normalised JSON, reproducing `YamlCompiler` + `camelizeCube`.

use serde_json::{Map, Value};

use crate::error::ErrorReporter;
use crate::naming::camelize_lower;

/// Parses YAML into an order-preserving JSON value.
pub fn parse_yaml(source: &str) -> Result<Option<Value>, serde_yaml::Error> {
    if source.trim().is_empty() {
        return Ok(None);
    }
    let value: Value = serde_yaml::from_str(source)?;
    if value.is_null() {
        return Ok(None);
    }
    Ok(Some(value))
}

/// Keys whose value must not be camelized at all (level 1 `meta`).
fn camelize_object_part(value: &mut Value, camelize_keys: bool, level: usize) {
    match value {
        Value::Array(items) => {
            for item in items.iter_mut() {
                camelize_object_part(item, true, level + 1);
            }
        }
        Value::Object(map) => {
            let keys: Vec<String> = map.keys().cloned().collect();
            let mut renames: Vec<(String, String)> = Vec::new();
            for key in keys {
                // `meta` payloads are user data: never touched.
                if !(level == 1 && key == "meta") {
                    // IGNORE_CAMELIZE: level 1 `granularities` keys are granularity
                    // names, so they must be kept verbatim.
                    let child_camelize = !(level == 1 && key == "granularities");
                    if let Some(child) = map.get_mut(&key) {
                        camelize_object_part(child, child_camelize, level + 1);
                    }
                }
                if camelize_keys {
                    let camelized = camelize_lower(&key);
                    if camelized != key {
                        renames.push((key, camelized));
                    }
                }
            }
            for (from, to) in renames {
                if let Some(v) = map.shift_remove(&from) {
                    map.insert(to, v);
                }
            }
        }
        _ => {}
    }
}

/// `camelizeCube`: camelize the cube's own keys, then deep-camelize the parts
/// that hold nested snake_case properties.
pub fn camelize_cube(cube: &mut Value) {
    let Value::Object(map) = cube else { return };

    let keys: Vec<String> = map.keys().cloned().collect();
    let mut renames: Vec<(String, String)> = Vec::new();
    for key in keys {
        let camelized = camelize_lower(&key);
        if camelized != key {
            renames.push((key, camelized));
        }
    }
    for (from, to) in renames {
        if let Some(v) = map.shift_remove(&from) {
            map.insert(to, v);
        }
    }

    for part in [
        "measures",
        "dimensions",
        "preAggregations",
        "cubes",
        "accessPolicy",
        "folders",
    ] {
        if let Some(v) = map.get_mut(part) {
            camelize_object_part(v, false, 0);
        }
    }
}

fn name_of(item: &Value) -> Option<&str> {
    item.get("name").and_then(Value::as_str)
}

/// `YamlCompiler.checkDuplicateNames`.
pub fn check_duplicate_names(
    items: &[Value],
    reporter: &mut ErrorReporter,
    message: impl Fn(&str) -> String,
) {
    let mut seen: Vec<&str> = Vec::new();
    for item in items {
        if let Some(name) = name_of(item) {
            if seen.contains(&name) {
                reporter.error(message(name));
            }
            seen.push(name);
        }
    }
}

/// Context for the duplicate-member message of a nested array.
struct ParentCtx<'a> {
    kind: &'a str,
    name: &'a str,
}

/// `YamlCompiler.yamlArrayToObj`: turns `[{name: x, ...}]` into `{x: {...}}`.
pub fn yaml_array_to_obj(
    value: Option<Value>,
    member_type: &str,
    cube_name: &str,
    reporter: &mut ErrorReporter,
) -> Value {
    yaml_array_to_obj_inner(value, member_type, cube_name, None, reporter)
}

fn yaml_array_to_obj_inner(
    value: Option<Value>,
    member_type: &str,
    cube_name: &str,
    parent: Option<ParentCtx<'_>>,
    reporter: &mut ErrorReporter,
) -> Value {
    let items = match value {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items,
        Some(_) => {
            reporter.error(format!("{member_type}s must be defined as array"));
            return Value::Object(Map::new());
        }
    };

    check_duplicate_names(&items, reporter, |name| {
        match &parent {
        Some(p) => format!(
            "Found duplicate {member_type} '{name}' in {} '{}' in cube '{cube_name}'.",
            p.kind, p.name
        ),
        None => format!(
            "Member names must be unique within a cube. Found duplicate {member_type} '{name}' in cube '{cube_name}'."
        ),
    }
    });

    let mut out = Map::new();
    for item in items {
        let Value::Object(mut obj) = item else {
            reporter.error(format!(
                "name isn't defined for {member_type}: {}",
                serde_json::to_string(&item).unwrap_or_default()
            ));
            continue;
        };

        let name = match obj.shift_remove("name").and_then(|v| match v {
            Value::String(s) => Some(s),
            _ => None,
        }) {
            Some(name) => name,
            None => {
                reporter.error(format!(
                    "name isn't defined for {member_type}: {}",
                    serde_json::to_string(&Value::Object(obj)).unwrap_or_default()
                ));
                continue;
            }
        };

        if member_type == "preAggregation" {
            if let Some(indexes) = obj.shift_remove("indexes") {
                let converted = yaml_array_to_obj_inner(
                    Some(indexes),
                    "preAggregation.index",
                    cube_name,
                    Some(ParentCtx {
                        kind: "pre-aggregation",
                        name: &name,
                    }),
                    reporter,
                );
                obj.insert("indexes".to_string(), converted);
            }
        }

        if member_type == "dimension" {
            if let Some(granularities) = obj.shift_remove("granularities") {
                let converted = yaml_array_to_obj_inner(
                    Some(granularities),
                    "dimension.granularity",
                    cube_name,
                    Some(ParentCtx {
                        kind: "time dimension",
                        name: &name,
                    }),
                    reporter,
                );
                obj.insert("granularities".to_string(), converted);
            }
        }

        if let Some(Value::Array(shifts)) = obj.get("timeShift") {
            let shifts = shifts.clone();
            check_duplicate_names(&shifts, reporter, |shift_name| {
                format!(
                    "Time shift names must be unique within a {member_type}. Found duplicate time shift '{shift_name}' in {member_type} '{name}' in cube '{cube_name}'."
                )
            });
        }

        out.insert(name, Value::Object(obj));
    }

    Value::Object(out)
}
