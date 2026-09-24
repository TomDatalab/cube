//! REST API authentication for Cube: a port of the `checkAuth` /
//! `contextToApiScopes` logic of `@cubejs-backend/api-gateway` to Rust.
//!
//! The crate has no HTTP framework dependency: [`Authenticator::authenticate`]
//! takes the raw `Authorization` header value and returns the verified
//! security context, ready to be wrapped into a middleware by the server.
//!
//! ```no_run
//! use cubeauth::{assert_api_scope, context_to_api_scopes, default_scopes, AuthConfig, Authenticator};
//!
//! # async fn example(header: Option<&str>) -> Result<(), cubeauth::AuthError> {
//! let config = AuthConfig::from_env().expect("valid CUBEJS_* configuration");
//! let auth = Authenticator::new(config);
//! auth.prefetch_jwks().await;
//!
//! let result = auth.authenticate(header).await?;
//! let scopes = context_to_api_scopes(&result.security_context, &default_scopes(auth.config()));
//! assert_api_scope(&scopes, "data")?;
//! # Ok(())
//! # }
//! ```

mod authenticator;
mod config;
mod error;
mod jwk;
mod scopes;
mod token;

pub use authenticator::{
    extract_token, has_dev_token_scope, AuthResult, Authenticator, SecurityContext, DEV_TOKEN_SCOPE,
};
pub use config::{
    AuthConfig, AuthConfigBuilder, AuthConfigError, DEFAULT_JWK_EXPIRE, DEFAULT_JWK_REFETCH_WINDOW,
    DEFAULT_JWK_RETRY,
};
pub use error::{AuthError, JwkError, TokenError};
pub use jwk::{
    parse_cache_control_max_age, JwkCache, JwkFetcher, JwkResponse, KeyFamily, ReqwestJwkFetcher,
    VerificationKey,
};
pub use scopes::{
    assert_api_scope, context_to_api_scopes, default_scopes, ALL_API_SCOPES, DEFAULT_API_SCOPES,
};
pub use token::{issue_token, IssueError, DEFAULT_TOKEN_LIFETIME_SECONDS};
