use std::path::{Path, PathBuf};

use cubeconfig::{ApiScope, ConfigError, CubeConfig, Env};
use serde_json::json;

fn full() -> CubeConfig {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/full");
    CubeConfig::load_with_env(dir, &Env::new()).expect("full fixture must load")
}

#[test]
fn exact_match_rule_wins() {
    let tenant = full()
        .for_security_context(&json!({ "tenant_id": "acme", "user_id": 7 }))
        .unwrap();

    assert_eq!(tenant.app_id, "acme");
    // No explicit orchestrator_id => it follows the app id, so tenants never
    // share a query cache by accident.
    assert_eq!(tenant.orchestrator_id, "acme");
    assert_eq!(tenant.model_path, PathBuf::from("model/acme"));
    assert_eq!(tenant.data_source, "warehouse");
    assert_eq!(tenant.pre_aggregations_schema, "acme_pre_aggregations");
    assert_eq!(
        tenant.api_scopes,
        vec![
            ApiScope::Meta,
            ApiScope::Data,
            ApiScope::Sql,
            ApiScope::Jobs
        ]
    );
    assert_eq!(tenant.matched_value.as_deref(), Some("acme"));

    // COMPILE_CONTEXT carries the rule extras plus both spellings of the
    // security context, as the JS data model used to see them.
    assert_eq!(tenant.compile_context["region"], json!("us"));
    assert_eq!(tenant.compile_context["tier"], json!("enterprise"));
    assert_eq!(
        tenant.compile_context["securityContext"]["user_id"],
        json!(7)
    );
    assert_eq!(
        tenant.compile_context["security_context"]["tenant_id"],
        json!("acme")
    );
}

#[test]
fn match_any_and_value_templating() {
    let config = full();

    let beta = config
        .for_security_context(&json!({ "tenant_id": "beta" }))
        .unwrap();
    assert_eq!(beta.app_id, "tenant_beta");
    assert_eq!(beta.model_path, PathBuf::from("model/beta"));
    assert_eq!(beta.data_source, "default");
    // Unspecified fields fall back to the top level.
    assert_eq!(beta.pre_aggregations_schema, "prod_pre_aggregations");
    assert_eq!(beta.api_scopes, config.api.default_scopes);

    let gamma = config
        .for_security_context(&json!({ "tenant_id": "gamma" }))
        .unwrap();
    assert_eq!(gamma.app_id, "tenant_gamma");
    assert_eq!(gamma.model_path, PathBuf::from("model/gamma"));
}

#[test]
fn glob_pattern_rule_with_explicit_orchestrator_id() {
    let tenant = full()
        .for_security_context(&json!({ "tenant_id": "eu_west" }))
        .unwrap();

    assert_eq!(tenant.app_id, "eu_west");
    assert_eq!(tenant.orchestrator_id, "eu_cluster");
    assert_eq!(tenant.model_path, PathBuf::from("model/eu"));
}

#[test]
fn a_missing_claim_falls_back_to_the_default_tenant() {
    let tenant = full()
        .for_security_context(&json!({ "user_id": 1 }))
        .unwrap();

    assert_eq!(tenant.app_id, "shared");
    assert_eq!(tenant.model_path, PathBuf::from("model/shared"));
    assert_eq!(tenant.data_source, "default");
    assert_eq!(tenant.matched_value, None);
}

#[test]
fn an_unmatched_claim_is_rejected_by_default() {
    let error = full()
        .for_security_context(&json!({ "tenant_id": "unknown" }))
        .unwrap_err();

    assert!(matches!(error, ConfigError::NoTenantMatch { .. }));
    let message = error.to_string();
    assert!(
        message.contains("no tenant matches tenant_id=\"unknown\""),
        "{message}"
    );
    assert!(message.contains("cube.yml"), "{message}");
    assert!(message.contains("tenants.on_missing: default"), "{message}");
}

#[test]
fn on_missing_default_routes_unknown_tenants_to_the_default_rule() {
    let config = load_inline(
        r#"
data_sources:
  default:
    type: postgres
tenants:
  claim: tenant_id
  on_missing: default
  default:
    app_id: shared
    model_path: model/shared
  rules:
    - match: acme
      app_id: acme
"#,
    );

    let tenant = config
        .for_security_context(&json!({ "tenant_id": "whoever" }))
        .unwrap();
    assert_eq!(tenant.app_id, "shared");
    assert_eq!(tenant.matched_value.as_deref(), Some("whoever"));
}

#[test]
fn a_dotted_claim_path_is_supported() {
    let config = load_inline(
        r#"
data_sources:
  default:
    type: postgres
tenants:
  claim: acl.org.id
  rules:
    - match_pattern: "*"
      app_id: "org_{value}"
      model_path: "model/{value}"
"#,
    );

    let tenant = config
        .for_security_context(&json!({ "acl": { "org": { "id": "42" } } }))
        .unwrap();
    assert_eq!(tenant.app_id, "org_42");
    assert_eq!(tenant.model_path, PathBuf::from("model/42"));
}

#[test]
fn numeric_claims_are_stringified() {
    let config = load_inline(
        r#"
data_sources:
  default:
    type: postgres
tenants:
  claim: tenant_id
  rules:
    - match: "17"
      app_id: "tenant_{value}"
"#,
    );

    let tenant = config
        .for_security_context(&json!({ "tenant_id": 17 }))
        .unwrap();
    assert_eq!(tenant.app_id, "tenant_17");
}

#[test]
fn a_missing_claim_without_a_default_is_an_error() {
    let config = load_inline(
        r#"
data_sources:
  default:
    type: postgres
tenants:
  claim: tenant_id
  rules:
    - match: acme
      app_id: acme
"#,
    );

    let error = config.for_security_context(&json!({})).unwrap_err();
    assert!(matches!(error, ConfigError::MissingTenantClaim { .. }));
    assert!(
        error
            .to_string()
            .contains("security context has no `tenant_id` claim"),
        "{error}"
    );
}

#[test]
fn without_a_tenants_block_every_context_maps_to_one_tenant() {
    let config = load_inline(
        r#"
app_id: solo
model_path: model
data_sources:
  default:
    type: postgres
"#,
    );

    let a = config
        .for_security_context(&json!({ "tenant_id": "acme" }))
        .unwrap();
    let b = config.for_security_context(&json!(null)).unwrap();
    assert_eq!(a.app_id, "solo");
    assert_eq!(b.app_id, "solo");
    assert_eq!(a.orchestrator_id, "solo");
    assert_eq!(a.data_source, "default");
}

#[test]
fn refresh_tenants_resolves_every_configured_context() {
    let tenants = full().refresh_tenants().unwrap();
    let app_ids: Vec<&str> = tenants.iter().map(|t| t.app_id.as_str()).collect();
    assert_eq!(app_ids, vec!["acme", "tenant_beta", "eu_west"]);
}

pub fn load_inline(yaml: &str) -> CubeConfig {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("cube.yml"), yaml).unwrap();
    CubeConfig::load_with_env(dir.path(), &Env::new()).expect("config must load")
}
