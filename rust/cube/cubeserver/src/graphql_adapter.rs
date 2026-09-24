//! The GraphQL API: `POST {basePath}/v1/graphql-to-json` and
//! `{basePath}/graphql`, backed by the `cubegraphql` crate.
//!
//! The schema is built from the compiled data model and cached until the
//! model changes, like `compilerApi.getGraphQLSchema()` in Node.js.

use std::sync::Arc;

use async_trait::async_trait;
use cubegraphql::async_graphql::dynamic::Schema;
use cubegraphql::{CubeQueryExecutor, CubeQueryRequest, GraphQLError, MetaConfig};
use serde_json::Value;
use tokio::sync::RwLock;

use crate::app::AppState;
use crate::error::ApiError;
use crate::services::{NormalizedRequest, RequestContext};

/// The schema and the meta config it was built from.
struct CompiledSchema {
    schema: Schema,
    meta: MetaConfig,
    /// The meta JSON the schema was built from, used to detect a change.
    source: Value,
}

/// Builds and caches the GraphQL schema for the current data model.
pub struct GraphQLSchemaCache {
    compiled: RwLock<Option<CompiledSchema>>,
}

impl std::fmt::Debug for GraphQLSchemaCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphQLSchemaCache").finish_non_exhaustive()
    }
}

impl Default for GraphQLSchemaCache {
    fn default() -> Self {
        Self {
            compiled: RwLock::new(None),
        }
    }
}

impl GraphQLSchemaCache {
    /// Returns the schema for `meta`, rebuilding it when the model changed.
    async fn ensure(&self, meta_json: &Value) -> Result<(), ApiError> {
        if let Some(compiled) = self.compiled.read().await.as_ref() {
            if &compiled.source == meta_json {
                return Ok(());
            }
        }

        let schema = cubegraphql::make_schema_from_value(meta_json).map_err(graphql_error)?;
        let meta = MetaConfig::from_value(meta_json).map_err(graphql_error)?;

        *self.compiled.write().await = Some(CompiledSchema {
            schema,
            meta,
            source: meta_json.clone(),
        });

        Ok(())
    }

    async fn with_schema<T>(
        &self,
        meta_json: &Value,
        f: impl FnOnce(&Schema, &MetaConfig) -> T,
    ) -> Result<T, ApiError> {
        self.ensure(meta_json).await?;
        let guard = self.compiled.read().await;
        let compiled = guard.as_ref().ok_or_else(ApiError::internal)?;

        Ok(f(&compiled.schema, &compiled.meta))
    }
}

fn graphql_error(err: GraphQLError) -> ApiError {
    ApiError::bad_request(err.to_string())
}

/// The data model as GraphQL needs it: hidden members included, because the
/// schema is filtered by `hasMembers`, not by request visibility.
async fn meta_json(state: &AppState, ctx: &RequestContext) -> Result<Value, ApiError> {
    state.meta_for(ctx).await?.meta(ctx, false).await
}

/// `POST {basePath}/v1/graphql-to-json`: translates a GraphQL document into
/// the REST query JSON. Always answers 200, like the Node.js route.
pub async fn graphql_to_json(
    state: &AppState,
    ctx: &RequestContext,
    body: &Value,
) -> Result<Value, ApiError> {
    let meta_json = meta_json(state, ctx).await?;

    let (response, error) = state
        .graphql_for(ctx)
        .await?
        .with_schema(&meta_json, |_schema, meta| {
            cubegraphql::graphql_to_json_route(body, meta)
        })
        .await?;

    if let Some(error) = error {
        tracing::warn!(error = %error, "GraphQL to JSON error");
    }

    Ok(response)
}

/// Runs the translated query through the `QueryService`, which is what the
/// REST `/v1/load` uses.
struct ServiceExecutor {
    state: AppState,
    ctx: RequestContext,
}

#[async_trait]
impl CubeQueryExecutor for ServiceExecutor {
    async fn load(&self, request: CubeQueryRequest) -> Result<Value, GraphQLError> {
        let query = request.query.into_value();

        let cache_mode = request
            .cache
            .as_deref()
            .and_then(|mode| serde_json::from_value(Value::String(mode.to_string())).ok());

        let (query_type, queries) =
            cubequery::get_normalized_queries(&query, false, cache_mode, &self.state.query_config)
                .map_err(|e| GraphQLError::Execution(e.message().to_string()))?;
        let pivot_query = cubequery::get_pivot_query(query_type, &queries)
            .map_err(|e| GraphQLError::Execution(e.message().to_string()))?;

        self.state
            .query_for(&self.ctx)
            .await
            .map_err(|e| GraphQLError::Execution(e.body.error))?
            .load(
                &self.ctx,
                NormalizedRequest {
                    query_type,
                    queries,
                    pivot_query,
                },
            )
            .await
            .map_err(|e| GraphQLError::Execution(e.body.error))
    }
}

/// `POST {basePath}/graphql`.
pub async fn graphql(
    state: &AppState,
    ctx: &RequestContext,
    body: &Value,
) -> Result<Value, ApiError> {
    let meta_json = meta_json(state, ctx).await?;
    let schema_cache = state.graphql_for(ctx).await?;
    schema_cache.ensure(&meta_json).await?;

    let executor: Arc<dyn CubeQueryExecutor> = Arc::new(ServiceExecutor {
        state: state.clone(),
        ctx: ctx.clone(),
    });

    let guard = schema_cache.compiled.read().await;
    let compiled = guard.as_ref().ok_or_else(ApiError::internal)?;

    Ok(cubegraphql::handle_graphql_request(&compiled.schema, body, executor).await)
}
