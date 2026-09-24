//! `/v1/pre-aggregations/*` and the `/cube-system/v1/pre-aggregations/*`
//! routes of `packages/cubejs-api-gateway/src/gateway.ts`.

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::error::ApiError;
use crate::handlers::auth::{assert_api_scope, ForContext};
use crate::services::RequestContext;

#[derive(Debug, Deserialize)]
pub struct CanUseBody {
    #[serde(rename = "transformedQuery")]
    transformed_query: Option<Value>,
    references: Option<Value>,
}

/// `POST /v1/pre-aggregations/can-use`, used by the Rollup Designer.
pub async fn can_use(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Json(body): Json<CanUseBody>,
) -> Result<impl IntoResponse, ApiError> {
    assert_api_scope(&state, &ctx, "meta").await?;

    let response = state
        .pre_aggregations
        .can_use(
            &ctx,
            body.transformed_query.unwrap_or(Value::Null),
            body.references.unwrap_or(Value::Null),
        )
        .await
        .map_err(|e| e.for_context(&state, &ctx))?;

    Ok(Json(response))
}

/// `POST /v1/pre-aggregations/jobs`.
pub async fn jobs(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Json(body): Json<Value>,
) -> Result<impl IntoResponse, ApiError> {
    assert_api_scope(&state, &ctx, "jobs").await?;

    let response = state
        .pre_aggregations
        .jobs(&ctx, body)
        .await
        .map_err(|e| e.for_context(&state, &ctx))?;

    Ok(Json(response))
}

/// System routes are reachable only with a playground-signed token, like
/// `checkAuthSystemMiddleware`.
fn assert_system(state: &AppState, ctx: &RequestContext) -> Result<(), ApiError> {
    if ctx.signed_with_playground_auth_secret {
        Ok(())
    } else {
        Err(ApiError::forbidden("Only for internal use").for_context(state, ctx))
    }
}

#[derive(Debug, Deserialize)]
pub struct ListParams {
    #[serde(rename = "cacheOnly")]
    cache_only: Option<String>,
    #[serde(rename = "metaOnly")]
    meta_only: Option<String>,
}

/// `GET /cube-system/v1/pre-aggregations`.
pub async fn list(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Query(params): Query<ListParams>,
) -> Result<impl IntoResponse, ApiError> {
    assert_system(&state, &ctx)?;

    // Node.js: `!!req.query.cacheOnly`, so any present value is true.
    let response = state
        .pre_aggregations
        .list(
            &ctx,
            params.cache_only.is_some(),
            params.meta_only.is_some(),
        )
        .await
        .map_err(|e| e.for_context(&state, &ctx))?;

    Ok(Json(response))
}

/// `GET /cube-system/v1/pre-aggregations/security-contexts`.
pub async fn security_contexts(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
) -> Result<impl IntoResponse, ApiError> {
    assert_system(&state, &ctx)?;

    let contexts = state
        .pre_aggregations
        .security_contexts()
        .await
        .map_err(|e| e.for_context(&state, &ctx))?;

    Ok(Json(json!({ "securityContexts": contexts })))
}

/// `GET /cube-system/v1/pre-aggregations/timezones`.
pub async fn timezones(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
) -> Result<impl IntoResponse, ApiError> {
    assert_system(&state, &ctx)?;

    let timezones = state
        .pre_aggregations
        .timezones(&ctx)
        .await
        .map_err(|e| e.for_context(&state, &ctx))?;

    Ok(Json(json!({ "timezones": timezones })))
}

#[derive(Debug, Deserialize)]
pub struct QueryBody {
    query: Option<Value>,
}

macro_rules! system_query_endpoint {
    ($name:ident, $method:ident) => {
        pub async fn $name(
            State(state): State<AppState>,
            Extension(ctx): Extension<RequestContext>,
            Json(body): Json<QueryBody>,
        ) -> Result<impl IntoResponse, ApiError> {
            assert_system(&state, &ctx)?;

            let response = state
                .pre_aggregations
                .$method(&ctx, body.query.unwrap_or(Value::Null))
                .await
                .map_err(|e| e.for_context(&state, &ctx))?;

            Ok(Json(response))
        }
    };
}

system_query_endpoint!(partitions, partitions);
system_query_endpoint!(preview, preview);
system_query_endpoint!(build, build);
system_query_endpoint!(cancel, cancel);

/// `POST /cube-system/v1/pre-aggregations/queue`.
pub async fn queue(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
) -> Result<impl IntoResponse, ApiError> {
    assert_system(&state, &ctx)?;

    let response = state
        .pre_aggregations
        .queue(&ctx)
        .await
        .map_err(|e| e.for_context(&state, &ctx))?;

    Ok(Json(response))
}
