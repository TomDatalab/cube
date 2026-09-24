//! Where `load` / `load_stream` actually run a query.
//!
//! Planning is native (see [`crate::planner`]), but executing a query needs the
//! query orchestrator: the queue, the pre-aggregation store and the driver
//! pool. That lives in its own crate, so this one only states the shape of the
//! hand-off and ships a stub that says so.

use async_trait::async_trait;
use cubesql::compile::engine::df::scan::{MemberField, SchemaRef};
use cubesql::transport::{CubeStreamReceiver, TransportLoadResponseColumnar};
use cubesql::CubeError;
use serde_json::Value;

/// The columnar `/v1/load` response body, which is what cubesql turns into
/// Arrow record batches.
pub type LoadResponse = TransportLoadResponseColumnar;

/// One result set of a [`LoadResponse`], re-exported so an executor can build
/// one without depending on `cubeclient` itself.
pub type LoadResult = cubeclient::models::V1LoadResult<LoadResultDataColumnar>;

/// The `members` / `columns` payload of one [`LoadResult`].
pub type LoadResultDataColumnar = cubeclient::models::V1LoadResultDataColumnar;

/// The `annotation` every [`LoadResult`] carries.
pub type LoadResultAnnotation = cubeclient::models::V1LoadResultAnnotation;

/// Runs one already-planned query.
///
/// `query` is the JSON request [`crate::transport::RustTransport`] builds -
/// the same `{ request, query, sqlQuery, streaming, cacheMode, queryKey }`
/// object the Node.js bridge posts to `sqlApiLoad`, so an orchestrator can
/// read it without a translation layer. `security_context` is the verified JWT
/// payload of the session, already switched to the `__user` in effect.
///
/// An implementation that cannot serve a request right now returns
/// [`CubeError::continue_wait`]; the transport retries, or reports it to the
/// caller when the caller asked for that.
#[async_trait]
pub trait QueryExecutor: Send + Sync + std::fmt::Debug {
    /// Runs the query and returns the whole result set.
    async fn execute(
        &self,
        query: Value,
        security_context: &Value,
    ) -> Result<LoadResponse, CubeError>;

    /// Runs the query and streams the result set back in batches.
    ///
    /// `schema` and `member_fields` describe the batches cubesql expects, so an
    /// implementation converts as it goes instead of buffering the whole
    /// result. The default refuses, because an executor that can only answer
    /// in one piece is still useful for everything but `stream_mode`.
    async fn execute_stream(
        &self,
        _query: Value,
        _security_context: &Value,
        _schema: SchemaRef,
        _member_fields: Vec<MemberField>,
    ) -> Result<CubeStreamReceiver, CubeError> {
        Err(CubeError::user(
            "Streaming is not supported by the configured query executor".to_string(),
        ))
    }
}

/// The executor in place until the query orchestrator is wired in.
///
/// Everything that does not need data - connecting, `SELECT 1`, the catalog
/// queries a BI tool runs on connect, `/v1/meta`, SQL generation - works
/// against it; a query that has to read a data source fails with a message
/// that says exactly what is missing rather than a cast or channel error.
#[derive(Debug, Default, Clone, Copy)]
pub struct NotConfiguredExecutor;

impl NotConfiguredExecutor {
    pub fn new() -> Self {
        Self
    }

    fn error() -> CubeError {
        CubeError::user(
            "No query executor is configured for the SQL API, so this query cannot be run. \
             The SQL API can compile and plan queries, but running one needs the query \
             orchestrator; pass it to the transport as a `QueryExecutor`."
                .to_string(),
        )
    }
}

#[async_trait]
impl QueryExecutor for NotConfiguredExecutor {
    async fn execute(
        &self,
        _query: Value,
        _security_context: &Value,
    ) -> Result<LoadResponse, CubeError> {
        Err(Self::error())
    }

    async fn execute_stream(
        &self,
        _query: Value,
        _security_context: &Value,
        _schema: SchemaRef,
        _member_fields: Vec<MemberField>,
    ) -> Result<CubeStreamReceiver, CubeError> {
        Err(Self::error())
    }
}
