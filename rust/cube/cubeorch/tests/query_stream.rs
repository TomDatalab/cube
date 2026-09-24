//! Persistent (streaming) queries end to end: `QueryCache.cachedQueryResult`'s branch A,
//! the queue's `stream` handler and `QueryStream` itself.
//!
//! Ported from the persistent path of
//! `packages/cubejs-query-orchestrator/src/orchestrator/QueryCache.ts:316-355, :812-882`.

use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use async_trait::async_trait;
use cubedriver::{
    Column, Driver, DriverConfig, DriverError, GenericType, QueryOptions, QueryResult, Row,
    StreamOptions, StreamTableData,
};
use cubeorch::{
    DriverFactory, FetchQueryOutcome, QueryBody, QueryCache, QueryCacheOptions, QueryOrchestrator,
    QueryOrchestratorOptions,
};
use cubequeue::{QueryQueueConfig, QueryStream};
use futures::StreamExt;
use serde_json::{json, Value};

/// A driver whose `stream` hands out `rows` numbered rows, and which records how far the
/// consumer actually pulled it.
struct StreamingDriver {
    config: DriverConfig,
    rows: usize,
    /// Yields an error instead of the row at this offset.
    fail_at: Option<usize>,
    /// `stream()` itself fails, before a single row exists.
    fail_on_open: bool,
    /// Rows the driver produced, which is what back-pressure limits.
    produced: Arc<AtomicUsize>,
    /// The row stream was dropped, releasing the connection behind it.
    released: Arc<AtomicBool>,
    streamed: Mutex<Vec<String>>,
}

impl StreamingDriver {
    fn with_rows(rows: usize) -> Arc<Self> {
        Arc::new(Self {
            config: DriverConfig::default(),
            rows,
            fail_at: None,
            fail_on_open: false,
            produced: Arc::new(AtomicUsize::new(0)),
            released: Arc::new(AtomicBool::new(false)),
            streamed: Mutex::new(Vec::new()),
        })
    }

    fn failing_at(rows: usize, fail_at: usize) -> Arc<Self> {
        let mut driver = Self::with_rows(rows);
        Arc::get_mut(&mut driver).unwrap().fail_at = Some(fail_at);

        driver
    }

    fn failing_to_open() -> Arc<Self> {
        let mut driver = Self::with_rows(0);
        Arc::get_mut(&mut driver).unwrap().fail_on_open = true;

        driver
    }

    fn produced(&self) -> usize {
        self.produced.load(Ordering::SeqCst)
    }

    fn released(&self) -> bool {
        self.released.load(Ordering::SeqCst)
    }

    fn streamed(&self) -> Vec<String> {
        self.streamed.lock().unwrap().clone()
    }
}

/// Flips a flag when the row stream is dropped, which is where a real driver releases its
/// connection.
struct ReleaseOnDrop(Arc<AtomicBool>);

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[async_trait]
impl Driver for StreamingDriver {
    fn config(&self) -> &DriverConfig {
        &self.config
    }

    async fn test_connection(&self) -> Result<(), DriverError> {
        Ok(())
    }

    async fn query(
        &self,
        _sql: &str,
        _params: &[Value],
        _options: &QueryOptions,
    ) -> Result<QueryResult, DriverError> {
        Ok(QueryResult::default())
    }

    async fn stream(
        &self,
        sql: &str,
        _params: &[Value],
        _options: &StreamOptions,
    ) -> Result<StreamTableData, DriverError> {
        self.streamed.lock().unwrap().push(sql.to_string());

        if self.fail_on_open {
            return Err(DriverError::Database {
                message: "relation \"orders\" does not exist".to_string(),
                code: Some("42P01".to_string()),
            });
        }

        let produced = self.produced.clone();
        let guard = ReleaseOnDrop(self.released.clone());
        let total = self.rows;
        let fail_at = self.fail_at;

        let rows = futures::stream::unfold((0usize, guard), move |(index, guard)| {
            let produced = produced.clone();

            async move {
                if fail_at == Some(index) {
                    return Some((
                        Err(DriverError::Query("connection reset by peer".to_string())),
                        (index + 1, guard),
                    ));
                }

                if index >= total {
                    return None;
                }

                produced.fetch_add(1, Ordering::SeqCst);

                Some((
                    Ok(vec![json!(index as i64), json!(format!("row-{index}"))]),
                    (index + 1, guard),
                ))
            }
        });

        Ok(StreamTableData {
            columns: vec![
                Column::new("a_0", GenericType::Int),
                Column::new("a_1", GenericType::String),
            ],
            rows: rows.boxed(),
        })
    }
}

fn factory_of(driver: Arc<StreamingDriver>) -> DriverFactory {
    Arc::new(move |_| {
        let driver = driver.clone();

        Box::pin(async move { Ok(driver as Arc<dyn Driver>) })
    })
}

fn queue_config(high_water_mark: usize) -> QueryQueueConfig {
    QueryQueueConfig {
        concurrency: 2,
        continue_wait_timeout: 2,
        query_stream_high_water_mark: high_water_mark,
        query_stream_idle_timeout: 5,
        ..Default::default()
    }
}

fn cache(driver: Arc<StreamingDriver>, high_water_mark: usize) -> Arc<QueryCache> {
    QueryCache::builder("test", factory_of(driver))
        .logger(Arc::new(|_, _| {}))
        .options(QueryCacheOptions::default())
        .queue_config(queue_config(high_water_mark))
        .build()
}

/// A `/v1/load` body of a persistent query, the way the SQL API builds one.
fn persistent_body(query: &str) -> QueryBody {
    QueryBody {
        query: Some(query.to_string()),
        values: Some(vec![]),
        data_source: Some("default".to_string()),
        persistent: true,
        request_id: Some("req-stream-span-1".to_string()),
        alias_name_to_member: Some(json!({
            "a_0": "Orders.count",
            "a_1": "Orders.status",
        })),
        ..Default::default()
    }
}

async fn stream_of(cache: &Arc<QueryCache>, body: &QueryBody) -> QueryStream {
    cache
        .cached_query_result(body, &[])
        .await
        .unwrap()
        .into_stream()
        .expect("a persistent query answers with a stream")
}

/// A streamed query delivers every row, in order, and ends.
#[tokio::test]
async fn a_persistent_query_streams_its_rows_in_order() {
    let driver = StreamingDriver::with_rows(5);
    let cache = cache(driver.clone(), 8);
    let body = persistent_body("SELECT * FROM orders");

    let stream = stream_of(&cache, &body).await;

    let mut rows: Vec<Row> = Vec::new();
    let mut columns = Vec::new();

    while let Some(batch) = stream.next_batch().await {
        let batch = batch.unwrap();
        columns = (*batch.columns).clone();
        rows.extend(batch.rows);
    }

    assert_eq!(rows.len(), 5);
    assert_eq!(
        rows.iter().map(|row| row[0].clone()).collect::<Vec<_>>(),
        (0..5).map(|index| json!(index)).collect::<Vec<Value>>()
    );
    // `aliasNameToMember` is applied, once, to the column names
    assert_eq!(
        columns,
        vec!["Orders.count".to_string(), "Orders.status".to_string()]
    );
    assert_eq!(driver.streamed(), vec!["SELECT * FROM orders".to_string()]);
    assert_eq!(driver.produced(), 5);
}

/// The rows of a persistent query never reach the result cache: the next request runs the
/// query again rather than being served a stream that is already gone.
#[tokio::test]
async fn a_streamed_result_is_not_cached() {
    let driver = StreamingDriver::with_rows(2);
    let cache = cache(driver.clone(), 8);
    let body = persistent_body("SELECT * FROM orders");

    for _ in 0..2 {
        let stream = stream_of(&cache, &body).await;
        assert_eq!(stream.collect_rows().await.unwrap().len(), 2);
    }

    assert_eq!(driver.streamed().len(), 2);
}

/// The driver is held at the high-water mark until the consumer catches up.
#[tokio::test]
async fn the_driver_is_held_at_the_high_water_mark() {
    let driver = StreamingDriver::with_rows(1_000);
    let cache = cache(driver.clone(), 4);
    let body = persistent_body("SELECT * FROM big");

    let stream = stream_of(&cache, &body).await;

    // Give the handler every chance to run away with the result set.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let produced = driver.produced();
    assert!(
        produced <= 8,
        "the driver ran {produced} rows ahead of a consumer that read none"
    );

    // Reading releases the buffer, and the driver may run ahead again.
    let first = stream.next_batch().await.unwrap().unwrap();
    assert_eq!(first.rows.len(), 4);

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(driver.produced() > produced);
    assert!(driver.produced() <= produced + 8);

    drop(stream);
}

/// A consumer that walks away mid-flight cancels the query: the driver's row stream is
/// dropped, which releases the connection behind it, instead of being read to the end.
#[tokio::test]
async fn dropping_the_stream_cancels_the_query() {
    let driver = StreamingDriver::with_rows(10_000);
    let cache = cache(driver.clone(), 4);
    let body = persistent_body("SELECT * FROM endless");

    let stream = stream_of(&cache, &body).await;
    let queue = cache.get_queue("default");

    assert_eq!(stream.next_batch().await.unwrap().unwrap().rows.len(), 4);

    drop(stream);

    // the handler notices without waiting for another row
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(driver.released(), "the driver's row stream was dropped");
    assert!(
        driver.produced() < 10_000,
        "the query was cancelled rather than drained, it produced {}",
        driver.produced()
    );
    assert!(
        queue.streams().is_empty(),
        "the stream left the queue's map"
    );
    assert!(
        queue
            .fetch_query_stage_state()
            .await
            .unwrap()
            .active
            .is_empty(),
        "the queue item was acknowledged and removed"
    );
}

/// Cancelling the query from outside destroys the stream, and the consumer sees it end.
#[tokio::test]
async fn cancelling_the_query_destroys_the_stream() {
    let driver = StreamingDriver::with_rows(10_000);
    let cache = cache(driver.clone(), 4);
    let body = persistent_body("SELECT * FROM cancelled");

    let stream = stream_of(&cache, &body).await;
    let queue = cache.get_queue("default");

    assert!(queue.destroy_query_stream(&QueryCache::query_body_cache_key(&body)));

    // whatever was buffered is still delivered, then the stream ends
    while stream.next_batch().await.is_some() {}

    assert!(stream.is_cancelled());
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(driver.released());
}

/// A data source that fails before a single row was read surfaces the failure to the
/// consumer rather than ending the stream as if the result set were empty.
#[tokio::test]
async fn a_driver_that_cannot_start_surfaces_its_error() {
    let driver = StreamingDriver::failing_to_open();
    let cache = cache(driver.clone(), 8);

    let stream = stream_of(&cache, &persistent_body("SELECT * FROM missing")).await;

    assert_eq!(
        stream.next_batch().await.unwrap().unwrap_err(),
        "relation \"orders\" does not exist"
    );
    assert!(stream.next_batch().await.is_none());
}

/// So does a failure halfway through the result set.
#[tokio::test]
async fn an_error_mid_result_set_reaches_the_consumer() {
    let driver = StreamingDriver::failing_at(1_000, 6);
    let cache = cache(driver.clone(), 4);

    let stream = stream_of(&cache, &persistent_body("SELECT * FROM flaky")).await;

    let mut rows = 0;
    let error = loop {
        match stream.next_batch().await {
            Some(Ok(batch)) => rows += batch.rows.len(),
            Some(Err(error)) => break error,
            None => panic!("the failure was swallowed into a clean end of stream"),
        }
    };

    assert_eq!(error, "connection reset by peer");
    assert_eq!(
        rows, 4,
        "the rows read before the failure are still delivered"
    );
    assert!(stream.next_batch().await.is_none());
}

/// `fetchQuery` hands the stream back untouched (`QO/QueryOrchestrator.ts:279-282`).
#[tokio::test]
async fn fetch_query_returns_the_stream_of_a_persistent_query() {
    let driver = StreamingDriver::with_rows(3);

    let orchestrator = QueryOrchestrator::new(
        "test",
        factory_of(driver.clone()),
        None,
        Arc::new(|_, _| {}),
        QueryOrchestratorOptions::default(),
    );

    let outcome = orchestrator
        .fetch_query(&persistent_body("SELECT * FROM orders"))
        .await
        .unwrap();

    assert!(matches!(outcome, FetchQueryOutcome::Stream(_)));

    let stream = outcome.into_stream().unwrap();
    assert_eq!(stream.collect_rows().await.unwrap().len(), 3);
}
