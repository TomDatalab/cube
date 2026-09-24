//! The whole stack against a real database: HTTP → auth → normalization →
//! planning → orchestrator cache and queue → driver → rows.
//!
//! Gated on `CUBEJS_TEST_PG_URL`, e.g.
//! `postgres://postgres:test@127.0.0.1:55432/analytics`, and skipped when it
//! is unset.

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use cubeplanner::Dialect;
use cubeserver::orchestrator_adapter::{orchestrator, OrchestratedQueryService};
use cubeserver::planner_adapter::PlannerQueryService;
use cubeserver::services::{
    AlwaysHealthy, AuthService, AuthenticatedRequest, MetaService, RequestContext,
    UnimplementedPreAggregationService,
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

/// Parses `postgres://user:pass@host:port/db` into the `CUBEJS_DB_*` variables
/// the driver reads, and sets them for this process.
fn set_env_from_url(url: &str) -> Option<()> {
    let rest = url.strip_prefix("postgres://")?;
    let (credentials, host_and_db) = rest.split_once('@')?;
    let (user, password) = credentials.split_once(':')?;
    let (host_port, database) = host_and_db.split_once('/')?;
    let (host, port) = host_port.split_once(':').unwrap_or((host_port, "5432"));

    std::env::set_var("CUBEJS_DB_TYPE", "postgres");
    std::env::set_var("CUBEJS_DB_HOST", host);
    std::env::set_var("CUBEJS_DB_PORT", port);
    std::env::set_var("CUBEJS_DB_NAME", database);
    std::env::set_var("CUBEJS_DB_USER", user);
    std::env::set_var("CUBEJS_DB_PASS", password);

    Some(())
}

macro_rules! require_pg {
    () => {
        match std::env::var("CUBEJS_TEST_PG_URL") {
            Ok(url) if !url.is_empty() => url,
            _ => {
                eprintln!("skipped: CUBEJS_TEST_PG_URL is not set");
                return;
            }
        }
    };
}

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
        Ok(vec!["data".to_string(), "meta".to_string()])
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

fn app(cache_prefix: &str) -> (tempfile::TempDir, axum::Router) {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("orders.yml"), MODEL).expect("write model");

    let planner =
        Arc::new(PlannerQueryService::load(dir.path(), Dialect::Postgres, 2).expect("plans"));
    let query = OrchestratedQueryService::new(
        planner,
        orchestrator(cache_prefix),
        "default".to_string(),
        Some("postgres".to_string()),
        false,
    );

    let router = build_app(AppState {
        config: Arc::new(ServerConfig::default()),
        query_config: Arc::new(cubequery::QueryConfig::default()),
        auth: Arc::new(AllowAll),
        meta: Arc::new(NoMeta),
        health: Arc::new(AlwaysHealthy),
        query: Arc::new(query),
        pre_aggregations: Arc::new(UnimplementedPreAggregationService),
        sql_conversion: Arc::new(cubeserver::services::UnimplementedSqlConversionService),
        graphql: Default::default(),
        ws: Default::default(),
        tenants: None,
    });

    (dir, router)
}

async fn load(router: axum::Router, query: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri("/cube/v1/load")
        .header(header::AUTHORIZATION, "Bearer any")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "query": query }).to_string()))
        .unwrap();
    let res = router.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn runs_a_query_against_a_real_database() {
    let url = require_pg!();
    set_env_from_url(&url).expect("a postgres:// url");

    let (_dir, router) = app("test_load_e2e");
    let (status, body) = load(
        router,
        json!({ "measures": ["orders.count"], "dimensions": ["orders.status"] }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");

    // Rows are keyed by member, not by the SQL alias.
    let rows = body["data"].as_array().expect("rows");
    assert!(!rows.is_empty(), "{body}");
    for row in rows {
        assert!(row.get("orders.status").is_some(), "{row}");
        assert!(row.get("orders.count").is_some(), "{row}");
        assert!(row.get("orders__status").is_none(), "{row}");
    }

    assert_eq!(body["dataSource"], "default");
    assert_eq!(body["dbType"], "postgres");
    assert!(body["lastRefreshTime"].is_string(), "{body}");
    // The normalized query is echoed back, as the Node.js body does.
    assert_eq!(body["query"]["measures"], json!(["orders.count"]));
}

#[tokio::test]
async fn a_granular_time_dimension_is_keyed_with_its_granularity() {
    let url = require_pg!();
    set_env_from_url(&url).expect("a postgres:// url");

    let (_dir, router) = app("test_load_e2e_td");
    let (status, body) = load(
        router,
        json!({
            "measures": ["orders.count"],
            "timeDimensions": [{
                "dimension": "orders.created_at",
                "granularity": "month",
                "dateRange": ["2026-01-01", "2026-12-31"]
            }]
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = body["data"].as_array().expect("rows");
    assert!(!rows.is_empty(), "{body}");
    assert!(
        rows[0].get("orders.created_at.month").is_some(),
        "{:?}",
        rows[0]
    );
}

#[tokio::test]
async fn a_filter_reaches_the_database_as_a_bound_parameter() {
    let url = require_pg!();
    set_env_from_url(&url).expect("a postgres:// url");

    let (_dir, router) = app("test_load_e2e_filter");

    // The assertions are relational rather than absolute: the test reads a
    // database it does not own, so a magic row count would break whenever the
    // data changes without saying anything about the filter.
    let count_of = |router: axum::Router, filter: Option<&'static str>| async move {
        let mut query = json!({ "measures": ["orders.count"] });
        if let Some(status) = filter {
            query["filters"] = json!([{
                "member": "orders.status",
                "operator": "equals",
                "values": [status]
            }]);
        }

        let (status_code, body) = load(router, query).await;
        assert_eq!(status_code, StatusCode::OK, "{body}");

        body["data"][0]["orders.count"]
            .as_str()
            .expect("a count")
            .parse::<u64>()
            .expect("a number")
    };

    let total = count_of(router.clone(), None).await;
    let shipped = count_of(router.clone(), Some("shipped")).await;
    let missing = count_of(router, Some("no-such-status")).await;

    assert!(total > 0, "the fixture table is empty");
    assert!(shipped > 0, "no order has the status the filter asks for");
    assert!(
        shipped < total,
        "the filter matched everything: {shipped} of {total}"
    );
    assert_eq!(missing, 0, "a status nothing carries must match nothing");
}

#[tokio::test]
async fn the_second_identical_query_is_served_from_the_cache() {
    let url = require_pg!();
    set_env_from_url(&url).expect("a postgres:// url");

    let (_dir, router) = app("test_load_e2e_cache");
    let query = json!({ "measures": ["orders.count"] });

    let (status, first) = load(router.clone(), query.clone()).await;
    assert_eq!(status, StatusCode::OK, "{first}");

    let (status, second) = load(router, query).await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(first["data"], second["data"]);
}

#[tokio::test]
async fn an_unknown_member_is_reported_to_the_client() {
    let url = require_pg!();
    set_env_from_url(&url).expect("a postgres:// url");

    let (_dir, router) = app("test_load_e2e_error");
    let (status, body) = load(router, json!({ "measures": ["orders.nope"] })).await;

    assert_ne!(status, StatusCode::OK);
    assert!(body["error"].as_str().unwrap().contains("nope"), "{body}");
}
