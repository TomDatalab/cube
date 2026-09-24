use indexmap::IndexMap;
use std::path::Path;

use serde::Deserialize;

use crate::env::{parse_bool, parse_u64, ConfigValue, Env};
use crate::error::{ConfigError, Origin, Result};

/// Database types the Node.js `DriverDependencies` map accepted, minus the ones
/// that only ever existed as JS drivers. Kept in sync with
/// `packages/cubejs-server-core/src/core/types.ts` (`DatabaseType`).
pub const KNOWN_DB_TYPES: &[&str] = &[
    "athena",
    "bigquery",
    "clickhouse",
    "crate",
    "cubestore",
    "databricks-jdbc",
    "dremio",
    "druid",
    "duckdb",
    "firebolt",
    "hive",
    "jdbc",
    "ksql",
    "materialize",
    "mongobi",
    "mssql",
    "mysql",
    "mysqlauroraserverless",
    "oracle",
    "pinot",
    "postgres",
    "prestodb",
    "questdb",
    "redshift",
    "snowflake",
    "sqlite",
    "trino",
    "vertica",
];

/// One entry of the `data_sources` mapping in `cube.yml`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataSourceSpec {
    #[serde(rename = "type")]
    pub db_type: Option<ConfigValue>,
    pub url: Option<ConfigValue>,
    pub host: Option<ConfigValue>,
    pub port: Option<ConfigValue>,
    pub database: Option<ConfigValue>,
    pub schema: Option<ConfigValue>,
    pub user: Option<ConfigValue>,
    pub password: Option<ConfigValue>,
    pub ssl: Option<ConfigValue>,
    pub max_pool: Option<ConfigValue>,
    pub export_bucket: Option<ConfigValue>,
    pub export_bucket_type: Option<ConfigValue>,
    /// Driver-specific passthrough, e.g. `warehouse` for Snowflake. Values may
    /// be `{ env: ... }` references too.
    #[serde(default)]
    pub options: IndexMap<String, ConfigValue>,
}

/// A data source after the file and the environment have been merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataSource {
    pub name: String,
    pub db_type: String,
    pub url: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub database: Option<String>,
    pub schema: Option<String>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub ssl: bool,
    pub max_pool: Option<u32>,
    pub export_bucket: Option<String>,
    pub export_bucket_type: Option<String>,
    pub options: IndexMap<String, String>,
}

/// Maps a canonical `CUBEJS_*` variable to its per-data-source form, exactly as
/// `keyByDataSource` does in `@cubejs-backend/shared`.
pub fn key_by_data_source(origin: &str, data_source: &str) -> String {
    if data_source == "default" {
        return origin.to_string();
    }
    match origin.strip_prefix("CUBEJS_") {
        Some(rest) => format!("CUBEJS_DS_{}_{}", data_source.to_uppercase(), rest),
        None => origin.to_string(),
    }
}

fn resolve_field(
    env: &Env,
    file: &Path,
    ds_name: &str,
    leaf: &str,
    origin_var: &str,
    spec: Option<&ConfigValue>,
) -> Option<(String, Origin)> {
    let env_var = key_by_data_source(origin_var, ds_name);
    if let Some(value) = env.get(&env_var) {
        return Some((value.to_string(), Origin::env(env_var)));
    }
    let spec = spec?;
    let origin = Origin::file(file, format!("data_sources.{ds_name}.{leaf}"));
    spec.resolve(env).map(|value| (value, origin))
}

impl DataSourceSpec {
    /// Merges this declaration with the environment (environment wins) and
    /// validates the result. Every diagnostic names the file key or the
    /// variable the bad value came from.
    pub(crate) fn resolve(&self, name: &str, env: &Env, file: &Path) -> Result<DataSource> {
        let get = |leaf: &str, var: &str, spec: Option<&ConfigValue>| {
            resolve_field(env, file, name, leaf, var, spec)
        };

        let db_type = match get("type", "CUBEJS_DB_TYPE", self.db_type.as_ref()) {
            Some((value, origin)) => {
                let value = value.trim().to_string();
                if !KNOWN_DB_TYPES.contains(&value.as_str()) {
                    return Err(ConfigError::invalid(
                        origin,
                        value,
                        format!(
                            "unknown database type; expected one of: {}",
                            KNOWN_DB_TYPES.join(", ")
                        ),
                    ));
                }
                value
            }
            None => {
                if let Some(ConfigValue::EnvRef { name: var, .. }) = self.db_type.as_ref() {
                    return Err(ConfigError::UnresolvedEnvRef {
                        origin: Origin::file(file, format!("data_sources.{name}.type")),
                        name: var.clone(),
                    });
                }
                return Err(ConfigError::missing(
                    Origin::file(file, format!("data_sources.{name}.type")),
                    format!(
                        "set it in cube.yml or export {}",
                        key_by_data_source("CUBEJS_DB_TYPE", name)
                    ),
                ));
            }
        };

        let port = match get("port", "CUBEJS_DB_PORT", self.port.as_ref()) {
            Some((value, origin)) => {
                let parsed = parse_u64(&value, origin.clone())?;
                if parsed == 0 || parsed > 65535 {
                    return Err(ConfigError::invalid(
                        origin,
                        value,
                        "must be a TCP port between 1 and 65535",
                    ));
                }
                Some(parsed as u16)
            }
            None => None,
        };

        let ssl = match get("ssl", "CUBEJS_DB_SSL", self.ssl.as_ref()) {
            Some((value, origin)) => parse_bool(&value, origin)?,
            None => false,
        };

        let max_pool = match get("max_pool", "CUBEJS_DB_MAX_POOL", self.max_pool.as_ref()) {
            Some((value, origin)) => {
                let parsed = parse_u64(&value, origin.clone())?;
                if parsed == 0 {
                    return Err(ConfigError::invalid(origin, value, "must be at least 1"));
                }
                Some(parsed as u32)
            }
            None => None,
        };

        let plain = |leaf: &str, var: &str, spec: Option<&ConfigValue>| {
            resolve_field(env, file, name, leaf, var, spec).map(|(value, _)| value)
        };

        let mut options = IndexMap::new();
        for (key, value) in &self.options {
            let origin = Origin::file(file, format!("data_sources.{name}.options.{key}"));
            if let Some(resolved) = value.resolve(env) {
                options.insert(key.clone(), resolved);
            } else if let ConfigValue::EnvRef { name: var, .. } = value {
                return Err(ConfigError::UnresolvedEnvRef {
                    origin,
                    name: var.clone(),
                });
            }
        }

        Ok(DataSource {
            name: name.to_string(),
            db_type,
            url: plain("url", "CUBEJS_DB_URL", self.url.as_ref()),
            host: plain("host", "CUBEJS_DB_HOST", self.host.as_ref()),
            port,
            database: plain("database", "CUBEJS_DB_NAME", self.database.as_ref()),
            schema: plain("schema", "CUBEJS_DB_SCHEMA", self.schema.as_ref()),
            user: plain("user", "CUBEJS_DB_USER", self.user.as_ref()),
            password: plain("password", "CUBEJS_DB_PASS", self.password.as_ref()),
            ssl,
            max_pool,
            export_bucket: plain(
                "export_bucket",
                "CUBEJS_DB_EXPORT_BUCKET",
                self.export_bucket.as_ref(),
            ),
            export_bucket_type: plain(
                "export_bucket_type",
                "CUBEJS_DB_EXPORT_BUCKET_TYPE",
                self.export_bucket_type.as_ref(),
            ),
            options,
        })
    }
}
