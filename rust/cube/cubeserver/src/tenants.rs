//! Per-tenant runtimes and runtime model reload.
//!
//! `cube.js` let a deployment decide, per request, which model to compile and
//! which database to talk to (`contextToAppId`, `repositoryFactory`,
//! `driverFactory`). With JavaScript configuration gone, `cubeconfig` resolves
//! the same thing declaratively from the security context, and this registry
//! turns a resolved tenant into the services that answer its requests.
//!
//! A deployment with no `tenants:` block resolves to one tenant, so the
//! single-tenant path is the multi-tenant path with one entry.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use cubeconfig::{CubeConfig, ResolvedTenant};
use cubeplanner::Dialect;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::error::ApiError;
use crate::graphql_adapter::GraphQLSchemaCache;
use crate::meta_adapter::{newest_mtime, ModelMetaService};
use crate::orchestrator_adapter::{orchestrator, OrchestratedQueryService};
use crate::planner_adapter::PlannerQueryService;
use crate::services::{MetaServiceRef, QueryServiceRef};

/// Everything that answers one tenant's requests, compiled from its model.
///
/// A reload builds a new value and swaps it in, so a request that already
/// holds one keeps serving from the model it started with.
pub struct TenantRuntime {
    pub tenant: ResolvedTenant,
    pub meta: MetaServiceRef,
    pub query: QueryServiceRef,
    pub graphql: Arc<GraphQLSchemaCache>,
    /// Incremented on every reload, so a caller can tell versions apart.
    pub generation: u64,
    /// The newest modification time seen in the model directory when this
    /// runtime was compiled.
    model_mtime: Option<SystemTime>,
}

impl std::fmt::Debug for TenantRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TenantRuntime")
            .field("app_id", &self.tenant.app_id)
            .field("model_path", &self.tenant.model_path)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// The dialect each configured data source is planned in, by data source
/// name, and the data sources whose type has no dialect, with the reason.
///
/// A data source without a type is left out of both: it is planned in the
/// deployment's default dialect.
pub fn data_source_dialects(
    config: &CubeConfig,
) -> (HashMap<String, Dialect>, Vec<(String, String)>) {
    let mut dialects = HashMap::new();
    let mut unplannable = Vec::new();
    for (name, data_source) in &config.data_sources {
        if data_source.db_type.trim().is_empty() {
            continue;
        }
        match Dialect::for_db_type(&data_source.db_type) {
            Ok(dialect) => {
                dialects.insert(name.clone(), dialect);
            }
            Err(err) => unplannable.push((name.clone(), err.message())),
        }
    }
    (dialects, unplannable)
}

/// Builds tenant runtimes on demand and keeps them until a reload.
pub struct TenantRegistry {
    config: Arc<CubeConfig>,
    /// The dialect of a data source that does not declare its own type.
    dialect: Dialect,
    /// Why the default dialect cannot be used, when the deployment's
    /// `CUBEJS_DB_TYPE` has none: a tenant that falls back to it is refused
    /// rather than planned in a dialect its database does not speak.
    default_dialect_error: Option<String>,
    db_type: Option<String>,
    planner_workers: usize,
    dev_mode: bool,
    runtimes: RwLock<HashMap<String, Arc<TenantRuntime>>>,
    generation: AtomicU64,
}

impl std::fmt::Debug for TenantRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TenantRegistry")
            .field("dialect", &self.dialect)
            .field("planner_workers", &self.planner_workers)
            .finish_non_exhaustive()
    }
}

impl TenantRegistry {
    pub fn new(
        config: Arc<CubeConfig>,
        dialect: Dialect,
        db_type: Option<String>,
        planner_workers: usize,
        dev_mode: bool,
    ) -> Self {
        Self {
            config,
            dialect,
            default_dialect_error: None,
            db_type,
            planner_workers,
            dev_mode,
            runtimes: RwLock::new(HashMap::new()),
            generation: AtomicU64::new(0),
        }
    }

    /// Refuses to plan for a tenant whose data source falls back to the
    /// default dialect, because the deployment's database type has none.
    pub fn with_unplannable_default(mut self, reason: impl Into<String>) -> Self {
        self.default_dialect_error = Some(reason.into());
        self
    }

    /// The dialect and database type a data source is planned in: its own
    /// `type` when `cube.yml` declares one, the deployment's otherwise. A type
    /// without a dialect is an error, never a silent fallback.
    fn plan_target(&self, data_source: &str) -> Result<(Dialect, Option<String>), String> {
        match self.config.data_sources.get(data_source) {
            Some(source) if !source.db_type.trim().is_empty() => {
                Dialect::for_db_type(&source.db_type)
                    .map(|dialect| (dialect, Some(source.db_type.clone())))
                    .map_err(|e| e.message())
            }
            _ => match &self.default_dialect_error {
                Some(reason) => Err(reason.clone()),
                None => Ok((self.dialect, self.db_type.clone())),
            },
        }
    }

    /// The runtime for `security_context`, compiling it on first use.
    pub async fn resolve(&self, security_context: &Value) -> Result<Arc<TenantRuntime>, ApiError> {
        let tenant = self
            .config
            .for_security_context(security_context)
            .map_err(|e| ApiError::forbidden(e.to_string()))?;

        if let Some(runtime) = self.runtimes.read().await.get(&tenant.app_id) {
            return Ok(runtime.clone());
        }

        // Compile outside the write lock so a slow model does not block other
        // tenants, then insert only if nobody else got there first.
        let compiled = Arc::new(self.compile(tenant).await?);

        let mut runtimes = self.runtimes.write().await;
        Ok(runtimes
            .entry(compiled.tenant.app_id.clone())
            .or_insert(compiled)
            .clone())
    }

    /// Compiles a tenant's model and builds its services.
    async fn compile(&self, tenant: ResolvedTenant) -> Result<TenantRuntime, ApiError> {
        let model_path = tenant.model_path.clone();
        let generation = self.generation.fetch_add(1, Ordering::SeqCst);

        // The tenant's `COMPILE_CONTEXT`, so its templates render the same way
        // for `/v1/meta` and for planning.
        let context = cubemodel::TemplateContext {
            compile_context: tenant.compile_context.clone(),
            variables: serde_json::Value::Object(Default::default()),
        };

        let meta = ModelMetaService::load_with_context(&model_path, self.dev_mode, context.clone())
            .map_err(|e| {
                ApiError::new(
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    format!(
                        "The data model of tenant {} did not compile: {}",
                        tenant.app_id, e
                    ),
                )
            })?;

        let (dialect, db_type) = self.plan_target(&tenant.data_source).map_err(|reason| {
            ApiError::new(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                format!(
                    "The data source {} of tenant {} cannot be planned: {}",
                    tenant.data_source, tenant.app_id, reason
                ),
            )
        })?;

        // A cube that declares its own data source is planned in that one's
        // dialect; the rest in the tenant's.
        let (by_data_source, _) = data_source_dialects(&self.config);
        let planner = Arc::new(
            PlannerQueryService::load_with_dialects(
                &model_path,
                dialect,
                by_data_source,
                self.planner_workers,
                context,
            )
            .map_err(|e| {
                ApiError::new(
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    format!(
                        "The data model of tenant {} cannot be planned: {}",
                        tenant.app_id,
                        e.message()
                    ),
                )
            })?,
        );

        // The cache prefix is the orchestrator id, so two tenants never share
        // a cached result even when they share a model.
        let query = Arc::new(OrchestratedQueryService::new(
            planner,
            orchestrator(&tenant.orchestrator_id),
            tenant.data_source.clone(),
            db_type,
            self.dev_mode,
        ));

        Ok(TenantRuntime {
            model_mtime: newest_mtime(&model_path),
            tenant,
            meta: Arc::new(meta),
            query,
            graphql: Arc::new(GraphQLSchemaCache::default()),
            generation,
        })
    }

    /// Recompiles every live tenant whose model changed on disk.
    ///
    /// A tenant whose model no longer compiles keeps serving the previous one
    /// and the error is reported, because dropping a working model over a
    /// typo would take the deployment down.
    pub async fn reload_changed(&self) -> Vec<(String, ApiError)> {
        let live: Vec<Arc<TenantRuntime>> = self.runtimes.read().await.values().cloned().collect();

        let mut failures = Vec::new();
        for runtime in live {
            let current = newest_mtime(&runtime.tenant.model_path);
            if current == runtime.model_mtime {
                continue;
            }

            match self.compile(runtime.tenant.clone()).await {
                Ok(compiled) => {
                    let app_id = compiled.tenant.app_id.clone();
                    self.runtimes
                        .write()
                        .await
                        .insert(app_id.clone(), Arc::new(compiled));
                    tracing::info!(tenant = %app_id, "reloaded the data model");
                }
                Err(err) => {
                    tracing::error!(
                        tenant = %runtime.tenant.app_id,
                        error = %err.body.error,
                        "the data model did not reload; keeping the previous one"
                    );
                    failures.push((runtime.tenant.app_id.clone(), err));
                }
            }
        }

        failures
    }

    /// Recompiles every live tenant, whether or not the files changed.
    pub async fn reload_all(&self) -> Vec<(String, ApiError)> {
        // Clearing the recorded time makes `reload_changed` see each as stale.
        {
            let mut runtimes = self.runtimes.write().await;
            for runtime in runtimes.values_mut() {
                *runtime = Arc::new(TenantRuntime {
                    tenant: runtime.tenant.clone(),
                    meta: runtime.meta.clone(),
                    query: runtime.query.clone(),
                    graphql: runtime.graphql.clone(),
                    generation: runtime.generation,
                    model_mtime: None,
                });
            }
        }

        self.reload_changed().await
    }

    /// How many tenants are compiled right now.
    pub async fn len(&self) -> usize {
        self.runtimes.read().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }

    /// Starts the loop that reloads a model after its files change.
    pub fn spawn_reload_loop(self: &Arc<Self>, interval: Duration) {
        let registry = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                registry.reload_changed().await;
            }
        });
    }
}
