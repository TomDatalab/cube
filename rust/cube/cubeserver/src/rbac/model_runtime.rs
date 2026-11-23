//! Keeps the private `live` folder in step with the model rows: it is
//! rewritten whenever the model version in the database changes.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::model_snapshot::{materialize, remove_stale_tmp, sources};
use super::store::{Store, StoreError};
use crate::tenants::TenantRegistry;

pub struct ModelRuntime {
    store: Store,
    cache_dir: PathBuf,
    live: PathBuf,
    seen: AtomicI64,
    /// One refresh at a time: the writer's and the loop's.
    refreshing: tokio::sync::Mutex<()>,
}

impl ModelRuntime {
    /// Reads the rows and the organization slugs, materializes them and
    /// remembers the version.
    pub async fn start(store: Store, cache_dir: PathBuf) -> Result<Arc<Self>, String> {
        let (version, live) = Self::snapshot(&store, &cache_dir)
            .await
            .map_err(|e| format!("cannot materialize the data model: {e}"))?;
        Ok(Arc::new(Self {
            store,
            cache_dir,
            live,
            seen: AtomicI64::new(version),
            refreshing: tokio::sync::Mutex::new(()),
        }))
    }

    async fn snapshot(store: &Store, cache_dir: &Path) -> Result<(i64, PathBuf), String> {
        let rows = store.model_rows().await.map_err(|e| e.to_string())?;
        Self::install(store, cache_dir, rows).await
    }

    /// Materializes `rows`. Callers never run two at once (`start` runs
    /// before any refresh; refreshes hold `refreshing`), so the stale
    /// temporary folders can be removed first.
    async fn install(
        store: &Store,
        cache_dir: &Path,
        rows: super::model_store::ModelRows,
    ) -> Result<(i64, PathBuf), String> {
        let slugs = store.organization_slugs().await.map_err(|e| e.to_string())?;
        let srcs = sources(&rows, &slugs);
        let (dir, version) = (cache_dir.to_path_buf(), rows.version);
        let live = tokio::task::spawn_blocking(move || {
            if let Err(e) = remove_stale_tmp(&dir) {
                tracing::warn!(error = %e, "could not remove stale model folders");
            }
            materialize(&dir, version, &srcs)
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
        Ok((version, live))
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
        match Self::install(&self.store, &self.cache_dir, rows).await {
            Ok((version, _)) => {
                self.seen.store(version, Ordering::SeqCst);
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

impl std::fmt::Debug for ModelRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelRuntime")
            .field("live", &self.live)
            .field("version", &self.version())
            .finish()
    }
}
