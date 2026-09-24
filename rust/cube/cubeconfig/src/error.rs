use std::fmt;
use std::path::{Path, PathBuf};

/// Where a configuration value came from, so that every diagnostic can point at
/// either a concrete `cube.yml` key or the environment variable that overrode it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// A key inside a configuration file, e.g. `data_sources.default.type`.
    File { path: PathBuf, key: String },
    /// An environment variable, e.g. `CUBEJS_DB_TYPE`.
    Env { name: String },
    /// A built-in default (used when a default is itself invalid).
    Default { key: String },
}

impl Origin {
    pub fn file(path: impl AsRef<Path>, key: impl Into<String>) -> Self {
        Origin::File {
            path: path.as_ref().to_path_buf(),
            key: key.into(),
        }
    }

    pub fn env(name: impl Into<String>) -> Self {
        Origin::Env { name: name.into() }
    }

    pub fn default_for(key: impl Into<String>) -> Self {
        Origin::Default { key: key.into() }
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Origin::File { path, key } => write!(f, "{}: key `{}`", path.display(), key),
            Origin::Env { name } => write!(f, "environment variable `{name}`"),
            Origin::Default { key } => write!(f, "built-in default for `{key}`"),
        }
    }
}

/// Every way loading a Cube configuration can fail.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("no cube.yml or cube.yaml found in {}", .dir.display())]
    NotFound { dir: PathBuf },

    #[error("both cube.yml and cube.yaml exist in {}; keep only one", .dir.display())]
    Ambiguous { dir: PathBuf },

    #[error("{}: cannot be read: {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{}: not valid YAML: {source}", .path.display())]
    Yaml {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },

    /// A value is present but unusable. Always names the offending key.
    #[error("{origin}: invalid value {value:?}: {reason}")]
    InvalidValue {
        origin: Origin,
        value: String,
        reason: String,
    },

    /// A value that has to be there is not.
    #[error("{origin}: is required but missing{}", suffix(.reason))]
    Missing { origin: Origin, reason: String },

    /// `{ env: NAME }` reference that the environment does not define.
    #[error("{origin}: references environment variable `{name}`, which is not set")]
    UnresolvedEnvRef { origin: Origin, name: String },

    /// The security context did not select any tenant.
    #[error("no tenant matches {claim}={value:?} (from {}); add a rule or set `tenants.on_missing: default`", .origin)]
    NoTenantMatch {
        origin: Origin,
        claim: String,
        value: String,
    },

    /// The security context has no value at all for the tenant claim.
    #[error(
        "security context has no `{claim}` claim and no `tenants.default` is configured ({origin})"
    )]
    MissingTenantClaim { origin: Origin, claim: String },
}

fn suffix(reason: &str) -> String {
    if reason.is_empty() {
        String::new()
    } else {
        format!(": {reason}")
    }
}

impl ConfigError {
    pub(crate) fn invalid(
        origin: Origin,
        value: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        ConfigError::InvalidValue {
            origin,
            value: value.into(),
            reason: reason.into(),
        }
    }

    pub(crate) fn missing(origin: Origin, reason: impl Into<String>) -> Self {
        ConfigError::Missing {
            origin,
            reason: reason.into(),
        }
    }
}

pub type Result<T> = std::result::Result<T, ConfigError>;
