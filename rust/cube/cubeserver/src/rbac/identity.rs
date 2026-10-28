//! Passwords (argon2id) and the user a token speaks for.

use std::sync::LazyLock;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use serde_json::Value;

use super::{unauthenticated, unavailable, Rbac};
use crate::error::ApiError;

pub const MIN_PASSWORD_LENGTH: usize = 10;

/// argon2id with the crate's default (OWASP) parameters.
pub fn hash_password(password: &str) -> Result<String, ApiError> {
    if password.chars().count() < MIN_PASSWORD_LENGTH {
        return Err(ApiError::bad_request(format!(
            "The password must have at least {MIN_PASSWORD_LENGTH} characters"
        )));
    }
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| {
            tracing::error!("Password hashing failed: {e}");
            ApiError::internal()
        })
}

/// Constant-time comparison; a malformed hash never matches.
pub fn verify_password(hash: &str, password: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

/// Checked when the user does not exist, so an unknown name costs the same
/// time as a wrong password.
static DUMMY_HASH: LazyLock<String> = LazyLock::new(|| {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(b"no user has this password", &salt)
        .map(|hash| hash.to_string())
        .unwrap_or_default()
});

/// The user `username` when `password` is theirs and they are enabled.
///
/// Runs the hash on a blocking thread. Unknown users are checked against a
/// dummy hash so their answer takes as long as a wrong password.
pub(crate) async fn check_credentials(
    rbac: &Rbac,
    username: &str,
    password: &str,
) -> Result<Option<crate::rbac::store::User>, ApiError> {
    let user = rbac.store.user(username).await.map_err(|e| {
        tracing::error!("RBAC store unavailable: {e}");
        unavailable()
    })?;
    let hash = match &user {
        Some(user) => user.password_hash.clone(),
        None => DUMMY_HASH.clone(),
    };
    let password = password.to_string();
    let matches = tokio::task::spawn_blocking(move || verify_password(&hash, &password))
        .await
        .map_err(|_| ApiError::internal())?;

    Ok(user.filter(|user| matches && !user.disabled))
}

/// Resolve o usuário do token: 401 sem `sub`, usuário inexistente/desativado,
/// ou `iat` < password_changed_at.
pub async fn resolve_identity(
    rbac: &Rbac,
    security_context: &Value,
) -> Result<(String, bool /*platform admin*/), ApiError> {
    let sub = security_context
        .get("sub")
        .and_then(Value::as_str)
        .filter(|sub| !sub.is_empty())
        .ok_or_else(unauthenticated)?;
    // Tokens without `iat` cannot be compared with the last password change.
    let issued_at = security_context
        .get("iat")
        .and_then(Value::as_f64)
        .ok_or_else(unauthenticated)?;

    let user = rbac
        .store
        .user(sub)
        .await
        .map_err(|e| {
            tracing::error!("RBAC store unavailable: {e}");
            unavailable()
        })?
        .filter(|user| !user.disabled)
        .ok_or_else(unauthenticated)?;

    // `iat` carries milliseconds (see `issue_user_token`), so a login right
    // after a password change is not mistaken for an older token.
    if iat_millis(issued_at) < user.password_changed_at.timestamp_millis() {
        return Err(unauthenticated());
    }

    let platform_admin = rbac.is_platform_admin(&user.id).await?;
    Ok((user.id, platform_admin))
}

/// Milliseconds of a NumericDate. Rounded, not truncated: `x.123` is stored
/// as a float slightly below or above it.
fn iat_millis(iat: f64) -> i64 {
    (iat * 1000.0).round() as i64
}

/// Claims of a token for `user`, valid for `ttl`: `{sub, iat, exp}`.
///
/// `iat` is a NumericDate with millisecond precision (RFC 7519 allows
/// fractions), compared with `password_changed_at` on every request.
pub fn user_token_claims(
    user: &str,
    ttl: std::time::Duration,
) -> (
    serde_json::Map<String, Value>,
    chrono::DateTime<chrono::Utc>,
) {
    let now = chrono::Utc::now();
    let expires_at = now + chrono::Duration::seconds(ttl.as_secs() as i64);
    let mut claims = serde_json::Map::new();
    claims.insert("sub".into(), Value::from(user));
    claims.insert(
        "iat".into(),
        Value::from(now.timestamp_millis() as f64 / 1000.0),
    );
    claims.insert("exp".into(), Value::from(expires_at.timestamp()));
    (claims, expires_at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iat_round_trips_to_the_millisecond() {
        let base = chrono::Utc::now().timestamp_millis();
        for ms in base..base + 5000 {
            assert_eq!(iat_millis(ms as f64 / 1000.0), ms);
        }
        let (claims, _) = user_token_claims("ann", std::time::Duration::from_secs(60));
        assert!(claims["iat"].as_f64().unwrap() >= base as f64 / 1000.0);
    }
}
