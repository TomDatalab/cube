//! Error types mirroring `CubejsHandlerError` from `cubejs-api-gateway`.
//!
//! The Node.js gateway wraps every failure of the token verification into a
//! single user-facing `Invalid token` error and keeps the original error
//! around for logging (`CubejsHandlerError.originalError`). [`AuthError`] is
//! the user-facing part, [`TokenError`] the detailed cause.

use thiserror::Error;

/// Detailed cause of a token verification failure. Never shown to the
/// client, only meant for logging (`originalError` in the Node.js code).
#[derive(Debug, Error)]
pub enum TokenError {
    /// `TokenExpiredError` in Node.js.
    #[error("jwt expired")]
    Expired,
    /// `NotBeforeError` in Node.js.
    #[error("jwt not active")]
    NotBefore,
    #[error("invalid exp value")]
    InvalidExp,
    #[error("invalid nbf value")]
    InvalidNbf,
    #[error("invalid signature")]
    InvalidSignature,
    #[error("invalid algorithm")]
    InvalidAlgorithm,
    #[error("jwt malformed")]
    Malformed,
    #[error("jwt audience invalid. expected: {expected}")]
    InvalidAudience { expected: String },
    #[error("jwt issuer invalid. expected: {expected}")]
    InvalidIssuer { expected: String },
    #[error("jwt subject invalid. expected: {expected}")]
    InvalidSubject { expected: String },
    /// No secret / key configured at all (upstream `jsonwebtoken` error).
    #[error("secret or public key must be provided")]
    NoSecret,
    /// `CUBEJS_JWT_KEY` looks like a PEM but cannot be parsed.
    #[error("invalid public key: {0}")]
    InvalidKey(String),
    #[error("Unable to decode JWT key")]
    UnableToDecode,
    #[error("JWT without kid inside headers")]
    NoKid,
    #[error("Unable to verify, JWK with kid: \"{kid}\" not found")]
    JwkNotFound { kid: String },
    #[error("{0}")]
    JwkFetch(#[from] JwkError),
    /// Any other error reported by the `jsonwebtoken` crate.
    #[error("{0}")]
    Jwt(jsonwebtoken::errors::Error),
}

impl From<jsonwebtoken::errors::Error> for TokenError {
    fn from(e: jsonwebtoken::errors::Error) -> Self {
        use jsonwebtoken::errors::ErrorKind;

        match e.kind() {
            ErrorKind::InvalidSignature => TokenError::InvalidSignature,
            ErrorKind::InvalidAlgorithm | ErrorKind::InvalidAlgorithmName => {
                TokenError::InvalidAlgorithm
            }
            ErrorKind::InvalidToken | ErrorKind::Base64(_) | ErrorKind::Utf8(_) => {
                TokenError::Malformed
            }
            ErrorKind::ExpiredSignature => TokenError::Expired,
            ErrorKind::ImmatureSignature => TokenError::NotBefore,
            _ => TokenError::Jwt(e),
        }
    }
}

/// Errors produced while fetching and parsing a JWK set.
#[derive(Debug, Error)]
pub enum JwkError {
    #[error("Unable to find keys inside response from JWK_URL")]
    NoKeys,
    #[error("Unable to find kid inside JWK")]
    NoKid,
    #[error("Unable to convert JWK with kid: \"{kid}\": {reason}")]
    InvalidJwk { kid: String, reason: String },
    #[error("JWK fetch error: {0}")]
    Fetch(String),
}

/// User-facing authentication / authorization error.
///
/// `Display` yields the same message the Node.js gateway puts into the
/// `{ "error": "..." }` response body.
#[derive(Debug, Error)]
pub enum AuthError {
    /// The request had no (or an empty) `Authorization` header.
    #[error("Authorization header isn't set")]
    AuthorizationHeaderMissing,
    /// The token could not be verified; see [`TokenError`] for the cause.
    #[error("Invalid token")]
    InvalidToken(#[source] TokenError),
    /// `assert_api_scope` failure.
    #[error("API scope is missing: {0}")]
    ApiScopeMissing(String),
}

impl AuthError {
    /// HTTP status code the error maps to.
    ///
    /// Authentication failures are `401`; a missing API scope is `403`
    /// (exactly what the Node.js gateway returns for scopes). Note that the
    /// Express gateway still answers `403` for authentication failures with
    /// a `@todo Move it to 401` comment; the native Rust gateway already
    /// uses `401`, and so do we.
    pub fn status_code(&self) -> u16 {
        match self {
            // Express answers 403 (`CubejsHandlerError(403, 'Forbidden', ...)`,
            // with a `@todo Move it to 401`); keep the public contract.
            AuthError::AuthorizationHeaderMissing | AuthError::InvalidToken(_) => 403,
            AuthError::ApiScopeMissing(_) => 403,
        }
    }

    /// The `type` of the corresponding `CubejsHandlerError`.
    pub fn error_type(&self) -> &'static str {
        match self {
            AuthError::AuthorizationHeaderMissing | AuthError::InvalidToken(_) => "Unauthorized",
            AuthError::ApiScopeMissing(_) => "Forbidden",
        }
    }

    /// Detailed cause for logging (`originalError` in Node.js), if any.
    pub fn cause(&self) -> Option<&TokenError> {
        match self {
            AuthError::InvalidToken(cause) => Some(cause),
            _ => None,
        }
    }
}
