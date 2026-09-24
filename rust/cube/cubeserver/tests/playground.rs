//! The Playground as the browser sees it: the page at `/`, its assets, the
//! bootstrap call, and the JSON 404 that must survive underneath.

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use cubeserver::services::{
    AlwaysHealthy, AuthService, AuthenticatedRequest, MetaService, RequestContext,
    UnimplementedQueryService,
};
use cubeserver::{build_app, ApiError, AppState, ServerConfig};
use http_body_util::BodyExt;
use serde_json::{json, Map, Value};
use tower::ServiceExt;

/// Issues a recognisable token, so a test can tell the app got this one.
#[derive(Debug)]
struct TokenIssuingAuth;

#[async_trait]
impl AuthService for TokenIssuingAuth {
    async fn authenticate(&self, _: Option<&str>) -> Result<AuthenticatedRequest, ApiError> {
        Ok(AuthenticatedRequest {
            security_context: json!({}),
            signed_with_playground_auth_secret: false,
        })
    }

    async fn api_scopes(&self, _: &Value) -> Result<Vec<String>, ApiError> {
        Ok(vec!["meta".to_string(), "data".to_string()])
    }

    async fn issue_token(&self, claims: Map<String, Value>) -> Result<String, ApiError> {
        Ok(format!("signed:{}", Value::Object(claims)))
    }
}

/// An authenticator with no secret, like a deployment that cannot sign.
#[derive(Debug)]
struct CannotIssue;

#[async_trait]
impl AuthService for CannotIssue {
    async fn authenticate(&self, _: Option<&str>) -> Result<AuthenticatedRequest, ApiError> {
        Ok(AuthenticatedRequest {
            security_context: json!({}),
            signed_with_playground_auth_secret: false,
        })
    }

    async fn api_scopes(&self, _: &Value) -> Result<Vec<String>, ApiError> {
        Ok(vec![])
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

/// A directory that looks like a Playground build, plus a model directory.
fn fixture() -> (tempfile::TempDir, ServerConfig) {
    let dir = tempfile::tempdir().expect("temp dir");

    let assets = dir.path().join("playground");
    std::fs::create_dir_all(assets.join("assets")).unwrap();
    std::fs::write(
        assets.join("index.html"),
        r#"<html><div id="playground-root"></div></html>"#,
    )
    .unwrap();
    std::fs::write(assets.join("assets/index.js"), "console.log('cube')").unwrap();

    let model = dir.path().join("model");
    std::fs::create_dir_all(model.join("views")).unwrap();
    std::fs::write(model.join("orders.yml"), "cubes: []").unwrap();
    std::fs::write(model.join("views/sales.yml"), "views: []").unwrap();
    std::fs::write(model.join("notes.txt"), "not a model file").unwrap();

    let config = ServerConfig {
        base_path: "/cube".to_string(),
        schema_path: model.to_string_lossy().into_owned(),
        playground_path: Some(assets.to_string_lossy().into_owned()),
        data_sources: vec![("default".to_string(), "postgres".to_string())],
        ..ServerConfig::default()
    };

    (dir, config)
}

fn app(config: ServerConfig, auth: Arc<dyn AuthService>) -> axum::Router {
    build_app(AppState {
        config: Arc::new(config),
        query_config: Arc::new(cubequery::QueryConfig::default()),
        auth,
        meta: Arc::new(NoMeta),
        health: Arc::new(AlwaysHealthy),
        query: Arc::new(UnimplementedQueryService),
        pre_aggregations: Arc::new(cubeserver::services::UnimplementedPreAggregationService),
        sql_conversion: Arc::new(cubeserver::services::UnimplementedSqlConversionService),
        graphql: Default::default(),
        ws: Default::default(),
        tenants: None,
    })
}

async fn get(router: axum::Router, path: &str) -> (StatusCode, String) {
    let request = Request::builder()
        .uri(path)
        .body(Body::empty())
        .expect("a request");
    let response = router.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();

    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn get_json(router: axum::Router, path: &str) -> (StatusCode, Value) {
    let (status, body) = get(router, path).await;
    let value = serde_json::from_str(&body).unwrap_or(Value::Null);

    (status, value)
}

#[tokio::test]
async fn the_page_is_served_at_the_root() {
    let (_dir, config) = fixture();
    let (status, body) = get(app(config, Arc::new(TokenIssuingAuth)), "/").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("playground-root"), "{body}");
}

#[tokio::test]
async fn assets_are_served_from_the_root_too() {
    let (_dir, config) = fixture();
    let (status, body) = get(app(config, Arc::new(TokenIssuingAuth)), "/assets/index.js").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "console.log('cube')");
}

/// The app derives the REST API address from `basePath`. Sending Node's
/// `/cubejs-api` would point it at nothing on this server.
#[tokio::test]
async fn the_context_names_this_servers_api_prefix_and_carries_a_token() {
    let (_dir, config) = fixture();
    let (status, body) = get_json(
        app(config, Arc::new(TokenIssuingAuth)),
        "/playground/context",
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["basePath"], "/cube");
    assert_eq!(body["cubejsToken"], "signed:{}");
    // The connection wizard writes .env and installs npm packages.
    assert_eq!(body["shouldStartConnectionWizardFlow"], json!(false));
    // Neither outbound analytics nor Cube Cloud live preview.
    assert_eq!(body["telemetry"], json!(false));
    assert_eq!(body["livePreview"], json!(false));
    assert_eq!(body["dbType"], "postgres");
}

#[tokio::test]
async fn a_deployment_that_cannot_sign_says_so_instead_of_serving_a_broken_app() {
    let (_dir, config) = fixture();
    let (status, body) = get_json(app(config, Arc::new(CannotIssue)), "/playground/context").await;

    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{body}");
}

#[tokio::test]
async fn the_file_list_holds_the_model_and_nothing_else() {
    let (_dir, config) = fixture();
    let (status, body) =
        get_json(app(config, Arc::new(TokenIssuingAuth)), "/playground/files").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let names: Vec<&str> = body["files"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|f| f["fileName"].as_str().expect("a name"))
        .collect();

    // Nested files are listed by their path relative to the model directory,
    // and a non-model file is left out.
    assert_eq!(names, vec!["orders.yml", "views/sales.yml"]);
    assert_eq!(body["files"][0]["content"], "cubes: []");
}

/// The landing page reads an empty list as "no data model yet".
#[tokio::test]
async fn a_missing_model_directory_is_an_empty_list_not_an_error() {
    let (_dir, mut config) = fixture();
    config.schema_path = "/no/such/model/directory".to_string();

    let (status, body) =
        get_json(app(config, Arc::new(TokenIssuingAuth)), "/playground/files").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["files"], json!([]));
}

#[tokio::test]
async fn the_connector_status_reflects_what_this_build_carries() {
    let (_dir, config) = fixture();
    let router = || app(config.clone(), Arc::new(TokenIssuingAuth));

    let (status, body) = get_json(router(), "/playground/driver?driver=postgres").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "installed");

    // Known to Cube, no Rust driver.
    let (_, body) = get_json(router(), "/playground/driver?driver=athena").await;
    assert_eq!(body["status"], "error");

    let (status, _) = get_json(router(), "/playground/driver?driver=nonsense").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// Serving the app at the root must not turn every mistyped API path into
/// the page; clients rely on the JSON 404.
#[tokio::test]
async fn an_unknown_path_is_still_a_json_404() {
    let (_dir, config) = fixture();
    let (status, body) = get_json(
        app(config, Arc::new(TokenIssuingAuth)),
        "/cube/v1/no-such-endpoint",
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "Cannot GET /cube/v1/no-such-endpoint");
}

#[tokio::test]
async fn nothing_is_mounted_when_no_assets_are_configured() {
    let (_dir, mut config) = fixture();
    config.playground_path = None;

    let router = || app(config.clone(), Arc::new(TokenIssuingAuth));

    let (status, body) = get_json(router(), "/").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "Cannot GET /");

    let (status, _) = get_json(router(), "/playground/context").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_directory_without_a_build_mounts_nothing() {
    let (dir, mut config) = fixture();
    config.playground_path = Some(dir.path().join("empty").to_string_lossy().into_owned());
    std::fs::create_dir_all(dir.path().join("empty")).unwrap();

    let (status, _) = get_json(app(config, Arc::new(TokenIssuingAuth)), "/").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_actions_that_need_javascript_explain_themselves() {
    let (_dir, config) = fixture();

    let request = Request::builder()
        .method("POST")
        .uri("/playground/generate-schema")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{}"))
        .unwrap();

    let response = app(config, Arc::new(TokenIssuingAuth))
        .oneshot(request)
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let message = body["error"].as_str().expect("a message");

    assert!(message.contains("YAML"), "{message}");
}

#[tokio::test]
async fn a_security_context_token_carries_the_claims_the_user_typed() {
    let (_dir, config) = fixture();

    let request = Request::builder()
        .method("POST")
        .uri("/playground/token")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "payload": { "tenant": "acme" } }).to_string(),
        ))
        .unwrap();

    let response = app(config, Arc::new(TokenIssuingAuth))
        .oneshot(request)
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(body["token"], r#"signed:{"tenant":"acme"}"#);
}
