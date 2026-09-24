//! Outgoing query description: SQL plus the optional parts of `HttpQuery`
//! (bound parameters, inline tables, tracing object, response format).
//!
//! Port of the request side of
//! `packages/cubejs-cubestore-driver/src/WebSocketConnection.ts`.

use crate::result::ResponseFormat;

/// A bound query parameter (`HttpParameterValue` union).
#[derive(Debug, Clone, PartialEq)]
pub enum QueryParameter {
    Null,
    Bool(bool),
    Int64(i64),
    Float64(f64),
    String(String),
    Binary(Vec<u8>),
}

impl From<bool> for QueryParameter {
    fn from(v: bool) -> Self {
        QueryParameter::Bool(v)
    }
}

impl From<i64> for QueryParameter {
    fn from(v: i64) -> Self {
        QueryParameter::Int64(v)
    }
}

impl From<f64> for QueryParameter {
    fn from(v: f64) -> Self {
        QueryParameter::Float64(v)
    }
}

impl From<String> for QueryParameter {
    fn from(v: String) -> Self {
        QueryParameter::String(v)
    }
}

impl From<&str> for QueryParameter {
    fn from(v: &str) -> Self {
        QueryParameter::String(v.to_string())
    }
}

/// An inline table shipped with the query (`HttpTable`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InlineTable {
    pub name: String,
    pub columns: Vec<String>,
    pub types: Vec<String>,
    /// Rows in CSV format.
    pub csv_rows: String,
}

/// Everything a query carries besides its SQL text.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryOptions {
    pub parameters: Vec<QueryParameter>,
    pub inline_tables: Vec<InlineTable>,
    /// JSON tracing object (`trace_obj`), already serialised.
    pub trace_obj: Option<String>,
    /// Result encoding asked of the server. `Completed` is a decode-only
    /// descriptor and is sent as `Arrow`.
    pub response_format: ResponseFormat,
}

impl Default for QueryOptions {
    fn default() -> Self {
        Self {
            parameters: Vec::new(),
            inline_tables: Vec::new(),
            trace_obj: None,
            response_format: ResponseFormat::Arrow,
        }
    }
}

impl QueryOptions {
    pub fn with_parameters(mut self, parameters: Vec<QueryParameter>) -> Self {
        self.parameters = parameters;
        self
    }

    pub fn with_inline_tables(mut self, inline_tables: Vec<InlineTable>) -> Self {
        self.inline_tables = inline_tables;
        self
    }

    pub fn with_trace_obj(mut self, trace_obj: Option<String>) -> Self {
        self.trace_obj = trace_obj;
        self
    }

    pub fn with_response_format(mut self, response_format: ResponseFormat) -> Self {
        self.response_format = response_format;
        self
    }
}
