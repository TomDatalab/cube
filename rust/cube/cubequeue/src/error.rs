use thiserror::Error;

/// Errors `QueryQueue` surfaces to its callers.
#[derive(Debug, Error)]
pub enum QueueError {
    /// The blocking wait for a result timed out. The message is the literal the whole
    /// continue-wait contract keys off: the gateway answers HTTP 200 with
    /// `{ error: "Continue wait" }` and the client re-issues the identical request
    /// (`QO/ContinueWaitError.ts`, spec §5).
    #[error("Continue wait")]
    ContinueWait,
    /// The query handler failed. Carries the message the handler produced, matching
    /// `parseResult`'s `throw new Error(result.error)`.
    #[error("{0}")]
    Execution(String),
    /// The queue storage failed.
    #[error("{0}")]
    Driver(String),
    #[error("Priority should be between -10000 and 10000")]
    InvalidPriority,
    /// A persistent query was asked for where no stream can be produced: through
    /// [`crate::QueryQueue::execute_in_queue`], which answers with a value, or on the
    /// `skipQueue` path, which has no queue item to hang a stream on
    /// (`QO/QueryQueue.ts:233-235`).
    #[error("Streaming queries to Cube Store aren't supported")]
    StreamingUnsupported,
}

impl QueueError {
    pub fn driver(message: impl Into<String>) -> Self {
        QueueError::Driver(message.into())
    }

    pub fn is_continue_wait(&self) -> bool {
        matches!(self, QueueError::ContinueWait)
    }
}

/// Raised inside `executeQuery` when the handler outruns `executionTimeout`; it never
/// reaches the caller, it is stored as the query's error result
/// (`QO/TimeoutError.ts`, `QO/QueryQueue.ts:648-666`).
#[derive(Debug, Error)]
#[error("Query execution timeout after {0} s of waiting")]
pub struct TimeoutError(pub u64);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continue_wait_message_is_literal() {
        assert_eq!(QueueError::ContinueWait.to_string(), "Continue wait");
        assert!(QueueError::ContinueWait.is_continue_wait());
        assert!(!QueueError::Execution("Continue wait".into()).is_continue_wait());
    }
}
