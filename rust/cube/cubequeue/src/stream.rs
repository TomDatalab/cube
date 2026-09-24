//! `QueryStream` — the object a persistent (streaming) query hands back.
//!
//! Port of `QO/QueryStream.ts` and of the `streams` map `QueryQueue` keeps
//! (`QO/QueryQueue.ts:111-181`).
//!
//! The Node implementation is a `stream.Transform` in object mode: the stream handler
//! pipes the driver's row stream into it, `highWaterMark`
//! (`CUBEJS_DB_QUERY_STREAM_HIGH_WATER_MARK`, 8192 rows) bounds what is buffered, and the
//! `aliasNameToMember` map renames every row's keys on the way through.
//!
//! Here the same shape is a channel: [`QueryStreamWriter`] is the producing end handed to
//! the stream handler, [`QueryStream`] is the consuming end handed to the caller of
//! `execute_stream_in_queue`. Rows travel in batches rather than one by one, so the alias
//! renaming happens once per stream (on the column names) instead of once per row, and
//! back-pressure is counted in rows so that the high-water mark keeps its meaning.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex, Weak,
    },
    time::Duration,
};

use serde_json::Value;
use tokio::sync::{mpsc, oneshot, Notify, Semaphore};

/// `getEnv('dbQueryStreamHighWaterMark')` (`packages/cubejs-backend-shared/src/env.ts:778`).
pub const DEFAULT_QUERY_STREAM_HIGH_WATER_MARK: usize = 8192;

/// The 5 minutes of inactivity after which `QueryStream.debounce` destroys a stream nobody
/// reads (`QO/QueryStream.ts:6, :76-85`).
pub const DEFAULT_QUERY_STREAM_IDLE_TIMEOUT_SECS: u64 = 5 * 60;

/// `CUBEJS_DB_QUERY_STREAM_HIGH_WATER_MARK`, in rows.
pub fn query_stream_high_water_mark() -> usize {
    parse_high_water_mark(std::env::var("CUBEJS_DB_QUERY_STREAM_HIGH_WATER_MARK").ok())
}

fn parse_high_water_mark(raw: Option<String>) -> usize {
    raw.and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_QUERY_STREAM_HIGH_WATER_MARK)
}

/// A positional row, the same shape as `cubedriver::Row`.
pub type StreamRow = Vec<Value>;

/// One batch of rows of a [`QueryStream`].
///
/// `columns` are the member names the caller asked for: the driver's column names with
/// `aliasNameToMember` applied, which is the renaming `QueryStream._transform` does per row.
#[derive(Clone, Debug, PartialEq)]
pub struct QueryStreamBatch {
    pub columns: Arc<Vec<String>>,
    pub rows: Vec<StreamRow>,
}

impl QueryStreamBatch {
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The rows as the objects the Node stream emits, for a caller that wants them by name.
    pub fn to_json_rows(&self) -> Vec<serde_json::Map<String, Value>> {
        self.rows
            .iter()
            .map(|row| {
                self.columns
                    .iter()
                    .cloned()
                    .zip(row.iter().cloned())
                    .collect()
            })
            .collect()
    }
}

/// Why a write to a [`QueryStreamWriter`] did not go through. Both variants mean the same
/// thing to a stream handler: stop reading the data source and return.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StreamClosed {
    /// The consumer dropped the stream or it was cancelled.
    #[error("Query stream was cancelled")]
    Cancelled,
    /// Nobody drained the stream for `query_stream_idle_timeout`, so it was destroyed
    /// (`QueryStream.debounce`).
    #[error("Query stream was destroyed after being idle")]
    Idle,
}

enum StreamMessage {
    Batch {
        batch: QueryStreamBatch,
        /// Row permits the reader hands back once it took the batch.
        permits: u32,
    },
    Error(String),
}

/// The state both ends of one stream share.
pub(crate) struct StreamShared {
    query_key_hash: String,
    high_water_mark: usize,
    /// One permit per buffered row: the high-water mark, counted the way object mode counts it.
    permits: Arc<Semaphore>,
    rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<StreamMessage>>,
    columns: Mutex<Arc<Vec<String>>>,
    alias_name_to_member: Option<HashMap<String, String>>,
    /// Live [`QueryStream`] handles. The last one to go cancels the query.
    consumers: AtomicUsize,
    cancelled: AtomicBool,
    cancel_notify: Notify,
    registry: Weak<QueryStreamRegistry>,
}

impl std::fmt::Debug for StreamShared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryStream")
            .field("queryKey", &self.query_key_hash)
            .field("highWaterMark", &self.high_water_mark)
            .field("cancelled", &self.cancelled.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl StreamShared {
    /// `QueryStream.destroy()`: nothing more will be read, the producer is told to stop and
    /// the stream leaves the queue's map (`QO/QueryStream.ts:63-71`).
    fn cancel(shared: &Arc<Self>) {
        if shared.cancelled.swap(true, Ordering::SeqCst) {
            return;
        }

        // Releases a producer blocked on the high-water mark with an error.
        shared.permits.close();
        shared.cancel_notify.notify_waiters();

        if let Some(registry) = shared.registry.upgrade() {
            registry.remove_if_same(&shared.query_key_hash, shared);
        }
    }
}

/// The consuming end of a persistent query.
///
/// Cloning hands out another handle to the *same* stream, which is what
/// `QueryQueue.waitForQueryStream` does when two callers wait for one persistent key; a
/// batch is delivered to exactly one of the handles. The query is cancelled once the last
/// handle is dropped, so a consumer that walks away does not leak the data source
/// connection behind it.
pub struct QueryStream {
    shared: Arc<StreamShared>,
}

impl QueryStream {
    fn attach(shared: Arc<StreamShared>) -> Self {
        shared.consumers.fetch_add(1, Ordering::SeqCst);

        Self { shared }
    }

    /// The hash of the persistent query key this stream belongs to.
    pub fn query_key_hash(&self) -> &str {
        &self.shared.query_key_hash
    }

    /// Rows the producer may run ahead of the consumer.
    pub fn high_water_mark(&self) -> usize {
        self.shared.high_water_mark
    }

    /// The member names of the rows, known once the driver answered. Every batch carries
    /// them too, so a consumer that only reads batches never needs this.
    pub fn columns(&self) -> Arc<Vec<String>> {
        self.shared.columns.lock().unwrap().clone()
    }

    /// The next batch, `None` once the query finished, `Some(Err)` when it failed.
    ///
    /// An error is terminal: the producer stops after reporting one.
    pub async fn next_batch(&self) -> Option<Result<QueryStreamBatch, String>> {
        let message = {
            let mut rx = self.shared.rx.lock().await;

            rx.recv().await
        };

        match message {
            Some(StreamMessage::Batch { batch, permits }) => {
                // The rows left the buffer, so the producer may run that far ahead again.
                self.shared.permits.add_permits(permits as usize);

                Some(Ok(batch))
            }
            Some(StreamMessage::Error(error)) => Some(Err(error)),
            None => None,
        }
    }

    /// Reads the whole stream into memory. Convenient for a caller that cannot stream and
    /// for tests; it defeats the point of a persistent query otherwise.
    pub async fn collect_rows(&self) -> Result<Vec<StreamRow>, String> {
        let mut rows = Vec::new();

        while let Some(batch) = self.next_batch().await {
            rows.extend(batch?.rows);
        }

        Ok(rows)
    }

    /// Consumes the handle as a [`futures::Stream`].
    pub fn into_stream(self) -> impl futures::Stream<Item = Result<QueryStreamBatch, String>> {
        futures::stream::unfold(self, |stream| async move {
            stream.next_batch().await.map(|item| (item, stream))
        })
    }

    pub fn is_cancelled(&self) -> bool {
        self.shared.cancelled.load(Ordering::SeqCst)
    }

    /// `QueryStream.destroy()` — stops the query behind the stream.
    pub fn cancel(&self) {
        StreamShared::cancel(&self.shared);
    }
}

impl Clone for QueryStream {
    fn clone(&self) -> Self {
        Self::attach(self.shared.clone())
    }
}

/// Two handles are equal when they are handles on the same stream.
impl PartialEq for QueryStream {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }
}

impl std::fmt::Debug for QueryStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.shared.fmt(f)
    }
}

impl Drop for QueryStream {
    fn drop(&mut self) {
        // The consumer went away: cancel rather than leave the query running into a buffer
        // nobody reads.
        if self.shared.consumers.fetch_sub(1, Ordering::SeqCst) == 1 {
            StreamShared::cancel(&self.shared);
        }
    }
}

/// The producing end, handed to the queue's stream handler.
pub struct QueryStreamWriter {
    shared: Arc<StreamShared>,
    tx: mpsc::UnboundedSender<StreamMessage>,
    idle_timeout: Duration,
}

impl std::fmt::Debug for QueryStreamWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.shared.fmt(f)
    }
}

impl QueryStreamWriter {
    /// Rows this writer may run ahead of the consumer, which is also the `highWaterMark`
    /// the driver's own row stream is asked for.
    pub fn high_water_mark(&self) -> usize {
        self.shared.high_water_mark
    }

    pub fn query_key_hash(&self) -> &str {
        &self.shared.query_key_hash
    }

    /// Publishes the column names of the result, with `aliasNameToMember` applied. Called
    /// once, as soon as the driver answered and before the first batch.
    ///
    /// An alias the map does not mention keeps its own name; the Node code writes
    /// `row[undefined]` in that case, which is a bug rather than a behaviour to port.
    pub fn set_columns(&self, columns: Vec<String>) -> Arc<Vec<String>> {
        let mapped: Vec<String> = match &self.shared.alias_name_to_member {
            None => columns,
            Some(alias_name_to_member) => columns
                .into_iter()
                .map(|column| alias_name_to_member.get(&column).cloned().unwrap_or(column))
                .collect(),
        };

        let mapped = Arc::new(mapped);
        *self.shared.columns.lock().unwrap() = mapped.clone();

        mapped
    }

    /// Hands a batch of rows to the consumer, waiting while the high-water mark is reached.
    pub async fn write(&self, rows: Vec<StreamRow>) -> Result<(), StreamClosed> {
        if self.shared.cancelled.load(Ordering::SeqCst) {
            return Err(StreamClosed::Cancelled);
        }

        if rows.is_empty() {
            return Ok(());
        }

        // A batch bigger than the whole buffer is let through rather than deadlocked.
        let permits = rows.len().min(self.shared.high_water_mark).max(1) as u32;

        let acquired = tokio::time::timeout(
            self.idle_timeout,
            self.shared.permits.clone().acquire_many_owned(permits),
        )
        .await;

        match acquired {
            // `QueryStream.debounce`: a stream nobody drains is destroyed.
            Err(_elapsed) => {
                StreamShared::cancel(&self.shared);

                return Err(StreamClosed::Idle);
            }
            Ok(Err(_closed)) => return Err(StreamClosed::Cancelled),
            // The reader hands the permits back when it takes the batch.
            Ok(Ok(permit)) => permit.forget(),
        }

        let batch = QueryStreamBatch {
            columns: self.columns(),
            rows,
        };

        if self
            .tx
            .send(StreamMessage::Batch { batch, permits })
            .is_err()
        {
            return Err(StreamClosed::Cancelled);
        }

        // `QueryStream._transform` drops the stream from the queue's map as soon as it
        // carries data, so that a later waiter is not handed a stream already in flight.
        // Unlike Node this waits for a consumer to have attached: the event that hands the
        // stream over is synchronous there and is not here.
        if self.shared.consumers.load(Ordering::SeqCst) > 0 {
            self.unregister();
        }

        Ok(())
    }

    /// Reports a failure to the consumer, which sees it as the stream's last item.
    ///
    /// `streamHandler` destroys the target stream with the error instead of letting the
    /// consumer see a clean end of stream (`QO/QueryCache.ts:820-846`).
    pub fn fail(&self, error: impl Into<String>) {
        let _ = self.tx.send(StreamMessage::Error(error.into()));
    }

    pub fn is_cancelled(&self) -> bool {
        self.shared.cancelled.load(Ordering::SeqCst)
    }

    /// Resolves once the consumer is gone, so a handler can stop reading its data source
    /// without waiting for the next row to arrive.
    pub async fn cancelled(&self) {
        let notified = self.shared.cancel_notify.notified();
        tokio::pin!(notified);
        // Registers before the flag is read, so a cancel in between is not missed.
        notified.as_mut().enable();

        if self.shared.cancelled.load(Ordering::SeqCst) {
            return;
        }

        notified.await;
    }

    fn columns(&self) -> Arc<Vec<String>> {
        self.shared.columns.lock().unwrap().clone()
    }

    fn unregister(&self) {
        if let Some(registry) = self.shared.registry.upgrade() {
            registry.remove_if_same(&self.shared.query_key_hash, &self.shared);
        }
    }
}

impl Drop for QueryStreamWriter {
    fn drop(&mut self) {
        // `executeQuery`'s `finally`: the map never keeps a stream whose handler is done.
        self.unregister();
    }
}

/// The `streams` map of a queue plus its `streamStarted` event
/// (`QO/QueryQueue.ts:111-118, :166-182`).
pub struct QueryStreamRegistry {
    streams: Mutex<HashMap<String, Arc<StreamShared>>>,
    /// Who is waiting for a stream of a key that has not started yet.
    waiters: Mutex<HashMap<String, Vec<StreamWaiter>>>,
    next_waiter_id: AtomicU64,
}

impl std::fmt::Debug for QueryStreamRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryStreamRegistry")
            .field("streams", &self.keys())
            .finish()
    }
}

impl QueryStreamRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            streams: Mutex::new(HashMap::new()),
            waiters: Mutex::new(HashMap::new()),
            next_waiter_id: AtomicU64::new(1),
        })
    }

    /// `QueryQueue.createQueryStream(key, aliasNameToMember)` — registers the stream and
    /// emits `streamStarted`.
    pub fn create(
        self: &Arc<Self>,
        query_key_hash: impl Into<String>,
        alias_name_to_member: Option<HashMap<String, String>>,
        high_water_mark: usize,
        idle_timeout: Duration,
    ) -> QueryStreamWriter {
        let query_key_hash = query_key_hash.into();
        let (tx, rx) = mpsc::unbounded_channel();
        let high_water_mark = high_water_mark.max(1);

        let shared = Arc::new(StreamShared {
            query_key_hash: query_key_hash.clone(),
            high_water_mark,
            permits: Arc::new(Semaphore::new(high_water_mark)),
            rx: tokio::sync::Mutex::new(rx),
            columns: Mutex::new(Arc::new(Vec::new())),
            alias_name_to_member,
            consumers: AtomicUsize::new(0),
            cancelled: AtomicBool::new(false),
            cancel_notify: Notify::new(),
            registry: Arc::downgrade(self),
        });

        // A stream replacing an earlier one under the same key cancels it, the way a second
        // `createQueryStream` orphans the first stream object in Node.
        let replaced = self
            .streams
            .lock()
            .unwrap()
            .insert(query_key_hash.clone(), shared.clone());

        if let Some(replaced) = replaced {
            StreamShared::cancel(&replaced);
        }

        // `streamEvents.emit('streamStarted', key)`. The waiters registered before the
        // query was dispatched are handed the stream here and now: looking it up when they
        // wake would miss a handler that started and finished in between.
        for waiter in self.take_waiters(&query_key_hash) {
            let _ = waiter.notify.send(shared.clone());
        }

        QueryStreamWriter {
            shared,
            tx,
            idle_timeout,
        }
    }

    /// `QueryQueue.getQueryStream(hash)`.
    pub fn get(&self, query_key_hash: &str) -> Option<QueryStream> {
        self.streams
            .lock()
            .unwrap()
            .get(query_key_hash)
            .cloned()
            .map(QueryStream::attach)
    }

    pub fn contains(&self, query_key_hash: &str) -> bool {
        self.streams.lock().unwrap().contains_key(query_key_hash)
    }

    pub fn len(&self) -> usize {
        self.streams.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn keys(&self) -> Vec<String> {
        self.streams.lock().unwrap().keys().cloned().collect()
    }

    /// The `stream` cancel handler: destroys the stream of a persistent key
    /// (`QO/QueryCache.ts:876-882`).
    pub fn destroy(&self, query_key_hash: &str) -> bool {
        let shared = self.streams.lock().unwrap().remove(query_key_hash);

        match shared {
            Some(shared) => {
                StreamShared::cancel(&shared);

                true
            }
            None => false,
        }
    }

    /// Registers interest in the stream of `query_key_hash` before the query is
    /// dispatched, which is what `executeInQueue` does with the `streamStarted` listener:
    /// a handler which starts fast would otherwise have come and gone.
    pub fn subscribe(self: &Arc<Self>, query_key_hash: &str) -> StreamWait {
        let id = self.next_waiter_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();

        self.waiters
            .lock()
            .unwrap()
            .entry(query_key_hash.to_string())
            .or_default()
            .push(StreamWaiter { id, notify: tx });

        StreamWait {
            registry: Arc::downgrade(self),
            query_key_hash: query_key_hash.to_string(),
            id,
            rx,
        }
    }

    fn take_waiters(&self, query_key_hash: &str) -> Vec<StreamWaiter> {
        self.waiters
            .lock()
            .unwrap()
            .remove(query_key_hash)
            .unwrap_or_default()
    }

    fn remove_waiter(&self, query_key_hash: &str, id: u64) {
        let mut waiters = self.waiters.lock().unwrap();

        if let Some(entries) = waiters.get_mut(query_key_hash) {
            entries.retain(|entry| entry.id != id);

            if entries.is_empty() {
                waiters.remove(query_key_hash);
            }
        }
    }

    fn remove_if_same(&self, query_key_hash: &str, shared: &Arc<StreamShared>) {
        let mut streams = self.streams.lock().unwrap();

        if let Some(current) = streams.get(query_key_hash) {
            if Arc::ptr_eq(current, shared) {
                streams.remove(query_key_hash);
            }
        }
    }
}

/// One registered waiter of [`QueryStreamRegistry::subscribe`].
struct StreamWaiter {
    id: u64,
    notify: oneshot::Sender<Arc<StreamShared>>,
}

/// A registered interest in the stream of one persistent key, the
/// `QueryStreamWait { promise, dispose }` of the source. Dropping it deregisters, so a
/// caller that timed out or gave up leaves nothing behind.
pub struct StreamWait {
    registry: Weak<QueryStreamRegistry>,
    query_key_hash: String,
    id: u64,
    rx: oneshot::Receiver<Arc<StreamShared>>,
}

impl std::fmt::Debug for StreamWait {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamWait")
            .field("queryKey", &self.query_key_hash)
            .finish()
    }
}

impl StreamWait {
    pub fn query_key_hash(&self) -> &str {
        &self.query_key_hash
    }

    /// Resolves once a stream started for this key. `None` when the registry went away.
    pub async fn recv(mut self) -> Option<QueryStream> {
        (&mut self.rx).await.ok().map(QueryStream::attach)
    }
}

impl Drop for StreamWait {
    fn drop(&mut self) {
        if let Some(registry) = self.registry.upgrade() {
            registry.remove_waiter(&self.query_key_hash, self.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::*;

    fn registry() -> Arc<QueryStreamRegistry> {
        QueryStreamRegistry::new()
    }

    fn rows(values: &[i64]) -> Vec<StreamRow> {
        values.iter().map(|value| vec![json!(value)]).collect()
    }

    #[tokio::test]
    async fn batches_arrive_in_order_and_the_stream_ends() {
        let registry = registry();
        let writer = registry.create("key", None, 8, Duration::from_secs(5));
        let stream = registry.get("key").unwrap();

        writer.set_columns(vec!["a".to_string()]);
        writer.write(rows(&[1, 2])).await.unwrap();
        writer.write(rows(&[3])).await.unwrap();
        drop(writer);

        let first = stream.next_batch().await.unwrap().unwrap();
        assert_eq!(first.rows, rows(&[1, 2]));
        assert_eq!(*first.columns, vec!["a".to_string()]);

        let second = stream.next_batch().await.unwrap().unwrap();
        assert_eq!(second.rows, rows(&[3]));

        assert!(stream.next_batch().await.is_none());
    }

    #[tokio::test]
    async fn alias_names_are_mapped_to_members_once() {
        let registry = registry();
        let alias = HashMap::from([("a_0".to_string(), "Orders.count".to_string())]);
        let writer = registry.create("key", Some(alias), 8, Duration::from_secs(5));
        let stream = registry.get("key").unwrap();

        writer.set_columns(vec!["a_0".to_string(), "unmapped".to_string()]);
        writer.write(vec![vec![json!(1), json!(2)]]).await.unwrap();

        let batch = stream.next_batch().await.unwrap().unwrap();
        assert_eq!(
            *batch.columns,
            vec!["Orders.count".to_string(), "unmapped".to_string()]
        );
        assert_eq!(batch.to_json_rows()[0]["Orders.count"], json!(1));
    }

    #[tokio::test]
    async fn writing_blocks_at_the_high_water_mark() {
        let registry = registry();
        let writer = registry.create("key", None, 4, Duration::from_secs(30));
        let stream = registry.get("key").unwrap();

        writer.write(rows(&[1, 2])).await.unwrap();
        writer.write(rows(&[3, 4])).await.unwrap();

        // 4 rows are buffered, so the next write waits for the consumer.
        let blocked = writer.write(rows(&[5]));
        tokio::pin!(blocked);

        assert!(
            tokio::time::timeout(Duration::from_millis(50), blocked.as_mut())
                .await
                .is_err()
        );

        let batch = stream.next_batch().await.unwrap().unwrap();
        assert_eq!(batch.rows, rows(&[1, 2]));

        // taking the batch released its rows
        tokio::time::timeout(Duration::from_millis(500), blocked)
            .await
            .expect("write resumes once the buffer drained")
            .unwrap();
    }

    #[tokio::test]
    async fn dropping_the_consumer_cancels_the_writer() {
        let registry = registry();
        let writer = registry.create("key", None, 2, Duration::from_secs(30));
        let stream = registry.get("key").unwrap();

        writer.write(rows(&[1, 2])).await.unwrap();

        let blocked = tokio::spawn({
            // the writer is blocked on a full buffer
            let registry = registry.clone();

            async move {
                let writer = writer;
                let _ = registry;

                writer.write(rows(&[3, 4])).await
            }
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(stream);

        assert_eq!(
            blocked.await.unwrap(),
            Err(StreamClosed::Cancelled),
            "a blocked write is released with an error when the consumer leaves"
        );
        assert!(!registry.contains("key"));
    }

    #[tokio::test]
    async fn an_idle_stream_is_destroyed() {
        let registry = registry();
        let writer = registry.create("key", None, 1, Duration::from_millis(50));
        let stream = registry.get("key").unwrap();

        writer.write(rows(&[1])).await.unwrap();

        assert_eq!(writer.write(rows(&[2])).await, Err(StreamClosed::Idle));
        assert!(stream.is_cancelled());
        assert!(!registry.contains("key"));
    }

    #[tokio::test]
    async fn an_error_reaches_the_consumer() {
        let registry = registry();
        let writer = registry.create("key", None, 4, Duration::from_secs(5));
        let stream = registry.get("key").unwrap();

        writer.write(rows(&[1])).await.unwrap();
        writer.fail("connection reset");
        drop(writer);

        assert!(stream.next_batch().await.unwrap().is_ok());
        assert_eq!(
            stream.next_batch().await.unwrap().unwrap_err(),
            "connection reset"
        );
        assert!(stream.next_batch().await.is_none());
    }

    #[tokio::test]
    async fn two_waiters_share_one_stream() {
        let registry = registry();
        let writer = registry.create("key", None, 4, Duration::from_secs(5));

        let first = registry.get("key").unwrap();
        let second = registry.get("key").unwrap();

        assert_eq!(first, second);

        writer.write(rows(&[1])).await.unwrap();
        // the stream left the map once it carries data
        assert!(!registry.contains("key"));

        // one batch, delivered once
        assert!(second.next_batch().await.is_some());
        drop(first);
    }

    #[tokio::test]
    async fn the_last_consumer_leaving_cancels_the_stream() {
        let registry = registry();
        let writer = registry.create("key", None, 4, Duration::from_secs(5));

        let first = registry.get("key").unwrap();
        let second = first.clone();

        drop(first);
        assert!(!writer.is_cancelled(), "one handle is still alive");

        drop(second);
        assert!(writer.is_cancelled());
    }

    #[tokio::test]
    async fn cancelled_resolves_when_the_stream_is_destroyed() {
        let registry = registry();
        let writer = registry.create("key", None, 4, Duration::from_secs(5));
        let stream = registry.get("key").unwrap();

        let waiting = tokio::spawn(async move {
            let writer = writer;
            writer.cancelled().await;

            true
        });

        tokio::time::sleep(Duration::from_millis(10)).await;
        stream.cancel();

        assert!(tokio::time::timeout(Duration::from_millis(500), waiting)
            .await
            .unwrap()
            .unwrap());
    }

    #[tokio::test]
    async fn destroying_a_registered_stream_stops_the_producer() {
        let registry = registry();
        let writer = registry.create("key", None, 4, Duration::from_secs(5));
        let stream = registry.get("key").unwrap();

        assert!(registry.destroy("key"));
        assert!(!registry.destroy("key"));

        assert_eq!(writer.write(rows(&[1])).await, Err(StreamClosed::Cancelled));
        assert!(stream.is_cancelled());
    }

    #[test]
    fn the_high_water_mark_falls_back_to_the_default() {
        assert_eq!(parse_high_water_mark(Some("4096".to_string())), 4096);
        assert_eq!(
            parse_high_water_mark(Some("nonsense".to_string())),
            DEFAULT_QUERY_STREAM_HIGH_WATER_MARK
        );
        assert_eq!(
            parse_high_water_mark(Some("0".to_string())),
            DEFAULT_QUERY_STREAM_HIGH_WATER_MARK
        );
        assert_eq!(
            parse_high_water_mark(None),
            DEFAULT_QUERY_STREAM_HIGH_WATER_MARK
        );
    }
}
