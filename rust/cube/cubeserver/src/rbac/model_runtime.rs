//! Keeps the private `live` folder in step with the model rows: it is
//! rewritten whenever the model version in the database changes. Each scope
//! (an organization, the Platform) is compiled from a folder of its own,
//! `scopes/<key>/v<version>/`, written on first use.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use super::model_snapshot::{materialize, remove_stale_tmp, sources};
use super::scope::{scope_sources, ModelScope};
use super::store::{Store, StoreError};
use crate::tenants::TenantRegistry;

/// The folder under the cache dir that holds one folder per scope.
const SCOPES_DIR: &str = "scopes";

/// What the current version materializes from.
struct Snapshot {
    version: i64,
    sources: Arc<Vec<(String, String)>>,
    slugs: Arc<BTreeSet<String>>,
}

pub struct ModelRuntime {
    store: Store,
    cache_dir: PathBuf,
    live: PathBuf,
    seen: AtomicI64,
    current: RwLock<Snapshot>,
    /// One refresh at a time: the writer's and the loop's.
    refreshing: tokio::sync::Mutex<()>,
}

impl ModelRuntime {
    /// Reads the rows and the organization slugs, materializes them and
    /// remembers the version.
    pub async fn start(store: Store, cache_dir: PathBuf) -> Result<Arc<Self>, String> {
        let rows = store
            .model_rows()
            .await
            .map_err(|e| format!("cannot materialize the data model: {e}"))?;
        let (snapshot, live) = Self::install(&store, &cache_dir, rows, true)
            .await
            .map_err(|e| format!("cannot materialize the data model: {e}"))?;
        Ok(Arc::new(Self {
            store,
            cache_dir,
            live,
            seen: AtomicI64::new(snapshot.version),
            current: RwLock::new(snapshot),
            refreshing: tokio::sync::Mutex::new(()),
        }))
    }

    /// Materializes `rows` into `live` and into every scope folder already
    /// there. Callers never run two at once (`start` runs before any
    /// refresh; refreshes hold `refreshing`), so `live`'s stale temporary
    /// folders can be removed first; a scope's are removed only at `start`,
    /// before any [`Self::scope_path`] can be writing one.
    async fn install(
        store: &Store,
        cache_dir: &Path,
        rows: super::model_store::ModelRows,
        starting: bool,
    ) -> Result<(Snapshot, PathBuf), String> {
        let slugs = store.organization_slugs().await.map_err(|e| e.to_string())?;
        let snapshot = Snapshot {
            version: rows.version,
            sources: Arc::new(sources(&rows, &slugs)),
            slugs: Arc::new(slugs.into_values().collect()),
        };
        let (dir, version) = (cache_dir.to_path_buf(), rows.version);
        let (srcs, org_slugs) = (snapshot.sources.clone(), snapshot.slugs.clone());
        let live = tokio::task::spawn_blocking(move || {
            if let Err(e) = remove_stale_tmp(&dir) {
                tracing::warn!(error = %e, "could not remove stale model folders");
            }
            let live = materialize(&dir, version, &srcs)?;
            for scope in existing_scopes(&dir, &org_slugs) {
                let scope_dir = dir.join(SCOPES_DIR).join(scope.key());
                if starting {
                    if let Err(e) = remove_stale_tmp(&scope_dir) {
                        tracing::warn!(error = %e, "could not remove stale model folders");
                    }
                }
                // Writes the new version and prunes the old ones; a scope
                // that fails here is written again on its next use.
                if let Err(e) =
                    materialize(&scope_dir, version, &scope_files(&srcs, &scope, &org_slugs))
                {
                    tracing::warn!(scope = scope.key(), error = %e, "could not materialize a model scope");
                }
            }
            Ok::<_, std::io::Error>(live)
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
        Ok((snapshot, live))
    }

    /// `<cache>/scopes/<key>/v<version>/` of the current version, written
    /// on first use.
    pub async fn scope_path(&self, scope: &ModelScope) -> Result<PathBuf, StoreError> {
        let (version, srcs, slugs) = {
            let current = self.current.read().unwrap_or_else(|e| e.into_inner());
            (current.version, current.sources.clone(), current.slugs.clone())
        };
        let scope_dir = self.cache_dir.join(SCOPES_DIR).join(scope.key());
        let path = scope_dir.join(format!("v{version}"));
        if path.is_dir() {
            return Ok(path);
        }
        let scope = scope.clone();
        tokio::task::spawn_blocking(move || {
            materialize(&scope_dir, version, &scope_files(&srcs, &scope, &slugs))
        })
        .await
        .map_err(|e| StoreError::Db(e.to_string()))?
        .map_err(|e| StoreError::Db(format!("cannot materialize model scope: {e}")))?;
        Ok(path)
    }

    /// Whether `scope` exists: an organization only if it has a slug.
    pub fn has_scope(&self, scope: &ModelScope) -> bool {
        match scope {
            ModelScope::Organization(slug) => {
                let current = self.current.read().unwrap_or_else(|e| e.into_inner());
                current.slugs.contains(slug)
            }
            ModelScope::All | ModelScope::Platform => true,
        }
    }

    pub fn live_path(&self) -> &Path {
        &self.live
    }

    pub fn version(&self) -> i64 {
        self.seen.load(Ordering::SeqCst)
    }

    /// When the database holds a newer version than the one seen, rewrites
    /// `live` and returns `true`. Serialized, and never goes back to an
    /// older version.
    pub async fn refresh(&self) -> Result<bool, StoreError> {
        let _one = self.refreshing.lock().await;
        if self.store.model_version().await? <= self.version() {
            return Ok(false);
        }
        let rows = self.store.model_rows().await?;
        if rows.version <= self.version() {
            return Ok(false);
        }
        match Self::install(&self.store, &self.cache_dir, rows, false).await {
            Ok((snapshot, _)) => {
                self.seen.store(snapshot.version, Ordering::SeqCst);
                *self.current.write().unwrap_or_else(|e| e.into_inner()) = snapshot;
                Ok(true)
            }
            Err(e) => Err(StoreError::Db(e)),
        }
    }

    /// Every `interval`: `refresh()`, and after a change `tenants.reload_all()`.
    pub fn spawn_loop(self: &Arc<Self>, interval: Duration, tenants: Arc<TenantRegistry>) {
        let runtime = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                match runtime.refresh().await {
                    Ok(true) => {
                        for (tenant, err) in tenants.reload_all().await {
                            tracing::error!(%tenant, error = %err.body.error, "model reload failed");
                        }
                    }
                    Ok(false) => {}
                    Err(e) => tracing::error!(error = %e, "model refresh failed"),
                }
            }
        });
    }
}

/// The file a scope with no sources is compiled from: the planner refuses a
/// folder without YAML, and such a scope serves an empty model instead.
const EMPTY_SCOPE_FILE: (&str, &str) = ("_empty.yml", "cubes: []\n");

/// What `scope`'s folder holds: its sources, or [`EMPTY_SCOPE_FILE`].
fn scope_files(
    srcs: &[(String, String)],
    scope: &ModelScope,
    org_slugs: &BTreeSet<String>,
) -> Vec<(String, String)> {
    let files = scope_sources(srcs, scope, org_slugs);
    if !files.is_empty() {
        return files;
    }
    let (path, content) = EMPTY_SCOPE_FILE;
    vec![(path.to_string(), content.to_string())]
}

/// The scopes with a folder under `<cache>/scopes/`: the Platform and the
/// organizations that still exist.
fn existing_scopes(cache_dir: &Path, org_slugs: &BTreeSet<String>) -> Vec<ModelScope> {
    let Ok(entries) = std::fs::read_dir(cache_dir.join(SCOPES_DIR)) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|key| match key.as_str() {
            k if k == ModelScope::Platform.key() => Some(ModelScope::Platform),
            k if org_slugs.contains(k) => Some(ModelScope::Organization(key)),
            _ => None,
        })
        .collect()
}

impl std::fmt::Debug for ModelRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelRuntime")
            .field("live", &self.live)
            .field("version", &self.version())
            .finish()
    }
}

/// The folder a boot service (meta, planner, SQL API) compiles. Without RBAC
/// it is `schema_path`; with RBAC it is `scope`'s folder, since the whole
/// model may hold the same cube name once per organization.
pub async fn boot_model_path(
    runtime: Option<&ModelRuntime>,
    schema_path: &str,
    scope: &ModelScope,
) -> Result<String, String> {
    let Some(runtime) = runtime else {
        return Ok(schema_path.to_string());
    };
    if !runtime.has_scope(scope) {
        return Err(format!("no organization {}", scope.key()));
    }
    let path = runtime.scope_path(scope).await.map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn boot_model_path_without_rbac_is_the_schema_path() {
        for scope in [
            ModelScope::Platform,
            ModelScope::Organization("digital".into()),
        ] {
            assert_eq!(
                boot_model_path(None, "model", &scope).await.unwrap(),
                "model"
            );
        }
    }
}
