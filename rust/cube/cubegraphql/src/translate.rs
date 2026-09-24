//! GraphQL document -> Cube REST query.
//!
//! A line-by-line port of `getJsonQuery`, `getJsonQueryFromGraphQLQuery`,
//! `whereArgToQueryFilters`, `mapWhereOperator` and `mapWhereValue` from
//! `packages/cubejs-api-gateway/src/graphql.ts`.

use serde_json::{json, Map, Value};
use std::ops::Deref;

use crate::ast::{is_truthy, parse_root_field, FieldNode};
use crate::error::GraphQLError;
use crate::meta::{MemberType, MetaConfig};
use crate::naming::capitalize;

/// The Cube REST query (`{ measures, dimensions, timeDimensions, ... }`) that
/// `POST {basePath}/v1/graphql-to-json` returns under `jsonQuery`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct CubeQueryJson(pub Value);

impl CubeQueryJson {
    /// The query as raw JSON.
    pub fn as_value(&self) -> &Value {
        &self.0
    }

    /// Consume into raw JSON.
    pub fn into_value(self) -> Value {
        self.0
    }

    /// `query.cache`, which the gateway forwards to `load()`.
    pub fn cache(&self) -> Option<&str> {
        self.0.get("cache").and_then(Value::as_str)
    }

    /// `query.timezone`, used when post-processing time values.
    pub fn timezone(&self) -> Option<&str> {
        self.0.get("timezone").and_then(Value::as_str)
    }
}

impl Deref for CubeQueryJson {
    type Target = Value;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<CubeQueryJson> for Value {
    fn from(value: CubeQueryJson) -> Self {
        value.0
    }
}

/// `getJsonQueryFromGraphQLQuery(query, metaConfig, variableValues)`.
///
/// This is the whole of `POST {basePath}/v1/graphql-to-json`: parse the
/// document, walk the `cube` field and produce the equivalent REST query.
pub fn graphql_to_json(
    query: &str,
    variables: Option<&Value>,
    meta: &MetaConfig,
) -> Result<CubeQueryJson, GraphQLError> {
    graphql_to_json_with_operation(query, variables, meta, None)
}

/// As [`graphql_to_json`], but picks a named operation out of a multi-operation
/// document.
pub fn graphql_to_json_with_operation(
    query: &str,
    variables: Option<&Value>,
    meta: &MetaConfig,
    operation_name: Option<&str>,
) -> Result<CubeQueryJson, GraphQLError> {
    let empty = Map::new();
    let variables = match variables {
        Some(Value::Object(obj)) => obj,
        Some(Value::Null) | None => &empty,
        Some(_) => {
            return Err(GraphQLError::Translate(
                "GraphQL variables must be an object".to_string(),
            ))
        }
    };

    let root = parse_root_field(query, variables, operation_name)?;

    Ok(get_json_query(meta, &root.arguments, &root))
}

/// `getJsonQuery(metaConfig, args, infos)`.
///
/// `args` are the arguments of the root `cube` field and `root` is that same
/// field's selection tree.
pub fn get_json_query(
    meta: &MetaConfig,
    args: &Map<String, Value>,
    root: &FieldNode,
) -> CubeQueryJson {
    let mut measures: Vec<Value> = Vec::new();
    let mut dimensions: Vec<Value> = Vec::new();
    let mut time_dimensions: Vec<Value> = Vec::new();
    let mut filters: Vec<Value> = Vec::new();
    let mut order: Vec<Value> = Vec::new();

    if let Some(where_arg) = arg(args, "where") {
        filters = where_arg_to_query_filters(where_arg, None, meta);
    }

    if let Some(Value::Object(order_by)) = arg(args, "orderBy") {
        for (cube_name, members) in order_by {
            if let Value::Object(members) = members {
                // Diverges from `graphql.ts:381`, which capitalizes
                // unconditionally here while the per-cube branch below looks
                // the name up first. A cube named `orders` — the spelling
                // Cube's own YAML models use — therefore became `Orders`, and
                // the query failed with "Cannot resolve: Orders". Both paths
                // resolve the name the same way now.
                let resolved = resolve_cube_name(cube_name, meta);
                for (member, value) in members {
                    order.push(json!([format!("{resolved}.{member}"), value]));
                }
            }
        }
    }

    for cube_node in &root.children {
        let cube_name = resolve_cube_name(&cube_node.name, meta);

        if let Some(Value::Object(order_by)) = cube_node.argument("orderBy") {
            for (key, value) in order_by {
                order.push(json!([format!("{cube_name}.{key}"), value]));
            }
        }

        if let Some(where_arg) = cube_node.argument("where") {
            // `graphql.ts` calls `whereArgToQueryFilters(whereArg, cubeName)`
            // without a meta config here, so the nested cube-name lookup always
            // falls back to `capitalize`.
            let mut cube_filters =
                where_arg_to_query_filters(where_arg, Some(&cube_name), &MetaConfig::default());
            cube_filters.append(&mut filters);
            filters = cube_filters;
        }

        // Push down all `inDateRange` filters to time dimensions to leverage
        // pre-aggregations.
        let mut date_range_filters: Map<String, Value> = Map::new();
        filters.retain(|f| {
            let is_in_date_range = f.get("operator").and_then(Value::as_str) == Some("inDateRange");
            let member = f.get("member").and_then(Value::as_str);

            match (is_in_date_range, member) {
                (true, Some(member)) if !date_range_filters.contains_key(member) => {
                    date_range_filters.insert(
                        member.to_string(),
                        f.get("values").cloned().unwrap_or(Value::Null),
                    );
                    false
                }
                _ => true,
            }
        });

        for member_node in &cube_node.children {
            let member_name = &member_node.name;
            let member_type = meta.member_type(&cube_name, member_name);
            let key = format!("{cube_name}.{member_name}");

            match member_type {
                Some(MemberType::Measures) => measures.push(Value::String(key)),
                Some(MemberType::Dimensions) => {
                    if member_node.children.is_empty() {
                        dimensions.push(Value::String(key));
                    } else {
                        for granularity_node in &member_node.children {
                            if granularity_node.name == "value" {
                                dimensions.push(Value::String(key.clone()));
                            } else {
                                let mut td = Map::new();
                                td.insert("dimension".to_string(), Value::String(key.clone()));
                                td.insert(
                                    "granularity".to_string(),
                                    Value::String(granularity_node.name.clone()),
                                );
                                if let Some(date_range) =
                                    date_range_filters.get(&key).filter(|v| is_truthy(v))
                                {
                                    td.insert("dateRange".to_string(), date_range.clone());
                                }
                                time_dimensions.push(Value::Object(td));
                            }
                        }
                    }
                }
                None => {}
            }
        }

        if !date_range_filters.is_empty() && time_dimensions.is_empty() {
            for (dimension, date_range) in &date_range_filters {
                time_dimensions.push(json!({
                    "dimension": dimension,
                    "dateRange": date_range,
                }));
            }
        }
    }

    let mut query = Map::new();
    insert_if(&mut query, "measures", Value::Array(measures));
    insert_if(&mut query, "dimensions", Value::Array(dimensions));
    insert_if(&mut query, "timeDimensions", Value::Array(time_dimensions));
    insert_if(&mut query, "order", Value::Array(order));
    if let Some(limit) = arg(args, "limit").filter(|v| is_truthy(v)) {
        query.insert("limit".to_string(), limit.clone());
    }
    if let Some(offset) = arg(args, "offset").filter(|v| is_truthy(v)) {
        query.insert("offset".to_string(), offset.clone());
    }
    if let Some(timezone) = arg(args, "timezone").filter(|v| is_truthy(v)) {
        query.insert("timezone".to_string(), timezone.clone());
    }
    insert_if(&mut query, "filters", Value::Array(filters));
    if let Some(cache) = arg(args, "cache").filter(|v| is_truthy(v)) {
        query.insert("cache".to_string(), cache.clone());
    }
    if let Some(ungrouped) = arg(args, "ungrouped").filter(|v| is_truthy(v)) {
        query.insert("ungrouped".to_string(), ungrouped.clone());
    }

    CubeQueryJson(Value::Object(query))
}

/// The cube name as the data model spells it.
///
/// A GraphQL field is `unCapitalize`d, so `Orders` arrives as `orders`. A
/// model that names its cube `orders` is matched as it is; one that names it
/// `Orders` is matched by capitalizing, which is what `graphql.ts` does on its
/// per-cube path.
fn resolve_cube_name(name: &str, meta: &MetaConfig) -> String {
    if meta.find_exact(name).is_some() {
        name.to_string()
    } else {
        capitalize(name)
    }
}

fn arg<'a>(args: &'a Map<String, Value>, name: &str) -> Option<&'a Value> {
    args.get(name).filter(|v| !v.is_null())
}

fn insert_if(query: &mut Map<String, Value>, key: &str, value: Value) {
    let keep = match &value {
        Value::Array(items) => !items.is_empty(),
        _ => true,
    };
    if keep {
        query.insert(key.to_string(), value);
    }
}

/// `whereArgToQueryFilters(whereArg, prefix, metaConfig)`.
pub fn where_arg_to_query_filters(
    where_arg: &Value,
    prefix: Option<&str>,
    meta: &MetaConfig,
) -> Vec<Value> {
    let mut query_filters: Vec<Value> = Vec::new();

    let Some(obj) = where_arg.as_object() else {
        return query_filters;
    };

    for (key, value) in obj {
        let cube_exists = meta.find_exact(key).is_some();
        let normalized_key = if cube_exists {
            key.clone()
        } else {
            capitalize(key)
        };

        if key == "OR" || key == "AND" {
            let mut filters: Vec<Value> = Vec::new();
            match value {
                Value::Array(items) => {
                    for item in items {
                        filters.extend(where_arg_to_query_filters(item, prefix, meta));
                    }
                }
                other => filters.extend(where_arg_to_query_filters(other, prefix, meta)),
            }
            let mut boolean_filter = Map::new();
            boolean_filter.insert(key.to_lowercase(), Value::Array(filters));
            query_filters.push(Value::Object(boolean_filter));
        } else if value.get("OR").is_some() || value.get("AND").is_some() {
            // users: {
            //   OR: { name: { equals: "Alex" } country: { equals: "US" } }
            //   age: { equals: 28 } # <-- will require AND
            // }
            let inner = value.as_object().expect("`get` succeeded, so an object");
            if inner.len() > 1 {
                let and: Vec<Value> = inner
                    .iter()
                    .map(|(k, v)| {
                        let mut entry = Map::new();
                        entry.insert(k.clone(), v.clone());
                        Value::Object(entry)
                    })
                    .collect();
                let mut wrapper = Map::new();
                wrapper.insert("AND".to_string(), Value::Array(and));
                query_filters.extend(where_arg_to_query_filters(
                    &Value::Object(wrapper),
                    Some(&normalized_key),
                    meta,
                ));
            } else {
                query_filters.extend(where_arg_to_query_filters(
                    value,
                    Some(&normalized_key),
                    meta,
                ));
            }
        } else if let Some(prefix) = prefix {
            // A subfilter: `{ country: { in: ["US"] } }`.
            if let Some(operators) = value.as_object() {
                for (operator, operand) in operators {
                    query_filters.push(filter(&format!("{prefix}.{key}"), operator, operand));
                }
            }
        } else if let Some(members) = value.as_object() {
            for (member, filters) in members {
                if let Some(operators) = filters.as_object() {
                    for (operator, operand) in operators {
                        query_filters.push(filter(
                            &format!("{normalized_key}.{member}"),
                            operator,
                            operand,
                        ));
                    }
                }
            }
        }
    }

    query_filters
}

fn filter(member: &str, operator: &str, value: &Value) -> Value {
    let mut out = Map::new();
    out.insert("member".to_string(), Value::String(member.to_string()));
    out.insert(
        "operator".to_string(),
        Value::String(map_where_operator(operator, value)),
    );
    // `...(mapWhereValue(...) && { values })` — a falsy result adds no `values`.
    if let Some(values) = map_where_value(operator, value).filter(is_truthy) {
        out.insert("values".to_string(), values);
    }
    Value::Object(out)
}

/// `mapWhereOperator(operator, value)`.
pub fn map_where_operator(operator: &str, value: &Value) -> String {
    match operator {
        "in" => "equals".to_string(),
        "notIn" => "notEquals".to_string(),
        "set" => {
            if value == &Value::Bool(true) {
                "set".to_string()
            } else {
                "notSet".to_string()
            }
        }
        other => other.to_string(),
    }
}

/// `mapWhereValue(operator, value)`.
///
/// Returns `None` for `set` (which carries no values), a bare string for a
/// single relative date range such as `"This month"`, and an array of
/// stringified values otherwise.
pub fn map_where_value(operator: &str, value: &Value) -> Option<Value> {
    let values: Vec<String> = match value {
        Value::Array(items) => items.iter().map(js_to_string).collect(),
        other => vec![js_to_string(other)],
    };

    if operator == "set" {
        return None;
    }

    if (operator == "inDateRange" || operator == "notInDateRange")
        && values.len() == 1
        && !is_iso_date_prefix(&values[0])
    {
        return Some(Value::String(values[0].clone()));
    }

    Some(Value::Array(
        values.into_iter().map(Value::String).collect(),
    ))
}

/// `/^\d\d\d\d-\d\d-\d\d/`.
fn is_iso_date_prefix(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 10
        && bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
}

/// JavaScript's `String(value)` for the JSON values a GraphQL argument can hold.
pub(crate) fn js_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => {
            // `(10).toString()` is `"10"`, not `"10.0"`.
            match n.as_f64() {
                Some(f) if f.fract() == 0.0 && f.is_finite() && f.abs() < 1e21 => {
                    format!("{}", f as i64)
                }
                _ => n.to_string(),
            }
        }
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn meta() -> MetaConfig {
        MetaConfig::from_value(&json!({
            "cubes": [
                {
                    "name": "Orders",
                    "measures": [{ "name": "Orders.count", "isVisible": true }],
                    "dimensions": [
                        { "name": "Orders.status", "type": "string", "isVisible": true },
                        { "name": "Orders.createdAt", "type": "time", "isVisible": true }
                    ]
                },
                {
                    "name": "Users",
                    "measures": [],
                    "dimensions": [{ "name": "Users.city", "type": "string", "isVisible": true }]
                }
            ]
        }))
        .unwrap()
    }

    #[test]
    fn maps_operator_names() {
        assert_eq!(map_where_operator("in", &json!(["a"])), "equals");
        assert_eq!(map_where_operator("notIn", &json!(["a"])), "notEquals");
        assert_eq!(map_where_operator("set", &json!(true)), "set");
        assert_eq!(map_where_operator("set", &json!(false)), "notSet");
        assert_eq!(map_where_operator("gte", &json!(1)), "gte");
    }

    #[test]
    fn maps_values() {
        assert_eq!(map_where_value("set", &json!(true)), None);
        assert_eq!(map_where_value("equals", &json!("a")), Some(json!(["a"])));
        assert_eq!(map_where_value("equals", &json!(28)), Some(json!(["28"])));
        assert_eq!(
            map_where_value("inDateRange", &json!("This month")),
            Some(json!("This month"))
        );
        assert_eq!(
            map_where_value("inDateRange", &json!(["2022-01-01", "2022-02-01"])),
            Some(json!(["2022-01-01", "2022-02-01"]))
        );
        // A single ISO date stays an array.
        assert_eq!(
            map_where_value("inDateRange", &json!(["2022-01-01"])),
            Some(json!(["2022-01-01"]))
        );
    }

    #[test]
    fn root_where_produces_prefixed_members() {
        let filters = where_arg_to_query_filters(
            &json!({ "orders": { "status": { "equals": "shipped" } } }),
            None,
            &meta(),
        );

        assert_eq!(
            filters,
            vec![json!({
                "member": "Orders.status",
                "operator": "equals",
                "values": ["shipped"]
            })]
        );
    }

    #[test]
    fn keeps_a_cube_name_that_exists_verbatim() {
        let meta = MetaConfig::from_value(&json!({
            "cubes": [{
                "name": "orders",
                "measures": [],
                "dimensions": [{ "name": "orders.status", "type": "string", "isVisible": true }]
            }]
        }))
        .unwrap();

        let filters = where_arg_to_query_filters(
            &json!({ "orders": { "status": { "equals": "shipped" } } }),
            None,
            &meta,
        );

        assert_eq!(filters[0]["member"], json!("orders.status"));
    }

    #[test]
    fn nested_boolean_filters() {
        let filters = where_arg_to_query_filters(
            &json!({
                "OR": [
                    { "users": { "OR": [
                        { "city": { "set": false } },
                        { "city": { "equals": "US" } }
                    ] } }
                ]
            }),
            None,
            &meta(),
        );

        assert_eq!(
            filters,
            vec![json!({
                "or": [{
                    "or": [
                        { "member": "Users.city", "operator": "notSet" },
                        { "member": "Users.city", "operator": "equals", "values": ["US"] }
                    ]
                }]
            })]
        );
    }

    #[test]
    fn a_single_boolean_filter_beside_a_plain_one_becomes_an_and() {
        let filters = where_arg_to_query_filters(
            &json!({
                "users": {
                    "OR": [{ "city": { "equals": "US" } }],
                    "id": { "equals": 28 }
                }
            }),
            None,
            &meta(),
        );

        assert_eq!(
            filters,
            vec![json!({
                "and": [
                    { "or": [{ "member": "Users.city", "operator": "equals", "values": ["US"] }] },
                    { "member": "Users.id", "operator": "equals", "values": ["28"] }
                ]
            })]
        );
    }

    #[test]
    fn translates_limit_offset_timezone_and_flags() {
        let query = graphql_to_json(
            r#"query CubeQuery {
                cube(limit: 10, offset: 5, timezone: "UTC", ungrouped: true, cache: "no-cache") {
                    orders { count }
                }
            }"#,
            None,
            &meta(),
        )
        .unwrap();

        assert_eq!(
            query.into_value(),
            json!({
                "measures": ["Orders.count"],
                "limit": 10,
                "offset": 5,
                "timezone": "UTC",
                "cache": "no-cache",
                "ungrouped": true
            })
        );
    }

    #[test]
    fn falsy_limit_is_dropped() {
        let query = graphql_to_json(
            "query CubeQuery { cube(limit: 0, ungrouped: false) { orders { count } } }",
            None,
            &meta(),
        )
        .unwrap();

        assert_eq!(query.into_value(), json!({ "measures": ["Orders.count"] }));
    }

    /// A model whose cubes are named in lower snake case, which is what
    /// Cube's YAML models use.
    fn snake_case_meta() -> MetaConfig {
        MetaConfig::from_value(&json!({
            "cubes": [
                {
                    "name": "line_items",
                    "measures": [{ "name": "line_items.revenue", "isVisible": true }],
                    "dimensions": [
                        { "name": "line_items.quantity", "type": "number", "isVisible": true }
                    ]
                }
            ]
        }))
        .unwrap()
    }

    #[test]
    fn root_order_by_names_the_cube_as_the_model_spells_it() {
        // `graphql.ts:381` capitalizes unconditionally, so a cube named
        // `line_items` became `Line_items` and the query failed to resolve.
        let query = graphql_to_json(
            "query CubeQuery { cube(orderBy: { line_items: { revenue: desc } }) \
             { line_items { revenue } } }",
            None,
            &snake_case_meta(),
        )
        .unwrap();

        assert_eq!(query["order"], json!([["line_items.revenue", "desc"]]));
        assert_eq!(query["measures"], json!(["line_items.revenue"]));
    }

    #[test]
    fn the_root_and_per_cube_order_by_agree() {
        let meta = snake_case_meta();

        let root = graphql_to_json(
            "query CubeQuery { cube(orderBy: { line_items: { revenue: desc } }) \
             { line_items { revenue } } }",
            None,
            &meta,
        )
        .unwrap();
        let per_cube = graphql_to_json(
            "query CubeQuery { cube { line_items(orderBy: { revenue: desc }) { revenue } } }",
            None,
            &meta,
        )
        .unwrap();

        assert_eq!(root["order"], per_cube["order"]);
    }

    #[test]
    fn root_order_by() {
        let query = graphql_to_json(
            "query CubeQuery { cube(orderBy: { orders: { count: desc } }) { orders { count } } }",
            None,
            &meta(),
        )
        .unwrap();

        assert_eq!(query["order"], json!([["Orders.count", "desc"]]));
    }

    #[test]
    fn variables_are_substituted() {
        let query = graphql_to_json(
            r#"query CubeQuery($status: String!) {
                cube(where: { orders: { status: { equals: $status } } }) {
                    orders { count }
                }
            }"#,
            Some(&json!({ "status": "shipped" })),
            &meta(),
        )
        .unwrap();

        assert_eq!(
            query["filters"],
            json!([{ "member": "Orders.status", "operator": "equals", "values": ["shipped"] }])
        );
    }

    #[test]
    fn unknown_members_are_ignored() {
        let query = graphql_to_json(
            "query CubeQuery { cube { orders { count nope } } }",
            None,
            &meta(),
        )
        .unwrap();

        assert_eq!(query.into_value(), json!({ "measures": ["Orders.count"] }));
    }

    #[test]
    fn per_cube_where_is_prefixed_with_the_cube_name() {
        let query = graphql_to_json(
            r#"query CubeQuery {
                cube { orders(where: { status: { equals: "shipped" } }) { count } }
            }"#,
            None,
            &meta(),
        )
        .unwrap();

        assert_eq!(
            query["filters"],
            json!([{ "member": "Orders.status", "operator": "equals", "values": ["shipped"] }])
        );
    }
}
