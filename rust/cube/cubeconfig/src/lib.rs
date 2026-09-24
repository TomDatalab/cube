//! Declarative configuration for the pure-Rust Cube backend.
//!
//! The Node.js backend was configured by a `cube.js` (or `cube.py`) file that
//! exported callbacks: `driverFactory`, `contextToAppId`, `queryRewrite` and
//! friends. A Rust backend with no embedded JavaScript runtime cannot execute
//! those, so everything they expressed is now either an environment variable
//! or a key in `cube.yml`.
//!
//! See `rust/cube/docs/config-migration.md` for the option-by-option
//! inventory, including what was dropped and why.
//!
//! ```no_run
//! use cubeconfig::CubeConfig;
//! use serde_json::json;
//!
//! let config = CubeConfig::load("/etc/cube")?;
//! let tenant = config.for_security_context(&json!({ "tenant_id": "acme" }))?;
//! let data_source = config.data_source(&tenant.data_source).unwrap();
//! # Ok::<(), cubeconfig::ConfigError>(())
//! ```

pub mod api;
pub mod data_source;
pub mod env;
pub mod error;
pub mod scheduled_refresh;
pub mod tenants;

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::Value;

pub use crate::api::{Api, ApiScope, Cors};
pub use crate::data_source::{DataSource, DataSourceSpec, KNOWN_DB_TYPES};
pub use crate::env::{ConfigValue, Env};
pub use crate::error::{ConfigError, Origin, Result};
pub use crate::scheduled_refresh::{RefreshContextSpec, ScheduledRefresh};
pub use crate::tenants::{OnMissing, ResolvedTenant, TenantRuleSpec, TenantsSpec};

use crate::api::ApiSpec;
use crate::env::parse_bool;
use crate::scheduled_refresh::ScheduledRefreshSpec;
use crate::tenants::TenantDefaults;

/// Accepted configuration file names, in the order they are looked up.
pub const CONFIG_FILE_NAMES: [&str; 2] = ["cube.yml", "cube.yaml"];

/// The only schema version this build understands.
pub const SCHEMA_VERSION: u32 = 1;

/// Verbosity of the backend logger. Mirrors the levels `CUBEJS_LOG_LEVEL`
/// accepted in the Node.js implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Trace,
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Trace => "trace",
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "trace" => Some(LogLevel::Trace),
            "debug" => Some(LogLevel::Debug),
            "info" => Some(LogLevel::Info),
            "warn" | "warning" => Some(LogLevel::Warn),
            "error" => Some(LogLevel::Error),
            _ => None,
        }
    }
}

/// The literal shape of `cube.yml`, before the environment is merged in.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CubeConfigSpec {
    pub version: Option<u32>,
    pub log_level: Option<LogLevel>,
    pub telemetry: Option<bool>,
    /// Where the data model lives when a tenant rule does not override it.
    pub model_path: Option<String>,
    pub pre_aggregations_schema: Option<String>,
    /// The app id used when there is no `tenants` block.
    pub app_id: Option<String>,
    #[serde(default)]
    pub api: ApiSpec,
    #[serde(default)]
    pub data_sources: IndexMap<String, DataSourceSpec>,
    pub tenants: Option<TenantsSpec>,
    #[serde(default)]
    pub scheduled_refresh: ScheduledRefreshSpec,
}

/// A fully resolved, validated configuration: file merged with environment,
/// environment winning.
#[derive(Debug, Clone)]
pub struct CubeConfig {
    /// The file this came from, or a synthetic `<environment>` path for
    /// [`CubeConfig::from_env`]. Used in every diagnostic.
    pub source: PathBuf,
    pub log_level: LogLevel,
    pub telemetry: bool,
    pub model_path: PathBuf,
    pub pre_aggregations_schema: String,
    pub app_id: String,
    pub api: Api,
    pub data_sources: IndexMap<String, DataSource>,
    pub scheduled_refresh: ScheduledRefresh,
    tenants: Option<TenantsSpec>,
}

impl CubeConfig {
    /// Loads `cube.yml` / `cube.yaml` from `dir` and merges the process
    /// environment over it.
    pub fn load(dir: impl AsRef<Path>) -> Result<Self> {
        Self::load_with_env(dir, &Env::from_process())
    }

    /// Like [`CubeConfig::load`], but against an explicit environment snapshot.
    pub fn load_with_env(dir: impl AsRef<Path>, env: &Env) -> Result<Self> {
        let path = Self::find(dir.as_ref())?;
        Self::load_file_with_env(path, env)
    }

    /// Loads one specific configuration file.
    pub fn load_file_with_env(path: impl AsRef<Path>, env: &Env) -> Result<Self> {
        let path = path.as_ref();
        let contents = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let spec: CubeConfigSpec =
            serde_yaml::from_str(&contents).map_err(|source| ConfigError::Yaml {
                path: path.to_path_buf(),
                source,
            })?;
        Self::from_spec(spec, env, path)
    }

    /// Builds a configuration purely from the process environment, for
    /// deployments that have no configuration file at all (single data source,
    /// single tenant) — the `CUBEJS_DB_*`-only setup.
    pub fn from_env() -> Result<Self> {
        Self::from_env_with(&Env::from_process())
    }

    /// Like [`CubeConfig::from_env`], but against an explicit snapshot.
    pub fn from_env_with(env: &Env) -> Result<Self> {
        let mut spec = CubeConfigSpec::default();
        spec.data_sources
            .insert("default".to_string(), DataSourceSpec::default());
        Self::from_spec(spec, env, Path::new("<environment>"))
    }

    /// Loads `dir`'s configuration file if there is one, otherwise falls back
    /// to a pure-environment configuration.
    pub fn load_or_env(dir: impl AsRef<Path>, env: &Env) -> Result<Self> {
        match Self::find(dir.as_ref()) {
            Ok(path) => Self::load_file_with_env(path, env),
            Err(ConfigError::NotFound { .. }) => Self::from_env_with(env),
            Err(other) => Err(other),
        }
    }

    /// Resolves the configuration file inside `dir`.
    pub fn find(dir: &Path) -> Result<PathBuf> {
        let found: Vec<PathBuf> = CONFIG_FILE_NAMES
            .iter()
            .map(|name| dir.join(name))
            .filter(|path| path.is_file())
            .collect();
        match found.len() {
            0 => Err(ConfigError::NotFound {
                dir: dir.to_path_buf(),
            }),
            1 => Ok(found.into_iter().next().expect("length checked")),
            _ => Err(ConfigError::Ambiguous {
                dir: dir.to_path_buf(),
            }),
        }
    }

    fn from_spec(spec: CubeConfigSpec, env: &Env, file: &Path) -> Result<Self> {
        if let Some(version) = spec.version {
            if version != SCHEMA_VERSION {
                return Err(ConfigError::invalid(
                    Origin::file(file, "version"),
                    version.to_string(),
                    format!("this build only understands version {SCHEMA_VERSION}"),
                ));
            }
        }

        let log_level = match env.get("CUBEJS_LOG_LEVEL") {
            Some(raw) => LogLevel::parse(raw).ok_or_else(|| {
                ConfigError::invalid(
                    Origin::env("CUBEJS_LOG_LEVEL"),
                    raw,
                    "expected one of: trace, debug, info, warn, error",
                )
            })?,
            None => spec.log_level.unwrap_or_default(),
        };

        let telemetry = match env.get("CUBEJS_TELEMETRY") {
            Some(raw) => parse_bool(raw, Origin::env("CUBEJS_TELEMETRY"))?,
            None => spec.telemetry.unwrap_or(true),
        };

        let model_path = env
            .get("CUBEJS_SCHEMA_PATH")
            .map(str::to_string)
            .or_else(|| spec.model_path.clone())
            .unwrap_or_else(|| "model".to_string());

        let pre_aggregations_schema = env
            .get("CUBEJS_PRE_AGGREGATIONS_SCHEMA")
            .map(str::to_string)
            .or_else(|| spec.pre_aggregations_schema.clone())
            .unwrap_or_else(|| "prod_pre_aggregations".to_string());

        let app_id = env
            .get("CUBEJS_APP")
            .map(str::to_string)
            .or_else(|| spec.app_id.clone())
            .unwrap_or_else(|| "STANDALONE".to_string());

        // Data source names: declared in the file, plus anything
        // CUBEJS_DATASOURCES adds. `default` is always implied.
        let mut names: Vec<String> = spec.data_sources.keys().cloned().collect();
        if let Some(raw) = env.get("CUBEJS_DATASOURCES") {
            for name in crate::env::parse_list(raw) {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        if names.is_empty() {
            names.push("default".to_string());
        }

        let mut data_sources = IndexMap::new();
        for name in &names {
            if name.trim().is_empty() {
                return Err(ConfigError::invalid(
                    Origin::file(file, "data_sources"),
                    name,
                    "a data source name must not be empty",
                ));
            }
            let ds_spec = spec
                .data_sources
                .get(name)
                .cloned()
                .unwrap_or_else(DataSourceSpec::default);
            data_sources.insert(name.clone(), ds_spec.resolve(name, env, file)?);
        }

        let api = spec.api.resolve(env, file)?;
        let scheduled_refresh = spec.scheduled_refresh.resolve(env, file)?;

        let known: Vec<String> = data_sources.keys().cloned().collect();
        if let Some(tenants) = &spec.tenants {
            tenants.validate(file, &known)?;
            if scheduled_refresh.enabled
                && spec.scheduled_refresh.contexts.is_empty()
                && !tenants.rules.is_empty()
            {
                return Err(ConfigError::missing(
                    Origin::file(file, "scheduled_refresh.contexts"),
                    "multi-tenant deployments must list the security contexts to refresh; \
                     nothing can enumerate tenants at runtime without JavaScript",
                ));
            }
        }

        Ok(CubeConfig {
            source: file.to_path_buf(),
            log_level,
            telemetry,
            model_path: PathBuf::from(model_path),
            pre_aggregations_schema,
            app_id,
            api,
            data_sources,
            scheduled_refresh,
            tenants: spec.tenants,
        })
    }

    /// The `tenants` block, if one was configured.
    pub fn tenants(&self) -> Option<&TenantsSpec> {
        self.tenants.as_ref()
    }

    pub fn is_multi_tenant(&self) -> bool {
        self.tenants.is_some()
    }

    pub fn data_source(&self, name: &str) -> Option<&DataSource> {
        self.data_sources.get(name)
    }

    /// Resolves a request's security context into the tenant that serves it.
    ///
    /// This is the declarative replacement for `contextToAppId`,
    /// `contextToOrchestratorId`, `repositoryFactory` and the
    /// security-context-switching form of `driverFactory`.
    /// Whether a `tenants:` block selects a model per security context.
    pub fn has_tenants(&self) -> bool {
        self.tenants.is_some()
    }

    pub fn for_security_context(&self, security_context: &Value) -> Result<ResolvedTenant> {
        let defaults = self.tenant_defaults();
        let tenant = match &self.tenants {
            Some(tenants) => tenants.resolve(&self.source, security_context, &defaults)?,
            None => crate::tenants::build(None, None, security_context, &defaults),
        };

        if self.data_source(&tenant.data_source).is_none() {
            return Err(ConfigError::invalid(
                Origin::file(&self.source, "tenants.rules[].data_source"),
                &tenant.data_source,
                format!(
                    "resolved to a data source that is not declared; declared: {}",
                    self.data_sources
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }

        Ok(tenant)
    }

    /// The tenants that background refresh runs for, derived from
    /// `scheduled_refresh.contexts`.
    pub fn refresh_tenants(&self) -> Result<Vec<ResolvedTenant>> {
        self.scheduled_refresh
            .contexts
            .iter()
            .map(|context| self.for_security_context(context))
            .collect()
    }

    fn tenant_defaults(&self) -> TenantDefaults {
        TenantDefaults {
            app_id: self.app_id.clone(),
            model_path: self.model_path.to_string_lossy().into_owned(),
            data_source: self
                .data_sources
                .keys()
                .next()
                .cloned()
                .unwrap_or_else(|| "default".to_string()),
            pre_aggregations_schema: self.pre_aggregations_schema.clone(),
            api_scopes: self.api.default_scopes.clone(),
        }
    }
}
