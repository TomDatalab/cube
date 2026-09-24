use cubenativeutils::CubeError;
use std::fmt;

/// How a dialect names a construct its database has no spelling for, as in
/// "Unsupported by the oracle dialect: …". An error the planner raises with
/// this in its message becomes [`PlannerError::Unsupported`], wherever in the
/// planner it surfaced.
pub const UNSUPPORTED_BY_DIALECT: &str = "Unsupported by the ";

/// Everything that can go wrong between a YAML model plus a query and the SQL
/// the planner produces.
#[derive(Debug)]
pub enum PlannerError {
    /// The data model could not be read or parsed.
    Model(String),
    /// The request is not a query this planner can make sense of.
    Query(String),
    /// A data-model or query feature this pure-Rust planner does not implement.
    /// Always names the feature, so the caller can fall back deliberately.
    Unsupported(String),
    /// The planner itself refused the query or failed to compile the model.
    Plan(CubeError),
}

impl PlannerError {
    pub fn model(message: impl Into<String>) -> Self {
        Self::Model(message.into())
    }

    pub fn query(message: impl Into<String>) -> Self {
        Self::Query(message.into())
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::Unsupported(message.into())
    }

    /// The human-readable message, without the variant prefix.
    pub fn message(&self) -> String {
        match self {
            Self::Model(m) | Self::Query(m) | Self::Unsupported(m) => m.clone(),
            Self::Plan(err) => err.message.clone(),
        }
    }
}

impl fmt::Display for PlannerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Model(m) => write!(f, "Data model error: {m}"),
            Self::Query(m) => write!(f, "Query error: {m}"),
            Self::Unsupported(m) => write!(f, "Unsupported: {m}"),
            Self::Plan(err) => write!(f, "Planning error: {}", err.message),
        }
    }
}

impl std::error::Error for PlannerError {}

impl From<CubeError> for PlannerError {
    fn from(err: CubeError) -> Self {
        // A dialect refusing a construct is a named limitation, not a planning
        // failure, even when the planner reached it deep in a sub-query.
        match err.message.find(UNSUPPORTED_BY_DIALECT) {
            Some(start) => Self::Unsupported(err.message[start..].to_string()),
            None => Self::Plan(err),
        }
    }
}
