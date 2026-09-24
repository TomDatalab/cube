//! Cube REST API server in Rust.
//!
//! Replaces `@cubejs-backend/server` + `@cubejs-backend/api-gateway`
//! (Express) with an axum application. The HTTP layer depends only on the
//! service traits in [`services`], so the surface can be wired to Rust
//! implementations one by one while keeping the public API contract of the
//! Node.js gateway (`packages/cubejs-api-gateway/src/gateway.ts`).

pub mod app;
pub mod auth_adapter;
pub mod config;
pub mod error;
pub mod graphql_adapter;
pub mod handlers;
pub mod health_adapter;
pub mod load_response;
pub mod meta_adapter;
pub mod orchestrator_adapter;
pub mod planner_adapter;
pub mod playground;
pub mod services;
pub mod sql_api;
pub mod tenants;
pub mod ws;

pub use app::{build_app, AppState};
pub use config::ServerConfig;
pub use error::ApiError;
