use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use cubeserver::services::{
    AlwaysHealthy, AuthService, AuthenticatedRequest, HealthService, MetaService,
    NormalizedRequest, QueryService, RequestContext, UnimplementedQueryService,
};
use cubeserver::{build_app, ApiError, AppState, ServerConfig};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

#[derive(Debug)]
struct FakeAuth;

#[async_trait]
impl AuthService for FakeAuth {
    async fn authenticate(
        &self,
        authorization: Option<&str>,
    ) -> Result<AuthenticatedRequest, ApiError> {
        match authorization {
            None => Err(ApiError::forbidden("Authorization header isn't set")),
            Some("Bearer meta-only") => Ok(AuthenticatedRequest {
                security_context: json!({ "scopes": ["meta"] }),
                signed_with_playground_auth_secret: false,
            }),
            Some("Bearer playground") => Ok(AuthenticatedRequest {
                security_context: json!({}),
                signed_with_playground_auth_secret: true,
            }),
            Some("Bearer valid") => Ok(AuthenticatedRequest {
                security_context: json!({ "uid": 5 }),
                signed_with_playground_auth_secret: false,
            }),
            // `/v1/convert-query` is behind the `sql` scope, which the
            // default set does not carry.
            Some("Bearer sql-scope") => Ok(AuthenticatedRequest {
                security_context: json!({ "scopes": ["sql", "data"] }),
                signed_with_playground_auth_secret: false,
            }),
            Some(_) => Err(ApiError::forbidden("Invalid token")),
        }
    }

    async fn api_scopes(&self, security_context: &Value) -> Result<Vec<String>, ApiError> {
        Ok(security_context
            .get("scopes")
            .and_then(|s| s.as_array())
            .map(|s| {
                s.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_else(|| vec!["data".into(), "meta".into()]))
    }
}

#[derive(Debug)]
struct FakeMeta;

#[async_trait]
impl MetaService for FakeMeta {
    async fn meta(&self, ctx: &RequestContext, only_views: bool) -> Result<Value, ApiError> {
        assert!(!ctx.request_id.is_empty());
        Ok(json!({ "cubes": if only_views { vec![] } else { vec![json!({ "name": "Foo" })] } }))
    }
}

#[derive(Debug)]
struct EchoQuery;

#[async_trait]
impl QueryService for EchoQuery {
    async fn load(
        &self,
        _ctx: &RequestContext,
        request: NormalizedRequest,
    ) -> Result<Value, ApiError> {
        Ok(json!({
            "queryType": request.query_type.as_str(),
            "queries": request.queries,
        }))
    }
}

#[derive(Debug)]
struct Down;

#[async_trait]
impl HealthService for Down {
    async fn readiness(&self) -> Result<(), String> {
        Err("db is down".into())
    }
    async fn liveness(&self) -> Result<(), String> {
        Ok(())
    }
}

fn app(query: Arc<dyn QueryService>, health: Arc<dyn HealthService>) -> axum::Router {
    app_with_config(ServerConfig::default(), query, health)
}

fn app_with_config(
    config: ServerConfig,
    query: Arc<dyn QueryService>,
    health: Arc<dyn HealthService>,
) -> axum::Router {
    build_app(AppState {
        config: Arc::new(config),
        query_config: Arc::new(cubequery::QueryConfig::default()),
        auth: Arc::new(FakeAuth),
        meta: Arc::new(FakeMeta),
        health,
        query,
        pre_aggregations: Arc::new(cubeserver::services::UnimplementedPreAggregationService),
        sql_conversion: Arc::new(cubeserver::services::UnimplementedSqlConversionService),
        graphql: Default::default(),
        ws: Default::default(),
        tenants: None,
    })
}

async fn call(app: axum::Router, req: Request<Body>) -> (StatusCode, Value) {
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, body)
}

fn get(uri: &str, token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().uri(uri);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    builder.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn probes() {
    let (status, body) = call(
        app(Arc::new(UnimplementedQueryService), Arc::new(AlwaysHealthy)),
        get("/readyz", None),
    )
    .await;
    assert_eq!(
        (status, body),
        (StatusCode::OK, json!({ "health": "HEALTH" }))
    );

    let (status, body) = call(
        app(Arc::new(UnimplementedQueryService), Arc::new(Down)),
        get("/readyz", None),
    )
    .await;
    assert_eq!(
        (status, body),
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "health": "DOWN" })
        )
    );

    let (status, body) = call(
        app(Arc::new(UnimplementedQueryService), Arc::new(Down)),
        get("/livez", None),
    )
    .await;
    assert_eq!(
        (status, body),
        (StatusCode::OK, json!({ "health": "HEALTH" }))
    );
}

#[tokio::test]
async fn meta() {
    let a = || app(Arc::new(UnimplementedQueryService), Arc::new(AlwaysHealthy));

    let (status, body) = call(a(), get("/cube/v1/meta", Some("valid"))).await;
    assert_eq!(
        (status, body),
        (StatusCode::OK, json!({ "cubes": [{ "name": "Foo" }] }))
    );

    let (status, body) = call(a(), get("/cube/v1/meta?onlyViews=true", Some("valid"))).await;
    assert_eq!((status, body), (StatusCode::OK, json!({ "cubes": [] })));

    let (status, body) = call(a(), get("/cube/v1/meta?extended", Some("valid"))).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert!(body["error"].as_str().unwrap().contains("extended"));

    let (status, body) = call(a(), get("/cube/v1/meta", None)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "Authorization header isn't set");
    assert!(
        body.get("requestId").is_none(),
        "requestId is only exposed in dev mode"
    );

    let (status, body) = call(a(), get("/cube/v1/meta", Some("nope"))).await;
    assert_eq!(
        (status, body["error"].clone()),
        (StatusCode::FORBIDDEN, json!("Invalid token"))
    );
}

#[tokio::test]
async fn api_scopes_are_enforced() {
    let a = || app(Arc::new(EchoQuery), Arc::new(AlwaysHealthy));

    let (status, _) = call(a(), get("/cube/v1/meta", Some("meta-only"))).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = call(a(), get("/cube/v1/load?query=%7B%7D", Some("meta-only"))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "API scope is missing: data");
}

#[tokio::test]
async fn load_normalizes_the_query_on_get_and_post() {
    let a = || app(Arc::new(EchoQuery), Arc::new(AlwaysHealthy));

    let (status, body) = call(
        a(),
        get(
            "/cube/v1/load?query=%7B%22measures%22%3A%5B%22Foo.bar%22%5D%7D",
            Some("valid"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["queryType"], "regularQuery");
    assert_eq!(body["queries"][0]["measures"], json!(["Foo.bar"]));
    // Normalization filled in the defaults of `normalizeQuery`.
    assert_eq!(body["queries"][0]["timezone"], "UTC");
    assert_eq!(body["queries"][0]["limit"], 10000);
    assert_eq!(body["queries"][0]["rowLimit"], 10000);

    let req = Request::builder()
        .method("POST")
        .uri("/cube/v1/load")
        .header(header::AUTHORIZATION, "Bearer valid")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"query":{"measures":["Foo.bar"],"timezone":"america/new_york","limit":5}}"#,
        ))
        .unwrap();
    let (status, body) = call(a(), req).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["queries"][0]["timezone"], "America/New_York");
    assert_eq!(body["queries"][0]["limit"], 5);

    // An array of queries is a blending request, which only a client that
    // sends `queryType` can read (`gateway.ts`: "... is not supported by the
    // client").
    let blending = |query_type: &str| {
        Request::builder()
            .method("POST")
            .uri("/cube/v1/load")
            .header(header::AUTHORIZATION, "Bearer valid")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(format!(
                r#"{{"query":[{{"measures":["Foo.bar"]}},{{"measures":["Foo.baz"]}}]{query_type}}}"#
            )))
            .unwrap()
    };
    let (status, body) = call(a(), blending("")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["error"],
        "'blendingQuery' query type is not supported by the client.Please update the client."
    );

    // `queryType=multi` runs every query and wraps the results, the shape
    // `@cubejs-client/core` reads (`response.results`).
    let (status, body) = call(a(), blending(r#","queryType":"multi""#)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["queryType"], "blendingQuery");
    assert_eq!(body["slowQuery"], false);
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["queries"][0]["measures"], json!(["Foo.bar"]));
    assert_eq!(results[1]["queries"][0]["measures"], json!(["Foo.baz"]));
    assert!(body["pivotQuery"].is_object());

    // A regular query with `queryType=multi` (what the Playground sends).
    let (status, body) = call(
        a(),
        get(
            "/cube/v1/load?queryType=multi&query=%7B%22measures%22%3A%5B%22Foo.bar%22%5D%7D",
            Some("valid"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["queryType"], "regularQuery");
    assert_eq!(body["results"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn load_rejects_invalid_queries_with_the_node_messages() {
    let a = || app(Arc::new(EchoQuery), Arc::new(AlwaysHealthy));

    let (status, body) = call(a(), get("/cube/v1/load", Some("valid"))).await;
    assert_eq!(
        (status, body["error"].clone()),
        (StatusCode::BAD_REQUEST, json!("Query param is required"))
    );

    let (status, body) = call(a(), get("/cube/v1/load?query=nope", Some("valid"))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"]
        .as_str()
        .unwrap()
        .starts_with("Unable to decode query param as JSON"));

    // An empty query fails validation, not parsing.
    let (status, body) = call(a(), get("/cube/v1/load?query=%7B%7D", Some("valid"))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["error"],
        "Query should contain either measures, dimensions or timeDimensions with granularities in order to be valid"
    );
}

#[tokio::test]
async fn unimplemented_query_endpoints_answer_501() {
    let a = || app(Arc::new(UnimplementedQueryService), Arc::new(AlwaysHealthy));
    for path in ["load", "sql", "dry-run"] {
        let (status, body) = call(
            a(),
            get(
                &format!("/cube/v1/{path}?query=%7B%22measures%22%3A%5B%22Foo.bar%22%5D%7D"),
                Some("valid"),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{path}");
        assert!(body["error"].as_str().unwrap().contains(path));
    }
}

#[tokio::test]
async fn system_context_requires_playground_token() {
    let a = || app(Arc::new(UnimplementedQueryService), Arc::new(AlwaysHealthy));

    let (status, _) = call(a(), get("/cube-system/v1/context", Some("valid"))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = call(a(), get("/cube-system/v1/context", Some("playground"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["basePath"], "/cube");
}

#[tokio::test]
async fn unknown_routes_are_json_404() {
    let (status, body) = call(
        app(Arc::new(UnimplementedQueryService), Arc::new(AlwaysHealthy)),
        get("/nope", None),
    )
    .await;
    assert_eq!(
        (status, body),
        (
            StatusCode::NOT_FOUND,
            json!({ "error": "Cannot GET /nope" })
        )
    );
}

#[tokio::test]
async fn request_id_is_exposed_in_dev_mode_and_for_playground_tokens() {
    let dev = ServerConfig {
        dev_mode: true,
        ..ServerConfig::default()
    };
    let req = Request::builder()
        .uri("/cube/v1/meta")
        .header("x-request-id", "req-42")
        .body(Body::empty())
        .unwrap();
    let (status, body) = call(
        app_with_config(
            dev,
            Arc::new(UnimplementedQueryService),
            Arc::new(AlwaysHealthy),
        ),
        req,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["requestId"], "req-42");

    let req = Request::builder()
        .uri("/cube/v1/load?query=%7B%22measures%22%3A%5B%22Foo.bar%22%5D%7D")
        .header(header::AUTHORIZATION, "Bearer playground")
        .header("x-request-id", "req-43")
        .body(Body::empty())
        .unwrap();
    let (status, body) = call(
        app(Arc::new(UnimplementedQueryService), Arc::new(AlwaysHealthy)),
        req,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(body["requestId"], "req-43");
}

#[tokio::test]
async fn real_authenticator_end_to_end() {
    use cubeauth::AuthConfig;
    use cubeserver::auth_adapter::CubeAuthService;
    use jsonwebtoken::{encode, EncodingKey, Header};

    let config = AuthConfig::builder().api_secret("secret").build();
    let router = build_app(AppState {
        config: Arc::new(ServerConfig::default()),
        query_config: Arc::new(cubequery::QueryConfig::default()),
        auth: Arc::new(CubeAuthService::new(config)),
        meta: Arc::new(FakeMeta),
        health: Arc::new(AlwaysHealthy),
        query: Arc::new(UnimplementedQueryService),
        pre_aggregations: Arc::new(cubeserver::services::UnimplementedPreAggregationService),
        sql_conversion: Arc::new(cubeserver::services::UnimplementedSqlConversionService),
        graphql: Default::default(),
        ws: Default::default(),
        tenants: None,
    });

    let token = encode(
        &Header::default(),
        &json!({ "uid": 5 }),
        &EncodingKey::from_secret(b"secret"),
    )
    .unwrap();
    let (status, body) = call(router.clone(), get("/cube/v1/meta", Some(&token))).await;
    assert_eq!(
        (status, body),
        (StatusCode::OK, json!({ "cubes": [{ "name": "Foo" }] }))
    );

    // `x-cube-authorization` wins over `authorization`, like the Node.js gateway.
    let req = Request::builder()
        .uri("/cube/v1/meta")
        .header("x-cube-authorization", &token)
        .header(header::AUTHORIZATION, "Bearer garbage")
        .body(Body::empty())
        .unwrap();
    let (status, _) = call(router.clone(), req).await;
    assert_eq!(status, StatusCode::OK);

    let wrong = encode(
        &Header::default(),
        &json!({ "uid": 5 }),
        &EncodingKey::from_secret(b"other"),
    )
    .unwrap();
    let (status, body) = call(router.clone(), get("/cube/v1/meta", Some(&wrong))).await;
    assert_eq!(
        (status, body["error"].clone()),
        (StatusCode::FORBIDDEN, json!("Invalid token"))
    );

    let (status, body) = call(router.clone(), get("/cube/v1/meta", None)).await;
    assert_eq!(
        (status, body["error"].clone()),
        (
            StatusCode::FORBIDDEN,
            json!("Authorization header isn't set")
        )
    );

    let (status, body) = call(router, get("/cube/v1/meta", Some("garbage"))).await;
    assert_eq!(
        (status, body["error"].clone()),
        (StatusCode::FORBIDDEN, json!("Invalid token"))
    );
}

#[tokio::test]
async fn convert_query_validates_its_body_like_node() {
    let a = || app(Arc::new(UnimplementedQueryService), Arc::new(AlwaysHealthy));

    let post = |body: Value| {
        Request::builder()
            .method("POST")
            .uri("/cube/v1/convert-query")
            .header(header::AUTHORIZATION, "Bearer sql-scope")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };

    let (status, body) = call(
        a(),
        post(json!({ "input": "graphql", "output": "rest", "query": "x" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "Unexpected input parameter value 'graphql'");

    let (status, body) = call(
        a(),
        post(json!({ "input": "sql", "output": "json", "query": "x" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "Unexpected output parameter value 'json'");

    let (status, body) = call(
        a(),
        post(json!({ "input": "sql", "output": "rest", "query": "   " })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "query parameter must be a non-empty string");

    // A valid body reaches the service, which is not started here.
    let (status, body) = call(
        a(),
        post(json!({ "input": "sql", "output": "rest", "query": "SELECT 1" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert!(
        body["error"].as_str().unwrap().contains("SQL API"),
        "{body}"
    );
}

#[tokio::test]
async fn cubesql_requires_a_query() {
    let a = || app(Arc::new(UnimplementedQueryService), Arc::new(AlwaysHealthy));

    let req = Request::builder()
        .method("POST")
        .uri("/cube/v1/cubesql")
        .header(header::AUTHORIZATION, "Bearer valid")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({}).to_string()))
        .unwrap();
    let (status, body) = call(a(), req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("query"), "{body}");
}

#[tokio::test]
async fn convert_query_requires_the_sql_scope() {
    let a = || app(Arc::new(UnimplementedQueryService), Arc::new(AlwaysHealthy));

    let req = Request::builder()
        .method("POST")
        .uri("/cube/v1/convert-query")
        .header(header::AUTHORIZATION, "Bearer meta-only")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "input": "sql", "output": "rest", "query": "SELECT 1" }).to_string(),
        ))
        .unwrap();
    let (status, body) = call(a(), req).await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "API scope is missing: sql");
}

#[tokio::test]
async fn connectors_lists_what_this_build_can_connect_to() {
    let (status, body) = call(
        app(Arc::new(UnimplementedQueryService), Arc::new(AlwaysHealthy)),
        get("/cube/v1/connectors", Some("valid")),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let list = body["connectors"].as_array().expect("a list");
    assert_eq!(list.len(), cubedriver::KNOWN_DB_TYPES.len());

    let postgres = list
        .iter()
        .find(|c| c["type"] == "postgres")
        .expect("postgres");
    assert_eq!(postgres["implemented"], json!(true));
    // Nothing is configured in this fixture.
    assert_eq!(postgres["dataSources"], json!([]));

    let athena = list.iter().find(|c| c["type"] == "athena").expect("athena");
    assert_eq!(athena["implemented"], json!(false));
}

#[tokio::test]
async fn connectors_names_the_configured_data_sources() {
    let config = ServerConfig {
        data_sources: vec![
            ("default".to_string(), "postgres".to_string()),
            ("warehouse".to_string(), "snowflake".to_string()),
        ],
        ..ServerConfig::default()
    };

    let (status, body) = call(
        app_with_config(
            config,
            Arc::new(UnimplementedQueryService),
            Arc::new(AlwaysHealthy),
        ),
        get("/cube/v1/connectors?onlyConfigured=true", Some("valid")),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["connectors"],
        json!([
            { "type": "postgres", "implemented": true, "dataSources": ["default"] },
            { "type": "snowflake", "implemented": true, "dataSources": ["warehouse"] },
        ])
    );
}

#[tokio::test]
async fn connectors_needs_a_token_and_the_meta_scope() {
    let a = || app(Arc::new(UnimplementedQueryService), Arc::new(AlwaysHealthy));

    let (status, _) = call(a(), get("/cube/v1/connectors", None)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // `sql-scope` carries `sql` and `data`, not `meta`.
    let (status, _) = call(a(), get("/cube/v1/connectors", Some("sql-scope"))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = call(a(), get("/cube/v1/connectors", Some("meta-only"))).await;
    assert_eq!(status, StatusCode::OK);
}
