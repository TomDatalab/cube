//! The WebSocket transport: protocol, authentication and subscriptions,
//! driven through `SubscriptionServer` without a real socket.

use std::sync::Arc;

use async_trait::async_trait;
use cubeserver::services::{
    AlwaysHealthy, AuthService, AuthenticatedRequest, MetaService, NormalizedRequest, QueryService,
    RequestContext, UnimplementedQueryService,
};
use cubeserver::ws::{SubscriptionServer, WsConnection};
use cubeserver::{ApiError, AppState, ServerConfig};
use serde_json::{json, Value};
use tokio::sync::Mutex;

/// Records what the server sent, per connection.
#[derive(Debug, Default)]
struct Recorder {
    sent: Mutex<Vec<(String, Value)>>,
}

impl Recorder {
    async fn messages(&self) -> Vec<Value> {
        self.sent
            .lock()
            .await
            .iter()
            .map(|(_, m)| m.clone())
            .collect()
    }

    async fn last(&self) -> Value {
        self.messages()
            .await
            .last()
            .cloned()
            .expect("a message was sent")
    }
}

#[async_trait]
impl WsConnection for Recorder {
    async fn send(&self, connection_id: &str, message: Value) {
        self.sent
            .lock()
            .await
            .push((connection_id.to_string(), message));
    }
}

#[derive(Debug)]
struct TokenAuth;

#[async_trait]
impl AuthService for TokenAuth {
    async fn authenticate(
        &self,
        authorization: Option<&str>,
    ) -> Result<AuthenticatedRequest, ApiError> {
        match authorization {
            Some("good") => Ok(AuthenticatedRequest {
                security_context: json!({ "uid": 1 }),
                signed_with_playground_auth_secret: false,
            }),
            Some("meta-only") => Ok(AuthenticatedRequest {
                security_context: json!({ "scopes": ["meta"] }),
                signed_with_playground_auth_secret: false,
            }),
            _ => Err(ApiError::forbidden("Invalid token")),
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
            .unwrap_or_else(|| vec!["data".into(), "meta".into(), "sql".into()]))
    }
}

#[derive(Debug)]
struct FakeMeta;

#[async_trait]
impl MetaService for FakeMeta {
    async fn meta(&self, _: &RequestContext, only_views: bool) -> Result<Value, ApiError> {
        Ok(json!({ "cubes": [], "onlyViews": only_views }))
    }
}

/// Counts how often `load` runs, so a subscription refresh is observable.
#[derive(Debug, Default)]
struct CountingLoad {
    calls: Mutex<usize>,
}

#[async_trait]
impl QueryService for CountingLoad {
    async fn load(
        &self,
        _: &RequestContext,
        request: NormalizedRequest,
    ) -> Result<Value, ApiError> {
        let mut calls = self.calls.lock().await;
        *calls += 1;
        Ok(json!({ "call": *calls, "measures": request.queries[0].measures }))
    }
}

fn state(query: Arc<dyn QueryService>) -> AppState {
    AppState {
        config: Arc::new(ServerConfig::default()),
        query_config: Arc::new(cubequery::QueryConfig::default()),
        auth: Arc::new(TokenAuth),
        meta: Arc::new(FakeMeta),
        health: Arc::new(AlwaysHealthy),
        query,
        pre_aggregations: Arc::new(cubeserver::services::UnimplementedPreAggregationService),
        sql_conversion: Arc::new(cubeserver::services::UnimplementedSqlConversionService),
        graphql: Default::default(),
        ws: Default::default(),
        tenants: None,
    }
}

fn server(query: Arc<dyn QueryService>) -> (Arc<Recorder>, SubscriptionServer) {
    let recorder = Arc::new(Recorder::default());
    let server = SubscriptionServer::new(state(query), recorder.clone());
    (recorder, server)
}

const CONN: &str = "conn-1";

async fn authenticate(server: &SubscriptionServer) {
    server
        .process_message(CONN, &json!({ "authorization": "good" }).to_string())
        .await;
}

#[tokio::test]
async fn authenticating_answers_with_a_handshake() {
    let (recorder, server) = server(Arc::new(UnimplementedQueryService));
    authenticate(&server).await;

    assert_eq!(recorder.last().await, json!({ "handshake": true }));
}

#[tokio::test]
async fn a_bad_token_is_rejected() {
    let (recorder, server) = server(Arc::new(UnimplementedQueryService));
    server
        .process_message(CONN, &json!({ "authorization": "nope" }).to_string())
        .await;

    let message = recorder.last().await;
    assert_eq!(message["message"]["error"], "Invalid token");
    assert_eq!(message["status"], 403);
}

#[tokio::test]
async fn a_method_before_authenticating_is_not_authorized() {
    let (recorder, server) = server(Arc::new(CountingLoad::default()));
    server
        .process_message(
            CONN,
            &json!({ "method": "load", "messageId": 1, "params": { "query": { "measures": ["a.b"] } } })
                .to_string(),
        )
        .await;

    let message = recorder.last().await;
    assert_eq!(message["message"]["error"], "Not authorized");
    assert_eq!(message["status"], 403);
    assert_eq!(message["messageId"], "1");
}

#[tokio::test]
async fn invalid_json_and_invalid_shapes_are_reported() {
    let (recorder, server) = server(Arc::new(UnimplementedQueryService));

    server.process_message(CONN, "{ not json").await;
    assert_eq!(
        recorder.last().await["message"]["error"],
        "Invalid JSON payload"
    );

    server
        .process_message(
            CONN,
            &json!({ "method": "load", "messageId": 1 }).to_string(),
        )
        .await;
    assert_eq!(
        recorder.last().await["message"]["error"],
        "Invalid message format"
    );

    server
        .process_message(
            CONN,
            &json!({ "authorization": "good", "x": 1 }).to_string(),
        )
        .await;
    assert_eq!(
        recorder.last().await["message"]["error"],
        "Invalid authorization message format"
    );
}

#[tokio::test]
async fn load_runs_the_query_and_marks_the_message_processed() {
    let load = Arc::new(CountingLoad::default());
    let (recorder, server) = server(load.clone());
    authenticate(&server).await;

    server
        .process_message(
            CONN,
            &json!({
                "method": "load",
                "messageId": 7,
                "params": { "query": { "measures": ["orders.count"] } }
            })
            .to_string(),
        )
        .await;

    let messages = recorder.messages().await;
    let result = &messages[messages.len() - 2];
    assert_eq!(result["messageId"], "7");
    assert_eq!(result["status"], 200);
    assert_eq!(result["message"]["call"], 1);
    assert_eq!(result["message"]["measures"], json!(["orders.count"]));

    assert_eq!(
        messages.last().unwrap(),
        &json!({ "messageProcessedId": "7" })
    );
}

#[tokio::test]
async fn meta_is_served_over_the_socket() {
    let (recorder, server) = server(Arc::new(UnimplementedQueryService));
    authenticate(&server).await;

    server
        .process_message(
            CONN,
            &json!({ "method": "meta", "messageId": "m", "params": { "onlyViews": true } })
                .to_string(),
        )
        .await;

    let messages = recorder.messages().await;
    let result = &messages[messages.len() - 2];
    assert_eq!(result["message"]["onlyViews"], true);
}

#[tokio::test]
async fn api_scopes_are_enforced_over_the_socket() {
    let (recorder, server) = server(Arc::new(CountingLoad::default()));
    server
        .process_message(CONN, &json!({ "authorization": "meta-only" }).to_string())
        .await;

    server
        .process_message(
            CONN,
            &json!({ "method": "load", "messageId": 1, "params": { "query": { "measures": ["a.b"] } } })
                .to_string(),
        )
        .await;

    let message = recorder.last().await;
    assert_eq!(message["message"]["error"], "API scope is missing: data");
    assert_eq!(message["status"], 403);
}

#[tokio::test]
async fn subscribe_registers_and_is_re_run_on_every_pass() {
    let load = Arc::new(CountingLoad::default());
    let (recorder, server) = server(load.clone());
    authenticate(&server).await;

    server
        .process_message(
            CONN,
            &json!({
                "method": "subscribe",
                "messageId": 3,
                "params": { "query": { "measures": ["orders.count"] } }
            })
            .to_string(),
        )
        .await;

    assert_eq!(*load.calls.lock().await, 1);
    assert_eq!(server.store().all_subscriptions().await.len(), 1);

    // A refresh pass re-runs the stored subscription.
    server.process_subscriptions().await;
    assert_eq!(*load.calls.lock().await, 2);
    let message = recorder.last().await;
    assert_eq!(message["messageProcessedId"], "3");

    // Unsubscribing stops it.
    server
        .process_message(CONN, &json!({ "unsubscribe": 3 }).to_string())
        .await;
    assert!(server.store().all_subscriptions().await.is_empty());

    server.process_subscriptions().await;
    assert_eq!(*load.calls.lock().await, 2);
}

#[tokio::test]
async fn disconnecting_drops_the_subscriptions_of_that_connection() {
    let load = Arc::new(CountingLoad::default());
    let (_recorder, server) = server(load.clone());
    authenticate(&server).await;

    server
        .process_message(
            CONN,
            &json!({ "method": "subscribe", "messageId": 1, "params": { "query": { "measures": ["a.b"] } } })
                .to_string(),
        )
        .await;
    assert_eq!(server.store().all_subscriptions().await.len(), 1);

    server.disconnect(CONN).await;
    assert!(server.store().all_subscriptions().await.is_empty());

    // The connection must authenticate again.
    server
        .process_message(
            CONN,
            &json!({ "method": "load", "messageId": 2, "params": { "query": { "measures": ["a.b"] } } })
                .to_string(),
        )
        .await;
    assert_eq!(server.store().all_subscriptions().await.len(), 0);
}

#[tokio::test]
async fn an_invalid_query_is_reported_with_the_rest_message() {
    let (recorder, server) = server(Arc::new(CountingLoad::default()));
    authenticate(&server).await;

    server
        .process_message(
            CONN,
            &json!({ "method": "load", "messageId": 1, "params": { "query": {} } }).to_string(),
        )
        .await;

    let message = recorder.last().await;
    assert_eq!(
        message["message"]["error"],
        "Query should contain either measures, dimensions or timeDimensions with granularities in order to be valid"
    );
    assert_eq!(message["status"], 400);
}
