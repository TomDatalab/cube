//! The GraphQL API served from a YAML data model, with no JavaScript.

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use cubeserver::meta_adapter::ModelMetaService;
use cubeserver::services::{
    AlwaysHealthy, AuthService, AuthenticatedRequest, NormalizedRequest, QueryService,
    RequestContext, UnimplementedPreAggregationService,
};
use cubeserver::{build_app, ApiError, AppState, ServerConfig};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const MODEL: &str = r#"
cubes:
  - name: orders
    sql_table: public.orders
    measures:
      - name: count
        type: count
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
      - name: status
        sql: status
        type: string
      - name: created_at
        sql: created_at
        type: time
"#;

#[derive(Debug)]
struct AllowAll;

#[async_trait]
impl AuthService for AllowAll {
    async fn authenticate(&self, _: Option<&str>) -> Result<AuthenticatedRequest, ApiError> {
        Ok(AuthenticatedRequest {
            security_context: json!({}),
            signed_with_playground_auth_secret: false,
        })
    }

    async fn api_scopes(&self, _: &Value) -> Result<Vec<String>, ApiError> {
        Ok(vec!["graphql".to_string(), "data".to_string()])
    }
}

/// Returns a fixed `/v1/load` body so the GraphQL response can be checked.
#[derive(Debug)]
struct FixedLoad;

#[async_trait]
impl QueryService for FixedLoad {
    async fn load(
        &self,
        _: &RequestContext,
        request: NormalizedRequest,
    ) -> Result<Value, ApiError> {
        assert_eq!(request.queries.len(), 1);
        Ok(json!({
            "data": [{ "orders.status": "shipped", "orders.count": 10 }],
            "annotation": { "measures": {}, "dimensions": {} },
            "lastRefreshTime": "2026-01-01T00:00:00.000Z",
        }))
    }
}

fn app() -> (tempfile::TempDir, axum::Router) {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("orders.yml"), MODEL).expect("write model");

    let meta = ModelMetaService::load(dir.path(), false).expect("model compiles");
    let router = build_app(AppState {
        config: Arc::new(ServerConfig::default()),
        query_config: Arc::new(cubequery::QueryConfig::default()),
        auth: Arc::new(AllowAll),
        meta: Arc::new(meta),
        health: Arc::new(AlwaysHealthy),
        query: Arc::new(FixedLoad),
        pre_aggregations: Arc::new(UnimplementedPreAggregationService),
        sql_conversion: Arc::new(cubeserver::services::UnimplementedSqlConversionService),
        graphql: Default::default(),
        ws: Default::default(),
        tenants: None,
    });

    (dir, router)
}

async fn post(router: axum::Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::AUTHORIZATION, "Bearer any")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = router.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn graphql_to_json_translates_a_document() {
    let (_dir, router) = app();
    let (status, body) = post(
        router,
        "/cube/v1/graphql-to-json",
        json!({
            "query": "{ cube(limit: 10) { orders { count status } } }"
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let query = &body["jsonQuery"];
    assert_eq!(query["measures"], json!(["orders.count"]));
    assert_eq!(query["dimensions"], json!(["orders.status"]));
    assert_eq!(query["limit"], 10);
}

#[tokio::test]
async fn graphql_to_json_translates_filters() {
    let (_dir, router) = app();
    let (status, body) = post(
        router,
        "/cube/v1/graphql-to-json",
        json!({
            "query": r#"{ cube(where: { orders: { status: { equals: "shipped" } } }) { orders { count } } }"#
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let filters = &body["jsonQuery"]["filters"];
    assert_eq!(filters[0]["member"], "orders.status");
    assert_eq!(filters[0]["operator"], "equals");
    assert_eq!(filters[0]["values"], json!(["shipped"]));
}

#[tokio::test]
async fn graphql_runs_the_query_and_shapes_the_response() {
    let (_dir, router) = app();
    let (status, body) = post(
        router,
        "/cube/graphql",
        json!({ "query": "{ cube { orders { count status } } }" }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body["errors"].is_null(), "{body}");
    let rows = &body["data"]["cube"];
    assert_eq!(rows[0]["orders"]["status"], "shipped");
    // A measure is exposed as a Float in the schema.
    assert_eq!(rows[0]["orders"]["count"], 10.0);
    assert_eq!(
        body["extensions"]["lastRefreshTime"],
        "2026-01-01T00:00:00.000Z"
    );
}

#[tokio::test]
async fn an_invalid_document_reports_graphql_errors() {
    let (_dir, router) = app();
    let (status, body) = post(
        router,
        "/cube/graphql",
        json!({ "query": "{ cube { orders { nope } } }" }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        body["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("nope"),
        "{body}"
    );
}

#[tokio::test]
async fn graphql_to_json_answers_200_with_a_null_query_on_a_bad_document() {
    let (_dir, router) = app();
    let (status, body) = post(
        router,
        "/cube/v1/graphql-to-json",
        json!({ "query": "{ this is not graphql" }),
    )
    .await;

    // The Node.js route always answers 200 and reports the failure as a null
    // query, so clients can distinguish it from a transport error.
    assert_eq!(status, StatusCode::OK);
    assert!(body["jsonQuery"].is_null(), "{body}");
}
