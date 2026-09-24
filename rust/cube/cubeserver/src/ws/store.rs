//! Port of `packages/cubejs-api-gateway/src/ws/local-subscription-store.ts`:
//! the per-connection subscription state of the WebSocket transport.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::RwLock;

use crate::services::RequestContext;
use crate::ws::message::MethodMessage;

/// `heartBeatInterval` of the Node.js store.
pub const DEFAULT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct Subscription {
    /// The original message, replayed on every refresh pass.
    pub message: MethodMessage,
    /// Opaque state the handler keeps between passes.
    pub state: Value,
    pub timestamp: Instant,
}

#[derive(Debug, Default)]
struct Connection {
    subscriptions: HashMap<String, Subscription>,
    auth_context: Option<RequestContext>,
}

/// In-process subscription store.
#[derive(Debug)]
pub struct LocalSubscriptionStore {
    connections: RwLock<HashMap<String, Connection>>,
    heartbeat_interval: Duration,
}

impl Default for LocalSubscriptionStore {
    fn default() -> Self {
        Self::new(DEFAULT_HEARTBEAT_INTERVAL)
    }
}

impl LocalSubscriptionStore {
    pub fn new(heartbeat_interval: Duration) -> Self {
        Self {
            connections: RwLock::new(HashMap::new()),
            heartbeat_interval,
        }
    }

    pub async fn get_subscription(
        &self,
        connection_id: &str,
        subscription_id: &str,
    ) -> Option<Subscription> {
        // Only reads: an unknown connection is not created here.
        self.connections
            .read()
            .await
            .get(connection_id)
            .and_then(|c| c.subscriptions.get(subscription_id).cloned())
    }

    pub async fn subscribe(
        &self,
        connection_id: &str,
        subscription_id: &str,
        message: MethodMessage,
        state: Value,
    ) {
        self.connections
            .write()
            .await
            .entry(connection_id.to_string())
            .or_default()
            .subscriptions
            .insert(
                subscription_id.to_string(),
                Subscription {
                    message,
                    state,
                    timestamp: Instant::now(),
                },
            );
    }

    pub async fn unsubscribe(&self, connection_id: &str, subscription_id: &str) {
        if let Some(connection) = self.connections.write().await.get_mut(connection_id) {
            connection.subscriptions.remove(subscription_id);
        }
    }

    /// Every live subscription, dropping the ones that went stale
    /// (`heartBeatInterval * 4`, like the Node.js store).
    pub async fn all_subscriptions(&self) -> Vec<(String, Subscription)> {
        let stale_after = self.heartbeat_interval * 4;
        let now = Instant::now();
        let mut connections = self.connections.write().await;
        let mut result = Vec::new();

        for (connection_id, connection) in connections.iter_mut() {
            connection
                .subscriptions
                .retain(|_, s| now.duration_since(s.timestamp) <= stale_after);

            for subscription in connection.subscriptions.values() {
                result.push((connection_id.clone(), subscription.clone()));
            }
        }

        result
    }

    pub async fn disconnect(&self, connection_id: &str) {
        self.connections.write().await.remove(connection_id);
    }

    pub async fn auth_context(&self, connection_id: &str) -> Option<RequestContext> {
        self.connections
            .read()
            .await
            .get(connection_id)
            .and_then(|c| c.auth_context.clone())
    }

    pub async fn set_auth_context(&self, connection_id: &str, context: RequestContext) {
        self.connections
            .write()
            .await
            .entry(connection_id.to_string())
            .or_default()
            .auth_context = Some(context);
    }

    pub async fn clear(&self) {
        self.connections.write().await.clear();
    }
}

pub type SubscriptionStoreRef = Arc<LocalSubscriptionStore>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ws::message::{Method, MethodParams};
    use serde_json::json;

    fn message() -> MethodMessage {
        MethodMessage {
            method: Method::Subscribe,
            message_id: "1".to_string(),
            request_id: None,
            params: MethodParams {
                query: Some(json!({ "measures": ["a.b"] })),
                ..MethodParams::default()
            },
        }
    }

    #[tokio::test]
    async fn subscribes_and_unsubscribes() {
        let store = LocalSubscriptionStore::default();
        assert!(store.get_subscription("c1", "1").await.is_none());

        store
            .subscribe("c1", "1", message(), json!({ "n": 1 }))
            .await;
        let subscription = store.get_subscription("c1", "1").await.expect("subscribed");
        assert_eq!(subscription.state, json!({ "n": 1 }));
        assert_eq!(store.all_subscriptions().await.len(), 1);

        store.unsubscribe("c1", "1").await;
        assert!(store.get_subscription("c1", "1").await.is_none());
        assert!(store.all_subscriptions().await.is_empty());
    }

    #[tokio::test]
    async fn reading_an_unknown_connection_does_not_create_it() {
        let store = LocalSubscriptionStore::default();
        assert!(store.get_subscription("ghost", "1").await.is_none());
        assert!(store.all_subscriptions().await.is_empty());
    }

    #[tokio::test]
    async fn disconnect_drops_everything_for_a_connection() {
        let store = LocalSubscriptionStore::default();
        store.subscribe("c1", "1", message(), Value::Null).await;
        store
            .set_auth_context("c1", RequestContext::default())
            .await;

        store.disconnect("c1").await;
        assert!(store.auth_context("c1").await.is_none());
        assert!(store.all_subscriptions().await.is_empty());
    }

    #[tokio::test]
    async fn stale_subscriptions_are_dropped() {
        // A one millisecond heartbeat makes anything older than 4 ms stale.
        let store = LocalSubscriptionStore::new(Duration::from_millis(1));
        store.subscribe("c1", "1", message(), Value::Null).await;

        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(store.all_subscriptions().await.is_empty());
        assert!(store.get_subscription("c1", "1").await.is_none());
    }
}
