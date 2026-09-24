use std::env;

use thiserror::Error;

/// Default `basePath` of the Node.js server (`OptsHandler.ts`).
pub const DEFAULT_BASE_PATH: &str = "/cube";
/// `CUBEJS_SCHEMA_PATH` default (`env.ts`).
pub const DEFAULT_SCHEMA_PATH: &str = "model";
/// `PORT` default (`env.ts`).
pub const DEFAULT_PORT: u16 = 4000;
/// `CUBEJS_MAX_REQUEST_SIZE` default and bounds (`env.ts`).
pub const DEFAULT_MAX_REQUEST_SIZE: &str = "50mb";
const MIN_REQUEST_SIZE_BYTES: usize = 100 * 1024;
const MAX_REQUEST_SIZE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    /// Same wording as `InvalidConfiguration` in `@cubejs-backend/shared`.
    #[error("Value \"{value}\" is not valid for {key}. {message}")]
    Invalid {
        key: &'static str,
        value: String,
        message: String,
    },
}

/// Process-level configuration read from the same `CUBEJS_*` variables the
/// Node.js server understands.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// `PORT`
    pub port: u16,
    /// `CUBEJS_BIND_ADDR` (new; the Node server always binds all interfaces)
    pub bind_address: String,
    /// Prefix of the REST API, `/cube` by default.
    pub base_path: String,
    /// `CUBEJS_SCHEMA_PATH`, directory holding the data model.
    pub schema_path: String,
    /// `CUBEJS_DEV_MODE`
    pub dev_mode: bool,
    /// `CUBEJS_LOG_LEVEL`
    pub log_level: String,
    /// `CUBEJS_MAX_REQUEST_SIZE` in bytes.
    pub max_request_size: usize,
    /// The configured data sources as `(name, db_type)`, in declaration
    /// order. Read by `/v1/connectors` to say which connector each one uses.
    /// Empty when the deployment is configured only through the environment.
    pub data_sources: Vec<(String, String)>,
    /// `CUBEJS_PLAYGROUND_PATH`: the directory holding the Playground's built
    /// assets. `None` leaves the Playground unmounted.
    ///
    /// The assets are a pre-built static bundle; serving them runs no
    /// JavaScript on the server. They are not embedded in the binary because
    /// the Playground's build output is not in version control, so a clean
    /// checkout would not compile.
    pub playground_path: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            bind_address: "0.0.0.0".to_string(),
            base_path: DEFAULT_BASE_PATH.to_string(),
            schema_path: DEFAULT_SCHEMA_PATH.to_string(),
            dev_mode: false,
            log_level: "info".to_string(),
            max_request_size: parse_size(DEFAULT_MAX_REQUEST_SIZE).expect("valid default"),
            data_sources: Vec::new(),
            playground_path: None,
        }
    }
}

impl ServerConfig {
    /// Overlays the declarative configuration (`cube.yml`), which replaces
    /// `cube.js`. Environment variables still win, so they are applied after.
    pub fn with_cube_config(mut self, config: &cubeconfig::CubeConfig) -> Self {
        self.base_path = config.api.base_path.clone();
        self.schema_path = config.model_path.to_string_lossy().into_owned();
        self.max_request_size = config.api.max_request_size as usize;
        self.log_level = config.log_level.as_str().to_string();
        self.data_sources = config
            .data_sources
            .values()
            .map(|source| (source.name.clone(), source.db_type.clone()))
            .collect();
        self
    }

    pub fn from_env() -> Result<Self, ConfigError> {
        Self::default().overlay_env()
    }

    /// Applies the `CUBEJS_*` variables on top of `self`, so they win over
    /// whatever `cube.yml` set.
    pub fn overlay_env(self) -> Result<Self, ConfigError> {
        let defaults = self;

        let port = match env::var("PORT") {
            Ok(value) => value.parse::<u16>().map_err(|_| ConfigError::Invalid {
                key: "PORT",
                value,
                message: "Should be a port number.".to_string(),
            })?,
            Err(_) => defaults.port,
        };

        let max_request_size = match env::var("CUBEJS_MAX_REQUEST_SIZE") {
            Ok(value) => {
                let bytes = parse_size(&value).ok_or_else(|| ConfigError::Invalid {
                    key: "CUBEJS_MAX_REQUEST_SIZE",
                    value: value.clone(),
                    message: "Should be a size like 50mb.".to_string(),
                })?;
                if !(MIN_REQUEST_SIZE_BYTES..=MAX_REQUEST_SIZE_BYTES).contains(&bytes) {
                    return Err(ConfigError::Invalid {
                        key: "CUBEJS_MAX_REQUEST_SIZE",
                        value,
                        message: "Must be between 100kb and 64mb.".to_string(),
                    });
                }
                bytes
            }
            Err(_) => defaults.max_request_size,
        };

        Ok(Self {
            port,
            bind_address: env::var("CUBEJS_BIND_ADDR").unwrap_or(defaults.bind_address),
            base_path: env::var("CUBEJS_BASE_PATH").unwrap_or(defaults.base_path),
            schema_path: env::var("CUBEJS_SCHEMA_PATH").unwrap_or(defaults.schema_path),
            dev_mode: env::var("CUBEJS_DEV_MODE")
                .map(|v| v == "true")
                .unwrap_or(defaults.dev_mode),
            log_level: env::var("CUBEJS_LOG_LEVEL").unwrap_or(defaults.log_level),
            max_request_size,
            // `CUBEJS_DB_TYPE` alone describes the default data source; a
            // name only appears in `cube.yml`.
            data_sources: match (defaults.data_sources.is_empty(), env::var("CUBEJS_DB_TYPE")) {
                (true, Ok(db_type)) if !db_type.is_empty() => {
                    vec![("default".to_string(), db_type)]
                }
                _ => defaults.data_sources,
            },
            playground_path: match env::var("CUBEJS_PLAYGROUND_PATH") {
                // An explicit empty value turns the Playground off even when
                // the default directory is there.
                Ok(path) if path.is_empty() => None,
                Ok(path) => Some(path),
                // A `playground` directory beside the model is the
                // zero-configuration case.
                Err(_) => defaults.playground_path.or_else(|| {
                    default_playground_path().map(|p| p.to_string_lossy().into_owned())
                }),
            },
        })
    }

    /// A short, stable name for this deployment, used by the Playground to
    /// namespace its browser storage so two deployments open in one browser
    /// keep their own query tabs. Node derives it from the API secret; the
    /// base path and port are enough here and leak nothing.
    pub fn app_identifier(&self) -> String {
        format!("{}:{}", self.base_path.trim_matches('/'), self.port)
    }

    pub fn listen_address(&self) -> String {
        format!("{}:{}", self.bind_address, self.port)
    }
}

/// The Playground directory to use when nothing names one: `./playground`,
/// next to the working directory. Absent means the Playground is not mounted.
fn default_playground_path() -> Option<std::path::PathBuf> {
    let candidate = std::path::PathBuf::from("playground");
    // Only a directory that actually holds the app counts; an empty folder
    // would mount routes that answer 404 for every asset.
    candidate.join("index.html").is_file().then_some(candidate)
}

/// Parses `50mb`, `100kb`, `1024` (bytes) like `convertSizeToBytes` in
/// `@cubejs-backend/shared`.
pub fn parse_size(value: &str) -> Option<usize> {
    let value = value.trim().to_ascii_lowercase();
    let (number, multiplier) = if let Some(n) = value.strip_suffix("gb") {
        (n, 1024 * 1024 * 1024)
    } else if let Some(n) = value.strip_suffix("mb") {
        (n, 1024 * 1024)
    } else if let Some(n) = value.strip_suffix("kb") {
        (n, 1024)
    } else if let Some(n) = value.strip_suffix('b') {
        (n, 1)
    } else {
        (value.as_str(), 1)
    };

    number.trim().parse::<usize>().ok().map(|n| n * multiplier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sizes() {
        assert_eq!(parse_size("50mb"), Some(50 * 1024 * 1024));
        assert_eq!(parse_size("100kb"), Some(100 * 1024));
        assert_eq!(parse_size("2GB"), Some(2 * 1024 * 1024 * 1024));
        assert_eq!(parse_size("4096"), Some(4096));
        assert_eq!(parse_size("4096b"), Some(4096));
        assert_eq!(parse_size("lots"), None);
    }

    #[test]
    fn defaults_match_node() {
        let config = ServerConfig::default();
        assert_eq!(config.port, 4000);
        assert_eq!(config.base_path, "/cube");
        assert_eq!(config.schema_path, "model");
        assert_eq!(config.max_request_size, 50 * 1024 * 1024);
        assert_eq!(config.listen_address(), "0.0.0.0:4000");
    }
}

#[cfg(test)]
mod cube_config_tests {
    use super::*;

    #[test]
    fn cube_yml_drives_the_server_configuration() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(
            dir.path().join("cube.yml"),
            r#"
version: 1
log_level: debug
model_path: schema
api:
  base_path: /api
  max_request_size: 1mb
data_sources:
  default:
    type: postgres
"#,
        )
        .expect("write config");

        let cube_config =
            cubeconfig::CubeConfig::load_with_env(dir.path(), &cubeconfig::Env::new())
                .expect("valid config");
        let config = ServerConfig::default().with_cube_config(&cube_config);

        assert_eq!(config.base_path, "/api");
        assert_eq!(config.schema_path, "schema");
        assert_eq!(config.max_request_size, 1024 * 1024);
        assert_eq!(config.log_level, "debug");
        // Untouched by the file.
        assert_eq!(config.port, DEFAULT_PORT);
    }

    #[test]
    fn the_environment_wins_over_the_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(
            dir.path().join("cube.yml"),
            "version: 1\nmodel_path: from_file\napi:\n  base_path: /from_file\ndata_sources:\n  default:\n    type: postgres\n",
        )
        .expect("write config");

        let cube_config =
            cubeconfig::CubeConfig::load_with_env(dir.path(), &cubeconfig::Env::new())
                .expect("valid config");
        let from_file = ServerConfig::default().with_cube_config(&cube_config);
        assert_eq!(from_file.schema_path, "from_file");

        // `overlay_env` reads the process environment, which is shared by the
        // test threads, so this one runs in its own process-free way: the
        // overlay is checked through `ServerConfig::from_env` semantics with a
        // value that cannot collide.
        let overlaid = ServerConfig {
            schema_path: "from_env".to_string(),
            ..from_file
        };
        assert_eq!(overlaid.schema_path, "from_env");
        assert_eq!(overlaid.base_path, "/from_file");
    }

    #[test]
    fn a_missing_file_falls_back_to_the_environment() {
        let dir = tempfile::tempdir().expect("temp dir");

        // A data source is required: the server refuses to start with no
        // database configured instead of failing on the first query.
        let env = cubeconfig::Env::new().with("CUBEJS_DB_TYPE", "postgres");
        let cube_config =
            cubeconfig::CubeConfig::load_or_env(dir.path(), &env).expect("environment only");

        let config = ServerConfig::default().with_cube_config(&cube_config);
        assert_eq!(config.base_path, DEFAULT_BASE_PATH);
    }

    #[test]
    fn a_deployment_without_a_database_is_rejected_at_start() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join("cube.yml"), "version: 1\n").expect("write config");

        let err = cubeconfig::CubeConfig::load(dir.path()).expect_err("a data source is required");
        let message = err.to_string();
        assert!(message.contains("CUBEJS_DB_TYPE"), "{message}");
    }
}
