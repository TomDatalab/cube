//! Queue value types.
//!
//! Port of `packages/cubejs-base-driver/src/queue-driver.interface.ts` and the query
//! definition `QueryQueue` puts on the queue.

use cubecache::CacheKey;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Primary key of a queue item. Cube Store hands out integers, the local driver counts.
pub type QueueId = u64;

/// `[keyHash, queueId]`
pub type QueryKeysTuple = (String, QueueId);

/// Lowest priority `executeInQueue` accepts (`QO/QueryQueue.ts:245-247`).
pub const MIN_PRIORITY: i32 = -10000;
/// Highest priority `executeInQueue` accepts.
pub const MAX_PRIORITY: i32 = 10000;

/// Higher priority wins, older wins within a priority. Only the rungs below carry meaning,
/// the range between them is open: `queuePriority` in a query body and `priority` on a
/// pre-aggregation are arbitrary integers from -10000 to 10000.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(i32)]
pub enum QueuePriority {
    /// A request is blocked on it: a user query, an awaited build, a refresh key
    Interactive = 10,
    /// Warmup sweep, above the background builds it warms
    Warmup = 1,
    /// A build nobody is waiting for
    Background = 0,
    /// Scheduled refresh, newest partition first from here downwards
    Scheduled = -1,
}

impl QueuePriority {
    pub const fn value(self) -> i32 {
        self as i32
    }
}

impl From<QueuePriority> for i32 {
    fn from(priority: QueuePriority) -> Self {
        priority.value()
    }
}

/// A queued query, the payload of `addToQueue` (`CSD/CubeStoreQueueDriver.ts:102-110`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryDef {
    pub queue_id: QueueId,
    pub query_handler: String,
    pub query: Value,
    pub query_key: CacheKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_query_key: Option<CacheKey>,
    pub priority: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub added_to_queue_time: i64,
    /// Merged in by `executeQuery` once the handler starts (`MERGE_EXTRA`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_query_time: Option<i64>,
    /// Whatever the query handler handed to its `setCancelHandler` callback. The cancel
    /// handler receives the definition with this field set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel_handler: Option<Value>,
}

impl QueryDef {
    /// `query.isJob` — a job never blocks on the result and never raises continue wait.
    pub fn is_job(&self) -> bool {
        query_flag(&self.query, "isJob")
    }

    /// `query.forceBuild`
    pub fn is_force_build(&self) -> bool {
        query_flag(&self.query, "forceBuild")
    }

    /// `query.orphanedTimeout`, in seconds.
    pub fn orphaned_timeout(&self) -> Option<u64> {
        query_u64(&self.query, "orphanedTimeout")
    }
}

/// Reads a boolean flag off the opaque query payload, JavaScript truthiness included.
pub fn query_flag(query: &Value, name: &str) -> bool {
    match query.get(name) {
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => value.as_f64().is_some_and(|value| value != 0.0),
        Some(Value::String(value)) => !value.is_empty(),
        _ => false,
    }
}

/// Reads a numeric field off the opaque query payload.
pub fn query_u64(query: &Value, name: &str) -> Option<u64> {
    query.get(name).and_then(Value::as_u64)
}

/// The fields `optimisticQueryUpdate` merges into a query definition.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QueryDefUpdate {
    pub start_query_time: Option<i64>,
    pub cancel_handler: Option<Value>,
}

impl QueryDefUpdate {
    pub fn start_query_time(value: i64) -> Self {
        Self {
            start_query_time: Some(value),
            ..Default::default()
        }
    }

    pub fn cancel_handler(value: Value) -> Self {
        Self {
            cancel_handler: Some(value),
            ..Default::default()
        }
    }

    pub fn apply(&self, def: &mut QueryDef) {
        if let Some(start_query_time) = self.start_query_time {
            def.start_query_time = Some(start_query_time);
        }

        if let Some(cancel_handler) = &self.cancel_handler {
            def.cancel_handler = Some(cancel_handler.clone());
        }
    }
}

/// What a query handler produced, as stored by `setResultAndRemoveQuery`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExecutionResult {
    Success {
        result: Value,
    },
    Error {
        error: String,
    },
    /// What a persistent query stores instead of rows: the rows went to the consumer
    /// through a `QueryStream`, and only the fact that the query ran is left to record
    /// (`QO/QueryQueue.ts:946-949`, "CubeStore has special handling for null").
    #[serde(rename_all = "camelCase")]
    Stream {
        stream_result: bool,
    },
}

impl ExecutionResult {
    pub fn success(result: Value) -> Self {
        ExecutionResult::Success { result }
    }

    pub fn error(error: impl Into<String>) -> Self {
        ExecutionResult::Error {
            error: error.into(),
        }
    }

    pub fn stream() -> Self {
        ExecutionResult::Stream {
            stream_result: true,
        }
    }

    /// A result of a persistent query carries no rows.
    pub fn is_stream(&self) -> bool {
        matches!(self, ExecutionResult::Stream { .. })
    }
}

/// `RetrieveForProcessingSuccess`
#[derive(Clone, Debug, PartialEq)]
pub struct RetrieveForProcessingSuccess {
    /// Active keys of this queue prefix, this one included.
    pub active: Vec<String>,
    /// Number of queries still waiting to be processed.
    pub queue_size: usize,
    pub def: QueryDef,
}

/// `AddToQueueResponse`
#[derive(Clone, Debug, PartialEq)]
pub struct AddToQueueResponse {
    /// `1` when this call put the query on the queue, `0` when it was already there.
    pub added: u32,
    pub queue_id: QueueId,
    pub queue_size: usize,
    pub added_to_queue_time: i64,
    /// `None` when the driver did not retrieve the item for processing in the same
    /// operation; the query then waits for reconciliation to pick it up.
    pub retrieved: Option<RetrieveForProcessingSuccess>,
}

/// `AddToQueueOptions`
#[derive(Clone, Debug, Default)]
pub struct AddToQueueOptions {
    pub queue_id: QueueId,
    pub stage_query_key: Option<CacheKey>,
    pub request_id: Option<String>,
    pub span_id: Option<String>,
    /// Overrides the queue wide `orphanedTimeout`, in seconds.
    pub orphaned_timeout: Option<u64>,
    /// The request uuid without its span suffix, used by Cube Store to hand the same
    /// result to every poll of one request.
    pub external_id: Option<String>,
}

/// `QueryStageStateResponse`
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QueryStageState {
    pub active: Vec<String>,
    pub to_process: Vec<String>,
    /// Empty when only the keys were requested. Kept in insertion order, which is what
    /// `Object.values(allQueryDefs)` iterates in Node.
    pub defs: Vec<(String, QueryDef)>,
}

/// What `QueryOrchestrator.queryStage` reports back to a continue-wait response
/// (`QO/QueryQueue.ts:688-712`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryStage {
    pub stage: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_elapsed: Option<i64>,
}

impl QueryStage {
    /// The literal reported while the handler runs.
    pub fn executing(time_elapsed: Option<i64>) -> Self {
        Self {
            stage: "Executing query".to_string(),
            time_elapsed,
        }
    }

    /// `#<n> in queue`, one based.
    pub fn in_queue(index: usize) -> Self {
        Self {
            stage: format!("#{} in queue", index + 1),
            time_elapsed: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn priority_rungs() {
        assert_eq!(QueuePriority::Interactive.value(), 10);
        assert_eq!(QueuePriority::Warmup.value(), 1);
        assert_eq!(QueuePriority::Background.value(), 0);
        assert_eq!(QueuePriority::Scheduled.value(), -1);
        assert_eq!(i32::from(QueuePriority::Interactive), 10);
        assert!(QueuePriority::Interactive > QueuePriority::Background);
    }

    #[test]
    fn query_stage_strings() {
        assert_eq!(QueryStage::executing(Some(12)).stage, "Executing query");
        assert_eq!(QueryStage::executing(Some(12)).time_elapsed, Some(12));
        assert_eq!(QueryStage::in_queue(0).stage, "#1 in queue");
        assert_eq!(QueryStage::in_queue(4).stage, "#5 in queue");
        assert_eq!(QueryStage::in_queue(0).time_elapsed, None);
        assert_eq!(
            serde_json::to_string(&QueryStage::in_queue(0)).unwrap(),
            r##"{"stage":"#1 in queue"}"##
        );
        assert_eq!(
            serde_json::to_string(&QueryStage::executing(Some(5))).unwrap(),
            r#"{"stage":"Executing query","timeElapsed":5}"#
        );
    }

    #[test]
    fn execution_result_round_trips() {
        assert_eq!(
            serde_json::to_string(&ExecutionResult::success(json!([1]))).unwrap(),
            r#"{"result":[1]}"#
        );
        assert_eq!(
            serde_json::to_string(&ExecutionResult::error("boom")).unwrap(),
            r#"{"error":"boom"}"#
        );
        assert_eq!(
            serde_json::from_str::<ExecutionResult>(r#"{"error":"boom"}"#).unwrap(),
            ExecutionResult::error("boom")
        );
        assert_eq!(
            serde_json::from_str::<ExecutionResult>(r#"{"result":null}"#).unwrap(),
            ExecutionResult::success(Value::Null)
        );
        assert_eq!(
            serde_json::to_string(&ExecutionResult::stream()).unwrap(),
            r#"{"streamResult":true}"#
        );
        assert_eq!(
            serde_json::from_str::<ExecutionResult>(r#"{"streamResult":true}"#).unwrap(),
            ExecutionResult::stream()
        );
        assert!(ExecutionResult::stream().is_stream());
        assert!(!ExecutionResult::success(Value::Null).is_stream());
    }

    #[test]
    fn query_flags_follow_javascript_truthiness() {
        let query = json!({ "isJob": true, "forceBuild": 0, "orphanedTimeout": 60, "empty": "" });

        assert!(query_flag(&query, "isJob"));
        assert!(!query_flag(&query, "forceBuild"));
        assert!(!query_flag(&query, "empty"));
        assert!(!query_flag(&query, "missing"));
        assert_eq!(query_u64(&query, "orphanedTimeout"), Some(60));
        assert_eq!(query_u64(&query, "missing"), None);
    }

    #[test]
    fn query_def_serializes_like_the_node_payload() {
        let def = QueryDef {
            queue_id: 7,
            query_handler: "query".to_string(),
            query: json!({ "isJob": false }),
            query_key: CacheKey::sql("SELECT 1", &[]),
            stage_query_key: Some(CacheKey::string("stage")),
            priority: 10,
            request_id: Some("req-1".to_string()),
            added_to_queue_time: 1700000000000,
            start_query_time: None,
            cancel_handler: None,
        };

        let json = serde_json::to_string(&def).unwrap();
        assert_eq!(
            json,
            r#"{"queueId":7,"queryHandler":"query","query":{"isJob":false},"queryKey":["SELECT 1",[]],"stageQueryKey":"stage","priority":10,"requestId":"req-1","addedToQueueTime":1700000000000}"#
        );
        assert_eq!(serde_json::from_str::<QueryDef>(&json).unwrap(), def);
    }

    #[test]
    fn query_def_update_merges_fields() {
        let mut def = QueryDef {
            queue_id: 1,
            query_handler: "query".to_string(),
            query: Value::Null,
            query_key: CacheKey::string("k"),
            stage_query_key: None,
            priority: 0,
            request_id: None,
            added_to_queue_time: 0,
            start_query_time: None,
            cancel_handler: None,
        };

        QueryDefUpdate::start_query_time(42).apply(&mut def);
        QueryDefUpdate::cancel_handler(json!("handle")).apply(&mut def);

        assert_eq!(def.start_query_time, Some(42));
        assert_eq!(def.cancel_handler, Some(json!("handle")));
    }
}
