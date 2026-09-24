use thiserror::Error;

/// Errors raised while building the schema or translating a GraphQL document
/// into a Cube REST query.
///
/// The messages mirror the ones `packages/cubejs-api-gateway/src/graphql.ts`
/// produces so that clients see the same text they did on the Node.js gateway.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum GraphQLError {
    /// The document could not be parsed as GraphQL.
    #[error("{0}")]
    Parse(String),
    /// The document parsed but is not a Cube query we can translate.
    #[error("{0}")]
    Translate(String),
    /// The meta config handed to the schema builder is not shaped like
    /// `{ "cubes": [...] }`.
    #[error("{0}")]
    Meta(String),
    /// `async-graphql` refused the generated schema.
    #[error("{0}")]
    Schema(String),
    /// The caller-supplied query executor failed.
    #[error("{0}")]
    Execution(String),
}

impl GraphQLError {
    /// `Variable "x" is not defined` — the exact message `graphql.ts` throws
    /// from `parseArgumentValue` / `getArgumentValue`.
    pub fn undefined_variable(name: &str) -> Self {
        GraphQLError::Translate(format!("Variable \"{name}\" is not defined"))
    }
}
