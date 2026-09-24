//! Error type shared by every driver.

use std::fmt;

/// Result alias used throughout the crate.
pub type Result<T, E = DriverError> = std::result::Result<T, E>;

/// Errors raised by the driver layer.
///
/// The variants mirror the error classes of the Node.js drivers:
/// `InvalidConfiguration`/`TypeError` thrown by `getEnv` → [`DriverError::Config`],
/// `ConnectionError` → [`DriverError::Connection`], `PostgresError` and raw
/// database errors → [`DriverError::Query`] / [`DriverError::Database`].
#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    /// Invalid or missing configuration (environment variables, options).
    #[error("{0}")]
    Config(String),

    /// The driver could not establish a connection.
    #[error("Unable to connect to the database ({pool_name}): {message}")]
    Connection { pool_name: String, message: String },

    /// Waiting for a pooled connection timed out
    /// (`ResourceRequest timed out (<pool>)` in the Node.js pool wrapper).
    #[error("ResourceRequest timed out ({0})")]
    PoolTimeout(String),

    /// Error reported by the database server while executing a statement.
    /// `message` is the bare server message (e.g. `relation "x" does not exist`),
    /// `code` is the SQLSTATE when known.
    #[error("{message}")]
    Database {
        message: String,
        code: Option<String>,
    },

    /// Driver-side error while preparing, sending or decoding a query.
    #[error("{0}")]
    Query(String),

    /// Value/type conversion failure while decoding a result set.
    #[error("{0}")]
    TypeDetection(String),

    /// The operation is not supported by this driver.
    #[error("{0}")]
    NotImplemented(String),

    /// The requested `CUBEJS_DB_TYPE` has no Rust implementation yet.
    #[error("Database driver is not implemented in Rust yet: {0}")]
    UnsupportedDriver(String),

    /// The requested `CUBEJS_DB_TYPE` existed in Node.js but is deliberately
    /// not carried over (e.g. it needs a JVM); the message names the native
    /// replacement.
    #[error("Database driver {db_type} is not available in the Rust server: {reason}")]
    DroppedDriver { db_type: String, reason: String },

    /// The requested `CUBEJS_DB_TYPE` is not a known Cube driver at all.
    #[error("Unknown database type: {0}")]
    UnknownDriver(String),

    /// I/O errors (reading SSL certificate files, ...).
    #[error("{0}")]
    Io(#[from] std::io::Error),

    /// Anything else.
    #[error("{0}")]
    Other(String),
}

impl DriverError {
    /// Builds a [`DriverError::Config`] from any displayable value.
    pub fn config(msg: impl fmt::Display) -> Self {
        DriverError::Config(msg.to_string())
    }

    /// Builds a [`DriverError::Query`] from any displayable value.
    pub fn query(msg: impl fmt::Display) -> Self {
        DriverError::Query(msg.to_string())
    }

    /// Mirrors `InvalidConfiguration` from `@cubejs-backend/shared`.
    pub fn invalid_configuration(key: &str, value: &str, description: &str) -> Self {
        DriverError::Config(format!(
            "Value \"{value}\" is not valid for {key}. {description}"
        ))
    }
}

impl From<serde_json::Error> for DriverError {
    fn from(e: serde_json::Error) -> Self {
        DriverError::TypeDetection(e.to_string())
    }
}

impl From<tokio_postgres::Error> for DriverError {
    fn from(e: tokio_postgres::Error) -> Self {
        if let Some(db) = e.as_db_error() {
            return DriverError::Database {
                message: db.message().to_string(),
                code: Some(db.code().code().to_string()),
            };
        }
        // `tokio_postgres::Error` prefixes the kind ("error connecting to server: ...");
        // keep the full text, it is what the user needs to see.
        DriverError::Query(e.to_string())
    }
}
