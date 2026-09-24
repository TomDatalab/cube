//! The bridge from the GraphQL layer back to whatever runs Cube queries.
//!
//! `graphql.ts` calls `apiGateway.load({ query, queryType, context, res,
//! apiType: 'graphql' })` from the `cube` resolver. This crate has no idea what
//! an api gateway is, so the caller supplies the callback instead.

use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

use crate::error::GraphQLError;
use crate::translate::CubeQueryJson;

/// `QueryType.REGULAR_QUERY`.
pub const REGULAR_QUERY: &str = "regularQuery";
/// The `apiType` the gateway tags GraphQL-originated loads with.
pub const API_TYPE: &str = "graphql";

/// Everything `graphql.ts` passes to `apiGateway.load`, minus the request
/// context (the caller's closure already owns that).
#[derive(Debug, Clone, PartialEq)]
pub struct CubeQueryRequest {
    /// The translated Cube REST query.
    pub query: CubeQueryJson,
    /// Always [`REGULAR_QUERY`].
    pub query_type: &'static str,
    /// `query.cache`, forwarded only when the document set it.
    pub cache: Option<String>,
    /// Always [`API_TYPE`].
    pub api_type: &'static str,
}

impl CubeQueryRequest {
    /// Build the request for a translated query.
    pub fn new(query: CubeQueryJson) -> Self {
        let cache = query.cache().map(str::to_string);
        Self {
            query,
            query_type: REGULAR_QUERY,
            cache,
            api_type: API_TYPE,
        }
    }
}

/// Runs a Cube query.
///
/// The returned JSON is the REST `/v1/load` body: `{ query, data, annotation,
/// lastRefreshTime?, usedPreAggregations? }`.
#[async_trait]
pub trait CubeQueryExecutor: Send + Sync {
    /// Execute one translated query.
    async fn load(&self, request: CubeQueryRequest) -> Result<Value, GraphQLError>;
}

/// A [`CubeQueryExecutor`] built from an async closure.
pub struct FnExecutor<F>(F);

#[async_trait]
impl<F, Fut> CubeQueryExecutor for FnExecutor<F>
where
    F: Fn(CubeQueryRequest) -> Fut + Send + Sync,
    Fut: Future<Output = Result<Value, GraphQLError>> + Send,
{
    async fn load(&self, request: CubeQueryRequest) -> Result<Value, GraphQLError> {
        (self.0)(request).await
    }
}

/// Wrap an async closure as an executor:
///
/// ```no_run
/// # use cubegraphql::{executor_fn, CubeQueryRequest};
/// # use serde_json::json;
/// let executor = executor_fn(|_req: CubeQueryRequest| async move { Ok(json!({ "data": [] })) });
/// ```
pub fn executor_fn<F, Fut>(f: F) -> Arc<dyn CubeQueryExecutor>
where
    F: Fn(CubeQueryRequest) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Value, GraphQLError>> + Send + 'static,
{
    Arc::new(FnExecutor(f))
}

/// Where the `cube` resolver stashes `annotation`, `lastRefreshTime` and
/// `usedPreAggregations` so they can be attached to the GraphQL response —
/// the Rust counterpart of `res.extensions` in `graphql.ts`.
#[derive(Clone, Default)]
pub struct ResponseExtensions(Arc<Mutex<BTreeMap<String, async_graphql::Value>>>);

impl ResponseExtensions {
    /// A fresh, empty slot.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the three keys `graphql.ts` copies out of the load result.
    pub fn record(&self, result: &Value) {
        let mut entries = BTreeMap::new();
        for key in ["annotation", "lastRefreshTime", "usedPreAggregations"] {
            if let Some(value) = result.get(key) {
                if !value.is_null() {
                    entries.insert(
                        key.to_string(),
                        async_graphql::Value::from_json(value.clone())
                            .unwrap_or(async_graphql::Value::Null),
                    );
                }
            }
        }

        if let Ok(mut slot) = self.0.lock() {
            *slot = entries;
        }
    }

    /// Take what was recorded.
    pub fn take(&self) -> BTreeMap<String, async_graphql::Value> {
        self.0
            .lock()
            .map(|mut slot| std::mem::take(&mut *slot))
            .unwrap_or_default()
    }
}
