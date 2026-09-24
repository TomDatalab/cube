//! Member expressions: the members the SQL API pushes down as an expression
//! rather than a name.

use cubeplanner::{plan, Model, PlanOptions, PlannedSql, PlannerQuery};
use serde_json::json;

fn test_model() -> Model {
    Model::from_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/model")).unwrap()
}

fn plan_query(value: serde_json::Value) -> PlannedSql {
    let query = PlannerQuery::from_value(value).unwrap_or_else(|e| panic!("{e}"));
    plan(&test_model(), &query, &PlanOptions::postgres()).unwrap_or_else(|e| panic!("{e}"))
}

fn plan_error(value: serde_json::Value) -> String {
    let query = match PlannerQuery::from_value(value) {
        Ok(query) => query,
        Err(err) => return err.message(),
    };
    plan(&test_model(), &query, &PlanOptions::postgres())
        .err()
        .map(|err| err.message())
        .unwrap_or_else(|| panic!("expected an error"))
}

/// The parsed shape: `expression` is the JS function body the gateway builds
/// out of cubesql's `SqlFunction`, and the SQL inside it is a template literal.
#[test]
fn a_measure_expression_is_planned_as_written() {
    let planned = plan_query(json!({
        "measures": [{
            "cubeName": "orders",
            "name": "double_amount",
            "expressionName": "double_amount",
            "definition": "orders.total_amount * 2",
            "expression": ["orders", "return `${orders.total_amount} * 2`"]
        }],
        "dimensions": ["orders.status"]
    }));

    assert!(
        planned.sql.contains(r#"sum("orders".amount) * 2"#),
        "{}",
        planned.sql
    );
    insta::assert_snapshot!(planned.sql);
}

#[test]
fn a_dimension_expression_is_planned_as_written() {
    let planned = plan_query(json!({
        "measures": ["orders.count"],
        "dimensions": [{
            "cubeName": "orders",
            "name": "upper_status",
            "expressionName": "upper_status",
            "expression": ["orders", "return `UPPER(${orders.status})`"]
        }]
    }));

    assert!(
        planned.sql.contains(r#"UPPER("orders".status)"#),
        "{}",
        planned.sql
    );
    insta::assert_snapshot!(planned.sql);
}

#[test]
fn a_segment_expression_becomes_a_predicate() {
    let planned = plan_query(json!({
        "measures": ["orders.count"],
        "segments": [{
            "cubeName": "orders",
            "name": "big_orders",
            "expressionName": "big_orders",
            "expression": ["orders", "return `${orders.id} > 100`"]
        }]
    }));

    assert!(
        planned.sql.contains(r#""orders".id > 100"#),
        "{}",
        planned.sql
    );
    insta::assert_snapshot!(planned.sql);
}

/// An expression may reach across cubes, and the join it needs is planned for
/// it the same way a named member's is.
#[test]
fn an_expression_pulls_in_the_join_it_needs() {
    let planned = plan_query(json!({
        "measures": ["orders.count"],
        "dimensions": [{
            "cubeName": "orders",
            "name": "city_status",
            "expressionName": "city_status",
            "expression": [
                "orders",
                "users",
                "return `${users.city} || '/' || ${orders.status}`"
            ]
        }]
    }));

    assert!(planned.sql.contains("LEFT JOIN"), "{}", planned.sql);
    assert!(
        planned
            .sql
            .contains(r#""users".city || '/' || "orders".status"#),
        "{}",
        planned.sql
    );
}

/// `PatchMeasure` re-aggregates an existing measure and pushes extra filters
/// inside the aggregation.
#[test]
fn a_patch_measure_expression_pushes_its_filters_into_the_aggregation() {
    let planned = plan_query(json!({
        "measures": [{
            "cubeName": "orders",
            "name": "shipped_amount",
            "expressionName": "shipped_amount",
            "expression": {
                "type": "PatchMeasure",
                "sourceMeasure": "orders.total_amount",
                "replaceAggregationType": null,
                "addFilters": [["orders", "return `${orders.status} = 'shipped'`"]]
            }
        }]
    }));

    assert!(planned.sql.contains("CASE WHEN"), "{}", planned.sql);
    assert!(
        planned.sql.contains(r#""orders".status = 'shipped'"#),
        "{}",
        planned.sql
    );
    insta::assert_snapshot!(planned.sql);
}

/// `replaceAggregationType` swaps the aggregation the source measure declares.
#[test]
fn a_patch_measure_can_replace_the_aggregation() {
    let planned = plan_query(json!({
        "measures": [{
            "cubeName": "orders",
            "name": "max_amount",
            "expressionName": "max_amount",
            "expression": {
                "type": "PatchMeasure",
                "sourceMeasure": "orders.total_amount",
                "replaceAggregationType": "max",
                "addFilters": []
            }
        }]
    }));

    assert!(planned.sql.contains("max("), "{}", planned.sql);
    assert!(!planned.sql.contains("sum("), "{}", planned.sql);
}

/// The shape cubesql itself emits, which the gateway would otherwise normalize
/// in JavaScript first.
#[test]
fn the_cubesql_input_shape_is_accepted_directly() {
    let from_input = plan_query(json!({
        "measures": [{
            "cubeName": "orders",
            "alias": "double_amount",
            "expr": {
                "type": "SqlFunction",
                "cubeParams": ["orders"],
                "sql": "${orders.total_amount} * 2"
            }
        }],
        "dimensions": ["orders.status"]
    }));

    let from_parsed = plan_query(json!({
        "measures": [{
            "cubeName": "orders",
            "name": "double_amount",
            "expressionName": "double_amount",
            "expression": ["orders", "return `${orders.total_amount} * 2`"]
        }],
        "dimensions": ["orders.status"]
    }));

    assert_eq!(from_input.sql, from_parsed.sql);
}

#[test]
fn the_cubesql_patch_measure_input_shape_is_accepted_directly() {
    let planned = plan_query(json!({
        "measures": [{
            "cubeName": "orders",
            "alias": "shipped_amount",
            "expr": {
                "type": "PatchMeasure",
                "sourceMeasure": "orders.total_amount",
                "replaceAggregationType": null,
                "addFilters": [{
                    "cubeParams": ["orders"],
                    "sql": "${orders.status} = 'shipped'"
                }]
            }
        }]
    }));

    assert!(planned.sql.contains("CASE WHEN"), "{}", planned.sql);
    assert!(
        planned.sql.contains(r#""orders".status = 'shipped'"#),
        "{}",
        planned.sql
    );
}

/// A `subqueryJoins` entry joins an already-rendered SELECT, and its `on` is a
/// member expression too.
#[test]
fn a_subquery_join_is_rendered_with_its_on_expression() {
    let planned = plan_query(json!({
        "measures": ["orders.count"],
        "dimensions": ["orders.status"],
        "subqueryJoins": [{
            "sql": "SELECT 1 AS user_id, 'vip' AS tier",
            "alias": "tiers",
            "joinType": "LEFT",
            "on": {
                "cubeName": "orders",
                "name": "tier_join",
                "expressionName": "tier_join",
                "expression": ["orders", "return `${orders.user_id} = tiers.user_id`"]
            }
        }]
    }));

    assert!(
        planned.sql.contains("SELECT 1 AS user_id, 'vip' AS tier"),
        "{}",
        planned.sql
    );
    assert!(
        planned.sql.contains(r#""orders".user_id = tiers.user_id"#),
        "{}",
        planned.sql
    );
    insta::assert_snapshot!(planned.sql);
}

// ---- errors ----------------------------------------------------------------

#[test]
fn a_function_body_that_is_not_a_template_literal_is_reported() {
    let message = plan_error(json!({
        "measures": [{
            "cubeName": "orders",
            "name": "broken",
            "expressionName": "broken",
            "expression": ["orders", "return orders.amount"]
        }]
    }));

    assert!(message.contains("broken"), "{message}");
    assert!(message.contains("template literal"), "{message}");
}

#[test]
fn an_unsupported_expression_struct_is_reported() {
    let message = plan_error(json!({
        "measures": [{
            "cubeName": "orders",
            "name": "weird",
            "expressionName": "weird",
            "expression": {
                "type": "SomethingElse",
                "sourceMeasure": "orders.total_amount",
                "addFilters": []
            }
        }]
    }));

    assert!(message.contains("weird"), "{message}");
    assert!(message.contains("PatchMeasure"), "{message}");
}

#[test]
fn an_expression_the_member_sql_parser_cannot_read_is_reported() {
    let message = plan_error(json!({
        "measures": [{
            "cubeName": "orders",
            "name": "tricky",
            "expressionName": "tricky",
            "expression": [
                "orders",
                "return `${SECURITY_CONTEXT.role.unsafeValue() === 'admin' ? 1 : 0}`"
            ]
        }]
    }));

    assert!(message.contains("tricky"), "{message}");
}

/// A member expression is addressed by its `expressionName` everywhere else in
/// the query.
#[test]
fn an_expression_can_be_ordered_by_its_name() {
    let planned = plan_query(json!({
        "measures": [{
            "cubeName": "orders",
            "name": "double_amount",
            "expressionName": "double_amount",
            "expression": ["orders", "return `${orders.total_amount} * 2`"]
        }],
        "dimensions": ["orders.status"],
        "order": [{ "id": "double_amount", "desc": true }]
    }));

    assert!(planned.sql.contains("ORDER BY"), "{}", planned.sql);
    assert!(planned.sql.contains("DESC"), "{}", planned.sql);
}

/// A member expression has no model member to name, so the alias map carries
/// the planner's own identity for it (`expr:<cube>.<expressionName>`). The JS
/// `aliasNameToMember` puts the whole expression object there for the same
/// reason. The alias itself stays the `expressionName` the caller sent.
#[test]
fn an_expression_is_keyed_by_its_alias_in_the_alias_map() {
    let planned = plan_query(json!({
        "measures": [{
            "cubeName": "orders",
            "name": "double_amount",
            "expressionName": "double_amount",
            "expression": ["orders", "return `${orders.total_amount} * 2`"]
        }],
        "dimensions": ["orders.status"]
    }));

    assert_eq!(
        planned.alias_name_to_member.get("double_amount"),
        Some(&"expr:orders.double_amount".to_string()),
        "{:?}",
        planned.alias_name_to_member
    );
    assert_eq!(
        planned.alias_name_to_member.get("orders__status"),
        Some(&"orders.status".to_string())
    );
}

#[test]
fn a_member_that_is_neither_a_name_nor_an_expression_is_reported() {
    let message = plan_error(json!({ "measures": [{ "cubeName": "orders" }] }));
    assert!(message.contains("`expression`"), "{message}");
    assert!(message.contains("`expr`"), "{message}");
    assert!(message.contains("`cubeName`"), "{message}");

    let message = plan_error(json!({ "measures": [42] }));
    assert!(
        message.contains("member name or a member expression"),
        "{message}"
    );
}

#[test]
fn a_malformed_expression_is_reported_against_the_shape_it_chose() {
    // `expression` picks the parsed shape, which needs a `cubeName`.
    let message = plan_error(json!({
        "measures": [{ "name": "x", "expression": ["return `1`"] }]
    }));
    assert!(message.contains("cubeName"), "{message}");

    // `expr` picks the SQL API shape, which needs an `alias`.
    let message = plan_error(json!({
        "measures": [{
            "cubeName": "orders",
            "expr": { "type": "SqlFunction", "cubeParams": [], "sql": "1" }
        }]
    }));
    assert!(message.contains("alias"), "{message}");
}
