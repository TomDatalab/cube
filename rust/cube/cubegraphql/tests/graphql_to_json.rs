//! Ports `GraphQL Schema > with camelCase / with snake_case` from
//! `packages/cubejs-api-gateway/test/graphql.test.ts`.
//!
//! The Jest suite snapshots the Cube query that reaches `apiGateway.load` for
//! every query in `test/graphql-queries/*.gql`; here we translate the very same
//! fixture files with [`cubegraphql::graphql_to_json`] and compare against those
//! recorded snapshots.
//!
//! One documented difference, for fixture 9 only: the Node.js **resolver** path
//! receives the root `where` argument after `graphql-js` has coerced it, and
//! coercion re-orders an input object's keys into input-type field order — so
//! the snapshot lists the `OR` filter before the `orders.status` one. Both
//! `getJsonQueryFromGraphQLQuery` (i.e. `POST /v1/graphql-to-json`, which is
//! what this function ports) and this crate keep document order instead. The
//! elements are identical and top-level filters are ANDed, so nothing about the
//! resulting query changes.

mod common;

use common::{base_queries, base_snake_case_queries, meta, meta_snake_case};
use cubegraphql::{graphql_to_json, GraphQLError, MetaConfig};
use serde_json::{json, Value};

/// The 11 snapshots of `test/__snapshots__/graphql.test.ts.snap`, parameterised
/// over the model's spelling of the cube and member names.
fn expected(orders: &str, users: &str, created_at: &str) -> Vec<Value> {
    let count = format!("{orders}.count");
    let status = format!("{orders}.status");
    let created = format!("{orders}.{created_at}");
    let city = format!("{users}.city");

    vec![
        json!({
            "measures": [count],
            "timeDimensions": [{ "dimension": created, "granularity": "day" }]
        }),
        json!({
            "measures": [count],
            "timeDimensions": [{ "dimension": created, "granularity": "day" }],
            "filters": [{ "member": status, "operator": "equals", "values": ["shipped"] }]
        }),
        json!({
            "measures": [count],
            "dimensions": [created],
            "timeDimensions": [{
                "dimension": created,
                "dateRange": ["2022-01-01", "2022-02-01"]
            }],
            "order": [[created, "asc"]]
        }),
        json!({
            "measures": [count],
            "timeDimensions": [{
                "dimension": created,
                "granularity": "day",
                "dateRange": ["2022-01-01", "2022-02-01"]
            }],
            "order": [[created, "asc"]]
        }),
        json!({
            "measures": [count],
            "dimensions": [created],
            "timeDimensions": [{ "dimension": created, "dateRange": "This month" }]
        }),
        json!({
            "measures": [count],
            "timeDimensions": [{
                "dimension": created,
                "granularity": "day",
                "dateRange": "This month"
            }]
        }),
        json!({
            "measures": [count],
            "timeDimensions": [{
                "dimension": created,
                "granularity": "day",
                "dateRange": "2 weeks ago to now"
            }]
        }),
        json!({
            "measures": [count],
            "timeDimensions": [{ "dimension": created, "granularity": "day" }],
            "filters": [{
                "member": created,
                "operator": "notInDateRange",
                "values": ["2022-01-01", "2022-02-01"]
            }]
        }),
        json!({
            "measures": [count],
            "dimensions": [status, created, city],
            "order": [[count, "desc"], [status, "asc"], [city, "desc"]]
        }),
        json!({
            "measures": [count],
            "dimensions": [status],
            "filters": [
                {
                    "member": status,
                    "operator": "equals",
                    "values": ["canceled", "active"]
                },
                {
                    "or": [{
                        "or": [
                            { "member": city, "operator": "notSet" },
                            { "member": city, "operator": "equals", "values": ["US"] }
                        ]
                    }]
                }
            ]
        }),
        json!({
            "measures": [count],
            "timeDimensions": [{
                "dimension": created,
                "granularity": "year",
                "dateRange": "This year"
            }],
            "order": [[created, "asc"]]
        }),
    ]
}

fn check(queries: &[String], expected: &[Value], meta: &MetaConfig) {
    assert_eq!(
        queries.len(),
        expected.len(),
        "the fixture file changed: {} queries but {} recorded snapshots",
        queries.len(),
        expected.len()
    );

    for (index, (query, expected)) in queries.iter().zip(expected).enumerate() {
        let actual = graphql_to_json(query, None, meta)
            .unwrap_or_else(|e| panic!("GraphQL query {index} failed to translate: {e}\n{query}"))
            .into_value();

        assert_eq!(&actual, expected, "GraphQL query {index}:\n{query}");
    }
}

#[test]
fn translates_the_camel_case_fixtures() {
    check(
        &base_queries(),
        &expected("Orders", "Users", "createdAt"),
        &meta(),
    );
}

#[test]
fn translates_the_snake_case_fixtures() {
    check(
        &base_snake_case_queries(),
        &expected("orders", "users", "created_at"),
        &meta_snake_case(),
    );
}

#[test]
fn the_fixture_files_carry_every_case() {
    assert_eq!(base_queries().len(), 11);
    assert_eq!(base_snake_case_queries().len(), 11);
}

#[test]
fn reports_an_undefined_variable() {
    let err = graphql_to_json(
        "query CubeQuery($status: String!) {
            cube(where: { orders: { status: { equals: $status } } }) { orders { count } }
        }",
        None,
        &meta(),
    )
    .unwrap_err();

    assert_eq!(
        err,
        GraphQLError::Translate("Variable \"status\" is not defined".to_string())
    );
}

#[test]
fn reports_a_syntax_error() {
    let err = graphql_to_json("query CubeQuery { cube {", None, &meta()).unwrap_err();
    assert!(
        matches!(err, GraphQLError::Parse(ref message) if message.contains("expected")
            || message.contains("Unexpected")),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_non_object_variables() {
    let err = graphql_to_json(
        "query CubeQuery { cube { orders { count } } }",
        Some(&json!("nope")),
        &meta(),
    )
    .unwrap_err();

    assert_eq!(
        err,
        GraphQLError::Translate("GraphQL variables must be an object".to_string())
    );
}

#[test]
fn accepts_the_rest_meta_shape_too() {
    let meta = MetaConfig::from_value(&common::meta_config_rest_shape()).unwrap();
    let actual = graphql_to_json(&base_queries()[0], None, &meta)
        .unwrap()
        .into_value();

    assert_eq!(actual, expected("Orders", "Users", "createdAt")[0]);
}
