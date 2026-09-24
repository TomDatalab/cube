//! `/v1/meta` served from a YAML data model, with no Node.js involved:
//! `cubemodel` compiles the model and `cubeserver` serves the REST contract
//! of `ApiGateway.meta`.

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use cubeserver::meta_adapter::ModelMetaService;
use cubeserver::services::{
    AlwaysHealthy, AuthService, AuthenticatedRequest, UnimplementedQueryService,
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
        format: currency
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
views:
  - name: orders_view
    cubes:
      - join_path: orders
        includes: "*"
        excludes:
          - id
"#;

#[derive(Debug)]
struct AllowAll {
    playground: bool,
}

#[async_trait]
impl AuthService for AllowAll {
    async fn authenticate(&self, _: Option<&str>) -> Result<AuthenticatedRequest, ApiError> {
        Ok(AuthenticatedRequest {
            security_context: json!({}),
            signed_with_playground_auth_secret: self.playground,
        })
    }

    async fn api_scopes(&self, _: &Value) -> Result<Vec<String>, ApiError> {
        Ok(vec!["meta".to_string(), "data".to_string()])
    }
}

/// Writes the model to a temporary directory and builds an app serving it.
fn app(dev_mode: bool, playground: bool) -> (tempfile::TempDir, axum::Router) {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("orders.yml"), MODEL).expect("write model");

    let meta = ModelMetaService::load(dir.path(), dev_mode).expect("model compiles");
    let router = build_app(AppState {
        config: Arc::new(ServerConfig::default()),
        query_config: Arc::new(cubequery::QueryConfig::default()),
        auth: Arc::new(AllowAll { playground }),
        meta: Arc::new(meta),
        health: Arc::new(AlwaysHealthy),
        query: Arc::new(UnimplementedQueryService),
        pre_aggregations: Arc::new(cubeserver::services::UnimplementedPreAggregationService),
        sql_conversion: Arc::new(cubeserver::services::UnimplementedSqlConversionService),
        graphql: Default::default(),
        ws: Default::default(),
        tenants: None,
    });

    (dir, router)
}

async fn meta(router: axum::Router, uri: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .uri(uri)
        .header(header::AUTHORIZATION, "Bearer any")
        .body(Body::empty())
        .unwrap();
    let res = router.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

fn names(cubes: &Value) -> Vec<String> {
    cubes
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn serves_cubes_and_views_from_yaml() {
    let (_dir, router) = app(false, false);
    let (status, body) = meta(router, "/cube/v1/meta").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(names(&body["cubes"]), vec!["orders", "orders_view"]);

    let orders = &body["cubes"][0];
    assert_eq!(orders["type"], "cube");
    assert_eq!(orders["title"], "Orders");
    assert_eq!(
        names(&orders["measures"]),
        vec!["orders.count", "orders.total_amount"]
    );
    assert_eq!(orders["measures"][0]["aggType"], "count");
    assert_eq!(orders["measures"][1]["format"], "currency");

    // A primary key is hidden by default, like `CubeToMetaTransformer`.
    assert_eq!(
        names(&orders["dimensions"]),
        vec!["orders.status", "orders.created_at"]
    );
    assert_eq!(orders["dimensions"][1]["type"], "time");

    // The view resolved `includes: "*"` minus the excluded primary key.
    let view = &body["cubes"][1];
    assert_eq!(view["type"], "view");
    assert_eq!(
        names(&view["dimensions"]),
        vec!["orders_view.status", "orders_view.created_at"]
    );
}

#[tokio::test]
async fn only_views_filters_the_response() {
    let (_dir, router) = app(false, false);
    let (status, body) = meta(router, "/cube/v1/meta?onlyViews=true").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(names(&body["cubes"]), vec!["orders_view"]);
}

#[tokio::test]
async fn hidden_members_appear_in_dev_mode_and_for_playground_tokens() {
    for (dev_mode, playground) in [(true, false), (false, true)] {
        let (_dir, router) = app(dev_mode, playground);
        let (status, body) = meta(router, "/cube/v1/meta").await;

        assert_eq!(status, StatusCode::OK);
        assert!(
            names(&body["cubes"][0]["dimensions"]).contains(&"orders.id".to_string()),
            "dev_mode={dev_mode} playground={playground}"
        );
    }
}

#[tokio::test]
async fn a_javascript_model_is_rejected_with_a_clear_error() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("orders.yml"), MODEL).expect("write model");
    std::fs::write(dir.path().join("legacy.js"), "cube('X', {});").expect("write js");

    let err =
        ModelMetaService::load(dir.path(), false).expect_err("JavaScript models are not supported");
    let message = err.to_string();
    assert!(message.contains("legacy.js"), "{message}");
    assert!(message.contains("not supported"), "{message}");
}
