use std::path::{Path, PathBuf};

use cubeconfig::{ApiScope, ConfigError, CubeConfig, Env, LogLevel};
use serde_json::json;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn empty_env() -> Env {
    Env::new()
}

fn load_full(env: &Env) -> CubeConfig {
    CubeConfig::load_with_env(fixture("full"), env).expect("full fixture must load")
}

#[test]
fn loads_a_full_example_configuration() {
    let config = load_full(&empty_env());

    assert_eq!(config.log_level, LogLevel::Warn);
    assert!(!config.telemetry);
    assert_eq!(config.model_path, PathBuf::from("model"));
    assert_eq!(config.pre_aggregations_schema, "prod_pre_aggregations");
    assert_eq!(config.app_id, "analytics");
    assert!(config.is_multi_tenant());

    assert_eq!(config.api.base_path, "/cube");
    assert_eq!(
        config.api.default_scopes,
        vec![ApiScope::Meta, ApiScope::Data, ApiScope::Graphql]
    );
    assert_eq!(config.api.max_request_size, 2 * 1024 * 1024);
    assert!(config.api.cors.enabled);
    assert_eq!(
        config.api.cors.origin,
        vec!["https://app.example.com", "https://admin.example.com"]
    );
    assert!(config.api.cors.credentials);
    assert_eq!(config.api.cors.max_age, Some(600));

    assert_eq!(
        config.data_sources.keys().collect::<Vec<_>>(),
        vec!["default", "warehouse"]
    );

    let default = config.data_source("default").unwrap();
    assert_eq!(default.db_type, "postgres");
    // `{ env: ..., default: ... }` falls back to the declared default.
    assert_eq!(default.host.as_deref(), Some("localhost"));
    assert_eq!(default.user.as_deref(), Some("cube"));
    // No env, no default => the field stays unset rather than becoming "".
    assert_eq!(default.password, None);
    assert_eq!(default.port, Some(5432));
    assert!(default.ssl);
    assert_eq!(default.max_pool, Some(8));

    let warehouse = config.data_source("warehouse").unwrap();
    assert_eq!(warehouse.db_type, "snowflake");
    assert_eq!(warehouse.export_bucket.as_deref(), Some("s3://cube-export"));
    assert_eq!(warehouse.options.get("warehouse").unwrap(), "COMPUTE_WH");
    assert_eq!(warehouse.options.get("role").unwrap(), "ANALYST");
    assert!(!warehouse.ssl);

    assert!(config.scheduled_refresh.enabled);
    assert_eq!(config.scheduled_refresh.interval_seconds, 60);
    assert_eq!(
        config.scheduled_refresh.timezones,
        vec!["UTC", "America/Los_Angeles"]
    );
    assert_eq!(config.scheduled_refresh.concurrency, Some(4));
    assert_eq!(config.scheduled_refresh.batch_size, 2);
    assert_eq!(config.scheduled_refresh.contexts.len(), 3);
}

#[test]
fn accepts_cube_yaml_as_well_as_cube_yml() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("cube.yaml"),
        "data_sources:\n  default:\n    type: postgres\n",
    )
    .unwrap();

    let config = CubeConfig::load_with_env(dir.path(), &empty_env()).unwrap();
    assert_eq!(config.data_source("default").unwrap().db_type, "postgres");
}

#[test]
fn rejects_a_directory_holding_both_file_names() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("cube.yml"), "{}").unwrap();
    std::fs::write(dir.path().join("cube.yaml"), "{}").unwrap();

    let error = CubeConfig::load_with_env(dir.path(), &empty_env()).unwrap_err();
    assert!(
        error.to_string().contains("keep only one"),
        "unexpected error: {error}"
    );
}

#[test]
fn reports_a_missing_configuration_file() {
    let dir = tempfile::tempdir().unwrap();
    let error = CubeConfig::load_with_env(dir.path(), &empty_env()).unwrap_err();
    assert!(matches!(error, ConfigError::NotFound { .. }));
    assert!(error
        .to_string()
        .starts_with("no cube.yml or cube.yaml found in"));
}

// ---------------------------------------------------------------- environment

#[test]
fn environment_wins_over_the_file() {
    let env = Env::new()
        .with("CUBEJS_LOG_LEVEL", "trace")
        .with("CUBEJS_TELEMETRY", "true")
        .with("CUBEJS_SCHEMA_PATH", "/srv/model")
        .with("CUBEJS_PRE_AGGREGATIONS_SCHEMA", "staging_pre_aggregations")
        .with("CUBEJS_APP", "from-env")
        .with("CUBEJS_API_BASE_PATH", "/api")
        .with("CUBEJS_DEFAULT_API_SCOPES", "meta,jobs")
        .with("CUBEJS_MAX_REQUEST_SIZE", "1mb")
        .with("CUBEJS_SCHEDULED_REFRESH_TIMEZONES", "Europe/Berlin")
        .with("CUBEJS_SCHEDULED_REFRESH_BATCH_SIZE", "10");

    let config = load_full(&env);

    assert_eq!(config.log_level, LogLevel::Trace);
    assert!(config.telemetry);
    assert_eq!(config.model_path, PathBuf::from("/srv/model"));
    assert_eq!(config.pre_aggregations_schema, "staging_pre_aggregations");
    assert_eq!(config.app_id, "from-env");
    assert_eq!(config.api.base_path, "/api");
    assert_eq!(
        config.api.default_scopes,
        vec![ApiScope::Meta, ApiScope::Jobs]
    );
    assert_eq!(config.api.max_request_size, 1024 * 1024);
    assert_eq!(config.scheduled_refresh.timezones, vec!["Europe/Berlin"]);
    assert_eq!(config.scheduled_refresh.batch_size, 10);
}

#[test]
fn environment_overrides_data_source_connection_fields() {
    let env = Env::new()
        .with("CUBEJS_DB_HOST", "db.internal")
        .with("CUBEJS_DB_PASS", "from-env")
        .with("CUBEJS_DB_PORT", "6432")
        .with("CUBEJS_DB_SSL", "false")
        // Non-default data sources use the CUBEJS_DS_<NAME>_* form.
        .with("CUBEJS_DS_WAREHOUSE_DB_USER", "SVC_CUBE");

    let config = load_full(&env);

    let default = config.data_source("default").unwrap();
    assert_eq!(default.host.as_deref(), Some("db.internal"));
    assert_eq!(default.password.as_deref(), Some("from-env"));
    assert_eq!(default.port, Some(6432));
    assert!(!default.ssl);

    let warehouse = config.data_source("warehouse").unwrap();
    assert_eq!(warehouse.user.as_deref(), Some("SVC_CUBE"));
    // The `default`-scoped variable must not leak into another data source.
    assert_eq!(warehouse.host, None);
}

#[test]
fn env_references_read_from_the_environment() {
    let env = Env::new().with("SNOWFLAKE_USER", "SVC");
    let config = load_full(&env);
    assert_eq!(
        config.data_source("warehouse").unwrap().user.as_deref(),
        Some("SVC")
    );
}

#[test]
fn from_env_builds_a_single_tenant_configuration() {
    let env = Env::new()
        .with("CUBEJS_DB_TYPE", "postgres")
        .with("CUBEJS_DB_HOST", "localhost")
        .with("CUBEJS_DB_NAME", "analytics")
        .with("CUBEJS_DB_USER", "cube")
        .with("CUBEJS_DB_PASS", "secret");

    let config = CubeConfig::from_env_with(&env).unwrap();

    assert!(!config.is_multi_tenant());
    assert_eq!(config.app_id, "STANDALONE");
    assert_eq!(config.model_path, PathBuf::from("model"));
    assert_eq!(config.api.base_path, "/cube");
    assert_eq!(config.api.default_scopes, ApiScope::defaults());
    assert!(config.telemetry);

    let default = config.data_source("default").unwrap();
    assert_eq!(default.db_type, "postgres");
    assert_eq!(default.database.as_deref(), Some("analytics"));

    let tenant = config
        .for_security_context(&json!({ "user": "bob" }))
        .unwrap();
    assert_eq!(tenant.app_id, "STANDALONE");
    assert_eq!(tenant.orchestrator_id, "STANDALONE");
    assert_eq!(tenant.data_source, "default");
}

#[test]
fn cubejs_datasources_declares_extra_data_sources() {
    let env = Env::new()
        .with("CUBEJS_DB_TYPE", "postgres")
        .with("CUBEJS_DATASOURCES", "default, reporting")
        .with("CUBEJS_DS_REPORTING_DB_TYPE", "clickhouse")
        .with("CUBEJS_DS_REPORTING_DB_HOST", "ch.internal");

    let config = CubeConfig::from_env_with(&env).unwrap();
    assert_eq!(
        config.data_sources.keys().collect::<Vec<_>>(),
        vec!["default", "reporting"]
    );
    let reporting = config.data_source("reporting").unwrap();
    assert_eq!(reporting.db_type, "clickhouse");
    assert_eq!(reporting.host.as_deref(), Some("ch.internal"));
}

#[test]
fn load_or_env_falls_back_when_there_is_no_file() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::new().with("CUBEJS_DB_TYPE", "postgres");
    let config = CubeConfig::load_or_env(dir.path(), &env).unwrap();
    assert_eq!(config.data_source("default").unwrap().db_type, "postgres");
}
