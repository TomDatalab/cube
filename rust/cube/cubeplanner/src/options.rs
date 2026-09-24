//! Per-request planning options.

use crate::dialect::Dialect;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Everything about a request that is not the query itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PlanOptions {
    /// The dialect the SQL is rendered for.
    pub dialect: Dialect,
    /// The request's security context, as the API gateway decoded it from the
    /// JWT. Member SQL reading `SECURITY_CONTEXT` resolves against it.
    pub security_context: Value,
    /// Query timezone. The query's own `timezone` wins when it sets one.
    pub timezone: Option<String>,
    /// The dialect external pre-aggregations are read through (CubeStore in
    /// production). `None` keeps every pre-aggregation in the source dialect.
    pub external_dialect: Option<Dialect>,
    /// Annotate the generated SQL with the members each expression came from.
    /// The query's own `exportAnnotatedSql` wins when it sets one.
    pub export_annotated_sql: bool,
    /// Convert a raw (non-granular) time dimension into the query timezone.
    /// The query's own `convertTzForRawTimeDimension` wins when it sets one.
    pub convert_tz_for_raw_time_dimension: bool,
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self {
            dialect: Dialect::Postgres,
            security_context: Value::Null,
            timezone: None,
            external_dialect: None,
            export_annotated_sql: false,
            convert_tz_for_raw_time_dimension: false,
        }
    }
}

impl PlanOptions {
    pub fn postgres() -> Self {
        Self::default()
    }

    pub fn with_dialect(mut self, dialect: Dialect) -> Self {
        self.dialect = dialect;
        self
    }

    pub fn with_security_context(mut self, security_context: Value) -> Self {
        self.security_context = security_context;
        self
    }

    pub fn with_timezone(mut self, timezone: impl Into<String>) -> Self {
        self.timezone = Some(timezone.into());
        self
    }

    /// The dialect external pre-aggregations are read through. Setting it to
    /// the source dialect is the same as leaving it unset.
    pub fn with_external_dialect(mut self, dialect: Dialect) -> Self {
        self.external_dialect = Some(dialect);
        self
    }

    pub fn with_export_annotated_sql(mut self, export_annotated_sql: bool) -> Self {
        self.export_annotated_sql = export_annotated_sql;
        self
    }

    pub fn with_convert_tz_for_raw_time_dimension(mut self, convert_tz: bool) -> Self {
        self.convert_tz_for_raw_time_dimension = convert_tz;
        self
    }
}
