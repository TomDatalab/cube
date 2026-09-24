//! Trino driver: port of `@cubejs-backend/trino-driver`.
//!
//! In Node `TrinoDriver extends PrestoDriver` with `engine: 'trino'` (the
//! `X-Trino-*` header family) and its own `testConnection`, which reads
//! `GET /v1/info` instead of the node list. Everything else — configuration,
//! SQL, streaming, export-bucket unload — is the Presto driver's, so this
//! wraps [`PrestoDriver`] and delegates to it.
//!
//! `dialectClass()` (`PrestodbQuery`) belongs to the SQL planner, not to the
//! driver layer.

use async_trait::async_trait;
use serde_json::Value;

use crate::config::DriverConfig;
use crate::driver::Driver;
use crate::error::{DriverError, Result};
use crate::prestodb::{Engine, PrestoConfig, PrestoDriver};
use crate::types::{
    DownloadQueryResultsOptions, DownloadedData, DriverCapabilities, QueryOptions, QueryResult,
    StreamOptions, StreamTableData, TableCsvData, TableStructure, UnloadOptions,
};

/// `getDefaultConcurrency` (inherited from the Presto driver).
pub const DEFAULT_CONCURRENCY: usize = crate::prestodb::DEFAULT_CONCURRENCY;

/// Configuration of [`TrinoDriver`]: the Presto configuration with the
/// Trino engine.
pub type TrinoConfig = PrestoConfig;

/// Builds a Trino configuration from the generic `CUBEJS_DB_*` settings.
pub fn trino_config_from_driver_config(driver: DriverConfig) -> TrinoConfig {
    PrestoConfig::from_driver_config(driver, Engine::Trino)
}

/// Trino driver.
#[derive(Debug)]
pub struct TrinoDriver {
    inner: PrestoDriver,
}

impl TrinoDriver {
    /// Creates the driver; the engine is forced to Trino, as in Node.
    pub fn new(mut config: TrinoConfig) -> Result<Self> {
        config.engine = Engine::Trino;
        Ok(Self {
            inner: PrestoDriver::new(config)?,
        })
    }

    /// Creates the driver from `CUBEJS_DB_*`.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Self::new(PrestoConfig::from_env(data_source, Engine::Trino)?)
    }

    /// The underlying Presto driver.
    pub fn presto(&self) -> &PrestoDriver {
        &self.inner
    }

    /// `GET /v1/info` with the custom headers and the authorization.
    async fn test_connection_via_info(&self) -> Result<()> {
        let config = self.inner.presto_config();
        let client = self.inner.client();
        let mut headers = reqwest::header::HeaderMap::new();
        for (name, value) in &config.headers {
            headers.insert(
                reqwest::header::HeaderName::from_bytes(name.as_bytes()).map_err(|e| {
                    DriverError::Config(format!("Invalid HTTP header name \"{name}\": {e}"))
                })?,
                reqwest::header::HeaderValue::from_str(value).map_err(|e| {
                    DriverError::Config(format!("Invalid value of HTTP header \"{name}\": {e}"))
                })?,
            );
        }
        // `custom_auth` wins over `basic_auth`; both cannot be set anyway.
        if let Some(authorization) = config.authorization()? {
            headers.insert(
                reqwest::header::AUTHORIZATION,
                reqwest::header::HeaderValue::from_str(&authorization).map_err(|e| {
                    DriverError::Config(format!("Invalid authorization header: {e}"))
                })?,
            );
        }
        let url = format!("{}/v1/info", client.base_url());
        let response = client
            .http()
            .get(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| DriverError::Connection {
                pool_name: "trino".to_string(),
                message: e.to_string(),
            })?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(DriverError::Connection {
                pool_name: "trino".to_string(),
                message: format!(
                    "Connection test failed: {} {} - {text}",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("")
                ),
            });
        }
        Ok(())
    }
}

#[async_trait]
impl Driver for TrinoDriver {
    fn config(&self) -> &DriverConfig {
        self.inner.config()
    }

    async fn test_connection(&self) -> Result<()> {
        if self.inner.presto_config().use_select_test_connection {
            return self.inner.test_connection_via_select().await;
        }
        self.test_connection_via_info().await
    }

    async fn query(
        &self,
        sql: &str,
        params: &[Value],
        options: &QueryOptions,
    ) -> Result<QueryResult> {
        self.inner.query(sql, params, options).await
    }

    fn capabilities(&self) -> DriverCapabilities {
        self.inner.capabilities()
    }

    fn information_schema_query(&self) -> String {
        self.inner.information_schema_query()
    }

    fn get_schemas_query(&self) -> String {
        self.inner.get_schemas_query()
    }

    fn get_tables_for_specific_schemas_query(&self, schemas_placeholders: &str) -> String {
        self.inner
            .get_tables_for_specific_schemas_query(schemas_placeholders)
    }

    fn get_columns_for_specific_tables_query(&self, condition_string: &str) -> String {
        self.inner
            .get_columns_for_specific_tables_query(condition_string)
    }

    async fn create_schema_if_not_exists(&self, schema_name: &str) -> Result<()> {
        self.inner.create_schema_if_not_exists(schema_name).await
    }

    async fn stream(
        &self,
        sql: &str,
        params: &[Value],
        options: &StreamOptions,
    ) -> Result<StreamTableData> {
        self.inner.stream(sql, params, options).await
    }

    async fn download_query_results(
        &self,
        sql: &str,
        params: &[Value],
        options: &DownloadQueryResultsOptions,
    ) -> Result<DownloadedData> {
        self.inner
            .download_query_results(sql, params, options)
            .await
    }

    async fn query_column_types(
        &self,
        sql: &str,
        params: &[Value],
        options: &QueryOptions,
    ) -> Result<TableStructure> {
        self.inner.query_column_types(sql, params, options).await
    }

    async fn is_unload_supported(&self, options: &UnloadOptions) -> Result<bool> {
        self.inner.is_unload_supported(options).await
    }

    async fn unload(&self, table: &str, options: &UnloadOptions) -> Result<TableCsvData> {
        self.inner.unload(table, options).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SslConfig;
    use crate::prestodb::mock_http::{new_log, start};
    use serde_json::json;
    use std::time::Duration;

    fn config(port: u16) -> TrinoConfig {
        let mut config = trino_config_from_driver_config(DriverConfig::default());
        config.host = "127.0.0.1".into();
        config.port = port;
        config.catalog = Some("tpch".into());
        config.user = Some("cube".into());
        config.check_interval = Duration::from_millis(1);
        config
    }

    // --- headers.test.ts ---

    #[tokio::test]
    async fn forwards_configured_custom_headers_on_test_connection() {
        let server = start(new_log(), |_| (200, "{}".to_string())).await;
        let mut config = config(server.port());
        config.headers = vec![
            ("X-Trino-Source".into(), "cube".into()),
            ("X-Trino-Routing-Group".into(), "etl".into()),
            (
                "X-Trino-Client-Tags".into(),
                "user=alice@example.com".into(),
            ),
            ("X-Mozart-User-Token".into(), "abc.def.ghi".into()),
        ];
        TrinoDriver::new(config)
            .unwrap()
            .test_connection()
            .await
            .unwrap();
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "GET");
        assert_eq!(requests[0].path, "/v1/info");
        assert_eq!(requests[0].header("X-Trino-Source"), Some("cube"));
        assert_eq!(requests[0].header("X-Trino-Routing-Group"), Some("etl"));
        assert_eq!(
            requests[0].header("X-Trino-Client-Tags"),
            Some("user=alice@example.com")
        );
        assert_eq!(
            requests[0].header("X-Mozart-User-Token"),
            Some("abc.def.ghi")
        );
    }

    #[tokio::test]
    async fn forwards_custom_headers_when_use_select_test_connection_is_enabled() {
        let server = start(new_log(), |r| {
            let port = r.port;
            if r.method == "POST" {
                (200, json!({ "id": "q", "infoUri": "x", "nextUri": format!("http://127.0.0.1:{port}/v1/statement/q/1"), "stats": { "state": "QUEUED" } }).to_string())
            } else {
                (200, json!({ "id": "q", "infoUri": "x", "stats": { "state": "FINISHED" }, "columns": [{"name": "_col0", "type": "integer"}], "data": [[1]] }).to_string())
            }
        })
        .await;
        let mut config = config(server.port());
        config.use_select_test_connection = true;
        config.headers = vec![
            ("X-Trino-Source".into(), "cube".into()),
            ("X-Trino-Routing-Group".into(), "etl".into()),
        ];
        TrinoDriver::new(config)
            .unwrap()
            .test_connection()
            .await
            .unwrap();
        let requests = server.requests();
        assert!(requests.iter().all(|r| r.path != "/v1/info"));
        let post = requests.iter().find(|r| r.method == "POST").unwrap();
        assert_eq!(post.body, "SELECT 1");
        // The protocol's own `Source` header wins over the custom one, as in
        // `presto-client`; the other custom headers are kept.
        assert_eq!(post.header("X-Trino-Routing-Group"), Some("etl"));
        assert_eq!(post.header("X-Trino-Source"), Some("nodejs-client"));
        assert_eq!(post.header("X-Trino-Catalog"), Some("tpch"));
        assert_eq!(post.header("X-Trino-User"), Some("cube"));
        assert_eq!(post.header("X-Presto-User"), None);
    }

    #[tokio::test]
    async fn test_connection_sends_authorization_and_reports_failures() {
        let server = start(new_log(), |_| (401, "nope".to_string())).await;
        let mut config = config(server.port());
        config.password = Some("secret".into());
        let err = TrinoDriver::new(config)
            .unwrap()
            .test_connection()
            .await
            .unwrap_err();
        assert!(err
            .to_string()
            .ends_with("Connection test failed: 401 Unauthorized - nope"));
        assert_eq!(
            server.requests()[0].header("Authorization"),
            Some("Basic Y3ViZTpzZWNyZXQ=")
        );

        let server = start(new_log(), |_| (200, "{}".to_string())).await;
        let mut config = self::config(server.port());
        config.auth_token = Some("tok".into());
        TrinoDriver::new(config)
            .unwrap()
            .test_connection()
            .await
            .unwrap();
        assert_eq!(
            server.requests()[0].header("Authorization"),
            Some("Bearer tok")
        );
    }

    // --- ssl.test.ts ---

    #[test]
    fn uses_https_when_ssl_is_configured() {
        let mut config = config(8443);
        config.host = "trino.local".into();
        config.ssl = Some(SslConfig {
            reject_unauthorized: false,
            ..Default::default()
        });
        let driver = TrinoDriver::new(config).unwrap();
        assert_eq!(
            driver.presto().client().base_url(),
            "https://trino.local:8443"
        );

        let mut config = self::config(8080);
        config.host = "trino.local".into();
        let driver = TrinoDriver::new(config).unwrap();
        assert_eq!(
            driver.presto().client().base_url(),
            "http://trino.local:8080"
        );
    }

    #[test]
    fn rejects_an_invalid_ca() {
        let mut config = config(8443);
        config.ssl = Some(SslConfig {
            ca: Some("-----BEGIN CERTIFICATE-----\nMIIC...\n-----END CERTIFICATE-----".into()),
            ..Default::default()
        });
        assert!(TrinoDriver::new(config).is_err());
    }

    #[test]
    fn engine_is_forced_to_trino() {
        let mut config = config(1);
        config.engine = Engine::Presto;
        let driver = TrinoDriver::new(config).unwrap();
        assert_eq!(driver.presto().presto_config().engine, Engine::Trino);
    }
}
