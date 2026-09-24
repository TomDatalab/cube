#![allow(dead_code)]

//! Shared fixtures: the same meta configs `test/graphql.test.ts` uses, and the
//! repository's own `.gql` query fixtures.

use cubegraphql::MetaConfig;
use serde_json::{json, Value};

/// `metaConfig` from `packages/cubejs-api-gateway/test/graphql.test.ts`.
pub fn meta_config_value() -> Value {
    json!([
        {
            "config": {
                "name": "Orders",
                "measures": [
                    { "name": "Orders.count", "isVisible": true },
                    { "name": "Orders.totalAmount", "isVisible": true }
                ],
                "dimensions": [
                    { "name": "Orders.id", "isVisible": true },
                    { "name": "Orders.status", "type": "string", "isVisible": true },
                    { "name": "Orders.createdAt", "type": "time", "isVisible": true }
                ]
            }
        },
        {
            "config": {
                "name": "Users",
                "measures": [{ "name": "Users.count", "isVisible": true }],
                "dimensions": [
                    { "name": "Users.id", "isVisible": true },
                    { "name": "Users.city", "type": "string", "isVisible": true },
                    { "name": "Users.createdAt", "type": "time", "isVisible": true }
                ]
            }
        }
    ])
}

/// `metaConfigSnakeCase` from the same test file.
pub fn meta_config_snake_case_value() -> Value {
    json!([
        {
            "config": {
                "name": "orders",
                "measures": [{ "name": "orders.count", "isVisible": true }],
                "dimensions": [
                    { "name": "orders.id", "isVisible": true },
                    { "name": "orders.status", "type": "string", "isVisible": true },
                    { "name": "orders.created_at", "type": "time", "isVisible": true }
                ]
            }
        },
        {
            "config": {
                "name": "users",
                "measures": [{ "name": "users.count", "isVisible": true }],
                "dimensions": [
                    { "name": "users.id", "isVisible": true },
                    { "name": "users.city", "type": "string", "isVisible": true },
                    { "name": "users.created_at", "type": "time", "isVisible": true }
                ]
            }
        }
    ])
}

/// The same meta config expressed the way `cubemodel::rest_meta_response`
/// emits it — `{ "cubes": [ ...configs ] }`.
pub fn meta_config_rest_shape() -> Value {
    let cubes: Vec<Value> = meta_config_value()
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["config"].clone())
        .collect();
    json!({ "cubes": cubes })
}

pub fn meta() -> MetaConfig {
    MetaConfig::from_value(&meta_config_value()).unwrap()
}

pub fn meta_snake_case() -> MetaConfig {
    MetaConfig::from_value(&meta_config_snake_case_value()).unwrap()
}

/// `test/graphql-queries/base.gql`, split the way the Jest suite splits it.
pub fn base_queries() -> Vec<String> {
    read_queries(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../packages/cubejs-api-gateway/test/graphql-queries/base.gql"
    ))
}

/// `test/graphql-queries/base-snake-case.gql`.
pub fn base_snake_case_queries() -> Vec<String> {
    read_queries(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../packages/cubejs-api-gateway/test/graphql-queries/base-snake-case.gql"
    ))
}

fn read_queries(path: &str) -> Vec<String> {
    let contents = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read the GraphQL fixtures at {path}: {e}"));

    contents
        .split("\n\n")
        .map(|query| query.trim().to_string())
        .filter(|query| !query.is_empty())
        .collect()
}
