use std::sync::Arc;

use axum::extract::Request;
use axum::http::{header, Method, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, get_service};
use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use crate::config::ServerConfig;
use crate::error::ApiError;
use crate::handlers;
use crate::services::{
    AuthServiceRef, HealthServiceRef, MetaServiceRef, PreAggregationServiceRef, QueryServiceRef,
    RequestContext, SqlConversionServiceRef,
};

#[derive(Clone, Debug)]
pub struct AppState {
    pub config: Arc<ServerConfig>,
    /// Limits and defaults applied while normalizing queries.
    pub query_config: Arc<cubequery::QueryConfig>,
    pub auth: AuthServiceRef,
    pub meta: MetaServiceRef,
    pub health: HealthServiceRef,
    pub query: QueryServiceRef,
    pub pre_aggregations: PreAggregationServiceRef,
    pub sql_conversion: SqlConversionServiceRef,
    /// WebSocket transport state (subscriptions and connections).
    pub ws: crate::ws::WsState,
    /// The cached GraphQL schema, rebuilt when the data model changes.
    pub graphql: Arc<crate::graphql_adapter::GraphQLSchemaCache>,
    /// Per-tenant runtimes. When absent the fields above answer every request,
    /// which is the single-tenant deployment.
    pub tenants: Option<Arc<crate::tenants::TenantRegistry>>,
}

impl AppState {
    /// The runtime serving this request, or `None` in a single-tenant
    /// deployment.
    pub async fn runtime_for(
        &self,
        ctx: &RequestContext,
    ) -> Result<Option<Arc<crate::tenants::TenantRuntime>>, ApiError> {
        match &self.tenants {
            Some(registry) => registry.resolve(&ctx.security_context).await.map(Some),
            None => Ok(None),
        }
    }

    /// The data model this request reads.
    pub async fn meta_for(&self, ctx: &RequestContext) -> Result<MetaServiceRef, ApiError> {
        Ok(match self.runtime_for(ctx).await? {
            Some(runtime) => runtime.meta.clone(),
            None => self.meta.clone(),
        })
    }

    /// The query service this request runs on.
    pub async fn query_for(&self, ctx: &RequestContext) -> Result<QueryServiceRef, ApiError> {
        Ok(match self.runtime_for(ctx).await? {
            Some(runtime) => runtime.query.clone(),
            None => self.query.clone(),
        })
    }

    /// The GraphQL schema cache of this request's model.
    pub async fn graphql_for(
        &self,
        ctx: &RequestContext,
    ) -> Result<Arc<crate::graphql_adapter::GraphQLSchemaCache>, ApiError> {
        Ok(match self.runtime_for(ctx).await? {
            Some(runtime) => runtime.graphql.clone(),
            None => self.graphql.clone(),
        })
    }

    /// The API scopes granted to this request.
    ///
    /// A tenant rule may narrow them (`api_scopes` in `cube.yml`); otherwise
    /// the auth service decides, as in Node.js.
    pub async fn api_scopes_for(&self, ctx: &RequestContext) -> Result<Vec<String>, ApiError> {
        if let Some(runtime) = self.runtime_for(ctx).await? {
            return Ok(runtime
                .tenant
                .api_scopes
                .iter()
                .map(|scope| scope.as_str().to_string())
                .collect());
        }

        self.auth.api_scopes(&ctx.security_context).await
    }
}

/// Builds the axum application with the route table of
/// `ApiGateway.initApp` (`packages/cubejs-api-gateway/src/gateway.ts`).
pub fn build_app(state: AppState) -> Router {
    let base = state.config.base_path.trim_end_matches('/').to_string();
    let max_request_size = state.config.max_request_size;

    let authenticated = Router::new()
        .route(&format!("{base}/v1/meta"), get(handlers::meta::meta))
        .route(
            &format!("{base}/v1/connectors"),
            get(handlers::connectors::list),
        )
        .route(
            &format!("{base}/v1/load"),
            get(handlers::query::load_get).post(handlers::query::load_post),
        )
        .route(
            &format!("{base}/v1/sql"),
            get(handlers::query::sql_get).post(handlers::query::sql_post),
        )
        .route(
            &format!("{base}/v1/dry-run"),
            get(handlers::query::dry_run_get).post(handlers::query::dry_run_post),
        )
        // `/v1/subscribe` is `/v1/load` under another path, as in Node.js.
        .route(
            &format!("{base}/v1/subscribe"),
            get(handlers::query::load_get).post(handlers::query::load_post),
        )
        .route(
            &format!("{base}/v1/running-query/{{requestId}}"),
            axum::routing::delete(handlers::query::cancel_running_query),
        )
        .route(
            &format!("{base}/v1/convert-query"),
            axum::routing::post(handlers::sql_convert::convert_query),
        )
        .route(
            &format!("{base}/v1/cubesql"),
            axum::routing::post(handlers::sql_convert::cubesql),
        )
        .route(
            &format!("{base}/v1/graphql-to-json"),
            axum::routing::post(handlers::graphql::graphql_to_json),
        )
        .route(
            &format!("{base}/graphql"),
            get(handlers::graphql::graphiql).post(handlers::graphql::graphql),
        )
        .route(
            &format!("{base}/v1/pre-aggregations/can-use"),
            axum::routing::post(handlers::preaggs::can_use),
        )
        .route(
            &format!("{base}/v1/pre-aggregations/jobs"),
            axum::routing::post(handlers::preaggs::jobs),
        )
        .route("/cube-system/v1/context", get(handlers::system::context))
        // System routes: reachable only with a playground-signed token.
        .route(
            "/cube-system/v1/pre-aggregations",
            get(handlers::preaggs::list),
        )
        .route(
            "/cube-system/v1/pre-aggregations/security-contexts",
            get(handlers::preaggs::security_contexts),
        )
        .route(
            "/cube-system/v1/pre-aggregations/timezones",
            get(handlers::preaggs::timezones),
        )
        .route(
            "/cube-system/v1/pre-aggregations/partitions",
            axum::routing::post(handlers::preaggs::partitions),
        )
        .route(
            "/cube-system/v1/pre-aggregations/preview",
            axum::routing::post(handlers::preaggs::preview),
        )
        .route(
            "/cube-system/v1/pre-aggregations/build",
            axum::routing::post(handlers::preaggs::build),
        )
        .route(
            "/cube-system/v1/pre-aggregations/queue",
            axum::routing::post(handlers::preaggs::queue),
        )
        .route(
            "/cube-system/v1/pre-aggregations/cancel",
            axum::routing::post(handlers::preaggs::cancel),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            handlers::auth::auth_middleware,
        ));

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::DELETE, Method::OPTIONS])
        // Same list as `@cubejs-backend/server` (`server.ts`).
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::HeaderName::from_static("x-request-id"),
        ]);

    // The Playground, when its assets are configured. It is served at `/`
    // because the bundle fetches its helper API relatively and derives the
    // REST API address from the page's own URL; see `crate::playground`.
    let playground = state
        .config
        .playground_path
        .as_deref()
        .and_then(crate::playground::PlaygroundAssets::open);

    let router = Router::new()
        // No scope: probes.
        .route("/readyz", get(handlers::health::readiness))
        .route("/livez", get(handlers::health::liveness))
        // The WebSocket transport authenticates with its own `authorization`
        // message, so it is not behind the HTTP auth middleware.
        .route(&format!("{base}/ws"), get(crate::ws::ws_handler))
        .merge(authenticated);

    let router = match playground {
        Some(assets) => router
            .merge(handlers::playground::routes())
            .route_service("/", get_service(assets.index()))
            // Assets live at the root too (`/assets/...`, `/antd.min.css`).
            // A path with no file behind it must still get the API's JSON
            // 404, so this falls through rather than returning the app.
            .fallback(move |request: Request| {
                let assets = assets.clone();
                // Kept because serving consumes the request, and the JSON
                // 404 names the method and path.
                let method = request.method().clone();
                let path = request.uri().path().to_string();

                async move {
                    let response = assets.serve(request).await;
                    if response.status() == StatusCode::NOT_FOUND {
                        ApiError::not_found(format!("Cannot {method} {path}")).into_response()
                    } else {
                        response
                    }
                }
            }),
        None => router.fallback(not_found),
    };

    router
        .layer(RequestBodyLimitLayer::new(max_request_size))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn not_found(req: Request) -> ApiError {
    ApiError::not_found(format!("Cannot {} {}", req.method(), req.uri().path()))
}
