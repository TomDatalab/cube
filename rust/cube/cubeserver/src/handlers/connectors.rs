//! `GET /v1/connectors` — the database connectors this build carries.
//!
//! New in the Rust server: the Node.js gateway has no equivalent, because
//! there each driver was an npm package resolved at run time. Here the set is
//! fixed at compile time, so a client can ask what this binary can connect to
//! without trying a query first.

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::error::ApiError;
use crate::handlers::auth::assert_api_scope;
use crate::services::RequestContext;

#[derive(Debug, Deserialize)]
pub struct ConnectorParams {
    /// `?onlyConfigured=true` keeps only the connectors this deployment uses.
    #[serde(rename = "onlyConfigured")]
    only_configured: Option<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Connector {
    /// The `CUBEJS_DB_TYPE` value, e.g. `postgres`.
    #[serde(rename = "type")]
    pub db_type: String,
    /// Whether this build can actually open a connection of this type. A
    /// connector that is known but not ported answers `false`, and asking for
    /// it fails at start-up rather than here.
    pub implemented: bool,
    /// The configured data sources using it, in declaration order.
    #[serde(rename = "dataSources")]
    pub data_sources: Vec<String>,
}

/// Every known connector, with the data sources that use it.
///
/// Implemented connectors come first, each group in alphabetical order, so
/// the useful half of the list is at the top.
pub fn connectors(configured: &[(String, String)]) -> Vec<Connector> {
    let mut list: Vec<Connector> = cubedriver::KNOWN_DB_TYPES
        .iter()
        .map(|db_type| Connector {
            db_type: (*db_type).to_string(),
            implemented: cubedriver::DriverFactory::is_implemented(db_type),
            data_sources: configured
                .iter()
                .filter(|(_, configured_type)| configured_type == db_type)
                .map(|(name, _)| name.clone())
                .collect(),
        })
        .collect();

    // A data source may name a connector that is not in the known list at
    // all; hiding it would make the endpoint lie about the deployment.
    for (name, db_type) in configured {
        if !cubedriver::DriverFactory::is_known(db_type) {
            match list.iter_mut().find(|c| &c.db_type == db_type) {
                Some(existing) => existing.data_sources.push(name.clone()),
                None => list.push(Connector {
                    db_type: db_type.clone(),
                    implemented: false,
                    data_sources: vec![name.clone()],
                }),
            }
        }
    }

    list.sort_by(|a, b| {
        b.implemented
            .cmp(&a.implemented)
            .then_with(|| a.db_type.cmp(&b.db_type))
    });
    list
}

pub async fn list(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Query(params): Query<ConnectorParams>,
) -> Result<impl IntoResponse, ApiError> {
    // Describing the deployment is metadata, like `/v1/meta`.
    assert_api_scope(&state, &ctx, "meta").await?;

    let mut list = connectors(&state.config.data_sources);
    if params.only_configured.as_deref() == Some("true") {
        list.retain(|connector| !connector.data_sources.is_empty());
    }

    Ok(Json(serde_json::json!({ "connectors": list })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find<'a>(list: &'a [Connector], db_type: &str) -> &'a Connector {
        list.iter()
            .find(|c| c.db_type == db_type)
            .unwrap_or_else(|| panic!("{db_type} is missing from the list"))
    }

    #[test]
    fn every_known_connector_is_listed() {
        let list = connectors(&[]);
        assert_eq!(list.len(), cubedriver::KNOWN_DB_TYPES.len());
        assert!(find(&list, "postgres").implemented);
        // Known to Cube, no Rust driver yet.
        assert!(!find(&list, "athena").implemented);
    }

    #[test]
    fn a_configured_data_source_is_named_under_its_connector() {
        let list = connectors(&[
            ("default".to_string(), "postgres".to_string()),
            ("warehouse".to_string(), "snowflake".to_string()),
        ]);

        assert_eq!(find(&list, "postgres").data_sources, vec!["default"]);
        assert_eq!(find(&list, "snowflake").data_sources, vec!["warehouse"]);
        assert!(find(&list, "mysql").data_sources.is_empty());
    }

    #[test]
    fn two_data_sources_may_share_one_connector() {
        let list = connectors(&[
            ("default".to_string(), "postgres".to_string()),
            ("reporting".to_string(), "postgres".to_string()),
        ]);

        assert_eq!(
            find(&list, "postgres").data_sources,
            vec!["default", "reporting"]
        );
    }

    #[test]
    fn an_unknown_connector_is_still_reported_when_it_is_configured() {
        let list = connectors(&[("odd".to_string(), "no-such-database".to_string())]);

        let odd = find(&list, "no-such-database");
        assert!(!odd.implemented);
        assert_eq!(odd.data_sources, vec!["odd"]);
    }

    #[test]
    fn implemented_connectors_come_first() {
        let list = connectors(&[]);
        let first_unimplemented = list
            .iter()
            .position(|c| !c.implemented)
            .expect("some connector is unported");

        assert!(list[..first_unimplemented].iter().all(|c| c.implemented));
        assert!(list[first_unimplemented..].iter().all(|c| !c.implemented));
        assert_eq!(first_unimplemented, cubedriver::IMPLEMENTED_DB_TYPES.len());
    }
}
