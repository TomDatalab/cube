//! Environment-driven driver configuration.
//!
//! This is a port of the database-related entries of
//! `packages/cubejs-backend-shared/src/env.ts` (`getEnv('db*')`,
//! `keyByDataSource`, `assertDataSource`, `convertTimeStrToSeconds`) and of
//! `BaseDriver.getSslOptions`.
//!
//! Environment variable naming (see [`env_key`]):
//!
//! * single data source (no `CUBEJS_DATASOURCES`): `CUBEJS_DB_HOST`
//! * multiple data sources, non-default one: `CUBEJS_DS_<NAME>_DB_HOST`
//! * pre-aggregation storage: `CUBEJS_PRE_AGGREGATIONS_DB_HOST` /
//!   `CUBEJS_DS_<NAME>_PRE_AGGREGATIONS_DB_HOST`

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use crate::error::{DriverError, Result};

/// Name of the implicit data source.
pub const DEFAULT_DATA_SOURCE: &str = "default";

/// Default pool size when `CUBEJS_DB_MAX_POOL` is not set (Postgres driver).
pub const DEFAULT_MAX_POOL_SIZE: usize = 8;
/// Default query timeout (`CUBEJS_DB_QUERY_TIMEOUT` defaults to `10m`).
pub const DEFAULT_QUERY_TIMEOUT: Duration = Duration::from_secs(600);
/// Default poll interval (`CUBEJS_DB_POLL_MAX_INTERVAL` defaults to `5s`).
pub const DEFAULT_POLL_MAX_INTERVAL: Duration = Duration::from_secs(5);
/// Default `testConnectionTimeout` of `BaseDriver` (10 s).
pub const DEFAULT_TEST_CONNECTION_TIMEOUT: Duration = Duration::from_millis(10_000);

/// Abstraction over `process.env` so that configuration parsing can be unit
/// tested without mutating the process environment.
pub trait EnvSource: Send + Sync {
    /// Returns the raw value of `key`, `None` when unset.
    fn get(&self, key: &str) -> Option<String>;
}

/// [`EnvSource`] backed by the real process environment.
#[derive(Debug, Default, Clone, Copy)]
pub struct ProcessEnv;

impl EnvSource for ProcessEnv {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

impl EnvSource for HashMap<String, String> {
    fn get(&self, key: &str) -> Option<String> {
        HashMap::get(self, key).cloned()
    }
}

impl<const N: usize> EnvSource for [(&'static str, &'static str); N] {
    fn get(&self, key: &str) -> Option<String> {
        self.iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| (*v).to_string())
    }
}

/// Parses `CUBEJS_DATASOURCES` (`"default, other"` → `["default", "other"]`).
pub fn data_sources(env: &dyn EnvSource) -> Vec<String> {
    match env.get("CUBEJS_DATASOURCES") {
        Some(v) if !v.trim().is_empty() => {
            v.trim().split(',').map(|s| s.trim().to_string()).collect()
        }
        _ => Vec::new(),
    }
}

/// Port of `assertDataSource`: returns the data source name when it is
/// declared (or when no `CUBEJS_DATASOURCES` are declared at all).
pub fn assert_data_source(declared: &[String], data_source: Option<&str>) -> Result<String> {
    let ds = data_source.unwrap_or(DEFAULT_DATA_SOURCE);
    if declared.is_empty() || declared.iter().any(|d| d == ds) {
        Ok(ds.to_string())
    } else {
        Err(DriverError::Config(format!(
            "The {ds} data source is missing in the declared CUBEJS_DATASOURCES."
        )))
    }
}

/// Port of `keyByDataSource`: maps a `CUBEJS_*` variable name to its
/// data-source / pre-aggregations specific counterpart.
pub fn env_key(
    origin: &str,
    declared: &[String],
    data_source: Option<&str>,
    pre_aggregations: bool,
) -> Result<String> {
    if let Some(ds) = data_source {
        assert_data_source(declared, Some(ds))?;
    }

    let key = match data_source {
        Some(ds) if !declared.is_empty() && ds != DEFAULT_DATA_SOURCE => {
            match origin.strip_prefix("CUBEJS_") {
                Some(rest) if !rest.contains("CUBEJS_") => {
                    format!("CUBEJS_DS_{}_{}", ds.to_uppercase(), rest)
                }
                _ => {
                    return Err(DriverError::Config(format!(
                        "The {origin} environment variable can not be converted for the {ds} data source."
                    )))
                }
            }
        }
        _ => origin.to_string(),
    };

    if pre_aggregations {
        if let Some(rest) = key.strip_prefix("CUBEJS_DS_") {
            // `^(CUBEJS_DS_[A-Z0-9_]+?_)(DB_|JDBC_|AWS_|DATABASE|FIREBOLT_)(.*)`
            // Non-greedy: the first `_` boundary after which a known section starts.
            let mut idx = 0;
            while let Some(pos) = rest[idx..].find('_') {
                let split = idx + pos + 1;
                let tail = &rest[split..];
                let section_ok = ["DB_", "JDBC_", "AWS_", "DATABASE", "FIREBOLT_"]
                    .iter()
                    .any(|s| tail.starts_with(s));
                let head = &rest[..split - 1];
                let head_ok = !head.is_empty()
                    && head
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
                if section_ok && head_ok {
                    return Ok(format!("CUBEJS_DS_{}_PRE_AGGREGATIONS_{}", head, tail));
                }
                idx = split;
            }
        }
        if let Some(rest) = key.strip_prefix("CUBEJS_") {
            return Ok(format!("CUBEJS_PRE_AGGREGATIONS_{rest}"));
        }
    }

    Ok(key)
}

/// Port of `convertTimeStrToSeconds`: `"30"`, `"30s"`, `"5m"`, `"1h"`.
pub fn parse_duration(input: &str, env_name: &str) -> Result<Duration> {
    const DESCRIPTION: &str = "Must be a number in seconds or duration string (1s, 1m, 1h).";

    if !input.is_empty() && input.chars().all(|c| c.is_ascii_digit()) {
        return input
            .parse::<u64>()
            .map(Duration::from_secs)
            .map_err(|_| DriverError::invalid_configuration(env_name, input, DESCRIPTION));
    }

    if input.len() > 1 {
        let (num, unit) = input.split_at(input.len() - 1);
        let multiplier = match unit.to_ascii_lowercase().as_str() {
            "h" => Some(3600),
            "m" => Some(60),
            "s" => Some(1),
            _ => None,
        };
        if let Some(m) = multiplier {
            if let Ok(n) = num.parse::<u64>() {
                return Ok(Duration::from_secs(n * m));
            }
        }
    }

    Err(DriverError::invalid_configuration(
        env_name,
        input,
        DESCRIPTION,
    ))
}

/// Strict boolean parsing (`asBoolStrict`): only `true` / `false`.
fn parse_bool_strict(value: &str, key: &str) -> Result<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(DriverError::Config(format!(
            "The {key} must be either 'true' or 'false'."
        ))),
    }
}

/// Port of `isSslCert`.
pub fn is_ssl_cert(content: &str) -> bool {
    content.starts_with("-----BEGIN CERTIFICATE-----")
}

/// Port of `isSslKey` (plus PKCS#8 which rustls handles natively).
pub fn is_ssl_key(content: &str) -> bool {
    content.starts_with("-----BEGIN RSA PRIVATE KEY-----")
        || content.starts_with("-----BEGIN EC PRIVATE KEY-----")
        || content.starts_with("-----BEGIN PRIVATE KEY-----")
}

/// Port of `isFilePath`: has a directory part (or root) and a file name.
pub fn is_file_path(fp: &str) -> bool {
    if fp.is_empty() {
        return false;
    }
    let p = Path::new(fp);
    let has_dir = p
        .parent()
        .map(|d| !d.as_os_str().is_empty())
        .unwrap_or(false);
    let has_name = p.file_name().is_some();
    has_dir && has_name
}

/// TLS options resolved from `CUBEJS_DB_SSL*` (port of `BaseDriver.getSslOptions`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SslConfig {
    /// PEM CA bundle (`CUBEJS_DB_SSL_CA`, inline or file path).
    pub ca: Option<String>,
    /// PEM client certificate (`CUBEJS_DB_SSL_CERT`).
    pub cert: Option<String>,
    /// PEM client private key (`CUBEJS_DB_SSL_KEY`).
    pub key: Option<String>,
    /// OpenSSL cipher list (`CUBEJS_DB_SSL_CIPHERS`). Kept for completeness;
    /// rustls does not accept OpenSSL cipher strings, so it is ignored.
    pub ciphers: Option<String>,
    /// Private-key passphrase (`CUBEJS_DB_SSL_PASSPHRASE`). Encrypted PEM keys
    /// are not supported by rustls; a configured passphrase is an error at connect time.
    pub passphrase: Option<String>,
    /// SNI / verification host name override (`CUBEJS_DB_SSL_SERVERNAME`).
    pub servername: Option<String>,
    /// `CUBEJS_DB_SSL_REJECT_UNAUTHORIZED` (default `false`: accept any server certificate).
    pub reject_unauthorized: bool,
}

/// Per data source connection settings (`CUBEJS_DB_*`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DataSourceConfig {
    /// Data source name (`default` when not using `CUBEJS_DATASOURCES`).
    pub data_source: String,
    /// Whether this configuration targets the pre-aggregations storage
    /// (`CUBEJS_PRE_AGGREGATIONS_DB_*`).
    pub pre_aggregations: bool,
    /// `CUBEJS_DB_TYPE` (never pre-aggregations specific).
    pub db_type: Option<String>,
    /// `CUBEJS_DB_URL`.
    pub url: Option<String>,
    /// `CUBEJS_DB_HOST`.
    pub host: Option<String>,
    /// `CUBEJS_DB_PORT`.
    pub port: Option<u16>,
    /// `CUBEJS_DB_SOCKET_PATH`.
    pub socket_path: Option<String>,
    /// `CUBEJS_DB_USER`.
    pub user: Option<String>,
    /// `CUBEJS_DB_PASS`.
    pub password: Option<String>,
    /// `CUBEJS_DB_NAME`.
    pub database: Option<String>,
    /// `CUBEJS_DB_SCHEMA` (deprecated alias of `CUBEJS_DB_NAME` in some drivers).
    pub schema: Option<String>,
    /// `CUBEJS_DB_SSL` / `CUBEJS_DB_SSL_*`; `None` when SSL is disabled.
    pub ssl: Option<SslConfig>,
    /// `CUBEJS_DB_MAX_POOL`.
    pub max_pool_size: Option<usize>,
    /// `CUBEJS_DB_MIN_POOL`.
    pub min_pool_size: Option<usize>,
    /// `CUBEJS_DB_QUERY_TIMEOUT` (default 10 minutes).
    pub query_timeout: Duration,
    /// `CUBEJS_DB_POLL_MAX_INTERVAL` (default 5 s).
    pub poll_max_interval: Duration,
    /// `CUBEJS_DB_POLL_TIMEOUT`.
    pub poll_timeout: Option<Duration>,
    /// `CUBEJS_DB_EXPORT_BUCKET_CSV_ESCAPE_SYMBOL`.
    pub export_bucket_csv_escape_symbol: Option<String>,
}

impl DataSourceConfig {
    /// Reads the configuration of `data_source` from the process environment.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Self::from_env_source(&ProcessEnv, data_source, false)
    }

    /// Reads the configuration from an arbitrary [`EnvSource`].
    pub fn from_env_source(
        env: &dyn EnvSource,
        data_source: Option<&str>,
        pre_aggregations: bool,
    ) -> Result<Self> {
        let reader = EnvReader::new(env, data_source, pre_aggregations)?;

        let db_type = reader.get_no_pre_aggs("CUBEJS_DB_TYPE")?;
        let url = reader.get("CUBEJS_DB_URL")?;
        let host = reader.get("CUBEJS_DB_HOST")?;
        let port = reader.get_port("CUBEJS_DB_PORT")?;
        let socket_path = reader.get("CUBEJS_DB_SOCKET_PATH")?;
        let user = reader.get("CUBEJS_DB_USER")?;
        let password = reader.get("CUBEJS_DB_PASS")?;
        let database = reader.get("CUBEJS_DB_NAME")?;
        let schema = reader.get("CUBEJS_DB_SCHEMA")?;
        let ssl = reader.ssl()?;
        let max_pool_size = reader.get_usize("CUBEJS_DB_MAX_POOL")?;
        let min_pool_size = reader.get_usize("CUBEJS_DB_MIN_POOL")?;
        let query_timeout = reader
            .get_duration("CUBEJS_DB_QUERY_TIMEOUT")?
            .unwrap_or(DEFAULT_QUERY_TIMEOUT);
        let poll_max_interval = reader
            .get_duration("CUBEJS_DB_POLL_MAX_INTERVAL")?
            .unwrap_or(DEFAULT_POLL_MAX_INTERVAL);
        let poll_timeout = reader.get_duration("CUBEJS_DB_POLL_TIMEOUT")?;
        let export_bucket_csv_escape_symbol =
            reader.get("CUBEJS_DB_EXPORT_BUCKET_CSV_ESCAPE_SYMBOL")?;

        Ok(Self {
            data_source: reader.data_source,
            pre_aggregations,
            db_type,
            url,
            host,
            port,
            socket_path,
            user,
            password,
            database,
            schema,
            ssl,
            max_pool_size,
            min_pool_size,
            query_timeout,
            poll_max_interval,
            poll_timeout,
            export_bucket_csv_escape_symbol,
        })
    }

    /// Effective pool size (`CUBEJS_DB_MAX_POOL` or 8).
    pub fn effective_max_pool_size(&self) -> usize {
        self.max_pool_size
            .filter(|v| *v > 0)
            .unwrap_or(DEFAULT_MAX_POOL_SIZE)
    }
}

/// Driver-wide settings: the data source plus the global knobs `BaseDriver`
/// reads through `getEnv` without a data source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverConfig {
    /// Connection settings.
    pub data_source: DataSourceConfig,
    /// `CUBEJS_DB_PRECISE_DECIMAL_IN_CUBESTORE` (default `false`).
    pub precise_decimal_in_cubestore: bool,
    /// `CUBEJS_DB_FETCH_COLUMNS_BY_ORDINAL_POSITION` (default `true`).
    pub fetch_columns_by_ordinal_position: bool,
    /// `testConnectionTimeout` option of `BaseDriver` (default 10 s).
    pub test_connection_timeout: Duration,
}

impl Default for DriverConfig {
    fn default() -> Self {
        Self {
            data_source: DataSourceConfig {
                data_source: DEFAULT_DATA_SOURCE.to_string(),
                query_timeout: DEFAULT_QUERY_TIMEOUT,
                poll_max_interval: DEFAULT_POLL_MAX_INTERVAL,
                ..Default::default()
            },
            precise_decimal_in_cubestore: false,
            fetch_columns_by_ordinal_position: true,
            test_connection_timeout: DEFAULT_TEST_CONNECTION_TIMEOUT,
        }
    }
}

impl DriverConfig {
    /// Reads everything from the process environment.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Self::from_env_source(&ProcessEnv, data_source, false)
    }

    /// Reads everything from an arbitrary [`EnvSource`].
    pub fn from_env_source(
        env: &dyn EnvSource,
        data_source: Option<&str>,
        pre_aggregations: bool,
    ) -> Result<Self> {
        let data_source = DataSourceConfig::from_env_source(env, data_source, pre_aggregations)?;
        let precise_decimal_in_cubestore = match env.get("CUBEJS_DB_PRECISE_DECIMAL_IN_CUBESTORE") {
            Some(v) if !v.is_empty() => {
                parse_bool_strict(&v, "CUBEJS_DB_PRECISE_DECIMAL_IN_CUBESTORE")?
            }
            _ => false,
        };
        let fetch_columns_by_ordinal_position =
            match env.get("CUBEJS_DB_FETCH_COLUMNS_BY_ORDINAL_POSITION") {
                Some(v) if !v.is_empty() => {
                    parse_bool_strict(&v, "CUBEJS_DB_FETCH_COLUMNS_BY_ORDINAL_POSITION")?
                }
                _ => true,
            };
        Ok(Self {
            data_source,
            precise_decimal_in_cubestore,
            fetch_columns_by_ordinal_position,
            test_connection_timeout: DEFAULT_TEST_CONNECTION_TIMEOUT,
        })
    }

    /// Convenience constructor for programmatic configuration.
    pub fn with_data_source(data_source: DataSourceConfig) -> Self {
        Self {
            data_source,
            ..Default::default()
        }
    }
}

/// Helper resolving data-source specific keys against an [`EnvSource`].
struct EnvReader<'a> {
    env: &'a dyn EnvSource,
    declared: Vec<String>,
    data_source: String,
    pre_aggregations: bool,
}

impl<'a> EnvReader<'a> {
    fn new(
        env: &'a dyn EnvSource,
        data_source: Option<&str>,
        pre_aggregations: bool,
    ) -> Result<Self> {
        let declared = data_sources(env);
        let data_source = assert_data_source(&declared, data_source)?;
        Ok(Self {
            env,
            declared,
            data_source,
            pre_aggregations,
        })
    }

    fn key(&self, origin: &str, pre_aggregations: bool) -> Result<String> {
        env_key(
            origin,
            &self.declared,
            Some(&self.data_source),
            pre_aggregations,
        )
    }

    fn raw(&self, origin: &str, pre_aggregations: bool) -> Result<(String, Option<String>)> {
        let key = self.key(origin, pre_aggregations)?;
        let value = self.env.get(&key).filter(|v| !v.is_empty());
        Ok((key, value))
    }

    fn get(&self, origin: &str) -> Result<Option<String>> {
        Ok(self.raw(origin, self.pre_aggregations)?.1)
    }

    fn get_no_pre_aggs(&self, origin: &str) -> Result<Option<String>> {
        Ok(self.raw(origin, false)?.1)
    }

    fn get_usize(&self, origin: &str) -> Result<Option<usize>> {
        let (key, value) = self.raw(origin, self.pre_aggregations)?;
        value
            .map(|v| {
                v.trim().parse::<usize>().map_err(|_| {
                    DriverError::Config(format!("env-var: \"{key}\" should be a valid integer"))
                })
            })
            .transpose()
    }

    fn get_port(&self, origin: &str) -> Result<Option<u16>> {
        let (key, value) = self.raw(origin, self.pre_aggregations)?;
        value
            .map(|v| {
                v.trim().parse::<u16>().map_err(|_| {
                    DriverError::Config(format!(
                        "env-var: \"{key}\" should be a valid port number (0-65535)"
                    ))
                })
            })
            .transpose()
    }

    fn get_duration(&self, origin: &str) -> Result<Option<Duration>> {
        let (key, value) = self.raw(origin, self.pre_aggregations)?;
        value.map(|v| parse_duration(&v, &key)).transpose()
    }

    fn get_bool(&self, origin: &str) -> Result<bool> {
        let (_, value) = self.raw(origin, self.pre_aggregations)?;
        match value {
            Some(v) => parse_bool_strict(&v, &self.key(origin, false)?),
            None => Ok(false),
        }
    }

    /// Port of `BaseDriver.getSslOptions`.
    fn ssl(&self) -> Result<Option<SslConfig>> {
        let use_ssl = self.get_bool("CUBEJS_DB_SSL")?;
        let reject_unauthorized = self.get_bool("CUBEJS_DB_SSL_REJECT_UNAUTHORIZED")?;
        if !use_ssl && !reject_unauthorized {
            return Ok(None);
        }

        // Note: the Node.js implementation resolves these keys without the
        // pre-aggregations flag; we keep that behaviour.
        let ca = self.ssl_material("CUBEJS_DB_SSL_CA", "ca", is_ssl_cert)?;
        let cert = self.ssl_material("CUBEJS_DB_SSL_CERT", "cert", is_ssl_cert)?;
        let key = self.ssl_material("CUBEJS_DB_SSL_KEY", "key", is_ssl_key)?;
        let ciphers = self.raw("CUBEJS_DB_SSL_CIPHERS", false)?.1;
        let passphrase = self.raw("CUBEJS_DB_SSL_PASSPHRASE", false)?.1;
        let servername = self.raw("CUBEJS_DB_SSL_SERVERNAME", false)?.1;

        Ok(Some(SslConfig {
            ca,
            cert,
            key,
            ciphers,
            passphrase,
            servername,
            reject_unauthorized,
        }))
    }

    fn ssl_material(
        &self,
        origin: &str,
        name: &str,
        validate: fn(&str) -> bool,
    ) -> Result<Option<String>> {
        let (key, value) = self.raw(origin, false)?;
        let Some(value) = value else {
            return Ok(None);
        };

        if validate(&value) {
            return Ok(Some(value));
        }

        if is_file_path(&value) {
            if !Path::new(&value).exists() {
                return Err(DriverError::Config(format!(
                    "Unable to find {name} from path: \"{value}\""
                )));
            }
            let content = std::fs::read_to_string(&value)?;
            if validate(&content) {
                return Ok(Some(content));
            }
            return Err(DriverError::Config(format!(
                "Content of the file from {key} is not a valid SSL {name}."
            )));
        }

        Err(DriverError::Config(format!(
            "{key} is not a valid SSL {name}. If it's a path, please specify it correctly"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn env_key_single_data_source() {
        let declared: Vec<String> = vec![];
        assert_eq!(
            env_key("CUBEJS_DB_HOST", &declared, None, false).unwrap(),
            "CUBEJS_DB_HOST"
        );
        assert_eq!(
            env_key("CUBEJS_DB_HOST", &declared, Some("default"), false).unwrap(),
            "CUBEJS_DB_HOST"
        );
        assert_eq!(
            env_key("CUBEJS_DB_HOST", &declared, Some("default"), true).unwrap(),
            "CUBEJS_PRE_AGGREGATIONS_DB_HOST"
        );
    }

    #[test]
    fn env_key_multiple_data_sources() {
        let declared = vec!["default".to_string(), "postgres".to_string()];
        assert_eq!(
            env_key("CUBEJS_DB_HOST", &declared, Some("postgres"), false).unwrap(),
            "CUBEJS_DS_POSTGRES_DB_HOST"
        );
        assert_eq!(
            env_key("CUBEJS_DB_HOST", &declared, Some("default"), false).unwrap(),
            "CUBEJS_DB_HOST"
        );
        assert_eq!(
            env_key("CUBEJS_DB_HOST", &declared, Some("postgres"), true).unwrap(),
            "CUBEJS_DS_POSTGRES_PRE_AGGREGATIONS_DB_HOST"
        );
        assert_eq!(
            env_key("CUBEJS_JDBC_URL", &declared, Some("postgres"), true).unwrap(),
            "CUBEJS_DS_POSTGRES_PRE_AGGREGATIONS_JDBC_URL"
        );
        assert_eq!(
            env_key("CUBEJS_CONCURRENCY", &declared, Some("postgres"), true).unwrap(),
            "CUBEJS_PRE_AGGREGATIONS_DS_POSTGRES_CONCURRENCY"
        );
        let err = env_key("CUBEJS_DB_HOST", &declared, Some("missing"), false).unwrap_err();
        assert_eq!(
            err.to_string(),
            "The missing data source is missing in the declared CUBEJS_DATASOURCES."
        );
    }

    #[test]
    fn env_key_with_underscored_data_source_name() {
        // The non-greedy regex in `keyByDataSource` splits at the first `_`
        // followed by a known section; we reproduce that quirk verbatim.
        let declared = vec!["default".to_string(), "my_db".to_string()];
        assert_eq!(
            env_key("CUBEJS_DB_HOST", &declared, Some("my_db"), true).unwrap(),
            "CUBEJS_DS_MY_PRE_AGGREGATIONS_DB_DB_HOST"
        );
        let declared = vec!["default".to_string(), "sales2".to_string()];
        assert_eq!(
            env_key("CUBEJS_AWS_KEY", &declared, Some("sales2"), true).unwrap(),
            "CUBEJS_DS_SALES2_PRE_AGGREGATIONS_AWS_KEY"
        );
    }

    #[test]
    fn parse_duration_variants() {
        assert_eq!(parse_duration("30", "K").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("30s", "K").unwrap(), Duration::from_secs(30));
        assert_eq!(
            parse_duration("10m", "K").unwrap(),
            Duration::from_secs(600)
        );
        assert_eq!(
            parse_duration("2H", "K").unwrap(),
            Duration::from_secs(7200)
        );
        let err = parse_duration("abc", "CUBEJS_DB_QUERY_TIMEOUT").unwrap_err();
        assert_eq!(
            err.to_string(),
            "Value \"abc\" is not valid for CUBEJS_DB_QUERY_TIMEOUT. Must be a number in seconds or duration string (1s, 1m, 1h)."
        );
    }

    #[test]
    fn file_path_detection() {
        assert!(is_file_path("/etc/ssl/ca.pem"));
        assert!(is_file_path("./certs/ca.pem"));
        assert!(!is_file_path("ca.pem"));
        assert!(!is_file_path(""));
        assert!(!is_file_path("-----BEGIN CERTIFICATE-----"));
    }

    #[test]
    fn defaults_when_nothing_is_set() {
        let cfg = DriverConfig::from_env_source(&env(&[]), None, false).unwrap();
        assert_eq!(cfg.data_source.data_source, "default");
        assert_eq!(cfg.data_source.host, None);
        assert_eq!(cfg.data_source.port, None);
        assert_eq!(cfg.data_source.ssl, None);
        assert_eq!(cfg.data_source.query_timeout, Duration::from_secs(600));
        assert_eq!(cfg.data_source.poll_max_interval, Duration::from_secs(5));
        assert_eq!(cfg.data_source.effective_max_pool_size(), 8);
        assert!(!cfg.precise_decimal_in_cubestore);
        assert!(cfg.fetch_columns_by_ordinal_position);
    }

    #[test]
    fn reads_single_data_source() {
        let e = env(&[
            ("CUBEJS_DB_TYPE", "postgres"),
            ("CUBEJS_DB_HOST", "db.local"),
            ("CUBEJS_DB_PORT", "5433"),
            ("CUBEJS_DB_NAME", "cube"),
            ("CUBEJS_DB_USER", "u"),
            ("CUBEJS_DB_PASS", "p"),
            ("CUBEJS_DB_MAX_POOL", "16"),
            ("CUBEJS_DB_QUERY_TIMEOUT", "2m"),
            ("CUBEJS_DB_SSL", "true"),
            ("CUBEJS_DB_SSL_REJECT_UNAUTHORIZED", "true"),
            ("CUBEJS_DB_SSL_SERVERNAME", "db.example.com"),
            ("CUBEJS_DB_PRECISE_DECIMAL_IN_CUBESTORE", "true"),
            ("CUBEJS_DB_FETCH_COLUMNS_BY_ORDINAL_POSITION", "false"),
        ]);
        let cfg = DriverConfig::from_env_source(&e, None, false).unwrap();
        let ds = &cfg.data_source;
        assert_eq!(ds.db_type.as_deref(), Some("postgres"));
        assert_eq!(ds.host.as_deref(), Some("db.local"));
        assert_eq!(ds.port, Some(5433));
        assert_eq!(ds.database.as_deref(), Some("cube"));
        assert_eq!(ds.user.as_deref(), Some("u"));
        assert_eq!(ds.password.as_deref(), Some("p"));
        assert_eq!(ds.max_pool_size, Some(16));
        assert_eq!(ds.query_timeout, Duration::from_secs(120));
        let ssl = ds.ssl.as_ref().unwrap();
        assert!(ssl.reject_unauthorized);
        assert_eq!(ssl.servername.as_deref(), Some("db.example.com"));
        assert!(cfg.precise_decimal_in_cubestore);
        assert!(!cfg.fetch_columns_by_ordinal_position);
    }

    #[test]
    fn ssl_disabled_by_default_and_validates_values() {
        let e = env(&[("CUBEJS_DB_SSL", "false")]);
        assert_eq!(
            DataSourceConfig::from_env_source(&e, None, false)
                .unwrap()
                .ssl,
            None
        );

        let e = env(&[("CUBEJS_DB_SSL", "yes")]);
        let err = DataSourceConfig::from_env_source(&e, None, false).unwrap_err();
        assert_eq!(
            err.to_string(),
            "The CUBEJS_DB_SSL must be either 'true' or 'false'."
        );

        let e = env(&[
            ("CUBEJS_DB_SSL", "true"),
            ("CUBEJS_DB_SSL_CA", "not-a-cert"),
        ]);
        let err = DataSourceConfig::from_env_source(&e, None, false).unwrap_err();
        assert_eq!(
            err.to_string(),
            "CUBEJS_DB_SSL_CA is not a valid SSL ca. If it's a path, please specify it correctly"
        );

        let e = env(&[
            ("CUBEJS_DB_SSL", "true"),
            ("CUBEJS_DB_SSL_CA", "/definitely/missing/ca.pem"),
        ]);
        let err = DataSourceConfig::from_env_source(&e, None, false).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Unable to find ca from path: \"/definitely/missing/ca.pem\""
        );

        let e = env(&[
            ("CUBEJS_DB_SSL", "true"),
            (
                "CUBEJS_DB_SSL_CA",
                "-----BEGIN CERTIFICATE-----\nabc\n-----END CERTIFICATE-----",
            ),
        ]);
        let ssl = DataSourceConfig::from_env_source(&e, None, false)
            .unwrap()
            .ssl
            .unwrap();
        assert!(ssl.ca.unwrap().starts_with("-----BEGIN CERTIFICATE-----"));
        assert!(!ssl.reject_unauthorized);
    }

    #[test]
    fn reads_named_data_source_and_pre_aggregations() {
        let e = env(&[
            ("CUBEJS_DATASOURCES", "default, sales"),
            ("CUBEJS_DB_HOST", "default-host"),
            ("CUBEJS_DB_TYPE", "postgres"),
            ("CUBEJS_DS_SALES_DB_TYPE", "mysql"),
            ("CUBEJS_DS_SALES_DB_HOST", "sales-host"),
            ("CUBEJS_DS_SALES_DB_PORT", "3306"),
            (
                "CUBEJS_DS_SALES_PRE_AGGREGATIONS_DB_HOST",
                "sales-preaggs-host",
            ),
        ]);
        let cfg = DataSourceConfig::from_env_source(&e, Some("sales"), false).unwrap();
        assert_eq!(cfg.data_source, "sales");
        assert_eq!(cfg.db_type.as_deref(), Some("mysql"));
        assert_eq!(cfg.host.as_deref(), Some("sales-host"));
        assert_eq!(cfg.port, Some(3306));

        let pre = DataSourceConfig::from_env_source(&e, Some("sales"), true).unwrap();
        assert!(pre.pre_aggregations);
        assert_eq!(pre.host.as_deref(), Some("sales-preaggs-host"));
        // db type is never pre-aggregation specific
        assert_eq!(pre.db_type.as_deref(), Some("mysql"));

        let def = DataSourceConfig::from_env_source(&e, None, false).unwrap();
        assert_eq!(def.host.as_deref(), Some("default-host"));

        let err = DataSourceConfig::from_env_source(&e, Some("nope"), false).unwrap_err();
        assert!(matches!(err, DriverError::Config(_)));
    }

    #[test]
    fn invalid_numbers_are_reported() {
        let e = env(&[("CUBEJS_DB_PORT", "abc")]);
        let err = DataSourceConfig::from_env_source(&e, None, false).unwrap_err();
        assert!(err.to_string().contains("CUBEJS_DB_PORT"));
        let e = env(&[("CUBEJS_DB_MAX_POOL", "-1")]);
        assert!(DataSourceConfig::from_env_source(&e, None, false).is_err());
    }
}
