//! Placeholder services used until the corresponding Rust crate is wired
//! into the binary. They fail closed: nothing is served without auth.

use async_trait::async_trait;
use axum::http::StatusCode;
use cubeserver::services::{MetaService, RequestContext};
use cubeserver::ApiError;
use serde_json::Value;

#[derive(Debug)]
pub struct NotConfiguredMeta;

#[async_trait]
impl MetaService for NotConfiguredMeta {
    async fn meta(&self, _ctx: &RequestContext, _only_views: bool) -> Result<Value, ApiError> {
        Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Data model is not configured",
        ))
    }
}
