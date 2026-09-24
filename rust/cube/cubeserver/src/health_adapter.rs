//! [`HealthService`] backed by the `cubedriver` and `cubeorch` crates — the
//! Rust replacement for the `/readyz` and `/livez` probes of the Node.js
//! gateway (`ApiGateway.readiness` / `ApiGateway.liveness`), which test the
//! data source and the orchestrator connections.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use cubedriver::Driver;
use cubeorch::{DriverFactory, QueryOrchestrator};
use tokio::sync::Mutex;

use crate::services::HealthService;

/// Tests the connection of every configured data source, plus the
/// orchestrator's own store (the cache driver).
///
/// Drivers are built on the first probe rather than at start-up, so a server
/// whose database is still coming up boots and reports `DOWN` until the
/// database answers, instead of failing to start. Each driver is then kept,
/// so a probe every few seconds reuses one connection pool per data source
/// rather than building a new one every time.
pub struct DriverHealthService {
    /// Data source names, in declaration order; `default` first.
    data_sources: Vec<String>,
    factory: DriverFactory,
    /// Tests the orchestrator's store, as `testOrchestratorConnections` does.
    orchestrator: Option<Arc<QueryOrchestrator>>,
    /// `standalone` in Node.js: readiness only probes the data sources when
    /// this server owns the orchestrator.
    standalone: bool,
    drivers: Mutex<HashMap<String, Arc<dyn Driver>>>,
}

impl std::fmt::Debug for DriverHealthService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DriverHealthService")
            .field("data_sources", &self.data_sources)
            .field("orchestrator", &self.orchestrator.is_some())
            .field("standalone", &self.standalone)
            .finish()
    }
}

impl DriverHealthService {
    pub fn new(
        data_sources: Vec<String>,
        factory: DriverFactory,
        orchestrator: Option<Arc<QueryOrchestrator>>,
        standalone: bool,
    ) -> Self {
        // A deployment configured only through the environment declares no
        // data source by name, but it still has the default one.
        let data_sources = if data_sources.is_empty() {
            vec!["default".to_string()]
        } else {
            data_sources
        };

        Self {
            data_sources,
            factory,
            orchestrator,
            standalone,
            drivers: Mutex::new(HashMap::new()),
        }
    }

    /// The driver of `data_source`, built once and kept.
    async fn driver(&self, data_source: &str) -> Result<Arc<dyn Driver>, String> {
        if let Some(driver) = self.drivers.lock().await.get(data_source) {
            return Ok(Arc::clone(driver));
        }

        // Built outside the lock: creating a driver reads the environment and
        // can block, and two probes racing here cost one extra pool at worst.
        let driver = (self.factory)(data_source.to_string())
            .await
            .map_err(|e| format!("{data_source}: {e}"))?;

        Ok(Arc::clone(
            self.drivers
                .lock()
                .await
                .entry(data_source.to_string())
                .or_insert(driver),
        ))
    }

    async fn test_data_sources(&self) -> Result<(), String> {
        for name in &self.data_sources {
            let driver = self.driver(name).await?;
            driver
                .test_connection()
                .await
                .map_err(|e| format!("{name}: {e}"))?;
        }
        Ok(())
    }

    async fn test_orchestrator(&self) -> Result<(), String> {
        match &self.orchestrator {
            Some(orchestrator) => orchestrator
                .test_connections()
                .await
                .map_err(|e| format!("orchestrator: {e}")),
            None => Ok(()),
        }
    }
}

#[async_trait]
impl HealthService for DriverHealthService {
    /// Node.js probes only the `default` data source here and leaves a
    /// `todo: test other data sources`; readiness that ignores a configured
    /// source would report `HEALTH` while queries against it fail, so every
    /// data source is probed. Recorded in `MIGRATION.md`.
    async fn readiness(&self) -> Result<(), String> {
        if !self.standalone {
            return Ok(());
        }

        self.test_data_sources().await?;
        self.test_orchestrator().await
    }

    async fn liveness(&self) -> Result<(), String> {
        self.test_data_sources().await?;
        self.test_orchestrator().await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use cubedriver::{DriverConfig, DriverError, QueryOptions, QueryResult};
    use cubeorch::OrchError;
    use serde_json::Value;

    use super::*;

    /// A driver that only answers `test_connection`, either way.
    struct FakeDriver {
        config: DriverConfig,
        reachable: bool,
    }

    #[async_trait]
    impl Driver for FakeDriver {
        fn config(&self) -> &DriverConfig {
            &self.config
        }

        async fn test_connection(&self) -> cubedriver::Result<()> {
            if self.reachable {
                Ok(())
            } else {
                Err(DriverError::Connection {
                    pool_name: "default".to_string(),
                    message: "connection refused".to_string(),
                })
            }
        }

        async fn query(
            &self,
            _sql: &str,
            _params: &[Value],
            _options: &QueryOptions,
        ) -> cubedriver::Result<QueryResult> {
            unreachable!("the probes never run a query")
        }
    }

    /// Counts how many drivers it built, to prove they are reused.
    fn factory(reachable: bool, built: Arc<AtomicUsize>) -> DriverFactory {
        Arc::new(move |_data_source: String| {
            let built = Arc::clone(&built);
            Box::pin(async move {
                built.fetch_add(1, Ordering::SeqCst);
                Ok(Arc::new(FakeDriver {
                    config: DriverConfig::default(),
                    reachable,
                }) as Arc<dyn Driver>)
            })
        })
    }

    fn failing_factory() -> DriverFactory {
        Arc::new(|data_source: String| {
            Box::pin(async move {
                Err(OrchError::Driver(format!(
                    "CUBEJS_DB_TYPE is not set for the {data_source} data source"
                )))
            })
        })
    }

    fn service(factory: DriverFactory) -> DriverHealthService {
        DriverHealthService::new(vec!["default".to_string()], factory, None, true)
    }

    #[tokio::test]
    async fn a_reachable_data_source_is_healthy() {
        let service = service(factory(true, Arc::new(AtomicUsize::new(0))));

        assert!(service.readiness().await.is_ok());
        assert!(service.liveness().await.is_ok());
    }

    /// The defect this replaced: the binary wired a service that always
    /// answered healthy, so both probes returned 200 with the database down.
    #[tokio::test]
    async fn an_unreachable_data_source_is_reported_down() {
        let service = service(factory(false, Arc::new(AtomicUsize::new(0))));

        let error = service.readiness().await.expect_err("readiness must fail");
        assert!(error.contains("default"), "{error}");
        assert!(error.contains("connection refused"), "{error}");
        assert!(service.liveness().await.is_err());
    }

    #[tokio::test]
    async fn a_driver_that_cannot_be_built_is_reported_down() {
        let service = service(failing_factory());

        let error = service.liveness().await.expect_err("liveness must fail");
        assert!(error.contains("CUBEJS_DB_TYPE"), "{error}");
    }

    #[tokio::test]
    async fn the_driver_is_built_once_and_reused_across_probes() {
        let built = Arc::new(AtomicUsize::new(0));
        let service = service(factory(true, Arc::clone(&built)));

        for _ in 0..5 {
            service.liveness().await.expect("healthy");
        }

        assert_eq!(built.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn every_configured_data_source_is_probed() {
        let built = Arc::new(AtomicUsize::new(0));
        let service = DriverHealthService::new(
            vec!["default".to_string(), "warehouse".to_string()],
            factory(true, Arc::clone(&built)),
            None,
            true,
        );

        service.readiness().await.expect("healthy");
        assert_eq!(built.load(Ordering::SeqCst), 2);
    }

    /// An API-only server does not own the orchestrator, so readiness says
    /// only that the process is up, as `standalone` does in Node.js.
    #[tokio::test]
    async fn readiness_skips_the_database_when_the_server_is_not_standalone() {
        let built = Arc::new(AtomicUsize::new(0));
        let service = DriverHealthService::new(
            vec!["default".to_string()],
            factory(false, Arc::clone(&built)),
            None,
            false,
        );

        assert!(service.readiness().await.is_ok());
        assert_eq!(built.load(Ordering::SeqCst), 0);
        // Liveness still probes.
        assert!(service.liveness().await.is_err());
    }

    #[tokio::test]
    async fn an_empty_data_source_list_still_probes_the_default_one() {
        let built = Arc::new(AtomicUsize::new(0));
        let service =
            DriverHealthService::new(Vec::new(), factory(true, Arc::clone(&built)), None, true);

        service.liveness().await.expect("healthy");
        assert_eq!(built.load(Ordering::SeqCst), 1);
    }
}
