//! [`QueryService::load`] backed by the `cubeorch` crate: the query is planned
//! by `cubeplanner`, then executed through the orchestrator's cache and queue.
//!
//! This is the Rust replacement for `ApiGateway.load` →
//! `OrchestratorApi.executeQuery`.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use cubeconfig::data_source::key_by_data_source;
use cubeconfig::CubeConfig;
use cubedriver::{Driver, DriverConfig, DriverFactory as DriverBuilder, EnvSource, ProcessEnv};
use cubeorch::api::{LoadOutcome, LoadService, OrchestratorApi, QueryCompilerFn};
use cubeorch::types::QueryBody;
use cubeorch::{OrchError, QueryOrchestrator, QueryOrchestratorOptions};
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::error::ApiError;
use crate::planner_adapter::PlannerQueryService;
use crate::services::{NormalizedRequest, QueryService, RequestContext};

/// The data sources `cube.yml` declares, as the `CUBEJS_*` variables a driver
/// reads, and each one's database type. Set once at start-up.
struct DeclaredDataSources {
    env: HashMap<String, String>,
    db_types: HashMap<String, String>,
    /// From RBAC, which is the only source of data source settings: what it
    /// declares wins over the process environment (a `.env` line left from
    /// before the import, say).
    authoritative: bool,
}

static DECLARED: OnceLock<DeclaredDataSources> = OnceLock::new();

/// Makes the data sources of `cube.yml` reachable by name.
///
/// A driver is configured from `CUBEJS_*` variables, so a data source only
/// `cube.yml` declares (`CUBEJS_DS_<NAME>_*` unset) would otherwise have no
/// host, type or credentials. Each declared field is offered under the
/// variable it stands for; a variable that is set still wins, as it does in
/// `cube.yml` itself.
pub fn declare_data_sources(config: &CubeConfig) {
    let declared = declared(config, HashMap::new(), false);
    // A second call (a test, say) keeps the first; the file is read once.
    let _ = DECLARED.set(declared);
}

/// Like [`declare_data_sources`] for the data sources RBAC holds: `extra` is
/// every stored setting as the variable a driver reads (driver-specific ones
/// included), and the declared values win over the process environment.
pub fn declare_data_sources_from_rbac(config: &CubeConfig, extra: HashMap<String, String>) {
    let _ = DECLARED.set(declared(config, extra, true));
}

fn declared(
    config: &CubeConfig,
    extra: HashMap<String, String>,
    authoritative: bool,
) -> DeclaredDataSources {
    let mut env = extra;
    let mut db_types = HashMap::new();

    let names: Vec<&str> = config.data_sources.keys().map(String::as_str).collect();
    env.insert("CUBEJS_DATASOURCES".to_string(), names.join(","));

    for (name, source) in &config.data_sources {
        let mut set = |var: &str, value: Option<String>| {
            if let Some(value) = value {
                env.insert(key_by_data_source(var, name), value);
            }
        };
        set("CUBEJS_DB_TYPE", Some(source.db_type.clone()));
        set("CUBEJS_DB_URL", source.url.clone());
        set("CUBEJS_DB_HOST", source.host.clone());
        set("CUBEJS_DB_PORT", source.port.map(|port| port.to_string()));
        set("CUBEJS_DB_NAME", source.database.clone());
        set("CUBEJS_DB_SCHEMA", source.schema.clone());
        set("CUBEJS_DB_USER", source.user.clone());
        set("CUBEJS_DB_PASS", source.password.clone());
        set("CUBEJS_DB_SSL", source.ssl.then(|| "true".to_string()));
        set("CUBEJS_DB_MAX_POOL", source.max_pool.map(|n| n.to_string()));
        set("CUBEJS_DB_EXPORT_BUCKET", source.export_bucket.clone());
        set(
            "CUBEJS_DB_EXPORT_BUCKET_TYPE",
            source.export_bucket_type.clone(),
        );

        if !source.db_type.trim().is_empty() {
            db_types.insert(name.clone(), source.db_type.clone());
        }
    }

    DeclaredDataSources {
        env,
        db_types,
        authoritative,
    }
}

/// The database type `cube.yml` declares for `data_source`.
fn declared_db_type(data_source: &str) -> Option<String> {
    DECLARED.get()?.db_types.get(data_source).cloned()
}

/// The process environment, falling back to what `cube.yml` declares.
struct DeclaredEnv;

impl EnvSource for DeclaredEnv {
    fn get(&self, key: &str) -> Option<String> {
        declared_lookup(DECLARED.get(), key, |k| ProcessEnv.get(k))
    }
}

fn declared_lookup(
    declared: Option<&DeclaredDataSources>,
    key: &str,
    process: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    let value = declared.and_then(|d| d.env.get(key).cloned());
    // The declared list is already the union of the file and
    // `CUBEJS_DATASOURCES`, so it wins over the variable alone; with RBAC
    // every declared value does.
    let wins = key == "CUBEJS_DATASOURCES" || declared.is_some_and(|d| d.authoritative);
    if wins && value.is_some() {
        return value;
    }
    process(key).or(value)
}

/// Builds a driver per data source name, as `driverFactory` did in `cube.js`.
pub fn driver_factory() -> cubeorch::DriverFactory {
    Arc::new(|data_source: String| {
        Box::pin(async move {
            let source = if data_source == "default" {
                None
            } else {
                Some(data_source.as_str())
            };

            let config = DriverConfig::from_env_source(&DeclaredEnv, source, false)
                .map_err(|e| OrchError::Driver(e.to_string()))?;
            let db_type = config.data_source.db_type.clone().ok_or_else(|| {
                OrchError::Driver(format!(
                    "CUBEJS_DB_TYPE is not set for the {data_source} data source"
                ))
            })?;

            let driver: Arc<dyn Driver> = DriverBuilder::create(&db_type, config)
                .map_err(|e| OrchError::Driver(e.to_string()))?;

            Ok(driver)
        })
    })
}

/// Serves `/v1/load` (orchestrated) and delegates `/v1/sql` and
/// `/v1/dry-run` to the planner, which needs no orchestrator.
pub struct OrchestratedQueryService {
    planner: Arc<PlannerQueryService>,
    data_source: String,
    load: LoadService,
}

impl std::fmt::Debug for OrchestratedQueryService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrchestratedQueryService")
            .finish_non_exhaustive()
    }
}

impl OrchestratedQueryService {
    /// The orchestrator entry point, shared with the SQL API so both run
    /// through one cache and one queue.
    pub fn api(&self) -> &Arc<cubeorch::api::OrchestratorApi> {
        self.load.api()
    }

    pub fn planner(&self) -> &Arc<PlannerQueryService> {
        &self.planner
    }

    pub fn new(
        planner: Arc<PlannerQueryService>,
        orchestrator: Arc<QueryOrchestrator>,
        data_source: String,
        db_type: Option<String>,
        dev_mode: bool,
    ) -> Self {
        let data_source_for_compiler = data_source.clone();
        // `dbType` is per data source, `extDbType` is the external store.
        let own_data_source = data_source.clone();
        let db_type_fn: Option<cubeorch::api::DbTypeFn> = Some(Arc::new(move |name: &str| {
            if name == own_data_source {
                db_type.clone().or_else(|| declared_db_type(name))
            } else {
                declared_db_type(name)
            }
        }));
        let api = OrchestratorApi::new(orchestrator).with_db_types(db_type_fn, None);
        // `LoadService`'s own compiler is only used by callers that do not
        // hold a plan already; `load` below plans once and runs the body
        // directly, so the query is never planned twice per request.
        let compiler = Self::compiler(planner.clone(), data_source_for_compiler);

        Self {
            planner,
            data_source,
            load: LoadService::new(api, compiler, dev_mode),
        }
    }

    /// Builds the orchestrator's `QueryBody` by planning the normalized query.
    ///
    /// Refresh-key queries and pre-aggregation descriptions are not produced
    /// yet: `cubeplanner::plan` returns the statement and its parameters, so
    /// a result is cached by its SQL rather than invalidated by a refresh key.
    fn compiler(planner: Arc<PlannerQueryService>, data_source: String) -> QueryCompilerFn {
        Arc::new(move |normalized_query: Value, security_context: Value| {
            let planner = planner.clone();
            let data_source = data_source.clone();

            Box::pin(async move {
                let statement = planner
                    .plan_value(&normalized_query, &security_context)
                    .await
                    .map_err(|failure| {
                        if failure.user_error {
                            OrchError::Execution(failure.message)
                        } else {
                            OrchError::Orchestration(failure.message)
                        }
                    })?;

                Ok(QueryBody {
                    query: Some(statement.sql),
                    // A `NULL` parameter is sent as an empty string, like the
                    // driver layer does for a bound `null`.
                    values: Some(
                        statement
                            .params
                            .into_iter()
                            .map(Option::unwrap_or_default)
                            .collect(),
                    ),
                    data_source: Some(statement.data_source.unwrap_or(data_source)),
                    ..QueryBody::default()
                })
            })
        })
    }
}

#[async_trait]
impl QueryService for OrchestratedQueryService {
    fn data_sources(&self) -> Option<Arc<crate::planner_adapter::DataSources>> {
        Some(self.planner.data_sources())
    }

    async fn load(
        &self,
        ctx: &RequestContext,
        request: NormalizedRequest,
    ) -> Result<Value, ApiError> {
        let normalized = request
            .queries
            .first()
            .ok_or_else(|| ApiError::bad_request("Query param is required"))?;
        let query = serde_json::to_value(normalized).map_err(|_| ApiError::internal())?;

        // One plan gives both the statement to run and the alias map the
        // response needs, like a single `getSql` call in Node.js. It is the
        // plan the request's join check runs on.
        let statement = self.planner.plan_query(ctx, &request, 0).await?;

        let body = QueryBody {
            query: Some(statement.sql),
            // A `NULL` parameter is sent as an empty string, like the driver
            // layer does for a bound `null`.
            values: Some(
                statement
                    .params
                    .into_iter()
                    .map(Option::unwrap_or_default)
                    .collect(),
            ),
            data_source: Some(
                statement
                    .data_source
                    .clone()
                    .unwrap_or_else(|| self.data_source.clone()),
            ),
            request_id: Some(ctx.request_id.clone()),
            ..QueryBody::default()
        };

        let outcome = self
            .load
            .api()
            .execute_query(&body)
            .await
            .map_err(orch_error)?;

        let mut result = cubeorch::api::LoadResult { query, outcome };
        rename_result_keys(&mut result, &statement.alias_name_to_member);

        // `Continue wait` is a 200 with an error body, which is how clients
        // know to poll (`gateway.ts:2577-2586`).
        if let LoadOutcome::ContinueWait { .. } = result.outcome {
            return Ok(self.load.prepare_result_transform_data(
                &result,
                Value::Null,
                Some(&ctx.request_id),
            ));
        }

        Ok(self.load.prepare_result_transform_data(
            &result,
            json!({ "measures": {}, "dimensions": {}, "segments": {}, "timeDimensions": {} }),
            Some(&ctx.request_id),
        ))
    }

    /// `load` plans through [`PlannerQueryService::plan_query`]; `sql` and
    /// `dry_run` are the planner's.
    fn checks_joins(&self) -> bool {
        true
    }

    async fn sql(
        &self,
        ctx: &RequestContext,
        request: NormalizedRequest,
    ) -> Result<Value, ApiError> {
        self.planner.sql(ctx, request).await
    }

    async fn dry_run(
        &self,
        ctx: &RequestContext,
        request: NormalizedRequest,
    ) -> Result<Value, ApiError> {
        self.planner.dry_run(ctx, request).await
    }
}

fn orch_error(err: OrchError) -> ApiError {
    match err {
        OrchError::Execution(message) => ApiError::bad_request(message),
        OrchError::NotImplemented(message) => ApiError::not_implemented(message),
        other => ApiError::new(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            other.to_string(),
        ),
    }
}

/// Builds the orchestrator from the environment.
pub fn orchestrator(cache_prefix: &str) -> Arc<QueryOrchestrator> {
    QueryOrchestrator::new(
        cache_prefix,
        driver_factory(),
        None,
        Arc::new(|event: &str, payload: Value| {
            tracing::info!(event, %payload, "orchestrator");
        }),
        QueryOrchestratorOptions::default(),
    )
}

/// Rewrites each row's keys from the SQL aliases to the member names, the way
/// `transformData` does in the Node.js gateway. An alias with no mapping is
/// left as it is rather than dropped.
fn rename_result_keys(result: &mut cubeorch::api::LoadResult, aliases: &HashMap<String, String>) {
    if aliases.is_empty() {
        return;
    }

    let LoadOutcome::Result(fetched) = &mut result.outcome else {
        return;
    };

    let Some(rows) = fetched.data.as_array_mut() else {
        return;
    };

    for row in rows {
        let Some(object) = row.as_object_mut() else {
            continue;
        };

        let renamed = object
            .iter()
            .map(|(alias, value)| {
                let member = aliases.get(alias).cloned().unwrap_or_else(|| alias.clone());
                (member, value.clone())
            })
            .collect();

        *object = renamed;
    }
}

#[cfg(test)]
mod declared_lookup_tests {
    use super::*;

    fn declared(authoritative: bool) -> DeclaredDataSources {
        DeclaredDataSources {
            env: [
                ("CUBEJS_DB_PASS".to_string(), "from-rbac".to_string()),
                ("CUBEJS_DATASOURCES".to_string(), "default".to_string()),
            ]
            .into(),
            db_types: HashMap::new(),
            authoritative,
        }
    }

    fn process(key: &str) -> Option<String> {
        match key {
            "CUBEJS_DB_PASS" => Some("from-dotenv".into()),
            "CUBEJS_DB_QUERY_TIMEOUT" => Some("30".into()),
            "CUBEJS_DATASOURCES" => Some("default,old".into()),
            _ => None,
        }
    }

    #[test]
    fn with_rbac_the_stored_value_wins_over_the_environment() {
        let d = declared(true);
        assert_eq!(declared_lookup(Some(&d), "CUBEJS_DB_PASS", process).as_deref(), Some("from-rbac"));
        // What RBAC does not hold still comes from the environment.
        assert_eq!(declared_lookup(Some(&d), "CUBEJS_DB_QUERY_TIMEOUT", process).as_deref(), Some("30"));
        assert_eq!(declared_lookup(Some(&d), "CUBEJS_DATASOURCES", process).as_deref(), Some("default"));
    }

    #[test]
    fn without_rbac_a_set_variable_wins_as_in_cube_yml() {
        let d = declared(false);
        assert_eq!(declared_lookup(Some(&d), "CUBEJS_DB_PASS", process).as_deref(), Some("from-dotenv"));
        assert_eq!(declared_lookup(Some(&d), "CUBEJS_DATASOURCES", process).as_deref(), Some("default"));
    }
}
