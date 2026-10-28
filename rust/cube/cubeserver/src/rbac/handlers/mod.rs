//! HTTP routes that exist only with RBAC on.

pub mod auth;

use axum::routing::{post, put};
use axum::Router;

use crate::app::AppState;
use crate::error::ApiError;
use crate::rbac::Rbac;

/// Routes reachable without a token (`POST /v1/auth/login`).
pub fn public_routes(base: &str) -> Router<AppState> {
    Router::new().route(&format!("{base}/v1/auth/login"), post(auth::login))
}

/// Routes behind the auth middleware.
pub fn authenticated_routes(base: &str) -> Router<AppState> {
    Router::new().route(
        &format!("{base}/v1/auth/password"),
        put(auth::change_password),
    )
}

/// The RBAC of a route mounted only when it is on.
pub(crate) fn rbac_of(state: &AppState) -> Result<&Rbac, ApiError> {
    state
        .rbac
        .as_deref()
        .ok_or_else(|| ApiError::not_found("RBAC is not enabled"))
}
