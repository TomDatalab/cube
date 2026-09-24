//! The axum WebSocket endpoint that drives [`SubscriptionServer`].
//!
//! Node.js runs `processSubscriptions` on a timer so every live subscription
//! is re-evaluated; the same loop runs here per process.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use futures::stream::StreamExt;
use futures::SinkExt;
use serde_json::Value;
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

use crate::app::AppState;
use crate::ws::server::{SubscriptionServer, WsConnection};
use crate::ws::store::{LocalSubscriptionStore, SubscriptionStoreRef};

/// How often every live subscription is re-run (`processSubscriptions`).
pub const SUBSCRIPTION_INTERVAL: Duration = Duration::from_secs(5);

/// Routes outgoing messages to the socket of each connection.
#[derive(Debug, Default)]
pub struct ConnectionRegistry {
    senders: RwLock<HashMap<String, mpsc::UnboundedSender<Value>>>,
}

impl ConnectionRegistry {
    async fn register(&self, connection_id: &str, sender: mpsc::UnboundedSender<Value>) {
        self.senders
            .write()
            .await
            .insert(connection_id.to_string(), sender);
    }

    async fn remove(&self, connection_id: &str) {
        self.senders.write().await.remove(connection_id);
    }
}

#[async_trait]
impl WsConnection for ConnectionRegistry {
    async fn send(&self, connection_id: &str, message: Value) {
        if let Some(sender) = self.senders.read().await.get(connection_id) {
            // A closed receiver means the client is gone; the socket task
            // cleans the registry up.
            let _ = sender.send(message);
        }
    }
}

/// Shared WebSocket state: one subscription store and registry per process.
#[derive(Clone, Debug)]
pub struct WsState {
    registry: Arc<ConnectionRegistry>,
    store: SubscriptionStoreRef,
}

impl Default for WsState {
    fn default() -> Self {
        Self {
            registry: Arc::new(ConnectionRegistry::default()),
            store: Arc::new(LocalSubscriptionStore::default()),
        }
    }
}

impl WsState {
    pub fn store(&self) -> &SubscriptionStoreRef {
        &self.store
    }

    fn server(&self, state: AppState) -> SubscriptionServer {
        SubscriptionServer::with_store(state, self.registry.clone(), self.store.clone())
    }

    /// Starts the loop that re-runs live subscriptions.
    pub fn spawn_subscription_loop(&self, state: AppState, interval: Duration) {
        let server = self.server(state);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            // The first tick fires immediately; skip it.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                server.process_subscriptions().await;
            }
        });
    }
}

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    let ws_state = state.ws.clone();
    ws.on_upgrade(move |socket| handle_socket(socket, state, ws_state))
}

async fn handle_socket(socket: WebSocket, state: AppState, ws_state: WsState) {
    let connection_id = Uuid::new_v4().to_string();
    let (mut sink, mut stream) = socket.split();
    let (sender, mut outgoing) = mpsc::unbounded_channel::<Value>();

    ws_state.registry.register(&connection_id, sender).await;

    // Outgoing messages are serialized on their own task so a slow client
    // never blocks message handling.
    let writer = tokio::spawn(async move {
        while let Some(message) = outgoing.recv().await {
            if sink.send(Message::Text(message.to_string())).await.is_err() {
                break;
            }
        }
    });

    let server = ws_state.server(state);

    while let Some(Ok(message)) = stream.next().await {
        match message {
            Message::Text(text) => server.process_message(&connection_id, &text).await,
            Message::Close(_) => break,
            // Ping/Pong are answered by axum; binary frames are not part of
            // the protocol.
            _ => {}
        }
    }

    server.disconnect(&connection_id).await;
    ws_state.registry.remove(&connection_id).await;
    writer.abort();
}
