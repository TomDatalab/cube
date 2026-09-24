//! A thread pool that owns the planner's models.
//!
//! [`cubeplanner::Model`] mirrors the `cube_bridge` traits, which are single
//! threaded, so it is built on `Rc` and is neither `Send` nor `Sync`. An async
//! server cannot hold one. [`PlannerPool`] gives each worker thread its own
//! compiled model and talks to those threads over channels, which confines the
//! `Rc`s to one thread without a single `unsafe` block and still plans several
//! requests at once.
//!
//! This mirrors `cubeserver::planner_adapter::PlannerPool`, which does the same
//! for the REST API.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use cubeplanner::{plan, Dialect, Model, PlanOptions, PlannerError, PlannerQuery};
use cubesql::CubeError;
use serde_json::Value;
use tokio::sync::oneshot;

use crate::model_source::ModelSource;

/// The statement and its bound parameters, in placeholder order.
pub type PlannedStatement = (String, Vec<Option<String>>);

struct Job {
    query: Box<PlannerQuery>,
    options: Box<PlanOptions>,
    reply: oneshot::Sender<Result<PlannedStatement, CubeError>>,
}

/// A fixed set of worker threads, each owning its own compiled [`Model`].
pub struct PlannerPool {
    jobs: Sender<Job>,
    workers: usize,
    source: ModelSource,
}

impl std::fmt::Debug for PlannerPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlannerPool")
            .field("source", &self.source)
            .field("workers", &self.workers)
            .finish()
    }
}

/// A planning failure caused by the request is the client's fault, anything
/// else is ours - the same split `ApiGateway.handleError` makes.
fn to_cube_error(err: PlannerError) -> CubeError {
    match err {
        PlannerError::Query(_) | PlannerError::Unsupported(_) => CubeError::user(err.message()),
        other => CubeError::internal(other.message()),
    }
}

impl PlannerPool {
    /// Compiles the model once up front so a broken model fails at start-up,
    /// then spawns `workers` threads that each compile their own copy.
    pub fn load(source: ModelSource, workers: usize) -> Result<Self, CubeError> {
        source.load_planner_model()?;

        let workers = workers.max(1);
        let (jobs, receiver) = mpsc::channel::<Job>();
        let receiver = Arc::new(Mutex::new(receiver));

        for index in 0..workers {
            let receiver = Arc::clone(&receiver);
            let source = source.clone();
            thread::Builder::new()
                .name(format!("cube-sql-planner-{index}"))
                .spawn(move || Self::worker(receiver, source))
                .map_err(|e| {
                    CubeError::internal(format!("Failed to spawn a planner thread: {e}"))
                })?;
        }

        Ok(Self {
            jobs,
            workers,
            source,
        })
    }

    pub fn workers(&self) -> usize {
        self.workers
    }

    fn worker(receiver: Arc<Mutex<Receiver<Job>>>, source: ModelSource) {
        let model = match source.load_planner_model() {
            Ok(model) => model,
            Err(err) => {
                // The model compiled at start-up, so this is unexpected;
                // answer every job with the error rather than exiting quietly
                // and leaving callers waiting on a dropped channel. `CubeError`
                // is not `Clone`, so the message is what is kept.
                let message = err.message;
                while let Ok(job) = Self::next_job(&receiver) {
                    let _ = job.reply.send(Err(CubeError::internal(message.clone())));
                }
                return;
            }
        };

        while let Ok(job) = Self::next_job(&receiver) {
            let result = Self::plan_one(&model, &job.query, &job.options);
            // A dropped receiver means the request is gone; nothing to do.
            let _ = job.reply.send(result);
        }
    }

    fn plan_one(
        model: &Model,
        query: &PlannerQuery,
        options: &PlanOptions,
    ) -> Result<PlannedStatement, CubeError> {
        plan(model, query, options)
            .map(|planned| (planned.sql.clone(), planned.param_strings()))
            .map_err(to_cube_error)
    }

    fn next_job(receiver: &Arc<Mutex<Receiver<Job>>>) -> Result<Job, ()> {
        let guard = receiver.lock().map_err(|_| ())?;
        guard.recv().map_err(|_| ())
    }

    /// Plans one query on a worker thread.
    pub async fn plan(
        &self,
        query: PlannerQuery,
        options: PlanOptions,
    ) -> Result<PlannedStatement, CubeError> {
        let (reply, response) = oneshot::channel();

        self.jobs
            .send(Job {
                query: Box::new(query),
                options: Box::new(options),
                reply,
            })
            .map_err(|_| CubeError::internal("The planner pool has shut down".to_string()))?;

        response
            .await
            .map_err(|_| CubeError::internal("The planner worker stopped".to_string()))?
    }
}

/// Which dialect a data source renders in. `default` covers everything the map
/// does not name.
#[derive(Debug, Clone)]
pub struct DialectMap {
    pub default: Dialect,
    pub by_data_source: std::collections::HashMap<String, Dialect>,
}

impl Default for DialectMap {
    fn default() -> Self {
        Self {
            default: Dialect::Postgres,
            by_data_source: std::collections::HashMap::new(),
        }
    }
}

/// Converts the SQL API's load query into the planner's query shape.
///
/// Both are the camelCase JSON of the REST API, so this is a re-read rather
/// than a field-by-field translation, which keeps the two from drifting.
pub fn to_planner_query(
    query: &cubesql::transport::TransportLoadRequestQuery,
    member_to_alias: Option<std::collections::HashMap<String, String>>,
) -> Result<PlannerQuery, CubeError> {
    let mut value = serde_json::to_value(query).map_err(|e| {
        CubeError::internal(format!("Failed to encode the load request query: {e}"))
    })?;

    // `limit` and `rowLimit` are the same field to the planner, and the SQL
    // API only ever sends `limit`.
    if let Some(object) = value.as_object_mut() {
        if object.contains_key("limit") {
            object.remove("rowLimit");
        }
    }

    let mut planner_query = PlannerQuery::from_value(value).map_err(to_cube_error)?;
    if member_to_alias.is_some() {
        planner_query.member_to_alias = member_to_alias;
    }

    Ok(planner_query)
}

/// The planner options for one request.
pub fn plan_options(dialects: &DialectMap, security_context: Value) -> PlanOptions {
    PlanOptions::default()
        .with_dialect(dialects.default)
        .with_security_context(security_context)
}
