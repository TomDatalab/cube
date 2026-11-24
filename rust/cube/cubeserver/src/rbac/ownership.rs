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
        // `set_policy` stamps the generation when it installs the policy.
        generation: 0,
    })
}

/// Builds the policy, installs it, recompiles the models that are live and
/// returns what the model index now rejects. A no-op (nothing rejected) when
/// ownership is not enabled on the `Rbac` ([`super::Rbac::enable_ownership`]).
pub async fn refresh_policy(state: &AppState) -> Result<Vec<Rejection>, ApiError> {
    let rbac = state.rbac.as_deref().ok_or_else(ApiError::internal)?;
    if !rbac.ownership_enabled() {
        return Ok(Vec::new());
    }
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

    let index = state.model_index.get(Path::new(&state.config.schema_path));
    // A Platform file that breaks every scope is logged once.
    for e in index.unique_errors() {
        tracing::warn!("The data model does not compile after the policy changed: {}: {}", e.file, e.message);
    }
    Ok(index.rejected.iter().map(|(_, r)| r.clone()).collect())
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
    if !state.rbac.as_deref().is_some_and(|r| r.ownership_enabled()) {
        return Vec::new();
    }
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

/// Pending work of the background retry: how many refreshes were asked for,
/// and whether a task is serving them.
#[derive(Debug, Default)]
pub struct RetryState {
    requested: AtomicU64,
    running: std::sync::atomic::AtomicBool,
}

impl RetryState {
    /// Records that a refresh is owed. `true` when the caller must start the
    /// task (none is running).
    fn request(&self) -> bool {
        self.requested.fetch_add(1, Ordering::SeqCst);
        !self.running.swap(true, Ordering::SeqCst)
    }
}

/// Clears `running` when dropped, panic included.
struct ClearOnDrop<'a>(&'a RetryState);

impl Drop for ClearOnDrop<'_> {
    fn drop(&mut self) {
        self.0.running.store(false, Ordering::SeqCst);
    }
}

/// Serves the requests: tries `refresh` with backoff until it succeeds (or
/// `tries` run out), then starts over if a request came in meanwhile.
async fn drive<F, Fut>(state: &RetryState, mut refresh: F, first_delay: std::time::Duration, tries: u32)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    loop {
        // Every request up to here is satisfied by an attempt started after.
        let target = state.requested.load(Ordering::SeqCst);
        let guard = ClearOnDrop(state);
        let mut delay = first_delay;
        for _ in 0..tries {
            tokio::time::sleep(delay).await;
            if refresh().await {
                break;
            }
            delay = (delay * 2).min(std::time::Duration::from_secs(30));
        }
        // Clear the flag first, then look for a request that raced the exit:
        // a requester either sees the flag cleared (and starts its own task)
        // or bumped the counter before this check.
        drop(guard);
        if state.requested.load(Ordering::SeqCst) == target || state.running.swap(true, Ordering::SeqCst) {
            return;
        }
    }
}

fn spawn_retry(state: AppState) {
    let Some(rbac) = state.rbac.clone() else {
        return;
    };
    let retry = rbac.refresh_retry.clone();
    if !retry.request() {
        return;
    }
    tokio::spawn(async move {
        drive(
            &retry,
            || {
                let state = state.clone();
                async move {
                    let ok = refresh_policy(&state).await.is_ok();
                    if ok {
                        tracing::info!("The ownership policy is refreshed");
                    }
                    ok
                }
            },
            std::time::Duration::from_secs(1),
            12,
        )
        .await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn a_request_during_the_last_attempt_is_not_lost() {
        let state = Arc::new(RetryState::default());
        assert!(state.request());
        // Only one task at a time.
        assert!(!state.request());

        let calls = AtomicU64::new(0);
        let s = state.clone();
        drive(
            &state,
            || {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                let s = s.clone();
                async move {
                    // A new failure is reported while the first attempt runs.
                    if n == 0 {
                        assert!(!s.request());
                    }
                    true
                }
            },
            std::time::Duration::from_millis(1),
            3,
        )
        .await;
        // The second request got its own attempt, and the flag is free.
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(state.request());
    }

    #[tokio::test]
    async fn a_panic_does_not_leave_the_flag_set() {
        let state = Arc::new(RetryState::default());
        assert!(state.request());
        let s = state.clone();
        let result = tokio::spawn(async move {
            drive(&s, || async { panic!("boom") }, std::time::Duration::from_millis(1), 1).await
        })
        .await;
        assert!(result.is_err());
        assert!(state.request());
    }
}
