//! `POST {basePath}/v1/graphql-to-json` and `{basePath}/graphql`
//! (`gateway.ts:339` and `:365`).

use axum::extract::State;
use axum::response::{Html, IntoResponse};
use axum::{Extension, Json};
use serde_json::Value;

use crate::app::AppState;
use crate::error::ApiError;
use crate::graphql_adapter;
use crate::handlers::auth::{assert_api_scope, ForContext};
use crate::services::RequestContext;

pub async fn graphql_to_json(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Json(body): Json<Value>,
) -> Result<impl IntoResponse, ApiError> {
    assert_api_scope(&state, &ctx, "graphql").await?;

    let response = graphql_adapter::graphql_to_json(&state, &ctx, &body)
        .await
        .map_err(|e| e.for_context(&state, &ctx))?;

    Ok(Json(response))
}

pub async fn graphql(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Json(body): Json<Value>,
) -> Result<impl IntoResponse, ApiError> {
    assert_api_scope(&state, &ctx, "graphql").await?;

    let response = graphql_adapter::graphql(&state, &ctx, &body)
        .await
        .map_err(|e| e.for_context(&state, &ctx))?;

    Ok(Json(response))
}

/// The GraphiQL page, served outside production like the Node.js route.
pub async fn graphiql(State(state): State<AppState>) -> impl IntoResponse {
    let endpoint = format!("{}/graphql", state.config.base_path);
    Html(cubegraphql::graphiql_html(&endpoint))
}
