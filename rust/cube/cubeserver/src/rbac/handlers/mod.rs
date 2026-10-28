//! HTTP routes that exist only with RBAC on.

pub mod auth;
pub mod connections;
pub mod organizations;
pub mod users;
pub mod workspaces;

use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use axum::routing::{get, patch, post, put};
use axum::Json;
use axum::Router;
use uuid::Uuid;

use crate::app::AppState;
use crate::error::ApiError;
use crate::rbac::identity::validate_user_id;
use crate::rbac::store::{StoreError, User};
use crate::rbac::{unauthenticated, Rbac};
use crate::services::RequestContext;

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
        .route(
            &format!("{base}/v1/organizations"),
            get(organizations::list).post(organizations::create),
        )
        .route(
            &format!("{base}/v1/organizations/:id"),
            get(organizations::get)
                .patch(organizations::update)
                .delete(organizations::delete),
        )
        .route(
            &format!("{base}/v1/organizations/:id/owners/:user"),
            put(organizations::add_owner).delete(organizations::remove_owner),
        )
        .route(
            &format!("{base}/v1/organizations/:id/workspaces"),
            get(workspaces::list).post(workspaces::create),
        )
        .route(
            &format!("{base}/v1/workspaces/:id"),
            get(workspaces::get)
                .patch(workspaces::update)
                .delete(workspaces::delete),
        )
        .route(
            &format!("{base}/v1/workspaces/:id/members"),
            get(workspaces::members),
        )
        .route(
            &format!("{base}/v1/workspaces/:id/members/:user"),
            put(workspaces::set_member).delete(workspaces::remove_member),
        )
        .route(
            &format!("{base}/v1/organizations/:id/connections"),
            get(connections::list).post(connections::create),
        )
        .route(
            &format!("{base}/v1/connections/:name"),
            put(connections::update).delete(connections::delete),
        )
        .route(
            &format!("{base}/v1/connections/:name/organization"),
            put(connections::assign_organization),
        )
        .route(
            &format!("{base}/v1/workspaces/:id/connections"),
            get(connections::of_workspace),
        )
        .route(
            &format!("{base}/v1/workspaces/:id/connections/:name"),
            put(connections::release).delete(connections::withdraw),
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

/// The RBAC and the caller's user id, or 401 without an identity.
pub(crate) fn caller<'a>(
    state: &'a AppState,
    ctx: &'a RequestContext,
) -> Result<(&'a Rbac, &'a str), ApiError> {
    let rbac = rbac_of(state)?;
    let user = ctx.user.as_deref().ok_or_else(unauthenticated)?;
    Ok((rbac, user))
}

/// A resource id from the path. Anything that is not a UUID cannot exist: 404.
pub(crate) fn resource_id(id: &str, kind: &str) -> Result<Uuid, ApiError> {
    id.parse()
        .map_err(|_| ApiError::not_found(format!("{kind} not found")))
}

/// A display name: trimmed, not empty.
pub(crate) fn resource_name(name: &str) -> Result<String, ApiError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("The name cannot be empty"));
    }
    Ok(name.to_string())
}

/// A user to grant something to: 400 when the id is malformed or the user
/// is disabled, 404 when unknown.
pub(crate) async fn existing_user(rbac: &Rbac, id: &str) -> Result<User, ApiError> {
    validate_user_id(id)?;
    let user = rbac
        .store
        .user(id)
        .await
        .map_err(|e| store_error(e, "Conflict"))?
        .ok_or_else(|| ApiError::not_found("User not found"))?;
    if user.disabled {
        return Err(ApiError::bad_request("User is disabled"));
    }
    Ok(user)
}

/// The RBAC of a route mounted only when it is on.
pub(crate) fn rbac_of(state: &AppState) -> Result<&Rbac, ApiError> {
    state
        .rbac
        .as_deref()
        .ok_or_else(|| ApiError::not_found("RBAC is not enabled"))
}
