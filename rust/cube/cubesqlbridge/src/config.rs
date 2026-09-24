//! Starting the SQL API.
//!
//! This is what `packages/cubejs-backend-native/src/config.rs` does, minus
//! Neon: it builds cubesql's service graph, replaces the two services that
//! used to call into JavaScript with [`RustTransport`] and
//! [`RustSqlAuthService`], and runs the Postgres-protocol server.

use std::sync::Arc;

use cubeauth::Authenticator;
use cubesql::config::processing_loop::ShutdownMode;
use cubesql::config::{Config, CubeServices};
use cubesql::sql::SqlAuthService;
use cubesql::transport::TransportService;
use cubesql::CubeError;

use crate::auth::{RustSqlAuthService, SqlAuthConfig};
use crate::executor::{NotConfiguredExecutor, QueryExecutor};
use crate::model_source::ModelSource;
use crate::planner::DialectMap;
use crate::transport::{RustTransport, RustTransportOptions};

/// Everything [`start_sql_api`] needs.
pub struct SqlApiConfig {
    /// Where the data model is read from.
    pub model_source: ModelSource,
    /// The address the Postgres-protocol server listens on, e.g.
    /// `0.0.0.0:15432`. `None` starts the services without a listener, which
    /// is what a test that drives the transport directly wants.
    pub postgres_bind_address: Option<String>,
    /// How many planner threads to run.
    pub planner_threads: usize,
    /// The dialect each data source renders in.
    pub dialects: DialectMap,
    /// Expose members that are not public. The dev-mode branch of
    /// `filterVisibleItemsInMeta`.
    pub include_hidden_members: bool,
    /// Runs the queries the planner produces.
    pub executor: Arc<dyn QueryExecutor>,
    /// `CUBEJS_SQL_USER` / `CUBEJS_SQL_PASSWORD` / `CUBEJS_SQL_SUPER_USER`.
    pub auth_config: SqlAuthConfig,
    /// When set, a Cube JWT is also accepted as the SQL password.
    pub authenticator: Option<Arc<Authenticator>>,
}

impl std::fmt::Debug for SqlApiConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqlApiConfig")
            .field("model_source", &self.model_source)
            .field("postgres_bind_address", &self.postgres_bind_address)
            .field("planner_threads", &self.planner_threads)
            .field("dialects", &self.dialects)
            .field("include_hidden_members", &self.include_hidden_members)
            .field("executor", &self.executor)
            .finish()
    }
}

impl SqlApiConfig {
    /// A configuration reading the model from `model_source` and everything
    /// else from the environment, with no query executor yet.
    pub fn from_env(model_source: ModelSource) -> Self {
        Self {
            model_source,
            // `CUBEJS_PG_SQL_PORT=false` is how Node spells "no SQL API",
            // so it is not a port.
            postgres_bind_address: std::env::var("CUBEJS_PG_SQL_PORT")
                .ok()
                .filter(|port| !port.is_empty() && port != "false")
                .map(|port| format!("0.0.0.0:{port}")),
            planner_threads: default_planner_threads(),
            dialects: DialectMap::default(),
            include_hidden_members: false,
            executor: Arc::new(NotConfiguredExecutor::new()),
            auth_config: SqlAuthConfig::from_env(),
            authenticator: None,
        }
    }

    pub fn with_bind_address(mut self, address: impl Into<String>) -> Self {
        self.postgres_bind_address = Some(address.into());
        self
    }

    pub fn with_executor(mut self, executor: Arc<dyn QueryExecutor>) -> Self {
        self.executor = executor;
        self
    }

    pub fn with_auth_config(mut self, auth_config: SqlAuthConfig) -> Self {
        self.auth_config = auth_config;
        self
    }

    pub fn with_authenticator(mut self, authenticator: Arc<Authenticator>) -> Self {
        self.authenticator = Some(authenticator);
        self
    }

    pub fn with_dialects(mut self, dialects: DialectMap) -> Self {
        self.dialects = dialects;
        self
    }
}

fn default_planner_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8)
}

/// The running SQL API: the service graph plus the handles of its loops.
pub struct SqlApi {
    services: CubeServices,
    transport: Arc<RustTransport>,
    auth: Arc<RustSqlAuthService>,
}

impl std::fmt::Debug for SqlApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqlApi")
            .field("transport", &self.transport)
            .field("auth", &self.auth)
            .finish()
    }
}

impl SqlApi {
    pub fn services(&self) -> &CubeServices {
        &self.services
    }

    pub fn transport(&self) -> &Arc<RustTransport> {
        &self.transport
    }

    pub fn auth(&self) -> &Arc<RustSqlAuthService> {
        &self.auth
    }

    /// Starts the Postgres-protocol listener and returns once it is running.
    pub async fn spawn_processing_loops(
        &self,
    ) -> Result<Vec<tokio::task::JoinHandle<Result<(), CubeError>>>, CubeError> {
        self.services.spawn_processing_loops().await
    }

    /// Runs until the listener stops.
    pub async fn wait_processing_loops(&self) -> Result<(), CubeError> {
        self.services.wait_processing_loops().await
    }

    pub async fn stop_processing_loops(&self, mode: ShutdownMode) -> Result<(), CubeError> {
        self.services.stop_processing_loops(mode).await
    }
}

/// Builds the SQL API's services with the pure-Rust transport and auth service.
///
/// The Postgres listener is not started yet; call
/// [`SqlApi::spawn_processing_loops`] or [`SqlApi::wait_processing_loops`].
pub async fn start_sql_api(config: SqlApiConfig) -> Result<SqlApi, CubeError> {
    let auth = {
        let service = RustSqlAuthService::new(config.auth_config.clone());
        Arc::new(match &config.authenticator {
            Some(authenticator) => service.with_authenticator(authenticator.clone()),
            None => service,
        })
    };

    let transport = Arc::new(RustTransport::new(RustTransportOptions {
        model_source: config.model_source.clone(),
        planner_threads: config.planner_threads,
        dialects: config.dialects.clone(),
        include_hidden_members: config.include_hidden_members,
        executor: config.executor.clone(),
        auth_config: config.auth_config.clone(),
        auth_service: Some(auth.clone()),
    })?);

    let bind_address = config.postgres_bind_address.clone();
    let cube_config = Config::default().update_config(move |mut c| {
        c.postgres_bind_address = bind_address;
        c
    });

    // `configure` registers cubesql's own defaults, including the HTTP
    // transport and the `CUBESQL_CUBE_TOKEN` auth service. Registering over
    // them afterwards is what the Node bridge does too.
    cube_config.configure().await;

    let injector = cube_config.injector();

    let transport_to_register = transport.clone();
    injector
        .register_typed::<dyn TransportService, _, _, _>(|_| async move { transport_to_register })
        .await;

    let auth_to_register = auth.clone();
    injector
        .register_typed::<dyn SqlAuthService, _, _, _>(|_| async move { auth_to_register })
        .await;

    Ok(SqlApi {
        services: cube_config.cube_services().await,
        transport,
        auth,
    })
}
