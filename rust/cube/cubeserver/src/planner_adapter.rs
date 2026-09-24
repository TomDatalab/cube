//! [`QueryService`] backed by the `cubeplanner` crate: `/v1/sql` and
//! `/v1/dry-run` plan SQL from the YAML data model with no JavaScript.
//!
//! `/v1/load` still needs the query orchestrator (queue, cache and
//! pre-aggregations), so it keeps answering 501 here.
//!
//! The planner's model is built on `Rc` (it mirrors the `cube_bridge` traits,
//! which are single threaded), so it is neither `Send` nor `Sync` and cannot
//! be shared by an async server. [`PlannerPool`] keeps each model on its own
//! OS thread and talks to those threads over channels, which confines the
//! `Rc`s to one thread and still plans several requests in parallel.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use async_trait::async_trait;
use axum::http::StatusCode;
use cubeplanner::{plan, Dialect, Model, PlanOptions, PlannerError, PlannerQuery};
use cubequery::types::QueryType;
use serde_json::{json, Map, Value};
use tokio::sync::oneshot;

use crate::error::ApiError;
use crate::services::{NormalizedRequest, QueryService, RequestContext};

/// The planned statement of one query, in a form that can cross threads.
#[derive(Debug, Clone)]
pub struct PlannedQuery {
    pub sql: String,
    pub params: Vec<Option<String>>,
    /// `{ member: "asc" | "desc" }`, as the REST response spells it.
    pub order: Map<String, Value>,
}

impl PlannedQuery {
    /// The `sql` member of the `/v1/sql` response.
    ///
    /// The Node.js gateway also returns orchestrator-derived fields
    /// (`preAggregations`, `cacheKeyQueries`, `aliasNameToMember`,
    /// `canUseTransformedQuery`, `external`, `dataSource`); they do not exist
    /// until the orchestrator is ported.
    pub fn to_json(&self) -> Value {
        json!({
            "sql": [self.sql, self.params],
            "order": Value::Object(self.order.clone()),
        })
    }
}

/// A planning failure in a form that can cross a thread boundary
/// (`PlannerError` carries a `CubeError`, which is not `Send`).
#[derive(Debug, Clone)]
pub struct PlanFailure {
    /// The request caused it, so the client gets a 4xx.
    pub user_error: bool,
    pub message: String,
}

impl PlanFailure {
    fn from_planner(err: PlannerError) -> Self {
        Self {
            user_error: matches!(err, PlannerError::Query(_) | PlannerError::Unsupported(_)),
            message: err.message(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            user_error: false,
            message: message.into(),
        }
    }
}

/// A planned statement: the SQL, its bound parameters, and the map that turns
/// result-set column names back into member names.
#[derive(Debug, Clone)]
pub struct PlannedStatement {
    pub sql: String,
    pub params: Vec<Option<String>>,
    /// `orders__status` → `orders.status` (`BaseQuery.aliasNameToMember`).
    pub alias_name_to_member: std::collections::HashMap<String, String>,
}

struct Job {
    query: PlannerQuery,
    security_context: Value,
    reply: oneshot::Sender<Result<PlannedStatement, PlanFailure>>,
}

/// A fixed set of worker threads, each owning its own compiled [`Model`].
pub struct PlannerPool {
    jobs: Sender<Job>,
    workers: usize,
    model_path: PathBuf,
}

impl std::fmt::Debug for PlannerPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlannerPool")
            .field("model_path", &self.model_path)
            .field("workers", &self.workers)
            .finish()
    }
}

impl PlannerPool {
    /// Compiles the model once up front to surface errors at start, then
    /// spawns `workers` threads that each compile their own copy.
    pub fn load(
        model_path: impl AsRef<Path>,
        dialect: Dialect,
        workers: usize,
    ) -> Result<Self, PlannerError> {
        Self::load_with_context(
            model_path,
            dialect,
            workers,
            cubemodel::TemplateContext::default(),
        )
    }

    /// As [`Self::load`], with the `COMPILE_CONTEXT` the templates see.
    pub fn load_with_context(
        model_path: impl AsRef<Path>,
        dialect: Dialect,
        workers: usize,
        context: cubemodel::TemplateContext,
    ) -> Result<Self, PlannerError> {
        let model_path = model_path.as_ref().to_path_buf();
        // Fail fast on a broken model instead of inside every worker.
        Model::from_dir_with_context(&model_path, &context)?;

        let workers = workers.max(1);
        let (jobs, receiver) = mpsc::channel::<Job>();
        let receiver = Arc::new(Mutex::new(receiver));

        for index in 0..workers {
            let receiver = Arc::clone(&receiver);
            let path = model_path.clone();
            let context = context.clone();
            thread::Builder::new()
                .name(format!("cube-planner-{index}"))
                .spawn(move || Self::worker(receiver, path, dialect, context))
                .map_err(|e| {
                    PlannerError::model(format!("Failed to spawn a planner thread: {e}"))
                })?;
        }

        Ok(Self {
            jobs,
            workers,
            model_path,
        })
    }

    fn worker(
        receiver: Arc<Mutex<Receiver<Job>>>,
        model_path: PathBuf,
        dialect: Dialect,
        context: cubemodel::TemplateContext,
    ) {
        let model = match Model::from_dir_with_context(&model_path, &context) {
            Ok(model) => model,
            Err(err) => {
                // The model compiled at start-up, so this is unexpected;
                // answer every job with the error rather than exiting quietly.
                loop {
                    let Ok(job) = Self::next_job(&receiver) else {
                        return;
                    };
                    let _ = job.reply.send(Err(PlanFailure::internal(err.message())));
                }
            }
        };

        while let Ok(job) = Self::next_job(&receiver) {
            let options = PlanOptions::default()
                .with_dialect(dialect)
                .with_security_context(job.security_context);

            let result = plan(&model, &job.query, &options)
                .map(|planned| PlannedStatement {
                    sql: planned.sql.clone(),
                    params: planned.param_strings(),
                    alias_name_to_member: planned.alias_name_to_member.clone(),
                })
                .map_err(PlanFailure::from_planner);

            // A dropped receiver means the request is gone; nothing to do.
            let _ = job.reply.send(result);
        }
    }

    fn next_job(receiver: &Arc<Mutex<Receiver<Job>>>) -> Result<Job, ()> {
        let guard = receiver.lock().map_err(|_| ())?;
        guard.recv().map_err(|_| ())
    }

    /// Plans one query on a worker thread.
    pub async fn plan(
        &self,
        query: PlannerQuery,
        security_context: Value,
    ) -> Result<PlannedStatement, PlanFailure> {
        let (reply, response) = oneshot::channel();

        self.jobs
            .send(Job {
                query,
                security_context,
                reply,
            })
            .map_err(|_| PlanFailure::internal("The planner pool has shut down"))?;

        response
            .await
            .map_err(|_| PlanFailure::internal("The planner worker stopped"))?
    }
}

/// Plans SQL for REST requests from a model directory.
#[derive(Debug)]
pub struct PlannerQueryService {
    pool: PlannerPool,
}

impl PlannerQueryService {
    pub fn load(
        model_path: impl AsRef<Path>,
        dialect: Dialect,
        workers: usize,
    ) -> Result<Self, PlannerError> {
        Ok(Self {
            pool: PlannerPool::load(model_path, dialect, workers)?,
        })
    }

    /// As [`Self::load`], with the `COMPILE_CONTEXT` the model's templates
    /// render with. Each tenant compiles with its own.
    pub fn load_with_context(
        model_path: impl AsRef<Path>,
        dialect: Dialect,
        workers: usize,
        context: cubemodel::TemplateContext,
    ) -> Result<Self, PlannerError> {
        Ok(Self {
            pool: PlannerPool::load_with_context(model_path, dialect, workers, context)?,
        })
    }

    /// A planning failure caused by the request is a client error; anything
    /// else is not, like `ApiGateway.handleError`.
    fn api_error(failure: PlanFailure) -> ApiError {
        if failure.user_error {
            ApiError::bad_request(failure.message)
        } else {
            ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, failure.message)
        }
    }

    /// Plans one already-normalized query, for callers that hold the JSON
    /// rather than a `NormalizedRequest` (the orchestrator's compiler).
    pub async fn plan_value(
        &self,
        normalized_query: &Value,
        security_context: &Value,
    ) -> Result<PlannedStatement, PlanFailure> {
        let mut value = normalized_query.clone();
        if let Some(object) = value.as_object_mut() {
            if object.contains_key("limit") {
                object.remove("rowLimit");
            }
        }

        let query = PlannerQuery::from_value(value).map_err(PlanFailure::from_planner)?;
        self.pool.plan(query, security_context.clone()).await
    }

    async fn plan_all(
        &self,
        ctx: &RequestContext,
        request: &NormalizedRequest,
    ) -> Result<Vec<PlannedQuery>, ApiError> {
        let mut planned = Vec::with_capacity(request.queries.len());

        for normalized in &request.queries {
            // Both shapes are the camelCase JSON of the REST API.
            let mut value = serde_json::to_value(normalized).map_err(|e| {
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to encode the normalized query: {e}"),
                )
            })?;

            // `remapToQueryAdapterFormat` mirrors `limit` into `rowLimit`, but
            // the planner treats `rowLimit` as an alias of `limit`, so sending
            // both is a duplicate field. They hold the same value here.
            if let Some(object) = value.as_object_mut() {
                if object.contains_key("limit") {
                    object.remove("rowLimit");
                }
            }
            let query = PlannerQuery::from_value(value)
                .map_err(|e| Self::api_error(PlanFailure::from_planner(e)))?;

            let statement = self
                .pool
                .plan(query, ctx.security_context.clone())
                .await
                .map_err(Self::api_error)?;

            planned.push(PlannedQuery {
                sql: statement.sql,
                params: statement.params,
                order: normalized
                    .order
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|item| {
                        (
                            item.id,
                            Value::String(if item.desc { "desc" } else { "asc" }.to_string()),
                        )
                    })
                    .collect(),
            });
        }

        Ok(planned)
    }
}

#[async_trait]
impl QueryService for PlannerQueryService {
    async fn sql(
        &self,
        ctx: &RequestContext,
        request: NormalizedRequest,
    ) -> Result<Value, ApiError> {
        let planned = self.plan_all(ctx, &request).await?;

        // A regular query answers with one object, the others with an array.
        Ok(if request.query_type == QueryType::RegularQuery {
            json!({ "sql": planned[0].to_json() })
        } else {
            Value::Array(
                planned
                    .iter()
                    .map(|p| json!({ "sql": p.to_json() }))
                    .collect(),
            )
        })
    }

    async fn dry_run(
        &self,
        ctx: &RequestContext,
        request: NormalizedRequest,
    ) -> Result<Value, ApiError> {
        let planned = self.plan_all(ctx, &request).await?;

        Ok(json!({
            "queryType": request.query_type.as_str(),
            "normalizedQueries": request.queries,
            "queryOrder": planned
                .iter()
                .map(|p| Value::Object(p.order.clone()))
                .collect::<Vec<_>>(),
            "pivotQuery": request.pivot_query,
        }))
    }
}
