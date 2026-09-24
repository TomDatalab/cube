//! Port of `packages/cubejs-api-gateway/src/ws/subscription-server.ts`.
//!
//! The server owns no transport: [`WsConnection`] is whatever can deliver a
//! JSON message to one client, so the same logic serves an axum WebSocket and
//! the tests.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::app::AppState;
use crate::error::ApiError;
use crate::services::{NormalizedRequest, RequestContext};
use crate::ws::message::{parse_message, MessageError, Method, MethodMessage, WsMessage};
use crate::ws::store::{LocalSubscriptionStore, SubscriptionStoreRef};

/// Delivers messages to one connected client.
#[async_trait]
pub trait WsConnection: Send + Sync + std::fmt::Debug {
    async fn send(&self, connection_id: &str, message: Value);
}

/// Serves the WebSocket protocol on top of the REST services.
#[derive(Debug)]
pub struct SubscriptionServer {
    state: AppState,
    store: SubscriptionStoreRef,
    connection: Arc<dyn WsConnection>,
}

impl SubscriptionServer {
    pub fn new(state: AppState, connection: Arc<dyn WsConnection>) -> Self {
        Self::with_store(
            state,
            connection,
            Arc::new(LocalSubscriptionStore::default()),
        )
    }

    pub fn with_store(
        state: AppState,
        connection: Arc<dyn WsConnection>,
        store: SubscriptionStoreRef,
    ) -> Self {
        Self {
            state,
            store,
            connection,
        }
    }

    pub fn store(&self) -> &SubscriptionStoreRef {
        &self.store
    }

    /// Entry point for a frame received from a client.
    pub async fn process_message(&self, connection_id: &str, body: &str) {
        let value: Value = match serde_json::from_str(body) {
            Ok(value) => value,
            Err(e) => {
                let err = MessageError::invalid_json(e.to_string());
                self.send_error(connection_id, None, 400, err.title).await;
                return;
            }
        };

        let message = match parse_message(&value) {
            Ok(message) => message,
            Err(err) => {
                // The Node.js server answers with the title, keeping the
                // detail for its own log.
                let message_id = value
                    .get("messageId")
                    .and_then(|v| crate::ws::message::parse_message_id(v).ok());
                tracing::debug!(detail = %err.detail, "{}", err.title);
                self.send_error(connection_id, message_id, 400, err.title)
                    .await;
                return;
            }
        };

        self.handle_message(connection_id, message, false).await;
    }

    async fn handle_message(&self, connection_id: &str, message: WsMessage, is_subscription: bool) {
        match message {
            WsMessage::Auth { authorization } => {
                self.handle_auth(connection_id, &authorization).await;
            }
            WsMessage::Unsubscribe { message_id } => {
                self.store.unsubscribe(connection_id, &message_id).await;
            }
            WsMessage::Method(method) => {
                self.handle_method(connection_id, method, is_subscription)
                    .await;
            }
        }
    }

    async fn handle_auth(&self, connection_id: &str, authorization: &str) {
        match self.state.auth.authenticate(Some(authorization)).await {
            Ok(authenticated) => {
                self.store
                    .set_auth_context(
                        connection_id,
                        RequestContext {
                            request_id: String::new(),
                            security_context: authenticated.security_context,
                            signed_with_playground_auth_secret: authenticated
                                .signed_with_playground_auth_secret,
                        },
                    )
                    .await;
                self.connection
                    .send(connection_id, json!({ "handshake": true }))
                    .await;
            }
            Err(err) => {
                self.connection
                    .send(
                        connection_id,
                        json!({ "message": { "error": err.body.error }, "status": err.status.as_u16() }),
                    )
                    .await;
            }
        }
    }

    async fn handle_method(
        &self,
        connection_id: &str,
        message: MethodMessage,
        is_subscription: bool,
    ) {
        let Some(auth_context) = self.store.auth_context(connection_id).await else {
            self.send_error(
                connection_id,
                Some(message.message_id.clone()),
                403,
                "Not authorized",
            )
            .await;
            return;
        };

        let subscription_id = message.message_id.clone();
        let base_request_id = message
            .request_id
            .clone()
            .unwrap_or_else(|| format!("{connection_id}-{subscription_id}"));
        let ctx = RequestContext {
            request_id: format!("{base_request_id}-span-{}", Uuid::new_v4()),
            ..auth_context
        };

        let result = self
            .dispatch(&ctx, &message, connection_id, is_subscription)
            .await;

        match result {
            Ok(Some(payload)) => {
                self.connection
                    .send(
                        connection_id,
                        json!({
                            "messageId": message.message_id,
                            "message": payload,
                            "status": 200,
                        }),
                    )
                    .await;
                self.connection
                    .send(
                        connection_id,
                        json!({ "messageProcessedId": message.message_id }),
                    )
                    .await;
            }
            // `unsubscribe` answers with the processed marker only.
            Ok(None) => {
                self.connection
                    .send(
                        connection_id,
                        json!({ "messageProcessedId": message.message_id }),
                    )
                    .await;
            }
            Err(err) => {
                self.connection
                    .send(
                        connection_id,
                        json!({
                            "messageId": message.message_id,
                            "message": { "error": err.body.error },
                            "status": err.status.as_u16(),
                        }),
                    )
                    .await;
            }
        }
    }

    /// Routes one method to the same services the REST handlers use.
    async fn dispatch(
        &self,
        ctx: &RequestContext,
        message: &MethodMessage,
        connection_id: &str,
        is_subscription: bool,
    ) -> Result<Option<Value>, ApiError> {
        match message.method {
            Method::Meta => {
                self.assert_scope(ctx, "meta").await?;
                let only_views = message.params.only_views.unwrap_or(false);
                let meta = self.state.meta_for(ctx).await?;
                Ok(Some(meta.meta(ctx, only_views).await?))
            }
            Method::Unsubscribe => {
                self.store
                    .unsubscribe(connection_id, &message.message_id)
                    .await;
                Ok(None)
            }
            Method::Load | Method::Sql | Method::DryRun | Method::Subscribe => {
                let scope = if message.method == Method::Sql {
                    "sql"
                } else {
                    "data"
                };
                self.assert_scope(ctx, scope).await?;

                let request = self.normalize(message)?;
                let query = self.state.query_for(ctx).await?;
                let response = match message.method {
                    Method::Sql => query.sql(ctx, request).await?,
                    Method::DryRun => query.dry_run(ctx, request).await?,
                    // `subscribe` runs the same load, and re-runs it on every
                    // refresh pass.
                    _ => {
                        crate::load_response::load(
                            &self.state,
                            ctx,
                            request,
                            message.params.query_type.as_deref(),
                        )
                        .await?
                    }
                };

                if message.method == Method::Subscribe && !is_subscription {
                    self.store
                        .subscribe(
                            connection_id,
                            &message.message_id,
                            message.clone(),
                            Value::Null,
                        )
                        .await;
                }

                Ok(Some(response))
            }
        }
    }

    fn normalize(&self, message: &MethodMessage) -> Result<NormalizedRequest, ApiError> {
        let query = message.params.query.clone().unwrap_or(Value::Null);
        let cache_mode = message
            .params
            .cache_mode
            .as_deref()
            .map(|mode| {
                serde_json::from_value(Value::String(mode.to_string()))
                    .map_err(|_| ApiError::bad_request(format!("Invalid cache mode: {mode}")))
            })
            .transpose()?;

        let (query_type, queries) =
            cubequery::get_normalized_queries(&query, false, cache_mode, &self.state.query_config)
                .map_err(query_error)?;
        let pivot_query = cubequery::get_pivot_query(query_type, &queries).map_err(query_error)?;

        Ok(NormalizedRequest {
            query_type,
            queries,
            pivot_query,
        })
    }

    async fn assert_scope(&self, ctx: &RequestContext, scope: &str) -> Result<(), ApiError> {
        let scopes = self.state.api_scopes_for(ctx).await?;
        if scopes.iter().any(|s| s == scope) {
            Ok(())
        } else {
            Err(ApiError::forbidden(format!(
                "API scope is missing: {scope}"
            )))
        }
    }

    async fn send_error(
        &self,
        connection_id: &str,
        message_id: Option<String>,
        status: u16,
        error: &str,
    ) {
        self.connection
            .send(
                connection_id,
                json!({
                    "messageId": message_id,
                    "message": { "error": error },
                    "status": status,
                }),
            )
            .await;
    }

    /// Re-runs every live subscription, which is how `/v1/subscribe` clients
    /// receive updates.
    pub async fn process_subscriptions(&self) {
        for (connection_id, subscription) in self.store.all_subscriptions().await {
            self.handle_message(
                &connection_id,
                WsMessage::Method(subscription.message),
                true,
            )
            .await;
        }
    }

    pub async fn disconnect(&self, connection_id: &str) {
        self.store.disconnect(connection_id).await;
    }
}

fn query_error(err: cubequery::QueryError) -> ApiError {
    if err.is_user_error() {
        ApiError::bad_request(err.message())
    } else {
        ApiError::new(axum::http::StatusCode::INTERNAL_SERVER_ERROR, err.message())
    }
}
