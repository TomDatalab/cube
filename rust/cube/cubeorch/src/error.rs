//! Errors of the orchestrator.

use cubecache::CacheError;
use cubedriver::DriverError;
use cubequeue::QueueError;
use thiserror::Error;

/// Every failure `fetch_query` can produce.
///
/// The variants are `Clone` because the refresh key debouncer hands one result to every
/// caller that joined an in-flight load, the same way the Node `AsyncDebounce` decorator
/// hands them all the same rejected promise.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum OrchError {
    /// The literal the whole continue-wait contract keys off (`QO/ContinueWaitError.ts`,
    /// spec §5). The gateway answers HTTP 200 with `{ error: "Continue wait", stage }`
    /// and the client re-issues the identical request.
    #[error("Continue wait")]
    ContinueWait,
    /// A query handler failed; the message is the one the data source produced.
    #[error("{0}")]
    Execution(String),
    /// `rollupOnlyMode` with nothing built (`QO/QueryOrchestrator.ts:240-245`) and the
    /// other orchestration level refusals.
    #[error("{0}")]
    Orchestration(String),
    #[error("{0}")]
    Cache(String),
    #[error("{0}")]
    Driver(String),
    /// A part of the Node orchestrator that this port does not implement yet. Never
    /// silently degraded into a wrong answer.
    #[error("{0}")]
    NotImplemented(String),
}

impl OrchError {
    pub fn execution(message: impl Into<String>) -> Self {
        OrchError::Execution(message.into())
    }

    pub fn orchestration(message: impl Into<String>) -> Self {
        OrchError::Orchestration(message.into())
    }

    pub fn not_implemented(message: impl Into<String>) -> Self {
        OrchError::NotImplemented(message.into())
    }

    pub fn is_continue_wait(&self) -> bool {
        matches!(self, OrchError::ContinueWait)
    }
}

impl From<QueueError> for OrchError {
    fn from(error: QueueError) -> Self {
        match error {
            QueueError::ContinueWait => OrchError::ContinueWait,
            // `parseResult` raises a plain `Error(result.error)`, so a handler that failed
            // with the literal continue-wait message is a continue wait as well: that is how
            // a pre-aggregation build queued behind another one propagates.
            QueueError::Execution(message) if message == "Continue wait" => OrchError::ContinueWait,
            QueueError::Execution(message) => OrchError::Execution(message),
            other => OrchError::Execution(other.to_string()),
        }
    }
}

impl From<CacheError> for OrchError {
    fn from(error: CacheError) -> Self {
        OrchError::Cache(error.to_string())
    }
}

impl From<DriverError> for OrchError {
    fn from(error: DriverError) -> Self {
        OrchError::Driver(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continue_wait_message_is_literal() {
        assert_eq!(OrchError::ContinueWait.to_string(), "Continue wait");
        assert!(OrchError::from(QueueError::ContinueWait).is_continue_wait());
        assert!(OrchError::from(QueueError::Execution("Continue wait".into())).is_continue_wait());
        assert!(!OrchError::from(QueueError::Execution("boom".into())).is_continue_wait());
    }
}
