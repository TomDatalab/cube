//! Ports `should make valid schema` / `should make valid schema when name is
//! not capitalized` from `test/graphql.test.ts`, plus checks on the rest of the
//! generated surface.

mod common;

use common::{meta, meta_config_rest_shape, meta_snake_case};
use cubegraphql::{make_schema, make_schema_from_value};

/// Pull one `type X { ... }` / `input X { ... }` block out of the SDL.
fn type_block(sdl: &str, header: &str) -> String {
    let start = sdl
        .find(&format!("{header} {{"))
        .unwrap_or_else(|| panic!("`{header}` is missing from the schema:\n{sdl}"));
    let rest = &sdl[start..];
    let end = rest.find("\n}").expect("unterminated type block");
    rest[..end].to_string()
}

fn expect_valid_schema(sdl: &str) {
    let orders = type_block(sdl, "type OrdersMembers");
    for field in ["id", "status", "createdAt"] {
        assert!(
            orders.contains(&format!("\t{field}:")) || orders.contains(&format!("  {field}:")),
            "OrdersMembers is missing `{field}`:\n{orders}"
        );
    }
}

#[test]
fn should_make_valid_schema() {
    let schema = make_schema(&meta()).unwrap();
    expect_valid_schema(&schema.sdl());
}

#[test]
fn should_make_valid_schema_when_name_is_not_capitalized() {
    // `makeSchema(JSON.parse(JSON.stringify(metaConfig).replace(/Orders/g, 'orders')))`
    let raw = common::meta_config_value()
        .to_string()
        .replace("Orders", "orders");
    let schema = make_schema_from_value(&serde_json::from_str(&raw).unwrap()).unwrap();
    expect_valid_schema(&schema.sdl());
}

#[test]
fn accepts_the_rest_meta_response_shape() {
    let schema = make_schema_from_value(&meta_config_rest_shape()).unwrap();
    expect_valid_schema(&schema.sdl());
}

#[test]
fn snake_case_members_keep_their_spelling() {
    let schema = make_schema(&meta_snake_case()).unwrap();
    let sdl = schema.sdl();

    let orders = type_block(&sdl, "type OrdersMembers");
    assert!(orders.contains("created_at: TimeDimension"), "{orders}");
}

#[test]
fn maps_member_types_to_scalars() {
    let sdl = make_schema(&meta()).unwrap().sdl();
    let orders = type_block(&sdl, "type OrdersMembers");

    // No `type` in the meta config falls through `mapType`'s default branch.
    assert!(orders.contains("count: String"), "{orders}");
    assert!(orders.contains("totalAmount: String"), "{orders}");
    assert!(orders.contains("status: String"), "{orders}");
    assert!(orders.contains("createdAt: TimeDimension"), "{orders}");
}

#[test]
fn builds_the_root_query_field() {
    let sdl = make_schema(&meta()).unwrap().sdl();
    let query = type_block(&sdl, "type Query");

    for argument in [
        "where: RootWhereInput",
        "limit: Int",
        "offset: Int",
        "timezone: String",
        "cache: String",
        "ungrouped: Boolean",
        "orderBy: RootOrderByInput",
    ] {
        assert!(
            query.contains(argument),
            "`{argument}` missing from:\n{query}"
        );
    }
    assert!(query.contains("): [Result!]!"), "{query}");
}

#[test]
fn builds_the_result_type_with_per_cube_arguments() {
    let sdl = make_schema(&meta()).unwrap().sdl();
    let result = type_block(&sdl, "type Result");

    assert!(result.contains("where: OrdersWhereInput"), "{result}");
    assert!(result.contains("orderBy: OrdersOrderByInput"), "{result}");
    assert!(result.contains("): OrdersMembers!"), "{result}");
    assert!(result.contains("): UsersMembers!"), "{result}");
}

#[test]
fn builds_the_time_dimension_type() {
    let sdl = make_schema(&meta()).unwrap().sdl();
    let time_dimension = type_block(&sdl, "type TimeDimension");

    for granularity in [
        "value", "second", "minute", "hour", "day", "week", "month", "quarter", "year",
    ] {
        assert!(
            time_dimension.contains(&format!("{granularity}: DateTime!")),
            "`{granularity}` missing from:\n{time_dimension}"
        );
    }
}

#[test]
fn builds_the_filter_input_types() {
    let sdl = make_schema(&meta()).unwrap().sdl();

    let float = type_block(&sdl, "input FloatFilter");
    for field in [
        "equals: Float",
        "notEquals: Float",
        "in: [Float]",
        "notIn: [Float]",
        "set: Boolean",
        "gt: Float",
        "lt: Float",
        "gte: Float",
        "lte: Float",
    ] {
        assert!(float.contains(field), "`{field}` missing from:\n{float}");
    }

    let string = type_block(&sdl, "input StringFilter");
    for field in [
        "equals: String",
        "notEquals: String",
        "in: [String]",
        "notIn: [String]",
        "contains: [String]",
        "notContains: [String]",
        "startsWith: [String]",
        "notStartsWith: [String]",
        "endsWith: [String]",
        "notEndsWith: [String]",
        "set: Boolean",
    ] {
        assert!(string.contains(field), "`{field}` missing from:\n{string}");
    }

    let date_time = type_block(&sdl, "input DateTimeFilter");
    for field in [
        "equals: [String]",
        "notEquals: [String]",
        "in: [String]",
        "notIn: [String]",
        "inDateRange: [String]",
        "notInDateRange: [String]",
        "beforeDate: String",
        "beforeOrOnDate: String",
        "afterDate: String",
        "afterOrOnDate: String",
        "set: Boolean",
    ] {
        assert!(
            date_time.contains(field),
            "`{field}` missing from:\n{date_time}"
        );
    }
}

#[test]
fn builds_the_where_and_order_by_inputs() {
    let sdl = make_schema(&meta()).unwrap().sdl();

    let orders_where = type_block(&sdl, "input OrdersWhereInput");
    assert!(
        orders_where.contains("AND: [OrdersWhereInput!]"),
        "{orders_where}"
    );
    assert!(
        orders_where.contains("OR: [OrdersWhereInput!]"),
        "{orders_where}"
    );
    assert!(
        orders_where.contains("status: StringFilter"),
        "{orders_where}"
    );
    assert!(
        orders_where.contains("createdAt: DateTimeFilter"),
        "{orders_where}"
    );

    let root_where = type_block(&sdl, "input RootWhereInput");
    assert!(
        root_where.contains("AND: [RootWhereInput!]"),
        "{root_where}"
    );
    assert!(
        root_where.contains("orders: OrdersWhereInput"),
        "{root_where}"
    );
    assert!(
        root_where.contains("users: UsersWhereInput"),
        "{root_where}"
    );

    let orders_order_by = type_block(&sdl, "input OrdersOrderByInput");
    assert!(
        orders_order_by.contains("status: OrderBy"),
        "{orders_order_by}"
    );

    let root_order_by = type_block(&sdl, "input RootOrderByInput");
    assert!(
        root_order_by.contains("orders: OrdersOrderByInput"),
        "{root_order_by}"
    );
}

#[test]
fn hidden_members_and_private_cubes_are_left_out() {
    let schema = make_schema_from_value(&serde_json::json!({
        "cubes": [
            {
                "name": "Orders",
                "measures": [{ "name": "Orders.count", "isVisible": true }],
                "dimensions": [{ "name": "Orders.secret", "type": "string", "isVisible": false }]
            },
            {
                "name": "Hidden",
                "public": false,
                "measures": [{ "name": "Hidden.count", "isVisible": true }],
                "dimensions": []
            }
        ]
    }))
    .unwrap();
    let sdl = schema.sdl();

    assert!(!sdl.contains("secret"), "{sdl}");
    assert!(!sdl.contains("HiddenMembers"), "{sdl}");
    assert!(sdl.contains("OrdersMembers"), "{sdl}");
}

#[test]
fn every_fixture_query_validates_against_the_schema() {
    // The Jest suite asserts that running every fixture through
    // `express-graphql` produces no error text; validating each document
    // against the schema is the same check without an HTTP server.
    let schema = make_schema(&meta()).unwrap();

    for (index, query) in common::base_queries().iter().enumerate() {
        let errors = run_query(&schema, query);
        assert!(
            errors.is_empty(),
            "GraphQL query {index} did not validate: {errors:?}\n{query}"
        );
    }
}

/// Run a document with no executor registered. Validation errors surface
/// before resolution, so a document that only fails at `load` time comes back
/// with the executor error instead — which is what we filter on.
fn run_query(schema: &async_graphql::dynamic::Schema, query: &str) -> Vec<String> {
    let response = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(async {
            cubegraphql::execute_request(
                schema,
                async_graphql::Request::new(query),
                cubegraphql::executor_fn(|_| async move {
                    Ok(serde_json::json!({
                        "query": {},
                        "annotation": { "measures": {}, "dimensions": {}, "timeDimensions": {} },
                        "data": []
                    }))
                }),
            )
            .await
        });

    response.errors.into_iter().map(|e| e.message).collect()
}
