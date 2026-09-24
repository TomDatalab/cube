//! Post-processing of a Cube REST result into the shape the GraphQL schema
//! resolves against — `parseDates` and the `R.set(R.lensPath(...))` reducer at
//! the bottom of `graphql.ts`.

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde_json::{Map, Value};

use crate::naming::un_capitalize;

/// `parseDates(result)` — rewrite every `time`-typed cell of `result.data` as
/// an ISO-8601 instant, interpreting naive timestamps in `result.query.timezone`.
///
/// Mirrors `moment.tz(value, timezone).toISOString()`.
pub fn parse_dates(result: &mut Value) {
    let timezone = result
        .get("query")
        .and_then(|q| q.get("timezone"))
        .and_then(Value::as_str)
        .unwrap_or("UTC")
        .to_string();

    let date_keys = time_member_keys(result);
    if date_keys.is_empty() {
        return;
    }

    let Some(Value::Array(rows)) = result.get_mut("data") else {
        return;
    };

    for row in rows {
        let Some(row) = row.as_object_mut() else {
            continue;
        };
        for key in &date_keys {
            if let Some(value) = row.get(key).and_then(Value::as_str) {
                if let Some(iso) = to_iso_string(value, &timezone) {
                    row.insert(key.clone(), Value::String(iso));
                }
            }
        }
    }
}

fn time_member_keys(result: &Value) -> Vec<String> {
    let mut keys = Vec::new();
    let Some(annotation) = result.get("annotation") else {
        return keys;
    };

    for section in ["measures", "dimensions", "timeDimensions"] {
        if let Some(Value::Object(members)) = annotation.get(section) {
            for (key, value) in members {
                if value.get("type").and_then(Value::as_str) == Some("time") {
                    keys.push(key.clone());
                }
            }
        }
    }

    keys
}

/// `moment.tz(value, timezone).toISOString()`.
fn to_iso_string(value: &str, timezone: &str) -> Option<String> {
    // An instant that already carries an offset is timezone independent.
    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Some(format_iso(parsed.with_timezone(&Utc)));
    }

    let naive = parse_naive(value)?;
    let tz: Tz = timezone.parse().unwrap_or(chrono_tz::UTC);

    let instant = tz
        .from_local_datetime(&naive)
        .earliest()
        .or_else(|| tz.from_local_datetime(&naive).latest())?;

    Some(format_iso(instant.with_timezone(&Utc)))
}

fn parse_naive(value: &str) -> Option<NaiveDateTime> {
    const DATE_TIME_FORMATS: [&str; 6] = [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ];

    for format in DATE_TIME_FORMATS {
        if let Ok(parsed) = NaiveDateTime::parse_from_str(value, format) {
            return Some(parsed);
        }
    }

    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
}

fn format_iso(value: DateTime<Utc>) -> String {
    value.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// The row reshaper from the `cube` resolver: turn flat
/// `{ "Orders.count": 10 }` rows into `{ "orders": { "count": 10 } }`, adding
/// the `value` leaf for plain time dimensions and dropping the un-granular
/// duplicate of a `timeDimensions` member.
pub fn shape_rows(result: &Value) -> Vec<Value> {
    let empty = Map::new();
    let dimensions = result
        .get("annotation")
        .and_then(|a| a.get("dimensions"))
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let time_dimensions = result
        .get("annotation")
        .and_then(|a| a.get("timeDimensions"))
        .and_then(Value::as_object)
        .unwrap_or(&empty);

    result
        .get("data")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    let mut out = Map::new();
                    if let Some(row) = row.as_object() {
                        for (key, value) in row {
                            let mut path: Vec<String> =
                                key.split('.').map(str::to_string).collect();
                            if path.is_empty() {
                                continue;
                            }
                            path[0] = un_capitalize(&path[0]);

                            if dimensions
                                .get(key)
                                .and_then(|d| d.get("type"))
                                .and_then(Value::as_str)
                                == Some("time")
                            {
                                path.push("value".to_string());
                            }

                            if time_dimensions.contains_key(key) && path.len() != 3 {
                                continue;
                            }

                            set_path(&mut out, &path, value.clone());
                        }
                    }
                    Value::Object(out)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn set_path(target: &mut Map<String, Value>, path: &[String], value: Value) {
    match path {
        [] => {}
        [last] => {
            target.insert(last.clone(), value);
        }
        [head, rest @ ..] => {
            let entry = target
                .entry(head.clone())
                .or_insert_with(|| Value::Object(Map::new()));
            if !entry.is_object() {
                *entry = Value::Object(Map::new());
            }
            set_path(
                entry.as_object_mut().expect("just ensured object"),
                rest,
                value,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shapes_measures_and_dimensions() {
        let result = json!({
            "annotation": {
                "measures": { "Orders.count": { "type": "number" } },
                "dimensions": { "Orders.status": { "type": "string" } },
                "timeDimensions": {}
            },
            "data": [{ "Orders.count": 10, "Orders.status": "completed" }]
        });

        assert_eq!(
            shape_rows(&result),
            vec![json!({ "orders": { "count": 10, "status": "completed" } })]
        );
    }

    #[test]
    fn plain_time_dimensions_get_a_value_leaf() {
        let result = json!({
            "annotation": {
                "measures": {},
                "dimensions": { "Orders.createdAt": { "type": "time" } },
                "timeDimensions": {}
            },
            "data": [{ "Orders.createdAt": "2022-01-01T00:00:00.000" }]
        });

        assert_eq!(
            shape_rows(&result),
            vec![json!({ "orders": { "createdAt": { "value": "2022-01-01T00:00:00.000" } } })]
        );
    }

    #[test]
    fn un_granular_time_dimension_duplicates_are_dropped() {
        let result = json!({
            "annotation": {
                "measures": {},
                "dimensions": {},
                "timeDimensions": {
                    "Orders.createdAt": { "type": "time" },
                    "Orders.createdAt.day": { "type": "time" }
                }
            },
            "data": [{
                "Orders.createdAt": "2022-01-01T00:00:00.000",
                "Orders.createdAt.day": "2022-01-01T00:00:00.000"
            }]
        });

        assert_eq!(
            shape_rows(&result),
            vec![json!({ "orders": { "createdAt": { "day": "2022-01-01T00:00:00.000" } } })]
        );
    }

    #[test]
    fn parses_dates_in_the_query_timezone() {
        let mut result = json!({
            "query": { "timezone": "America/Los_Angeles" },
            "annotation": {
                "measures": {},
                "dimensions": {},
                "timeDimensions": { "Orders.createdAt.day": { "type": "time" } }
            },
            "data": [{ "Orders.createdAt.day": "2022-01-01T00:00:00.000" }]
        });

        parse_dates(&mut result);

        assert_eq!(
            result["data"][0]["Orders.createdAt.day"],
            json!("2022-01-01T08:00:00.000Z")
        );
    }

    #[test]
    fn offsets_are_preserved_as_instants() {
        let mut result = json!({
            "query": { "timezone": "America/Los_Angeles" },
            "annotation": {
                "measures": {},
                "dimensions": {},
                "timeDimensions": { "Orders.createdAt.day": { "type": "time" } }
            },
            "data": [{ "Orders.createdAt.day": "2022-01-01T00:00:00Z" }]
        });

        parse_dates(&mut result);

        assert_eq!(
            result["data"][0]["Orders.createdAt.day"],
            json!("2022-01-01T00:00:00.000Z")
        );
    }
}
