//! Acceptance tests of the queue engine, ported from
//! `packages/cubejs-query-orchestrator/test/unit/QueryQueue.abstract.ts`.

use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use cubecache::CacheKey;
use cubequeue::{
    ExecuteInQueueOptions, QueryQueue, QueryQueueConfig, QueryStreamWriter, QueueError,
    QueuePriority, SendProcessMessageFn, StreamClosed, StreamRow,
};
use serde_json::{json, Value};
use tokio::task::JoinHandle;

/// Mirrors the `delay` handler of the Node suite: it publishes a cancellation handle,
/// waits, and answers with the configured result plus the number of earlier calls.
struct Harness {
    queue: Arc<QueryQueue>,
    handles: Arc<Mutex<Vec<JoinHandle<()>>>>,
    logs: Arc<Mutex<Vec<String>>>,
    calls: Arc<AtomicUsize>,
    in_flight: Arc<AtomicUsize>,
    max_in_flight: Arc<AtomicUsize>,
    cancelled: Arc<Mutex<Vec<Value>>>,
    stream: Arc<StreamState>,
}

/// What the stream handler of the harness recorded.
#[derive(Default)]
struct StreamState {
    /// Batches the handler managed to write.
    writes: AtomicUsize,
    /// The handler noticed that its consumer went away.
    cancelled: AtomicBool,
    /// The handler wrote everything it was asked for.
    completed: AtomicBool,
    /// Why a write did not go through.
    closed: Mutex<Vec<StreamClosed>>,
}

impl Harness {
    fn new(config: QueryQueueConfig) -> Self {
        let handles: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));
        let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let in_flight = Arc::new(AtomicUsize::new(0));
        let max_in_flight = Arc::new(AtomicUsize::new(0));
        let cancelled: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let stream = Arc::new(StreamState::default());

        let send_process_message: SendProcessMessageFn = {
            let handles = handles.clone();

            Arc::new(move |queue, hash, queue_id, retrieved| {
                let handles = handles.clone();

                Box::pin(async move {
                    let handle = tokio::spawn(queue.execute_query(hash, queue_id, retrieved));
                    handles.lock().unwrap().push(handle);

                    Ok(())
                })
            })
        };

        let logger = {
            let logs = logs.clone();

            Arc::new(move |message: &str, _event: Value| {
                logs.lock().unwrap().push(message.to_string());
            })
        };

        let queue = QueryQueue::builder("test_query_queue")
            .config(QueryQueueConfig {
                logger: Some(logger),
                send_process_message_fn: Some(send_process_message),
                process_uid: Some("00000000-0000-4000-8000-000000000000".to_string()),
                ..config
            })
            .query_handler("foo", |query: Value, _cancel| async move {
                Ok(json!(format!(
                    "{} bar",
                    query[0].as_str().unwrap_or_default()
                )))
            })
            .query_handler("fails", |_query: Value, _cancel| async move {
                Err("handler exploded".to_string())
            })
            .query_handler("delay", {
                let calls = calls.clone();
                let in_flight = in_flight.clone();
                let max_in_flight = max_in_flight.clone();

                move |query: Value, cancel: cubequeue::CancelHandlerSetter| {
                    let calls = calls.clone();
                    let in_flight = in_flight.clone();
                    let max_in_flight = max_in_flight.clone();

                    async move {
                        let running = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                        max_in_flight.fetch_max(running, Ordering::SeqCst);

                        let result = format!(
                            "{}{}",
                            query["result"].as_str().unwrap_or_default(),
                            calls.fetch_add(1, Ordering::SeqCst)
                        );

                        cancel.set(json!(result)).await;

                        tokio::time::sleep(Duration::from_millis(
                            query["delay"].as_u64().unwrap_or(0),
                        ))
                        .await;

                        in_flight.fetch_sub(1, Ordering::SeqCst);

                        Ok(json!(result))
                    }
                }
            })
            .stream_handler({
                let state = stream.clone();

                move |query: Value, writer: QueryStreamWriter| {
                    let state = state.clone();

                    async move {
                        if let Some(columns) = query["columns"].as_array() {
                            writer.set_columns(
                                columns
                                    .iter()
                                    .map(|column| column.as_str().unwrap_or_default().to_string())
                                    .collect(),
                            );
                        }

                        // Keeps the stream open without data, so that a test can attach to
                        // it before it starts flowing.
                        if let Some(hold) = query["holdMs"].as_u64() {
                            tokio::time::sleep(Duration::from_millis(hold)).await;
                        }

                        // A driver that fails before a single row reached the stream.
                        if let Some(error) = query["failBefore"].as_str() {
                            writer.fail(error);

                            return Err(error.to_string());
                        }

                        for batch in query["batches"].as_array().cloned().unwrap_or_default() {
                            let rows: Vec<StreamRow> = batch
                                .as_array()
                                .cloned()
                                .unwrap_or_default()
                                .into_iter()
                                .map(|row| row.as_array().cloned().unwrap_or_default())
                                .collect();

                            tokio::select! {
                                biased;

                                // The consumer walked away: stop reading the data source.
                                _ = writer.cancelled() => {
                                    state.cancelled.store(true, Ordering::SeqCst);

                                    return Ok(());
                                }
                                written = writer.write(rows) => match written {
                                    Ok(()) => {
                                        state.writes.fetch_add(1, Ordering::SeqCst);
                                    }
                                    Err(closed) => {
                                        state.closed.lock().unwrap().push(closed);

                                        return Ok(());
                                    }
                                },
                            }
                        }

                        // A driver that failed halfway through the result set.
                        if let Some(error) = query["failAfter"].as_str() {
                            writer.fail(error);

                            return Err(error.to_string());
                        }

                        state.completed.store(true, Ordering::SeqCst);

                        Ok(())
                    }
                }
            })
            .cancel_handler("delay", {
                let cancelled = cancelled.clone();

                move |def: cubequeue::QueryDef| {
                    let cancelled = cancelled.clone();

                    async move {
                        cancelled
                            .lock()
                            .unwrap()
                            .push(serde_json::to_value(&def.query_key).unwrap());

                        Ok(())
                    }
                }
            })
            .build();

        Self {
            queue,
            handles,
            logs,
            calls,
            in_flight,
            max_in_flight,
            cancelled,
            stream,
        }
    }

    /// Awaits every dispatched execution, including the ones a reconciliation started
    /// while the previous batch was draining.
    async fn await_processing(&self) {
        loop {
            let batch: Vec<JoinHandle<()>> = self.handles.lock().unwrap().drain(..).collect();

            if batch.is_empty() {
                break;
            }

            for handle in batch {
                let _ = handle.await;
            }
        }

        self.queue.shutdown().await;
    }

    fn logs(&self) -> Vec<String> {
        self.logs.lock().unwrap().clone()
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

fn delay_query(delay_ms: u64, result: &str) -> Value {
    json!({ "delay": delay_ms, "result": result })
}

fn job_query(delay_ms: u64, result: &str) -> Value {
    json!({ "delay": delay_ms, "result": result, "isJob": true })
}

fn config(concurrency: usize, continue_wait_timeout: u64) -> QueryQueueConfig {
    QueryQueueConfig {
        concurrency,
        continue_wait_timeout,
        execution_timeout: 30,
        orphaned_timeout: 120,
        heart_beat_interval: 30,
        // small enough that a test can fill the buffer
        query_stream_high_water_mark: 4,
        query_stream_idle_timeout: 5,
        ..Default::default()
    }
}

fn stream_query(batches: Value) -> Value {
    json!({ "columns": ["a", "b"], "batches": batches })
}

fn rows(batch: &[i64]) -> Vec<StreamRow> {
    batch.iter().map(|value| vec![json!(value)]).collect()
}

/// `gutter`: a query which is not cached anywhere is queued, executed and returned.
#[tokio::test]
async fn a_queued_query_is_executed_and_returned() {
    let harness = Harness::new(config(2, 5));
    let query_key = CacheKey::sql("select * from", &[]);

    let result = harness
        .queue
        .execute_in_queue(
            "foo",
            query_key.clone(),
            json!(["select * from", []]),
            QueuePriority::Interactive.into(),
            ExecuteInQueueOptions::default(),
        )
        .await
        .unwrap();

    assert_eq!(result, json!("select * from bar"));

    harness.await_processing().await;

    let logs = harness.logs();
    assert!(logs.contains(&"Added to queue".to_string()));
    assert!(logs.contains(&"Performing query".to_string()));
    assert!(logs.contains(&"Performing query completed".to_string()));
}

/// An already computed result is handed back without queueing anything again — the same
/// convergence the continue-wait polling loop relies on.
#[tokio::test]
async fn an_existing_result_is_served_without_queueing() {
    let harness = Harness::new(config(2, 5));
    let query_key = CacheKey::sql("select * from cached", &[]);

    // a job does not consume the result, it stays in the queue driver
    let job = harness
        .queue
        .execute_in_queue(
            "delay",
            query_key.clone(),
            job_query(0, "1"),
            QueuePriority::Background.into(),
            ExecuteInQueueOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(job, Value::Null);

    harness.await_processing().await;
    assert_eq!(harness.calls(), 1);

    let result = harness
        .queue
        .execute_in_queue(
            "delay",
            query_key,
            delay_query(0, "1"),
            QueuePriority::Background.into(),
            ExecuteInQueueOptions::default(),
        )
        .await
        .unwrap();

    assert_eq!(result, json!("10"));
    // the handler did not run a second time and nothing was added to the queue
    assert_eq!(harness.calls(), 1);
    assert_eq!(
        harness
            .logs()
            .iter()
            .filter(|message| *message == "Added to queue")
            .count(),
        1
    );
}

/// `timeout - continue wait`: a result which is not ready within `continueWaitTimeout`
/// surfaces as the literal `Continue wait` error.
#[tokio::test]
async fn a_slow_query_raises_continue_wait() {
    let harness = Harness::new(config(2, 1));
    let query_key = CacheKey::sql("select * from 2", &[]);

    let error = harness
        .queue
        .execute_in_queue(
            "delay",
            query_key.clone(),
            delay_query(2500, "1"),
            QueuePriority::Interactive.into(),
            ExecuteInQueueOptions::default(),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, QueueError::ContinueWait));
    assert_eq!(error.to_string(), "Continue wait");
    assert!(error.is_continue_wait());

    // the query keeps running, and the retry after it finished gets the result
    harness.await_processing().await;

    let result = harness
        .queue
        .execute_in_queue(
            "delay",
            query_key,
            delay_query(2500, "1"),
            QueuePriority::Interactive.into(),
            ExecuteInQueueOptions::default(),
        )
        .await
        .unwrap();

    assert_eq!(result, json!("10"));
    assert_eq!(harness.calls(), 1);
}

/// A job never raises continue wait, it returns as soon as it is queued.
#[tokio::test]
async fn a_job_never_raises_continue_wait() {
    let harness = Harness::new(config(2, 1));

    let result = harness
        .queue
        .execute_in_queue(
            "delay",
            CacheKey::sql("select * from job", &[]),
            job_query(1500, "1"),
            QueuePriority::Background.into(),
            ExecuteInQueueOptions::default(),
        )
        .await
        .unwrap();

    assert_eq!(result, Value::Null);

    harness.await_processing().await;
}

/// `negative priority`/`priority`: higher priority runs first, whatever the arrival order.
#[tokio::test]
async fn queued_queries_run_in_priority_order() {
    let harness = Harness::new(config(1, 10));
    let order: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));

    // occupies the single worker slot while the others queue up
    harness
        .queue
        .execute_in_queue(
            "delay",
            CacheKey::string("blocker"),
            job_query(500, "b"),
            QueuePriority::Interactive.into(),
            ExecuteInQueueOptions::default(),
        )
        .await
        .unwrap();

    let mut waiters = Vec::new();

    for (name, priority) in [
        ("scheduled", QueuePriority::Scheduled),
        ("background", QueuePriority::Background),
        ("warmup", QueuePriority::Warmup),
        ("interactive", QueuePriority::Interactive),
    ] {
        let queue = harness.queue.clone();
        let order = order.clone();

        waiters.push(tokio::spawn(async move {
            queue
                .execute_in_queue(
                    "delay",
                    CacheKey::string(name),
                    delay_query(10, name),
                    priority.into(),
                    ExecuteInQueueOptions::default(),
                )
                .await
                .unwrap();

            order.lock().unwrap().push(name);
        }));

        // keeps the arrival order deterministic: priority, not arrival, has to decide
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    for waiter in waiters {
        waiter.await.unwrap();
    }

    harness.await_processing().await;

    assert_eq!(
        *order.lock().unwrap(),
        vec!["interactive", "warmup", "background", "scheduled"]
    );
}

/// The concurrency budget is never exceeded, and the queue still drains.
#[tokio::test]
async fn concurrency_is_limited() {
    let harness = Harness::new(config(2, 10));
    let mut waiters = Vec::new();

    for index in 0..6 {
        let queue = harness.queue.clone();

        waiters.push(tokio::spawn(async move {
            queue
                .execute_in_queue(
                    "delay",
                    CacheKey::string(format!("concurrent-{index}")),
                    delay_query(100, "c"),
                    QueuePriority::Background.into(),
                    ExecuteInQueueOptions::default(),
                )
                .await
                .unwrap()
        }));
    }

    for waiter in waiters {
        waiter.await.unwrap();
    }

    harness.await_processing().await;

    assert_eq!(harness.calls(), 6);
    assert_eq!(harness.max_in_flight.load(Ordering::SeqCst), 2);
    assert_eq!(harness.in_flight.load(Ordering::SeqCst), 0);
}

/// `cancelQuery` removes the query and runs its cancel handler with the handle the query
/// handler published.
#[tokio::test]
async fn a_running_query_can_be_cancelled() {
    let harness = Harness::new(config(1, 10));
    let query_key = CacheKey::string("cancel-me");

    harness
        .queue
        .execute_in_queue(
            "delay",
            query_key.clone(),
            job_query(2000, "c"),
            QueuePriority::Interactive.into(),
            ExecuteInQueueOptions::default(),
        )
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(harness.queue.cancel_query(&query_key, None).await.unwrap());
    assert_eq!(*harness.cancelled.lock().unwrap(), vec![json!("cancel-me")]);

    harness.await_processing().await;

    let logs = harness.logs();
    assert!(logs.contains(&"Cancelling query manual".to_string()));
    // the query was removed while it ran, so its result has nowhere to go
    assert!(logs.contains(&"Orphaned execution result".to_string()));
}

/// Cancelling an unknown query is a no-op rather than an error.
#[tokio::test]
async fn cancelling_an_unknown_query_does_nothing() {
    let harness = Harness::new(config(1, 1));

    assert!(harness
        .queue
        .cancel_query(&CacheKey::string("nothing-here"), None)
        .await
        .unwrap());
    assert!(harness.cancelled.lock().unwrap().is_empty());
}

/// A query which outruns `executionTimeout` is cancelled and its error is what the client
/// gets, rather than a continue wait.
#[tokio::test]
async fn an_execution_timeout_cancels_the_query() {
    let harness = Harness::new(QueryQueueConfig {
        execution_timeout: 1,
        ..config(1, 10)
    });

    let error = harness
        .queue
        .execute_in_queue(
            "delay",
            CacheKey::string("too-slow"),
            delay_query(5000, "t"),
            QueuePriority::Interactive.into(),
            ExecuteInQueueOptions::default(),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, QueueError::Execution(_)));
    assert!(
        error.to_string().contains("timeout"),
        "unexpected error: {error}"
    );

    harness.await_processing().await;

    let logs = harness.logs();
    assert!(logs.contains(&"Error while querying".to_string()));
    assert!(logs.contains(&"Cancelling query due to timeout".to_string()));
    assert_eq!(*harness.cancelled.lock().unwrap(), vec![json!("too-slow")]);
}

/// A failing handler surfaces its message, it is not a continue wait.
#[tokio::test]
async fn a_failing_handler_surfaces_its_error() {
    let harness = Harness::new(config(1, 10));

    let error = harness
        .queue
        .execute_in_queue(
            "fails",
            CacheKey::string("boom"),
            Value::Null,
            QueuePriority::Interactive.into(),
            ExecuteInQueueOptions::default(),
        )
        .await
        .unwrap_err();

    assert_eq!(error.to_string(), "handler exploded");

    harness.await_processing().await;
}

/// The heartbeat of a running query keeps it out of the stalled set, so reconciliation
/// never cancels a long query which is making progress.
#[tokio::test]
async fn the_heartbeat_keeps_a_long_query_alive() {
    // heart_beat_timeout is heart_beat_interval * 4, so 4 seconds here
    let harness = Harness::new(QueryQueueConfig {
        heart_beat_interval: 1,
        ..config(1, 10)
    });
    let query_key = CacheKey::string("long-running");

    let queue = harness.queue.clone();
    let waiter = tokio::spawn(async move {
        queue
            .execute_in_queue(
                "delay",
                CacheKey::string("long-running"),
                delay_query(4500, "h"),
                QueuePriority::Interactive.into(),
                ExecuteInQueueOptions::default(),
            )
            .await
    });

    let connection = harness
        .queue
        .queue_driver()
        .create_connection()
        .await
        .unwrap();
    assert_eq!(connection.redis_hash(&query_key), "long-running");

    // well past the stall timeout, the heartbeat has been keeping the item alive
    tokio::time::sleep(Duration::from_millis(4200)).await;

    assert_eq!(connection.get_stalled_queries().await.unwrap(), vec![]);
    assert_eq!(
        connection.get_active_queries().await.unwrap().len(),
        1,
        "the query should still be running"
    );

    assert_eq!(waiter.await.unwrap().unwrap(), json!("h0"));

    harness.await_processing().await;
    assert!(harness.cancelled.lock().unwrap().is_empty());
    assert!(!harness
        .logs()
        .contains(&"Removing orphaned query".to_string()));
}

/// `stage reporting` and `priority stage reporting`: the strings a continue-wait response
/// carries.
#[tokio::test]
async fn query_stages_report_execution_and_queue_position() {
    let harness = Harness::new(config(1, 10));

    let running = {
        let queue = harness.queue.clone();

        tokio::spawn(async move {
            queue
                .execute_in_queue(
                    "delay",
                    CacheKey::string("stage-running"),
                    delay_query(600, "1"),
                    QueuePriority::Interactive.into(),
                    ExecuteInQueueOptions {
                        stage_query_key: Some(CacheKey::string("stage-1")),
                        request_id: Some("9f056234-aa57-4702-ab30-145221da6a46-span-1".to_string()),
                        span_id: Some("span-id".to_string()),
                    },
                )
                .await
        })
    };

    tokio::time::sleep(Duration::from_millis(100)).await;

    let stage = harness
        .queue
        .query_stage(&CacheKey::string("stage-1"), None, None)
        .await
        .unwrap()
        .expect("the running query has a stage");
    assert_eq!(stage.stage, "Executing query");
    assert!(stage.time_elapsed.unwrap_or(-1) >= 0);

    let queued = {
        let queue = harness.queue.clone();

        tokio::spawn(async move {
            queue
                .execute_in_queue(
                    "delay",
                    CacheKey::string("stage-queued"),
                    delay_query(10, "2"),
                    QueuePriority::Background.into(),
                    ExecuteInQueueOptions {
                        stage_query_key: Some(CacheKey::string("stage-2")),
                        request_id: Some("4274691a-5f4c-480e-89c4-d2b9d989891c-span-1".to_string()),
                        ..Default::default()
                    },
                )
                .await
        })
    };

    tokio::time::sleep(Duration::from_millis(100)).await;

    let stage = harness
        .queue
        .query_stage(&CacheKey::string("stage-2"), None, None)
        .await
        .unwrap()
        .expect("the waiting query has a stage");
    assert_eq!(stage.stage, "#1 in queue");
    assert_eq!(stage.time_elapsed, None);

    // a priority filter which matches nothing hides the query
    assert_eq!(
        harness
            .queue
            .query_stage(
                &CacheKey::string("stage-2"),
                Some(QueuePriority::Interactive.into()),
                None
            )
            .await
            .unwrap(),
        None
    );

    running.await.unwrap().unwrap();
    queued.await.unwrap().unwrap();
    harness.await_processing().await;

    // nothing is queued any more
    assert_eq!(
        harness
            .queue
            .query_stage(&CacheKey::string("stage-1"), None, None)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        harness
            .queue
            .query_stage(&CacheKey::string("unknown-stage"), None, None)
            .await
            .unwrap(),
        None
    );
}

/// A priority outside the accepted range is rejected before anything is queued.
#[tokio::test]
async fn an_out_of_range_priority_is_rejected() {
    let harness = Harness::new(config(1, 1));

    for priority in [-10_001, 10_001] {
        let error = harness
            .queue
            .execute_in_queue(
                "foo",
                CacheKey::string("priority"),
                json!(["select", []]),
                priority,
                ExecuteInQueueOptions::default(),
            )
            .await
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "Priority should be between -10000 and 10000"
        );
    }

    assert!(!harness.logs().contains(&"Added to queue".to_string()));
}

/// `skipQueue` runs the handler inline, without touching the queue driver.
#[tokio::test]
async fn skip_queue_runs_the_handler_inline() {
    let harness = Harness::new(QueryQueueConfig {
        skip_queue: true,
        ..config(1, 1)
    });

    let result = harness
        .queue
        .execute_in_queue(
            "foo",
            CacheKey::sql("select * from inline", &[]),
            json!(["select * from inline", []]),
            QueuePriority::Interactive.into(),
            ExecuteInQueueOptions::default(),
        )
        .await
        .unwrap();

    assert_eq!(result, json!("select * from inline bar"));
    assert!(!harness.logs().contains(&"Added to queue".to_string()));
}

/// A persistent query answers with a stream, so it does not come back through
/// `execute_in_queue`, which answers with a value.
#[tokio::test]
async fn streaming_queries_are_rejected_by_the_value_path() {
    let harness = Harness::new(config(1, 1));

    let error = harness
        .queue
        .execute_in_queue(
            "stream",
            CacheKey::sql("select * from stream", &[]).with_persistent(true),
            Value::Null,
            QueuePriority::Interactive.into(),
            ExecuteInQueueOptions::default(),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, QueueError::StreamingUnsupported));
}

/// `Streaming queries to Cube Store aren't supported`: the inline path has no queue item
/// to hang a stream on.
#[tokio::test]
async fn streaming_needs_a_queue() {
    let harness = Harness::new(QueryQueueConfig {
        skip_queue: true,
        ..config(1, 1)
    });

    let error = harness
        .queue
        .execute_stream_in_queue(
            CacheKey::sql("select * from stream", &[]),
            stream_query(json!([[[1]]])),
            QueuePriority::Interactive.into(),
            ExecuteInQueueOptions::default(),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, QueueError::StreamingUnsupported));
    assert_eq!(
        error.to_string(),
        "Streaming queries to Cube Store aren't supported"
    );
}

/// A persistent query delivers its batches in order and ends when the handler is done.
#[tokio::test]
async fn a_persistent_query_streams_its_batches_in_order() {
    let harness = Harness::new(config(2, 5));
    let query_key = CacheKey::sql("select * from streamed", &[]);

    let stream = harness
        .queue
        .execute_stream_in_queue(
            query_key.clone(),
            stream_query(json!([[[1], [2]], [[3]], [[4]]])),
            QueuePriority::Interactive.into(),
            ExecuteInQueueOptions::default(),
        )
        .await
        .unwrap();

    // the queue key of a persistent query belongs to this process
    assert_eq!(
        stream.query_key_hash(),
        harness.queue.persistent_hash(&query_key)
    );
    assert!(stream
        .query_key_hash()
        .ends_with("@00000000-0000-4000-8000-000000000000"));

    let mut batches = Vec::new();

    while let Some(batch) = stream.next_batch().await {
        batches.push(batch.unwrap());
    }

    assert_eq!(
        batches
            .iter()
            .map(|batch| batch.rows.clone())
            .collect::<Vec<_>>(),
        vec![rows(&[1, 2]), rows(&[3]), rows(&[4])]
    );
    assert_eq!(*batches[0].columns, vec!["a".to_string(), "b".to_string()]);

    harness.await_processing().await;

    assert!(harness.stream.completed.load(Ordering::SeqCst));
    assert_eq!(harness.stream.writes.load(Ordering::SeqCst), 3);
    // the handler is done, so the stream left the map
    assert!(harness.queue.streams().is_empty());
}

/// `waitForQueryStream` hands a second waiter the very same stream, which is how a retry
/// of the same persistent request joins the query that is already running.
#[tokio::test]
async fn a_second_waiter_gets_the_same_stream() {
    let harness = Harness::new(config(2, 5));
    let query_key = CacheKey::sql("select * from shared-stream", &[]);
    let mut query = stream_query(json!([[[1]]]));
    query["holdMs"] = json!(300);

    let first = harness
        .queue
        .execute_stream_in_queue(
            query_key.clone(),
            query,
            QueuePriority::Interactive.into(),
            ExecuteInQueueOptions::default(),
        )
        .await
        .unwrap();

    let second = harness
        .queue
        .wait_for_query_stream(&harness.queue.persistent_hash(&query_key))
        .await
        .expect("the stream is still in the map, it carries no data yet");

    assert_eq!(first, second, "both waiters hold the same stream");

    // one batch, delivered to one of the handles
    let batch = second.next_batch().await.unwrap().unwrap();
    assert_eq!(batch.rows, rows(&[1]));
    assert!(second.next_batch().await.is_none());

    harness.await_processing().await;
    assert!(harness.stream.completed.load(Ordering::SeqCst));
}

/// A consumer that walks away mid-flight cancels the query behind the stream instead of
/// leaving it to fill a buffer nobody reads.
#[tokio::test]
async fn dropping_the_stream_cancels_the_query() {
    let harness = Harness::new(config(2, 5));
    let query_key = CacheKey::sql("select * from abandoned", &[]);

    let stream = harness
        .queue
        .execute_stream_in_queue(
            query_key.clone(),
            // more rows than the high-water mark, so the handler cannot run away
            stream_query(json!([[[1], [2]], [[3], [4]], [[5], [6]], [[7], [8]]])),
            QueuePriority::Interactive.into(),
            ExecuteInQueueOptions::default(),
        )
        .await
        .unwrap();

    drop(stream);

    harness.await_processing().await;

    let closed = harness.stream.closed.lock().unwrap().clone();

    assert!(
        harness.stream.cancelled.load(Ordering::SeqCst)
            || closed.contains(&StreamClosed::Cancelled),
        "the handler was told to stop, instead it saw {closed:?}"
    );
    assert!(
        !harness.stream.completed.load(Ordering::SeqCst),
        "the query did not run to completion"
    );
    assert!(harness.stream.writes.load(Ordering::SeqCst) < 4);
    // nothing is left behind: neither the stream nor the queue item
    assert!(harness.queue.streams().is_empty());
    assert_eq!(
        harness
            .queue
            .fetch_query_stage_state()
            .await
            .unwrap()
            .active,
        Vec::<String>::new()
    );
}

/// A failure in the middle of a result set reaches the consumer as the stream's last item
/// rather than being swallowed into a clean end of stream.
#[tokio::test]
async fn a_stream_error_reaches_the_consumer() {
    let harness = Harness::new(config(2, 5));
    let mut query = stream_query(json!([[[1]]]));
    query["failAfter"] = json!("connection reset by peer");

    let stream = harness
        .queue
        .execute_stream_in_queue(
            CacheKey::sql("select * from failing", &[]),
            query,
            QueuePriority::Interactive.into(),
            ExecuteInQueueOptions::default(),
        )
        .await
        .unwrap();

    assert_eq!(stream.next_batch().await.unwrap().unwrap().rows, rows(&[1]));
    assert_eq!(
        stream.next_batch().await.unwrap().unwrap_err(),
        "connection reset by peer"
    );
    assert!(stream.next_batch().await.is_none());

    harness.await_processing().await;
    assert!(!harness.stream.completed.load(Ordering::SeqCst));
    assert!(harness.logs().contains(&"Error while querying".to_string()));
}

/// A data source that fails before a single row was read surfaces the same way.
#[tokio::test]
async fn a_stream_that_never_started_surfaces_its_error() {
    let harness = Harness::new(config(2, 5));
    let mut query = stream_query(json!([[[1]]]));
    query["failBefore"] = json!("no such table");

    let stream = harness
        .queue
        .execute_stream_in_queue(
            CacheKey::sql("select * from missing", &[]),
            query,
            QueuePriority::Interactive.into(),
            ExecuteInQueueOptions::default(),
        )
        .await
        .unwrap();

    assert_eq!(
        stream.next_batch().await.unwrap().unwrap_err(),
        "no such table"
    );
    assert!(stream.next_batch().await.is_none());

    harness.await_processing().await;
}

/// A persistent key of another process is left alone by reconciliation: only the process
/// whose uid the key carries may run it.
#[tokio::test]
async fn persistent_keys_of_other_processes_are_not_picked_up() {
    let harness = Harness::new(config(1, 1));

    let own = CacheKey::sql("select * from persistent", &[]).with_persistent(true);
    assert!(harness
        .queue
        .redis_hash(&own)
        .ends_with("@00000000-0000-4000-8000-000000000000"));

    let connection = harness
        .queue
        .queue_driver()
        .create_connection()
        .await
        .unwrap();

    // a key stamped with a different process uid
    let foreign = CacheKey::string("deadbeef@11111111-1111-4111-8111-111111111111");

    connection
        .add_to_queue(
            &foreign,
            "delay",
            job_query(10, "p"),
            QueuePriority::Interactive.into(),
            &cubequeue::AddToQueueOptions {
                queue_id: 99,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    harness.queue.reconcile_queue().await.unwrap();
    harness.await_processing().await;

    // nothing ran: the item still sits in the queue
    assert_eq!(harness.calls(), 0);
    assert_eq!(connection.get_to_process_queries().await.unwrap().len(), 1);
}
