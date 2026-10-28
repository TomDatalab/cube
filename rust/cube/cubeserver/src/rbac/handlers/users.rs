//! User management, for platform admins: `/v1/users`.
//!
//! Responses carry `{id, name, email, disabled, createdAt}`, never the
//! password hash.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{body, rbac_of, store_error};
use crate::app::AppState;
use crate::error::ApiError;
use crate::rbac::identity::{hash_password, user_token_claims, validate_user_id};
use crate::rbac::store::{NewUser, User};
use crate::rbac::{unauthenticated, Rbac};
use crate::services::RequestContext;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateUser {
    id: String,
    name: Option<String>,
    email: Option<String>,
    password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateUser {
    name: Option<String>,
    email: Option<String>,
    disabled: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetPassword {
    password: String,
}

fn view(user: &User) -> Value {
    json!({
        "id": user.id,
        "name": user.name,
        "email": user.email,
        "disabled": user.disabled,
        "createdAt": user.created_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    })
}

/// 401 without an identity, 403 unless the caller is a platform admin.
fn require_admin<'a>(state: &'a AppState, ctx: &RequestContext) -> Result<&'a Rbac, ApiError> {
    let rbac = rbac_of(state)?;
    if ctx.user.is_none() {
        return Err(unauthenticated());
    }
    if !ctx.platform_admin {
        return Err(ApiError::forbidden("Only platform admins manage users"));
    }
    Ok(rbac)
}

/// `{id, name?, email?, password}` → 201 with the user.
pub async fn create(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    request: Result<Json<CreateUser>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let rbac = require_admin(&state, &ctx)?;
    let request = body(request)?;
    validate_user_id(&request.id)?;

    let password = request.password;
    let password_hash = rbac.run_hashing(move || hash_password(&password)).await??;
    let user = rbac
        .store
        .create_user(&NewUser {
            id: request.id,
            name: request.name,
            email: request.email,
            password_hash,
        })
        .await
        .map_err(|e| store_error(e, "User already exists"))?;
    Ok((StatusCode::CREATED, Json(view(&user))))
}

/// → `{users: [...]}`.
pub async fn list(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
) -> Result<Json<Value>, ApiError> {
    let rbac = require_admin(&state, &ctx)?;
    let users = rbac
        .store
        .users()
        .await
        .map_err(|e| store_error(e, "User already exists"))?;
    let users: Vec<Value> = users.iter().map(view).collect();
    Ok(Json(json!({ "users": users })))
}

/// `{name?, email?, disabled?}`: disabling locks the user out at once.
pub async fn update(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
    request: Result<Json<UpdateUser>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let rbac = require_admin(&state, &ctx)?;
    let request = body(request)?;
    validate_user_id(&id)?;
    let user = rbac
        .store
        .update_user(&id, request.name, request.email, request.disabled)
        .await
        .map_err(|e| store_error(e, "User already exists"))?
        .ok_or_else(user_not_found)?;
    Ok(Json(view(&user)))
}

/// `{password}`: an admin reset. Every token the user holds stops working.
pub async fn set_password(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
    request: Result<Json<SetPassword>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let rbac = require_admin(&state, &ctx)?;
    let request = body(request)?;
    validate_user_id(&id)?;

    let password = request.password;
    let hash = rbac.run_hashing(move || hash_password(&password)).await??;
    rbac.store
        .set_password_hash(&id, &hash)
        .await
        .map_err(|e| match e {
            crate::rbac::store::StoreError::NotFound => user_not_found(),
            e => store_error(e, "User already exists"),
        })?;
    Ok(StatusCode::NO_CONTENT)
}

/// A service token for the user, issued like a login token. A disabled user
/// gets 409: the token could not authenticate anyway.
pub async fn issue_token(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let rbac = require_admin(&state, &ctx)?;
    validate_user_id(&id)?;
    let user = rbac
        .store
        .user(&id)
        .await
        .map_err(|e| store_error(e, "User already exists"))?
        .ok_or_else(user_not_found)?;
    if user.disabled {
        return Err(ApiError::new(StatusCode::CONFLICT, "The user is disabled"));
    }
    let (claims, expires_at) = user_token_claims(&user.id, rbac.config.token_ttl);
    let token = state.auth.issue_token(claims).await?;
    Ok(Json(json!({
        "token": token,
        "expiresAt": expires_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    })))
}

fn user_not_found() -> ApiError {
    ApiError::not_found("User not found")
}
