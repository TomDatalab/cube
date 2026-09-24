//! `/cube-system/v1/context` (`createSystemContextHandler`). System routes
//! are only reachable with a token signed by the playground auth secret,
//! like `checkAuthSystemMiddleware` in Node.js.

use axum::extract::State;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use serde::Serialize;

use crate::app::AppState;
use crate::error::ApiError;
use crate::handlers::auth::ForContext;
use crate::services::RequestContext;

#[derive(Serialize)]
struct SystemContext {
    #[serde(rename = "basePath")]
    base_path: String,
    #[serde(rename = "dockerVersion")]
    docker_version: Option<String>,
    #[serde(rename = "serverCoreVersion")]
    server_core_version: Option<&'static str>,
}

pub async fn context(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
) -> Result<impl IntoResponse, ApiError> {
    if !ctx.signed_with_playground_auth_secret {
        return Err(ApiError::forbidden("Only for internal use").for_context(&state, &ctx));
    }

    Ok(Json(SystemContext {
        base_path: state.config.base_path.clone(),
        docker_version: std::env::var("CUBEJS_DOCKER_IMAGE_VERSION").ok(),
        server_core_version: Some(env!("CARGO_PKG_VERSION")),
    }))
}
