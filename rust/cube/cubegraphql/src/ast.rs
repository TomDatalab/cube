//! A minimal, JSON-flavoured view of the GraphQL selection tree.
//!
//! `graphql.ts` walks `graphql-js` `FieldNode`s directly, both from the
//! resolver (`getJsonQuery`) and from a freshly parsed document
//! (`getJsonQueryFromGraphQLQuery`). [`FieldNode`] is the one shape both of our
//! paths produce, so the translator in [`crate::translate`] has a single
//! implementation.

use async_graphql::parser::types::{
    DocumentOperations, ExecutableDocument, Field, FragmentDefinition, Selection,
};
use async_graphql::parser::{parse_query, Positioned};
use async_graphql::Name;
use async_graphql_value::Value as GqlValue;
use serde_json::{Map, Value};
use std::collections::HashMap;

use crate::error::GraphQLError;

/// A selected field with its arguments (variables already substituted) and its
/// selected children.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FieldNode {
    /// The field name as written in the document.
    pub name: String,
    /// The response key, when an alias was used.
    pub alias: Option<String>,
    /// Field arguments, in document order.
    pub arguments: Map<String, Value>,
    /// Selected child fields, `__typename` and `@skip`/`@include`-excluded
    /// fields already removed — i.e. `getFieldNodeChildren`.
    pub children: Vec<FieldNode>,
}

impl FieldNode {
    /// Look up an argument. Mirrors `getArgumentValue`.
    pub fn argument(&self, name: &str) -> Option<&Value> {
        self.arguments.get(name).filter(|v| !v.is_null())
    }
}

/// Parse a GraphQL document and return the first selected field of the chosen
/// operation — the `cube` field.
///
/// `graphql.ts` takes `operation.selectionSet.selections[0]` of the first
/// `OperationDefinition` and ignores `operationName`; we honour
/// `operation_name` when the document declares several operations, and
/// otherwise behave the same.
pub fn parse_root_field(
    query: &str,
    variables: &Map<String, Value>,
    operation_name: Option<&str>,
) -> Result<FieldNode, GraphQLError> {
    let document: ExecutableDocument =
        parse_query(query).map_err(|e| GraphQLError::Parse(e.to_string()))?;

    let operation = select_operation(&document, operation_name)?;

    let selections = &operation.selection_set.node.items;
    let first = selections
        .iter()
        .find_map(|selection| match &selection.node {
            Selection::Field(field) => Some(&field.node),
            _ => None,
        })
        .ok_or_else(|| {
            GraphQLError::Translate("GraphQL operation has an empty selection set".to_string())
        })?;

    field_node(first, &document.fragments, variables)
}

fn select_operation<'a>(
    document: &'a ExecutableDocument,
    operation_name: Option<&str>,
) -> Result<&'a async_graphql::parser::types::OperationDefinition, GraphQLError> {
    match &document.operations {
        DocumentOperations::Single(op) => Ok(&op.node),
        DocumentOperations::Multiple(ops) => {
            if let Some(name) = operation_name {
                ops.get(name).map(|op| &op.node).ok_or_else(|| {
                    GraphQLError::Translate(format!("Unknown operation named \"{name}\""))
                })
            } else {
                // `HashMap` is unordered; take the operation that appears first
                // in the document so behaviour is deterministic.
                ops.values()
                    .min_by_key(|op| (op.pos.line, op.pos.column))
                    .map(|op| &op.node)
                    .ok_or_else(|| {
                        GraphQLError::Translate("GraphQL document has no operation".to_string())
                    })
            }
        }
    }
}

fn field_node(
    field: &Field,
    fragments: &HashMap<Name, Positioned<FragmentDefinition>>,
    variables: &Map<String, Value>,
) -> Result<FieldNode, GraphQLError> {
    let mut arguments = Map::new();
    for (name, value) in &field.arguments {
        arguments.insert(
            name.node.to_string(),
            parse_argument_value(&value.node, variables)?,
        );
    }

    let mut children = Vec::new();
    collect_children(
        &field.selection_set.node.items,
        fragments,
        variables,
        &mut children,
    )?;

    Ok(FieldNode {
        name: field.name.node.to_string(),
        alias: field.alias.as_ref().map(|a| a.node.to_string()),
        arguments,
        children,
    })
}

fn collect_children(
    selections: &[Positioned<Selection>],
    fragments: &HashMap<Name, Positioned<FragmentDefinition>>,
    variables: &Map<String, Value>,
    out: &mut Vec<FieldNode>,
) -> Result<(), GraphQLError> {
    for selection in selections {
        match &selection.node {
            Selection::Field(field) => {
                let field = &field.node;
                if field.name.node.as_str() == "__typename" {
                    continue;
                }
                if !apply_directives_ast(&field.directives, variables) {
                    continue;
                }
                out.push(field_node(field, fragments, variables)?);
            }
            // `graphql.ts` drops fragments (it hands `getJsonQuery` an empty
            // `fragments` map); we expand them so that the translated query
            // matches what the executor would actually resolve.
            Selection::FragmentSpread(spread) => {
                if let Some(fragment) = fragments.get(&spread.node.fragment_name.node) {
                    collect_children(
                        &fragment.node.selection_set.node.items,
                        fragments,
                        variables,
                        out,
                    )?;
                }
            }
            Selection::InlineFragment(fragment) => {
                collect_children(
                    &fragment.node.selection_set.node.items,
                    fragments,
                    variables,
                    out,
                )?;
            }
        }
    }

    Ok(())
}

fn apply_directives_ast(
    directives: &[Positioned<async_graphql::parser::types::Directive>],
    variables: &Map<String, Value>,
) -> bool {
    let resolved: Vec<(String, Option<Value>)> = directives
        .iter()
        .map(|directive| {
            let directive = &directive.node;
            let if_arg = directive
                .arguments
                .iter()
                .find(|(name, _)| name.node.as_str() == "if")
                .map(|(_, value)| match &value.node {
                    GqlValue::Variable(name) => {
                        variables.get(name.as_str()).cloned().unwrap_or(Value::Null)
                    }
                    other => parse_argument_value(other, variables).unwrap_or(Value::Null),
                });
            (directive.name.node.to_string(), if_arg)
        })
        .collect();

    apply_directives(&resolved)
}

/// `applyDirectives` from `graphql.ts`: `@include(if: false)` and
/// `@skip(if: true)` drop the field.
pub(crate) fn apply_directives(directives: &[(String, Option<Value>)]) -> bool {
    directives.iter().all(|(name, if_arg)| match if_arg {
        Some(value) => match name.as_str() {
            "include" => is_truthy(value),
            "skip" => !is_truthy(value),
            _ => true,
        },
        None => true,
    })
}

/// JavaScript truthiness, for the `&&` / `!` checks ported from `graphql.ts`.
pub(crate) fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0 && !f.is_nan()).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        // `[]` and `{}` are truthy in JS.
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// `parseArgumentValue` from `graphql.ts`.
///
/// One intentional difference: `IntValue`/`FloatValue` become JSON numbers
/// rather than the raw source strings `graphql-js` exposes, so
/// `limit: 10` translates to `"limit": 10` and not `"limit": "10"`. Everything
/// downstream stringifies filter values anyway, so no filter output changes.
pub fn parse_argument_value(
    value: &GqlValue,
    variables: &Map<String, Value>,
) -> Result<Value, GraphQLError> {
    Ok(match value {
        GqlValue::Boolean(b) => Value::Bool(*b),
        GqlValue::Number(n) => Value::Number(n.clone()),
        GqlValue::String(s) => Value::String(s.clone()),
        GqlValue::Enum(name) => Value::String(name.to_string()),
        GqlValue::List(items) => Value::Array(
            items
                .iter()
                .map(|item| parse_argument_value(item, variables))
                .collect::<Result<_, _>>()?,
        ),
        GqlValue::Object(fields) => {
            let mut obj = Map::new();
            for (name, item) in fields {
                obj.insert(name.to_string(), parse_argument_value(item, variables)?);
            }
            Value::Object(obj)
        }
        GqlValue::Variable(name) => match variables.get(name.as_str()) {
            Some(Value::Null) | None => {
                return Err(GraphQLError::undefined_variable(name.as_str()))
            }
            Some(value) => value.clone(),
        },
        // `NullValue` and `BinaryValue` fall through `parseArgumentValue`'s
        // `default:` branch and yield `undefined`.
        GqlValue::Null | GqlValue::Binary(_) => Value::Null,
    })
}

/// Convert an `async-graphql` const value (variables already resolved) to JSON.
pub(crate) fn const_value_to_json(value: &async_graphql::Value) -> Value {
    value.clone().into_json().unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_arguments_and_children() {
        let root = parse_root_field(
            "query CubeQuery { cube(limit: 10) { orders { count createdAt { day } } } }",
            &Map::new(),
            None,
        )
        .unwrap();

        assert_eq!(root.name, "cube");
        assert_eq!(root.argument("limit"), Some(&json!(10)));
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].name, "orders");
        assert_eq!(root.children[0].children.len(), 2);
        assert_eq!(root.children[0].children[1].children[0].name, "day");
    }

    #[test]
    fn drops_typename_and_applies_directives() {
        let mut variables = Map::new();
        variables.insert("withStatus".to_string(), json!(false));

        let root = parse_root_field(
            "query CubeQuery($withStatus: Boolean!) { cube { orders { __typename count status @include(if: $withStatus) } } }",
            &variables,
            None,
        )
        .unwrap();

        let names: Vec<_> = root.children[0]
            .children
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, vec!["count"]);
    }

    #[test]
    fn expands_fragments() {
        let root = parse_root_field(
            "query CubeQuery { cube { orders { ...F } } } fragment F on OrdersMembers { count }",
            &Map::new(),
            None,
        )
        .unwrap();

        assert_eq!(root.children[0].children[0].name, "count");
    }

    #[test]
    fn undefined_variable_is_an_error() {
        let err = parse_root_field(
            "query CubeQuery($limit: Int) { cube(limit: $limit) { orders { count } } }",
            &Map::new(),
            None,
        )
        .unwrap_err();

        assert_eq!(err.to_string(), "Variable \"limit\" is not defined");
    }

    #[test]
    fn syntax_errors_are_reported() {
        let err = parse_root_field("query {", &Map::new(), None).unwrap_err();
        assert!(matches!(err, GraphQLError::Parse(_)));
    }
}
