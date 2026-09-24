//! ClickHouse driver: port of `@cubejs-backend/clickhouse-driver` over
//! ClickHouse's HTTP interface with `reqwest` (rustls, never OpenSSL).
//!
//! The Node driver talks to `@clickhouse/client`, which is itself an HTTP
//! client; this port speaks the same protocol directly — `JSONCompact` for
//! result sets, `JSONCompactEachRowWithNamesAndTypes` for streams, `DESCRIBE`
//! for column types — and interpolates query parameters client-side with
//! `formatMySql`, exactly as the Node driver does.

pub mod transform;
pub mod types;

use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::Value;

use crate::config::DriverConfig;
use crate::driver::Driver;
use crate::error::{DriverError, Result};
use crate::escape::format_mysql;
use crate::types::{
    Column, DownloadQueryResultsOptions, DownloadedData, DriverCapabilities, GenericType,
    QueryOptions, QueryResult, Row, SchemaName, StreamOptions, StreamTableData, TableStructure,
};

pub use transform::{column_converter, ColumnConverter};
pub use types::{clickhouse_to_generic, to_generic_type};

/// Default HTTP port.
pub const DEFAULT_PORT: u16 = 8123;
/// Default database (`CUBEJS_DB_NAME`).
pub const DEFAULT_DATABASE: &str = "default";
/// `getDefaultConcurrency`.
pub const DEFAULT_CONCURRENCY: usize = 10;

/// Configuration of [`ClickHouseDriver`] (`ClickHouseDriverConfig`).
#[derive(Debug, Clone)]
pub struct ClickHouseConfig {
    /// Connection settings and global driver knobs.
    pub driver: DriverConfig,
    /// `http(s)://host:port`.
    pub url: String,
    /// `CUBEJS_DB_USER`.
    pub username: Option<String>,
    /// `CUBEJS_DB_PASS`.
    pub password: Option<String>,
    /// `CUBEJS_DB_NAME` (default `default`).
    pub database: String,
    /// `readOnly` passed by the caller; `None` means "not specified".
    pub read_only: Option<bool>,
    /// `CUBEJS_DB_CLICKHOUSE_READONLY`.
    pub read_only_mode: bool,
    /// `CUBEJS_DB_CLICKHOUSE_COMPRESSION`.
    pub compression: bool,
    /// `CUBEJS_DB_QUERY_TIMEOUT` (default 10 minutes).
    pub request_timeout: Duration,
    /// `max_open_connections`.
    pub max_pool_size: usize,
    /// Extra HTTP headers (driver-factory only; no environment variable).
    pub headers: Vec<(String, String)>,
}

impl ClickHouseConfig {
    /// Builds the ClickHouse configuration from a generic [`DriverConfig`].
    pub fn from_driver_config(driver: DriverConfig) -> Self {
        let ds = &driver.data_source;
        let protocol = if ds.ssl.is_some() { "https" } else { "http" };
        let host = ds.host.clone().unwrap_or_default();
        let port = ds.port.unwrap_or(DEFAULT_PORT);
        let url = format!("{protocol}://{host}:{port}");
        let database = ds
            .database
            .clone()
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| DEFAULT_DATABASE.to_string());
        let request_timeout = ds.query_timeout;
        let max_pool_size = ds
            .max_pool_size
            .filter(|v| *v > 0)
            .unwrap_or(DEFAULT_CONCURRENCY);

        Self {
            username: ds.user.clone(),
            password: ds.password.clone(),
            url,
            database,
            read_only: None,
            read_only_mode: false,
            compression: false,
            request_timeout,
            max_pool_size,
            headers: Vec::new(),
            driver,
        }
    }

    /// Reads the configuration of `data_source` from the environment.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        let driver = DriverConfig::from_env(data_source)?;
        let mut config = Self::from_driver_config(driver);
        config.read_only_mode = env_bool("CUBEJS_DB_CLICKHOUSE_READONLY")?;
        config.compression = env_bool("CUBEJS_DB_CLICKHOUSE_COMPRESSION")?;
        Ok(config)
    }

    /// Reads the configuration from an `http://user:pass@host:port/db` URL,
    /// for tests and tooling.
    pub fn from_url(url: &str) -> Result<Self> {
        let parsed = url::Url::parse(url)
            .map_err(|e| DriverError::Config(format!("Invalid ClickHouse URL: {e}")))?;
        let mut driver = DriverConfig::default();
        driver.data_source.host = parsed.host_str().map(|h| h.to_string());
        driver.data_source.port = Some(parsed.port().unwrap_or(DEFAULT_PORT));
        if !parsed.username().is_empty() {
            driver.data_source.user = Some(parsed.username().to_string());
        }
        driver.data_source.password = parsed.password().map(|p| p.to_string());
        let path = parsed.path().trim_start_matches('/');
        if !path.is_empty() {
            driver.data_source.database = Some(path.to_string());
        }
        if parsed.scheme() == "https" {
            driver.data_source.ssl = Some(crate::config::SslConfig::default());
        }
        Ok(Self::from_driver_config(driver))
    }

    /// `clickhouse_settings` sent with every request.
    ///
    /// A `readonly = 1` user may not change settings, so only the progress
    /// header setting is sent in read-only mode.
    pub fn clickhouse_settings(&self) -> Vec<(&'static str, &'static str)> {
        let mut settings = vec![
            // Node's HTTP client caps header size and these can get very large.
            ("send_progress_in_http_headers", "0"),
        ];
        if !self.read_only_mode {
            settings.push(("join_use_nulls", "1"));
            // Pins every DateTime value to the width its column type implies,
            // which is what the converters in `transform` key off.
            settings.push(("date_time_output_format", "simple"));
        }
        settings
    }
}

fn env_bool(key: &str) -> Result<bool> {
    match std::env::var(key) {
        Ok(v) if !v.is_empty() => match v.to_ascii_lowercase().as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(DriverError::Config(format!(
                "The {key} must be either 'true' or 'false'."
            ))),
        },
        _ => Ok(false),
    }
}

/// The `JSONCompact` envelope.
#[derive(Debug, Default, Deserialize)]
struct JsonCompactResponse {
    #[serde(default)]
    meta: Vec<ColumnMeta>,
    #[serde(default)]
    data: Vec<Vec<Value>>,
    #[serde(default)]
    exception: Option<String>,
}

/// One entry of the `meta` array.
#[derive(Debug, Clone, Deserialize)]
struct ColumnMeta {
    name: String,
    #[serde(rename = "type")]
    type_: String,
}

/// ClickHouse driver.
pub struct ClickHouseDriver {
    config: ClickHouseConfig,
    client: reqwest::Client,
}

impl std::fmt::Debug for ClickHouseDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClickHouseDriver")
            .field("url", &self.config.url)
            .field("database", &self.config.database)
            .finish()
    }
}

impl ClickHouseDriver {
    /// Creates the driver and its HTTP client. No request is made until the
    /// first query.
    pub fn new(config: ClickHouseConfig) -> Result<Self> {
        Ok(Self {
            client: Self::build_client(&config, config.max_pool_size)?,
            config,
        })
    }

    /// Creates the driver from `CUBEJS_DB_*`.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Self::new(ClickHouseConfig::from_env(data_source)?)
    }

    fn build_client(config: &ClickHouseConfig, max_pool_size: usize) -> Result<reqwest::Client> {
        let mut headers = reqwest::header::HeaderMap::new();
        let mut insert = |name: &str, value: &str| -> Result<()> {
            let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|e| DriverError::Config(format!("Invalid HTTP header name: {e}")))?;
            let value = reqwest::header::HeaderValue::from_str(value)
                .map_err(|e| DriverError::Config(format!("Invalid HTTP header value: {e}")))?;
            headers.insert(name, value);
            Ok(())
        };
        if let Some(user) = &config.username {
            insert("X-ClickHouse-User", user)?;
        }
        if let Some(password) = &config.password {
            insert("X-ClickHouse-Key", password)?;
        }
        for (name, value) in &config.headers {
            insert(name, value)?;
        }

        let mut builder = reqwest::Client::builder()
            .user_agent(format!("CubeDev_Cube/{}", env!("CARGO_PKG_VERSION")))
            .default_headers(headers)
            .timeout(config.request_timeout)
            .pool_max_idle_per_host(max_pool_size);
        if let Some(ssl) = &config.driver.data_source.ssl {
            if !ssl.reject_unauthorized {
                builder = builder.danger_accept_invalid_certs(true);
            }
            if let Some(ca) = &ssl.ca {
                let cert = reqwest::Certificate::from_pem(ca.as_bytes()).map_err(|e| {
                    DriverError::Config(format!("Invalid CUBEJS_DB_SSL_CA for ClickHouse: {e}"))
                })?;
                builder = builder.add_root_certificate(cert);
            }
        }
        builder
            .build()
            .map_err(|e| DriverError::Config(format!("Unable to build the HTTP client: {e}")))
    }

    /// The driver configuration.
    pub fn clickhouse_config(&self) -> &ClickHouseConfig {
        &self.config
    }

    /// `buildQueryId`: `<request uuid prefix>-<uuid>` or a bare uuid.
    fn build_query_id(&self, request_id: Option<&str>) -> String {
        let prefix = request_id
            .map(extract_request_uuid)
            .filter(|p| !p.is_empty())
            .map(|p| p.chars().take(63).collect::<String>())
            .unwrap_or_default();
        // A random id is enough; Cube only uses it to `KILL QUERY` later.
        let unique = uuid_v4();
        if prefix.is_empty() {
            unique
        } else {
            format!("{prefix}-{unique}")
        }
    }

    fn request(&self, query_id: &str, default_format: Option<&str>) -> reqwest::RequestBuilder {
        let mut request = self
            .client
            .post(&self.config.url)
            .query(&[("database", self.config.database.as_str())])
            .query(&[("query_id", query_id)]);
        if let Some(format) = default_format {
            request = request.query(&[("default_format", format)]);
        }
        for (k, v) in self.config.clickhouse_settings() {
            request = request.query(&[(k, v)]);
        }
        if self.config.compression {
            request = request.query(&[("enable_http_compression", "1")]);
        }
        request
    }

    /// Sends `sql` and returns the raw response body, mapping the HTTP and
    /// ClickHouse errors onto [`DriverError`].
    async fn send(
        &self,
        sql: String,
        query_id: &str,
        default_format: Option<&str>,
    ) -> Result<String> {
        let response = self
            .request(query_id, default_format)
            .body(sql)
            .send()
            .await
            .map_err(|e| DriverError::Connection {
                pool_name: self.config.url.clone(),
                message: e.to_string(),
            })?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| DriverError::Query(e.to_string()))?;
        if !status.is_success() {
            return Err(DriverError::Database {
                message: body.trim().to_string(),
                code: Some(status.as_u16().to_string()),
            });
        }
        Ok(body)
    }

    /// `queryResponse`: runs the query in `JSONCompact` and parses the envelope.
    async fn query_response(
        &self,
        sql: &str,
        params: &[Value],
        options: &QueryOptions,
    ) -> Result<(Vec<Column>, Vec<Row>)> {
        let formatted = format_mysql(sql, params);
        let query_id = self.build_query_id(options.request_id.as_deref());

        let body = self
            .send(formatted, &query_id, Some("JSONCompact"))
            .await
            .map_err(|e| DriverError::Query(format!("Query failed: {e}; query id: {query_id}")))?;

        if body.trim().is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }

        let response: JsonCompactResponse = serde_json::from_str(&body).map_err(|e| {
            DriverError::Query(format!(
                "Query failed: unable to parse the JSONCompact response: {e}; query id: {query_id}"
            ))
        })?;

        // Up to ClickHouse 25.x, failures after the first flushed block are
        // appended to a 200 response; newer versions truncate the JSON and are
        // rejected while parsing it above.
        if let Some(exception) = response.exception {
            return Err(DriverError::Database {
                message: format!(
                    "ClickHouse aborted after {} row(s): {exception}",
                    response.data.len()
                ),
                code: None,
            });
        }

        Ok(self.normalise_response(response))
    }

    /// `normaliseResponse` + the column types derived from `meta`.
    fn normalise_response(&self, response: JsonCompactResponse) -> (Vec<Column>, Vec<Row>) {
        let columns: Vec<Column> = response
            .meta
            .iter()
            .map(|m| Column::new(m.name.clone(), self.to_generic_type(&m.type_, None, None)))
            .collect();
        let converters: Vec<Option<ColumnConverter>> = response
            .meta
            .iter()
            .map(|m| column_converter(&m.type_))
            .collect();

        let rows = response
            .data
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .enumerate()
                    .map(|(i, value)| match converters.get(i).copied().flatten() {
                        Some(converter) => transform::convert(converter, &value),
                        None => value,
                    })
                    .collect()
            })
            .collect();

        (columns, rows)
    }

    /// `command`: runs a statement that returns no result set.
    pub async fn command(&self, sql: &str, options: &QueryOptions) -> Result<()> {
        let query_id = self.build_query_id(options.request_id.as_deref());
        self.send(sql.to_string(), &query_id, None)
            .await
            .map_err(|e| {
                DriverError::Query(format!("Command failed: {e}; query id: {query_id}"))
            })?;
        Ok(())
    }

    /// `KILL QUERY WHERE query_id = ?`, the cancellation path of `withCancel`.
    pub async fn kill_query(&self, query_id: &str) -> Result<()> {
        let sql = format_mysql(
            "KILL QUERY WHERE query_id = ?",
            &[Value::from(query_id.to_string())],
        );
        self.command(&sql, &QueryOptions::default()).await
    }
}

/// `extractRequestUUID`: `"<uuid>-span-<n>"` → `"<uuid>"`.
fn extract_request_uuid(request_id: &str) -> String {
    match request_id.find("-span-") {
        Some(idx) => request_id[..idx].to_string(),
        None => request_id.to_string(),
    }
}

/// A version-4 UUID, without adding a `uuid` dependency for one call site.
fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    // Mixes the monotonic clock with the address of a stack local, which is
    // enough entropy for a query id (it only has to be unique per server).
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let local = 0u8;
    let addr = &local as *const u8 as usize as u128;
    let mut bytes = [0u8; 16];
    bytes[..16].copy_from_slice(&(nanos ^ (addr.rotate_left(64))).to_be_bytes());
    // Version 4, variant 1.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[async_trait]
impl Driver for ClickHouseDriver {
    fn config(&self) -> &DriverConfig {
        &self.config.driver
    }

    async fn test_connection(&self) -> Result<()> {
        self.query("SELECT 1", &[], &QueryOptions::default())
            .await?;
        Ok(())
    }

    async fn query(
        &self,
        sql: &str,
        params: &[Value],
        options: &QueryOptions,
    ) -> Result<QueryResult> {
        let (columns, rows) = self.query_response(sql, params, options).await?;
        Ok(QueryResult::new(columns, rows))
    }

    fn to_generic_type(
        &self,
        db_type: &str,
        precision: Option<i64>,
        scale: Option<i64>,
    ) -> GenericType {
        types::to_generic_type(
            db_type,
            precision,
            scale,
            self.config.driver.precise_decimal_in_cubestore,
        )
    }

    fn read_only(&self) -> bool {
        match self.config.read_only {
            Some(read_only) => read_only || self.config.read_only_mode,
            None if self.config.read_only_mode => true,
            // TODO this is a bit inconsistent with readOnly (same as the Node driver)
            None => true,
        }
    }

    fn capabilities(&self) -> DriverCapabilities {
        DriverCapabilities {
            unload_without_temp_table: true,
            incremental_schema_loading: true,
            ..Default::default()
        }
    }

    fn information_schema_query(&self) -> String {
        format!(
            "
      SELECT name as column_name,
             table as table_name,
             database as table_schema,
             type as data_type
        FROM system.columns
       WHERE database = '{}'
    ",
            self.config.database
        )
    }

    fn get_tables_for_specific_schemas_query(&self, schemas_placeholders: &str) -> String {
        format!(
            "
      SELECT database as schema_name,
            name as table_name
      FROM system.tables
      WHERE database IN ({schemas_placeholders})
    "
        )
    }

    fn get_columns_for_specific_tables_query(&self, condition_string: &str) -> String {
        let q = |i: &str| self.quote_identifier(i);
        format!(
            "
      SELECT name as {},
             table as {},
             database as {},
             type as {}
      FROM system.columns
      WHERE {condition_string}
    ",
            q("column_name"),
            q("table_name"),
            q("schema_name"),
            q("data_type"),
        )
    }

    fn column_name_for_schema_name(&self) -> String {
        "database".to_string()
    }

    fn column_name_for_table_name(&self) -> String {
        "table".to_string()
    }

    async fn get_schemas(&self) -> Result<Vec<SchemaName>> {
        Ok(vec![SchemaName {
            schema_name: self.config.database.clone(),
        }])
    }

    async fn get_tables_query(&self, schema_name: &str) -> Result<Vec<String>> {
        let result = self
            .query(
                "SELECT name as table_name FROM system.tables WHERE database = ?",
                &[Value::from(schema_name)],
                &QueryOptions::default(),
            )
            .await?;
        Ok((0..result.len())
            .filter_map(|i| result.get_string(i, "table_name"))
            .collect())
    }

    async fn create_schema_if_not_exists(&self, schema_name: &str) -> Result<()> {
        self.command(
            &format!("CREATE DATABASE IF NOT EXISTS {schema_name}"),
            &QueryOptions::default(),
        )
        .await
    }

    async fn drop_table(&self, table_name: &str, options: &QueryOptions) -> Result<()> {
        self.command(&format!("DROP TABLE {table_name}"), options)
            .await
    }

    async fn create_table(&self, quoted_table_name: &str, columns: &[Column]) -> Result<()> {
        let create_table_sql = self.create_table_sql(quoted_table_name, columns);
        self.command(&create_table_sql, &QueryOptions::default())
            .await
            .map_err(|e| {
                DriverError::Query(format!("Create table {quoted_table_name} failed: {e}"))
            })
    }

    async fn query_column_types(
        &self,
        sql: &str,
        params: &[Value],
        options: &QueryOptions,
    ) -> Result<TableStructure> {
        // `DESCRIBE` rows have a fixed shape; see
        // https://clickhouse.com/docs/en/sql-reference/statements/describe-table
        let columns = self
            .query(&format!("DESCRIBE {sql}"), params, options)
            .await?;
        Ok((0..columns.len())
            .filter_map(|i| {
                let name = columns.get_string(i, "name")?;
                let type_ = columns.get_string(i, "type")?;
                Some(Column::new(name, self.to_generic_type(&type_, None, None)))
            })
            .collect())
    }

    async fn is_unload_supported(&self, _options: &crate::types::UnloadOptions) -> Result<bool> {
        // The S3 export bucket is not ported yet.
        Ok(false)
    }

    async fn stream(
        &self,
        sql: &str,
        params: &[Value],
        options: &StreamOptions,
    ) -> Result<StreamTableData> {
        let formatted = format_mysql(sql, params);
        let query_id = self.build_query_id(options.request_id.as_deref());
        const FORMAT: &str = "JSONCompactEachRowWithNamesAndTypes";

        let response = self
            .request(&query_id, Some(FORMAT))
            .body(formatted)
            .send()
            .await
            .map_err(|e| {
                DriverError::Query(format!("Stream query failed: {e}; query id: {query_id}"))
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(DriverError::Query(format!(
                "Stream query failed: {}; query id: {query_id}",
                body.trim()
            )));
        }

        // The first two lines are the column names and the column types.
        let mut lines = response
            .bytes_stream()
            .map(|chunk| chunk.map_err(|e| DriverError::Query(e.to_string())));

        let mut buffer = Vec::new();
        let mut pending: Vec<String> = Vec::new();
        while pending.len() < 2 {
            match lines.next().await {
                Some(Ok(chunk)) => {
                    buffer.extend_from_slice(&chunk);
                    drain_lines(&mut buffer, &mut pending);
                }
                Some(Err(e)) => return Err(e),
                None => break,
            }
        }
        if pending.len() < 2 {
            return Err(DriverError::Query(format!(
                "Stream query failed: the response is missing the names/types header; query id: {query_id}"
            )));
        }

        let names: Vec<String> = serde_json::from_str(&pending[0])?;
        let type_names: Vec<String> = serde_json::from_str(&pending[1])?;
        if names.len() != type_names.len() {
            return Err(DriverError::Query(format!(
                "Unexpected names and types length mismatch; names {} vs types {}",
                names.len(),
                type_names.len()
            )));
        }
        let columns: Vec<Column> = names
            .iter()
            .zip(type_names.iter())
            .map(|(name, type_)| Column::new(name.clone(), self.to_generic_type(type_, None, None)))
            .collect();
        let converters: Vec<Option<ColumnConverter>> =
            type_names.iter().map(|t| column_converter(t)).collect();

        let leftover: Vec<String> = pending.split_off(2);
        let rows = futures::stream::try_unfold(
            (
                lines,
                buffer,
                leftover.into_iter().collect::<Vec<_>>(),
                0usize,
            ),
            move |(mut lines, mut buffer, mut ready, mut index)| {
                let converters = converters.clone();
                async move {
                    loop {
                        if index < ready.len() {
                            let line = ready[index].clone();
                            index += 1;
                            let raw: Vec<Value> = serde_json::from_str(&line)?;
                            let row: Row = raw
                                .into_iter()
                                .enumerate()
                                .map(|(i, v)| match converters.get(i).copied().flatten() {
                                    Some(c) => transform::convert(c, &v),
                                    None => v,
                                })
                                .collect();
                            return Ok(Some((row, (lines, buffer, ready, index))));
                        }
                        ready.clear();
                        index = 0;
                        match lines.next().await {
                            Some(Ok(chunk)) => {
                                buffer.extend_from_slice(&chunk);
                                drain_lines(&mut buffer, &mut ready);
                            }
                            Some(Err(e)) => return Err(e),
                            None => {
                                // Flush a trailing line without a newline.
                                if !buffer.is_empty() {
                                    let line = String::from_utf8_lossy(&buffer).trim().to_string();
                                    buffer.clear();
                                    if !line.is_empty() {
                                        ready.push(line);
                                        continue;
                                    }
                                }
                                return Ok(None);
                            }
                        }
                    }
                }
            },
        )
        .boxed();

        Ok(StreamTableData { columns, rows })
    }

    async fn download_query_results(
        &self,
        sql: &str,
        params: &[Value],
        options: &DownloadQueryResultsOptions,
    ) -> Result<DownloadedData> {
        if options.stream_import {
            return Ok(DownloadedData::Stream(
                self.stream(sql, params, &options.stream).await?,
            ));
        }
        let (columns, rows) = self
            .query_response(
                sql,
                params,
                &QueryOptions {
                    request_id: options.stream.request_id.clone(),
                    ..Default::default()
                },
            )
            .await?;
        Ok(DownloadedData::Memory(QueryResult::new(columns, rows)))
    }
}

/// Moves every complete `\n`-terminated line out of `buffer` into `out`.
fn drain_lines(buffer: &mut Vec<u8>, out: &mut Vec<String>) {
    while let Some(pos) = buffer.iter().position(|b| *b == b'\n') {
        let line: Vec<u8> = buffer.drain(..=pos).collect();
        let line = String::from_utf8_lossy(&line).trim().to_string();
        if !line.is_empty() {
            out.push(line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn driver() -> ClickHouseDriver {
        let mut config = DriverConfig::default();
        config.data_source.host = Some("ch".into());
        config.data_source.database = Some("test".into());
        ClickHouseDriver::new(ClickHouseConfig::from_driver_config(config)).unwrap()
    }

    #[test]
    fn url_and_defaults() {
        let d = driver();
        assert_eq!(d.clickhouse_config().url, "http://ch:8123");
        assert_eq!(d.clickhouse_config().database, "test");
        assert_eq!(
            d.clickhouse_config().request_timeout,
            Duration::from_secs(600)
        );
        assert_eq!(d.clickhouse_config().max_pool_size, 10);
        assert!(d.read_only());
        assert_eq!(d.param(0), "?");
        assert_eq!(d.quote_identifier("a"), "\"a\"");
        assert!(d.capabilities().unload_without_temp_table);
        assert!(d.capabilities().incremental_schema_loading);

        // https when SSL is on, and the port default
        let mut config = DriverConfig::default();
        config.data_source.host = Some("ch".into());
        config.data_source.ssl = Some(crate::config::SslConfig::default());
        let cfg = ClickHouseConfig::from_driver_config(config);
        assert_eq!(cfg.url, "https://ch:8123");
        assert_eq!(cfg.database, "default");
    }

    #[test]
    fn config_from_url() {
        let cfg = ClickHouseConfig::from_url("http://u:p@host:9000/db").unwrap();
        assert_eq!(cfg.url, "http://host:9000");
        assert_eq!(cfg.username.as_deref(), Some("u"));
        assert_eq!(cfg.password.as_deref(), Some("p"));
        assert_eq!(cfg.database, "db");

        let cfg = ClickHouseConfig::from_url("https://host/").unwrap();
        assert_eq!(cfg.url, "https://host:8123");
        assert_eq!(cfg.database, "default");
        assert!(ClickHouseConfig::from_url("nonsense").is_err());
    }

    #[test]
    fn settings_depend_on_readonly_mode() {
        let mut cfg = ClickHouseConfig::from_driver_config(DriverConfig::default());
        let settings = cfg.clickhouse_settings();
        assert!(settings.contains(&("send_progress_in_http_headers", "0")));
        assert!(settings.contains(&("join_use_nulls", "1")));
        assert!(settings.contains(&("date_time_output_format", "simple")));

        cfg.read_only_mode = true;
        let settings = cfg.clickhouse_settings();
        assert_eq!(settings, vec![("send_progress_in_http_headers", "0")]);
    }

    #[test]
    fn read_only_resolution() {
        let base = || {
            let mut c = DriverConfig::default();
            c.data_source.host = Some("ch".into());
            ClickHouseConfig::from_driver_config(c)
        };
        // nothing specified → read only
        assert!(ClickHouseDriver::new(base()).unwrap().read_only());
        // explicitly writable
        let mut cfg = base();
        cfg.read_only = Some(false);
        assert!(!ClickHouseDriver::new(cfg).unwrap().read_only());
        // explicitly writable but the ClickHouse user is read-only
        let mut cfg = base();
        cfg.read_only = Some(false);
        cfg.read_only_mode = true;
        assert!(ClickHouseDriver::new(cfg).unwrap().read_only());
    }

    #[test]
    fn sql_contract() {
        let d = driver();
        let q = d.information_schema_query();
        assert!(q.contains("FROM system.columns"));
        assert!(q.contains("WHERE database = 'test'"));
        assert!(q.contains("SELECT name as column_name"));

        let q = d.get_tables_for_specific_schemas_query("?, ?");
        assert!(q.contains("FROM system.tables"));
        assert!(q.contains("WHERE database IN (?, ?)"));

        let q = d.get_columns_for_specific_tables_query("1 = 1");
        assert!(q.contains("database as \"schema_name\""));
        assert!(q.contains("FROM system.columns"));

        assert_eq!(d.column_name_for_schema_name(), "database");
        assert_eq!(d.column_name_for_table_name(), "table");
        assert_eq!(
            d.create_table_sql("t", &[Column::new("a", "int"), Column::new("b", "string")]),
            r#"CREATE TABLE t ("a" int, "b" string)"#
        );
    }

    #[tokio::test]
    async fn get_schemas_returns_the_configured_database() {
        let d = driver();
        assert_eq!(
            d.get_schemas().await.unwrap(),
            vec![SchemaName {
                schema_name: "test".into()
            }]
        );
    }

    #[test]
    fn response_is_normalised_per_column_type() {
        let d = driver();
        let response: JsonCompactResponse = serde_json::from_value(json!({
            "meta": [
                {"name": "n", "type": "Int64"},
                {"name": "s", "type": "String"},
                {"name": "d", "type": "Date"},
                {"name": "t", "type": "DateTime"},
                {"name": "a", "type": "Array(Int32)"},
            ],
            "data": [[1, "x", "2020-01-01", "2020-01-01 12:00:00", [1, 2]]],
            "rows": 1
        }))
        .unwrap();

        let (columns, rows) = d.normalise_response(response);
        assert_eq!(
            columns,
            vec![
                Column::new("n", "bigint"),
                Column::new("s", "text"),
                Column::new("d", "date"),
                Column::new("t", "timestamp"),
                Column::new("a", "int[]"),
            ]
        );
        assert_eq!(
            rows[0],
            vec![
                json!("1"),
                json!("x"),
                json!("2020-01-01T00:00:00.000"),
                json!("2020-01-01T12:00:00.000"),
                json!([1, 2]),
            ]
        );
    }

    #[test]
    fn query_ids_carry_the_request_prefix() {
        let d = driver();
        let id = d.build_query_id(Some("abc-span-1"));
        assert!(id.starts_with("abc-"), "{id}");
        let id = d.build_query_id(None);
        assert_eq!(id.len(), 36, "{id}");
        assert_ne!(d.build_query_id(None), d.build_query_id(None));
        assert_eq!(extract_request_uuid("abc-span-1-2"), "abc");
        assert_eq!(extract_request_uuid("abc"), "abc");
    }

    #[test]
    fn line_draining() {
        let mut buffer = b"[1]\n[2]\npart".to_vec();
        let mut out = Vec::new();
        drain_lines(&mut buffer, &mut out);
        assert_eq!(out, vec!["[1]".to_string(), "[2]".to_string()]);
        assert_eq!(buffer, b"part");
    }
}
