//! Multi-tenancy and runtime model reload.
//!
//! `cube.yml` selects a model, a data source and a cache prefix per security
//! context, replacing `contextToAppId` / `repositoryFactory` / `driverFactory`
//! from `cube.js`. A tenant's model is compiled on first use and recompiled
//! after its files change.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use cubeplanner::Dialect;
use cubeserver::services::{
    AlwaysHealthy, AuthService, AuthenticatedRequest, MetaService, RequestContext,
    UnimplementedPreAggregationService, UnimplementedQueryService,
    UnimplementedSqlConversionService,
};
use cubeserver::tenants::TenantRegistry;
use cubeserver::{build_app, ApiError, AppState, ServerConfig};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const ORDERS_MODEL: &str = r#"
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
"#;

const VISITS_MODEL: &str = r#"
cubes:
  - name: visits
    sql_table: public.visits
    measures:
      - name: count
        type: count
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
"#;

/// Hands back whatever security context the token spells, so a test can pick
/// the tenant.
#[derive(Debug)]
struct TenantAuth;

#[async_trait]
impl AuthService for TenantAuth {
    async fn authenticate(
        &self,
        authorization: Option<&str>,
    ) -> Result<AuthenticatedRequest, ApiError> {
        let tenant = authorization
            .and_then(|value| value.strip_prefix("Bearer "))
            .unwrap_or_default();

        Ok(AuthenticatedRequest {
            security_context: json!({ "tenant_id": tenant }),
            signed_with_playground_auth_secret: false,
        })
    }

    async fn api_scopes(&self, _: &Value) -> Result<Vec<String>, ApiError> {
        Ok(vec!["meta".to_string(), "data".to_string()])
    }
}

/// Only reached when no tenant registry is configured.
#[derive(Debug)]
struct NeverUsedMeta;

#[async_trait]
impl MetaService for NeverUsedMeta {
    async fn meta(&self, _: &RequestContext, _: bool) -> Result<Value, ApiError> {
        panic!("the tenant's own meta service should answer");
    }
}

/// Writes two tenant models and the `cube.yml` that selects between them.
fn deployment() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");

    std::fs::create_dir_all(dir.path().join("models/acme")).unwrap();
    std::fs::write(dir.path().join("models/acme/orders.yml"), ORDERS_MODEL).unwrap();

    std::fs::create_dir_all(dir.path().join("models/globex")).unwrap();
    std::fs::write(dir.path().join("models/globex/visits.yml"), VISITS_MODEL).unwrap();

    std::fs::write(
        dir.path().join("cube.yml"),
        format!(
            r#"
version: 1
data_sources:
  default:
    type: postgres
tenants:
  claim: tenant_id
  on_missing: error
  rules:
    - match: acme
      app_id: acme
      model_path: {root}/models/acme
    - match: globex
      app_id: globex
      model_path: {root}/models/globex
      api_scopes: [meta]
"#,
            root = dir.path().display()
        ),
    )
    .unwrap();

    dir
}

fn registry(dir: &tempfile::TempDir) -> Arc<TenantRegistry> {
    let config = cubeconfig::CubeConfig::load_with_env(dir.path(), &cubeconfig::Env::new())
        .expect("valid cube.yml");

    Arc::new(TenantRegistry::new(
        Arc::new(config),
        Dialect::Postgres,
        Some("postgres".to_string()),
        1,
        false,
    ))
}

fn app(registry: Arc<TenantRegistry>) -> axum::Router {
    build_app(AppState {
        config: Arc::new(ServerConfig::default()),
        query_config: Arc::new(cubequery::QueryConfig::default()),
        auth: Arc::new(TenantAuth),
        meta: Arc::new(NeverUsedMeta),
        health: Arc::new(AlwaysHealthy),
        query: Arc::new(UnimplementedQueryService),
        pre_aggregations: Arc::new(UnimplementedPreAggregationService),
        sql_conversion: Arc::new(UnimplementedSqlConversionService),
        graphql: Default::default(),
        ws: Default::default(),
        tenants: Some(registry),
    })
}

async fn meta(router: axum::Router, token: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .uri("/cube/v1/meta")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let res = router.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

fn cube_names(body: &Value) -> Vec<String> {
    body["cubes"]
        .as_array()
        .map(|cubes| {
            cubes
                .iter()
                .map(|c| c["name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn each_tenant_sees_its_own_model() {
    let dir = deployment();
    let registry = registry(&dir);
    let router = app(registry.clone());

    let (status, body) = meta(router.clone(), "acme").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(cube_names(&body), vec!["orders"]);

    let (status, body) = meta(router, "globex").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(cube_names(&body), vec!["visits"]);

    // Both models are compiled and kept.
    assert_eq!(registry.len().await, 2);
}

#[tokio::test]
async fn a_tenant_model_is_compiled_once_and_reused() {
    let dir = deployment();
    let registry = registry(&dir);
    let router = app(registry.clone());

    let first = registry
        .resolve(&json!({ "tenant_id": "acme" }))
        .await
        .expect("resolves");
    let (status, _) = meta(router, "acme").await;
    assert_eq!(status, StatusCode::OK);

    let second = registry
        .resolve(&json!({ "tenant_id": "acme" }))
        .await
        .expect("resolves");

    assert_eq!(first.generation, second.generation);
    assert!(Arc::ptr_eq(&first, &second), "the runtime was rebuilt");
    assert_eq!(registry.len().await, 1);
}

#[tokio::test]
async fn an_unknown_tenant_is_rejected() {
    let dir = deployment();
    let registry = registry(&dir);
    let router = app(registry);

    let (status, body) = meta(router, "nobody").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

#[tokio::test]
async fn a_tenant_rule_can_narrow_the_api_scopes() {
    let dir = deployment();
    let registry = registry(&dir);
    let router = app(registry);

    // `globex` is limited to `meta`, so a data request is refused even though
    // the auth service grants `data`.
    let req = Request::builder()
        .method("POST")
        .uri("/cube/v1/load")
        .header(header::AUTHORIZATION, "Bearer globex")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "query": { "measures": ["visits.count"] } }).to_string(),
        ))
        .unwrap();
    let res = router.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"], "API scope is missing: data");

    // `acme` keeps both scopes, so it gets past the check.
    let req = Request::builder()
        .method("POST")
        .uri("/cube/v1/load")
        .header(header::AUTHORIZATION, "Bearer acme")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "query": { "measures": ["orders.count"] } }).to_string(),
        ))
        .unwrap();
    let res = router.oneshot(req).await.unwrap();
    assert_ne!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_changed_model_is_reloaded() {
    let dir = deployment();
    let registry = registry(&dir);
    let router = app(registry.clone());

    let (_, body) = meta(router.clone(), "acme").await;
    assert_eq!(cube_names(&body), vec!["orders"]);
    let before = registry
        .resolve(&json!({ "tenant_id": "acme" }))
        .await
        .unwrap()
        .generation;

    // Nothing changed, so nothing is recompiled.
    assert!(registry.reload_changed().await.is_empty());
    assert_eq!(
        registry
            .resolve(&json!({ "tenant_id": "acme" }))
            .await
            .unwrap()
            .generation,
        before
    );

    // A file-system timestamp needs to move for the change to be visible.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    std::fs::write(
        dir.path().join("models/acme/orders.yml"),
        format!(
            "{ORDERS_MODEL}\n      - name: status\n        sql: status\n        type: string\n"
        ),
    )
    .unwrap();

    assert!(registry.reload_changed().await.is_empty());

    let (status, body) = meta(router, "acme").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let dimensions = body["cubes"][0]["dimensions"]
        .as_array()
        .expect("dimensions")
        .iter()
        .map(|d| d["name"].as_str().unwrap_or_default().to_string())
        .collect::<Vec<_>>();
    assert!(
        dimensions.contains(&"orders.status".to_string()),
        "{dimensions:?}"
    );

    assert!(
        registry
            .resolve(&json!({ "tenant_id": "acme" }))
            .await
            .unwrap()
            .generation
            > before
    );
}

#[tokio::test]
async fn a_model_that_stops_compiling_keeps_serving_the_previous_one() {
    let dir = deployment();
    let registry = registry(&dir);
    let router = app(registry.clone());

    let (status, body) = meta(router.clone(), "acme").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    tokio::time::sleep(Duration::from_millis(1100)).await;
    std::fs::write(
        dir.path().join("models/acme/orders.yml"),
        "cubes:\n  - name: broken\n    measures: not a list\n",
    )
    .unwrap();

    let failures = registry.reload_changed().await;
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert_eq!(failures[0].0, "acme");

    // The previous model still answers.
    let (status, body) = meta(router, "acme").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(cube_names(&body), vec!["orders"]);
}

#[tokio::test]
async fn reload_all_recompiles_even_without_a_change() {
    let dir = deployment();
    let registry = registry(&dir);

    let before = registry
        .resolve(&json!({ "tenant_id": "acme" }))
        .await
        .unwrap()
        .generation;

    assert!(registry.reload_all().await.is_empty());

    let after = registry
        .resolve(&json!({ "tenant_id": "acme" }))
        .await
        .unwrap()
        .generation;
    assert!(after > before, "{before} -> {after}");
}

/// A reload must move the model *and* the planner together.
///
/// Reloading only the meta service left `/v1/meta` describing a measure that
/// `/v1/load` then refused with "Cannot resolve", which is worse than not
/// reloading at all. The registry rebuilds the whole runtime, so the two can
/// never disagree.
#[tokio::test]
async fn a_reload_moves_the_model_and_the_planner_together() {
    let dir = deployment();
    let registry = registry(&dir);
    let router = app(registry.clone());

    let (status, body) = meta(router.clone(), "acme").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let before = measure_names(&body);
    assert!(
        !before.contains(&"orders.revenue".to_string()),
        "{before:?}"
    );

    // A file-system timestamp has to move for the change to be visible.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    std::fs::write(
        dir.path().join("models/acme/orders.yml"),
        r#"
cubes:
  - name: orders
    sql_table: public.orders
    measures:
      - name: count
        type: count
      - name: revenue
        sql: amount
        type: sum
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
"#,
    )
    .unwrap();

    let failures = registry.reload_changed().await;
    assert!(failures.is_empty(), "{failures:?}");

    // The measure is described...
    let (status, body) = meta(router, "acme").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        measure_names(&body).contains(&"orders.revenue".to_string()),
        "{:?}",
        measure_names(&body)
    );

    // ...and the planner knows it, which is what the two-service reload broke.
    let runtime = registry
        .resolve(&json!({ "tenant_id": "acme" }))
        .await
        .expect("resolves");
    let planned = runtime
        .query
        .sql(
            &RequestContext::default(),
            normalized(json!({ "measures": ["orders.revenue"] })),
        )
        .await;
    assert!(
        planned.is_ok(),
        "the planner still holds the old model: {planned:?}"
    );
}

fn measure_names(body: &Value) -> Vec<String> {
    body["cubes"][0]["measures"]
        .as_array()
        .map(|measures| {
            measures
                .iter()
                .map(|m| m["name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Normalizes `query` the way the HTTP handlers do.
fn normalized(query: Value) -> cubeserver::services::NormalizedRequest {
    let config = cubequery::QueryConfig::default();
    let (query_type, queries) =
        cubequery::get_normalized_queries(&query, false, None, &config).expect("valid query");
    let pivot_query = cubequery::get_pivot_query(query_type, &queries).expect("pivot");

    cubeserver::services::NormalizedRequest {
        query_type,
        queries,
        pivot_query,
    }
}

/// A tenant is planned in its own data source's dialect, and a data source
/// whose type has no dialect is refused by name instead of being planned as
/// Postgres.
#[tokio::test]
async fn a_tenant_is_planned_in_its_data_sources_dialect() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::create_dir_all(dir.path().join("models/acme")).unwrap();
    std::fs::write(dir.path().join("models/acme/orders.yml"), ORDERS_MODEL).unwrap();
    std::fs::write(
        dir.path().join("cube.yml"),
        format!(
            r#"
version: 1
data_sources:
  default:
    type: postgres
  warehouse:
    type: mysql
  legacy:
    type: jdbc
tenants:
  claim: tenant_id
  on_missing: error
  rules:
    - match: acme
      app_id: acme
      model_path: {root}/models/acme
      data_source: warehouse
    - match: globex
      app_id: globex
      model_path: {root}/models/acme
      data_source: legacy
"#,
            root = dir.path().display()
        ),
    )
    .unwrap();

    let config = cubeconfig::CubeConfig::load_with_env(dir.path(), &cubeconfig::Env::new())
        .expect("valid cube.yml");
    let (dialects, unplannable) = cubeserver::tenants::data_source_dialects(&config);
    assert_eq!(dialects.get("default"), Some(&Dialect::Postgres));
    assert_eq!(dialects.get("warehouse"), Some(&Dialect::MySql));
    assert_eq!(unplannable.len(), 1, "{unplannable:?}");
    assert_eq!(unplannable[0].0, "legacy");
    assert!(unplannable[0].1.contains("`jdbc`"), "{unplannable:?}");

    let registry = Arc::new(TenantRegistry::new(
        Arc::new(config),
        Dialect::Postgres,
        Some("postgres".to_string()),
        1,
        false,
    ));
    let router = app(registry.clone());

    let sql = |token: &'static str| {
        let router = router.clone();
        async move {
            let req = Request::builder()
                .method("POST")
                .uri("/cube/v1/sql")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "query": { "measures": ["orders.count"] } }).to_string(),
                ))
                .unwrap();
            let res = router.oneshot(req).await.unwrap();
            let status = res.status();
            let bytes = res.into_body().collect().await.unwrap().to_bytes();
            (status, serde_json::from_slice::<Value>(&bytes).unwrap())
        }
    };

    let (status, body) = sql("acme").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rendered = body["sql"]["sql"][0].as_str().expect("sql string");
    assert!(
        rendered.contains("`orders`"),
        "MySQL quoting expected: {rendered}"
    );

    let (status, body) = sql("globex").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    let error = body["error"].as_str().unwrap_or_default();
    assert!(error.contains("legacy"), "{body}");
    assert!(error.contains("`jdbc`"), "{body}");
}

/// A deployment whose own database type has no dialect refuses every tenant
/// that falls back to it, rather than planning it as Postgres.
#[tokio::test]
async fn an_unplannable_default_refuses_its_tenants() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::create_dir_all(dir.path().join("models/acme")).unwrap();
    std::fs::write(dir.path().join("models/acme/orders.yml"), ORDERS_MODEL).unwrap();
    // No `data_sources`: the default one is described by `CUBEJS_DB_TYPE`,
    // a type the configuration accepts but the planner has no dialect for.
    std::fs::write(
        dir.path().join("cube.yml"),
        format!(
            r#"
version: 1
tenants:
  claim: tenant_id
  on_missing: error
  rules:
    - match: acme
      app_id: acme
      model_path: {root}/models/acme
"#,
            root = dir.path().display()
        ),
    )
    .unwrap();
    let config = cubeconfig::CubeConfig::load_with_env(
        dir.path(),
        &cubeconfig::Env::new().with("CUBEJS_DB_TYPE", "jdbc"),
    )
    .expect("valid cube.yml");

    // What `main` builds when the default type has no dialect.
    let registry = TenantRegistry::new(
        Arc::new(config),
        Dialect::Postgres,
        Some("jdbc".to_string()),
        1,
        false,
    )
    .with_unplannable_default("no dialect for `jdbc`");
    let err = registry
        .resolve(&json!({ "tenant_id": "acme" }))
        .await
        .expect_err("the tenant has no dialect to be planned in");
    assert_eq!(err.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(err.body.error.contains("`jdbc`"), "{}", err.body.error);
    assert!(err.body.error.contains("default"), "{}", err.body.error);
}
