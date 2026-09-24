use crate::gateway::GatewayAuthContextRef;
use async_trait::async_trait;
use cubesql::CubeError;
use std::fmt::Debug;

/// Options for the `GET /v1/meta` REST endpoint.
#[derive(Debug, Clone, Default)]
pub struct GatewayMetaRequest {
    /// `?onlyViews=true` — return only cubes of type `view`.
    pub only_views: bool,
}

/// Provides the payload for `GET /v1/meta` of the native API gateway.
///
/// While the schema compiler still lives in Node.js, the implementation is
/// `NodeBridgeTransport`, which calls back into the JS `ApiGateway.meta`.
/// Once the compiler is ported to Rust, a native implementation replaces it
/// without touching the HTTP handler.
#[async_trait]
pub trait GatewayMetaService: Send + Sync + Debug {
    async fn meta(
        &self,
        auth_context: &GatewayAuthContextRef,
        request: GatewayMetaRequest,
    ) -> Result<serde_json::Value, CubeError>;
}
