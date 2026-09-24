//! [`AuthService`] backed by the `cubeauth` crate (JWT/JWK verification,
//! playground secret, API scopes) — the Rust replacement for `checkAuth` and
//! `contextToApiScopes` of the Node.js gateway.

use std::sync::Arc;

use async_trait::async_trait;
use axum::http::StatusCode;
use cubeauth::{AuthConfig, AuthError, Authenticator};
use serde_json::{Map, Value};

use crate::error::ApiError;
use crate::services::{AuthService, AuthenticatedRequest};

pub struct CubeAuthService {
    authenticator: Arc<Authenticator>,
    default_scopes: Vec<String>,
}

impl std::fmt::Debug for CubeAuthService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CubeAuthService")
            .field("default_scopes", &self.default_scopes)
            .finish_non_exhaustive()
    }
}

impl CubeAuthService {
    pub fn new(config: AuthConfig) -> Self {
        let default_scopes = cubeauth::default_scopes(&config);
        Self {
            authenticator: Arc::new(Authenticator::new(config)),
            default_scopes,
        }
    }

    pub fn from_authenticator(authenticator: Arc<Authenticator>) -> Self {
        let default_scopes = cubeauth::default_scopes(authenticator.config());
        Self {
            authenticator,
            default_scopes,
        }
    }

    /// Warms the JWK cache like `jwks.fetchOnly` at server start.
    pub async fn prefetch_jwks(&self) {
        self.authenticator.prefetch_jwks().await;
    }
}

pub fn api_error(err: AuthError) -> ApiError {
    let status = StatusCode::from_u16(err.status_code()).unwrap_or(StatusCode::FORBIDDEN);
    ApiError::new(status, err.to_string())
}

#[async_trait]
impl AuthService for CubeAuthService {
    async fn issue_token(&self, claims: Map<String, Value>) -> Result<String, ApiError> {
        cubeauth::issue_token(
            self.authenticator.config(),
            claims,
            cubeauth::DEFAULT_TOKEN_LIFETIME_SECONDS,
        )
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
    }

    async fn authenticate(
        &self,
        authorization: Option<&str>,
    ) -> Result<AuthenticatedRequest, ApiError> {
        let result = self
            .authenticator
            .authenticate(authorization)
            .await
            .map_err(api_error)?;

        Ok(AuthenticatedRequest {
            security_context: self
                .authenticator
                .extract_security_context(&result.security_context),
            signed_with_playground_auth_secret: result.signed_with_playground_auth_secret,
        })
    }

    async fn api_scopes(&self, security_context: &Value) -> Result<Vec<String>, ApiError> {
        Ok(cubeauth::context_to_api_scopes(
            security_context,
            &self.default_scopes,
        ))
    }
}
