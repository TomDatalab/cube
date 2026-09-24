//! The `/v1/load` response, shared by the REST and WebSocket transports:
//! `ApiGateway.load` after the queries are normalized.
//!
//! * every normalized query runs (a `compareDateRange` or blending request
//!   has several), each annotated from the meta config (`prepareAnnotation`);
//! * `queryType=multi`, which `@cubejs-client/core` always sends, answers
//!   `{ queryType, results, pivotQuery, slowQuery }` (`ResultMultiWrapper`);
//!   without it the first result is returned as is, and a query type other
//!   than `regularQuery` is refused, as older clients cannot read it.

use futures::future::try_join_all;
use serde_json::{json, Map, Value};

use crate::app::AppState;
use crate::error::ApiError;
use crate::services::{NormalizedRequest, RequestContext};
use cubequery::QueryType;

/// `TIME_SERIES` keys in `@cubejs-backend/shared`.
const PREDEFINED_GRANULARITIES: &[&str] = &[
    "second", "minute", "hour", "day", "week", "month", "quarter", "year",
];

/// `ApiGateway.load` from the normalized request on. `query_type_param` is
/// the request's `queryType` (`'multi'` for current clients).
pub async fn load(
    state: &AppState,
    ctx: &RequestContext,
    request: NormalizedRequest,
    query_type_param: Option<&str>,
) -> Result<Value, ApiError> {
    if request.query_type != QueryType::RegularQuery && query_type_param.is_none() {
        return Err(ApiError::bad_request(format!(
            "'{}' query type is not supported by the client.Please update the client.",
            request.query_type.as_str()
        )));
    }

    let service = state.query_for(ctx).await?;
    // The same visibility-filtered meta config `/v1/meta` serves, as
    // `filterVisibleItemsInMeta` does in Node.js. A model that failed to load
    // cannot plan either, so an empty annotation is never actually returned.
    let meta = match state.meta_for(ctx).await {
        Ok(meta) => meta.meta(ctx, false).await.unwrap_or(Value::Null),
        Err(_) => Value::Null,
    };

    let NormalizedRequest {
        query_type,
        queries,
        pivot_query,
    } = request;

    let mut results = try_join_all(queries.into_iter().map(|query| {
        let service = service.clone();
        let single = NormalizedRequest {
            query_type,
            queries: vec![query],
            pivot_query: pivot_query.clone(),
        };
        async move { service.load(ctx, single).await }
    }))
    .await?;

    // `Continue wait` (or any body without a result) answers the whole
    // request, so the client polls again.
    if let Some(pending) = results.iter().find(|r| r.get("error").is_some()) {
        return Ok(pending.clone());
    }

    for result in results.iter_mut() {
        if let Some(object) = result.as_object_mut() {
            let annotation = object
                .get("query")
                .map(|q| prepare_annotation(&meta, q))
                .unwrap_or_else(empty_annotation);
            object.insert("annotation".to_string(), annotation);
        }
    }

    if query_type_param == Some("multi") {
        let slow_query = results
            .iter()
            .any(|r| r.get("slowQuery").and_then(Value::as_bool) == Some(true));
        return Ok(json!({
            "queryType": query_type.as_str(),
            "results": results,
            "pivotQuery": pivot_query,
            "slowQuery": slow_query,
        }));
    }

    Ok(results.into_iter().next().unwrap_or(Value::Null))
}

fn empty_annotation() -> Value {
    json!({ "measures": {}, "dimensions": {}, "segments": {}, "timeDimensions": {} })
}

/// `prepareAnnotation(metaConfig, query)`: titles, types and formats of the
/// members the query uses, keyed as the query names them.
pub fn prepare_annotation(meta: &Value, query: &Value) -> Value {
    let members = |key: &str| -> Vec<Value> {
        query
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };

    let mut measures = Map::new();
    for member in members("measures") {
        if let Some((key, item)) = annotation(meta, MemberType::Measures, &member) {
            measures.insert(key, item);
        }
    }

    let dimension_list = members("dimensions");
    let mut dimensions = Map::new();
    for member in &dimension_list {
        if let Some((key, item)) = annotation(meta, MemberType::Dimensions, member) {
            dimensions.insert(key, item);
        }
    }

    let mut segments = Map::new();
    for member in members("segments") {
        if let Some((key, item)) = annotation(meta, MemberType::Segments, &member) {
            segments.insert(key, item);
        }
    }

    let mut time_dimensions = Map::new();
    for td in members("timeDimensions") {
        let (Some(dimension), Some(granularity)) = (
            td.get("dimension").and_then(Value::as_str),
            td.get("granularity").and_then(Value::as_str),
        ) else {
            continue;
        };

        let with_granularity = Value::String(format!("{dimension}.{granularity}"));
        if let Some((key, mut item)) =
            annotation(meta, MemberType::Dimensions, &with_granularity)
        {
            let granularity_meta = if PREDEFINED_GRANULARITIES.contains(&granularity) {
                json!({
                    "name": granularity,
                    "title": granularity,
                    "interval": format!("1 {granularity}"),
                })
            } else {
                item.get("granularities")
                    .and_then(Value::as_array)
                    .and_then(|all| {
                        all.iter()
                            .find(|g| g.get("name").and_then(Value::as_str) == Some(granularity))
                    })
                    .cloned()
                    .unwrap_or(Value::Null)
            };
            if let Some(object) = item.as_object_mut() {
                object.remove("granularities");
                if !granularity_meta.is_null() {
                    object.insert("granularity".to_string(), granularity_meta);
                }
            }
            time_dimensions.insert(key, item);
        }

        // Deprecated in Node.js but kept: the time dimension without its
        // granularity, unless it is also a plain dimension of the query.
        if dimension_list
            .iter()
            .any(|d| d.as_str() == Some(dimension))
        {
            continue;
        }
        if let Some((key, mut item)) = annotation(
            meta,
            MemberType::Dimensions,
            &Value::String(dimension.to_string()),
        ) {
            if let Some(object) = item.as_object_mut() {
                object.remove("granularities");
            }
            time_dimensions.insert(key, item);
        }
    }

    json!({
        "measures": measures,
        "dimensions": dimensions,
        "segments": segments,
        "timeDimensions": time_dimensions,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MemberType {
    Measures,
    Dimensions,
    Segments,
}

impl MemberType {
    fn key(self) -> &'static str {
        match self {
            MemberType::Measures => "measures",
            MemberType::Dimensions => "dimensions",
            MemberType::Segments => "segments",
        }
    }
}

/// `annotation(configMap, memberType)(member)`.
fn annotation(meta: &Value, member_type: MemberType, member: &Value) -> Option<(String, Value)> {
    // A member expression is annotated under `cube.name`.
    let (cube_name, field_name, key) = match member {
        Value::String(name) => {
            let mut parts = name.split('.');
            let cube = parts.next()?.to_string();
            let field = parts.next()?.to_string();
            (cube, field, name.clone())
        }
        Value::Object(expression) => {
            let cube = expression.get("cubeName")?.as_str()?.to_string();
            let field = expression.get("name")?.as_str()?.to_string();
            let key = format!("{cube}.{field}");
            (cube, field, key)
        }
        _ => return None,
    };
    let full_name = format!("{cube_name}.{field_name}");

    let config = meta
        .get("cubes")?
        .as_array()?
        .iter()
        .find(|cube| cube.get("name").and_then(Value::as_str) == Some(cube_name.as_str()))?
        .get(member_type.key())?
        .as_array()?
        .iter()
        .find(|m| m.get("name").and_then(Value::as_str) == Some(full_name.as_str()))?;

    let mut item = Map::new();
    let mut copy = |field: &str| {
        if let Some(value) = config.get(field) {
            item.insert(field.to_string(), value.clone());
        }
    };
    for field in [
        "title",
        "shortTitle",
        "description",
        "type",
        "format",
        "currency",
        "meta",
    ] {
        copy(field);
    }
    match member_type {
        MemberType::Measures => {
            copy("drillMembers");
            copy("drillMembersGrouped");
        }
        MemberType::Dimensions => copy("granularities"),
        MemberType::Segments => {}
    }

    Some((key, Value::Object(item)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> Value {
        json!({ "cubes": [{
            "name": "orders",
            "measures": [{
                "name": "orders.count", "title": "Orders Count", "shortTitle": "Count",
                "type": "number", "drillMembers": ["orders.id"],
                "drillMembersGrouped": { "measures": [], "dimensions": ["orders.id"] },
                "aggType": "count"
            }],
            "dimensions": [
                { "name": "orders.status", "title": "Orders Status", "shortTitle": "Status", "type": "string" },
                { "name": "orders.created_at", "title": "Orders Created at", "shortTitle": "Created at",
                  "type": "time",
                  "granularities": [{ "name": "fiscal", "title": "Fiscal", "interval": "1 year", "offset": "3 months" }] }
            ],
            "segments": [{ "name": "orders.done", "title": "Orders Done", "shortTitle": "Done" }]
        }]})
    }

    #[test]
    fn annotates_every_member_kind() {
        let annotation = prepare_annotation(
            &meta(),
            &json!({
                "measures": ["orders.count"],
                "dimensions": ["orders.status"],
                "segments": ["orders.done"],
                "timeDimensions": [{ "dimension": "orders.created_at", "granularity": "month" }]
            }),
        );

        assert_eq!(
            annotation["measures"]["orders.count"],
            json!({
                "title": "Orders Count", "shortTitle": "Count", "type": "number",
                "drillMembers": ["orders.id"],
                "drillMembersGrouped": { "measures": [], "dimensions": ["orders.id"] }
            })
        );
        assert_eq!(
            annotation["dimensions"]["orders.status"]["title"],
            json!("Orders Status")
        );
        assert_eq!(annotation["segments"]["orders.done"]["shortTitle"], json!("Done"));
        assert_eq!(
            annotation["timeDimensions"]["orders.created_at.month"]["granularity"],
            json!({ "name": "month", "title": "month", "interval": "1 month" })
        );
        // the deprecated entry without granularity, and no granularity list
        let plain = &annotation["timeDimensions"]["orders.created_at"];
        assert_eq!(plain["type"], json!("time"));
        assert!(plain.get("granularities").is_none());
    }

    #[test]
    fn custom_granularities_come_from_the_meta() {
        let annotation = prepare_annotation(
            &meta(),
            &json!({
                "dimensions": ["orders.created_at"],
                "timeDimensions": [{ "dimension": "orders.created_at", "granularity": "fiscal" }]
            }),
        );
        assert_eq!(
            annotation["timeDimensions"]["orders.created_at.fiscal"]["granularity"]["offset"],
            json!("3 months")
        );
        // also a plain dimension: no extra deprecated entry
        assert!(annotation["timeDimensions"].get("orders.created_at").is_none());
    }

    #[test]
    fn unknown_members_are_left_out() {
        let annotation = prepare_annotation(
            &meta(),
            &json!({ "measures": ["orders.nope", "missing.count"] }),
        );
        assert_eq!(annotation["measures"], json!({}));
    }
}
