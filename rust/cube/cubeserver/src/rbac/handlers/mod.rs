//! HTTP routes that exist only with RBAC on.

pub mod auth;
pub mod connections;
pub mod cube_models;
pub mod groups;
pub mod model;
pub mod objects;
pub mod organizations;
pub mod roles;
pub mod rules;
pub mod users;
pub mod workspaces;

use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use axum::routing::{get, patch, post, put};
use axum::Json;
use axum::Router;
use serde::Deserialize;
use uuid::Uuid;

use crate::app::AppState;
use crate::error::ApiError;
use crate::rbac::identity::validate_user_id;
use crate::rbac::store::{Search, StoreError, User};
use crate::rbac::{authz_error, unauthenticated, Rbac, RelationshipFilter};
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
        .route(&format!("{base}/v1/users/:id/access"), get(users::access))
        .route(
            &format!("{base}/v1/users/:id/permissions"),
            get(users::permissions),
        )
        .route(
            &format!("{base}/v1/users/:id/attributes"),
            get(users::attributes).put(users::set_attributes),
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
        .route(&format!("{base}/v1/workspaces"), get(workspaces::list_all))
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
            &format!("{base}/v1/connections"),
            get(connections::list_all),
        )
        .route(
            &format!("{base}/v1/connections/:name"),
            put(connections::update).delete(connections::delete),
        )
        .route(
            &format!("{base}/v1/connections/:name/organization"),
            put(connections::assign_organization).delete(connections::unassign_organization),
        )
        .route(
            &format!("{base}/v1/workspaces/:id/connections"),
            get(connections::of_workspace),
        )
        .route(
            &format!("{base}/v1/workspaces/:id/connections/:name"),
            put(connections::release).delete(connections::withdraw),
        )
        .route(
            &format!("{base}/v1/organizations/:id/roles"),
            get(roles::list).post(roles::create),
        )
        .route(
            &format!("{base}/v1/organizations/:id/model-members"),
            get(roles::model_members),
        )
        .route(
            &format!("{base}/v1/roles/:id"),
            get(roles::get).patch(roles::update).delete(roles::delete),
        )
        .route(
            &format!("{base}/v1/workspaces/:id/rules"),
            get(rules::of_workspace).post(rules::create),
        )
        .route(&format!("{base}/v1/rules"), get(rules::list_all))
        .route(
            &format!("{base}/v1/rules/:id"),
            get(rules::get).patch(rules::update).delete(rules::delete),
        )
        .route(&format!("{base}/v1/rules/:id/members"), get(rules::members))
        .route(
            &format!("{base}/v1/rules/:id/members/:user"),
            put(rules::add_member).delete(rules::remove_member),
        )
        .route(&format!("{base}/v1/groups"), get(groups::list_all))
        .route(
            &format!("{base}/v1/organizations/:id/groups"),
            get(groups::of_organization).post(groups::create),
        )
        .route(
            &format!("{base}/v1/groups/:id"),
            get(groups::get).patch(groups::update).delete(groups::delete),
        )
        .route(&format!("{base}/v1/groups/:id/members"), get(groups::members))
        .route(
            &format!("{base}/v1/groups/:id/members/:user"),
            put(groups::add_member).delete(groups::remove_member),
        )
        .route(&format!("{base}/v1/groups/:id/rules"), get(groups::rules))
        .route(
            &format!("{base}/v1/groups/:id/rules/:rule"),
            put(groups::add_rule).delete(groups::remove_rule),
        )
        .route(
            &format!("{base}/v1/workspaces/:id/objects"),
            get(objects::list).post(objects::create),
        )
        .route(
            &format!("{base}/v1/organizations/:id/model"),
            get(model::org_detail),
        )
        .route(
            &format!("{base}/v1/organizations/:id/model/files/:file"),
            get(model::org_get_file)
                .put(model::org_put_file)
                .delete(model::org_delete_file),
        )
        .route(
            &format!("{base}/v1/organizations/:id/model/generate"),
            post(model::org_generate),
        )
        .route(
            &format!("{base}/v1/organizations/:id/cube-models"),
            get(cube_models::list).post(cube_models::create),
        )
        .route(
            &format!("{base}/v1/cube-models/:id"),
            get(cube_models::get)
                .put(cube_models::update)
                .delete(cube_models::delete),
        )
        .route(
            &format!("{base}/v1/cube-models/:id/build"),
            post(cube_models::build),
        )
        .route(
            &format!("{base}/v1/cube-models/:id/create-tables"),
            post(cube_models::create_tables),
        )
        .route(
            &format!("{base}/v1/cube-models/:id/sql"),
            post(cube_models::sql),
        )
        .route(
            &format!("{base}/v1/platform/model/generate"),
            post(model::platform_generate),
        )
        .route(
            &format!("{base}/v1/platform/model"),
            get(model::platform_detail),
        )
        .route(
            &format!("{base}/v1/platform/model/files/:file"),
            get(model::platform_get_file)
                .put(model::platform_put_file)
                .delete(model::platform_delete_file),
        )
        .route(
            &format!("{base}/v1/model/files/:file/organization"),
            put(model::move_file),
        )
        .route(&format!("{base}/v1/model/export"), get(model::export))
        .route(
            &format!("{base}/v1/model/sync/:connection/context"),
            get(model::sync_context),
        )
        .route(
            &format!("{base}/v1/model/sync/:connection"),
            put(model::sync_put),
        )
        .route(
            &format!("{base}/v1/objects/:id"),
            get(objects::get)
                .put(objects::update)
                .delete(objects::delete),
        )
}

/// The largest page a listing hands out.
const MAX_LIMIT: i64 = 200;

/// `?q&limit&offset` of the management listings.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchParams {
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

impl SearchParams {
    /// The search to run, or 400 for a `limit` above 200 or a negative
    /// `limit` or `offset`. A blank `q` is no search.
    pub(crate) fn search(&self) -> Result<Search, ApiError> {
        if self.limit.is_some_and(|l| !(0..=MAX_LIMIT).contains(&l)) {
            return Err(ApiError::bad_request(format!(
                "limit must be between 0 and {MAX_LIMIT}"
            )));
        }
        if self.offset.is_some_and(|o| o < 0) {
            return Err(ApiError::bad_request("offset cannot be negative"));
        }
        Ok(Search {
            q: self
                .q
                .as_deref()
                .map(str::trim)
                .filter(|q| !q.is_empty())
                .map(str::to_string),
            limit: self.limit,
            offset: self.offset.unwrap_or(0),
        })
    }

    /// Whether any of `q`, `limit` and `offset` came, i.e. the answer
    /// carries a `total`.
    pub(crate) fn has_paging(&self) -> bool {
        self.q.is_some() || self.limit.is_some() || self.offset.is_some()
    }
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
        StoreError::Invalid(m) => ApiError::bad_request(m),
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

/// The caller's username, what `created_by`/`updated_by` record; 401
/// without an identity.
pub(crate) fn actor(ctx: &RequestContext) -> Result<&str, ApiError> {
    ctx.username.as_deref().ok_or_else(unauthenticated)
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

/// The user a path segment or body field names, by UUID or by username
/// (`Store::resolve_user`): 400 when the key is malformed, 404 when no user
/// has it. Everything downstream takes the returned `User.id`.
pub(crate) async fn named_user(rbac: &Rbac, key: &str) -> Result<User, ApiError> {
    validate_user_id(key)?;
    rbac.store
        .resolve_user(key)
        .await
        .map_err(|e| store_error(e, "Conflict"))?
        .ok_or_else(user_not_found)
}

pub(crate) fn user_not_found() -> ApiError {
    ApiError::not_found("User not found")
}

/// The SpiceDB subject id a member path names, to remove it from `kind`
/// `id` by one of `relations`. The key itself when that resource relates a
/// subject with exactly this id: a subject left without a user row (an
/// orphan) is removed, even when a live user has since taken its name.
/// Otherwise the user's UUID ([`named_user`]), or the key when no user has
/// it. 400 when the key is malformed.
pub(crate) async fn member_subject(
    rbac: &Rbac,
    key: &str,
    kind: &'static str,
    id: String,
    relations: &[&str],
) -> Result<String, ApiError> {
    validate_user_id(key)?;
    let literal = rbac
        .authz
        .read(&RelationshipFilter {
            kind,
            id: Some(id),
            relation: None,
            subject: Some(organizations::user_ref(key)),
        })
        .await
        .map_err(authz_error)?;
    if literal.iter().any(|r| relations.contains(&r.relation)) {
        return Ok(key.to_string());
    }
    Ok(rbac
        .store
        .resolve_user(key)
        .await
        .map_err(|e| store_error(e, "Conflict"))?
        .map_or_else(|| key.to_string(), |user| user.id))
}

/// A user to grant something to ([`named_user`]): also 400 when the user
/// is disabled.
pub(crate) async fn existing_user(rbac: &Rbac, key: &str) -> Result<User, ApiError> {
    let user = named_user(rbac, key).await?;
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
