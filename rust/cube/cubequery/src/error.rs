use thiserror::Error;

/// Error produced while parsing, validating or normalizing a query.
///
/// Mirrors the two error classes the Node.js gateway distinguishes:
/// `UserError` (a client mistake, answered with HTTP 400) and a plain `Error`
/// (answered with HTTP 500). Messages are user facing and kept identical to
/// the Node.js implementation wherever the Node code has a fixed message.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum QueryError {
    /// Port of the gateway's `UserError`.
    #[error("{0}")]
    User(String),
    /// Port of a plain `Error` thrown by the gateway.
    #[error("{0}")]
    Internal(String),
}

impl QueryError {
    pub fn user(message: impl Into<String>) -> Self {
        QueryError::User(message.into())
    }

    pub fn internal(message: impl Into<String>) -> Self {
        QueryError::Internal(message.into())
    }

    /// `true` when the error is a client mistake (`UserError` in Node.js).
    pub fn is_user_error(&self) -> bool {
        matches!(self, QueryError::User(_))
    }

    /// The user facing message.
    pub fn message(&self) -> &str {
        match self {
            QueryError::User(m) | QueryError::Internal(m) => m,
        }
    }
}
