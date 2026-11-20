//! The Data Source page with RBAC on: data sources live in the RBAC
//! database ([`crate::rbac::data_sources`]), not in `cube.yml`/`.env`.
//! What the file still declares (nothing, once the start-up import ran) is
//! read as before, and moves into RBAC when it is edited.

use std::collections::{BTreeMap, BTreeSet};

use cubeconfig::data_source::key_by_data_source;
use serde_json::{json, Value};

use super::data_sources::{
    current_variables, edited, env_with_file, kept_if_empty, load, restart_if_enabled,
    split_changes, test_variables, without_password,
};
use crate::app::AppState;
use crate::error::ApiError;
use crate::rbac::data_sources::{load as load_stored, load_each, save, StoredDataSource, Unreadable};
use crate::rbac::Rbac;

fn unavailable(e: String) -> ApiError {
    tracing::error!("RBAC data sources: {e}");
    ApiError::new(
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "The data sources cannot be read right now",
    )
}

/// Every data source RBAC holds, each read on its own: one that cannot be
/// read does not hide the others.
pub(crate) async fn stored(
    rbac: &Rbac,
) -> Result<Vec<Result<StoredDataSource, Unreadable>>, ApiError> {
    let each = load_each(&rbac.store, &rbac.config.secret_key)
        .await
        .map_err(unavailable)?;
    for unreadable in each.iter().filter_map(|r| r.as_ref().err()) {
        tracing::error!("RBAC data sources: {}", unreadable.error);
    }
    Ok(each)
}

/// Every data source RBAC holds, by name, with its type: read without
/// opening any secret.
pub(crate) async fn types(rbac: &Rbac) -> Result<BTreeMap<String, String>, ApiError> {
    Ok(rbac
        .store
        .data_sources()
        .await
        .map_err(|e| unavailable(e.to_string()))?
        .into_iter()
        .map(|row| (row.name, row.db_type))
        .collect())
}

/// A data source RBAC holds but cannot read, as the listing shows it.
pub(crate) fn listed_unreadable(ds: &Unreadable) -> Value {
    json!({
        "id": ds.id,
        "name": ds.name,
        "type": ds.db_type,
        "active": false,
        "error": "Its settings cannot be read: CUBE_RBAC_SECRET_KEY is not the key they were \
                  saved with, or the row is damaged. Delete it and add it again.",
    })
}

/// `name`'s settings and whether RBAC holds them (`false`: only `cube.yml`
/// declares it). `None` when neither has it.
pub(crate) async fn current(
    state: &AppState,
    rbac: &Rbac,
    name: &str,
) -> Result<Option<(BTreeMap<String, String>, bool)>, ApiError> {
    if let Some(ds) = load_stored(&rbac.store, &rbac.config.secret_key, name)
        .await
        .map_err(unavailable)?
    {
        return Ok(Some((ds.variables, true)));
    }
    let Some(path) = &state.config.config_file else {
        return Ok(None);
    };
    let env = env_with_file(&path.with_file_name(".env"));
    let config = load(path, &env)?;
    if !config.data_sources.contains_key(name) {
        return Ok(None);
    }
    Ok(Some((current_variables(&config, name, &env)?, false)))
}

/// A stored data source as `GET /playground/data-sources` lists it.
pub(crate) fn listed(ds: &StoredDataSource, active: bool) -> Value {
    let var = |k: &str| ds.variables.get(k).filter(|v| !v.is_empty());
    let host = var("CUBEJS_DB_HOST")
        .cloned()
        .or_else(|| var("CUBEJS_DB_URL").map(|url| without_password(url).0));
    json!({
        "id": ds.id,
        "name": ds.name,
        "type": var("CUBEJS_DB_TYPE"),
        "host": host,
        "port": var("CUBEJS_DB_PORT").and_then(|p| p.parse::<u16>().ok()),
        "database": var("CUBEJS_DB_NAME"),
        "user": var("CUBEJS_DB_USER"),
        "hasPassword": var("CUBEJS_DB_PASS").is_some(),
        "active": active,
    })
}

/// `GET .../:name`: the settings without the secrets.
pub(crate) fn without_secrets(name: &str, mut variables: BTreeMap<String, String>) -> Value {
    let db_type = variables.remove("CUBEJS_DB_TYPE").unwrap_or_default();
    let secrets: Vec<String> = variables
        .keys()
        .filter(|var| kept_if_empty(var, &variables))
        .cloned()
        .collect();
    variables.retain(|var, _| !secrets.contains(var));
    json!({
        "name": name,
        "type": db_type,
        "variables": variables,
        "hasPassword": secrets.iter().any(|s| s == "CUBEJS_DB_PASS"),
        "secrets": secrets,
    })
}

/// What a new data source must pass before it is written: a free name, and
/// a connection that works (one that does not is one /readyz would report
/// as down from the moment the server restarts).
pub(crate) async fn prepare_create(
    state: &AppState,
    rbac: &Rbac,
    name: &str,
    variables: &BTreeMap<String, String>,
) -> Result<(), ApiError> {
    if current(state, rbac, name).await?.is_some() {
        return Err(ApiError::bad_request(format!(
            "A data source named {name} already exists"
        )));
    }
    test_variables(variables).await
}

/// `PUT`: the form's fields; an empty secret keeps the stored one.
pub(crate) async fn update(
    state: &AppState,
    rbac: &Rbac,
    name: &str,
    variables: &BTreeMap<String, String>,
) -> Result<Value, ApiError> {
    let (current, _) = current(state, rbac, name)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("No data source named {name}")))?;
    let (set, cleared) = split_changes(variables, &current)?;
    if set.contains_key("CUBEJS_DB_TYPE") && set.get("CUBEJS_DB_TYPE") != current.get("CUBEJS_DB_TYPE") {
        return Err(ApiError::bad_request(
            "The database type cannot change; add a new data source instead",
        ));
    }
    let next = edited(&current, &set, &cleared);
    if next == current {
        return Ok(json!({ "name": name, "restarting": false }));
    }
    test_variables(&next).await?;
    save(&rbac.store, &rbac.config.secret_key, name, &next)
        .await
        .map_err(unavailable)?;
    tracing::info!(data_source = %name, "data source edited (RBAC)");
    // A cleared setting must not come back from the environment the
    // restarted process inherits.
    let remove = gone(name, &current, &next);
    Ok(json!({ "name": name, "restarting": restart_if_enabled(state, Vec::new(), remove) }))
}

/// Whether RBAC holds `name`, readable or not.
pub(crate) async fn holds(rbac: &Rbac, name: &str) -> Result<bool, ApiError> {
    Ok(rbac
        .store
        .data_source(name)
        .await
        .map_err(|e| unavailable(e.to_string()))?
        .is_some())
}

/// `DELETE` of a data source RBAC holds (the caller checked `default` and
/// the cubes using it). `false` when RBAC does not hold it.
pub(crate) async fn delete(state: &AppState, rbac: &Rbac, name: &str) -> Result<Option<Value>, ApiError> {
    let Some(row) = rbac
        .store
        .data_source(name)
        .await
        .map_err(|e| unavailable(e.to_string()))?
    else {
        return Ok(None);
    };
    // One whose secrets cannot be opened is deleted all the same; its
    // plain settings say what leaves the environment.
    let current = match load_stored(&rbac.store, &rbac.config.secret_key, name).await {
        Ok(Some(ds)) => ds.variables,
        _ => {
            let mut plain = row.settings;
            plain.insert("CUBEJS_DB_TYPE".to_string(), row.db_type);
            plain
        }
    };
    rbac.store
        .delete_data_source(name)
        .await
        .map_err(|e| unavailable(e.to_string()))?;
    tracing::info!(data_source = %name, "data source deleted (RBAC)");
    let remove = gone(name, &current, &BTreeMap::new());
    Ok(Some(json!({ "name": name, "restarting": restart_if_enabled(state, Vec::new(), remove) })))
}

/// The variables of `name` that `before` had and `after` has not.
fn gone(name: &str, before: &BTreeMap<String, String>, after: &BTreeMap<String, String>) -> Vec<String> {
    let after: BTreeSet<&String> = after.keys().collect();
    before
        .keys()
        .filter(|var| !after.contains(var))
        .map(|var| key_by_data_source(var, name))
        .collect()
}
