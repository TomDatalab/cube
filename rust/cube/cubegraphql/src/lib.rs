//! Cube's GraphQL API, in pure Rust.
//!
//! This is a port of `packages/cubejs-api-gateway/src/graphql.ts` together with
//! the two routes that mount it in `gateway.ts`:
//!
//! * `POST {basePath}/v1/graphql-to-json` — [`graphql_to_json_route`], which
//!   wraps [`graphql_to_json`];
//! * `{basePath}/graphql` — [`handle_graphql_request`] for `POST`, and
//!   [`graphiql_html`] for the `GET` IDE page.
//!
//! The schema is built at runtime from the data model with
//! `async_graphql::dynamic`, and query execution is delegated to a
//! caller-supplied [`CubeQueryExecutor`] so this crate never depends on the
//! query orchestrator.
//!
//! ```no_run
//! use std::sync::Arc;
//! use cubegraphql::{executor_fn, handle_graphql_request, make_schema_from_value};
//! use serde_json::json;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let meta = json!({ "cubes": [{
//!     "name": "Orders",
//!     "measures": [{ "name": "Orders.count", "type": "number", "isVisible": true }],
//!     "dimensions": []
//! }] });
//!
//! let schema = make_schema_from_value(&meta)?;
//! let executor = executor_fn(|_req| async move {
//!     Ok(json!({ "query": {}, "annotation": {}, "data": [] }))
//! });
//!
//! let body = json!({ "query": "query CubeQuery { cube { orders { count } } }" });
//! let response = handle_graphql_request(&schema, &body, Arc::clone(&executor)).await;
//! # Ok(())
//! # }
//! ```

#![deny(missing_docs)]

mod ast;
mod error;
mod executor;
mod meta;
mod naming;
mod response;
mod schema;
mod translate;

pub use ast::{parse_argument_value, parse_root_field, FieldNode};
pub use error::GraphQLError;
pub use executor::{
    executor_fn, CubeQueryExecutor, CubeQueryRequest, FnExecutor, ResponseExtensions, API_TYPE,
    REGULAR_QUERY,
};
pub use meta::{CubeMeta, MemberMeta, MemberType, MetaConfig};
pub use naming::{camelize, capitalize, object_name, safe_name, un_capitalize};
pub use response::{parse_dates, shape_rows};
pub use schema::{
    make_schema, make_schema_from_value, map_type, CUBE_FIELD, DATE_TIME, RESULT_TYPE,
};
pub use translate::{
    get_json_query, graphql_to_json, graphql_to_json_with_operation, map_where_operator,
    map_where_value, where_arg_to_query_filters, CubeQueryJson,
};

/// Re-exported so callers can build requests and read responses without adding
/// their own `async-graphql` dependency.
pub use async_graphql;

use async_graphql::dynamic::Schema;
use async_graphql::{Request, Response, Variables};
use serde_json::{json, Map, Value};
use std::sync::Arc;

/// `POST {basePath}/v1/graphql-to-json`.
///
/// Takes the request body (`{ "query": "...", "variables": { ... } }`) and
/// returns the response body. Exactly like the Node.js route, a translation
/// failure is reported as `{ "jsonQuery": null }` rather than an HTTP error;
/// the error is handed back so the caller can log it the way `gateway.ts` does.
pub fn graphql_to_json_route(body: &Value, meta: &MetaConfig) -> (Value, Option<GraphQLError>) {
    let query = match body.get("query").and_then(Value::as_str) {
        Some(query) => query,
        None => {
            return (
                json!({ "jsonQuery": Value::Null }),
                Some(GraphQLError::Translate(
                    "Request is missing a `query`".to_string(),
                )),
            )
        }
    };

    match graphql_to_json(query, body.get("variables"), meta) {
        Ok(json_query) => (json!({ "jsonQuery": json_query.into_value() }), None),
        Err(e) => (json!({ "jsonQuery": Value::Null }), Some(e)),
    }
}

/// Execute a GraphQL request against a schema built by [`make_schema`].
///
/// This is the `graphqlHTTP({ schema, context, extensions })` middleware of
/// `gateway.ts`, minus the HTTP plumbing: it injects the executor, runs the
/// request, and copies `annotation` / `lastRefreshTime` / `usedPreAggregations`
/// into the response `extensions`.
pub async fn execute_request(
    schema: &Schema,
    request: impl Into<Request>,
    executor: Arc<dyn CubeQueryExecutor>,
) -> Response {
    let extensions = ResponseExtensions::new();
    let request = request.into().data(executor).data(extensions.clone());

    let mut response = schema.execute(request).await;
    response.extensions.extend(extensions.take());
    response
}

/// As [`execute_request`], but speaks raw JSON: takes
/// `{ "query", "variables", "operationName" }` and returns the GraphQL response
/// body (`{ "data", "errors", "extensions" }`).
pub async fn handle_graphql_request(
    schema: &Schema,
    body: &Value,
    executor: Arc<dyn CubeQueryExecutor>,
) -> Value {
    let Some(query) = body.get("query").and_then(Value::as_str) else {
        return json!({
            "errors": [{ "message": "Must provide query string." }]
        });
    };

    let mut request = Request::new(query);

    if let Some(operation_name) = body.get("operationName").and_then(Value::as_str) {
        request = request.operation_name(operation_name);
    }

    if let Some(Value::Object(variables)) = body.get("variables") {
        request = request.variables(Variables::from_json(Value::Object(variables.clone())));
    }

    let response = execute_request(schema, request, executor).await;

    serde_json::to_value(&response).unwrap_or_else(
        |e| json!({ "errors": [{ "message": format!("Failed to serialize response: {e}") }] }),
    )
}

/// The GraphiQL IDE page served on `GET {basePath}/graphql`, with the header
/// editor enabled as the Node.js gateway does outside production.
pub fn graphiql_html(endpoint: &str) -> String {
    let endpoint = endpoint.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
  <head>
    <title>Cube GraphQL API</title>
    <style>body {{ margin: 0; height: 100vh; }} #graphiql {{ height: 100vh; }}</style>
    <link rel="stylesheet" href="https://unpkg.com/graphiql@3/graphiql.min.css" />
  </head>
  <body>
    <div id="graphiql">Loading…</div>
    <script crossorigin src="https://unpkg.com/react@18/umd/react.production.min.js"></script>
    <script crossorigin src="https://unpkg.com/react-dom@18/umd/react-dom.production.min.js"></script>
    <script src="https://unpkg.com/graphiql@3/graphiql.min.js"></script>
    <script>
      const fetcher = GraphiQL.createFetcher({{ url: "{endpoint}" }});
      ReactDOM.createRoot(document.getElementById('graphiql')).render(
        React.createElement(GraphiQL, {{ fetcher, isHeadersEditorEnabled: true }})
      );
    </script>
  </body>
</html>
"#
    )
}

/// Parse the `variables` member of a GraphQL HTTP request body.
pub fn parse_variables(body: &Value) -> Map<String, Value> {
    match body.get("variables") {
        Some(Value::Object(obj)) => obj.clone(),
        _ => Map::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> MetaConfig {
        MetaConfig::from_value(&json!({
            "cubes": [{
                "name": "Orders",
                "measures": [{ "name": "Orders.count", "type": "number", "isVisible": true }],
                "dimensions": [{ "name": "Orders.status", "type": "string", "isVisible": true }]
            }]
        }))
        .unwrap()
    }

    #[test]
    fn graphql_to_json_route_returns_the_query() {
        let (body, error) = graphql_to_json_route(
            &json!({ "query": "query CubeQuery { cube { orders { count } } }" }),
            &meta(),
        );

        assert!(error.is_none());
        assert_eq!(
            body,
            json!({ "jsonQuery": { "measures": ["Orders.count"] } })
        );
    }

    #[test]
    fn graphql_to_json_route_reports_a_null_query_on_failure() {
        let (body, error) = graphql_to_json_route(&json!({ "query": "query {" }), &meta());

        assert_eq!(body, json!({ "jsonQuery": null }));
        assert!(matches!(error, Some(GraphQLError::Parse(_))));
    }

    #[test]
    fn graphiql_page_embeds_the_endpoint() {
        assert!(graphiql_html("/cube/graphql").contains("\"/cube/graphql\""));
    }
}
