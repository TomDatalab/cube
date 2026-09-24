//! REST API query handling for Cube: the Rust port of
//! `packages/cubejs-api-gateway/src/query.js`, `date-parser.js` and the
//! request parsing of `gateway.ts`.
//!
//! The crate has no HTTP framework dependency: it turns the JSON a client
//! sends into a validated [`NormalizedQuery`] the planner can consume, with
//! the same lenient input shapes and the same user-facing error messages as
//! the Node.js gateway.

pub mod chrono_text;
pub mod config;
pub mod date_parser;
pub mod error;
pub mod moment;
pub mod normalize;
pub mod request;
pub mod timezone;
pub mod types;

pub use config::QueryConfig;
pub use date_parser::{date_parser, DateRangeBounds};
pub use error::QueryError;
pub use normalize::{
    get_query_granularity, normalize_query, normalize_query_filters, normalize_query_order,
    resolve_date_range,
};
pub use request::{
    compare_date_range_transformer, get_normalized_queries, get_pivot_query, parse_query_param,
};
pub use timezone::canonical_timezone;
pub use types::*;
