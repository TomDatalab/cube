//! Ports the `GraphQL Schema > extensions` describe block of
//! `test/graphql.test.ts`: run a document end to end against a stub executor
//! and check the shaped data plus the response extensions.

mod common;

use common::meta;
use cubegraphql::{
    execute_request, executor_fn, handle_graphql_request, make_schema, CubeQueryRequest,
    GraphQLError, API_TYPE, REGULAR_QUERY,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const MOCK_LAST_REFRESH_TIME: &str = "2025-06-01T12:00:00.000Z";

fn mock_annotation() -> Value {
    json!({
        "measures": {
            "Orders.count": { "title": "Orders Count", "shortTitle": "Count", "type": "number" },
            "Orders.totalAmount": {
                "title": "Orders Total Amount",
                "shortTitle": "Total Amount",
                "type": "number"
            }
        },
        "dimensions": {
            "Orders.status": { "title": "Orders Status", "shortTitle": "Status", "type": "string" }
        },
        "segments": {},
        "timeDimensions": {}
    })
}

fn mock_result() -> Value {
    json!({
        "query": {},
        "annotation": mock_annotation(),
        "lastRefreshTime": MOCK_LAST_REFRESH_TIME,
        "data": [
            { "Orders.count": 10, "Orders.totalAmount": 500, "Orders.status": "completed" },
            { "Orders.count": 5, "Orders.totalAmount": 200, "Orders.status": "pending" }
        ]
    })
}

const ORDERS_QUERY: &str = "query CubeQuery {
    cube {
        orders { count totalAmount status }
    }
}";

#[tokio::test]
async fn should_return_annotation_and_last_refresh_time_in_extensions() {
    let schema = make_schema(&meta()).unwrap();
    let executor = executor_fn(|_req| async move { Ok(mock_result()) });

    let response =
        execute_request(&schema, async_graphql::Request::new(ORDERS_QUERY), executor).await;

    assert!(response.errors.is_empty(), "{:?}", response.errors);

    let body = serde_json::to_value(&response).unwrap();
    assert_eq!(body["extensions"]["annotation"], mock_annotation());
    assert_eq!(
        body["extensions"]["lastRefreshTime"],
        json!(MOCK_LAST_REFRESH_TIME)
    );
}

#[tokio::test]
async fn should_return_used_pre_aggregations_in_extensions() {
    let used_pre_aggregations = json!({
        "schema.orders_main20240101": {
            "preAggregationId": "Orders.main",
            "lastUpdatedAt": 1712000000000u64,
            "type": "rollup"
        }
    });

    let schema = make_schema(&meta()).unwrap();
    let expected = used_pre_aggregations.clone();
    let executor = executor_fn(move |_req| {
        let mut result = mock_result();
        result["usedPreAggregations"] = expected.clone();
        async move { Ok(result) }
    });

    let body = handle_graphql_request(&schema, &json!({ "query": ORDERS_QUERY }), executor).await;

    assert_eq!(body.get("errors"), None, "{body}");
    assert_eq!(
        body["extensions"]["usedPreAggregations"],
        used_pre_aggregations
    );
}

#[tokio::test]
async fn should_accumulate_all_measures_and_dimensions() {
    let schema = make_schema(&meta()).unwrap();
    let executor = executor_fn(|_req| async move { Ok(mock_result()) });

    let body = handle_graphql_request(&schema, &json!({ "query": ORDERS_QUERY }), executor).await;

    assert_eq!(body.get("errors"), None, "{body}");
    assert_eq!(
        body["data"]["cube"],
        json!([
            { "orders": { "count": "10", "totalAmount": "500", "status": "completed" } },
            { "orders": { "count": "5", "totalAmount": "200", "status": "pending" } }
        ])
    );
}

#[tokio::test]
async fn the_executor_receives_the_translated_query() {
    let seen: Arc<Mutex<Option<CubeQueryRequest>>> = Arc::new(Mutex::new(None));
    let sink = Arc::clone(&seen);

    let schema = make_schema(&meta()).unwrap();
    let executor = executor_fn(move |req: CubeQueryRequest| {
        let sink = Arc::clone(&sink);
        async move {
            *sink.lock().unwrap() = Some(req);
            Ok(json!({
                "query": { "timezone": "UTC" },
                "annotation": {
                    "measures": { "Orders.count": { "type": "number" } },
                    "dimensions": {},
                    "timeDimensions": { "Orders.createdAt.day": { "type": "time" } }
                },
                "data": [{ "Orders.count": 10, "Orders.createdAt.day": "2022-01-01T00:00:00.000" }]
            }))
        }
    });

    let query = "query CubeQuery {
        cube(limit: 10, timezone: \"UTC\", cache: \"no-cache\") {
            orders(orderBy: { count: desc }) { count createdAt { day } }
        }
    }";

    let response = execute_request(&schema, async_graphql::Request::new(query), executor).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);

    let request = seen.lock().unwrap().clone().expect("executor was called");
    assert_eq!(request.query_type, REGULAR_QUERY);
    assert_eq!(request.api_type, API_TYPE);
    assert_eq!(request.cache.as_deref(), Some("no-cache"));
    assert_eq!(
        request.query.into_value(),
        json!({
            "measures": ["Orders.count"],
            "timeDimensions": [{ "dimension": "Orders.createdAt", "granularity": "day" }],
            "order": [["Orders.count", "desc"]],
            "limit": 10,
            "timezone": "UTC",
            "cache": "no-cache"
        })
    );
}

#[tokio::test]
async fn time_dimensions_are_returned_as_instants() {
    let schema = make_schema(&meta()).unwrap();
    let executor = executor_fn(|_req| async move {
        Ok(json!({
            "query": { "timezone": "America/Los_Angeles" },
            "annotation": {
                "measures": {},
                "dimensions": {},
                "timeDimensions": { "Orders.createdAt.day": { "type": "time" } }
            },
            "data": [{ "Orders.createdAt.day": "2022-01-01T00:00:00.000" }]
        }))
    });

    let body = handle_graphql_request(
        &schema,
        &json!({ "query": "query CubeQuery { cube { orders { createdAt { day } } } }" }),
        executor,
    )
    .await;

    assert_eq!(body.get("errors"), None, "{body}");
    assert_eq!(
        body["data"]["cube"],
        json!([{ "orders": { "createdAt": { "day": "2022-01-01T08:00:00.000Z" } } }])
    );
}

#[tokio::test]
async fn executor_failures_surface_as_graphql_errors() {
    let schema = make_schema(&meta()).unwrap();
    let executor =
        executor_fn(|_req| async move { Err(GraphQLError::Execution("boom".to_string())) });

    let body = handle_graphql_request(&schema, &json!({ "query": ORDERS_QUERY }), executor).await;

    assert_eq!(body["errors"][0]["message"], json!("boom"));
}

#[tokio::test]
async fn unknown_fields_are_rejected_by_validation() {
    let schema = make_schema(&meta()).unwrap();
    let executor = executor_fn(|_req| async move { Ok(mock_result()) });

    let body = handle_graphql_request(
        &schema,
        &json!({ "query": "query CubeQuery { cube { orders { nope } } }" }),
        executor,
    )
    .await;

    assert!(
        body["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("nope"),
        "{body}"
    );
}

#[tokio::test]
async fn variables_are_forwarded_to_the_translator() {
    let seen: Arc<Mutex<Option<CubeQueryRequest>>> = Arc::new(Mutex::new(None));
    let sink = Arc::clone(&seen);

    let schema = make_schema(&meta()).unwrap();
    let executor = executor_fn(move |req: CubeQueryRequest| {
        let sink = Arc::clone(&sink);
        async move {
            *sink.lock().unwrap() = Some(req);
            Ok(mock_result())
        }
    });

    let body = handle_graphql_request(
        &schema,
        &json!({
            "query": "query CubeQuery($status: String!) {
                cube(where: { orders: { status: { equals: $status } } }) { orders { count } }
            }",
            "variables": { "status": "shipped" }
        }),
        executor,
    )
    .await;

    assert_eq!(body.get("errors"), None, "{body}");

    let request = seen.lock().unwrap().clone().expect("executor was called");
    assert_eq!(
        request.query.as_value()["filters"],
        json!([{ "member": "Orders.status", "operator": "equals", "values": ["shipped"] }])
    );
}

#[tokio::test]
async fn a_request_without_a_query_is_rejected() {
    let schema = make_schema(&meta()).unwrap();
    let executor = executor_fn(|_req| async move { Ok(mock_result()) });

    let body = handle_graphql_request(&schema, &json!({}), executor).await;

    assert_eq!(
        body["errors"][0]["message"],
        json!("Must provide query string.")
    );
}

#[tokio::test]
async fn introspection_works() {
    let schema = make_schema(&meta()).unwrap();
    let executor = executor_fn(|_req| async move { Ok(mock_result()) });

    let body = handle_graphql_request(
        &schema,
        &json!({ "query": "{ __schema { queryType { name } } }" }),
        executor,
    )
    .await;

    assert_eq!(
        body["data"]["__schema"]["queryType"]["name"],
        json!("Query")
    );
}
