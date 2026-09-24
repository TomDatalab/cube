//! `/v1/load`, `/v1/sql`, `/v1/dry-run` (GET with `?query=`, POST with a JSON
//! body), mirroring `ApiGateway.load` / `sql` / `dryRun`.
//!
//! Parsing, validation and normalization are done by the `cubequery` crate
//! (the port of `query.js`), so handlers hand the service the same normalized
//! queries the Node.js gateway produces.

use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use axum::{Extension, Json};
use cubequery::{get_normalized_queries, get_pivot_query, QueryError};
use serde::Deserialize;
use serde_json::Value;

use crate::app::AppState;
use crate::error::ApiError;
use crate::handlers::auth::{assert_api_scope, ForContext};
use crate::services::{NormalizedRequest, RequestContext};

#[derive(Debug, Deserialize)]
pub struct QueryParams {
    query: Option<String>,
    #[serde(rename = "queryType")]
    query_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct QueryBody {
    query: Option<Value>,
    #[serde(rename = "queryType")]
    query_type: Option<String>,
}

/// A `UserError` is a client mistake (HTTP 400); a plain `Error` is not
/// (HTTP 500), like `ApiGateway.handleError`.
fn query_error(err: QueryError) -> ApiError {
    if err.is_user_error() {
        ApiError::bad_request(err.message())
    } else {
        ApiError::new(axum::http::StatusCode::INTERNAL_SERVER_ERROR, err.message())
    }
}

fn normalize(state: &AppState, value: &Value) -> Result<NormalizedRequest, ApiError> {
    let (query_type, queries) =
        get_normalized_queries(value, false, None, &state.query_config).map_err(query_error)?;
    let pivot_query = get_pivot_query(query_type, &queries).map_err(query_error)?;

    Ok(NormalizedRequest {
        query_type,
        queries,
        pivot_query,
    })
}

macro_rules! query_endpoint {
    ($get:ident, $post:ident, $method:ident) => {
        pub async fn $get(
            State(state): State<AppState>,
            Extension(ctx): Extension<RequestContext>,
            Query(params): Query<QueryParams>,
        ) -> Result<impl IntoResponse, ApiError> {
            assert_api_scope(&state, &ctx, "data").await?;

            // GET: `query` is a JSON document in the query string.
            let value = params.query.map(Value::String).unwrap_or(Value::Null);
            let request = normalize(&state, &value).map_err(|e| e.for_context(&state, &ctx))?;

            let response = state
                .query_for(&ctx)
                .await?
                .$method(&ctx, request)
                .await
                .map_err(|e| e.for_context(&state, &ctx))?;
            Ok(Json(response))
        }

        pub async fn $post(
            State(state): State<AppState>,
            Extension(ctx): Extension<RequestContext>,
            Json(body): Json<QueryBody>,
        ) -> Result<impl IntoResponse, ApiError> {
            assert_api_scope(&state, &ctx, "data").await?;

            let value = body.query.unwrap_or(Value::Null);
            let request = normalize(&state, &value).map_err(|e| e.for_context(&state, &ctx))?;

            let response = state
                .query_for(&ctx)
                .await?
                .$method(&ctx, request)
                .await
                .map_err(|e| e.for_context(&state, &ctx))?;
            Ok(Json(response))
        }
    };
}

/// `GET /v1/load` (and `/v1/subscribe`): the response is shaped by
/// [`crate::load_response`], which honours `queryType=multi`.
pub async fn load_get(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Query(params): Query<QueryParams>,
) -> Result<impl IntoResponse, ApiError> {
    assert_api_scope(&state, &ctx, "data").await?;

    let value = params.query.map(Value::String).unwrap_or(Value::Null);
    let request = normalize(&state, &value).map_err(|e| e.for_context(&state, &ctx))?;
    let response = crate::load_response::load(&state, &ctx, request, params.query_type.as_deref())
        .await
        .map_err(|e| e.for_context(&state, &ctx))?;
    Ok(Json(response))
}

/// `POST /v1/load` (and `/v1/subscribe`).
pub async fn load_post(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Json(body): Json<QueryBody>,
) -> Result<impl IntoResponse, ApiError> {
    assert_api_scope(&state, &ctx, "data").await?;

    let value = body.query.unwrap_or(Value::Null);
    let request = normalize(&state, &value).map_err(|e| e.for_context(&state, &ctx))?;
    let response = crate::load_response::load(&state, &ctx, request, body.query_type.as_deref())
        .await
        .map_err(|e| e.for_context(&state, &ctx))?;
    Ok(Json(response))
}

query_endpoint!(sql_get, sql_post, sql);
query_endpoint!(dry_run_get, dry_run_post, dry_run);

/// `DELETE /v1/running-query/:requestId` (`ApiGateway.cancelQuery`).
pub async fn cancel_running_query(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(request_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    assert_api_scope(&state, &ctx, "data").await?;

    let cancelled = state
        .query_for(&ctx)
        .await?
        .cancel_query(&ctx, &request_id)
        .await
        .map_err(|e| e.for_context(&state, &ctx))?;

    Ok(Json(serde_json::json!({ "result": cancelled })))
}
