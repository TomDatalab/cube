//! Port of `createDefaultCheckAuth` / `createCheckAuthFn` /
//! `createCheckAuthSystemFn` / `createSecurityContextExtractor` from
//! `cubejs-api-gateway/src/gateway.ts`.

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{decode, decode_header, Algorithm, Validation};
use serde_json::{Map, Value};

use crate::config::AuthConfig;
use crate::error::{AuthError, TokenError};
use crate::jwk::{JwkCache, JwkFetcher, KeyFamily, ReqwestJwkFetcher, VerificationKey};

/// Scope a playground-signed token must carry to unlock developer
/// affordances (`DEV_TOKEN_SCOPE` in gateway.ts).
pub const DEV_TOKEN_SCOPE: &str = "dev-token";

/// The verified JWT payload as stored in `req.securityContext` by the
/// Node.js gateway. `Value::Null` stands for `undefined` (only possible when
/// `enforce_security_checks` is off).
pub type SecurityContext = Value;

/// Outcome of a successful [`Authenticator::authenticate`] call.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthResult {
    pub security_context: SecurityContext,
    /// `req.signedWithPlaygroundAuthSecret`: the token was accepted through
    /// the playground secret *and* carries the `dev-token` scope.
    pub signed_with_playground_auth_secret: bool,
}

impl AuthResult {
    fn empty() -> Self {
        Self {
            security_context: Value::Null,
            signed_with_playground_auth_secret: false,
        }
    }
}

/// Everything `jwt.verify` in Node.js checks besides the signature.
struct VerifyOptions<'a> {
    algorithms: Option<&'a [String]>,
    audience: Option<&'a str>,
    issuer: Option<&'a [String]>,
    subject: Option<&'a str>,
}

const HS_ALGS: [Algorithm; 3] = [Algorithm::HS256, Algorithm::HS384, Algorithm::HS512];
const PUB_KEY_ALGS: [Algorithm; 9] = [
    Algorithm::RS256,
    Algorithm::RS384,
    Algorithm::RS512,
    Algorithm::PS256,
    Algorithm::PS384,
    Algorithm::PS512,
    Algorithm::ES256,
    Algorithm::ES384,
    Algorithm::EdDSA,
];

fn algorithm_family(alg: Algorithm) -> KeyFamily {
    match alg {
        Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512 => KeyFamily::Hmac,
        Algorithm::RS256
        | Algorithm::RS384
        | Algorithm::RS512
        | Algorithm::PS256
        | Algorithm::PS384
        | Algorithm::PS512 => KeyFamily::Rsa,
        Algorithm::ES256 | Algorithm::ES384 => KeyFamily::Ec,
        Algorithm::EdDSA => KeyFamily::Ed,
    }
}

/// `Math.floor(Date.now() / 1000)`.
fn clock_timestamp() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as f64)
        .unwrap_or(0.0)
}

/// `jwt.verify(token, key, options)`: signature plus the claim checks the
/// Node.js library performs, in the same order and with the same messages.
fn verify_token(
    token: &str,
    key: &VerificationKey,
    options: &VerifyOptions<'_>,
) -> Result<Value, TokenError> {
    let header = decode_header(token)?;

    let allowed: Vec<Algorithm> = match options.algorithms {
        Some(names) => names
            .iter()
            .filter_map(|name| Algorithm::from_str(name).ok())
            .collect(),
        None => match key.family {
            KeyFamily::Hmac => HS_ALGS.to_vec(),
            _ => PUB_KEY_ALGS.to_vec(),
        },
    };
    if !allowed.contains(&header.alg) {
        return Err(TokenError::InvalidAlgorithm);
    }
    // jsonwebtoken refuses algorithm lists spanning several key families,
    // so keep the ones the key can actually be used with.
    let usable: Vec<Algorithm> = allowed
        .into_iter()
        .filter(|alg| algorithm_family(*alg) == key.family)
        .collect();
    if !usable.contains(&header.alg) {
        return Err(TokenError::InvalidAlgorithm);
    }

    let mut validation = Validation::new(header.alg);
    validation.algorithms = usable;
    validation.validate_exp = false;
    validation.validate_nbf = false;
    validation.validate_aud = false;
    validation.required_spec_claims.clear();
    validation.leeway = 0;

    let payload = decode::<Value>(token, &key.key, &validation)?.claims;

    let now = clock_timestamp();
    if let Some(nbf) = payload.get("nbf") {
        let nbf = nbf.as_f64().ok_or(TokenError::InvalidNbf)?;
        if nbf > now {
            return Err(TokenError::NotBefore);
        }
    }
    if let Some(exp) = payload.get("exp") {
        let exp = exp.as_f64().ok_or(TokenError::InvalidExp)?;
        if now >= exp {
            return Err(TokenError::Expired);
        }
    }

    if let Some(audience) = options.audience {
        let matches = match payload.get("aud") {
            Some(Value::String(aud)) => aud == audience,
            Some(Value::Array(auds)) => auds.iter().any(|aud| aud.as_str() == Some(audience)),
            _ => false,
        };
        if !matches {
            return Err(TokenError::InvalidAudience {
                expected: audience.to_string(),
            });
        }
    }

    if let Some(issuers) = options.issuer {
        let matches = payload
            .get("iss")
            .and_then(Value::as_str)
            .map(|iss| issuers.iter().any(|expected| expected == iss))
            .unwrap_or(false);
        if !matches {
            return Err(TokenError::InvalidIssuer {
                expected: issuers.join(","),
            });
        }
    }

    if let Some(subject) = options.subject {
        if payload.get("sub").and_then(Value::as_str) != Some(subject) {
            return Err(TokenError::InvalidSubject {
                expected: subject.to_string(),
            });
        }
    }

    Ok(payload)
}

/// Tries every candidate key in order (`CUBEJS_API_SECRETS` rotation):
/// token-level failures (expiry, nbf) reproduce for every key, so they are
/// surfaced immediately instead of being shadowed by a later "invalid
/// signature".
fn verify_with_candidates(
    token: &str,
    keys: &[VerificationKey],
    options: &VerifyOptions<'_>,
) -> Result<Value, TokenError> {
    let mut last_error = None;
    for key in keys {
        match verify_token(token, key, options) {
            Ok(payload) => return Ok(payload),
            Err(e @ (TokenError::Expired | TokenError::NotBefore)) => return Err(e),
            Err(e) => last_error = Some(e),
        }
    }
    Err(last_error.unwrap_or(TokenError::NoSecret))
}

/// `hasDevTokenScope`.
pub fn has_dev_token_scope(security_context: &Value) -> bool {
    security_context
        .get("scope")
        .and_then(Value::as_array)
        .map(|scope| scope.iter().any(|s| s.as_str() == Some(DEV_TOKEN_SCOPE)))
        .unwrap_or(false)
}

/// `extractAuthorizationHeaderWithSchema`: `"Bearer <token>"`,
/// `"Authorization: <token>"` and a bare `"<token>"` all yield `<token>`
/// (JS `header.split(' ', 2)` semantics).
pub fn extract_token(authorization_header: &str) -> &str {
    match authorization_header.split_once(' ') {
        Some((_, rest)) => rest.split(' ').next().unwrap_or(""),
        None => authorization_header,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AuthPath {
    Main,
    Playground,
}

/// Verifies REST API tokens (`checkAuthFn` / `checkAuthSystemFn`).
pub struct Authenticator {
    config: AuthConfig,
    /// Candidate keys of the main path (`jwt_key` > `api_secrets` > `api_secret`).
    main_keys: Vec<VerificationKey>,
    /// Set when `jwt_key` looks like a PEM but could not be parsed; every
    /// main-path verification then fails with this error.
    main_key_error: Option<String>,
    /// Present when `jwk_url` is configured; replaces `main_keys`.
    jwk_cache: Option<JwkCache>,
    /// Keys of the playground / system path, HS256 only.
    system_keys: Vec<VerificationKey>,
    u_deprecation_shown: AtomicBool,
}

impl Authenticator {
    /// Builds an authenticator fetching JWK sets over HTTP (`reqwest`).
    pub fn new(config: AuthConfig) -> Self {
        let fetcher: Arc<dyn JwkFetcher> = Arc::new(ReqwestJwkFetcher::new(config.jwk_retry));
        Self::with_jwk_fetcher(config, fetcher)
    }

    /// Builds an authenticator with a custom [`JwkFetcher`].
    pub fn with_jwk_fetcher(config: AuthConfig, fetcher: Arc<dyn JwkFetcher>) -> Self {
        let mut main_key_error = None;
        let main_keys = config
            .candidate_secrets()
            .iter()
            .filter_map(|secret| {
                if secret.contains("-----BEGIN") {
                    match VerificationKey::from_pem(secret.as_bytes()) {
                        Ok(key) => Some(key),
                        Err(e) => {
                            main_key_error = Some(e.to_string());
                            None
                        }
                    }
                } else {
                    Some(VerificationKey::from_secret(secret.as_bytes()))
                }
            })
            .collect();

        let jwk_cache = config
            .jwk_url
            .as_ref()
            .map(|_| JwkCache::new(&config, fetcher));

        // `createCheckAuthSystemFn`: `{ key: playgroundAuthSecret, algorithms: ['HS256'] }`;
        // without a playground secret `createDefaultCheckAuth` falls back to
        // the rotation list / api secret.
        let system_keys = match &config.playground_auth_secret {
            Some(secret) => vec![VerificationKey::from_secret(secret.as_bytes())],
            None => config
                .candidate_secrets()
                .iter()
                .filter(|secret| !secret.contains("-----BEGIN"))
                .map(|secret| VerificationKey::from_secret(secret.as_bytes()))
                .collect(),
        };

        Self {
            config,
            main_keys,
            main_key_error,
            jwk_cache,
            system_keys,
            u_deprecation_shown: AtomicBool::new(false),
        }
    }

    pub fn config(&self) -> &AuthConfig {
        &self.config
    }

    /// Pre-caches the JWK set to speed up the first authentication
    /// (`jwks.fetchOnly` at gateway construction). Errors are logged, not
    /// returned, exactly like the Node.js prefetch.
    pub async fn prefetch_jwks(&self) {
        if let (Some(cache), Some(url)) = (&self.jwk_cache, &self.config.jwk_url) {
            if let Err(e) = cache.fetch(url).await {
                log::warn!("JWKs Prefetching Error: {e}");
            }
        }
    }

    /// `checkAuthFn`: the main verification, falling back to the
    /// playground secret when one is configured. On fallback failure the
    /// *main* error is reported.
    pub async fn authenticate(
        &self,
        authorization_header: Option<&str>,
    ) -> Result<AuthResult, AuthError> {
        let token = Self::token_from_header(authorization_header);

        match self.check(token, AuthPath::Main).await {
            Ok(result) => Ok(result),
            Err(main_error) => {
                if self.config.playground_auth_secret.is_some() {
                    if let Ok(result) = self.check(token, AuthPath::Playground).await {
                        return Ok(result);
                    }
                }
                Err(main_error)
            }
        }
    }

    /// `checkAuthSystemFn`: verification with the playground secret only
    /// (used by the `/cubejs-system/v1/*` routes).
    pub async fn authenticate_system(
        &self,
        authorization_header: Option<&str>,
    ) -> Result<AuthResult, AuthError> {
        self.check(
            Self::token_from_header(authorization_header),
            AuthPath::Playground,
        )
        .await
    }

    /// `createSecurityContextExtractor`: the security context handed to
    /// queries. With `jwt_claims_namespace` it is that claim (or `{}`);
    /// otherwise the legacy `u` claim is merged into the root and removed.
    pub fn extract_security_context(&self, security_context: &Value) -> Value {
        let Some(object) = security_context.as_object() else {
            return Value::Object(Map::new());
        };

        if let Some(namespace) = &self.config.jwt_claims_namespace {
            return object
                .get(namespace)
                .cloned()
                .unwrap_or_else(|| Value::Object(Map::new()));
        }

        match object.get("u") {
            Some(u) if is_truthy(u) => {
                if !self.u_deprecation_shown.swap(true, Ordering::Relaxed) {
                    log::warn!(
                        "JWT U Property Deprecation: Storing security context in the u property \
                         within the payload is now deprecated, please migrate: \
                         https://github.com/cube-js/cube.js/blob/master/DEPRECATION.md#authinfo"
                    );
                }
                let mut merged = object.clone();
                merged.remove("u");
                if let Some(u) = u.as_object() {
                    for (k, v) in u {
                        merged.insert(k.clone(), v.clone());
                    }
                }
                Value::Object(merged)
            }
            _ => security_context.clone(),
        }
    }

    fn token_from_header(authorization_header: Option<&str>) -> Option<&str> {
        authorization_header
            .map(extract_token)
            .filter(|token| !token.is_empty())
    }

    /// The function returned by `createDefaultCheckAuth`.
    async fn check(&self, token: Option<&str>, path: AuthPath) -> Result<AuthResult, AuthError> {
        let enforce = self.config.enforce_security_checks;

        let Some(token) = token else {
            return if enforce {
                Err(AuthError::AuthorizationHeaderMissing)
            } else {
                Ok(AuthResult::empty())
            };
        };

        let verified = match path {
            AuthPath::Main => self.verify_main(token).await,
            AuthPath::Playground => self.verify_system(token),
        };

        match verified {
            Ok(security_context) => {
                let signed_with_playground_auth_secret =
                    path == AuthPath::Playground && has_dev_token_scope(&security_context);
                Ok(AuthResult {
                    security_context,
                    signed_with_playground_auth_secret,
                })
            }
            Err(e) if enforce => Err(AuthError::InvalidToken(e)),
            Err(_) => Ok(AuthResult::empty()),
        }
    }

    fn main_options(&self) -> VerifyOptions<'_> {
        VerifyOptions {
            algorithms: self.config.jwt_algorithms.as_deref(),
            audience: self.config.jwt_audience.as_deref(),
            issuer: self.config.jwt_issuer.as_deref(),
            subject: self.config.jwt_subject.as_deref(),
        }
    }

    async fn verify_main(&self, token: &str) -> Result<Value, TokenError> {
        let options = self.main_options();

        if let (Some(cache), Some(url)) = (&self.jwk_cache, &self.config.jwk_url) {
            let header = decode_header(token).map_err(|_| TokenError::UnableToDecode)?;
            let kid = header
                .kid
                .filter(|kid| !kid.is_empty())
                .ok_or(TokenError::NoKid)?;
            let key = cache
                .get_key_by_kid(url, &kid)
                .await?
                .ok_or(TokenError::JwkNotFound { kid })?;
            return verify_token(token, &key, &options);
        }

        if let Some(reason) = &self.main_key_error {
            return Err(TokenError::InvalidKey(reason.clone()));
        }

        verify_with_candidates(token, &self.main_keys, &options)
    }

    fn verify_system(&self, token: &str) -> Result<Value, TokenError> {
        let algorithms = ["HS256".to_string()];
        let options = VerifyOptions {
            algorithms: Some(&algorithms),
            audience: None,
            issuer: None,
            subject: None,
        };
        verify_with_candidates(token, &self.system_keys, &options)
    }
}

/// JavaScript truthiness for a JSON value.
fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}
