use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::env::{parse_bool, parse_list, parse_size, parse_u64, Env};
use crate::error::{ConfigError, Origin, Result};

/// The API surfaces a request may reach. Mirrors `ApiScopes` in
/// `packages/cubejs-api-gateway/src/types/strings.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiScope {
    Graphql,
    Meta,
    Data,
    Sql,
    Jobs,
}

impl ApiScope {
    pub fn as_str(self) -> &'static str {
        match self {
            ApiScope::Graphql => "graphql",
            ApiScope::Meta => "meta",
            ApiScope::Data => "data",
            ApiScope::Sql => "sql",
            ApiScope::Jobs => "jobs",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "graphql" => Some(ApiScope::Graphql),
            "meta" => Some(ApiScope::Meta),
            "data" => Some(ApiScope::Data),
            "sql" => Some(ApiScope::Sql),
            "jobs" => Some(ApiScope::Jobs),
            _ => None,
        }
    }

    /// The Node.js default (`contextToApiScopesDefFn`): everything but `jobs`.
    pub fn defaults() -> Vec<Self> {
        vec![
            ApiScope::Graphql,
            ApiScope::Meta,
            ApiScope::Data,
            ApiScope::Sql,
        ]
    }
}

/// The `api.cors` block. Replaces `http.cors` from `cube.js`, which took raw
/// `cors` middleware options (including callbacks).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorsSpec {
    pub enabled: Option<bool>,
    /// Exact origins, or `["*"]`. Regular expressions and callbacks are gone.
    pub origin: Option<Vec<String>>,
    pub methods: Option<Vec<String>>,
    pub allowed_headers: Option<Vec<String>>,
    pub exposed_headers: Option<Vec<String>>,
    pub credentials: Option<bool>,
    pub max_age: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cors {
    pub enabled: bool,
    pub origin: Vec<String>,
    pub methods: Vec<String>,
    pub allowed_headers: Vec<String>,
    pub exposed_headers: Vec<String>,
    pub credentials: bool,
    pub max_age: Option<u64>,
}

impl Default for Cors {
    fn default() -> Self {
        Cors {
            enabled: true,
            origin: vec!["*".to_string()],
            methods: vec![
                "GET".to_string(),
                "POST".to_string(),
                "OPTIONS".to_string(),
                "PUT".to_string(),
                "PATCH".to_string(),
                "DELETE".to_string(),
            ],
            allowed_headers: vec!["*".to_string()],
            exposed_headers: Vec::new(),
            credentials: false,
            max_age: None,
        }
    }
}

/// The `api` block of `cube.yml`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiSpec {
    pub base_path: Option<String>,
    pub default_scopes: Option<Vec<ApiScope>>,
    /// Byte count or size string (`50mb`).
    pub max_request_size: Option<String>,
    pub cors: Option<CorsSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Api {
    pub base_path: String,
    pub default_scopes: Vec<ApiScope>,
    pub max_request_size: u64,
    pub cors: Cors,
}

pub const DEFAULT_BASE_PATH: &str = "/cube";
pub const DEFAULT_MAX_REQUEST_SIZE: u64 = 50 * 1024 * 1024;
const MIN_REQUEST_SIZE: u64 = 100 * 1024;
const MAX_REQUEST_SIZE: u64 = 64 * 1024 * 1024;

impl ApiSpec {
    pub(crate) fn resolve(&self, env: &Env, file: &Path) -> Result<Api> {
        let base_path = match env.get("CUBEJS_API_BASE_PATH") {
            Some(value) => (value.to_string(), Origin::env("CUBEJS_API_BASE_PATH")),
            None => match &self.base_path {
                Some(value) => (value.clone(), Origin::file(file, "api.base_path")),
                None => (
                    DEFAULT_BASE_PATH.to_string(),
                    Origin::default_for("api.base_path"),
                ),
            },
        };
        if !base_path.0.starts_with('/') {
            return Err(ConfigError::invalid(
                base_path.1,
                base_path.0,
                "must start with `/`",
            ));
        }

        let default_scopes = match env.get("CUBEJS_DEFAULT_API_SCOPES") {
            Some(raw) => {
                let origin = Origin::env("CUBEJS_DEFAULT_API_SCOPES");
                parse_list(raw)
                    .iter()
                    .map(|token| {
                        ApiScope::parse(token).ok_or_else(|| {
                            ConfigError::invalid(
                                origin.clone(),
                                token,
                                "not an API scope; expected graphql, meta, data, sql or jobs",
                            )
                        })
                    })
                    .collect::<Result<Vec<_>>>()?
            }
            None => self
                .default_scopes
                .clone()
                .unwrap_or_else(ApiScope::defaults),
        };
        if default_scopes.is_empty() {
            return Err(ConfigError::invalid(
                Origin::file(file, "api.default_scopes"),
                "[]",
                "at least one API scope must be granted",
            ));
        }

        let max_request_size = match env.get("CUBEJS_MAX_REQUEST_SIZE") {
            Some(raw) => {
                let origin = Origin::env("CUBEJS_MAX_REQUEST_SIZE");
                let bytes = parse_size(raw, origin.clone())?;
                check_request_size(bytes, raw, origin)?
            }
            None => match &self.max_request_size {
                Some(raw) => {
                    let origin = Origin::file(file, "api.max_request_size");
                    let bytes = parse_size(raw, origin.clone())?;
                    check_request_size(bytes, raw, origin)?
                }
                None => DEFAULT_MAX_REQUEST_SIZE,
            },
        };

        let mut cors = Cors::default();
        if let Some(spec) = &self.cors {
            if let Some(v) = spec.enabled {
                cors.enabled = v;
            }
            if let Some(v) = &spec.origin {
                cors.origin = v.clone();
            }
            if let Some(v) = &spec.methods {
                cors.methods = v.clone();
            }
            if let Some(v) = &spec.allowed_headers {
                cors.allowed_headers = v.clone();
            }
            if let Some(v) = &spec.exposed_headers {
                cors.exposed_headers = v.clone();
            }
            if let Some(v) = spec.credentials {
                cors.credentials = v;
            }
            cors.max_age = spec.max_age.or(cors.max_age);
        }
        if let Some(raw) = env.get("CUBEJS_CORS_ORIGIN") {
            cors.origin = parse_list(raw);
        }
        if let Some(raw) = env.get("CUBEJS_CORS_ENABLED") {
            cors.enabled = parse_bool(raw, Origin::env("CUBEJS_CORS_ENABLED"))?;
        }
        if let Some(raw) = env.get("CUBEJS_CORS_MAX_AGE") {
            cors.max_age = Some(parse_u64(raw, Origin::env("CUBEJS_CORS_MAX_AGE"))?);
        }
        if cors.credentials && cors.origin.iter().any(|o| o == "*") {
            return Err(ConfigError::invalid(
                Origin::file(file, "api.cors.credentials"),
                "true",
                "credentials cannot be combined with the wildcard origin `*`; list explicit origins",
            ));
        }

        Ok(Api {
            base_path: base_path.0,
            default_scopes,
            max_request_size,
            cors,
        })
    }
}

fn check_request_size(bytes: u64, raw: &str, origin: Origin) -> Result<u64> {
    if !(MIN_REQUEST_SIZE..=MAX_REQUEST_SIZE).contains(&bytes) {
        return Err(ConfigError::invalid(
            origin,
            raw,
            "must be between 100kb and 64mb",
        ));
    }
    Ok(bytes)
}
