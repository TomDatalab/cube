use std::sync::Arc;

use cubeauth::AuthConfig;
use cubeserver::auth_adapter::CubeAuthService;
use cubeserver::health_adapter::DriverHealthService;
use cubeserver::meta_adapter::ModelMetaService;
use cubeserver::planner_adapter::PlannerQueryService;
use cubeserver::services::{
    AlwaysHealthy, HealthServiceRef, MetaServiceRef, QueryServiceRef, UnimplementedQueryService,
};
use cubeserver::{build_app, AppState, ServerConfig};
use tracing_subscriber::EnvFilter;

mod bootstrap;

/// Worker stack size.
///
/// The SQL API's planner (DataFusion plus the e-graph rewriter) recurses
/// deeply enough to overflow tokio's 2 MiB default while planning an ordinary
/// grouped query. cubesql's own harness runs at 8 MiB
/// (`cubesql/src/compile/test/mod.rs:1460`), so the server matches it, and
/// `CUBEJS_WORKER_STACK_SIZE` raises it for a model that needs more.
const DEFAULT_WORKER_STACK_SIZE: usize = 8 * 1024 * 1024;

fn worker_stack_size() -> usize {
    std::env::var("CUBEJS_WORKER_STACK_SIZE")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|size| *size >= DEFAULT_WORKER_STACK_SIZE)
        .unwrap_or(DEFAULT_WORKER_STACK_SIZE)
}

const USAGE: &str = "\
Usage: cube-server [--version | --help]

Serves the REST, GraphQL, WebSocket and SQL APIs and the Playground.
Configuration comes from CUBEJS_* environment variables and an optional
cube.yml in the working directory; the data model is read from
CUBEJS_SCHEMA_PATH (default: model).";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The server takes no arguments; anything else is a mistake worth
    // stopping on rather than silently starting a server.
    if let Some(arg) = std::env::args().nth(1) {
        match arg.as_str() {
            "--version" | "-V" => println!("cube-server {}", env!("CARGO_PKG_VERSION")),
            "--help" | "-h" => println!("{USAGE}"),
            other => {
                eprintln!("cube-server: unknown argument `{other}`\n\n{USAGE}");
                std::process::exit(2);
            }
        }
        return Ok(());
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(worker_stack_size())
        .build()?;

    runtime.block_on(run())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    // `cube.yml` replaces `cube.js`; a missing file means environment-only
    // configuration, which is what a single data source needs.
    let cube_config =
        match cubeconfig::CubeConfig::load_or_env(".", &cubeconfig::Env::from_process()) {
            Ok(config) => config,
            Err(err) => {
                // Print the message, not the debug form: it names the file and key.
                eprintln!("Configuration error: {err}");
                std::process::exit(1);
            }
        };

    // Drivers read `CUBEJS_*` variables; this lets them find a data source
    // only `cube.yml` declares.
    cubeserver::orchestrator_adapter::declare_data_sources(&cube_config);

    // The environment wins over the file, so it is applied last.
    let config = ServerConfig::default()
        .with_cube_config(&cube_config)
        .overlay_env()?;

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new(config.log_level.clone())),
        )
        .init();

    let auth_config = AuthConfig::from_env()?;
    // The Playground bootstraps by fetching a token from an unauthenticated
    // route, so mounting it where authentication is enforced would hand the
    // API away to anyone who can reach the port. Node.js has the same
    // property and only runs its dev server outside production.
    let mut config = config;
    if config.playground_path.is_some() && auth_config.enforce_security_checks {
        eprintln!(
            "⚠️  The Playground is not mounted: NODE_ENV=production enforces \
             authentication, and the Playground hands out a token without one."
        );
        config.playground_path = None;
    }
    let config = config;

    let auth = CubeAuthService::new(auth_config);
    auth.prefetch_jwks().await;

    // A model that does not compile must not take the server down silently:
    // the error is reported and `/v1/meta` answers 503 until it is fixed.
    let meta: MetaServiceRef = match ModelMetaService::load(&config.schema_path, config.dev_mode) {
        Ok(service) => {
            println!("📦 Data model loaded from {}", config.schema_path);
            Arc::new(service)
        }
        Err(err) => {
            eprintln!(
                "Failed to compile the data model in {}:\n{}",
                config.schema_path, err
            );
            Arc::new(bootstrap::NotConfiguredMeta)
        }
    };

    let mut sql_api: Option<Arc<cubesqlbridge::SqlApi>> = None;
    // Kept so `/readyz` and `/livez` can test the orchestrator's store, the
    // way `testOrchestratorConnections` does in Node.js.
    let mut health_orchestrator: Option<Arc<cubeorch::QueryOrchestrator>> = None;

    // `/v1/sql` and `/v1/dry-run` only need the planner; `/v1/load` also goes
    // through the orchestrator, and the SQL API shares it.
    // The dialect comes from the default data source, or `CUBEJS_DB_TYPE`;
    // Postgres only when neither names a type. A type the planner has no
    // dialect for refuses to start the planner: planning it as Postgres would
    // send the database SQL it does not speak.
    let db_type = cube_config
        .data_sources
        .get("default")
        .map(|ds| ds.db_type.clone())
        .filter(|db_type| !db_type.trim().is_empty())
        .or_else(|| {
            std::env::var("CUBEJS_DB_TYPE")
                .ok()
                .filter(|db_type| !db_type.trim().is_empty())
        });
    let default_dialect = match db_type.as_deref() {
        None => Ok(cubeplanner::Dialect::default()),
        Some(db_type) => cubeplanner::Dialect::for_db_type(db_type).map_err(|e| e.message()),
    };
    // Every other data source is planned in its own type's dialect.
    let (data_source_dialects, unplannable) =
        cubeserver::tenants::data_source_dialects(&cube_config);
    for (data_source, reason) in &unplannable {
        eprintln!("The {data_source} data source cannot be planned: {reason}");
    }
    let dialect = default_dialect.clone().unwrap_or_default();
    let planner_workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2);
    let planner = match &default_dialect {
        Ok(dialect) => PlannerQueryService::load_with_dialects(
            &config.schema_path,
            *dialect,
            data_source_dialects.clone(),
            planner_workers,
            cubemodel::TemplateContext::default(),
        )
        .map_err(|err| err.message()),
        Err(reason) => Err(reason.clone()),
    };
    let query: QueryServiceRef = match planner {
        Ok(service) => {
            println!("🧮 SQL planner ready ({dialect:?}, {planner_workers} workers)");

            // `/v1/load` goes through the orchestrator's cache and queue;
            // `/v1/sql` and `/v1/dry-run` only need the planner.
            let orchestrator = cubeserver::orchestrator_adapter::orchestrator(&cube_config.app_id);
            health_orchestrator = Some(orchestrator.clone());
            println!(
                "🗃️  Query orchestrator ready (cache prefix {})",
                cube_config.app_id
            );

            let planner = Arc::new(service);
            let orchestrated = Arc::new(
                cubeserver::orchestrator_adapter::OrchestratedQueryService::new(
                    planner.clone(),
                    orchestrator,
                    "default".to_string(),
                    db_type.clone(),
                    config.dev_mode,
                ),
            );

            // The SQL API shares the orchestrator, so a statement sent
            // over the Postgres protocol hits the same cache and queue as
            // the REST API.
            let executor = Arc::new(cubeserver::sql_api::OrchestratedExecutor::new(
                planner,
                orchestrated.api().clone(),
                "default".to_string(),
            ));
            // A data source the SQL API cannot render for would fall back
            // to the default dialect, so the SQL API does not start.
            if !unplannable.is_empty() {
                eprintln!(
                    "The SQL API did not start: {} data source(s) have no SQL dialect",
                    unplannable.len()
                );
            } else {
                match cubeserver::sql_api::start(
                    &config.schema_path,
                    dialect,
                    data_source_dialects.clone(),
                    planner_workers,
                    executor,
                )
                .await
                {
                    Ok(Some(api)) => {
                        println!("🐘 SQL API is listening on the Postgres protocol");
                        sql_api = Some(Arc::new(api));
                    }
                    Ok(None) => {}
                    Err(err) => eprintln!("The SQL API did not start: {err}"),
                }
            }

            orchestrated
        }
        Err(reason) => {
            eprintln!("SQL planning is unavailable: {reason}");
            Arc::new(UnimplementedQueryService)
        }
    };

    // Multi-tenancy: when `cube.yml` declares tenants, each security context
    // resolves to its own model, data source and cache prefix. A deployment
    // with no `tenants:` block resolves to one tenant, so the registry serves
    // both cases and a reload swaps the model, the planner and the GraphQL
    // schema together. Reloading only some of them would leave `/v1/meta`
    // describing members that queries cannot resolve.
    let cube_config = Arc::new(cube_config);
    let mut registry = cubeserver::tenants::TenantRegistry::new(
        cube_config.clone(),
        dialect,
        db_type.clone(),
        planner_workers,
        config.dev_mode,
    );
    if let Err(reason) = &default_dialect {
        registry = registry.with_unplannable_default(reason.clone());
    }
    let tenants = Arc::new(registry);
    if cube_config.has_tenants() {
        println!("🏘️  Multi-tenant mode: a model is compiled per tenant on first use");
    }

    // Runtime model reload: recompile after the files change. A model that
    // stops compiling keeps the previous one serving.
    if let Some(interval) = model_reload_interval(config.dev_mode) {
        tenants.spawn_reload_loop(interval);
        println!(
            "👀 Watching the data model, reloading at most every {}s",
            interval.as_secs()
        );
    }

    // `/readyz` and `/livez` test the configured data sources and the
    // orchestrator's store. With no planner there is no orchestrator and no
    // query path to speak of, so the probes only report the process is up.
    let health: HealthServiceRef = match &health_orchestrator {
        Some(orchestrator) => {
            let names: Vec<String> = cube_config.data_sources.keys().cloned().collect();
            println!(
                "🩺 Health probes test {} data source(s) and the orchestrator store",
                names.len().max(1)
            );
            Arc::new(DriverHealthService::new(
                names,
                cubeserver::orchestrator_adapter::driver_factory(),
                Some(orchestrator.clone()),
                // This server owns the orchestrator; there is no API-only
                // split yet, so readiness always probes.
                true,
            ))
        }
        None => Arc::new(AlwaysHealthy),
    };

    let ws_state = cubeserver::ws::WsState::default();

    let state = AppState {
        tenants: Some(tenants),
        config: Arc::new(config.clone()),
        query_config: Arc::new(cubequery::QueryConfig::from_env()?),
        auth: Arc::new(auth),
        meta,
        health: health.clone(),
        query,
        pre_aggregations: Arc::new(cubeserver::services::UnimplementedPreAggregationService),
        sql_conversion: match &sql_api {
            Some(api) => Arc::new(cubeserver::sql_api::SqlApiConversionService::new(
                api.clone(),
            )),
            None => Arc::new(cubeserver::services::UnimplementedSqlConversionService),
        },
        graphql: Default::default(),
        ws: ws_state.clone(),
    };

    // Re-runs live WebSocket subscriptions, like `processSubscriptions` in
    // the Node.js server.
    ws_state.spawn_subscription_loop(
        state.clone(),
        cubeserver::ws::handler::SUBSCRIPTION_INTERVAL,
    );

    let listener = tokio::net::TcpListener::bind(config.listen_address()).await?;
    println!(
        "🚀 Cube API server (Rust) is listening on {}",
        config.listen_address()
    );
    if let Some(path) = &config.playground_path {
        println!(
            "🎛️  Playground served at http://{}/ from {path}",
            config.listen_address()
        );
    }

    axum::serve(listener, build_app(state)).await?;

    Ok(())
}

/// How often the data model is checked for changes, or `None` to never check.
///
/// `CUBEJS_MODEL_RELOAD_INTERVAL` is the number of seconds; `0` turns it off.
/// It defaults to five seconds in dev mode and off otherwise, so a production
/// deployment reloads only when it is asked to.
fn model_reload_interval(dev_mode: bool) -> Option<std::time::Duration> {
    let seconds = match std::env::var("CUBEJS_MODEL_RELOAD_INTERVAL") {
        Ok(value) => value.parse::<u64>().ok()?,
        Err(_) if dev_mode => 5,
        Err(_) => 0,
    };

    (seconds > 0).then(|| std::time::Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_worker_stack_is_large_enough_for_the_sql_planner() {
        // A 2 MiB tokio default overflows while planning a grouped query, so
        // the floor is what cubesql's own harness uses.
        assert_eq!(DEFAULT_WORKER_STACK_SIZE, 8 * 1024 * 1024);

        temp_env::with_var("CUBEJS_WORKER_STACK_SIZE", None::<&str>, || {
            assert_eq!(worker_stack_size(), DEFAULT_WORKER_STACK_SIZE);
        });

        // A larger value is honoured.
        temp_env::with_var("CUBEJS_WORKER_STACK_SIZE", Some("33554432"), || {
            assert_eq!(worker_stack_size(), 32 * 1024 * 1024);
        });

        // A smaller one is not: it would reintroduce the overflow.
        temp_env::with_var("CUBEJS_WORKER_STACK_SIZE", Some("1024"), || {
            assert_eq!(worker_stack_size(), DEFAULT_WORKER_STACK_SIZE);
        });

        temp_env::with_var("CUBEJS_WORKER_STACK_SIZE", Some("not a number"), || {
            assert_eq!(worker_stack_size(), DEFAULT_WORKER_STACK_SIZE);
        });
    }
}
