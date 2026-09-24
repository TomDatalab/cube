//! Every validation failure must name the file (or the environment variable)
//! and the exact key, so an operator can fix it without reading Rust.

use cubeconfig::{ConfigError, CubeConfig, Env};

fn load_err(yaml: &str) -> ConfigError {
    load_err_with(yaml, Env::new())
}

fn load_err_with(yaml: &str, env: Env) -> ConfigError {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("cube.yml"), yaml).unwrap();
    CubeConfig::load_with_env(dir.path(), &env).expect_err("expected the config to be rejected")
}

const MINIMAL: &str = "data_sources:\n  default:\n    type: postgres\n";

#[test]
fn unknown_database_type_names_the_key_and_lists_the_valid_values() {
    let error = load_err("data_sources:\n  default:\n    type: mongodb\n");
    let message = error.to_string();
    assert!(
        message.contains("cube.yml: key `data_sources.default.type`"),
        "{message}"
    );
    assert!(message.contains("invalid value \"mongodb\""), "{message}");
    assert!(message.contains("unknown database type"), "{message}");
    assert!(message.contains("postgres"), "{message}");
}

#[test]
fn a_missing_database_type_says_which_variable_would_supply_it() {
    let error = load_err("data_sources:\n  reporting: {}\n");
    let message = error.to_string();
    assert!(
        message.contains("key `data_sources.reporting.type`"),
        "{message}"
    );
    assert!(
        message.contains("export CUBEJS_DS_REPORTING_DB_TYPE"),
        "{message}"
    );
}

#[test]
fn a_bad_port_names_the_key() {
    let error = load_err("data_sources:\n  default:\n    type: postgres\n    port: 99999\n");
    let message = error.to_string();
    assert!(
        message.contains("key `data_sources.default.port`"),
        "{message}"
    );
    assert!(message.contains("between 1 and 65535"), "{message}");
}

#[test]
fn a_bad_port_from_the_environment_names_the_variable() {
    let error = load_err_with(MINIMAL, Env::new().with("CUBEJS_DB_PORT", "not-a-port"));
    let message = error.to_string();
    assert!(
        message.contains("environment variable `CUBEJS_DB_PORT`"),
        "{message}"
    );
    assert!(message.contains("non-negative integer"), "{message}");
}

#[test]
fn an_unresolved_env_reference_is_reported_for_a_required_field() {
    let error = load_err("data_sources:\n  default:\n    type: { env: MY_DB_TYPE }\n");
    let message = error.to_string();
    assert!(
        message.contains("key `data_sources.default.type`"),
        "{message}"
    );
    assert!(
        message.contains("references environment variable `MY_DB_TYPE`, which is not set"),
        "{message}"
    );
}

#[test]
fn a_bad_boolean_names_the_key() {
    let error = load_err("data_sources:\n  default:\n    type: postgres\n    ssl: maybe\n");
    let message = error.to_string();
    assert!(
        message.contains("key `data_sources.default.ssl`"),
        "{message}"
    );
    assert!(message.contains("must be a boolean"), "{message}");
}

#[test]
fn an_unknown_top_level_key_is_rejected_with_the_file_name() {
    let error = load_err("data_sourcs:\n  default:\n    type: postgres\n");
    assert!(matches!(error, ConfigError::Yaml { .. }));
    let message = error.to_string();
    assert!(message.contains("cube.yml"), "{message}");
    assert!(message.contains("unknown field `data_sourcs`"), "{message}");
}

#[test]
fn an_unsupported_schema_version_is_rejected() {
    let error = load_err("version: 2\ndata_sources:\n  default:\n    type: postgres\n");
    let message = error.to_string();
    assert!(message.contains("key `version`"), "{message}");
    assert!(message.contains("only understands version 1"), "{message}");
}

#[test]
fn base_path_must_be_absolute() {
    let error = load_err(&format!("{MINIMAL}api:\n  base_path: cube\n"));
    let message = error.to_string();
    assert!(message.contains("key `api.base_path`"), "{message}");
    assert!(message.contains("must start with `/`"), "{message}");
}

#[test]
fn request_size_bounds_are_enforced() {
    let error = load_err(&format!("{MINIMAL}api:\n  max_request_size: 1kb\n"));
    let message = error.to_string();
    assert!(message.contains("key `api.max_request_size`"), "{message}");
    assert!(message.contains("between 100kb and 64mb"), "{message}");

    let error = load_err_with(MINIMAL, Env::new().with("CUBEJS_MAX_REQUEST_SIZE", "1tb"));
    assert!(
        error
            .to_string()
            .contains("environment variable `CUBEJS_MAX_REQUEST_SIZE`"),
        "{error}"
    );
}

#[test]
fn wildcard_cors_origin_cannot_be_combined_with_credentials() {
    let error = load_err(&format!(
        "{MINIMAL}api:\n  cors:\n    origin: [\"*\"]\n    credentials: true\n"
    ));
    let message = error.to_string();
    assert!(message.contains("key `api.cors.credentials`"), "{message}");
    assert!(message.contains("wildcard origin"), "{message}");
}

#[test]
fn an_unknown_api_scope_in_the_environment_is_rejected() {
    let error = load_err_with(
        MINIMAL,
        Env::new().with("CUBEJS_DEFAULT_API_SCOPES", "meta,admin"),
    );
    let message = error.to_string();
    assert!(
        message.contains("environment variable `CUBEJS_DEFAULT_API_SCOPES`"),
        "{message}"
    );
    assert!(message.contains("not an API scope"), "{message}");
}

#[test]
fn an_unknown_log_level_is_rejected() {
    let error = load_err_with(MINIMAL, Env::new().with("CUBEJS_LOG_LEVEL", "verbose"));
    let message = error.to_string();
    assert!(
        message.contains("environment variable `CUBEJS_LOG_LEVEL`"),
        "{message}"
    );
    assert!(
        message.contains("trace, debug, info, warn, error"),
        "{message}"
    );
}

#[test]
fn a_tenant_rule_needs_exactly_one_matcher() {
    let error = load_err(&format!(
        "{MINIMAL}tenants:\n  claim: tenant_id\n  rules:\n    - app_id: acme\n"
    ));
    let message = error.to_string();
    assert!(message.contains("key `tenants.rules[0]`"), "{message}");
    assert!(
        message.contains("one of `match`, `match_any` or `match_pattern`"),
        "{message}"
    );

    let error = load_err(&format!(
        "{MINIMAL}tenants:\n  claim: tenant_id\n  rules:\n    - match: a\n      match_pattern: \"a*\"\n"
    ));
    assert!(error.to_string().contains("use exactly one of"), "{error}");
}

#[test]
fn a_tenant_rule_pointing_at_an_undeclared_data_source_is_rejected() {
    let error = load_err(&format!(
        "{MINIMAL}tenants:\n  claim: tenant_id\n  rules:\n    - match: acme\n      data_source: warehouse\n"
    ));
    let message = error.to_string();
    assert!(
        message.contains("key `tenants.rules[0].data_source`"),
        "{message}"
    );
    assert!(message.contains("no such data source"), "{message}");
    assert!(
        message.contains("declared data sources: default"),
        "{message}"
    );
}

#[test]
fn on_missing_default_requires_a_default_block() {
    let error = load_err(&format!(
        "{MINIMAL}tenants:\n  claim: tenant_id\n  on_missing: default\n  rules:\n    - match: acme\n"
    ));
    let message = error.to_string();
    assert!(message.contains("key `tenants.default`"), "{message}");
    assert!(
        message.contains("requires a `tenants.default` block"),
        "{message}"
    );
}

#[test]
fn the_default_tenant_must_not_declare_a_matcher() {
    let error = load_err(&format!(
        "{MINIMAL}tenants:\n  claim: tenant_id\n  default:\n    match: acme\n"
    ));
    let message = error.to_string();
    assert!(message.contains("key `tenants.default`"), "{message}");
    assert!(message.contains("must not declare a matcher"), "{message}");
}

#[test]
fn multi_tenant_refresh_requires_an_explicit_context_list() {
    let error = load_err(&format!(
        "{MINIMAL}tenants:\n  claim: tenant_id\n  rules:\n    - match: acme\n\
         scheduled_refresh:\n  enabled: true\n"
    ));
    let message = error.to_string();
    assert!(
        message.contains("key `scheduled_refresh.contexts`"),
        "{message}"
    );
    assert!(
        message.contains("nothing can enumerate tenants at runtime"),
        "{message}"
    );
}

#[test]
fn an_implausible_timezone_is_rejected() {
    let error = load_err(&format!(
        "{MINIMAL}scheduled_refresh:\n  timezones: [\"Not A Zone\"]\n"
    ));
    let message = error.to_string();
    assert!(
        message.contains("key `scheduled_refresh.timezones`"),
        "{message}"
    );
    assert!(message.contains("IANA time zone name"), "{message}");
}

#[test]
fn a_zero_refresh_interval_is_rejected() {
    let error = load_err(&format!("{MINIMAL}scheduled_refresh:\n  interval: 0\n"));
    let message = error.to_string();
    assert!(
        message.contains("key `scheduled_refresh.interval`"),
        "{message}"
    );
    assert!(message.contains("`enabled: false`"), "{message}");
}

#[test]
fn a_mapping_value_that_is_not_an_env_reference_is_rejected() {
    let error =
        load_err("data_sources:\n  default:\n    type: postgres\n    host: { hostname: db }\n");
    let message = error.to_string();
    assert!(message.contains("cube.yml"), "{message}");
    assert!(
        message.contains("must be an environment reference"),
        "{message}"
    );
}

#[test]
fn from_env_without_a_database_type_explains_what_to_set() {
    let error = CubeConfig::from_env_with(&Env::new()).unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("key `data_sources.default.type`"),
        "{message}"
    );
    assert!(message.contains("export CUBEJS_DB_TYPE"), "{message}");
}
