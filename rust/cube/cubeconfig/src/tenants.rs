use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::Value;

use crate::api::ApiScope;
use crate::error::{ConfigError, Origin, Result};

/// What to do when the security context carries a claim value that no rule matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnMissing {
    /// Reject the request (safe default: no accidental cross-tenant reads).
    #[default]
    Error,
    /// Fall back to `tenants.default`.
    Default,
}

/// The `tenants` block: a declarative replacement for `contextToAppId`,
/// `contextToOrchestratorId`, `repositoryFactory` and the common
/// `driverFactory` switch over a security-context claim.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantsSpec {
    /// The security-context claim to switch on. Dotted paths are supported,
    /// e.g. `acl.tenant_id`.
    pub claim: String,
    #[serde(default)]
    pub on_missing: OnMissing,
    /// Used when the claim is absent, and when `on_missing: default`.
    pub default: Option<TenantRuleSpec>,
    #[serde(default)]
    pub rules: Vec<TenantRuleSpec>,
}

/// One tenant rule. Exactly one of `match`, `match_any` or `match_pattern`
/// must be present on a rule inside `rules`; `tenants.default` has none.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantRuleSpec {
    #[serde(rename = "match")]
    pub match_exact: Option<String>,
    pub match_any: Option<Vec<String>>,
    /// Simple glob: `*` matches any run of characters. E.g. `eu_*`.
    pub match_pattern: Option<String>,

    /// Values may contain `{value}`, replaced by the matched claim value.
    pub app_id: Option<String>,
    pub orchestrator_id: Option<String>,
    pub model_path: Option<String>,
    pub data_source: Option<String>,
    pub pre_aggregations_schema: Option<String>,
    pub api_scopes: Option<Vec<ApiScope>>,
    /// Extra values merged into `COMPILE_CONTEXT` for this tenant.
    #[serde(default)]
    pub compile_context: IndexMap<String, Value>,
}

impl TenantRuleSpec {
    fn predicate_count(&self) -> usize {
        usize::from(self.match_exact.is_some())
            + usize::from(self.match_any.is_some())
            + usize::from(self.match_pattern.is_some())
    }

    fn matches(&self, value: &str) -> bool {
        if let Some(exact) = &self.match_exact {
            if exact == value {
                return true;
            }
        }
        if let Some(any) = &self.match_any {
            if any.iter().any(|v| v == value) {
                return true;
            }
        }
        if let Some(pattern) = &self.match_pattern {
            if glob_match(pattern, value) {
                return true;
            }
        }
        false
    }
}

/// Everything the rest of the backend needs to serve one request: the
/// resolved equivalent of `contextToAppId` + `contextToOrchestratorId` +
/// `repositoryFactory` + `driverFactory` + `COMPILE_CONTEXT`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTenant {
    pub app_id: String,
    pub orchestrator_id: String,
    pub model_path: PathBuf,
    pub data_source: String,
    pub pre_aggregations_schema: String,
    pub api_scopes: Vec<ApiScope>,
    /// `COMPILE_CONTEXT` as the data model sees it: the security context under
    /// both `securityContext` and `security_context`, plus rule extras.
    pub compile_context: Value,
    /// The claim value this tenant was selected by, if any.
    pub matched_value: Option<String>,
}

/// Defaults applied when a rule leaves a field out.
pub(crate) struct TenantDefaults {
    pub app_id: String,
    pub model_path: String,
    pub data_source: String,
    pub pre_aggregations_schema: String,
    pub api_scopes: Vec<ApiScope>,
}

impl TenantsSpec {
    pub(crate) fn validate(&self, file: &Path, known_data_sources: &[String]) -> Result<()> {
        if self.claim.trim().is_empty() {
            return Err(ConfigError::invalid(
                Origin::file(file, "tenants.claim"),
                &self.claim,
                "must be a non-empty security context claim name",
            ));
        }

        for (index, rule) in self.rules.iter().enumerate() {
            let key = format!("tenants.rules[{index}]");
            match rule.predicate_count() {
                1 => {}
                0 => {
                    return Err(ConfigError::missing(
                        Origin::file(file, &key),
                        "a rule needs one of `match`, `match_any` or `match_pattern`",
                    ))
                }
                _ => {
                    return Err(ConfigError::invalid(
                        Origin::file(file, &key),
                        "multiple matchers",
                        "use exactly one of `match`, `match_any` or `match_pattern`",
                    ))
                }
            }
            rule.validate_data_source(file, &key, known_data_sources)?;
        }

        if let Some(default) = &self.default {
            if default.predicate_count() > 0 {
                return Err(ConfigError::invalid(
                    Origin::file(file, "tenants.default"),
                    "matcher",
                    "`tenants.default` is the fallback and must not declare a matcher",
                ));
            }
            default.validate_data_source(file, "tenants.default", known_data_sources)?;
        }

        if self.on_missing == OnMissing::Default && self.default.is_none() {
            return Err(ConfigError::missing(
                Origin::file(file, "tenants.default"),
                "`tenants.on_missing: default` requires a `tenants.default` block",
            ));
        }

        Ok(())
    }

    pub(crate) fn resolve(
        &self,
        file: &Path,
        security_context: &Value,
        defaults: &TenantDefaults,
    ) -> Result<ResolvedTenant> {
        let claim_value = lookup_claim(security_context, &self.claim);

        let (rule, matched) = match claim_value {
            Some(value) => match self.rules.iter().find(|r| r.matches(&value)) {
                Some(rule) => (Some(rule), Some(value)),
                None => match (self.on_missing, self.default.as_ref()) {
                    (OnMissing::Default, Some(default)) => (Some(default), Some(value)),
                    _ => {
                        return Err(ConfigError::NoTenantMatch {
                            origin: Origin::file(file, "tenants.rules"),
                            claim: self.claim.clone(),
                            value,
                        })
                    }
                },
            },
            None => match self.default.as_ref() {
                Some(default) => (Some(default), None),
                None => {
                    return Err(ConfigError::MissingTenantClaim {
                        origin: Origin::file(file, "tenants.default"),
                        claim: self.claim.clone(),
                    })
                }
            },
        };

        Ok(build(rule, matched, security_context, defaults))
    }
}

impl TenantRuleSpec {
    fn validate_data_source(&self, file: &Path, key: &str, known: &[String]) -> Result<()> {
        if let Some(data_source) = &self.data_source {
            // A templated name can only be checked at request time.
            if !data_source.contains('{') && !known.iter().any(|k| k == data_source) {
                return Err(ConfigError::invalid(
                    Origin::file(file, format!("{key}.data_source")),
                    data_source,
                    format!(
                        "no such data source; declared data sources: {}",
                        if known.is_empty() {
                            "<none>".to_string()
                        } else {
                            known.join(", ")
                        }
                    ),
                ));
            }
        }
        Ok(())
    }
}

pub(crate) fn build(
    rule: Option<&TenantRuleSpec>,
    matched: Option<String>,
    security_context: &Value,
    defaults: &TenantDefaults,
) -> ResolvedTenant {
    let subst = |template: &str| -> String {
        match &matched {
            Some(value) => template.replace("{value}", value),
            None => template.to_string(),
        }
    };

    let pick = |from_rule: Option<&String>, fallback: &str| -> String {
        subst(from_rule.map(String::as_str).unwrap_or(fallback))
    };

    let app_id = pick(rule.and_then(|r| r.app_id.as_ref()), &defaults.app_id);
    let orchestrator_id = rule
        .and_then(|r| r.orchestrator_id.as_ref())
        .map(|t| subst(t))
        .unwrap_or_else(|| app_id.clone());

    let mut compile_context = serde_json::Map::new();
    if let Some(rule) = rule {
        for (key, value) in &rule.compile_context {
            compile_context.insert(key.clone(), value.clone());
        }
    }
    compile_context.insert("securityContext".to_string(), security_context.clone());
    compile_context.insert("security_context".to_string(), security_context.clone());

    ResolvedTenant {
        model_path: PathBuf::from(pick(
            rule.and_then(|r| r.model_path.as_ref()),
            &defaults.model_path,
        )),
        data_source: pick(
            rule.and_then(|r| r.data_source.as_ref()),
            &defaults.data_source,
        ),
        pre_aggregations_schema: pick(
            rule.and_then(|r| r.pre_aggregations_schema.as_ref()),
            &defaults.pre_aggregations_schema,
        ),
        api_scopes: rule
            .and_then(|r| r.api_scopes.clone())
            .unwrap_or_else(|| defaults.api_scopes.clone()),
        app_id,
        orchestrator_id,
        compile_context: Value::Object(compile_context),
        matched_value: matched,
    }
}

/// Reads a (possibly dotted) claim out of a security context, stringifying
/// scalars the way a JS `contextToAppId` template literal would.
pub fn lookup_claim(context: &Value, claim: &str) -> Option<String> {
    let mut current = context;
    for segment in claim.split('.') {
        current = current.get(segment)?;
    }
    match current {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::String(_) => None,
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// `*` wildcard matching, enough for `eu_*` / `*_staging` style rules.
fn glob_match(pattern: &str, value: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == value;
    }

    let mut rest = value;
    let last = parts.len() - 1;
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if index == 0 {
            match rest.strip_prefix(part) {
                Some(r) => rest = r,
                None => return false,
            }
        } else if index == last {
            return rest.len() >= part.len() && rest.ends_with(part);
        } else {
            match rest.find(part) {
                Some(at) => rest = &rest[at + part.len()..],
                None => return false,
            }
        }
    }
    true
}
