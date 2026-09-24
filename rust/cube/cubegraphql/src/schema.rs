//! The runtime-built GraphQL schema — `makeSchema(metaConfig)`.
//!
//! Cube's schema depends entirely on the data model, so it is assembled with
//! `async_graphql::dynamic` rather than derived from Rust types.

use async_graphql::dynamic::{
    Enum, Field, FieldFuture, FieldValue, InputObject, InputValue, Object, Scalar, Schema,
    SchemaBuilder, TypeRef,
};
use async_graphql::SelectionField;
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::sync::Arc;

use crate::ast::{apply_directives, const_value_to_json, FieldNode};
use crate::error::GraphQLError;
use crate::executor::{CubeQueryExecutor, CubeQueryRequest, ResponseExtensions};
use crate::meta::{CubeMeta, MemberMeta, MetaConfig};
use crate::naming::{object_name, un_capitalize};
use crate::response::{parse_dates, shape_rows};
use crate::translate::get_json_query;

/// The `DateTime` scalar (`graphql-scalars`' `DateTimeResolver` in the Node.js
/// gateway).
pub const DATE_TIME: &str = "DateTime";
/// The root query field.
pub const CUBE_FIELD: &str = "cube";
/// The per-row object type.
pub const RESULT_TYPE: &str = "Result";

const GRANULARITIES: [&str; 9] = [
    "value", "second", "minute", "hour", "day", "week", "month", "quarter", "year",
];

/// Build the schema from a `{ "cubes": [...] }` meta config.
pub fn make_schema_from_value(meta: &Value) -> Result<Schema, GraphQLError> {
    make_schema(&MetaConfig::from_value(meta)?)
}

/// `makeSchema(metaConfig)`.
pub fn make_schema(meta: &MetaConfig) -> Result<Schema, GraphQLError> {
    let meta = Arc::new(meta.clone());

    let cubes: Vec<&CubeMeta> = {
        let mut seen = HashSet::new();
        meta.schema_cubes()
            .filter(|cube| {
                let key = object_name(&cube.name);
                is_valid_name(&key) && seen.insert(key)
            })
            .collect()
    };

    if cubes.is_empty() {
        return Err(GraphQLError::Schema(
            "Cannot build a GraphQL schema: the data model exposes no cubes with visible members"
                .to_string(),
        ));
    }

    let mut builder: SchemaBuilder = Schema::build("Query", None, None)
        .register(date_time_scalar())
        .register(float_filter())
        .register(string_filter())
        .register(date_time_filter())
        .register(order_by_enum())
        .register(time_dimension_type());

    for cube in &cubes {
        builder = builder
            .register(members_type(cube))
            .register(where_input(cube))
            .register(order_by_input(cube));
    }

    builder = builder
        .register(root_where_input(&cubes))
        .register(root_order_by_input(&cubes))
        .register(result_type(&cubes))
        .register(query_type(Arc::clone(&meta)));

    builder
        .finish()
        .map_err(|e| GraphQLError::Schema(e.to_string()))
}

fn date_time_scalar() -> Scalar {
    Scalar::new(DATE_TIME).description(
        "A date-time string at UTC, such as 2007-12-03T10:15:30Z, \
         compliant with the date-time format outlined in section 5.6 of the RFC 3339 profile \
         of the ISO 8601 standard for representation of dates and times using the Gregorian calendar.",
    )
}

fn float_filter() -> InputObject {
    InputObject::new("FloatFilter")
        .field(InputValue::new("equals", TypeRef::named(TypeRef::FLOAT)))
        .field(InputValue::new("notEquals", TypeRef::named(TypeRef::FLOAT)))
        .field(InputValue::new("in", TypeRef::named_list(TypeRef::FLOAT)))
        .field(InputValue::new(
            "notIn",
            TypeRef::named_list(TypeRef::FLOAT),
        ))
        .field(InputValue::new("set", TypeRef::named(TypeRef::BOOLEAN)))
        .field(InputValue::new("gt", TypeRef::named(TypeRef::FLOAT)))
        .field(InputValue::new("lt", TypeRef::named(TypeRef::FLOAT)))
        .field(InputValue::new("gte", TypeRef::named(TypeRef::FLOAT)))
        .field(InputValue::new("lte", TypeRef::named(TypeRef::FLOAT)))
}

fn string_filter() -> InputObject {
    InputObject::new("StringFilter")
        .field(InputValue::new("equals", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new(
            "notEquals",
            TypeRef::named(TypeRef::STRING),
        ))
        .field(InputValue::new("in", TypeRef::named_list(TypeRef::STRING)))
        .field(InputValue::new(
            "notIn",
            TypeRef::named_list(TypeRef::STRING),
        ))
        .field(InputValue::new(
            "contains",
            TypeRef::named_list(TypeRef::STRING),
        ))
        // `graphql.ts` declares `notContains` twice; nexus keeps one field.
        .field(InputValue::new(
            "notContains",
            TypeRef::named_list(TypeRef::STRING),
        ))
        .field(InputValue::new(
            "startsWith",
            TypeRef::named_list(TypeRef::STRING),
        ))
        .field(InputValue::new(
            "notStartsWith",
            TypeRef::named_list(TypeRef::STRING),
        ))
        .field(InputValue::new(
            "endsWith",
            TypeRef::named_list(TypeRef::STRING),
        ))
        .field(InputValue::new(
            "notEndsWith",
            TypeRef::named_list(TypeRef::STRING),
        ))
        .field(InputValue::new("set", TypeRef::named(TypeRef::BOOLEAN)))
}

fn date_time_filter() -> InputObject {
    InputObject::new("DateTimeFilter")
        .field(InputValue::new(
            "equals",
            TypeRef::named_list(TypeRef::STRING),
        ))
        .field(InputValue::new(
            "notEquals",
            TypeRef::named_list(TypeRef::STRING),
        ))
        .field(InputValue::new("in", TypeRef::named_list(TypeRef::STRING)))
        .field(InputValue::new(
            "notIn",
            TypeRef::named_list(TypeRef::STRING),
        ))
        .field(InputValue::new(
            "inDateRange",
            TypeRef::named_list(TypeRef::STRING),
        ))
        .field(InputValue::new(
            "notInDateRange",
            TypeRef::named_list(TypeRef::STRING),
        ))
        .field(InputValue::new(
            "beforeDate",
            TypeRef::named(TypeRef::STRING),
        ))
        .field(InputValue::new(
            "beforeOrOnDate",
            TypeRef::named(TypeRef::STRING),
        ))
        .field(InputValue::new(
            "afterDate",
            TypeRef::named(TypeRef::STRING),
        ))
        .field(InputValue::new(
            "afterOrOnDate",
            TypeRef::named(TypeRef::STRING),
        ))
        .field(InputValue::new("set", TypeRef::named(TypeRef::BOOLEAN)))
}

fn order_by_enum() -> Enum {
    Enum::new("OrderBy").items(["asc", "desc"])
}

fn time_dimension_type() -> Object {
    GRANULARITIES
        .into_iter()
        .fold(Object::new("TimeDimension"), |object, granularity| {
            object.field(json_scalar_field(
                granularity,
                TypeRef::named_nn(DATE_TIME),
                DATE_TIME,
            ))
        })
}

/// `mapType(type, isInputType)`.
pub fn map_type(member_type: Option<&str>, is_input_type: bool) -> &'static str {
    match member_type {
        Some("time") => {
            if is_input_type {
                DATE_TIME
            } else {
                "TimeDimension"
            }
        }
        Some("string") => TypeRef::STRING,
        Some("number") => TypeRef::FLOAT,
        _ => TypeRef::STRING,
    }
}

fn members_type_name(cube: &CubeMeta) -> String {
    format!("{}Members", object_name(&cube.name))
}

fn where_input_name(cube: &CubeMeta) -> String {
    format!("{}WhereInput", object_name(&cube.name))
}

fn order_by_input_name(cube: &CubeMeta) -> String {
    format!("{}OrderByInput", object_name(&cube.name))
}

fn members_type(cube: &CubeMeta) -> Object {
    let mut object = Object::new(members_type_name(cube));

    for member in visible_named_members(cube) {
        let type_name = map_type(member.member_type.as_deref(), false);
        let mut field = if type_name == "TimeDimension" {
            json_object_field(&member.field_name(), TypeRef::named(type_name))
        } else {
            json_scalar_field(&member.field_name(), TypeRef::named(type_name), type_name)
        };
        if let Some(description) = &member.description {
            field = field.description(description.clone());
        }
        object = object.field(field);
    }

    object
}

fn where_input(cube: &CubeMeta) -> InputObject {
    let name = where_input_name(cube);
    let mut input = InputObject::new(name.clone())
        .field(InputValue::new("AND", TypeRef::named_nn_list(name.clone())))
        .field(InputValue::new("OR", TypeRef::named_nn_list(name)));

    for member in visible_named_members(cube) {
        let filter = format!("{}Filter", map_type(member.member_type.as_deref(), true));
        input = input.field(InputValue::new(member.field_name(), TypeRef::named(filter)));
    }

    input
}

fn order_by_input(cube: &CubeMeta) -> InputObject {
    visible_named_members(cube).fold(
        InputObject::new(order_by_input_name(cube)),
        |input, member| {
            input.field(InputValue::new(
                member.field_name(),
                TypeRef::named("OrderBy"),
            ))
        },
    )
}

fn root_where_input(cubes: &[&CubeMeta]) -> InputObject {
    let mut input = InputObject::new("RootWhereInput")
        .field(InputValue::new(
            "AND",
            TypeRef::named_nn_list("RootWhereInput"),
        ))
        .field(InputValue::new(
            "OR",
            TypeRef::named_nn_list("RootWhereInput"),
        ));

    for cube in unique_by_field_name(cubes) {
        input = input.field(InputValue::new(
            un_capitalize(&cube.name),
            TypeRef::named(where_input_name(cube)),
        ));
    }

    input
}

fn root_order_by_input(cubes: &[&CubeMeta]) -> InputObject {
    unique_by_field_name(cubes).into_iter().fold(
        InputObject::new("RootOrderByInput"),
        |input, cube| {
            input.field(InputValue::new(
                un_capitalize(&cube.name),
                TypeRef::named(order_by_input_name(cube)),
            ))
        },
    )
}

fn result_type(cubes: &[&CubeMeta]) -> Object {
    unique_by_field_name(cubes)
        .into_iter()
        .fold(Object::new(RESULT_TYPE), |object, cube| {
            object.field(
                json_object_field(
                    &un_capitalize(&cube.name),
                    TypeRef::named_nn(members_type_name(cube)),
                )
                .argument(InputValue::new(
                    "where",
                    TypeRef::named(where_input_name(cube)),
                ))
                .argument(InputValue::new(
                    "orderBy",
                    TypeRef::named(order_by_input_name(cube)),
                )),
            )
        })
}

fn query_type(meta: Arc<MetaConfig>) -> Object {
    Object::new("Query").field(
        Field::new(
            CUBE_FIELD,
            TypeRef::named_nn_list_nn(RESULT_TYPE),
            move |ctx| {
                let meta = Arc::clone(&meta);
                FieldFuture::new(async move {
                    let executor = Arc::clone(ctx.data::<Arc<dyn CubeQueryExecutor>>()?);
                    let extensions = ctx.data::<ResponseExtensions>()?.clone();

                    let root = selection_to_field_node(ctx.ctx.field())?;
                    let query = get_json_query(&meta, &root.arguments, &root);

                    let mut result = executor.load(CubeQueryRequest::new(query)).await?;
                    parse_dates(&mut result);
                    extensions.record(&result);

                    let rows = shape_rows(&result);
                    Ok(Some(FieldValue::list(
                        rows.into_iter().map(FieldValue::owned_any),
                    )))
                })
            },
        )
        .argument(InputValue::new("where", TypeRef::named("RootWhereInput")))
        .argument(InputValue::new("limit", TypeRef::named(TypeRef::INT)))
        .argument(InputValue::new("offset", TypeRef::named(TypeRef::INT)))
        .argument(InputValue::new("timezone", TypeRef::named(TypeRef::STRING)))
        .argument(InputValue::new("cache", TypeRef::named(TypeRef::STRING)))
        .argument(InputValue::new(
            "ungrouped",
            TypeRef::named(TypeRef::BOOLEAN),
        ))
        .argument(InputValue::new(
            "orderBy",
            TypeRef::named("RootOrderByInput"),
        )),
    )
}

/// A field that reads `parent[name]` out of the shaped row and hands it on as
/// another JSON object.
fn json_object_field(name: &str, ty: TypeRef) -> Field {
    let key = name.to_string();
    Field::new(name, ty, move |ctx| {
        let key = key.clone();
        FieldFuture::new(async move {
            let parent = ctx.parent_value.try_downcast_ref::<Value>()?;
            let value = parent.get(&key).cloned().unwrap_or(Value::Null);
            Ok(Some(FieldValue::owned_any(value)))
        })
    })
}

/// A leaf field that reads `parent[name]` and coerces it to the declared scalar,
/// the way `graphql-js` coerces a serialized value on output.
fn json_scalar_field(name: &str, ty: TypeRef, type_name: &'static str) -> Field {
    let key = name.to_string();
    Field::new(name, ty, move |ctx| {
        let key = key.clone();
        FieldFuture::new(async move {
            let parent = ctx.parent_value.try_downcast_ref::<Value>()?;
            let value = parent.get(&key).cloned().unwrap_or(Value::Null);
            Ok(coerce_scalar(type_name, &value).map(FieldValue::value))
        })
    })
}

fn coerce_scalar(type_name: &str, value: &Value) -> Option<async_graphql::Value> {
    if value.is_null() {
        return None;
    }

    Some(match type_name {
        TypeRef::FLOAT => match value {
            Value::Number(n) => n
                .as_f64()
                .and_then(async_graphql::Number::from_f64)
                .map(async_graphql::Value::Number)?,
            Value::String(s) => s
                .parse::<f64>()
                .ok()
                .and_then(async_graphql::Number::from_f64)
                .map(async_graphql::Value::Number)?,
            Value::Bool(b) => async_graphql::Value::Number(u64::from(*b).into()),
            _ => return None,
        },
        TypeRef::INT => match value {
            Value::Number(n) => n.as_i64().map(|i| async_graphql::Value::Number(i.into()))?,
            Value::String(s) => s
                .parse::<i64>()
                .ok()
                .map(|i| async_graphql::Value::Number(i.into()))?,
            _ => return None,
        },
        TypeRef::BOOLEAN => async_graphql::Value::Boolean(crate::ast::is_truthy(value)),
        // `String`, `DateTime` and anything else: `String(value)`.
        _ => async_graphql::Value::String(crate::translate::js_to_string(value)),
    })
}

/// Rebuild the `getFieldNodeChildren` view of the live selection set.
fn selection_to_field_node(field: SelectionField<'_>) -> async_graphql::Result<FieldNode> {
    let mut arguments = Map::new();
    for (name, value) in field.arguments()? {
        arguments.insert(name.to_string(), const_value_to_json(&value));
    }

    let mut children = Vec::new();
    for child in field.selection_set() {
        if child.name() == "__typename" {
            continue;
        }

        let directives: Vec<(String, Option<Value>)> = child
            .directives()?
            .iter()
            .map(|directive| {
                let if_arg = directive
                    .arguments
                    .iter()
                    .find(|(name, _)| name.node.as_str() == "if")
                    .map(|(_, value)| const_value_to_json(&value.node));
                (directive.name.node.to_string(), if_arg)
            })
            .collect();

        if !apply_directives(&directives) {
            continue;
        }

        children.push(selection_to_field_node(child)?);
    }

    Ok(FieldNode {
        name: field.name().to_string(),
        alias: field.alias().map(str::to_string),
        arguments,
        children,
    })
}

fn visible_named_members(cube: &CubeMeta) -> impl Iterator<Item = &MemberMeta> {
    cube.visible_members()
        .filter(|member| is_valid_name(&member.field_name()))
}

fn unique_by_field_name<'a>(cubes: &[&'a CubeMeta]) -> Vec<&'a CubeMeta> {
    let mut seen = HashSet::new();
    cubes
        .iter()
        .copied()
        .filter(|cube| {
            let name = un_capitalize(&cube.name);
            is_valid_name(&name) && seen.insert(name)
        })
        .collect()
}

/// `/^[_A-Za-z][_0-9A-Za-z]*$/` — the GraphQL name grammar. Members that cannot
/// be expressed as GraphQL fields are left out rather than producing an
/// invalid schema.
fn is_valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first == '_' || first.is_ascii_alphabetic() => {
            chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_member_types() {
        assert_eq!(map_type(Some("time"), false), "TimeDimension");
        assert_eq!(map_type(Some("time"), true), "DateTime");
        assert_eq!(map_type(Some("string"), false), "String");
        assert_eq!(map_type(Some("number"), false), "Float");
        assert_eq!(map_type(None, false), "String");
        assert_eq!(map_type(Some("boolean"), false), "String");
    }

    #[test]
    fn coerces_output_scalars_like_graphql_js() {
        assert_eq!(
            coerce_scalar("String", &json!(10)),
            Some(async_graphql::Value::String("10".to_string()))
        );
        assert_eq!(coerce_scalar("String", &json!(null)), None);
        assert_eq!(
            coerce_scalar("Float", &json!("1.5")),
            Some(async_graphql::Value::Number(
                async_graphql::Number::from_f64(1.5).unwrap()
            ))
        );
    }

    #[test]
    fn rejects_a_model_with_nothing_to_expose() {
        let meta = MetaConfig::from_value(&json!({ "cubes": [] })).unwrap();
        assert!(matches!(make_schema(&meta), Err(GraphQLError::Schema(_))));
    }

    #[test]
    fn validates_graphql_names() {
        assert!(is_valid_name("created_at"));
        assert!(is_valid_name("_x"));
        assert!(!is_valid_name("1x"));
        assert!(!is_valid_name("a-b"));
        assert!(!is_valid_name(""));
    }
}
