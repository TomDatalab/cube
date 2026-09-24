use std::collections::HashMap;

use serde::{Deserialize, Deserializer};

use crate::error::{ConfigError, Origin, Result};

/// A snapshot of the process environment.
///
/// Taking the environment as an explicit value (rather than reading
/// `std::env` deep inside the loader) keeps loading pure and makes the
/// override rules testable without mutating global state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Env {
    vars: HashMap<String, String>,
}

impl Env {
    /// Snapshot of the real process environment.
    pub fn from_process() -> Self {
        Env {
            vars: std::env::vars().collect(),
        }
    }

    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) -> &mut Self {
        self.vars.insert(name.into(), value.into());
        self
    }

    pub fn with(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.set(name, value);
        self
    }

    /// Returns the variable, treating an empty / whitespace-only value as unset,
    /// which is how `env-var` behaves in the Node.js implementation.
    pub fn get(&self, name: &str) -> Option<&str> {
        match self.vars.get(name) {
            Some(v) if !v.trim().is_empty() => Some(v.as_str()),
            _ => None,
        }
    }

    pub fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// All variable names currently set, sorted, for prefix scans.
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.vars.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }
}

impl<K: Into<String>, V: Into<String>> FromIterator<(K, V)> for Env {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Env {
            vars: iter
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        }
    }
}

/// A scalar configuration value: either written inline, or a reference to an
/// environment variable (`{ env: CUBEJS_DB_PASS }`, optionally with a default).
///
/// Secrets belong in the environment, so every connection field accepts this
/// form instead of a bare literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigValue {
    Literal(String),
    EnvRef {
        name: String,
        default: Option<String>,
    },
}

impl ConfigValue {
    pub fn literal(value: impl Into<String>) -> Self {
        ConfigValue::Literal(value.into())
    }

    /// Resolves the value against `env`. Returns `None` when an `env:`
    /// reference is unset and carries no `default`.
    pub fn resolve(&self, env: &Env) -> Option<String> {
        match self {
            ConfigValue::Literal(v) => Some(v.clone()),
            ConfigValue::EnvRef { name, default } => match env.get(name) {
                Some(v) => Some(v.to_string()),
                None => default.clone(),
            },
        }
    }

    /// Like [`ConfigValue::resolve`], but fails when nothing resolves.
    pub fn resolve_required(&self, env: &Env, origin: Origin) -> Result<String> {
        match self.resolve(env) {
            Some(v) => Ok(v),
            None => match self {
                ConfigValue::EnvRef { name, .. } => Err(ConfigError::UnresolvedEnvRef {
                    origin,
                    name: name.clone(),
                }),
                ConfigValue::Literal(_) => unreachable!("literals always resolve"),
            },
        }
    }
}

impl<'de> Deserialize<'de> for ConfigValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        use serde::de::Error as _;

        let raw = serde_yaml::Value::deserialize(deserializer)?;
        match raw {
            serde_yaml::Value::String(s) => Ok(ConfigValue::Literal(s)),
            serde_yaml::Value::Bool(b) => Ok(ConfigValue::Literal(b.to_string())),
            serde_yaml::Value::Number(n) => Ok(ConfigValue::Literal(n.to_string())),
            serde_yaml::Value::Mapping(map) => {
                let name = map
                    .get(serde_yaml::Value::String("env".into()))
                    .ok_or_else(|| {
                        D::Error::custom(
                            "a mapping value must be an environment reference: `{ env: NAME }`",
                        )
                    })?;
                let name = name.as_str().ok_or_else(|| {
                    D::Error::custom("`env` must be an environment variable name (a string)")
                })?;
                let default = map
                    .get(serde_yaml::Value::String("default".into()))
                    .map(scalar_to_string)
                    .transpose()
                    .map_err(D::Error::custom)?;
                for key in map.keys() {
                    match key.as_str() {
                        Some("env") | Some("default") => {}
                        other => {
                            return Err(D::Error::custom(format!(
                                "unknown key {:?} in environment reference; expected `env` and optionally `default`",
                                other.unwrap_or("<non-string>")
                            )))
                        }
                    }
                }
                Ok(ConfigValue::EnvRef {
                    name: name.to_string(),
                    default,
                })
            }
            other => Err(D::Error::custom(format!(
                "expected a scalar or `{{ env: NAME }}`, got {}",
                type_name(&other)
            ))),
        }
    }
}

fn scalar_to_string(value: &serde_yaml::Value) -> std::result::Result<String, String> {
    match value {
        serde_yaml::Value::String(s) => Ok(s.clone()),
        serde_yaml::Value::Bool(b) => Ok(b.to_string()),
        serde_yaml::Value::Number(n) => Ok(n.to_string()),
        other => Err(format!(
            "`default` must be a scalar, got {}",
            type_name(other)
        )),
    }
}

fn type_name(value: &serde_yaml::Value) -> &'static str {
    match value {
        serde_yaml::Value::Null => "null",
        serde_yaml::Value::Bool(_) => "a boolean",
        serde_yaml::Value::Number(_) => "a number",
        serde_yaml::Value::String(_) => "a string",
        serde_yaml::Value::Sequence(_) => "a list",
        serde_yaml::Value::Mapping(_) => "a mapping",
        serde_yaml::Value::Tagged(_) => "a tagged value",
    }
}

/// Parses the strict booleans Cube accepts in environment variables.
pub(crate) fn parse_bool(raw: &str, origin: Origin) -> Result<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(ConfigError::invalid(
            origin,
            raw,
            "must be a boolean: true or false",
        )),
    }
}

pub(crate) fn parse_u64(raw: &str, origin: Origin) -> Result<u64> {
    raw.trim()
        .parse::<u64>()
        .map_err(|_| ConfigError::invalid(origin, raw, "must be a non-negative integer"))
}

/// Parses `50mb` / `512kb` / `1gb` / a plain byte count, as
/// `CUBEJS_MAX_REQUEST_SIZE` does today.
pub(crate) fn parse_size(raw: &str, origin: Origin) -> Result<u64> {
    let value = raw.trim().to_ascii_lowercase();
    let (digits, multiplier) = if let Some(rest) = value.strip_suffix("kb") {
        (rest, 1024)
    } else if let Some(rest) = value.strip_suffix("mb") {
        (rest, 1024 * 1024)
    } else if let Some(rest) = value.strip_suffix("gb") {
        (rest, 1024 * 1024 * 1024)
    } else {
        (value.as_str(), 1)
    };

    digits
        .trim()
        .parse::<u64>()
        .map(|n| n * multiplier)
        .map_err(|_| {
            ConfigError::invalid(
                origin,
                raw,
                "must be a byte count or a size string such as 100kb, 50mb or 1gb",
            )
        })
}

/// Splits a comma-separated environment variable, trimming and dropping empties.
pub(crate) fn parse_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}
