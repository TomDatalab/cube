//! API scopes (`contextToApiScopes` / `assertApiScope` in `gateway.ts`).

use serde_json::Value;

use crate::config::AuthConfig;
use crate::error::AuthError;

/// Every scope the gateway knows about (the `ApiScopes` TypeScript type).
pub const ALL_API_SCOPES: [&str; 5] = ["graphql", "meta", "data", "sql", "jobs"];

/// `contextToApiScopesDefFn`: scopes granted when neither
/// `CUBEJS_DEFAULT_API_SCOPES` nor a user hook is configured. Note that
/// `jobs` is *not* part of it.
pub const DEFAULT_API_SCOPES: [&str; 4] = ["graphql", "meta", "data", "sql"];

/// Default scopes for a configuration: `CUBEJS_DEFAULT_API_SCOPES` when the
/// variable is set (even if empty), [`DEFAULT_API_SCOPES`] otherwise.
pub fn default_scopes(config: &AuthConfig) -> Vec<String> {
    match &config.default_api_scopes {
        Some(scopes) => scopes.clone(),
        None => DEFAULT_API_SCOPES.iter().map(|s| s.to_string()).collect(),
    }
}

/// Default `contextToApiScopes` implementation: ignores the security
/// context and grants `default_scopes`. A user-defined hook will be
/// pluggable in place of this function.
pub fn context_to_api_scopes(_security_context: &Value, default_scopes: &[String]) -> Vec<String> {
    default_scopes.to_vec()
}

/// `assertApiScope`: fails with `API scope is missing: {scope}` (HTTP 403)
/// unless `scope` is in `scopes`.
pub fn assert_api_scope(scopes: &[String], scope: &str) -> Result<(), AuthError> {
    if scopes.iter().any(|s| s == scope) {
        Ok(())
    } else {
        Err(AuthError::ApiScopeMissing(scope.to_string()))
    }
}
