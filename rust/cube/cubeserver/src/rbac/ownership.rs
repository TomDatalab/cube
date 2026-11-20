//! The folder ownership policy (`cubemodel::ownership`) built from the RBAC
//! store: which data sources each organization holds, and which stay with the
//! platform. It is installed at start-up and rebuilt whenever a connection
//! moves between organizations or an organization is created or deleted.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use cubemodel::ownership::{set_policy, OwnershipPolicy, Rejection};

use super::store::{Search, Store, StoreError};
use crate::app::AppState;
use crate::error::ApiError;

/// Every policy gets a new number, so caches keyed on it see the change.
static GENERATION: AtomicU64 = AtomicU64::new(1);

/// The policy for the organizations and connections `store` holds.
///
/// `data_source_names` are the data sources the deployment declares; those no
/// connection claims belong to the platform. An organization without
/// connections is in the policy with an empty set.
pub async fn build_policy(
    store: &Store,
    data_source_names: &[String],
) -> Result<OwnershipPolicy, StoreError> {
    let organizations = store.search_organizations(None, &Search::default()).await?;
    let connections = store.all_connections().await?;

    let slug_of: BTreeMap<_, _> = organizations
        .items
        .iter()
        .map(|o| (o.id, o.slug.clone()))
        .collect();
    let mut by_slug: BTreeMap<String, BTreeSet<String>> = organizations
        .items
        .iter()
        .map(|o| (o.slug.clone(), BTreeSet::new()))
        .collect();
    let mut claimed: BTreeSet<&str> = BTreeSet::new();
    for connection in &connections {
        if let Some(slug) = slug_of.get(&connection.organization_id) {
            by_slug
                .entry(slug.clone())
                .or_default()
                .insert(connection.name.clone());
            claimed.insert(&connection.name);
        }
    }
    let platform = data_source_names
        .iter()
        .filter(|name| !claimed.contains(name.as_str()))
        .cloned()
        .collect();

    Ok(OwnershipPolicy {
        organizations: by_slug,
        platform,
        generation: GENERATION.fetch_add(1, Ordering::SeqCst),
    })
}

/// Builds the policy, installs it, recompiles the models that are live and
/// returns what the model index now rejects.
pub async fn refresh_policy(state: &AppState) -> Result<Vec<Rejection>, ApiError> {
    let rbac = state.rbac.as_deref().ok_or_else(ApiError::internal)?;
    // Build, install and reload as one step: two refreshes in flight could
    // otherwise install an older policy over a newer one.
    let _serial = rbac.refresh_lock.lock().await;
    let policy = build_policy(&rbac.store, &rbac.data_source_names())
        .await
        .map_err(|e| {
            tracing::error!("Cannot build the ownership policy: {e}");
            super::unavailable()
        })?;
    set_policy(Some(Arc::new(policy)));

    // Meta, planner and GraphQL of every live tenant load the model with the
    // policy that was current at that time.
    if let Some(tenants) = &state.tenants {
        tenants.reload_all().await;
    }

    match state.model_index.get(Path::new(&state.config.schema_path)) {
        Ok(index) => Ok(index.rejected.iter().map(|(_, r)| r.clone()).collect()),
        Err(e) => {
            tracing::warn!("Cannot index the data model after the policy changed: {e}");
            Ok(Vec::new())
        }
    }
}

/// Logs the files a move left in the wrong folder, as `move <file> to <dest>/`.
pub fn warn_rejections(connection: &str, destination: &str, rejections: &[Rejection]) {
    let marker = format!("data source {connection} ");
    for r in rejections.iter().filter(|r| r.message.contains(&marker)) {
        tracing::warn!(
            connection,
            cube = %r.cube,
            "{}: move {} to {destination}/",
            r.message,
            r.file
        );
    }
}

/// [`refresh_policy`] after the database already changed: a failure must not
/// fail the request (the change is committed and a retry of it would not
/// heal). It is logged and retried in the background, with backoff, until it
/// succeeds; at most one retry task runs at a time.
pub async fn refresh_after_commit(state: &AppState) -> Vec<Rejection> {
    match refresh_policy(state).await {
        Ok(rejections) => rejections,
        Err(e) => {
            tracing::error!(
                "Cannot refresh the ownership policy after a change: {}; retrying in the background",
                e.body.error
            );
            spawn_retry(state.clone());
            Vec::new()
        }
    }
}

fn spawn_retry(state: AppState) {
    let Some(rbac) = state.rbac.clone() else {
        return;
    };
    if rbac.refresh_retrying.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        let mut delay = std::time::Duration::from_secs(1);
        for _ in 0..12 {
            tokio::time::sleep(delay).await;
            if refresh_policy(&state).await.is_ok() {
                tracing::info!("The ownership policy is refreshed");
                break;
            }
            delay = (delay * 2).min(std::time::Duration::from_secs(30));
        }
        rbac.refresh_retrying.store(false, Ordering::SeqCst);
    });
}
