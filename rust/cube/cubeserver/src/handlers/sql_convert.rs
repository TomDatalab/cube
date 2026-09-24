//! `POST /v1/convert-query` and `POST /v1/cubesql` (`gateway.ts:503,544`).

use axum::extract::State;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::Value;

use crate::app::AppState;
use crate::error::ApiError;
use crate::handlers::auth::{assert_api_scope, ForContext};
use crate::services::RequestContext;

#[derive(Debug, Deserialize)]
pub struct ConvertBody {
    input: Option<String>,
    output: Option<String>,
    query: Option<String>,
}

/// `POST /v1/convert-query`.
pub async fn convert_query(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Json(body): Json<ConvertBody>,
) -> Result<impl IntoResponse, ApiError> {
    assert_api_scope(&state, &ctx, "sql").await?;

    // The same three checks the Node.js handler makes, with its messages.
    let input = body.input.as_deref().unwrap_or_default();
    if input != "sql" {
        return Err(
            ApiError::bad_request(format!("Unexpected input parameter value '{input}'"))
                .for_context(&state, &ctx),
        );
    }

    let output = body.output.as_deref().unwrap_or_default();
    if output != "rest" {
        return Err(
            ApiError::bad_request(format!("Unexpected output parameter value '{output}'"))
                .for_context(&state, &ctx),
        );
    }

    let query = body.query.unwrap_or_default();
    if query.trim().is_empty() {
        return Err(
            ApiError::bad_request("query parameter must be a non-empty string")
                .for_context(&state, &ctx),
        );
    }

    let response = state
        .sql_conversion
        .convert_query(&ctx, &query)
        .await
        .map_err(|e| e.for_context(&state, &ctx))?;

    Ok(Json(response))
}

#[derive(Debug, Deserialize)]
pub struct CubeSqlBody {
    query: Option<String>,
}

/// `POST /v1/cubesql`.
pub async fn cubesql(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Json(body): Json<CubeSqlBody>,
) -> Result<impl IntoResponse, ApiError> {
    assert_api_scope(&state, &ctx, "data").await?;

    let query = body.query.unwrap_or_default();
    if query.trim().is_empty() {
        return Err(
            ApiError::bad_request("Invalid query format: \"query\" is required")
                .for_context(&state, &ctx),
        );
    }

    let response: Value = state
        .sql_conversion
        .cubesql(&ctx, &query)
        .await
        .map_err(|e| e.for_context(&state, &ctx))?;

    Ok(Json(response))
}
