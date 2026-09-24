//! `/v1/sql` and `/v1/dry-run` planned from a YAML data model with no
//! JavaScript: `cubequery` normalizes, `cubeplanner` (Tesseract) plans.

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use cubeplanner::Dialect;
use cubeserver::planner_adapter::PlannerQueryService;
use cubeserver::services::{
    AlwaysHealthy, AuthService, AuthenticatedRequest, MetaService, RequestContext,
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
      - name: total_amount
        type: sum
        sql: "{CUBE}.amount"
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
            security_context: json!({ "tenant_id": 7 }),
            signed_with_playground_auth_secret: false,
        })
    }

    async fn api_scopes(&self, _: &Value) -> Result<Vec<String>, ApiError> {
        Ok(vec!["data".to_string(), "sql".to_string()])
    }
}

#[derive(Debug)]
struct NoMeta;

#[async_trait]
impl MetaService for NoMeta {
    async fn meta(&self, _: &RequestContext, _: bool) -> Result<Value, ApiError> {
        Ok(json!({ "cubes": [] }))
    }
}

fn app(model: &str) -> (tempfile::TempDir, axum::Router) {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("orders.yml"), model).expect("write model");

    let query = PlannerQueryService::load(dir.path(), Dialect::Postgres, 2).expect("model plans");
    let router = build_app(AppState {
        config: Arc::new(ServerConfig::default()),
        query_config: Arc::new(cubequery::QueryConfig::default()),
        auth: Arc::new(AllowAll),
        meta: Arc::new(NoMeta),
        health: Arc::new(AlwaysHealthy),
        query: Arc::new(query),
        pre_aggregations: Arc::new(cubeserver::services::UnimplementedPreAggregationService),
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
async fn plans_a_grouped_query() {
    let (_dir, router) = app(MODEL);
    let (status, body) = post(
        router,
        "/cube/v1/sql",
        json!({ "query": { "measures": ["orders.count"], "dimensions": ["orders.status"] } }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let sql = body["sql"]["sql"][0].as_str().expect("sql string");
    assert!(sql.contains("count("), "{sql}");
    assert!(sql.contains("public.orders"), "{sql}");
    assert!(sql.contains("GROUP BY"), "{sql}");
    // The default limit of `normalizeQuery` reaches the statement.
    assert!(sql.contains("LIMIT 10000"), "{sql}");
    assert_eq!(body["sql"]["sql"][1], json!([]));
}

#[tokio::test]
async fn plans_a_time_dimension_with_filters_and_binds_parameters() {
    let (_dir, router) = app(MODEL);
    let (status, body) = post(
        router,
        "/cube/v1/sql",
        json!({ "query": {
            "measures": ["orders.total_amount"],
            "timeDimensions": [{
                "dimension": "orders.created_at",
                "granularity": "month",
                "dateRange": ["2026-01-01", "2026-03-31"]
            }],
            "filters": [{ "member": "orders.status", "operator": "equals", "values": ["shipped"] }],
            "order": { "orders.total_amount": "desc" },
            "limit": 10
        }}),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let sql = body["sql"]["sql"][0].as_str().expect("sql string");
    assert!(sql.contains("date_trunc('month'"), "{sql}");
    assert!(sql.contains("LIMIT 10"), "{sql}");
    // Values are bound, never interpolated.
    assert!(sql.contains("$1"), "{sql}");
    assert!(!sql.contains("shipped"), "{sql}");

    // The date range was resolved to day bounds by normalization.
    assert_eq!(
        body["sql"]["sql"][1],
        json!([
            "2026-01-01T00:00:00.000",
            "2026-03-31T23:59:59.999",
            "shipped"
        ])
    );
    assert_eq!(
        body["sql"]["order"],
        json!({ "orders.total_amount": "desc" })
    );
}

#[tokio::test]
async fn dry_run_returns_the_normalized_queries_and_pivot() {
    let (_dir, router) = app(MODEL);
    let (status, body) = post(
        router,
        "/cube/v1/dry-run",
        json!({ "query": { "measures": ["orders.count"], "dimensions": ["orders.status"] } }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["queryType"], "regularQuery");
    assert_eq!(
        body["normalizedQueries"][0]["measures"],
        json!(["orders.count"])
    );
    assert_eq!(body["normalizedQueries"][0]["timezone"], "UTC");
    assert!(body["pivotQuery"].is_object());
    assert!(body["queryOrder"].is_array());
}

#[tokio::test]
async fn an_unknown_member_is_a_client_error() {
    let (_dir, router) = app(MODEL);
    let (status, body) = post(
        router,
        "/cube/v1/sql",
        json!({ "query": { "measures": ["orders.nope"] } }),
    )
    .await;

    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::INTERNAL_SERVER_ERROR,
        "unexpected status {status}: {body}"
    );
    assert!(
        body["error"].as_str().unwrap().contains("nope"),
        "{}",
        body["error"]
    );
}

#[tokio::test]
async fn security_context_reaches_the_planner() {
    // `SECURITY_CONTEXT` in a cube's SQL is resolved per request, so the
    // planner must receive the authenticated context.
    let model = r#"
cubes:
  - name: orders
    sql: "SELECT * FROM public.orders WHERE {SECURITY_CONTEXT.tenant_id.filter('tenant_id')}"
    measures:
      - name: count
        type: count
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
"#;
    let (_dir, router) = app(model);
    let (status, body) = post(
        router,
        "/cube/v1/sql",
        json!({ "query": { "measures": ["orders.count"] } }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let sql = body["sql"]["sql"][0].as_str().expect("sql string");
    assert!(sql.contains("tenant_id ="), "{sql}");
    // The value is bound, not inlined.
    assert!(!sql.contains("tenant_id = 7"), "{sql}");
    assert_eq!(body["sql"]["sql"][1], json!(["7"]));
}
