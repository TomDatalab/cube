//! `checkAuth` + `requestContextMiddleware` of the Node.js gateway.

use axum::extract::{Request, State};
use axum::http::header::AUTHORIZATION;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use uuid::Uuid;

use crate::app::AppState;
use crate::error::ApiError;
use crate::services::RequestContext;

pub const REQUEST_ID_HEADER: &str = "x-request-id";
pub const CUBE_AUTHORIZATION_HEADER: &str = "x-cube-authorization";

pub async fn auth_middleware(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let request_id = req
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_string())
        .unwrap_or_else(|| format!("{}-span-1", Uuid::new_v4()));

    // Node.js: `req.headers['x-cube-authorization'] || req.headers.authorization`
    let header = req
        .headers()
        .get(CUBE_AUTHORIZATION_HEADER)
        .or_else(|| req.headers().get(AUTHORIZATION));
    let authorization = match header {
        Some(value) => Some(
            value
                .to_str()
                .map_err(|_| {
                    ApiError::forbidden("Invalid token")
                        .with_request_id_if(state.config.dev_mode, &request_id)
                })?
                .to_string(),
        ),
        None => None,
    };

    let authenticated = state
        .auth
        .authenticate(authorization.as_deref())
        .await
        .map_err(|e| e.with_request_id_if(state.config.dev_mode, &request_id))?;

    req.extensions_mut().insert(RequestContext {
        request_id,
        security_context: authenticated.security_context,
        signed_with_playground_auth_secret: authenticated.signed_with_playground_auth_secret,
    });

    Ok(next.run(req).await.into_response())
}

/// `assertApiScope` of the Node.js gateway.
pub async fn assert_api_scope(
    state: &AppState,
    ctx: &RequestContext,
    scope: &str,
) -> Result<(), ApiError> {
    let scopes = state.api_scopes_for(ctx).await?;
    if scopes.iter().any(|s| s == scope) {
        Ok(())
    } else {
        Err(
            ApiError::forbidden(format!("API scope is missing: {}", scope))
                .with_request_id(&ctx.request_id),
        )
    }
}

/// Attaches `requestId` under the same conditions as the Node.js gateway.
pub trait ForContext {
    fn for_context(self, state: &AppState, ctx: &RequestContext) -> Self;
}

impl ForContext for ApiError {
    fn for_context(self, state: &AppState, ctx: &RequestContext) -> Self {
        self.with_request_id_if(
            state.config.dev_mode || ctx.signed_with_playground_auth_secret,
            &ctx.request_id,
        )
    }
}
