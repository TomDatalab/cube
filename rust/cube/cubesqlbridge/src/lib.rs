//! The SQL API with no JavaScript in it.
//!
//! `cubesql` - the Postgres wire-protocol server and the DataFusion planner -
//! reaches the rest of Cube through two traits: `TransportService` for the
//! data model, SQL generation and query execution, and `SqlAuthService` for
//! the credentials of a connection. In the Node.js build both are implemented
//! by `packages/cubejs-backend-native`, which calls JavaScript functions over
//! a Neon channel for every one of them.
//!
//! This crate implements both in Rust:
//!
//! * [`RustTransport`] answers `meta`, `compiler_id`, `sql`,
//!   `can_switch_user_for_session` and `log_load_state` from the compiled
//!   model ([`cubemodel`]), the Tesseract planner ([`cubeplanner`]) and the
//!   process's own configuration. `load` and `load_stream` are the two that
//!   need the query orchestrator, so they go to an injected
//!   [`QueryExecutor`] - see [`executor`].
//! * [`RustSqlAuthService`] replaces the JS `checkSqlAuth` with
//!   `CUBEJS_SQL_USER` / `CUBEJS_SQL_PASSWORD` / `CUBEJS_SQL_SUPER_USER`, and
//!   optionally accepts a Cube JWT as the password through [`cubeauth`].
//! * [`start_sql_api`] wires the two into cubesql's service graph and runs the
//!   Postgres-protocol server.
//!
//! ```no_run
//! use std::sync::Arc;
//! use cubesqlbridge::{start_sql_api, ModelSource, SqlApiConfig};
//!
//! # async fn example() -> Result<(), cubesql::CubeError> {
//! let config = SqlApiConfig::from_env(ModelSource::dir("model/cubes"))
//!     .with_bind_address("0.0.0.0:15432");
//! let api = start_sql_api(config).await?;
//! api.wait_processing_loops().await?;
//! # Ok(())
//! # }
//! ```
//!
//! # No scripting runtime
//!
//! Nothing here embeds JavaScript or Python, and nothing calls back into a
//! Node.js process. Everything the SQL API cannot yet do natively fails with
//! an error naming what is missing, rather than falling back to a bridge.

// `cubesql::CubeError` carries an optional backtrace, so it is a large error
// type - but it is the error type of every trait this crate implements, so it
// cannot be boxed without changing signatures cubesql owns.
#![allow(clippy::result_large_err)]

pub mod auth;
pub mod config;
pub mod convert;
pub mod executor;
pub mod meta;
pub mod model_source;
pub mod planner;
pub mod sql_generator;
pub mod transport;

pub use auth::{RustSqlAuthContext, RustSqlAuthService, SqlAuthConfig};
pub use config::{start_sql_api, SqlApi, SqlApiConfig};
pub use convert::{rest4sql, sql4sql, ConvertedQuery, Sql4SqlPlan};
pub use executor::{
    LoadResponse, LoadResult, LoadResultAnnotation, LoadResultDataColumnar, NotConfiguredExecutor,
    QueryExecutor,
};
pub use meta::{
    compiler_id, cube_meta, data_source_to_sql_generator, member_to_data_source, meta_context,
    DEFAULT_DATA_SOURCE,
};
pub use model_source::ModelSource;
pub use planner::{DialectMap, PlannedStatement, PlannerPool};
pub use sql_generator::RustSqlGenerator;
pub use transport::{RustTransport, RustTransportOptions};

/// The SQL dialect the planner renders for, re-exported so a caller does not
/// need `cubeplanner` in its own dependency list.
pub use cubeplanner::Dialect;
/// Re-exported so a server can name cubesql's types without its own dependency.
pub use cubesql;
