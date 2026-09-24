//! Cube Store driver: port of `@cubejs-backend/cubestore-driver`
//! (`CubeStoreDriver.ts` + `WebSocketConnection.ts`) on top of the
//! [`cubestore_ws_transport`] WebSocket client and the FlatBuffers codec in
//! `cubeshared`.
//!
//! Cube Store is both the external (pre-aggregation) store and the backing
//! store of the orchestrator's queue and cache, so this driver is the one the
//! rest of the Rust backend leans on hardest.

pub mod convert;
mod semver;

use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use cubestore_ws_transport::{
    Client, ClientConfig, InlineTable as WsInlineTable, QueryOptions as WsQueryOptions,
    QueryParameter, ResponseFormat, TransportError,
};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::config::{DriverConfig, EnvSource, ProcessEnv};
use crate::driver::Driver;
use crate::error::{DriverError, Result};
use crate::escape::{escape_mysql_string, format_mysql};
use crate::types::{
    Column, CreateTableIndex, DownloadedData, DriverCapabilities, ExternalCreateTableOptions,
    GenericType, IndexSql, QueryOptions, QueryResult, TableMemoryData, TableName, TableStructure,
};

pub use convert::to_query_result;
pub use semver::is_version_gte;

/// Default host (`127.0.0.1`, not `localhost`: Node 18 resolves `localhost` to
/// IPv6 first, and the Node driver pins the IPv4 address for the same reason).
pub const DEFAULT_HOST: &str = "127.0.0.1";
/// Default port (`CUBEJS_CUBESTORE_PORT`).
pub const DEFAULT_PORT: u16 = 3030;
/// Rows inserted per `INSERT` statement by `importRows`.
pub const INSERT_BATCH_SIZE: usize = 2000;
/// Key of the per-query `sendParameters` switch in [`QueryOptions::extra`].
pub const SEND_PARAMETERS_OPTION: &str = "sendParameters";
/// Key of the per-query `responseFormat` override in [`QueryOptions::extra`].
pub const RESPONSE_FORMAT_OPTION: &str = "responseFormat";

/// `CubeStoreCapabilityMinVersion`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CubeStoreCapability {
    QueueExclusive,
    QueueExternalId,
    QueueAddAndRetrieve,
    SendableParameters,
    ArrowFormat,
}

impl CubeStoreCapability {
    /// Minimum Cube Store version providing this capability.
    pub fn min_version(self) -> &'static str {
        match self {
            CubeStoreCapability::QueueExclusive => "1.6.22",
            CubeStoreCapability::QueueExternalId => "1.6.26",
            CubeStoreCapability::QueueAddAndRetrieve => "1.7.25",
            CubeStoreCapability::SendableParameters => "1.6.38",
            CubeStoreCapability::ArrowFormat => "1.6.66",
        }
    }
}

/// `GenericTypeToCubeStore`.
fn generic_to_cubestore(generic: &GenericType) -> String {
    match generic {
        GenericType::String => "varchar(255)".to_string(),
        GenericType::Text => "varchar(255)".to_string(),
        GenericType::Other(name) => match name.as_str() {
            "uuid" => "varchar(64)".to_string(),
            // Cube Store uses an old SQL parser that does not support a custom
            // timestamp precision, which the (old) Athena driver emitted.
            "timestamp(3)" => "timestamp".to_string(),
            // Comes from JDBC. We might consider decimal96 here.
            "bigdecimal" => "decimal".to_string(),
            _ => name.clone(),
        },
        other => other.to_string(),
    }
}

/// `CreateTableOptions` of `CubeStoreDriver`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreateTableOptions {
    pub stream_offset: Option<String>,
    pub input_format: Option<String>,
    pub build_range_end: Option<String>,
    pub unique_key: Option<String>,
    /// Pre-rendered `INDEX ...` clauses.
    pub indexes: Option<String>,
    /// `LOCATION` entries; one `?` placeholder is emitted per file.
    pub files: Vec<String>,
    /// Pre-rendered ` AGGREGATIONS (...)` clause.
    pub aggregations: Option<String>,
    pub select_statement: Option<String>,
    /// `(table_name, columns)` of the source table for streaming imports.
    pub source_table: Option<SourceTable>,
    pub seal_at: Option<String>,
    pub delimiter: Option<String>,
    pub disable_quoting: bool,
}

/// `options.sourceTable` of `createTableSqlWithOptions`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceTable {
    pub table_name: String,
    pub types: Vec<Column>,
}

/// Configuration of [`CubeStoreDriver`] (`ConnectionConfig` + the
/// `CUBEJS_CUBESTORE_*` environment).
#[derive(Debug, Clone)]
pub struct CubeStoreConfig {
    /// Generic driver configuration (used by the `BaseDriver` defaults).
    pub driver: DriverConfig,
    /// Base URL without a trailing slash and without the `/ws` suffix
    /// (`ws://127.0.0.1:3030`).
    pub base_url: String,
    /// `CUBEJS_CUBESTORE_USER`.
    pub user: Option<String>,
    /// `CUBEJS_CUBESTORE_PASS`.
    pub password: Option<String>,
    /// `CUBEJS_CUBESTORE_SENDABLE_PARAMETERS` (default `true`). Parameters are
    /// only sent when the server is new enough as well.
    pub sendable_parameters: bool,
    /// `CUBEJS_CUBESTORE_MAX_CONNECT_RETRIES` (default 20).
    pub max_connect_retries: u32,
    /// `CUBEJS_CUBESTORE_NO_HEART_BEAT_TIMEOUT` (default 30 s).
    pub no_heart_beat_timeout: Duration,
    /// Connection timeout of a single attempt.
    pub connect_timeout: Duration,
    /// `CUBEJS_INSTANCE_ID`, attached to every tracing object.
    pub instance_id: Option<String>,
}

impl Default for CubeStoreConfig {
    fn default() -> Self {
        Self {
            driver: DriverConfig::default(),
            base_url: format!("ws://{DEFAULT_HOST}:{DEFAULT_PORT}"),
            user: None,
            password: None,
            sendable_parameters: true,
            max_connect_retries: 20,
            no_heart_beat_timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(10),
            instance_id: None,
        }
    }
}

/// Port of the `baseUrl` expression of `CubeStoreDriver`:
/// `(url ?? ws://host:port/).replace(/\/ws$/, '/').replace(/\/$/, '')`.
pub fn base_url(url: Option<&str>, host: &str, port: u16) -> String {
    let raw = match url {
        Some(u) if !u.is_empty() => u.to_string(),
        _ => format!("ws://{host}:{port}/"),
    };
    let stripped = match raw.strip_suffix("/ws") {
        Some(head) => format!("{head}/"),
        None => raw,
    };
    stripped
        .strip_suffix('/')
        .map(|s| s.to_string())
        .unwrap_or(stripped)
}

impl CubeStoreConfig {
    /// Reads the configuration from the process environment.
    pub fn from_env() -> Result<Self> {
        Self::from_env_source(&ProcessEnv)
    }

    /// Reads the configuration from an arbitrary [`EnvSource`].
    pub fn from_env_source(env: &dyn EnvSource) -> Result<Self> {
        let get = |key: &str| env.get(key).filter(|v| !v.is_empty());

        let host = get("CUBEJS_CUBESTORE_HOST").unwrap_or_else(|| DEFAULT_HOST.to_string());
        let port = match get("CUBEJS_CUBESTORE_PORT") {
            Some(v) => v.trim().parse::<u16>().map_err(|_| {
                DriverError::Config(
                    "env-var: \"CUBEJS_CUBESTORE_PORT\" should be a valid port number (0-65535)"
                        .to_string(),
                )
            })?,
            None => DEFAULT_PORT,
        };
        let sendable_parameters = match get("CUBEJS_CUBESTORE_SENDABLE_PARAMETERS") {
            Some(v) => parse_bool_strict(&v, "CUBEJS_CUBESTORE_SENDABLE_PARAMETERS")?,
            None => true,
        };
        let max_connect_retries = match get("CUBEJS_CUBESTORE_MAX_CONNECT_RETRIES") {
            Some(v) => v.trim().parse::<u32>().map_err(|_| {
                DriverError::Config(
                    "env-var: \"CUBEJS_CUBESTORE_MAX_CONNECT_RETRIES\" should be a valid integer"
                        .to_string(),
                )
            })?,
            None => 20,
        };
        let no_heart_beat_timeout = match get("CUBEJS_CUBESTORE_NO_HEART_BEAT_TIMEOUT") {
            Some(v) => Duration::from_secs(v.trim().parse::<u64>().map_err(|_| {
                DriverError::Config(
                    "env-var: \"CUBEJS_CUBESTORE_NO_HEART_BEAT_TIMEOUT\" should be a valid integer"
                        .to_string(),
                )
            })?),
            None => Duration::from_secs(30),
        };

        Ok(Self {
            driver: DriverConfig::default(),
            base_url: base_url(get("CUBEJS_CUBESTORE_URL").as_deref(), &host, port),
            user: get("CUBEJS_CUBESTORE_USER"),
            password: get("CUBEJS_CUBESTORE_PASS"),
            sendable_parameters,
            max_connect_retries,
            no_heart_beat_timeout,
            connect_timeout: Duration::from_secs(10),
            instance_id: get("CUBEJS_INSTANCE_ID"),
        })
    }

    /// Builds a configuration from a base URL (`ws://host:port`, with or
    /// without the `/ws` suffix), for tests and tooling.
    pub fn from_url(url: &str) -> Self {
        Self {
            base_url: base_url(Some(url), DEFAULT_HOST, DEFAULT_PORT),
            ..Default::default()
        }
    }

    /// The WebSocket endpoint (`<base_url>/ws`).
    pub fn ws_url(&self) -> String {
        format!("{}/ws", self.base_url)
    }

    /// The HTTP endpoint used by the temp-file upload API
    /// (`baseUrl.replace(/^ws/, 'http')`).
    pub fn http_url(&self) -> String {
        match self.base_url.strip_prefix("ws") {
            Some(rest) => format!("http{rest}"),
            None => self.base_url.clone(),
        }
    }

    fn client_config(&self) -> Result<ClientConfig> {
        let url = url::Url::parse(&self.ws_url())
            .map_err(|e| DriverError::Config(format!("Invalid Cube Store URL: {e}")))?;
        let mut cfg = ClientConfig::new(url);
        if self.user.is_some() || self.password.is_some() {
            cfg = cfg.with_credentials(
                self.user.clone().unwrap_or_default(),
                self.password.clone().unwrap_or_default(),
            );
        }
        cfg.max_connect_retries = self.max_connect_retries;
        cfg.no_heartbeat_timeout = self.no_heart_beat_timeout;
        cfg.connect_timeout = self.connect_timeout;
        Ok(cfg)
    }
}

fn parse_bool_strict(value: &str, key: &str) -> Result<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(DriverError::Config(format!(
            "The {key} must be either 'true' or 'false'."
        ))),
    }
}

/// Cube Store driver.
pub struct CubeStoreDriver {
    config: CubeStoreConfig,
    /// Lazily established connection; the transport reconnects on its own, so
    /// one client is kept for the lifetime of the driver.
    client: Mutex<Option<Client>>,
    closed: AtomicBool,
}

impl std::fmt::Debug for CubeStoreDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CubeStoreDriver")
            .field("base_url", &self.config.base_url)
            .finish()
    }
}

impl CubeStoreDriver {
    /// Creates the driver. No connection is opened until the first query.
    pub fn new(config: CubeStoreConfig) -> Result<Self> {
        // Fail fast on a malformed URL instead of at the first query.
        config.client_config()?;
        Ok(Self {
            config,
            client: Mutex::new(None),
            closed: AtomicBool::new(false),
        })
    }

    /// Creates the driver from `CUBEJS_CUBESTORE_*`.
    pub fn from_env() -> Result<Self> {
        Self::new(CubeStoreConfig::from_env()?)
    }

    /// The driver configuration.
    pub fn cubestore_config(&self) -> &CubeStoreConfig {
        &self.config
    }

    /// Returns the (connected) transport client, connecting on first use.
    async fn client(&self) -> Result<Client> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(DriverError::Connection {
                pool_name: self.config.base_url.clone(),
                message: "Cube Store connection is closed".to_string(),
            });
        }
        let mut guard = self.client.lock().await;
        if let Some(client) = guard.as_ref() {
            return Ok(client.clone());
        }
        let client = Client::connect(self.config.client_config()?)
            .await
            .map_err(|e| self.transport_error(e))?;
        *guard = Some(client.clone());
        Ok(client)
    }

    fn transport_error(&self, e: TransportError) -> DriverError {
        match e {
            // A query error carries the message Cube Store produced.
            TransportError::Query(message) => DriverError::Database {
                message,
                code: None,
            },
            TransportError::Connect(_)
            | TransportError::Disconnected
            | TransportError::Closed
            | TransportError::Auth(_) => DriverError::Connection {
                pool_name: self.config.base_url.clone(),
                message: e.to_string(),
            },
            TransportError::InvalidUrl(m) => DriverError::Config(m),
            other => DriverError::Query(other.to_string()),
        }
    }

    /// The version reported by Cube Store in the `X-CubeStore-Version` upgrade
    /// header, `"0.0.0"` when it did not send one (as in the Node driver).
    pub async fn cube_store_version(&self) -> Result<String> {
        let client = self.client().await?;
        Ok(client.server_version().unwrap_or("0.0.0").to_string())
    }

    /// `hasCapability`.
    pub async fn has_capability(&self, capability: CubeStoreCapability) -> Result<bool> {
        let version = self.cube_store_version().await?;
        Ok(is_version_gte(Some(&version), capability.min_version()))
    }

    /// Whether this query should send bound parameters rather than inlining
    /// them client-side.
    ///
    /// Opt-in per call, exactly like the `sendParameters` option of the Node
    /// driver: only Cube Store's own commands (`CACHE GET ?`, `CACHE SET TTL ?
    /// ? ?`, the `QUEUE` statements) take bound parameters, so the cache and
    /// queue drivers set it while every other caller — pre-aggregation loads,
    /// `information_schema` queries, inserts — gets the client-side
    /// interpolation that Cube Store's SQL planner needs. Use
    /// [`QueryOptions::with_send_parameters`] to set it.
    ///
    /// The flag is only honoured when `CUBEJS_CUBESTORE_SENDABLE_PARAMETERS`
    /// is on *and* the server is new enough.
    pub async fn send_parameters(&self, options: &QueryOptions) -> Result<bool> {
        let requested = options
            .extra
            .get(SEND_PARAMETERS_OPTION)
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !requested || !self.config.sendable_parameters {
            return Ok(false);
        }
        self.has_capability(CubeStoreCapability::SendableParameters)
            .await
    }

    /// `{...queryTracingObj, instance: getEnv('instanceId')}`, serialised.
    fn trace_obj(&self, options: &QueryOptions) -> Option<String> {
        let mut map = serde_json::Map::new();
        if let Some(request_id) = &options.request_id {
            map.insert("requestId".to_string(), Value::from(request_id.clone()));
        }
        for (k, v) in &options.extra {
            // Driver-level switches are not part of the tracing object.
            if k == SEND_PARAMETERS_OPTION || k == RESPONSE_FORMAT_OPTION {
                continue;
            }
            map.insert(k.clone(), v.clone());
        }
        if let Some(instance) = &self.config.instance_id {
            map.insert("instance".to_string(), Value::from(instance.clone()));
        }
        if map.is_empty() {
            return None;
        }
        serde_json::to_string(&Value::Object(map)).ok()
    }

    /// `serializeParameter`: a JSON value → an `HttpParameterValue`.
    fn to_parameter(value: &Value) -> Result<QueryParameter> {
        Ok(match value {
            Value::Null => QueryParameter::Null,
            Value::Bool(b) => QueryParameter::Bool(*b),
            Value::Number(n) => match n.as_i64() {
                Some(i) => QueryParameter::Int64(i),
                None => QueryParameter::Float64(n.as_f64().ok_or_else(|| {
                    DriverError::Query(format!("Unsupported numeric parameter: {n}"))
                })?),
            },
            Value::String(s) => QueryParameter::String(s.clone()),
            Value::Array(_) | Value::Object(_) => {
                return Err(DriverError::Query(
                    "Parameter with type: object is not supported".to_string(),
                ))
            }
        })
    }

    /// `createIndexString`.
    fn create_index_string(index: &CreateTableIndex) -> String {
        let prefix = match index.type_.as_str() {
            "aggregate" => "AGGREGATE ",
            _ => "",
        };
        format!(
            "{prefix}INDEX {} ({})",
            index.index_name,
            index.columns.join(",")
        )
    }

    /// `createTableSqlWithOptions`.
    pub fn create_table_sql_with_options(
        &self,
        table_name: &str,
        columns: &[Column],
        options: &CreateTableOptions,
    ) -> String {
        let mut sql = self.create_table_sql(table_name, columns);
        let mut with_entries: Vec<String> = Vec::new();

        if let Some(input_format) = &options.input_format {
            with_entries.push(format!("input_format = '{input_format}'"));
        }
        if let Some(delimiter) = &options.delimiter {
            with_entries.push(format!("delimiter = '{delimiter}'"));
        }
        if options.disable_quoting {
            with_entries.push("disable_quoting = true".to_string());
        }
        if let Some(build_range_end) = &options.build_range_end {
            with_entries.push(format!("build_range_end = '{build_range_end}'"));
        }
        if let Some(seal_at) = &options.seal_at {
            with_entries.push(format!("seal_at = '{seal_at}'"));
        }
        if let Some(select_statement) = &options.select_statement {
            with_entries.push(format!(
                "select_statement = {}",
                escape_mysql_string(select_statement)
            ));
        }
        if let Some(source_table) = &options.source_table {
            let types = source_table
                .types
                .iter()
                .map(|t| format!("{} {}", t.name, self.from_generic_type(&t.type_)))
                .collect::<Vec<_>>()
                .join(", ");
            with_entries.push(format!(
                "source_table = {}",
                escape_mysql_string(&format!(
                    "CREATE TABLE {} ({types})",
                    source_table.table_name
                ))
            ));
        }
        if let Some(stream_offset) = &options.stream_offset {
            with_entries.push(format!("stream_offset = '{stream_offset}'"));
        }
        if !with_entries.is_empty() {
            sql = format!("{sql} WITH ({})", with_entries.join(", "));
        }
        if let Some(unique_key) = &options.unique_key {
            sql = format!("{sql} UNIQUE KEY ({unique_key})");
        }
        if let Some(aggregations) = &options.aggregations {
            sql = format!("{sql} {aggregations}");
        }
        if let Some(indexes) = &options.indexes {
            sql = format!("{sql} {indexes}");
        }
        if !options.files.is_empty() {
            sql = format!(
                "{sql} LOCATION {}",
                options
                    .files
                    .iter()
                    .map(|_| "?")
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        sql
    }

    /// `createTableWithOptions`.
    pub async fn create_table_with_options(
        &self,
        table_name: &str,
        columns: &[Column],
        options: &CreateTableOptions,
        query_options: &QueryOptions,
    ) -> Result<()> {
        let sql = self.create_table_sql_with_options(table_name, columns, options);
        let params: Vec<Value> = options
            .files
            .iter()
            .map(|f| Value::from(f.clone()))
            .collect();
        self.query(&sql, &params, query_options)
            .await
            .map_err(|e| DriverError::Query(format!("Error during create table: {sql}: {e}")))?;
        Ok(())
    }

    /// `getTablesQuery`, keeping the `build_range_end` column the orchestrator
    /// needs (the [`Driver::get_tables_query`] override returns names only).
    pub async fn get_tables_with_build_range(&self, schema_name: &str) -> Result<QueryResult> {
        self.query(
            &format!(
                "SELECT table_name, build_range_end FROM information_schema.tables WHERE table_schema = {}",
                self.param(0)
            ),
            &[Value::from(schema_name)],
            &QueryOptions::default(),
        )
        .await
    }

    /// `getPrefixTablesQuery`.
    pub async fn get_prefix_tables_query(
        &self,
        schema_name: &str,
        table_prefixes: &[String],
    ) -> Result<QueryResult> {
        let prefix_where = table_prefixes
            .iter()
            .map(|_| "table_name LIKE CONCAT(?, '%')")
            .collect::<Vec<_>>()
            .join(" OR ");
        let mut params: Vec<Value> = vec![Value::from(schema_name)];
        params.extend(table_prefixes.iter().map(|p| Value::from(p.clone())));
        self.query(
            &format!(
                "SELECT table_name, build_range_end FROM information_schema.tables WHERE table_schema = {} AND ({prefix_where})",
                self.param(0)
            ),
            &params,
            &QueryOptions::default(),
        )
        .await
    }

    /// Renders the ` AGGREGATIONS (...)` and `INDEX ...` clauses of
    /// `uploadTableWithIndexes`.
    fn external_clauses(external_options: &ExternalCreateTableOptions) -> (String, String) {
        let indexes = external_options
            .create_table_indexes
            .iter()
            .map(Self::create_index_string)
            .collect::<Vec<_>>()
            .join(" ");

        let has_aggregating_indexes = external_options
            .create_table_indexes
            .iter()
            .any(|i| i.type_ == "aggregate");

        let aggregations =
            if has_aggregating_indexes && !external_options.aggregations_columns.is_empty() {
                format!(
                    " AGGREGATIONS ({})",
                    external_options.aggregations_columns.join(", ")
                )
            } else {
                String::new()
            };

        (indexes, aggregations)
    }

    /// `importRows`: `CREATE TABLE` followed by batched `INSERT`s.
    async fn import_rows(
        &self,
        table: &str,
        columns: &[Column],
        indexes: &str,
        aggregations: &str,
        table_data: &TableMemoryData,
        query_options: &QueryOptions,
    ) -> Result<()> {
        if columns.is_empty() {
            return Err(DriverError::Query(
                "Unable to import (as rows) in Cube Store: empty columns. Most probably, introspection has failed."
                    .to_string(),
            ));
        }

        self.create_table_with_options(
            table,
            columns,
            &CreateTableOptions {
                indexes: non_empty(indexes),
                aggregations: non_empty(aggregations),
                build_range_end: build_range_end(query_options),
                ..Default::default()
            },
            query_options,
        )
        .await?;

        let quoted_columns = columns
            .iter()
            .map(|c| self.quote_identifier(&c.name))
            .collect::<Vec<_>>()
            .join(", ");

        let upload = async {
            for chunk in table_data.rows.chunks(INSERT_BATCH_SIZE) {
                let mut placeholders = String::new();
                let mut params: Vec<Value> = Vec::with_capacity(chunk.len() * columns.len());
                for (i, row) in chunk.iter().enumerate() {
                    if i > 0 {
                        placeholders.push_str(", ");
                    }
                    let _ = write!(
                        placeholders,
                        "({})",
                        columns
                            .iter()
                            .enumerate()
                            .map(|(p, _)| self.param(p + i * columns.len()))
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    for c in columns {
                        let value = table_data
                            .column_index(&c.name)
                            .and_then(|idx| row.get(idx).cloned())
                            .unwrap_or(Value::Null);
                        params.push(self.to_column_value(&value, &c.type_));
                    }
                }

                self.query(
                    &format!(
                        "INSERT INTO {table}
        ({quoted_columns})
        VALUES {placeholders}"
                    ),
                    &params,
                    query_options,
                )
                .await?;
            }
            Ok::<(), DriverError>(())
        };

        if let Err(e) = upload.await {
            if let Err(drop_err) = self.drop_table(table, &QueryOptions::default()).await {
                log::warn!("Unable to drop table {table} after failed upload: {drop_err}");
            }
            return Err(e);
        }
        Ok(())
    }

    /// `importCsvFile`: a `CREATE TABLE ... LOCATION` over the unloaded files.
    async fn import_csv_file(
        &self,
        table: &str,
        columns: &[Column],
        table_data: &crate::types::TableCsvData,
        indexes: &str,
        aggregations: &str,
        query_options: &QueryOptions,
    ) -> Result<()> {
        if columns.is_empty() {
            return Err(DriverError::Query(
                "Unable to import (as csv) in Cube Store: empty columns. Most probably, introspection has failed."
                    .to_string(),
            ));
        }

        let options = Self::csv_import_options(table_data, indexes, aggregations, query_options);
        self.create_table_with_options(table, columns, &options, query_options)
            .await
    }

    /// The `CREATE TABLE … WITH (…) LOCATION` options of an unloaded CSV
    /// (the pure part of [`CubeStoreDriver::import_csv_file`]).
    pub fn csv_import_options(
        table_data: &crate::types::TableCsvData,
        indexes: &str,
        aggregations: &str,
        query_options: &QueryOptions,
    ) -> CreateTableOptions {
        let mut options = CreateTableOptions {
            build_range_end: build_range_end(query_options),
            indexes: non_empty(indexes),
            aggregations: non_empty(aggregations),
            ..Default::default()
        };
        if !table_data.csv_file.is_empty() {
            options.input_format = Some(
                if table_data.csv_no_header {
                    "csv_no_header"
                } else {
                    "csv"
                }
                .to_string(),
            );
            options.delimiter = table_data.csv_delimiter.clone();
            options.disable_quoting = table_data.csv_disable_quoting;
            options.files = table_data.csv_file.clone();
        }
        options
    }

    /// Uploads whatever `download_query_results` / `download_table` produced.
    ///
    /// Covers the in-memory (`rows`) and CSV (`csvFile`) branches of
    /// `uploadTableWithIndexes`; the streaming branches are not ported yet.
    pub async fn import_downloaded_table(
        &self,
        table: &str,
        columns: &[Column],
        table_data: DownloadedData,
        external_options: &ExternalCreateTableOptions,
        query_options: &QueryOptions,
    ) -> Result<()> {
        let (indexes, aggregations) = Self::external_clauses(external_options);
        match &table_data {
            DownloadedData::Memory(rows) => {
                self.import_rows(table, columns, &indexes, &aggregations, rows, query_options)
                    .await
            }
            DownloadedData::Csv(csv) => {
                self.import_csv_file(table, columns, csv, &indexes, &aggregations, query_options)
                    .await
            }
            DownloadedData::Stream(_) => Err(DriverError::NotImplemented(
                "Streaming upload to Cube Store (temp-file import) is not implemented yet."
                    .to_string(),
            )),
        }
    }
}

/// `queryTracingObj?.buildRangeEnd`.
fn build_range_end(options: &QueryOptions) -> Option<String> {
    options
        .extra
        .get("buildRangeEnd")
        .and_then(Value::as_str)
        .map(|s| s.to_string())
}

fn non_empty(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

#[async_trait]
impl Driver for CubeStoreDriver {
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
        let client = self.client().await?;

        let send_parameters = self.send_parameters(options).await?;
        // When the server cannot take bound parameters the driver inlines them
        // client-side, exactly like `formatSql(query, values)` does.
        let (sql, parameters) = if send_parameters {
            (
                sql.to_string(),
                params
                    .iter()
                    .map(Self::to_parameter)
                    .collect::<Result<Vec<_>>>()?,
            )
        } else {
            (format_mysql(sql, params), Vec::new())
        };

        let response_format = match options
            .extra
            .get(RESPONSE_FORMAT_OPTION)
            .and_then(Value::as_str)
        {
            Some("legacy") => ResponseFormat::Legacy,
            Some("arrow") => ResponseFormat::Arrow,
            _ => {
                if self
                    .has_capability(CubeStoreCapability::ArrowFormat)
                    .await?
                {
                    ResponseFormat::Arrow
                } else {
                    ResponseFormat::Legacy
                }
            }
        };

        let inline_tables: Vec<WsInlineTable> = options
            .inline_tables
            .iter()
            .map(|t| WsInlineTable {
                name: t.name.clone(),
                columns: t.columns.iter().map(|c| c.name.clone()).collect(),
                types: t.columns.iter().map(|c| c.type_.to_string()).collect(),
                csv_rows: t.csv_rows.clone(),
            })
            .collect();

        let result = client
            .query_with_options(
                sql,
                WsQueryOptions {
                    parameters,
                    inline_tables,
                    trace_obj: self.trace_obj(options),
                    response_format,
                },
            )
            .await
            .map_err(|e| self.transport_error(e))?;

        to_query_result(&result)
    }

    fn quote_identifier(&self, identifier: &str) -> String {
        format!("`{identifier}`")
    }

    #[allow(clippy::wrong_self_convention)]
    fn from_generic_type(&self, generic: &GenericType) -> String {
        generic_to_cubestore(generic)
    }

    fn to_column_value(&self, value: &Value, generic_type: &GenericType) -> Value {
        match (generic_type, value) {
            (GenericType::Timestamp, Value::String(s)) => Value::String(s.replacen('Z', "", 1)),
            (GenericType::Boolean, Value::String(s)) => match s.to_lowercase().as_str() {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                _ => value.clone(),
            },
            _ => value.clone(),
        }
    }

    fn capabilities(&self) -> DriverCapabilities {
        DriverCapabilities {
            csv_import: true,
            stream_import: true,
            ..Default::default()
        }
    }

    fn information_schema_query(&self) -> String {
        format!(
            "
      SELECT columns.column_name as {},
             columns.table_name as {},
             columns.table_schema as {},
             columns.data_type as {}
      FROM information_schema.columns as columns
      WHERE columns.table_schema NOT IN ('information_schema', 'system')",
            self.quote_identifier("column_name"),
            self.quote_identifier("table_name"),
            self.quote_identifier("table_schema"),
            self.quote_identifier("data_type"),
        )
    }

    async fn release(&self) -> Result<()> {
        self.closed.store(true, Ordering::SeqCst);
        if let Some(client) = self.client.lock().await.take() {
            client.close();
        }
        Ok(())
    }

    async fn get_tables_query(&self, schema_name: &str) -> Result<Vec<String>> {
        let result = self.get_tables_with_build_range(schema_name).await?;
        Ok((0..result.len())
            .filter_map(|i| result.get_string(i, "table_name"))
            .collect())
    }

    async fn table_column_types(&self, table: &str) -> Result<TableStructure> {
        let TableName { schema, name } = TableName::split(table);
        // JS: `const [schema, name] = table.split('.')` keeps only the 2nd part.
        let name = name.split('.').next().unwrap_or("").to_string();

        let q = |i: &str| self.quote_identifier(i);
        let result = self
            .query(
                &format!(
                    "SELECT column_name as {},
             table_name as {},
             table_schema as {},
             data_type as {}
      FROM information_schema.columns
      WHERE table_name = {} AND table_schema = {}",
                    q("column_name"),
                    q("table_name"),
                    q("table_schema"),
                    q("data_type"),
                    self.param(0),
                    self.param(1),
                ),
                &[Value::from(name), Value::from(schema)],
                &QueryOptions::default(),
            )
            .await?;

        Ok((0..result.len())
            .filter_map(|i| {
                let name = result.get_string(i, "column_name")?;
                let data_type = result.get_string(i, "data_type")?;
                Some(Column::new(
                    name,
                    self.to_generic_type(&data_type, None, None),
                ))
            })
            .collect())
    }

    async fn upload_table_with_indexes(
        &self,
        table: &str,
        columns: &[Column],
        table_data: &TableMemoryData,
        _indexes_sql: &[IndexSql],
        _unique_key_columns: &[String],
        external_options: &ExternalCreateTableOptions,
    ) -> Result<()> {
        let (indexes, aggregations) = Self::external_clauses(external_options);
        self.import_rows(
            table,
            columns,
            &indexes,
            &aggregations,
            table_data,
            &QueryOptions::default(),
        )
        .await
    }

    /// Cube Store imports the unloaded files itself (`CREATE TABLE … LOCATION`),
    /// so a CSV download is never materialised in memory.
    async fn upload_downloaded_table_with_indexes(
        &self,
        table: &str,
        columns: &[Column],
        table_data: DownloadedData,
        _indexes_sql: &[IndexSql],
        _unique_key_columns: &[String],
        external_options: &ExternalCreateTableOptions,
    ) -> Result<()> {
        self.import_downloaded_table(
            table,
            columns,
            table_data,
            external_options,
            &QueryOptions::default(),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CreateTableIndex;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn driver() -> CubeStoreDriver {
        CubeStoreDriver::new(CubeStoreConfig::default()).unwrap()
    }

    #[test]
    fn base_url_normalisation() {
        assert_eq!(base_url(None, "127.0.0.1", 3030), "ws://127.0.0.1:3030");
        assert_eq!(base_url(None, "cubestore", 9999), "ws://cubestore:9999");
        assert_eq!(base_url(Some("ws://h:3030/ws"), "x", 1), "ws://h:3030");
        assert_eq!(base_url(Some("ws://h:3030/"), "x", 1), "ws://h:3030");
        assert_eq!(base_url(Some("ws://h:3030"), "x", 1), "ws://h:3030");
        assert_eq!(
            base_url(Some("wss://cloud.example/staging/3/ws"), "x", 1),
            "wss://cloud.example/staging/3"
        );
    }

    #[test]
    fn config_from_env() {
        let cfg = CubeStoreConfig::from_env_source(&env(&[])).unwrap();
        assert_eq!(cfg.base_url, "ws://127.0.0.1:3030");
        assert_eq!(cfg.ws_url(), "ws://127.0.0.1:3030/ws");
        assert_eq!(cfg.http_url(), "http://127.0.0.1:3030");
        assert!(cfg.sendable_parameters);
        assert_eq!(cfg.max_connect_retries, 20);
        assert_eq!(cfg.no_heart_beat_timeout, Duration::from_secs(30));
        assert_eq!(cfg.user, None);

        let cfg = CubeStoreConfig::from_env_source(&env(&[
            ("CUBEJS_CUBESTORE_HOST", "cubestore"),
            ("CUBEJS_CUBESTORE_PORT", "9999"),
            ("CUBEJS_CUBESTORE_USER", "u"),
            ("CUBEJS_CUBESTORE_PASS", "p"),
            ("CUBEJS_CUBESTORE_SENDABLE_PARAMETERS", "false"),
            ("CUBEJS_CUBESTORE_MAX_CONNECT_RETRIES", "3"),
            ("CUBEJS_CUBESTORE_NO_HEART_BEAT_TIMEOUT", "7"),
        ]))
        .unwrap();
        assert_eq!(cfg.base_url, "ws://cubestore:9999");
        assert_eq!(cfg.user.as_deref(), Some("u"));
        assert_eq!(cfg.password.as_deref(), Some("p"));
        assert!(!cfg.sendable_parameters);
        assert_eq!(cfg.max_connect_retries, 3);
        assert_eq!(cfg.no_heart_beat_timeout, Duration::from_secs(7));

        let err = CubeStoreConfig::from_env_source(&env(&[(
            "CUBEJS_CUBESTORE_SENDABLE_PARAMETERS",
            "yes",
        )]))
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "The CUBEJS_CUBESTORE_SENDABLE_PARAMETERS must be either 'true' or 'false'."
        );
        assert!(
            CubeStoreConfig::from_env_source(&env(&[("CUBEJS_CUBESTORE_PORT", "abc")])).is_err()
        );
    }

    #[test]
    fn capability_versions() {
        assert!(is_version_gte(
            Some("1.6.38"),
            CubeStoreCapability::SendableParameters.min_version()
        ));
        assert!(!is_version_gte(
            Some("1.6.37"),
            CubeStoreCapability::SendableParameters.min_version()
        ));
        assert!(is_version_gte(
            Some("1.7.0"),
            CubeStoreCapability::ArrowFormat.min_version()
        ));
        assert!(!is_version_gte(
            Some("0.0.0"),
            CubeStoreCapability::ArrowFormat.min_version()
        ));
    }

    #[test]
    fn type_mapping() {
        let d = driver();
        assert_eq!(d.from_generic_type(&GenericType::String), "varchar(255)");
        assert_eq!(d.from_generic_type(&GenericType::Text), "varchar(255)");
        assert_eq!(
            d.from_generic_type(&GenericType::parse("uuid")),
            "varchar(64)"
        );
        assert_eq!(
            d.from_generic_type(&GenericType::parse("timestamp(3)")),
            "timestamp"
        );
        assert_eq!(
            d.from_generic_type(&GenericType::parse("bigdecimal")),
            "decimal"
        );
        assert_eq!(d.from_generic_type(&GenericType::Int), "int");
        assert_eq!(d.from_generic_type(&GenericType::Bigint), "bigint");
        assert_eq!(
            d.from_generic_type(&GenericType::Decimal(Some((10, 2)))),
            "decimal(10, 2)"
        );
        // inherited from BaseDriver
        assert_eq!(d.to_generic_type("int8", None, None), GenericType::Bigint);
        assert_eq!(
            d.to_generic_type("character varying", None, None),
            GenericType::Text
        );
    }

    #[test]
    fn column_values_are_coerced() {
        let d = driver();
        assert_eq!(
            d.to_column_value(
                &Value::from("2020-01-01T00:00:00.000Z"),
                &GenericType::Timestamp
            ),
            Value::from("2020-01-01T00:00:00.000")
        );
        assert_eq!(
            d.to_column_value(&Value::from("TRUE"), &GenericType::Boolean),
            Value::Bool(true)
        );
        assert_eq!(
            d.to_column_value(&Value::from("false"), &GenericType::Boolean),
            Value::Bool(false)
        );
        assert_eq!(
            d.to_column_value(&Value::from("maybe"), &GenericType::Boolean),
            Value::from("maybe")
        );
        assert_eq!(
            d.to_column_value(&Value::from(1), &GenericType::Int),
            Value::from(1)
        );
    }

    #[test]
    fn create_table_sql_and_options() {
        let d = driver();
        let columns = vec![Column::new("id", "int"), Column::new("name", "string")];
        assert_eq!(
            d.create_table_sql("test.t", &columns),
            "CREATE TABLE test.t (`id` int, `name` varchar(255))"
        );

        let sql = d.create_table_sql_with_options(
            "test.t",
            &columns,
            &CreateTableOptions {
                input_format: Some("csv_no_header".into()),
                delimiter: Some("|".into()),
                disable_quoting: true,
                build_range_end: Some("2020-01-01T00:00:00.000".into()),
                seal_at: Some("2020-01-02T00:00:00.000".into()),
                unique_key: Some("id".into()),
                indexes: Some("INDEX i (id)".into()),
                aggregations: Some(" AGGREGATIONS (sum(x))".into()),
                files: vec!["temp://a.csv.gz".into(), "temp://b.csv.gz".into()],
                ..Default::default()
            },
        );
        assert_eq!(
            sql,
            "CREATE TABLE test.t (`id` int, `name` varchar(255)) WITH (input_format = 'csv_no_header', delimiter = '|', disable_quoting = true, build_range_end = '2020-01-01T00:00:00.000', seal_at = '2020-01-02T00:00:00.000') UNIQUE KEY (id)  AGGREGATIONS (sum(x)) INDEX i (id) LOCATION ?, ?"
        );

        // select_statement / source_table are escaped as SQL string literals
        let sql = d.create_table_sql_with_options(
            "t",
            &columns,
            &CreateTableOptions {
                select_statement: Some("SELECT 'a'".into()),
                source_table: Some(SourceTable {
                    table_name: "src".into(),
                    types: vec![Column::new("a", "string")],
                }),
                stream_offset: Some("earliest".into()),
                ..Default::default()
            },
        );
        assert!(sql.contains("select_statement = 'SELECT ''a'''"));
        assert!(sql.contains("source_table = 'CREATE TABLE src (a varchar(255))'"));
        assert!(sql.contains("stream_offset = 'earliest'"));
    }

    /// `queryTracingObj.buildRangeEnd`, as the orchestrator passes it.
    fn build_range_end_options(build_range_end: &str) -> QueryOptions {
        let mut options = QueryOptions::default();
        options.extra.insert(
            "buildRangeEnd".to_string(),
            Value::from(build_range_end.to_string()),
        );
        options
    }

    #[tokio::test]
    async fn csv_downloads_become_a_location_import() {
        let d = driver();
        let csv = crate::types::TableCsvData {
            csv_file: vec![
                "https://bucket/a-0.csv.gz".into(),
                "https://bucket/a-1.csv.gz".into(),
            ],
            csv_no_header: true,
            csv_delimiter: Some("|".into()),
            csv_disable_quoting: true,
            types: Some(vec![Column::new("id", "bigint")]),
            export_bucket_csv_escape_symbol: Some("\\".into()),
        };
        let options = CubeStoreDriver::csv_import_options(
            &csv,
            "INDEX i (id)",
            " AGGREGATIONS (sum(x))",
            &build_range_end_options("2020-01-01T00:00:00.000"),
        );
        assert_eq!(options.input_format.as_deref(), Some("csv_no_header"));
        assert_eq!(options.delimiter.as_deref(), Some("|"));
        assert!(options.disable_quoting);
        assert_eq!(options.files, csv.csv_file);
        assert_eq!(
            options.build_range_end.as_deref(),
            Some("2020-01-01T00:00:00.000")
        );

        let sql =
            d.create_table_sql_with_options("test.t", &[Column::new("id", "bigint")], &options);
        assert_eq!(
            sql,
            "CREATE TABLE test.t (`id` bigint) WITH (input_format = 'csv_no_header', delimiter = '|', disable_quoting = true, build_range_end = '2020-01-01T00:00:00.000')  AGGREGATIONS (sum(x)) INDEX i (id) LOCATION ?, ?"
        );

        // A memory download still goes through the INSERT path, and a stream
        // is rejected with a named error.
        let err = d
            .import_downloaded_table(
                "test.t",
                &[Column::new("id", "bigint")],
                DownloadedData::Stream(crate::types::StreamTableData {
                    columns: vec![],
                    rows: Box::pin(futures::stream::empty()),
                }),
                &ExternalCreateTableOptions::default(),
                &QueryOptions::default(),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Streaming upload to Cube Store"));
    }

    #[test]
    fn index_clauses() {
        assert_eq!(
            CubeStoreDriver::create_index_string(&CreateTableIndex {
                index_name: "idx".into(),
                type_: "regular".into(),
                columns: vec!["a".into(), "b".into()],
            }),
            "INDEX idx (a,b)"
        );
        assert_eq!(
            CubeStoreDriver::create_index_string(&CreateTableIndex {
                index_name: "agg".into(),
                type_: "aggregate".into(),
                columns: vec!["a".into()],
            }),
            "AGGREGATE INDEX agg (a)"
        );

        let (indexes, aggregations) =
            CubeStoreDriver::external_clauses(&ExternalCreateTableOptions {
                aggregations_columns: vec!["sum(x)".into()],
                create_table_indexes: vec![CreateTableIndex {
                    index_name: "agg".into(),
                    type_: "aggregate".into(),
                    columns: vec!["a".into()],
                }],
                seal_at: None,
            });
        assert_eq!(indexes, "AGGREGATE INDEX agg (a)");
        assert_eq!(aggregations, " AGGREGATIONS (sum(x))");

        // aggregations are only emitted when an aggregating index exists
        let (_, aggregations) = CubeStoreDriver::external_clauses(&ExternalCreateTableOptions {
            aggregations_columns: vec!["sum(x)".into()],
            create_table_indexes: vec![CreateTableIndex {
                index_name: "i".into(),
                type_: "regular".into(),
                columns: vec!["a".into()],
            }],
            seal_at: None,
        });
        assert_eq!(aggregations, "");
    }

    #[test]
    fn sql_contract() {
        let d = driver();
        assert_eq!(d.param(0), "?");
        assert_eq!(d.quote_identifier("a"), "`a`");
        let q = d.information_schema_query();
        assert!(q.contains("FROM information_schema.columns as columns"));
        assert!(q.contains("WHERE columns.table_schema NOT IN ('information_schema', 'system')"));
        assert!(q.contains("columns.column_name as `column_name`"));
        assert!(d.capabilities().csv_import);
        assert!(d.capabilities().stream_import);
        assert!(!d.read_only());
    }

    #[test]
    fn parameters_are_serialised_by_json_type() {
        assert_eq!(
            CubeStoreDriver::to_parameter(&Value::Null).unwrap(),
            QueryParameter::Null
        );
        assert_eq!(
            CubeStoreDriver::to_parameter(&Value::from(true)).unwrap(),
            QueryParameter::Bool(true)
        );
        assert_eq!(
            CubeStoreDriver::to_parameter(&Value::from(7)).unwrap(),
            QueryParameter::Int64(7)
        );
        assert_eq!(
            CubeStoreDriver::to_parameter(&Value::from(1.5)).unwrap(),
            QueryParameter::Float64(1.5)
        );
        assert_eq!(
            CubeStoreDriver::to_parameter(&Value::from("x")).unwrap(),
            QueryParameter::String("x".into())
        );
        assert_eq!(
            CubeStoreDriver::to_parameter(&serde_json::json!([1]))
                .unwrap_err()
                .to_string(),
            "Parameter with type: object is not supported"
        );
    }

    #[tokio::test]
    async fn release_is_idempotent_without_a_connection() {
        let d = driver();
        d.release().await.unwrap();
        d.release().await.unwrap();
        // a closed driver refuses new queries rather than reconnecting
        let err = d
            .query("SELECT 1", &[], &QueryOptions::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Cube Store connection is closed"));
    }
}
