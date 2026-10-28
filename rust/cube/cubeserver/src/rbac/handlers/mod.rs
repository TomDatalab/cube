//! HTTP routes that exist only with RBAC on.

pub mod auth;
pub mod users;

use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use axum::routing::{get, patch, post, put};
use axum::Json;
use axum::Router;

use crate::app::AppState;
use crate::error::ApiError;
use crate::rbac::store::StoreError;
use crate::rbac::Rbac;

/// Routes reachable without a token (`POST /v1/auth/login`).
pub fn public_routes(base: &str) -> Router<AppState> {
    Router::new().route(&format!("{base}/v1/auth/login"), post(auth::login))
}

/// Routes behind the auth middleware.
pub fn authenticated_routes(base: &str) -> Router<AppState> {
    Router::new()
        .route(
            &format!("{base}/v1/auth/password"),
            put(auth::change_password),
        )
        .route(
            &format!("{base}/v1/users"),
            get(users::list).post(users::create),
        )
        .route(&format!("{base}/v1/users/:id"), patch(users::update))
        .route(
            &format!("{base}/v1/users/:id/password"),
            put(users::set_password),
        )
        .route(
            &format!("{base}/v1/users/:id/tokens"),
            post(users::issue_token),
        )
}

/// The JSON body of a request, or 400.
pub(crate) fn body<T>(request: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    request
        .map(|Json(body)| body)
        .map_err(|e| ApiError::bad_request(e.body_text()))
}

/// A store failure as an API answer: 409 for a duplicate, 503 otherwise.
pub(crate) fn store_error(error: StoreError, conflict: &str) -> ApiError {
    match error {
        StoreError::Conflict(_) => ApiError::new(StatusCode::CONFLICT, conflict),
        e => {
            tracing::error!("RBAC store failed: {e}");
            crate::rbac::unavailable()
        }
    }
}

/// The RBAC of a route mounted only when it is on.
pub(crate) fn rbac_of(state: &AppState) -> Result<&Rbac, ApiError> {
    state
        .rbac
        .as_deref()
        .ok_or_else(|| ApiError::not_found("RBAC is not enabled"))
}
