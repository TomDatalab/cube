//! The SQL API (Postgres wire protocol) started from the server binary, with
//! the query orchestrator behind it.
//!
//! `cubesqlbridge` implements cubesql's `TransportService` in Rust and takes
//! query execution as an injected trait; this is the implementation that runs
//! those queries through the same orchestrator the REST `/v1/load` uses.

// `cubesql::CubeError` is 128 bytes and is the error type of the traits this
// module implements, so it can be neither boxed nor replaced here.
#![allow(clippy::result_large_err)]

use std::sync::Arc;

use async_trait::async_trait;
use cubeorch::api::{LoadOutcome, OrchestratorApi};
use cubeorch::types::QueryBody;
use cubeplanner::Dialect;
use cubesqlbridge::cubesql::compile::engine::df::scan::{
    transform_response, JsonColumnarValueObject, MemberField, SchemaRef,
};
use cubesqlbridge::cubesql::transport::CubeStreamReceiver;
use cubesqlbridge::cubesql::CubeError;
use cubesqlbridge::{
    LoadResponse, LoadResult, LoadResultAnnotation, LoadResultDataColumnar, ModelSource,
    QueryExecutor, SqlApi, SqlApiConfig, SqlAuthConfig,
};
use serde_json::Value;

use crate::planner_adapter::PlannerQueryService;

/// Runs the SQL API's queries through the orchestrator.
pub struct OrchestratedExecutor {
    planner: Arc<PlannerQueryService>,
    api: Arc<OrchestratorApi>,
    data_source: String,
}

impl std::fmt::Debug for OrchestratedExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrchestratedExecutor")
            .field("data_source", &self.data_source)
            .finish_non_exhaustive()
    }
}

impl OrchestratedExecutor {
    pub fn new(
        planner: Arc<PlannerQueryService>,
        api: Arc<OrchestratorApi>,
        data_source: String,
    ) -> Self {
        Self {
            planner,
            api,
            data_source,
        }
    }
}

#[async_trait]
impl QueryExecutor for OrchestratedExecutor {
    async fn execute(
        &self,
        query: Value,
        security_context: &Value,
    ) -> Result<LoadResponse, CubeError> {
        // The SQL API posts `{request, query, session, ...}`, like the Node
        // bridge did; the query itself is what the orchestrator runs.
        let normalized = query.get("query").cloned().unwrap_or(query);

        let statement = self
            .planner
            .plan_value(&normalized, security_context)
            .await
            .map_err(|failure| CubeError::internal(failure.message))?;

        // The cubes' own data source, or this executor's when they name none.
        let data_source = statement
            .data_source
            .clone()
            .unwrap_or_else(|| self.data_source.clone());

        let body = QueryBody {
            query: Some(statement.sql),
            values: Some(
                statement
                    .params
                    .iter()
                    .cloned()
                    .map(Option::unwrap_or_default)
                    .collect(),
            ),
            data_source: Some(data_source.clone()),
            ..QueryBody::default()
        };

        let outcome = self
            .api
            .execute_query(&body)
            .await
            .map_err(|e| CubeError::internal(e.to_string()))?;

        let LoadOutcome::Result(fetched) = outcome else {
            // The SQL API has no polling contract, so a continue-wait is an
            // error here rather than a body the client retries.
            return Err(CubeError::internal(
                "The query is still being processed, try again".to_string(),
            ));
        };

        Ok(columnar_response(
            &fetched.data,
            &statement.alias_name_to_member,
            fetched.last_refresh_time,
            data_source,
        ))
    }

    /// `stream_mode`: rows travel to cubesql in Arrow batches as the driver
    /// produces them, instead of being buffered into one response.
    async fn execute_stream(
        &self,
        query: Value,
        security_context: &Value,
        schema: SchemaRef,
        member_fields: Vec<MemberField>,
    ) -> Result<CubeStreamReceiver, CubeError> {
        let normalized = query.get("query").cloned().unwrap_or(query);

        let statement = self
            .planner
            .plan_value(&normalized, security_context)
            .await
            .map_err(|failure| CubeError::internal(failure.message))?;

        let body = QueryBody {
            query: Some(statement.sql),
            values: Some(
                statement
                    .params
                    .iter()
                    .cloned()
                    .map(Option::unwrap_or_default)
                    .collect(),
            ),
            data_source: Some(
                statement
                    .data_source
                    .clone()
                    .unwrap_or_else(|| self.data_source.clone()),
            ),
            // A persistent query is answered with a stream rather than rows.
            persistent: true,
            alias_name_to_member: serde_json::to_value(&statement.alias_name_to_member).ok(),
            ..QueryBody::default()
        };

        let LoadOutcome::Stream(stream) = self
            .api
            .execute_query(&body)
            .await
            .map_err(|e| CubeError::internal(e.to_string()))?
        else {
            return Err(CubeError::internal(
                "The orchestrator answered a persistent query with rows".to_string(),
            ));
        };

        // One batch in flight at a time: the orchestrator's own high-water
        // mark is what bounds memory, so a deeper channel here would only
        // buffer twice.
        let (sender, receiver) = tokio::sync::mpsc::channel(1);

        tokio::spawn(async move {
            while let Some(batch) = stream.next_batch().await {
                let message = match batch {
                    Ok(batch) => to_record_batch(batch, schema.clone(), &member_fields),
                    Err(error) => Err(CubeError::internal(error)),
                };

                // A closed receiver means cubesql stopped reading; dropping
                // the stream cancels the query.
                if sender.send(Some(message)).await.is_err() {
                    return;
                }
            }

            // `None` closes the stream for the consumer.
            let _ = sender.send(None).await;
        });

        Ok(receiver)
    }
}

/// Turns one batch of rows into the Arrow batch cubesql's scan expects.
fn to_record_batch(
    batch: cubeorch::QueryStreamBatch,
    schema: SchemaRef,
    member_fields: &[MemberField],
) -> Result<cubesqlbridge::cubesql::compile::engine::df::scan::RecordBatch, CubeError> {
    // The batch is row oriented and the transform reads columns, so pivot it
    // once per batch rather than once per value.
    let columns: Vec<Vec<Value>> = (0..batch.columns.len())
        .map(|index| {
            batch
                .rows
                .iter()
                .map(|row| row.get(index).cloned().unwrap_or(Value::Null))
                .collect()
        })
        .collect();

    let mut response = JsonColumnarValueObject::try_new(batch.columns.as_ref().clone(), columns)?;

    transform_response(&mut response, schema, &member_fields.to_vec())
}

/// Turns the orchestrator's rows into the columnar shape cubesql expects:
/// an ordered member list and one value array per member.
fn columnar_response(
    data: &Value,
    alias_name_to_member: &std::collections::HashMap<String, String>,
    last_refresh_time: Option<i64>,
    data_source: String,
) -> LoadResponse {
    let rows = data.as_array().map(Vec::as_slice).unwrap_or_default();

    // Column order comes from the first row, so the members line up with the
    // arrays below.
    let aliases: Vec<String> = rows
        .first()
        .and_then(Value::as_object)
        .map(|row| row.keys().cloned().collect())
        .unwrap_or_default();

    let members: Vec<String> = aliases
        .iter()
        .map(|alias| {
            alias_name_to_member
                .get(alias)
                .cloned()
                .unwrap_or_else(|| alias.clone())
        })
        .collect();

    let columns: Vec<Vec<Value>> = aliases
        .iter()
        .map(|alias| {
            rows.iter()
                .map(|row| row.get(alias).cloned().unwrap_or(Value::Null))
                .collect()
        })
        .collect();

    let empty = Value::Object(serde_json::Map::new());
    LoadResponse {
        pivot_query: None,
        slow_query: None,
        query_type: None,
        results: vec![LoadResult {
            data_source: Some(data_source),
            annotation: Box::new(LoadResultAnnotation::new(
                empty.clone(),
                empty.clone(),
                empty.clone(),
                empty,
            )),
            data: LoadResultDataColumnar::new(members, columns),
            refresh_key_values: None,
            last_refresh_time: last_refresh_time.map(cubeorch::api::to_iso_string),
            external: None,
            used_pre_aggregations: None,
        }],
    }
}

/// Starts the SQL API when a port is configured, mirroring Node's
/// `CUBEJS_PG_SQL_PORT`.
pub async fn start(
    model_path: &str,
    dialect: Dialect,
    by_data_source: std::collections::HashMap<String, Dialect>,
    planner_threads: usize,
    executor: Arc<dyn QueryExecutor>,
) -> Result<Option<SqlApi>, CubeError> {
    let mut config = SqlApiConfig::from_env(ModelSource::dir(model_path));
    if config.postgres_bind_address.is_none() {
        return Ok(None);
    }

    config.planner_threads = planner_threads;
    // A cube on a data source with its own type renders in that type's
    // dialect; the rest render in the default data source's.
    config.dialects = cubesqlbridge::DialectMap {
        default: dialect,
        by_data_source,
    };
    config.executor = executor;
    config.auth_config = SqlAuthConfig::from_env();

    let api = cubesqlbridge::start_sql_api(config).await?;
    api.spawn_processing_loops().await?;

    Ok(Some(api))
}

/// [`SqlConversionService`] over a running SQL API.
pub struct SqlApiConversionService {
    api: Arc<SqlApi>,
}

impl std::fmt::Debug for SqlApiConversionService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqlApiConversionService")
            .finish_non_exhaustive()
    }
}

impl SqlApiConversionService {
    pub fn new(api: Arc<SqlApi>) -> Self {
        Self { api }
    }
}

#[async_trait]
impl crate::services::SqlConversionService for SqlApiConversionService {
    async fn convert_query(
        &self,
        ctx: &crate::services::RequestContext,
        sql: &str,
    ) -> Result<Value, crate::error::ApiError> {
        let converted =
            cubesqlbridge::rest4sql(self.api.services(), sql, Some(ctx.security_context.clone()))
                .await
                .map_err(conversion_error)?;

        serde_json::to_value(converted).map_err(|_| crate::error::ApiError::internal())
    }

    async fn cubesql(
        &self,
        ctx: &crate::services::RequestContext,
        sql: &str,
    ) -> Result<Value, crate::error::ApiError> {
        let plan =
            cubesqlbridge::sql4sql(self.api.services(), sql, Some(ctx.security_context.clone()))
                .await
                .map_err(conversion_error)?;

        serde_json::to_value(plan).map_err(|_| crate::error::ApiError::internal())
    }
}

/// A statement the user wrote wrongly is a client error.
fn conversion_error(err: CubeError) -> crate::error::ApiError {
    crate::error::ApiError::bad_request(err.to_string())
}
