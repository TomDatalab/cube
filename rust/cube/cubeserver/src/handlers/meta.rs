//! `GET /v1/meta` (`ApiGateway.meta` / `metaExtended`).

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::{Extension, Json};
use serde::Deserialize;

use crate::app::AppState;
use crate::error::ApiError;
use crate::handlers::auth::{assert_api_scope, ForContext};
use crate::services::RequestContext;

#[derive(Debug, Deserialize)]
pub struct MetaParams {
    #[serde(rename = "onlyViews")]
    only_views: Option<String>,
    extended: Option<String>,
}

pub async fn meta(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Query(params): Query<MetaParams>,
) -> Result<impl IntoResponse, ApiError> {
    assert_api_scope(&state, &ctx, "meta").await?;

    // Node.js: `req.query.onlyViews === 'true'`, `'extended' in req.query`
    let only_views = params.only_views.as_deref() == Some("true");
    let meta = state.meta_for(&ctx).await?;
    let response = if params.extended.is_some() {
        meta.meta_extended(&ctx, only_views).await
    } else {
        meta.meta(&ctx, only_views).await
    }
    .map_err(|e| e.for_context(&state, &ctx))?;

    Ok(Json(response))
}
