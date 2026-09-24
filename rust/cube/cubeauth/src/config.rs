//! Authentication configuration, read from the same `CUBEJS_*` variables the
//! Node.js server (`@cubejs-backend/shared` `env.ts` + `OptsHandler`) uses.

use std::time::Duration;

use thiserror::Error;

/// Default JWK cache lifetime when the response has no `Cache-Control:
/// max-age` and `jwk_default_expire` is not set (`jwk.ts`).
pub const DEFAULT_JWK_EXPIRE: Duration = Duration::from_secs(5 * 60);
/// Default `jwkRefetchWindow` (`jwk.ts`).
pub const DEFAULT_JWK_REFETCH_WINDOW: Duration = Duration::from_secs(60);
/// Default `jwkRetry` (`jwk.ts`).
pub const DEFAULT_JWK_RETRY: u32 = 3;

#[derive(Debug, Error)]
pub enum AuthConfigError {
    #[error("Invalid value for {name}: {value:?}, {reason}")]
    InvalidValue {
        name: &'static str,
        value: String,
        reason: &'static str,
    },
}

/// Authentication settings of the REST API.
#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// `CUBEJS_API_SECRET`.
    pub api_secret: Option<String>,
    /// `CUBEJS_API_SECRETS` rotation list. Takes precedence over
    /// [`api_secret`](Self::api_secret) when non-empty.
    pub api_secrets: Vec<String>,
    /// `CUBEJS_JWT_KEY` (`jwt.key`). Wins over both `api_secrets` and
    /// `api_secret`. Either an HMAC secret or a PEM encoded public key.
    pub jwt_key: Option<String>,
    /// `CUBEJS_JWK_URL` (`jwt.jwkUrl`). When set, tokens are verified with
    /// the JWK set fetched from this URL instead of the secrets.
    pub jwk_url: Option<String>,
    /// `CUBEJS_JWT_ALGS` (`jwt.algorithms`). `None` means the
    /// `jsonwebtoken` defaults: HS256/384/512 for a secret, RS/PS/ES for a
    /// public key.
    pub jwt_algorithms: Option<Vec<String>>,
    /// `CUBEJS_JWT_AUDIENCE` (`jwt.audience`).
    pub jwt_audience: Option<String>,
    /// `CUBEJS_JWT_ISSUER` (`jwt.issuer`), comma separated list.
    pub jwt_issuer: Option<Vec<String>>,
    /// `CUBEJS_JWT_SUBJECT` (`jwt.subject`).
    pub jwt_subject: Option<String>,
    /// `CUBEJS_JWT_CLAIMS_NAMESPACE` (`jwt.claimsNamespace`).
    pub jwt_claims_namespace: Option<String>,
    /// `jwt.jwkRetry`.
    pub jwk_retry: u32,
    /// `jwt.jwkDefaultExpire`.
    pub jwk_default_expire: Option<Duration>,
    /// `jwt.jwkRefetchWindow`.
    pub jwk_refetch_window: Duration,
    /// `CUBEJS_PLAYGROUND_AUTH_SECRET`.
    pub playground_auth_secret: Option<String>,
    /// `CUBEJS_DEFAULT_API_SCOPES`. `None` when the variable is unset;
    /// `Some(vec![])` when it is set but empty (which denies everything,
    /// like the Node.js gateway does).
    pub default_api_scopes: Option<Vec<String>>,
    /// `CUBEJS_DEV_MODE`.
    pub dev_mode: bool,
    /// `enforceSecurityChecks`: when `false` the Node.js gateway swallows
    /// authentication failures and continues with an empty security
    /// context. Node.js derives it from `NODE_ENV === 'production'`.
    pub enforce_security_checks: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            api_secret: None,
            api_secrets: Vec::new(),
            jwt_key: None,
            jwk_url: None,
            jwt_algorithms: None,
            jwt_audience: None,
            jwt_issuer: None,
            jwt_subject: None,
            jwt_claims_namespace: None,
            jwk_retry: DEFAULT_JWK_RETRY,
            jwk_default_expire: None,
            jwk_refetch_window: DEFAULT_JWK_REFETCH_WINDOW,
            playground_auth_secret: None,
            default_api_scopes: None,
            dev_mode: false,
            enforce_security_checks: true,
        }
    }
}

impl AuthConfig {
    /// Builder entry point (see [`AuthConfigBuilder`]).
    pub fn builder() -> AuthConfigBuilder {
        AuthConfigBuilder::default()
    }

    /// Reads the configuration from the process environment.
    pub fn from_env() -> Result<Self, AuthConfigError> {
        Self::from_env_with(|name| std::env::var(name).ok())
    }

    /// Reads the configuration through `lookup` (a `std::env::var`
    /// replacement, handy for tests).
    pub fn from_env_with<F>(lookup: F) -> Result<Self, AuthConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        // env-var `.asString()` yields '' for an empty variable, which every
        // consumer then treats as "not set" (falsy).
        let string = |name: &str| lookup(name).filter(|v| !v.is_empty());
        // env-var `.asArray(',')`: set → split on ',' dropping empty items
        // (an empty variable yields `[]`, which is truthy in JS); unset → undefined.
        let array = |name: &str| {
            lookup(name).map(|v| {
                v.split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
            })
        };
        // env-var `.asBoolStrict()`.
        let bool_strict = |name: &'static str| -> Result<bool, AuthConfigError> {
            match lookup(name).as_deref() {
                None | Some("") | Some("false") => Ok(false),
                Some("true") => Ok(true),
                Some(other) => Err(AuthConfigError::InvalidValue {
                    name,
                    value: other.to_string(),
                    reason: "should be either \"true\" or \"false\"",
                }),
            }
        };

        // Comma-separated rotation list. Trimmed, empties dropped, deduplicated.
        let api_secrets = string("CUBEJS_API_SECRETS")
            .map(|raw| {
                let mut unique: Vec<String> = Vec::new();
                for secret in raw.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
                    if !unique.iter().any(|s| s == secret) {
                        unique.push(secret.to_string());
                    }
                }
                unique
            })
            .unwrap_or_default();

        Ok(Self {
            api_secret: string("CUBEJS_API_SECRET"),
            api_secrets,
            jwt_key: string("CUBEJS_JWT_KEY"),
            jwk_url: string("CUBEJS_JWK_URL"),
            jwt_algorithms: array("CUBEJS_JWT_ALGS"),
            jwt_audience: string("CUBEJS_JWT_AUDIENCE"),
            jwt_issuer: array("CUBEJS_JWT_ISSUER"),
            jwt_subject: string("CUBEJS_JWT_SUBJECT"),
            jwt_claims_namespace: string("CUBEJS_JWT_CLAIMS_NAMESPACE"),
            playground_auth_secret: string("CUBEJS_PLAYGROUND_AUTH_SECRET"),
            default_api_scopes: array("CUBEJS_DEFAULT_API_SCOPES"),
            dev_mode: bool_strict("CUBEJS_DEV_MODE")?,
            enforce_security_checks: lookup("NODE_ENV").as_deref() == Some("production"),
            ..Self::default()
        })
    }

    /// Secrets the main (non-playground) HMAC path tries, in order:
    /// `jwt_key` wins, then the `api_secrets` rotation list, then
    /// `api_secret`. Empty when nothing is configured.
    pub fn candidate_secrets(&self) -> Vec<String> {
        if let Some(key) = &self.jwt_key {
            vec![key.clone()]
        } else if !self.api_secrets.is_empty() {
            self.api_secrets.clone()
        } else if let Some(secret) = &self.api_secret {
            vec![secret.clone()]
        } else {
            Vec::new()
        }
    }
}

/// Fluent builder for [`AuthConfig`], mostly for tests.
#[derive(Debug, Default, Clone)]
pub struct AuthConfigBuilder {
    config: AuthConfig,
}

impl AuthConfigBuilder {
    pub fn api_secret(mut self, secret: impl Into<String>) -> Self {
        self.config.api_secret = Some(secret.into());
        self
    }

    pub fn api_secrets<I, S>(mut self, secrets: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.config.api_secrets = secrets.into_iter().map(Into::into).collect();
        self
    }

    pub fn jwt_key(mut self, key: impl Into<String>) -> Self {
        self.config.jwt_key = Some(key.into());
        self
    }

    pub fn jwk_url(mut self, url: impl Into<String>) -> Self {
        self.config.jwk_url = Some(url.into());
        self
    }

    pub fn jwt_algorithms<I, S>(mut self, algorithms: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.config.jwt_algorithms = Some(algorithms.into_iter().map(Into::into).collect());
        self
    }

    pub fn jwt_audience(mut self, audience: impl Into<String>) -> Self {
        self.config.jwt_audience = Some(audience.into());
        self
    }

    pub fn jwt_issuer<I, S>(mut self, issuers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.config.jwt_issuer = Some(issuers.into_iter().map(Into::into).collect());
        self
    }

    pub fn jwt_subject(mut self, subject: impl Into<String>) -> Self {
        self.config.jwt_subject = Some(subject.into());
        self
    }

    pub fn jwt_claims_namespace(mut self, namespace: impl Into<String>) -> Self {
        self.config.jwt_claims_namespace = Some(namespace.into());
        self
    }

    pub fn jwk_retry(mut self, retry: u32) -> Self {
        self.config.jwk_retry = retry;
        self
    }

    pub fn jwk_default_expire(mut self, expire: Duration) -> Self {
        self.config.jwk_default_expire = Some(expire);
        self
    }

    pub fn jwk_refetch_window(mut self, window: Duration) -> Self {
        self.config.jwk_refetch_window = window;
        self
    }

    pub fn playground_auth_secret(mut self, secret: impl Into<String>) -> Self {
        self.config.playground_auth_secret = Some(secret.into());
        self
    }

    pub fn default_api_scopes<I, S>(mut self, scopes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.config.default_api_scopes = Some(scopes.into_iter().map(Into::into).collect());
        self
    }

    pub fn dev_mode(mut self, dev_mode: bool) -> Self {
        self.config.dev_mode = dev_mode;
        self
    }

    pub fn enforce_security_checks(mut self, enforce: bool) -> Self {
        self.config.enforce_security_checks = enforce;
        self
    }

    pub fn build(self) -> AuthConfig {
        self.config
    }
}
