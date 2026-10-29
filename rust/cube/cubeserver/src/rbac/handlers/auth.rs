//! `POST /v1/auth/login` and `PUT /v1/auth/password`.

use axum::extract::rejection::JsonRejection;
use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{body, rbac_of};
use crate::app::AppState;
use crate::error::ApiError;
use crate::rbac::identity::{
    check_credentials, hash_password, user_token_claims, validate_user_id,
};
use crate::rbac::store::StoreError;
use crate::rbac::{unauthenticated, unavailable, Rbac};
use crate::services::RequestContext;

/// The same answer for an unknown user, a wrong password and a disabled user.
const INVALID_LOGIN: &str = "Invalid username or password";

#[derive(Deserialize)]
pub struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangePasswordRequest {
    current_password: String,
    new_password: String,
}

/// Waits before answering a failure, to slow down guessing.
async fn failure(rbac: &Rbac, error: ApiError) -> ApiError {
    tokio::time::sleep(rbac.config.failure_delay).await;
    error
}

/// `{username, password}` → `{token, expiresAt, user: {id, name, platformAdmin}}`.
pub async fn login(
    State(state): State<AppState>,
    request: Result<Json<LoginRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let rbac = rbac_of(&state)?;
    let request = body(request)?;

    // SpiceDB is asked before the password is checked, so an outage answers
    // 503 whether or not the password is right: the answer never tells a
    // correct password from a wrong one. A name that is not a valid id
    // cannot be asked about, and is refused below like an unknown user.
    let platform_admin = if validate_user_id(&request.username).is_ok() {
        rbac.is_platform_admin(&request.username).await?
    } else {
        false
    };

    let Some(user) = check_credentials(rbac, &request.username, &request.password).await? else {
        return Err(failure(rbac, ApiError::new(StatusCode::UNAUTHORIZED, INVALID_LOGIN)).await);
    };
    let (claims, expires_at) = user_token_claims(&user.id, rbac.config.token_ttl);
    let token = state.auth.issue_token(claims).await?;

    Ok(Json(json!({
        "token": token,
        "expiresAt": expires_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "user": {
            "id": user.id,
            "name": user.name,
            "platformAdmin": platform_admin,
        },
    })))
}

/// `{currentPassword, newPassword}`: users change their own password. Every
/// token issued before stops working.
pub async fn change_password(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    request: Result<Json<ChangePasswordRequest>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let rbac = rbac_of(&state)?;
    let user = ctx.user.as_deref().ok_or_else(unauthenticated)?;
    let request = body(request)?;

    if check_credentials(rbac, user, &request.current_password)
        .await?
        .is_none()
    {
        return Err(failure(
            rbac,
            ApiError::forbidden("The current password is not correct"),
        )
        .await);
    }

    let new_password = request.new_password;
    let hash = rbac
        .run_hashing(move || hash_password(&new_password))
        .await??;
    rbac.store
        .set_password_hash(user, &hash)
        .await
        .map_err(|e| match e {
            StoreError::NotFound => unauthenticated(),
            e => {
                tracing::error!("Cannot store a new password: {e}");
                unavailable()
            }
        })?;

    Ok(StatusCode::NO_CONTENT)
}
