//! ksqlDB driver: port of `@cubejs-backend/ksql-driver` over ksqlDB's REST API
//! (`POST <CUBEJS_DB_URL>/ksql`) with `reqwest` (rustls).
//!
//! ksqlDB is never queried directly: `SELECT` statements are refused, and
//! data reaches Cube only through **streaming pre-aggregations** in Cube
//! Store. The driver creates the pre-aggregation's ksqlDB table
//! (`CREATE TABLE … WITH (KEY_FORMAT='JSON') AS …`), describes it, and hands
//! Cube Store a *streaming source*: either ksqlDB itself (`type: 'ksql'`) or,
//! when `CUBEJS_DB_KAFKA_HOST` is set, the table's Kafka topic
//! (`type: 'kafka'`, one `stream://…/<partition>` location per partition), so
//! that Cube Store consumes the stream on its own.
//!
//! What is here, mirroring the Node driver method by method:
//!
//! * `query` / `prepareQueryWithParams` (ANSI `formatAnsi` interpolation,
//!   trailing `;`, `ksql.streams.auto.offset.reset` from `streamOffset`);
//! * `testConnection` (`SHOW VARIABLES`, then a Kafka metadata round trip
//!   through the pure-Rust `rskafka` client when Kafka is configured —
//!   `kafkajs` `admin().connect()` in Node, SASL/PLAIN and TLS alike);
//! * `SHOW TABLES` / `SHOW STREAMS` / `DESCRIBE` introspection
//!   (`tablesSchema`, `getTablesQuery`, `tableColumnTypes` including the
//!   `WINDOWSTART` / `WINDOWEND` columns of windowed tables);
//! * `loadPreAggregationIntoTable`, `dropTable` (serialised, `DELETE TOPIC`),
//!   `createSchemaIfNotExists` (a no-op);
//! * `downloadTable` / `downloadQueryResults` →
//!   [`KsqlDriver::download_table_streaming`] /
//!   [`KsqlDriver::download_query_results_streaming`], returning
//!   [`KsqlStreamingTableData`] (`StreamingSourceTableData`), and
//!   [`KsqlStreamingTableData::import_into_cube_store`], the port of Cube
//!   Store's `importStreamingSource` (`CREATE SOURCE OR UPDATE` + `CREATE
//!   TABLE … LOCATION 'stream://…'`).
//!
//! Not ported, with a named error (see [`STREAMING_DOWNLOAD_NOT_WIRED`]):
//! the [`Driver::download_table`] / [`Driver::download_query_results`] trait
//! methods cannot return a streaming source, because
//! [`crate::types::DownloadedData`] has no streaming-source variant and the
//! trait's `stream_offset` option is a `bool` (Node passes the
//! pre-aggregation's `'earliest'`/`'latest'` string through it). Wiring that
//! into the orchestrator is a change to shared types, left out on purpose.
//!
//! The SQL dialect (`KsqlQuery`) belongs to the planner, not to this crate.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::config::{data_sources, env_key, DriverConfig, EnvSource, ProcessEnv};
use crate::cubestore::{CreateTableOptions, CubeStoreDriver, SourceTable};
use crate::driver::Driver;
use crate::error::{DriverError, Result};
use crate::escape::format_ansi;
use crate::types::{
    Column, DatabaseStructure, DownloadQueryResultsOptions, DownloadTableOptions, DownloadedData,
    DriverCapabilities, ExternalCreateTableOptions, QueryOptions, QueryResult, SchemaColumn,
    TableMemoryData, TableStructure,
};

/// `getDefaultConcurrency`.
pub const DEFAULT_CONCURRENCY: usize = 1;

/// `driverEnvVariables`.
pub const DRIVER_ENV_VARIABLES: &[&str] = &["CUBEJS_DB_URL", "CUBEJS_DB_USER", "CUBEJS_DB_PASS"];

/// Error text of a `SELECT` sent to ksqlDB.
pub const SELECT_NOT_ALLOWED: &str = "Select queries for ksql allowed only from Cube Store. In order to query ksql create pre-aggregation first.";

/// Error text of `downloadQueryResults` for a query that is not `SELECT * FROM <table>`.
pub const NO_SOURCE_TABLE: &str = "Unable to detect a source table for ksql download query. In order to query ksql use \"SELECT * FROM <TABLE>\"";

/// Named error of the [`Driver::download_table`] / [`Driver::download_query_results`]
/// trait methods (see the module documentation).
pub const STREAMING_DOWNLOAD_NOT_WIRED: &str = "KsqlStreamingDownloadNotWired: ksql pre-aggregations are streaming sources for Cube Store, which the generic Driver::download_table / download_query_results cannot return (DownloadedData has no streaming-source variant). Use KsqlDriver::download_table_streaming / download_query_results_streaming and KsqlStreamingTableData::import_into_cube_store.";

/// Query option carrying the pre-aggregation's `streamOffset`
/// (`earliest` / `latest`), read from [`QueryOptions::extra`].
pub const STREAM_OFFSET_OPTION: &str = "streamOffset";

/// Configuration of [`KsqlDriver`] (`KsqlDriverOptions`).
#[derive(Debug, Clone)]
pub struct KsqlConfig {
    /// Connection settings and global driver knobs.
    pub driver: DriverConfig,
    /// `CUBEJS_DB_URL`: the ksqlDB server (`http://localhost:8088`).
    pub url: Option<String>,
    /// `CUBEJS_DB_USER`.
    pub username: Option<String>,
    /// `CUBEJS_DB_PASS`.
    pub password: Option<String>,
    /// `CUBEJS_DB_KAFKA_HOST`: comma-separated brokers for direct downloads.
    pub kafka_host: Option<String>,
    /// `CUBEJS_DB_KAFKA_USER` (SASL/PLAIN when set).
    pub kafka_user: Option<String>,
    /// `CUBEJS_DB_KAFKA_PASS`.
    pub kafka_password: Option<String>,
    /// `CUBEJS_DB_KAFKA_USE_SSL` (default `false`).
    pub kafka_use_ssl: bool,
    /// `streamingSourceName` option (default `default`). No environment variable.
    pub streaming_source_name: Option<String>,
}

impl KsqlConfig {
    /// Builds the configuration from a generic [`DriverConfig`]. The Kafka
    /// settings are read by [`KsqlConfig::apply_env`].
    pub fn from_driver_config(driver: DriverConfig) -> Self {
        let ds = &driver.data_source;
        Self {
            url: ds.url.clone().filter(|u| !u.is_empty()),
            username: ds.user.clone(),
            password: ds.password.clone(),
            kafka_host: None,
            kafka_user: None,
            kafka_password: None,
            kafka_use_ssl: false,
            streaming_source_name: None,
            driver,
        }
    }

    /// Reads the configuration of `data_source` from the environment.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        let driver = DriverConfig::from_env(data_source)?;
        let mut config = Self::from_driver_config(driver);
        config.apply_env()?;
        Ok(config)
    }

    /// Reads `CUBEJS_DB_KAFKA_*` (data source and pre-aggregation aware)
    /// from the process environment.
    pub fn apply_env(&mut self) -> Result<()> {
        self.apply_env_source(&ProcessEnv)
    }

    /// [`KsqlConfig::apply_env`] against an arbitrary [`EnvSource`].
    pub fn apply_env_source(&mut self, env: &dyn EnvSource) -> Result<()> {
        let declared = data_sources(env);
        let ds = self.driver.data_source.data_source.clone();
        let pre_aggregations = self.driver.data_source.pre_aggregations;
        let key = |origin: &str| env_key(origin, &declared, Some(&ds), pre_aggregations);
        let get = |origin: &str| -> Result<Option<String>> {
            Ok(env.get(&key(origin)?).filter(|v| !v.is_empty()))
        };

        self.kafka_host = get("CUBEJS_DB_KAFKA_HOST")?;
        self.kafka_user = get("CUBEJS_DB_KAFKA_USER")?;
        self.kafka_password = get("CUBEJS_DB_KAFKA_PASS")?;
        self.kafka_use_ssl = match get("CUBEJS_DB_KAFKA_USE_SSL")? {
            // env-var's `asBool`.
            Some(v) => match v.to_lowercase().as_str() {
                "true" | "1" => true,
                "false" | "0" => false,
                _ => {
                    return Err(DriverError::Config(format!(
                        "env-var: \"{}\" should be either \"true\", \"false\", \"TRUE\", \"FALSE\", 1, or 0",
                        key("CUBEJS_DB_KAFKA_USE_SSL")?
                    )))
                }
            },
            None => false,
        };
        Ok(())
    }

    /// The brokers of `CUBEJS_DB_KAFKA_HOST` (`split(',').map(trim)`).
    pub fn kafka_brokers(&self) -> Vec<String> {
        self.kafka_host
            .as_deref()
            .map(|h| h.split(',').map(|b| b.trim().to_string()).collect())
            .unwrap_or_default()
    }
}

/// `streamingSource` of `StreamingSourceTableData`.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamingSource {
    /// `<streamingSourceName>` or `<streamingSourceName>-kafka`.
    pub name: String,
    /// `ksql` or `kafka`.
    pub type_: String,
    /// Credentials in the order the Node driver declares them (it matters:
    /// they become the `VALUES (...)` of `CREATE SOURCE`). Unset values are
    /// `null`.
    pub credentials: Vec<(String, Value)>,
}

/// `StreamingSourceTableData`: what `downloadTable` / `downloadQueryResults`
/// return for ksqlDB.
#[derive(Debug, Clone, PartialEq)]
pub struct KsqlStreamingTableData {
    /// `outputColumnTypes`, else the source table's column types.
    pub types: TableStructure,
    /// Partitions of the source (one Cube Store location each).
    pub partitions: Option<u64>,
    /// ksqlDB table name, or its Kafka topic for a direct Kafka download.
    pub streaming_table: String,
    /// `earliest` / `latest`.
    pub stream_offset: Option<String>,
    /// The `SELECT` Cube Store runs over the stream (download query results).
    pub select_statement: Option<String>,
    pub streaming_source: StreamingSource,
    /// Set when `outputColumnTypes` was given: the source table's own types.
    pub source_table: Option<SourceTable>,
}

impl KsqlStreamingTableData {
    /// `CREATE SOURCE OR UPDATE` statement (and parameters) Cube Store's
    /// `importStreamingSource` runs.
    pub fn create_source_sql(&self) -> (String, Vec<Value>) {
        let source = &self.streaming_source;
        // JS: `${array}` joins with ',' (no space).
        let values = source
            .credentials
            .iter()
            .map(|(k, _)| format!("{k} = ?"))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "CREATE SOURCE OR UPDATE `{}` as ? VALUES ({values})",
            source.name
        );
        let mut params = vec![Value::from(source.type_.clone())];
        params.extend(source.credentials.iter().map(|(_, v)| v.clone()));
        (sql, params)
    }

    /// `stream://<source>/<table>`, or one location per partition.
    pub fn locations(&self) -> Vec<String> {
        let base = format!(
            "stream://{}/{}",
            self.streaming_source.name, self.streaming_table
        );
        match self.partitions {
            Some(partitions) if partitions > 0 => {
                (0..partitions).map(|i| format!("{base}/{i}")).collect()
            }
            _ => vec![base],
        }
    }

    /// `CreateTableOptions` of `importStreamingSource`.
    pub fn cube_store_create_table_options(
        &self,
        unique_key_columns: &[String],
        indexes: Option<String>,
        build_range_end: Option<String>,
        seal_at: Option<String>,
    ) -> CreateTableOptions {
        CreateTableOptions {
            build_range_end,
            unique_key: Some(unique_key_columns.join(",")).filter(|k| !k.is_empty()),
            indexes: indexes.filter(|i| !i.is_empty()),
            files: self.locations(),
            select_statement: self.select_statement.clone(),
            source_table: self.source_table.clone(),
            stream_offset: self.stream_offset.clone(),
            seal_at,
            ..Default::default()
        }
    }

    /// Port of `CubeStoreDriver.importStreamingSource`: registers the source
    /// and creates `table` over the stream.
    ///
    /// `unique_key_columns` is required (`None` is the "older version of
    /// orchestrator" error of the Node driver). `buildRangeEnd` is read from
    /// `query_options.extra`, like the rest of the Cube Store driver does.
    pub async fn import_into_cube_store(
        &self,
        cubestore: &CubeStoreDriver,
        table: &str,
        columns: &[Column],
        unique_key_columns: Option<&[String]>,
        external_options: &ExternalCreateTableOptions,
        query_options: &QueryOptions,
    ) -> Result<()> {
        let Some(unique_key_columns) = unique_key_columns else {
            return Err(DriverError::Config(
                "Older version of orchestrator is being used with newer version of Cube Store driver. Please upgrade cube.js.".to_string(),
            ));
        };
        let (sql, params) = self.create_source_sql();
        cubestore.query(&sql, &params, query_options).await?;

        let indexes = external_options
            .create_table_indexes
            .iter()
            .map(|index| {
                let prefix = if index.type_ == "aggregate" {
                    "AGGREGATE "
                } else {
                    ""
                };
                format!(
                    "{prefix}INDEX {} ({})",
                    index.index_name,
                    index.columns.join(",")
                )
            })
            .collect::<Vec<_>>()
            .join(" ");
        let build_range_end = query_options
            .extra
            .get("buildRangeEnd")
            .and_then(Value::as_str)
            .map(str::to_string);
        let options = self.cube_store_create_table_options(
            unique_key_columns,
            Some(indexes),
            build_range_end,
            external_options.seal_at.clone(),
        );
        cubestore
            .create_table_with_options(table, columns, &options, query_options)
            .await
    }
}

/// One field of `DESCRIBE`.
#[derive(Debug, Clone, Deserialize)]
pub struct KsqlField {
    pub name: String,
    /// `KEY` for key columns.
    #[serde(rename = "type", default)]
    pub type_: Option<String>,
    pub schema: KsqlFieldSchema,
}

/// `fields[].schema`.
#[derive(Debug, Clone, Deserialize)]
pub struct KsqlFieldSchema {
    #[serde(rename = "type")]
    pub type_: String,
}

/// `sourceDescription` of `DESCRIBE`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KsqlSourceDescription {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub fields: Vec<KsqlField>,
    #[serde(rename = "type", default)]
    pub type_: Option<String>,
    /// `SESSION` / `HOPPING` / `TUMBLING` for windowed sources.
    #[serde(default)]
    pub window_type: Option<String>,
    #[serde(default)]
    pub partitions: Option<u64>,
    #[serde(default)]
    pub topic: Option<String>,
}

/// `KsqlDescribeResponse`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KsqlDescribeResponse {
    pub source_description: KsqlSourceDescription,
}

#[derive(Debug, Deserialize)]
struct KsqlNamed {
    name: String,
}

/// A table or stream of `fetchTables`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KsqlTableRef {
    pub table_name: String,
    pub table_schema: String,
    pub full_table_name: String,
}

/// `KsqlQuery.extractTableFromSimpleSelectAsteriskQuery`.
pub fn extract_table_from_simple_select_asterisk_query(sql: &str) -> Option<String> {
    // JS `.` does not match line terminators; `\n` is replaced beforehand.
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = PATTERN.get_or_init(|| {
        regex::Regex::new(
            r#"(?i)^\s*select\s+[^\n\r\x{2028}\x{2029}]*\s+from\s+([a-zA-Z0-9_\-`".*]+)\s*"#,
        )
        .expect("valid regex")
    });
    let sql = sql.replace('\n', " ");
    re.captures(&sql)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// `tableDashName`: the first `.` becomes `-` (`schema.table` → `schema-table`).
pub fn table_dash_name(table: &str) -> String {
    table.replacen('.', "-", 1)
}

/// `fetchTables` filtering / splitting of `SHOW TABLES` + `SHOW STREAMS` names.
pub fn split_table_names(names: &[String], schema_name: Option<&str>) -> Vec<KsqlTableRef> {
    names
        .iter()
        .map(|n| n.split('-').map(str::to_string).collect::<Vec<_>>())
        .filter(|parts| match schema_name {
            None | Some("") => true,
            Some(schema) => {
                parts.get(1).is_some_and(|t| !t.is_empty())
                    && parts[0].to_lowercase() == schema.to_lowercase()
            }
        })
        .map(|parts| {
            let second = parts.get(1).filter(|p| !p.is_empty());
            KsqlTableRef {
                table_name: second.cloned().unwrap_or_else(|| parts[0].clone()),
                table_schema: if second.is_some() {
                    parts[0].clone()
                } else {
                    String::new()
                },
                full_table_name: parts.join("-"),
            }
        })
        .collect()
}

/// Represents `data[0]` of a `/ksql` response (an arbitrary object) as a
/// one-row result whose columns are the object's keys.
pub fn statement_to_result(statement: &Value) -> QueryResult {
    match statement {
        Value::Null => QueryResult::default(),
        Value::Object(map) => QueryResult::new(
            map.keys().map(|k| Column::new(k.clone(), "text")).collect(),
            vec![map.values().cloned().collect()],
        ),
        other => QueryResult::new(
            vec![Column::new("result", "text")],
            vec![vec![other.clone()]],
        ),
    }
}

/// ksqlDB driver.
pub struct KsqlDriver {
    config: KsqlConfig,
    client: reqwest::Client,
    /// `dropTableMutex`.
    drop_table_mutex: Mutex<()>,
}

impl std::fmt::Debug for KsqlDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KsqlDriver")
            .field("url", &self.config.url)
            .field("kafka_host", &self.config.kafka_host)
            .finish()
    }
}

impl KsqlDriver {
    /// Creates the driver. Nothing is sent until the first statement.
    pub fn new(config: KsqlConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(format!("CubeDev_Cube/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| DriverError::Config(format!("Unable to build the HTTP client: {e}")))?;
        Ok(Self {
            config,
            client,
            drop_table_mutex: Mutex::new(()),
        })
    }

    /// Creates the driver from `CUBEJS_DB_*`.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Self::new(KsqlConfig::from_env(data_source)?)
    }

    /// The driver configuration.
    pub fn ksql_config(&self) -> &KsqlConfig {
        &self.config
    }

    /// `prepareQueryWithParams`: ANSI client-side interpolation.
    pub fn prepare_query_with_params(&self, query: &str, values: &[Value]) -> String {
        format_ansi(query, values)
    }

    /// The `/ksql` request body.
    pub fn statement_body(
        &self,
        query: &str,
        values: &[Value],
        stream_offset: Option<&str>,
    ) -> Value {
        let mut body = json!({
            "ksql": format!("{};", self.prepare_query_with_params(query, values)),
        });
        if let Some(offset) = stream_offset.filter(|o| !o.is_empty()) {
            body["streamsProperties"] = json!({ "ksql.streams.auto.offset.reset": offset });
        }
        body
    }

    /// `apiQuery`.
    async fn api_query(&self, path: &str, body: Value) -> Result<Value> {
        let ksql = body
            .get("ksql")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let fail =
            |message: String| DriverError::Query(format!("ksql API error for '{ksql}': {message}"));
        let Some(url) = &self.config.url else {
            return Err(fail("CUBEJS_DB_URL is not set".to_string()));
        };
        let mut request = self
            .client
            .post(format!("{}{path}", url.trim_end_matches('/')))
            .header("Accept", "application/vnd.ksql.v1+json")
            .json(&body);
        if self.config.username.is_some() || self.config.password.is_some() {
            request = request.basic_auth(
                self.config.username.clone().unwrap_or_default(),
                Some(self.config.password.clone().unwrap_or_default()),
            );
        }
        let response = request.send().await.map_err(|e| fail(e.to_string()))?;
        let status = response.status();
        let text = response.text().await.map_err(|e| fail(e.to_string()))?;
        if !status.is_success() {
            let message = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_string))
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| format!("Request failed with status code {}", status.as_u16()));
            return Err(fail(message));
        }
        serde_json::from_str(&text).map_err(|e| fail(format!("unexpected response: {e}: {text}")))
    }

    /// `query`: runs one ksqlDB statement and returns `data[0]` as JSON.
    pub async fn statement(
        &self,
        query: &str,
        values: &[Value],
        stream_offset: Option<&str>,
    ) -> Result<Value> {
        if query.to_lowercase().starts_with("select") {
            return Err(DriverError::Query(SELECT_NOT_ALLOWED.to_string()));
        }
        let data = self
            .api_query("/ksql", self.statement_body(query, values, stream_offset))
            .await?;
        Ok(match data {
            Value::Array(mut items) if !items.is_empty() => items.swap_remove(0),
            Value::Array(_) => Value::Null,
            other => other,
        })
    }

    /// Kafka connectivity check (`kafkaClient.admin().connect()`).
    pub async fn test_kafka_connection(&self) -> Result<()> {
        let brokers = self.config.kafka_brokers();
        if brokers.is_empty() {
            return Ok(());
        }
        let mut builder = rskafka::client::ClientBuilder::new(brokers.clone())
            .client_id("Cube")
            .backoff_config(rskafka::BackoffConfig {
                deadline: Some(self.test_connection_timeout()),
                ..Default::default()
            });
        if self.config.kafka_use_ssl {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .map_err(|e| DriverError::Config(format!("Kafka TLS configuration: {e}")))?
            .with_root_certificates(roots)
            .with_no_client_auth();
            builder = builder.tls_config(Arc::new(tls));
        }
        if let Some(user) = &self.config.kafka_user {
            builder = builder.sasl_config(rskafka::client::SaslConfig::Plain(
                rskafka::client::Credentials::new(
                    user.clone(),
                    self.config.kafka_password.clone().unwrap_or_default(),
                ),
            ));
        }
        let connect = async {
            let client = builder.build().await?;
            client.list_topics().await?;
            Ok::<(), rskafka::client::error::Error>(())
        };
        match tokio::time::timeout(self.test_connection_timeout(), connect).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(DriverError::Connection {
                pool_name: format!("kafka {}", brokers.join(",")),
                message: e.to_string(),
            }),
            Err(_) => Err(DriverError::Connection {
                pool_name: format!("kafka {}", brokers.join(",")),
                message: "timed out".to_string(),
            }),
        }
    }

    /// `fetchTables`.
    pub async fn fetch_tables(&self, schema_name: Option<&str>) -> Result<Vec<KsqlTableRef>> {
        let (tables, streams) = futures::try_join!(
            self.statement("SHOW TABLES", &[], None),
            self.statement("SHOW STREAMS", &[], None),
        )?;
        let names_of = |v: &Value, key: &str| -> Result<Vec<String>> {
            let list: Vec<KsqlNamed> =
                serde_json::from_value(v.get(key).cloned().unwrap_or(Value::Array(vec![])))
                    .map_err(|e| {
                        DriverError::Query(format!("Unexpected SHOW {key} response: {e}"))
                    })?;
            Ok(list.into_iter().map(|t| t.name).collect())
        };
        let mut names = names_of(&tables, "tables")?;
        names.extend(names_of(&streams, "streams")?);
        Ok(split_table_names(&names, schema_name))
    }

    /// `DESCRIBE <table>` (the name is dash-converted and quoted).
    pub async fn describe_table(&self, streaming_table: &str) -> Result<KsqlDescribeResponse> {
        self.describe_raw(&table_dash_name(streaming_table)).await
    }

    async fn describe_raw(&self, name: &str) -> Result<KsqlDescribeResponse> {
        let data = self
            .statement(
                &format!("DESCRIBE {}", self.quote_identifier(name)),
                &[],
                None,
            )
            .await?;
        serde_json::from_value(data)
            .map_err(|e| DriverError::Query(format!("Unexpected DESCRIBE response: {e}")))
    }

    /// `tableColumnTypes` of an already fetched description.
    pub fn describe_column_types(&self, describe: &KsqlDescribeResponse) -> TableStructure {
        let description = &describe.source_description;
        let columns: Vec<(String, String)> = if description
            .window_type
            .as_deref()
            .is_some_and(|w| !w.is_empty())
        {
            let is_key = |f: &&KsqlField| f.type_.as_deref() == Some("KEY");
            description
                .fields
                .iter()
                .filter(is_key)
                .map(|f| (f.name.clone(), f.schema.type_.clone()))
                .chain([
                    ("WINDOWSTART".to_string(), "INTEGER".to_string()),
                    ("WINDOWEND".to_string(), "INTEGER".to_string()),
                ])
                .chain(
                    description
                        .fields
                        .iter()
                        .filter(|f| !is_key(f))
                        .map(|f| (f.name.clone(), f.schema.type_.clone())),
                )
                .collect()
        } else {
            description
                .fields
                .iter()
                .map(|f| (f.name.clone(), f.schema.type_.clone()))
                .collect()
        };
        columns
            .into_iter()
            .map(|(name, type_)| Column::new(name, self.to_generic_type(&type_, None, None)))
            .collect()
    }

    /// `getStreamingTableData`.
    pub async fn streaming_table_data(
        &self,
        streaming_table: &str,
        select_statement: Option<String>,
        stream_offset: Option<String>,
        output_column_types: Option<TableStructure>,
    ) -> Result<KsqlStreamingTableData> {
        let describe = self.describe_table(streaming_table).await?;
        Ok(self.build_streaming_table_data(
            streaming_table,
            &describe,
            select_statement,
            stream_offset,
            output_column_types,
        ))
    }

    fn build_streaming_table_data(
        &self,
        streaming_table: &str,
        describe: &KsqlDescribeResponse,
        select_statement: Option<String>,
        stream_offset: Option<String>,
        output_column_types: Option<TableStructure>,
    ) -> KsqlStreamingTableData {
        let name = self
            .config
            .streaming_source_name
            .clone()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "default".to_string());
        let kafka_direct_download = self.config.kafka_host.is_some();
        let opt = |v: &Option<String>| v.clone().map(Value::from).unwrap_or(Value::Null);
        let streaming_source = if kafka_direct_download {
            StreamingSource {
                name: format!("{name}-kafka"),
                type_: "kafka".to_string(),
                credentials: vec![
                    ("user".to_string(), opt(&self.config.kafka_user)),
                    ("password".to_string(), opt(&self.config.kafka_password)),
                    ("host".to_string(), opt(&self.config.kafka_host)),
                    (
                        "use_ssl".to_string(),
                        Value::Bool(self.config.kafka_use_ssl),
                    ),
                ],
            }
        } else {
            StreamingSource {
                name,
                type_: "ksql".to_string(),
                credentials: vec![
                    ("user".to_string(), opt(&self.config.username)),
                    ("password".to_string(), opt(&self.config.password)),
                    ("url".to_string(), opt(&self.config.url)),
                ],
            }
        };
        let source_table_types = self.describe_column_types(describe);
        let streaming_table = if kafka_direct_download {
            describe
                .source_description
                .topic
                .clone()
                .unwrap_or_default()
        } else {
            streaming_table.to_string()
        };
        let source_table = output_column_types.as_ref().map(|_| SourceTable {
            table_name: streaming_table.clone(),
            types: source_table_types.clone(),
        });
        KsqlStreamingTableData {
            types: output_column_types.unwrap_or(source_table_types),
            partitions: describe.source_description.partitions,
            streaming_table,
            stream_offset,
            select_statement,
            streaming_source,
            source_table,
        }
    }

    /// `downloadTable`: the streaming source of a pre-aggregation table.
    pub async fn download_table_streaming(
        &self,
        table: &str,
        stream_offset: Option<String>,
    ) -> Result<KsqlStreamingTableData> {
        self.streaming_table_data(&table_dash_name(table), None, stream_offset, None)
            .await
    }

    /// `downloadQueryResults`: only `SELECT * FROM <table>`-shaped queries,
    /// whose (interpolated) text becomes Cube Store's `select_statement`.
    pub async fn download_query_results_streaming(
        &self,
        query: &str,
        params: &[Value],
        stream_offset: Option<String>,
        output_column_types: Option<TableStructure>,
    ) -> Result<KsqlStreamingTableData> {
        let Some(table) = extract_table_from_simple_select_asterisk_query(query) else {
            return Err(DriverError::Query(NO_SOURCE_TABLE.to_string()));
        };
        let select_statement = self.prepare_query_with_params(query, params);
        self.streaming_table_data(
            &table,
            Some(select_statement),
            stream_offset,
            output_column_types,
        )
        .await
    }
}

fn stream_offset(options: &QueryOptions) -> Option<String> {
    options
        .extra
        .get(STREAM_OFFSET_OPTION)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[async_trait]
impl Driver for KsqlDriver {
    fn config(&self) -> &DriverConfig {
        &self.config.driver
    }

    /// `SHOW VARIABLES`, then the Kafka brokers when configured.
    async fn test_connection(&self) -> Result<()> {
        self.statement("SHOW VARIABLES", &[], None).await?;
        self.test_kafka_connection().await
    }

    /// Runs a statement; `data[0]` is returned as a one-row result (see
    /// [`statement_to_result`]). `QueryOptions.extra.streamOffset` sets
    /// `ksql.streams.auto.offset.reset`.
    async fn query(
        &self,
        sql: &str,
        params: &[Value],
        options: &QueryOptions,
    ) -> Result<QueryResult> {
        let data = self
            .statement(sql, params, stream_offset(options).as_deref())
            .await?;
        Ok(statement_to_result(&data))
    }

    /// Backticks, like ksqlDB.
    fn quote_identifier(&self, identifier: &str) -> String {
        format!("`{identifier}`")
    }

    fn capabilities(&self) -> DriverCapabilities {
        DriverCapabilities {
            streaming_source: true,
            ..Default::default()
        }
    }

    /// There are no schemas in ksqlDB.
    async fn create_schema_if_not_exists(&self, _schema_name: &str) -> Result<()> {
        Ok(())
    }

    /// `getTablesQuery`: `schema-table` names of `schema_name`.
    async fn get_tables_query(&self, schema_name: &str) -> Result<Vec<String>> {
        Ok(self
            .fetch_tables(Some(schema_name))
            .await?
            .into_iter()
            .filter(|t| !t.table_schema.is_empty())
            .map(|t| t.table_name)
            .collect())
    }

    /// `tablesSchema`: `DESCRIBE` of every table and stream.
    async fn tables_schema(&self) -> Result<DatabaseStructure> {
        let tables = self.fetch_tables(None).await?;
        let describes = futures::future::try_join_all(
            tables.iter().map(|t| self.describe_raw(&t.full_table_name)),
        )
        .await?;
        let mut schema = DatabaseStructure::new();
        for (table, describe) in tables.into_iter().zip(describes) {
            schema.entry(table.table_schema).or_default().insert(
                table.table_name,
                describe
                    .source_description
                    .fields
                    .into_iter()
                    .map(|f| SchemaColumn {
                        name: f.name,
                        type_: f.schema.type_,
                        attributes: Vec::new(),
                        foreign_keys: Vec::new(),
                    })
                    .collect(),
            );
        }
        Ok(schema)
    }

    async fn table_column_types(&self, table: &str) -> Result<TableStructure> {
        let describe = self.describe_table(table).await?;
        Ok(self.describe_column_types(&describe))
    }

    async fn table_column_types_with_precision(&self, table: &str) -> Result<TableStructure> {
        self.table_column_types(table).await
    }

    /// `loadPreAggregationIntoTable`: the table name becomes its dash form.
    async fn load_pre_aggregation_into_table(
        &self,
        pre_aggregation_table_name: &str,
        load_sql: &str,
        params: &[Value],
        options: &QueryOptions,
    ) -> Result<QueryResult> {
        let sql = load_sql.replacen(
            pre_aggregation_table_name,
            &table_dash_name(pre_aggregation_table_name),
            1,
        );
        let mut query_options = QueryOptions::default();
        if let Some(offset) = stream_offset(options) {
            query_options
                .extra
                .insert(STREAM_OFFSET_OPTION.to_string(), Value::from(offset));
        }
        self.query(&sql, params, &query_options).await
    }

    /// `DROP TABLE … DELETE TOPIC`, one at a time.
    async fn drop_table(&self, table_name: &str, options: &QueryOptions) -> Result<()> {
        let _guard = self.drop_table_mutex.lock().await;
        self.query(
            &format!(
                "DROP TABLE {} DELETE TOPIC",
                self.quote_identifier(&table_dash_name(table_name))
            ),
            &[],
            options,
        )
        .await?;
        Ok(())
    }

    async fn download_query_results(
        &self,
        _sql: &str,
        _params: &[Value],
        _options: &DownloadQueryResultsOptions,
    ) -> Result<DownloadedData> {
        Err(DriverError::NotImplemented(
            STREAMING_DOWNLOAD_NOT_WIRED.to_string(),
        ))
    }

    async fn download_table(
        &self,
        _table: &str,
        _options: &DownloadTableOptions,
    ) -> Result<TableMemoryData> {
        Err(DriverError::NotImplemented(
            STREAMING_DOWNLOAD_NOT_WIRED.to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn driver() -> KsqlDriver {
        let mut driver = DriverConfig::default();
        driver.data_source.url = Some("http://localhost:8088".into());
        KsqlDriver::new(KsqlConfig::from_driver_config(driver)).unwrap()
    }

    fn config(pairs: &[(&str, &str)]) -> Result<KsqlConfig> {
        let env: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let driver = DriverConfig::from_env_source(&env, None, false)?;
        let mut config = KsqlConfig::from_driver_config(driver);
        config.apply_env_source(&env)?;
        Ok(config)
    }

    #[test]
    fn config_from_env() {
        let c = config(&[
            ("CUBEJS_DB_URL", "http://ksql:8088"),
            ("CUBEJS_DB_USER", "u"),
            ("CUBEJS_DB_PASS", "p"),
            ("CUBEJS_DB_KAFKA_HOST", "k1:9092, k2:9092"),
            ("CUBEJS_DB_KAFKA_USER", "ku"),
            ("CUBEJS_DB_KAFKA_PASS", "kp"),
            ("CUBEJS_DB_KAFKA_USE_SSL", "true"),
        ])
        .unwrap();
        assert_eq!(c.url.as_deref(), Some("http://ksql:8088"));
        assert_eq!(c.username.as_deref(), Some("u"));
        assert_eq!(c.password.as_deref(), Some("p"));
        assert_eq!(c.kafka_brokers(), vec!["k1:9092", "k2:9092"]);
        assert_eq!(c.kafka_user.as_deref(), Some("ku"));
        assert_eq!(c.kafka_password.as_deref(), Some("kp"));
        assert!(c.kafka_use_ssl);

        let c = config(&[]).unwrap();
        assert!(c.kafka_brokers().is_empty());
        assert!(!c.kafka_use_ssl);
        assert!(
            config(&[("CUBEJS_DB_KAFKA_USE_SSL", "1")])
                .unwrap()
                .kafka_use_ssl
        );
        assert!(config(&[("CUBEJS_DB_KAFKA_USE_SSL", "maybe")]).is_err());
    }

    #[test]
    fn sql_contract() {
        let d = driver();
        assert_eq!(d.quote_identifier("T"), "`T`");
        assert!(d.capabilities().streaming_source);
        assert!(!d.capabilities().csv_import);
        assert!(!d.read_only());
        assert_eq!(DEFAULT_CONCURRENCY, 1);
    }

    #[tokio::test]
    async fn select_is_refused() {
        let d = driver();
        for sql in ["SELECT 1", "select * from t", "Select x FROM y"] {
            let err = d
                .query(sql, &[], &QueryOptions::default())
                .await
                .unwrap_err();
            assert_eq!(err.to_string(), SELECT_NOT_ALLOWED);
        }
    }

    #[tokio::test]
    async fn trait_downloads_fail_with_a_named_error() {
        let d = driver();
        let err = d
            .download_table("s.t", &DownloadTableOptions::default())
            .await
            .unwrap_err();
        assert!(err.to_string().starts_with("KsqlStreamingDownloadNotWired"));
        let err = d
            .download_query_results(
                "SELECT * FROM t",
                &[],
                &DownloadQueryResultsOptions::default(),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().starts_with("KsqlStreamingDownloadNotWired"));
    }

    #[tokio::test]
    async fn download_query_results_needs_a_simple_select() {
        let d = driver();
        let err = d
            .download_query_results_streaming("SHOW TABLES", &[], None, None)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), NO_SOURCE_TABLE);
    }

    #[tokio::test]
    async fn create_schema_is_a_no_op() {
        driver().create_schema_if_not_exists("any").await.unwrap();
    }

    #[test]
    fn request_body() {
        let d = driver();
        assert_eq!(
            d.statement_body("SHOW STREAMS", &[], None),
            json!({ "ksql": "SHOW STREAMS;" })
        );
        assert_eq!(
            d.statement_body(
                "DROP TABLE `a` WHERE x = ?",
                &[json!("v")],
                Some("earliest")
            ),
            json!({
                "ksql": "DROP TABLE `a` WHERE x = 'v';",
                "streamsProperties": { "ksql.streams.auto.offset.reset": "earliest" },
            })
        );
    }

    #[test]
    fn dash_names() {
        assert_eq!(
            table_dash_name("stb_pre_aggregations.orders_main"),
            "stb_pre_aggregations-orders_main"
        );
        assert_eq!(table_dash_name("a.b.c"), "a-b.c");
        assert_eq!(table_dash_name("plain"), "plain");
    }

    #[test]
    fn table_names_are_split_on_dashes() {
        let names: Vec<String> = ["S-T1", "PLAIN", "s-T2", "OTHER-T3", "A-B-C"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let all = split_table_names(&names, None);
        assert_eq!(
            all[0],
            KsqlTableRef {
                table_name: "T1".into(),
                table_schema: "S".into(),
                full_table_name: "S-T1".into()
            }
        );
        assert_eq!(
            all[1],
            KsqlTableRef {
                table_name: "PLAIN".into(),
                table_schema: "".into(),
                full_table_name: "PLAIN".into()
            }
        );
        assert_eq!(all[4].table_name, "B");
        assert_eq!(all[4].full_table_name, "A-B-C");

        let s: Vec<_> = split_table_names(&names, Some("s"))
            .into_iter()
            .map(|t| t.table_name)
            .collect();
        assert_eq!(s, vec!["T1", "T2"]);
    }

    #[test]
    fn simple_select_asterisk_extraction() {
        let t = extract_table_from_simple_select_asterisk_query;
        assert_eq!(t("SELECT * FROM `orders`").as_deref(), Some("`orders`"));
        assert_eq!(
            t("  select *\nfrom S.T WHERE x = 1").as_deref(),
            Some("S.T")
        );
        assert_eq!(t("SELECT a, b FROM \"s-t\" x").as_deref(), Some("\"s-t\""));
        // Greedy: the last `FROM` wins, as in the JS regex.
        assert_eq!(
            t("SELECT * FROM (SELECT * FROM inner_t) AS q").as_deref(),
            Some("inner_t")
        );
        assert_eq!(t("SHOW TABLES"), None);
        assert_eq!(t("SELECT 1"), None);
    }

    fn describe(windowed: bool) -> KsqlDescribeResponse {
        serde_json::from_value(json!({
            "@type": "sourceDescription",
            "sourceDescription": {
                "name": "S-T",
                "type": "TABLE",
                "windowType": if windowed { json!("TUMBLING") } else { Value::Null },
                "partitions": 3,
                "topic": "s_t_topic",
                "fields": [
                    { "name": "VAL", "schema": { "type": "BIGINT", "fields": null } },
                    { "name": "ID", "type": "KEY", "schema": { "type": "STRING" } },
                    { "name": "TS", "schema": { "type": "TIMESTAMP" } },
                    { "name": "AMOUNT", "schema": { "type": "DOUBLE" } },
                ],
            }
        }))
        .unwrap()
    }

    #[test]
    fn column_types_from_describe() {
        let d = driver();
        assert_eq!(
            d.describe_column_types(&describe(false)),
            vec![
                Column::new("VAL", "bigint"),
                Column::new("ID", "text"),
                // Unknown types pass through unchanged, like `BaseDriver.toGenericType`.
                Column::new("TS", "TIMESTAMP"),
                Column::new("AMOUNT", "DOUBLE"),
            ]
        );
        assert_eq!(
            d.describe_column_types(&describe(true)),
            vec![
                Column::new("ID", "text"),
                Column::new("WINDOWSTART", "int"),
                Column::new("WINDOWEND", "int"),
                Column::new("VAL", "bigint"),
                Column::new("TS", "TIMESTAMP"),
                Column::new("AMOUNT", "DOUBLE"),
            ]
        );
    }

    #[test]
    fn streaming_data_through_ksql() {
        let mut driver_config = DriverConfig::default();
        driver_config.data_source.url = Some("http://ksql:8088".into());
        driver_config.data_source.user = Some("u".into());
        let d = KsqlDriver::new(KsqlConfig::from_driver_config(driver_config)).unwrap();
        let data = d.build_streaming_table_data(
            "S-T",
            &describe(false),
            Some("SELECT * FROM `S-T`".into()),
            Some("earliest".into()),
            Some(vec![Column::new("ID", "text")]),
        );
        assert_eq!(data.streaming_table, "S-T");
        assert_eq!(data.partitions, Some(3));
        assert_eq!(data.types, vec![Column::new("ID", "text")]);
        assert_eq!(data.streaming_source.name, "default");
        assert_eq!(data.streaming_source.type_, "ksql");
        let source_table = data.source_table.clone().unwrap();
        assert_eq!(source_table.table_name, "S-T");
        assert_eq!(source_table.types.len(), 4);

        let (sql, params) = data.create_source_sql();
        assert_eq!(
            sql,
            "CREATE SOURCE OR UPDATE `default` as ? VALUES (user = ?,password = ?,url = ?)"
        );
        assert_eq!(
            params,
            vec![
                json!("ksql"),
                json!("u"),
                Value::Null,
                json!("http://ksql:8088")
            ]
        );
        assert_eq!(
            data.locations(),
            vec![
                "stream://default/S-T/0",
                "stream://default/S-T/1",
                "stream://default/S-T/2"
            ]
        );

        let options = data.cube_store_create_table_options(
            &["ID".to_string(), "VAL".to_string()],
            Some(String::new()),
            Some("2024-01-01T00:00:00.000".into()),
            None,
        );
        assert_eq!(options.unique_key.as_deref(), Some("ID,VAL"));
        assert_eq!(options.indexes, None);
        assert_eq!(options.files.len(), 3);
        assert_eq!(options.stream_offset.as_deref(), Some("earliest"));
        assert_eq!(
            options.select_statement.as_deref(),
            Some("SELECT * FROM `S-T`")
        );
        let options = data.cube_store_create_table_options(&[], None, None, None);
        assert_eq!(options.unique_key, None);
    }

    #[test]
    fn streaming_data_through_kafka() {
        let mut c = config(&[
            ("CUBEJS_DB_URL", "http://ksql:8088"),
            ("CUBEJS_DB_KAFKA_HOST", "broker:9092"),
            ("CUBEJS_DB_KAFKA_USER", "ku"),
        ])
        .unwrap();
        c.streaming_source_name = Some("events".into());
        let d = KsqlDriver::new(c).unwrap();
        let data = d.build_streaming_table_data("S-T", &describe(false), None, None, None);
        assert_eq!(data.streaming_table, "s_t_topic");
        assert_eq!(data.streaming_source.name, "events-kafka");
        assert_eq!(data.streaming_source.type_, "kafka");
        assert_eq!(data.source_table, None);
        assert_eq!(data.types.len(), 4);
        let (sql, params) = data.create_source_sql();
        assert_eq!(
            sql,
            "CREATE SOURCE OR UPDATE `events-kafka` as ? VALUES (user = ?,password = ?,host = ?,use_ssl = ?)"
        );
        assert_eq!(
            params,
            vec![
                json!("kafka"),
                json!("ku"),
                Value::Null,
                json!("broker:9092"),
                json!(false)
            ]
        );
        assert_eq!(data.locations()[0], "stream://events-kafka/s_t_topic/0");
    }

    #[test]
    fn statement_results() {
        let r = statement_to_result(&json!({ "@type": "tables", "tables": [] }));
        assert_eq!(r.len(), 1);
        assert_eq!(r.get(0, "tables"), Some(&json!([])));
        assert!(statement_to_result(&Value::Null).is_empty());
    }

    // Port of `test/unit/params-escaping.test.ts`.
    mod params_escaping {
        use super::driver;
        use serde_json::json;

        #[test]
        fn doubles_quotes_so_a_value_cannot_break_out_of_the_literal() {
            let sql = driver().prepare_query_with_params(
                "CREATE TABLE t AS SELECT * FROM s WHERE status = ?",
                &[json!("a' OR 1=1 --")],
            );
            assert_eq!(
                sql,
                "CREATE TABLE t AS SELECT * FROM s WHERE status = 'a'' OR 1=1 --'"
            );
        }

        #[test]
        fn keeps_the_literal_closed_for_a_value_ending_in_a_backslash() {
            let sql = driver().prepare_query_with_params(
                "CREATE TABLE t AS SELECT * FROM s WHERE name = ? AND status = ?",
                &[json!(r"payload\"), json!("new")],
            );
            assert_eq!(
                sql,
                r"CREATE TABLE t AS SELECT * FROM s WHERE name = 'payload\' AND status = 'new'"
            );
        }

        #[test]
        fn keeps_the_literal_closed_for_a_backslash_then_quote_payload() {
            let sql = driver().prepare_query_with_params(
                "CREATE TABLE t AS SELECT * FROM s WHERE name = ?",
                &[json!(r"foo\' OR 1=1 --")],
            );
            assert_eq!(
                sql,
                r"CREATE TABLE t AS SELECT * FROM s WHERE name = 'foo\'' OR 1=1 --'"
            );
        }

        #[test]
        fn does_not_double_literal_backslashes() {
            let sql = driver().prepare_query_with_params(
                "CREATE TABLE t AS SELECT * FROM s WHERE path = ?",
                &[json!(r"folder\\name")],
            );
            assert_eq!(
                sql,
                r"CREATE TABLE t AS SELECT * FROM s WHERE path = 'folder\\name'"
            );
        }

        #[test]
        fn escapes_every_element_of_an_array_parameter() {
            let sql = driver().prepare_query_with_params(
                "CREATE TABLE t AS SELECT * FROM s WHERE status IN (?)",
                &[json!(["it's", "b"])],
            );
            assert_eq!(
                sql,
                "CREATE TABLE t AS SELECT * FROM s WHERE status IN ('it''s', 'b')"
            );
        }

        #[test]
        fn substitutes_multiple_placeholders_in_order() {
            let sql = driver().prepare_query_with_params(
                "CREATE TABLE t AS SELECT * FROM s WHERE status = ? AND amount > ?",
                &[json!("new"), json!(100)],
            );
            assert_eq!(
                sql,
                "CREATE TABLE t AS SELECT * FROM s WHERE status = 'new' AND amount > 100"
            );
        }

        #[test]
        fn leaves_a_query_without_parameters_untouched() {
            assert_eq!(
                driver().prepare_query_with_params("SHOW VARIABLES", &[]),
                "SHOW VARIABLES"
            );
        }
    }
}
