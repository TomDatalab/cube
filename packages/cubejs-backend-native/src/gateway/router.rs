use crate::gateway::gateway_auth_middleware;
use crate::gateway::handlers::{meta_handler_v1, stream_handler_v2};
use crate::gateway::state::ApiGatewayStateRef;
use axum::routing::{get, MethodRouter};
use axum::Router;

pub type RApiGatewayRouter = Router<ApiGatewayStateRef>;

#[derive(Debug, Clone)]
pub struct ApiGatewayRouterBuilder {
    router: RApiGatewayRouter,
}

impl ApiGatewayRouterBuilder {
    pub fn new(state: ApiGatewayStateRef) -> Self {
        let authenticated = |method_router: MethodRouter<ApiGatewayStateRef>| {
            method_router.layer(axum::middleware::from_fn_with_state(
                state.clone(),
                gateway_auth_middleware,
            ))
        };

        let router = Router::new()
            // REST API v1 — endpoints migrated from packages/cubejs-api-gateway.
            // Paths are relative to the Node.js base path (default `/cubejs-api`),
            // which Express strips before proxying here.
            .route("/v1/meta", authenticated(get(meta_handler_v1)))
            // API v2 — native-only endpoints.
            .route("/v2/stream", authenticated(get(stream_handler_v2)));

        Self { router }
    }

    pub fn route(self, path: &str, method_router: MethodRouter<ApiGatewayStateRef>) -> Self {
        Self {
            router: self.router.route(path, method_router),
        }
    }

    pub fn build(self) -> RApiGatewayRouter {
        self.router
    }
}
