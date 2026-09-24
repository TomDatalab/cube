//! `/v1/convert-query` and `/v1/cubesql`: turning a SQL statement into the
//! REST query it is equivalent to, and reporting how it would be answered.

use std::sync::Arc;

use cubesql::CubeError;
use cubesqlbridge::{
    rest4sql, sql4sql, start_sql_api, ConvertedQuery, LoadResponse, ModelSource, QueryExecutor,
    Sql4SqlPlan, SqlApi, SqlApiConfig, SqlAuthConfig,
};
use serde_json::{json, Value};

const MODEL: &str = include_str!("model.yml");

/// Refuses every query: conversion never executes anything.
#[derive(Debug)]
struct NeverRuns;

#[async_trait::async_trait]
impl QueryExecutor for NeverRuns {
    async fn execute(&self, _: Value, _: &Value) -> Result<LoadResponse, CubeError> {
        Err(CubeError::internal(
            "conversion must not execute the query".to_string(),
        ))
    }
}

async fn api() -> SqlApi {
    let config = SqlApiConfig {
        model_source: ModelSource::yaml(MODEL),
        // No listener: the conversion entry points drive the services
        // directly, without a client connection.
        postgres_bind_address: None,
        planner_threads: 2,
        dialects: Default::default(),
        include_hidden_members: false,
        executor: Arc::new(NeverRuns),
        auth_config: SqlAuthConfig::default(),
        authenticator: None,
    };

    start_sql_api(config).await.expect("the SQL API starts")
}

#[tokio::test]
async fn converts_a_grouped_query_to_a_rest_query() {
    let api = api().await;

    let converted = rest4sql(
        api.services(),
        "SELECT status, MEASURE(count) FROM orders GROUP BY 1",
        None,
    )
    .await
    .expect("conversion runs");

    let ConvertedQuery::Ok { status, query } = converted else {
        panic!("expected a converted query, got {converted:?}");
    };
    assert_eq!(status, "ok");

    let query = serde_json::to_value(&*query).expect("serializes");
    assert_eq!(query["measures"], json!(["orders.count"]));
    assert_eq!(query["dimensions"], json!(["orders.status"]));
}

#[tokio::test]
async fn converts_a_filtered_query() {
    let api = api().await;

    let converted = rest4sql(
        api.services(),
        "SELECT MEASURE(count) FROM orders WHERE status = 'shipped'",
        None,
    )
    .await
    .expect("conversion runs");

    let ConvertedQuery::Ok { query, .. } = converted else {
        panic!("expected a converted query, got {converted:?}");
    };
    let query = serde_json::to_value(&*query).expect("serializes");
    assert_eq!(query["measures"], json!(["orders.count"]));

    let filters = query["filters"].as_array().expect("filters");
    assert_eq!(filters.len(), 1, "{query}");
    assert_eq!(filters[0]["member"], "orders.status");
    assert_eq!(filters[0]["values"], json!(["shipped"]));
}

#[tokio::test]
async fn converts_a_query_over_a_view() {
    let api = api().await;

    let converted = rest4sql(api.services(), "SELECT MEASURE(count) FROM sales", None)
        .await
        .expect("conversion runs");

    let ConvertedQuery::Ok { query, .. } = converted else {
        panic!("expected a converted query, got {converted:?}");
    };
    let query = serde_json::to_value(&*query).expect("serializes");
    assert_eq!(query["measures"], json!(["sales.count"]));
}

#[tokio::test]
async fn a_statement_that_is_not_a_cube_scan_is_reported_in_the_body() {
    let api = api().await;

    let converted = rest4sql(api.services(), "SELECT 1", None)
        .await
        .expect("conversion runs");

    // The Node.js endpoint answers with an error *body*, not a failure.
    let ConvertedQuery::Error { status, error } = converted else {
        panic!("expected an error body, got {converted:?}");
    };
    assert_eq!(status, "error");
    assert_eq!(
        error,
        "Provided sql query can not be converted to rest query."
    );
}

#[tokio::test]
async fn an_unparseable_statement_is_an_error() {
    let api = api().await;

    let result = rest4sql(api.services(), "this is not sql", None).await;
    assert!(result.is_err(), "{result:?}");
}

#[tokio::test]
async fn an_unknown_member_is_an_error() {
    let api = api().await;

    let result = rest4sql(api.services(), "SELECT nope FROM orders", None).await;
    assert!(result.is_err(), "{result:?}");
}

#[tokio::test]
async fn sql4sql_reports_the_statement_sent_to_the_data_source() {
    let api = api().await;

    let plan = sql4sql(
        api.services(),
        "SELECT status, MEASURE(count) FROM orders GROUP BY 1",
        None,
    )
    .await
    .expect("planning runs");

    let Sql4SqlPlan::Ok { sql, .. } = plan else {
        panic!("expected a planned statement, got {plan:?}");
    };
    assert!(sql.to_lowercase().contains("from"), "{sql}");
    assert!(sql.contains("public.orders"), "{sql}");
}

#[tokio::test]
async fn sql4sql_reports_an_unplannable_statement() {
    let api = api().await;

    let plan = sql4sql(api.services(), "SELECT 1", None)
        .await
        .expect("planning runs");

    let Sql4SqlPlan::Error { error } = plan else {
        panic!("expected an error, got {plan:?}");
    };
    assert!(!error.is_empty());
}

#[tokio::test]
async fn the_security_context_reaches_the_conversion() {
    let api = api().await;

    // A security context must not change whether a statement converts; it
    // changes the SQL the planner produces, which `sql4sql` reports.
    let converted = rest4sql(
        api.services(),
        "SELECT MEASURE(count) FROM orders",
        Some(json!({ "tenant_id": 7 })),
    )
    .await
    .expect("conversion runs");

    assert!(converted.is_ok(), "{converted:?}");
}
