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

use std::collections::{BTreeSet, HashMap};
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
    /// The data source the query's cubes declare, `None` when they declare
    /// none and the query runs on the caller's default one.
    pub data_source: Option<String>,
}

struct Job {
    query: PlannerQuery,
    /// Every member and cube name the query mentions, to find its data source.
    members: Vec<String>,
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
        Self::load_with_dialects(model_path, dialect, HashMap::new(), workers, context)
    }

    /// As [`Self::load_with_context`], planning a query whose cubes declare a
    /// `data_source` in that data source's dialect. `dialect` covers the cubes
    /// that declare none.
    pub fn load_with_dialects(
        model_path: impl AsRef<Path>,
        dialect: Dialect,
        by_data_source: HashMap<String, Dialect>,
        workers: usize,
        context: cubemodel::TemplateContext,
    ) -> Result<Self, PlannerError> {
        let model_path = model_path.as_ref().to_path_buf();
        let by_data_source = Arc::new(by_data_source);
        // Fail fast on a broken model instead of inside every worker.
        Model::from_dir_with_context(&model_path, &context)?;

        let workers = workers.max(1);
        let (jobs, receiver) = mpsc::channel::<Job>();
        let receiver = Arc::new(Mutex::new(receiver));

        for index in 0..workers {
            let receiver = Arc::clone(&receiver);
            let path = model_path.clone();
            let context = context.clone();
            let by_data_source = by_data_source.clone();
            thread::Builder::new()
                .name(format!("cube-planner-{index}"))
                .spawn(move || Self::worker(receiver, path, dialect, by_data_source, context))
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
        by_data_source: Arc<HashMap<String, Dialect>>,
        context: cubemodel::TemplateContext,
    ) {
        let compiled = Model::from_dir_with_context(&model_path, &context)
            .map_err(|err| err.message())
            .and_then(|model| {
                let sources = DataSources::load(&model_path, &context)?;
                Ok((model, sources))
            });
        let (model, sources) = match compiled {
            Ok(compiled) => compiled,
            Err(message) => {
                // The model compiled at start-up, so this is unexpected;
                // answer every job with the error rather than exiting quietly.
                loop {
                    let Ok(job) = Self::next_job(&receiver) else {
                        return;
                    };
                    let _ = job.reply.send(Err(PlanFailure::internal(message.clone())));
                }
            }
        };

        while let Ok(job) = Self::next_job(&receiver) {
            let result = sources
                .of_query(&job.members)
                .and_then(|data_source| {
                    let dialect = match &data_source {
                        None => dialect,
                        Some(name) => by_data_source.get(name).copied().ok_or_else(|| {
                            PlanFailure::internal(format!(
                                "The {name} data source is not declared in cube.yml, or its type has no SQL dialect"
                            ))
                        })?,
                    };
                    let options = PlanOptions::default()
                        .with_dialect(dialect)
                        .with_security_context(job.security_context);

                    plan(&model, &job.query, &options)
                        .map(|planned| PlannedStatement {
                            sql: planned.sql.clone(),
                            params: planned.param_strings(),
                            alias_name_to_member: planned.alias_name_to_member.clone(),
                            data_source,
                        })
                        .map_err(PlanFailure::from_planner)
                });

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
        members: Vec<String>,
        security_context: Value,
    ) -> Result<PlannedStatement, PlanFailure> {
        let (reply, response) = oneshot::channel();

        self.jobs
            .send(Job {
                query,
                members,
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

    /// As [`Self::load_with_context`], with a dialect per declared data source
    /// (see [`PlannerPool::load_with_dialects`]).
    pub fn load_with_dialects(
        model_path: impl AsRef<Path>,
        dialect: Dialect,
        by_data_source: HashMap<String, Dialect>,
        workers: usize,
        context: cubemodel::TemplateContext,
    ) -> Result<Self, PlannerError> {
        Ok(Self {
            pool: PlannerPool::load_with_dialects(
                model_path,
                dialect,
                by_data_source,
                workers,
                context,
            )?,
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

        let members = query_members(&value);
        let query = PlannerQuery::from_value(value).map_err(PlanFailure::from_planner)?;
        self.pool
            .plan(query, members, security_context.clone())
            .await
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
            let members = query_members(&value);
            let query = PlannerQuery::from_value(value)
                .map_err(|e| Self::api_error(PlanFailure::from_planner(e)))?;

            let statement = self
                .pool
                .plan(query, members, ctx.security_context.clone())
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

/// Which data source each cube and member of a model reads from, as
/// `CompilerApi.memberToDataSource` answers it.
struct DataSources {
    /// `cube.member` → data source; a view's member maps to the data source
    /// of the cube it was included from.
    members: HashMap<String, String>,
    /// Cube name → data source, for a query that names a cube but no member.
    cubes: HashMap<String, String>,
}

impl DataSources {
    fn load(model_path: &Path, context: &cubemodel::TemplateContext) -> Result<Self, String> {
        let model = cubemodel::ModelLoader::with_context(context.clone())
            .load_dir_with(model_path)
            .map_err(|e| e.to_string())?;

        let cubes = model
            .cube_list()
            .into_iter()
            .filter(|cube| !cube.is_view)
            .map(|cube| {
                let data_source = cube
                    .data_source
                    .clone()
                    .unwrap_or_else(|| cubesqlbridge::DEFAULT_DATA_SOURCE.to_string());
                (cube.name.clone(), data_source)
            })
            .collect();

        Ok(Self {
            members: cubesqlbridge::member_to_data_source(&model),
            cubes,
        })
    }

    /// The one data source `members` read from, `None` for the default one.
    ///
    /// A query spanning two data sources is refused, as the Node.js
    /// `CompilerApi` does: no single database can run its SQL.
    fn of_query(&self, members: &[String]) -> Result<Option<String>, PlanFailure> {
        let found: BTreeSet<&str> = members
            .iter()
            .filter_map(|name| {
                let mut path = name.split('.');
                let cube = path.next()?;
                match path.next() {
                    Some(member) => self
                        .members
                        .get(&format!("{cube}.{member}"))
                        .or_else(|| self.cubes.get(cube)),
                    None => self.cubes.get(cube),
                }
            })
            .map(String::as_str)
            .collect();

        let mut found = found.into_iter();
        match (found.next(), found.next()) {
            (None, _) => Ok(None),
            (Some(only), None) if only == cubesqlbridge::DEFAULT_DATA_SOURCE => Ok(None),
            (Some(only), None) => Ok(Some(only.to_string())),
            (Some(first), Some(second)) => {
                let rest: Vec<&str> = found.collect();
                let names = std::iter::once(first)
                    .chain(std::iter::once(second))
                    .chain(rest)
                    .collect::<Vec<_>>()
                    .join(", ");
                Err(PlanFailure {
                    user_error: true,
                    message: format!(
                        "The query reads from more than one data source ({names}); a query can only use cubes of one data source"
                    ),
                })
            }
        }
    }
}

/// Every member and cube name a normalized query mentions: its measures,
/// dimensions, segments, time dimensions, filters (nested `and`/`or`
/// included), member expressions' cubes and join hints.
fn query_members(query: &Value) -> Vec<String> {
    fn push_str(out: &mut Vec<String>, value: Option<&Value>) {
        if let Some(name) = value.and_then(Value::as_str) {
            out.push(name.to_string());
        }
    }

    fn filters(out: &mut Vec<String>, value: &Value) {
        for filter in value.as_array().into_iter().flatten() {
            push_str(out, filter.get("member"));
            push_str(out, filter.get("dimension"));
            for group in ["and", "or"] {
                if let Some(nested) = filter.get(group) {
                    filters(out, nested);
                }
            }
        }
    }

    let mut out = Vec::new();
    for key in ["measures", "dimensions", "segments"] {
        for member in query
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            match member {
                Value::String(name) => out.push(name.clone()),
                // A member expression names the cube it is evaluated in.
                Value::Object(_) => push_str(&mut out, member.get("cubeName")),
                _ => {}
            }
        }
    }
    for time_dimension in query
        .get("timeDimensions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        push_str(&mut out, time_dimension.get("dimension"));
    }
    if let Some(value) = query.get("filters") {
        filters(&mut out, value);
    }
    for hint in query
        .get("joinHints")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match hint {
            Value::String(cube) => out.push(cube.clone()),
            Value::Array(path) => {
                out.extend(path.iter().filter_map(Value::as_str).map(str::to_string))
            }
            _ => {}
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL: &str = r#"
cubes:
  - name: orders
    sql_table: public.orders
    dimensions:
      - name: status
        sql: status
        type: string
    measures:
      - name: count
        type: count

  - name: events
    data_source: warehouse
    sql_table: events
    dimensions:
      - name: kind
        sql: kind
        type: string
      - name: ts
        sql: ts
        type: time
    measures:
      - name: count
        type: count

views:
  - name: activity
    cubes:
      - join_path: events
        includes: [kind, count]
"#;

    fn sources() -> DataSources {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("model.yml"), MODEL).unwrap();
        DataSources::load(dir.path(), &cubemodel::TemplateContext::default()).unwrap()
    }

    fn of(query: Value) -> Result<Option<String>, PlanFailure> {
        sources().of_query(&query_members(&query))
    }

    #[test]
    fn a_cube_without_a_data_source_runs_on_the_default_one() {
        let query = json!({ "measures": ["orders.count"], "dimensions": ["orders.status"] });
        assert_eq!(of(query).unwrap(), None);
    }

    #[test]
    fn a_cube_that_declares_one_runs_there() {
        let query = json!({
            "measures": ["events.count"],
            "timeDimensions": [{ "dimension": "events.ts", "granularity": "day" }],
        });
        assert_eq!(of(query).unwrap().as_deref(), Some("warehouse"));
    }

    #[test]
    fn a_view_member_runs_where_its_cube_does() {
        let query = json!({ "measures": ["activity.count"], "dimensions": ["activity.kind"] });
        assert_eq!(of(query).unwrap().as_deref(), Some("warehouse"));
    }

    #[test]
    fn nested_filters_count() {
        let query = json!({
            "measures": ["orders.count"],
            "filters": [{ "or": [{ "member": "events.kind", "operator": "set" }] }],
        });
        let failure = of(query).unwrap_err();
        assert!(failure.user_error);
        assert!(
            failure.message.contains("default, warehouse"),
            "{}",
            failure.message
        );
    }
}
