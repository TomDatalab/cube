//! The orchestrator's HTTP facing edge.
//!
//! Port of `SC/OrchestratorApi.executeQuery` (the "Continue wait" contract of spec §5) and
//! of the `/v1/load` response shaping of `ApiGateway.prepareResultTransformData`
//! (`GW/gateway.ts:2050-2076`, `:142-195`).

use std::{sync::Arc, time::Duration};

use cubequeue::{QueryStage, QueryStream};
use serde_json::{json, Map, Value};

use crate::{
    error::OrchError,
    orchestrator::{FetchQueryOutcome, FetchQueryResult, QueryOrchestrator},
    types::{CacheMode, QueryBody},
};

/// Resolves `contextToDbType(dataSource)`.
pub type DbTypeFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// What `executeQuery` produces: either a result or the continue-wait answer, which the
/// gateway returns as HTTP 200 with `{ error: "Continue wait" }`.
#[derive(Clone, Debug, PartialEq)]
pub enum LoadOutcome {
    Result(Box<FetchQueryResult>),
    /// A persistent query: its rows travel on the stream instead of in a response body.
    /// `/v1/load` never asks for one, the SQL API's `stream_mode` does.
    Stream(QueryStream),
    BuildOnly(Value),
    Job(Vec<Value>),
    /// `{ error: 'Continue wait', stage }`. `stage` is `None` for a scheduled refresh,
    /// where the Node code answers `stage: null` without asking the queue.
    ContinueWait {
        stage: Option<QueryStage>,
    },
}

/// `OrchestratorApi` (`SC/OrchestratorApi.ts`).
pub struct OrchestratorApi {
    orchestrator: Arc<QueryOrchestrator>,
    continue_wait_timeout: u64,
    db_type: Option<DbTypeFn>,
    ext_db_type: Option<String>,
}

impl std::fmt::Debug for OrchestratorApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrchestratorApi")
            .field("continue_wait_timeout", &self.continue_wait_timeout)
            .field("ext_db_type", &self.ext_db_type)
            .finish_non_exhaustive()
    }
}

impl OrchestratorApi {
    /// `continueWaitTimeout` defaults to 10 s and is validated to `0..90`
    /// (`SC/optionsValidate.ts:16,125`).
    pub const DEFAULT_CONTINUE_WAIT_TIMEOUT: u64 = 10;

    pub fn new(orchestrator: Arc<QueryOrchestrator>) -> Arc<Self> {
        Arc::new(Self {
            orchestrator,
            continue_wait_timeout: Self::DEFAULT_CONTINUE_WAIT_TIMEOUT,
            db_type: None,
            ext_db_type: None,
        })
    }

    #[must_use]
    pub fn with_continue_wait_timeout(mut self: Arc<Self>, seconds: u64) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("configure the api before sharing it")
            .continue_wait_timeout = seconds.min(90);
        self
    }

    #[must_use]
    pub fn with_db_types(
        mut self: Arc<Self>,
        db_type: Option<DbTypeFn>,
        ext_db_type: Option<String>,
    ) -> Arc<Self> {
        let api = Arc::get_mut(&mut self).expect("configure the api before sharing it");
        api.db_type = db_type;
        api.ext_db_type = ext_db_type;
        self
    }

    pub fn orchestrator(&self) -> &Arc<QueryOrchestrator> {
        &self.orchestrator
    }

    pub fn continue_wait_timeout(&self) -> u64 {
        self.continue_wait_timeout
    }

    /// `OrchestratorApi.executeQuery(query)` (`SC/OrchestratorApi.ts:76-170`).
    ///
    /// The whole `fetchQuery` runs under `continueWaitTimeout`; both that timeout and a
    /// `ContinueWaitError` raised inside become the same answer, so a client that re-issues
    /// the identical request converges on the result rather than restarting the work.
    pub async fn execute_query(&self, query_body: &QueryBody) -> Result<LoadOutcome, OrchError> {
        if query_body.load_refresh_keys_only {
            let values = self.orchestrator.load_refresh_keys(query_body).await?;

            return Ok(LoadOutcome::BuildOnly(Value::Array(values)));
        }

        // A job never blocks: it is answered as soon as the build is queued.
        if query_body.is_job {
            return match self.orchestrator.fetch_query(query_body).await? {
                FetchQueryOutcome::Job(jobs) => Ok(LoadOutcome::Job(jobs)),
                FetchQueryOutcome::Result(result) => Ok(LoadOutcome::Result(result)),
                FetchQueryOutcome::Stream(stream) => Ok(LoadOutcome::Stream(stream)),
                FetchQueryOutcome::BuildOnly {
                    used_pre_aggregations,
                    last_refresh_time,
                } => Ok(LoadOutcome::BuildOnly(json!({
                    "usedPreAggregations": used_pre_aggregations,
                    "lastRefreshTime": last_refresh_time.map(to_iso_string),
                }))),
            };
        }

        let outcome = tokio::time::timeout(
            Duration::from_secs(self.continue_wait_timeout),
            self.orchestrator.fetch_query(query_body),
        )
        .await;

        let error = match outcome {
            Ok(Ok(FetchQueryOutcome::Result(mut result))) => {
                result.db_type = self.db_type_of(result.data_source.as_deref());
                result.ext_db_type = self.ext_db_type.clone();

                return Ok(LoadOutcome::Result(result));
            }
            Ok(Ok(FetchQueryOutcome::Job(jobs))) => return Ok(LoadOutcome::Job(jobs)),
            Ok(Ok(FetchQueryOutcome::Stream(stream))) => return Ok(LoadOutcome::Stream(stream)),
            Ok(Ok(FetchQueryOutcome::BuildOnly {
                used_pre_aggregations,
                last_refresh_time,
            })) => {
                return Ok(LoadOutcome::BuildOnly(json!({
                    "usedPreAggregations": used_pre_aggregations,
                    "lastRefreshTime": last_refresh_time.map(to_iso_string),
                })))
            }
            Ok(Err(error)) => error,
            // The outer timeout is converted exactly like a `ContinueWaitError`.
            Err(_elapsed) => OrchError::ContinueWait,
        };

        if !error.is_continue_wait() {
            return Err(error);
        }

        // A scheduled refresh has no client to report a stage to.
        if query_body.scheduled_refresh {
            return Ok(LoadOutcome::ContinueWait { stage: None });
        }

        let from_cache = self
            .orchestrator
            .result_from_cache_if_exists(query_body)
            .await?;

        // The stale cache modes answer with what is there rather than making the user wait.
        if matches!(
            query_body.cache_mode,
            Some(CacheMode::StaleIfSlow) | Some(CacheMode::StaleWhileRevalidate)
        ) {
            if let Some(mut cached) = from_cache {
                cached.slow_query = true;
                cached.db_type = self.db_type_of(cached.data_source.as_deref());
                cached.ext_db_type = self.ext_db_type.clone();

                return Ok(LoadOutcome::Result(Box::new(cached)));
            }
        }

        Ok(LoadOutcome::ContinueWait {
            stage: self.orchestrator.query_stage(query_body).await?,
        })
    }

    fn db_type_of(&self, data_source: Option<&str>) -> Option<String> {
        self.db_type
            .as_ref()
            .and_then(|db_type| db_type(data_source.unwrap_or("default")))
    }
}

/// Compiles a normalized query into the SQL query body the orchestrator runs.
///
/// The compilation itself belongs to the data model workstream, so it is injected here.
pub type QueryCompilerFn = Arc<
    dyn Fn(Value, Value) -> futures::future::BoxFuture<'static, Result<QueryBody, OrchError>>
        + Send
        + Sync,
>;

/// What [`LoadService::load`] hands back: the normalized query it ran and the outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadResult {
    pub query: Value,
    pub outcome: LoadOutcome,
}

/// The `/v1/load` entry point the HTTP server calls.
pub struct LoadService {
    api: Arc<OrchestratorApi>,
    compiler: QueryCompilerFn,
    /// `getEnv('devMode') || context.signedWithPlaygroundAuthSecret`.
    dev_mode: bool,
}

impl std::fmt::Debug for LoadService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadService")
            .field("dev_mode", &self.dev_mode)
            .finish_non_exhaustive()
    }
}

impl LoadService {
    pub fn new(api: Arc<OrchestratorApi>, compiler: QueryCompilerFn, dev_mode: bool) -> Self {
        Self {
            api,
            compiler,
            dev_mode,
        }
    }

    pub fn api(&self) -> &Arc<OrchestratorApi> {
        &self.api
    }

    /// Compiles `normalized_query` under `security_context` and runs it.
    pub async fn load(
        &self,
        normalized_query: &Value,
        security_context: &Value,
    ) -> Result<LoadResult, OrchError> {
        let query_body =
            (self.compiler)(normalized_query.clone(), security_context.clone()).await?;
        let outcome = self.api.execute_query(&query_body).await?;

        Ok(LoadResult {
            query: normalized_query.clone(),
            outcome,
        })
    }

    /// `ApiGateway.prepareResultTransformData` (`GW/gateway.ts:2050-2076`) — the root object
    /// of a `/v1/load` result, without the `data`/`annotation` transform, which the result
    /// wrapper fills in.
    pub fn prepare_result_transform_data(
        &self,
        result: &LoadResult,
        annotation: Value,
        request_id: Option<&str>,
    ) -> Value {
        match &result.outcome {
            // The gateway answers HTTP 200 with this body and the client re-issues the
            // identical request (`GW/gateway.ts:2577-2586`).
            LoadOutcome::ContinueWait { stage } => {
                let mut body = Map::new();
                body.insert("error".to_string(), Value::String("Continue wait".into()));
                if let Some(stage) = stage {
                    body.insert(
                        "stage".to_string(),
                        serde_json::to_value(stage).unwrap_or(Value::Null),
                    );
                }
                if let Some(request_id) = request_id {
                    body.insert("requestId".to_string(), Value::String(request_id.into()));
                }

                Value::Object(body)
            }
            LoadOutcome::Job(jobs) => Value::Array(jobs.clone()),
            // A streamed result has no body to shape: the caller reads the stream itself.
            LoadOutcome::Stream(_) => {
                let mut body = Map::new();

                body.insert("query".to_string(), result.query.clone());

                if let Some(request_id) = request_id {
                    body.insert("requestId".to_string(), Value::String(request_id.into()));
                }

                Value::Object(body)
            }
            LoadOutcome::BuildOnly(value) => value.clone(),
            LoadOutcome::Result(response) => {
                let mut body = Map::new();

                body.insert("query".to_string(), result.query.clone());
                body.insert(
                    "lastRefreshTime".to_string(),
                    response
                        .last_refresh_time
                        .map(to_iso_string)
                        .map(Value::String)
                        .unwrap_or(Value::Null),
                );

                // Identity of the pre-aggregations behind this result, so a client can join
                // it to the build it is waiting on. Dev mode replaces it with the full object.
                if let Some(used) = public_used_pre_aggregations(response) {
                    body.insert("usedPreAggregations".to_string(), used);
                }

                if self.dev_mode {
                    body.insert(
                        "refreshKeyValues".to_string(),
                        response
                            .refresh_key_values
                            .clone()
                            .map(Value::Array)
                            .unwrap_or(Value::Null),
                    );

                    if let Some(used) = non_empty_used_pre_aggregations(response) {
                        body.insert("usedPreAggregations".to_string(), used);
                    }

                    if let Some(request_id) = request_id {
                        body.insert("requestId".to_string(), Value::String(request_id.into()));
                    }
                }

                body.insert("annotation".to_string(), annotation);
                body.insert(
                    "dataSource".to_string(),
                    option_string(response.data_source.clone()),
                );
                body.insert(
                    "dbType".to_string(),
                    option_string(response.db_type.clone()),
                );
                body.insert(
                    "extDbType".to_string(),
                    option_string(response.ext_db_type.clone()),
                );
                body.insert(
                    "external".to_string(),
                    response.external.map(Value::Bool).unwrap_or(Value::Null),
                );
                body.insert("slowQuery".to_string(), Value::Bool(response.slow_query));
                body.insert(
                    "total".to_string(),
                    response
                        .total
                        .map(|total| Value::Number(total.into()))
                        .unwrap_or(Value::Null),
                );
                body.insert("data".to_string(), response.data.clone());

                Value::Object(body)
            }
        }
    }
}

fn option_string(value: Option<String>) -> Value {
    value.map(Value::String).unwrap_or(Value::Null)
}

/// Epoch milliseconds as the ISO string `lastRefreshTime` is reported with.
pub fn to_iso_string(timestamp_ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(timestamp_ms)
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// A query that hit no pre-aggregation reports nothing rather than an empty object, so the
/// key is simply absent from the response (`GW/gateway.ts:146-157`).
fn non_empty_used_pre_aggregations(response: &FetchQueryResult) -> Option<Value> {
    if response.used_pre_aggregations.is_empty() {
        return None;
    }

    serde_json::to_value(&response.used_pre_aggregations).ok()
}

/// The fields of `usedPreAggregations` that are safe to report to any client
/// (`GW/gateway.ts:159-195`).
///
/// `refreshKeyValues` is left out: those are raw rows of the refresh key queries, and a
/// `refreshKey.sql` is often written without the security context filtering the cube
/// applies, so the values can describe data the caller cannot otherwise reach.
/// `targetTableName` is left out too — it names one physical build down to its hashes, and
/// `preAggregationId` plus the entry key already identify the pre-aggregation.
fn public_used_pre_aggregations(response: &FetchQueryResult) -> Option<Value> {
    if response.used_pre_aggregations.is_empty() {
        return None;
    }

    let redacted: Map<String, Value> = response
        .used_pre_aggregations
        .iter()
        .map(|(table_name, usage)| {
            let mut fields = Map::new();

            // Absent fields are dropped rather than emitted as `null`.
            if let Some(pre_aggregation_id) = &usage.pre_aggregation_id {
                fields.insert(
                    "preAggregationId".to_string(),
                    Value::String(pre_aggregation_id.clone()),
                );
            }
            if let Some(last_updated_at) = usage.last_updated_at {
                fields.insert("lastUpdatedAt".to_string(), json!(last_updated_at));
            }
            if let Some(r#type) = &usage.r#type {
                fields.insert("type".to_string(), Value::String(r#type.clone()));
            }

            (table_name.clone(), Value::Object(fields))
        })
        .collect();

    Some(Value::Object(redacted))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::types::UsedPreAggregation;

    fn response() -> FetchQueryResult {
        let mut used = BTreeMap::new();
        used.insert(
            "stb_pre_aggregations.orders_main".to_string(),
            UsedPreAggregation {
                target_table_name: Some("stb_pre_aggregations.orders_main_abc_def_1fm6652".into()),
                refresh_key_values: Some(vec![json!([{ "max": "2020-01-01" }])]),
                last_updated_at: Some(1600329890000),
                pre_aggregation_id: Some("Orders.main".into()),
                r#type: Some("rollup".into()),
            },
        );

        FetchQueryResult {
            data: json!([{ "count": 1 }]),
            refresh_key_values: Some(vec![json!([{ "max": "2020-01-01" }])]),
            last_refresh_time: Some(1600329890000),
            data_source: Some("default".into()),
            external: Some(false),
            used_pre_aggregations: used,
            db_type: Some("postgres".into()),
            ext_db_type: Some("cubestore".into()),
            slow_query: false,
            total: None,
        }
    }

    fn service(dev_mode: bool) -> LoadService {
        let compiler: QueryCompilerFn = Arc::new(|_, _| {
            Box::pin(async { Err(OrchError::orchestration("not used in this test")) })
        });

        LoadService {
            api: OrchestratorApi::new(QueryOrchestrator::from_parts(
                crate::cache::QueryCache::builder(
                    "test",
                    Arc::new(|_| Box::pin(async { Err(OrchError::orchestration("no driver")) })),
                )
                .build(),
                crate::preaggs::PreAggregations::new(
                    "test",
                    Arc::new(|_| Box::pin(async { Err(OrchError::orchestration("no driver")) })),
                    None,
                    Arc::new(|_, _| {}),
                    crate::cache::QueryCache::builder(
                        "test",
                        Arc::new(|_| {
                            Box::pin(async { Err(OrchError::orchestration("no driver")) })
                        }),
                    )
                    .build(),
                    Default::default(),
                ),
                false,
            )),
            compiler,
            dev_mode,
        }
    }

    fn body(dev_mode: bool) -> Value {
        service(dev_mode).prepare_result_transform_data(
            &LoadResult {
                query: json!({ "measures": ["Orders.count"] }),
                outcome: LoadOutcome::Result(Box::new(response())),
            },
            json!({ "measures": {} }),
            Some("req-1"),
        )
    }

    #[test]
    fn redacts_used_pre_aggregations_outside_dev_mode() {
        let body = body(false);

        assert_eq!(
            body["usedPreAggregations"],
            json!({
                "stb_pre_aggregations.orders_main": {
                    "preAggregationId": "Orders.main",
                    "lastUpdatedAt": 1600329890000i64,
                    "type": "rollup",
                }
            })
        );
        assert!(body.get("refreshKeyValues").is_none());
        assert!(body.get("requestId").is_none());
        assert_eq!(body["lastRefreshTime"], json!("2020-09-17T08:04:50.000Z"));
    }

    #[test]
    fn dev_mode_reports_the_full_object() {
        let body = body(true);

        assert_eq!(
            body["usedPreAggregations"]["stb_pre_aggregations.orders_main"]["targetTableName"],
            json!("stb_pre_aggregations.orders_main_abc_def_1fm6652")
        );
        assert_eq!(body["refreshKeyValues"], json!([[{ "max": "2020-01-01" }]]));
        assert_eq!(body["requestId"], json!("req-1"));
    }

    #[test]
    fn a_query_without_pre_aggregations_omits_the_key() {
        let mut response = response();
        response.used_pre_aggregations.clear();

        let body = service(true).prepare_result_transform_data(
            &LoadResult {
                query: json!({}),
                outcome: LoadOutcome::Result(Box::new(response)),
            },
            json!({}),
            Some("req-1"),
        );

        assert!(body.get("usedPreAggregations").is_none());
    }

    #[test]
    fn continue_wait_body_carries_the_stage() {
        let body = service(false).prepare_result_transform_data(
            &LoadResult {
                query: json!({}),
                outcome: LoadOutcome::ContinueWait {
                    stage: Some(QueryStage::in_queue(2)),
                },
            },
            json!({}),
            Some("req-1"),
        );

        assert_eq!(
            body,
            json!({ "error": "Continue wait", "stage": { "stage": "#3 in queue" }, "requestId": "req-1" })
        );
    }

    #[test]
    fn scheduled_refresh_continue_wait_has_no_stage() {
        let body = service(false).prepare_result_transform_data(
            &LoadResult {
                query: json!({}),
                outcome: LoadOutcome::ContinueWait { stage: None },
            },
            json!({}),
            None,
        );

        assert_eq!(body, json!({ "error": "Continue wait" }));
    }

    #[test]
    fn iso_strings() {
        assert_eq!(to_iso_string(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(to_iso_string(1600329890789), "2020-09-17T08:04:50.789Z");
    }
}
