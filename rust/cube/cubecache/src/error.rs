use thiserror::Error;

#[derive(Debug, Error)]
pub enum CacheError {
    #[error("Cache driver error: {0}")]
    Driver(String),
    #[error("Cache value is not valid JSON: {0}")]
    Serde(#[from] serde_json::Error),
}

impl CacheError {
    pub fn driver(message: impl Into<String>) -> Self {
        CacheError::Driver(message.into())
    }
}
