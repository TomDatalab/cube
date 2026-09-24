//! Service traits the HTTP layer depends on.
//!
//! Each trait maps to a part of the Node.js backend that is being ported:
//! implementations live in `cubeauth`, `cubemodel`, `cubequery`,
//! `cubedriver` and the orchestrator crates. Handlers never talk to those
//! crates directly, so a surface can move to Rust without touching routing.

use std::fmt::Debug;
use std::sync::Arc;

use async_trait::async_trait;
use cubequery::types::{NormalizedQuery, PivotQuery, QueryType};
use serde_json::{Map, Value};

use crate::error::ApiError;

/// Result of authenticating one request (`req.context` in Node.js).
#[derive(Debug, Clone, Default)]
pub struct RequestContext {
    pub request_id: String,
    pub security_context: Value,
    pub signed_with_playground_auth_secret: bool,
}

#[async_trait]
pub trait AuthService: Send + Sync + Debug {
    /// `checkAuth` middleware: verifies the `Authorization` header (or its
    /// absence) and returns the security context.
    async fn authenticate(
        &self,
        authorization: Option<&str>,
    ) -> Result<AuthenticatedRequest, ApiError>;

    /// `contextToApiScopes`: API scopes granted to a security context.
    async fn api_scopes(&self, security_context: &Value) -> Result<Vec<String>, ApiError>;

    /// Signs a token the Playground can call the REST API with, the way the
    /// Node.js dev server mints `cubejsToken`.
    ///
    /// Only the Playground needs this, so the default refuses: an
    /// authenticator that cannot sign should not silently hand out a token.
    async fn issue_token(&self, _claims: Map<String, Value>) -> Result<String, ApiError> {
        Err(ApiError::not_implemented(
            "This authenticator cannot issue tokens",
        ))
    }
}

#[derive(Debug, Clone, Default)]
pub struct AuthenticatedRequest {
    pub security_context: Value,
    pub signed_with_playground_auth_secret: bool,
}

/// `GET /v1/meta` provider (`ApiGateway.meta` / `metaExtended`).
#[async_trait]
pub trait MetaService: Send + Sync + Debug {
    async fn meta(&self, ctx: &RequestContext, only_views: bool) -> Result<Value, ApiError>;

    async fn meta_extended(
        &self,
        _ctx: &RequestContext,
        _only_views: bool,
    ) -> Result<Value, ApiError> {
        Err(ApiError::not_implemented(
            "/v1/meta?extended is not implemented yet",
        ))
    }
}

/// `/readyz` and `/livez` probes.
#[async_trait]
pub trait HealthService: Send + Sync + Debug {
    /// Readiness: data source + orchestrator connections in standalone mode.
    async fn readiness(&self) -> Result<(), String>;
    /// Liveness: connections of all known data sources.
    async fn liveness(&self) -> Result<(), String>;
}

/// One request to `/v1/load`, `/v1/sql` or `/v1/dry-run` after parsing and
/// normalization: what `getNormalizedQueries` returns in the Node.js gateway.
#[derive(Debug, Clone)]
pub struct NormalizedRequest {
    pub query_type: QueryType,
    pub queries: Vec<NormalizedQuery>,
    /// `getPivotQuery` of the request, used to shape the response.
    pub pivot_query: PivotQuery,
}

/// Query endpoints (`/v1/load`, `/v1/sql`, `/v1/dry-run`). Implemented once
/// the planner and orchestrator are wired; the default answers 501 with the
/// same body shape as the Node.js gateway errors.
#[async_trait]
pub trait QueryService: Send + Sync + Debug {
    async fn load(
        &self,
        _ctx: &RequestContext,
        _request: NormalizedRequest,
    ) -> Result<Value, ApiError> {
        Err(ApiError::not_implemented("/v1/load is not implemented yet"))
    }

    async fn sql(
        &self,
        _ctx: &RequestContext,
        _request: NormalizedRequest,
    ) -> Result<Value, ApiError> {
        Err(ApiError::not_implemented("/v1/sql is not implemented yet"))
    }

    async fn dry_run(
        &self,
        _ctx: &RequestContext,
        _request: NormalizedRequest,
    ) -> Result<Value, ApiError> {
        Err(ApiError::not_implemented(
            "/v1/dry-run is not implemented yet",
        ))
    }

    /// `DELETE /v1/running-query/:requestId` — cancels the running query of a
    /// request and reports whether anything was cancelled.
    async fn cancel_query(
        &self,
        _ctx: &RequestContext,
        _request_id: &str,
    ) -> Result<bool, ApiError> {
        Err(ApiError::not_implemented(
            "/v1/running-query is not implemented yet",
        ))
    }
}

/// Pre-aggregation management: `/v1/pre-aggregations/*` and the
/// `/cube-system/v1/pre-aggregations/*` routes of the Node.js gateway.
///
/// Every method defaults to 501 so the routes exist with their real contract
/// while the orchestrator is being ported.
#[async_trait]
pub trait PreAggregationService: Send + Sync + Debug {
    /// `POST /v1/pre-aggregations/can-use` (Rollup Designer).
    async fn can_use(
        &self,
        _ctx: &RequestContext,
        _transformed_query: Value,
        _references: Value,
    ) -> Result<Value, ApiError> {
        Err(ApiError::not_implemented(
            "/v1/pre-aggregations/can-use is not implemented yet",
        ))
    }

    /// `POST /v1/pre-aggregations/jobs`.
    async fn jobs(&self, _ctx: &RequestContext, _body: Value) -> Result<Value, ApiError> {
        Err(ApiError::not_implemented(
            "/v1/pre-aggregations/jobs is not implemented yet",
        ))
    }

    /// `GET /cube-system/v1/pre-aggregations`.
    async fn list(
        &self,
        _ctx: &RequestContext,
        _cache_only: bool,
        _meta_only: bool,
    ) -> Result<Value, ApiError> {
        Err(ApiError::not_implemented(
            "/cube-system/v1/pre-aggregations is not implemented yet",
        ))
    }

    /// `POST /cube-system/v1/pre-aggregations/partitions`.
    async fn partitions(&self, _ctx: &RequestContext, _query: Value) -> Result<Value, ApiError> {
        Err(ApiError::not_implemented(
            "/cube-system/v1/pre-aggregations/partitions is not implemented yet",
        ))
    }

    /// `POST /cube-system/v1/pre-aggregations/preview`.
    async fn preview(&self, _ctx: &RequestContext, _query: Value) -> Result<Value, ApiError> {
        Err(ApiError::not_implemented(
            "/cube-system/v1/pre-aggregations/preview is not implemented yet",
        ))
    }

    /// `POST /cube-system/v1/pre-aggregations/build`.
    async fn build(&self, _ctx: &RequestContext, _query: Value) -> Result<Value, ApiError> {
        Err(ApiError::not_implemented(
            "/cube-system/v1/pre-aggregations/build is not implemented yet",
        ))
    }

    /// `POST /cube-system/v1/pre-aggregations/queue`.
    async fn queue(&self, _ctx: &RequestContext) -> Result<Value, ApiError> {
        Err(ApiError::not_implemented(
            "/cube-system/v1/pre-aggregations/queue is not implemented yet",
        ))
    }

    /// `POST /cube-system/v1/pre-aggregations/cancel`.
    async fn cancel(&self, _ctx: &RequestContext, _query: Value) -> Result<Value, ApiError> {
        Err(ApiError::not_implemented(
            "/cube-system/v1/pre-aggregations/cancel is not implemented yet",
        ))
    }

    /// The security contexts the scheduled refresh runs for
    /// (`scheduledRefreshContexts`). Empty when none are configured.
    async fn security_contexts(&self) -> Result<Vec<Value>, ApiError> {
        Ok(Vec::new())
    }

    /// `scheduledRefreshTimeZones`.
    async fn timezones(&self, _ctx: &RequestContext) -> Result<Vec<String>, ApiError> {
        Ok(Vec::new())
    }
}

/// SQL-to-query conversion: `POST /v1/convert-query` and `POST /v1/cubesql`.
/// Both are answered by the SQL API, so they are 501 when it is not started.
#[async_trait]
pub trait SqlConversionService: Send + Sync + Debug {
    /// `POST /v1/convert-query`: the REST query a SQL statement is
    /// equivalent to.
    async fn convert_query(&self, _ctx: &RequestContext, _sql: &str) -> Result<Value, ApiError> {
        Err(ApiError::not_implemented(
            "/v1/convert-query needs the SQL API, which is not started",
        ))
    }

    /// `POST /v1/cubesql`: how a statement would be answered.
    async fn cubesql(&self, _ctx: &RequestContext, _sql: &str) -> Result<Value, ApiError> {
        Err(ApiError::not_implemented(
            "/v1/cubesql needs the SQL API, which is not started",
        ))
    }
}

/// Answers 501 for both conversion routes.
#[derive(Debug, Default)]
pub struct UnimplementedSqlConversionService;

impl SqlConversionService for UnimplementedSqlConversionService {}

pub type SqlConversionServiceRef = Arc<dyn SqlConversionService>;

/// Answers 501 for every pre-aggregation route.
#[derive(Debug, Default)]
pub struct UnimplementedPreAggregationService;

impl PreAggregationService for UnimplementedPreAggregationService {}

pub type AuthServiceRef = Arc<dyn AuthService>;
pub type PreAggregationServiceRef = Arc<dyn PreAggregationService>;
pub type MetaServiceRef = Arc<dyn MetaService>;
pub type HealthServiceRef = Arc<dyn HealthService>;
pub type QueryServiceRef = Arc<dyn QueryService>;

/// Placeholder used until a surface is ported: every query endpoint answers
/// 501 so deployments can see what is missing instead of getting Node.js
/// behavior silently.
#[derive(Debug, Default)]
pub struct UnimplementedQueryService;

impl QueryService for UnimplementedQueryService {}

/// Health service that always reports healthy; used when no data source is
/// configured (e.g. `--check-config`) and in tests.
#[derive(Debug, Default)]
pub struct AlwaysHealthy;

#[async_trait]
impl HealthService for AlwaysHealthy {
    async fn readiness(&self) -> Result<(), String> {
        Ok(())
    }

    async fn liveness(&self) -> Result<(), String> {
        Ok(())
    }
}
