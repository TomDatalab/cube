//! Issuing a token, for the Playground.
//!
//! The rest of this crate only verifies tokens. The Playground is the one
//! caller that needs one minted: the Node.js dev server hands the browser
//! `jwt.sign({}, apiSecret, { expiresIn: '1d' })` so the single-page app can
//! call the REST API (`cubejs-server-core/src/core/DevServer.ts:52`).
//!
//! Signing lives here rather than in the server so that the secret is read in
//! one place, through the same [`AuthConfig`] the verifier uses.

use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{encode, EncodingKey, Header};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::config::AuthConfig;

/// Why a token could not be issued. Separate from [`crate::TokenError`],
/// which describes why an incoming token was rejected.
#[derive(Debug, Error)]
pub enum IssueError {
    #[error("Cannot issue a token: no API secret is configured (CUBEJS_API_SECRET)")]
    NoSecret,
    #[error("Failed to sign the token: {0}")]
    Signing(String),
}

/// One day, the expiry the Node.js dev server uses.
pub const DEFAULT_TOKEN_LIFETIME_SECONDS: u64 = 24 * 60 * 60;

/// Signs `claims` with the deployment's API secret, HS256.
///
/// `lifetime_seconds` becomes the `exp` claim. An `exp` already in `claims`
/// is kept, so a caller can ask for something other than the default.
///
/// Fails when no secret is configured: a token signed with a guessable
/// fallback would be worse than no token, since the Node.js default of
/// `'secret'` is public knowledge.
pub fn issue_token(
    config: &AuthConfig,
    claims: Map<String, Value>,
    lifetime_seconds: u64,
) -> Result<String, IssueError> {
    let secret = config
        .candidate_secrets()
        .into_iter()
        .next()
        .filter(|secret| !secret.is_empty())
        .ok_or(IssueError::NoSecret)?;

    let mut claims = claims;
    claims.entry("exp".to_string()).or_insert_with(|| {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Value::from(now + lifetime_seconds)
    });

    encode(
        &Header::default(),
        &Value::Object(claims),
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| IssueError::Signing(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AuthConfigBuilder;

    fn config() -> AuthConfig {
        AuthConfigBuilder::default().api_secret("a-secret").build()
    }

    /// The point of the whole module: the Playground's token must pass the
    /// server's own check, or the app authenticates against nothing.
    #[tokio::test]
    async fn a_token_it_issues_is_a_token_the_server_accepts() {
        let token = issue_token(&config(), Map::new(), DEFAULT_TOKEN_LIFETIME_SECONDS)
            .expect("a signed token");

        // Three dot-separated segments, i.e. a JWS.
        assert_eq!(token.split('.').count(), 3, "{token}");

        crate::Authenticator::new(config())
            .authenticate(Some(&format!("Bearer {token}")))
            .await
            .expect("the server accepts its own token");
    }

    #[tokio::test]
    async fn a_token_signed_with_another_secret_is_rejected() {
        let other = AuthConfigBuilder::default()
            .api_secret("a-different-secret")
            .build();
        let token = issue_token(&other, Map::new(), 60).expect("a signed token");

        crate::Authenticator::new(config())
            .authenticate(Some(&format!("Bearer {token}")))
            .await
            .expect_err("a foreign signature must not pass");
    }

    #[tokio::test]
    async fn an_expired_token_is_rejected() {
        let mut claims = Map::new();
        claims.insert("exp".to_string(), Value::from(1_000_000_000_u64));
        let token = issue_token(&config(), claims, 60).expect("a signed token");

        crate::Authenticator::new(config())
            .authenticate(Some(&format!("Bearer {token}")))
            .await
            .expect_err("an expired token must not pass");
    }

    #[test]
    fn the_expiry_is_set_from_the_lifetime() {
        let token = issue_token(&config(), Map::new(), 60).expect("a signed token");
        let claims = decode_claims(&token);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let exp = claims["exp"].as_u64().expect("an exp claim");

        assert!(exp > now, "the token is already expired");
        assert!(exp <= now + 60, "the expiry is further out than asked");
    }

    #[test]
    fn claims_are_carried_through() {
        let mut claims = Map::new();
        claims.insert("uid".to_string(), Value::from(7));

        let token = issue_token(&config(), claims, 60).expect("a signed token");
        assert_eq!(decode_claims(&token)["uid"], Value::from(7));
    }

    #[test]
    fn an_explicit_expiry_is_kept() {
        let mut claims = Map::new();
        claims.insert("exp".to_string(), Value::from(2_000_000_000_u64));

        let token = issue_token(&config(), claims, 60).expect("a signed token");
        assert_eq!(decode_claims(&token)["exp"], Value::from(2_000_000_000_u64));
    }

    #[test]
    fn without_a_secret_no_token_is_issued() {
        let error = issue_token(
            &AuthConfig::default(),
            Map::new(),
            DEFAULT_TOKEN_LIFETIME_SECONDS,
        )
        .expect_err("a config with no secret cannot sign");

        assert!(format!("{error}").contains("CUBEJS_API_SECRET"), "{error}");
    }

    /// Reads the payload without verifying, which is all these tests need.
    fn decode_claims(token: &str) -> Value {
        use base64::Engine;

        let payload = token.split('.').nth(1).expect("a payload segment");
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .expect("base64url");

        serde_json::from_slice(&bytes).expect("json claims")
    }
}
