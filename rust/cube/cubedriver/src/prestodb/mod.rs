//! Presto driver: port of `@cubejs-backend/prestodb-driver` over the Presto
//! client REST protocol (`reqwest` with rustls). The Trino driver
//! ([`crate::trino`]) is the same driver speaking `X-Trino-*` headers.
//!
//! * [`client`]: the `POST /v1/statement` + `nextUri` polling protocol
//!   (port of `presto-client` 1.2 plus the Cube custom-headers patch).
//! * [`export_bucket`]: listing and signing the unloaded CSV files in S3 or
//!   GCS (`extractUnloadedFilesFromS3` / `extractFilesFromGCS`).
//!
//! Values are inlined client-side with the ANSI escaper (`formatAnsi`), as in
//! Node; the rows keep the JSON values the coordinator sends.
//!
//! Differences from Node, all deliberate:
//!
//! * rows are returned in server order. The Node driver prepended each page
//!   (`concat(normalData, fullData)`), reversing the page order of results
//!   longer than one page — which broke `ORDER BY`;
//! * a streamed result resolves even when the statement finishes without
//!   columns (Node never resolved);
//! * the S3 listing prefix ends with `/` so that `schema/orders` does not also
//!   pick up the files of `schema/orders_2`, and listings are paginated;
//! * `headers` (custom HTTP headers) can only be set programmatically: they
//!   came from `cube.js` in Node, which the Rust server does not load.

pub mod client;
pub mod export_bucket;

use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::Value;

use crate::config::{data_sources, env_key, DriverConfig, EnvSource, ProcessEnv, SslConfig};
use crate::driver::Driver;
use crate::error::{DriverError, Result};
use crate::escape::format_ansi;
use crate::type_detection::detect_types_from_tabular;
use crate::types::{
    Column, DownloadQueryResultsOptions, DownloadedData, DriverCapabilities, QueryOptions,
    QueryResult, Row, StreamOptions, StreamTableData, TableCsvData, TableStructure, UnloadOptions,
};

pub use client::{Engine, PrestoColumn, StatementClient, StatementOptions};

/// `getDefaultConcurrency`.
pub const DEFAULT_CONCURRENCY: usize = 2;
/// `presto-client` default host.
pub const DEFAULT_HOST: &str = "localhost";
/// `presto-client` default port.
pub const DEFAULT_PORT: u16 = 8080;
/// `SUPPORTED_BUCKET_TYPES`.
pub const SUPPORTED_BUCKET_TYPES: &[&str] = &["gcs", "s3"];

/// Export bucket settings (`PrestoDriverExportBucket`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrestoExportBucket {
    /// `CUBEJS_DB_EXPORT_BUCKET_TYPE` (`gcs` or `s3`).
    pub bucket_type: Option<String>,
    /// `CUBEJS_DB_EXPORT_BUCKET`.
    pub export_bucket: Option<String>,
    /// `CUBEJS_DB_EXPORT_BUCKET_AWS_KEY`.
    pub access_key_id: Option<String>,
    /// `CUBEJS_DB_EXPORT_BUCKET_AWS_SECRET`.
    pub secret_access_key: Option<String>,
    /// `CUBEJS_DB_EXPORT_BUCKET_AWS_REGION`.
    pub region: Option<String>,
    /// `CUBEJS_DB_EXPORT_GCS_CREDENTIALS`, decoded from base64 (service-account JSON).
    pub gcs_credentials: Option<String>,
    /// `exportBucketS3AdvancedFS`: write through `s3a://` instead of `s3://`.
    pub s3_advanced_fs: bool,
    /// `exportBucketCsvEscapeSymbol` (a programmatic option in Node too).
    pub csv_escape_symbol: Option<String>,
    /// S3-compatible endpoint (MinIO, …); defaults to `AWS_ENDPOINT_URL_S3` /
    /// `AWS_ENDPOINT_URL`, else AWS.
    pub s3_endpoint: Option<String>,
}

/// Configuration of [`PrestoDriver`] (`PrestoDriverConfiguration`).
#[derive(Debug, Clone)]
pub struct PrestoConfig {
    /// Connection settings and global driver knobs.
    pub driver: DriverConfig,
    /// `presto` or `trino` headers.
    pub engine: Engine,
    /// `CUBEJS_DB_HOST` (default `localhost`).
    pub host: String,
    /// `CUBEJS_DB_PORT` (default 8080).
    pub port: u16,
    /// `CUBEJS_DB_PRESTO_CATALOG`, else the deprecated `CUBEJS_DB_CATALOG`.
    pub catalog: Option<String>,
    /// `CUBEJS_DB_NAME`, else `CUBEJS_DB_SCHEMA`.
    pub schema: Option<String>,
    /// `CUBEJS_DB_USER` (the client falls back to `$USER`).
    pub user: Option<String>,
    /// `CUBEJS_DB_PASS`: HTTP basic auth with the user.
    pub password: Option<String>,
    /// `CUBEJS_DB_PRESTO_AUTH_TOKEN`: `Authorization: Bearer <token>`.
    pub auth_token: Option<String>,
    /// `CUBEJS_DB_SSL*`: HTTPS when set.
    pub ssl: Option<SslConfig>,
    /// `CUBEJS_DB_QUERY_TIMEOUT` (whole-query timeout; zero disables it).
    pub query_timeout: Duration,
    /// `CUBEJS_DB_USE_SELECT_TEST_CONNECTION`.
    pub use_select_test_connection: bool,
    /// Custom headers sent with every request (programmatic only).
    pub headers: Vec<(String, String)>,
    /// `X-*-Source` (default `nodejs-client`, as the Node client sends).
    pub source: String,
    /// Poll interval while the query is queued/running (800 ms).
    pub check_interval: Duration,
    /// Export bucket.
    pub export_bucket: PrestoExportBucket,
}

/// Reads data-source aware `CUBEJS_*` variables (`getEnv(…, { dataSource, preAggregations })`).
pub(crate) struct DsEnv<'a> {
    env: &'a dyn EnvSource,
    declared: Vec<String>,
    data_source: String,
    pre_aggregations: bool,
}

impl<'a> DsEnv<'a> {
    pub(crate) fn new(env: &'a dyn EnvSource, driver: &DriverConfig) -> Self {
        Self {
            env,
            declared: data_sources(env),
            data_source: driver.data_source.data_source.clone(),
            pre_aggregations: driver.data_source.pre_aggregations,
        }
    }

    /// The key without the pre-aggregations prefix (used in Node's messages).
    pub(crate) fn display_key(&self, origin: &str) -> String {
        env_key(origin, &self.declared, Some(&self.data_source), false)
            .unwrap_or_else(|_| origin.to_string())
    }

    pub(crate) fn get(&self, origin: &str) -> Result<Option<String>> {
        let key = env_key(
            origin,
            &self.declared,
            Some(&self.data_source),
            self.pre_aggregations,
        )?;
        Ok(self.env.get(&key).filter(|v| !v.is_empty()))
    }

    /// Strict `true` / `false` (`default('false')`).
    pub(crate) fn get_bool(&self, origin: &str) -> Result<Option<bool>> {
        match self.get(origin)? {
            None => Ok(None),
            Some(v) => match v.to_lowercase().as_str() {
                "true" => Ok(Some(true)),
                "false" => Ok(Some(false)),
                _ => Err(DriverError::Config(format!(
                    "The {} must be either 'true' or 'false'.",
                    self.display_key(origin)
                ))),
            },
        }
    }
}

impl PrestoConfig {
    /// Builds the configuration from the generic `CUBEJS_DB_*` settings.
    pub fn from_driver_config(driver: DriverConfig, engine: Engine) -> Self {
        let ds = &driver.data_source;
        Self {
            engine,
            host: ds
                .host
                .clone()
                .filter(|h| !h.is_empty())
                .unwrap_or_else(|| DEFAULT_HOST.to_string()),
            port: ds.port.unwrap_or(DEFAULT_PORT),
            catalog: None,
            schema: ds
                .database
                .clone()
                .or_else(|| ds.schema.clone())
                .filter(|s| !s.is_empty()),
            user: ds.user.clone(),
            password: ds.password.clone(),
            auth_token: None,
            ssl: ds.ssl.clone(),
            query_timeout: ds.query_timeout,
            use_select_test_connection: false,
            headers: Vec::new(),
            source: client::DEFAULT_SOURCE.to_string(),
            check_interval: client::DEFAULT_CHECK_INTERVAL,
            export_bucket: PrestoExportBucket::default(),
            driver,
        }
    }

    /// Reads the Presto specific variables from the process environment.
    pub fn apply_env(&mut self) -> Result<()> {
        self.apply_env_source(&ProcessEnv)
    }

    /// Reads `CUBEJS_DB_PRESTO_*`, `CUBEJS_DB_CATALOG`,
    /// `CUBEJS_DB_USE_SELECT_TEST_CONNECTION` and the export bucket variables.
    pub fn apply_env_source(&mut self, env: &dyn EnvSource) -> Result<()> {
        let e = DsEnv::new(env, &self.driver);
        self.catalog = match e.get("CUBEJS_DB_PRESTO_CATALOG")? {
            Some(c) => Some(c),
            None => {
                let c = e.get("CUBEJS_DB_CATALOG")?;
                if c.is_some() {
                    log::warn!(
                        "The CUBEJS_DB_CATALOG is deprecated. Please, use the CUBEJS_DB_PRESTO_CATALOG instead."
                    );
                }
                c
            }
        };
        self.auth_token = e.get("CUBEJS_DB_PRESTO_AUTH_TOKEN")?;
        self.use_select_test_connection = e
            .get_bool("CUBEJS_DB_USE_SELECT_TEST_CONNECTION")?
            .unwrap_or(false);

        let bucket_type = e.get("CUBEJS_DB_EXPORT_BUCKET_TYPE")?;
        if let Some(t) = &bucket_type {
            if !SUPPORTED_BUCKET_TYPES.contains(&t.as_str()) {
                return Err(DriverError::Config(format!(
                    "The {} must be one of the [{}].",
                    e.display_key("CUBEJS_DB_EXPORT_BUCKET_TYPE"),
                    SUPPORTED_BUCKET_TYPES.join(", ")
                )));
            }
        }
        let gcs_credentials = match e.get("CUBEJS_DB_EXPORT_GCS_CREDENTIALS")? {
            None => None,
            Some(encoded) => {
                use base64::Engine as _;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(encoded.trim())
                    .map_err(|err| {
                        DriverError::Config(format!(
                            "CUBEJS_DB_EXPORT_GCS_CREDENTIALS is not valid base64 data: {err}"
                        ))
                    })?;
                let json = String::from_utf8(bytes).map_err(|err| {
                    DriverError::Config(format!(
                        "CUBEJS_DB_EXPORT_GCS_CREDENTIALS is not valid UTF-8: {err}"
                    ))
                })?;
                serde_json::from_str::<Value>(&json).map_err(|err| {
                    DriverError::Config(format!(
                        "CUBEJS_DB_EXPORT_GCS_CREDENTIALS is not valid JSON: {err}"
                    ))
                })?;
                Some(json)
            }
        };
        self.export_bucket = PrestoExportBucket {
            bucket_type,
            export_bucket: e.get("CUBEJS_DB_EXPORT_BUCKET")?,
            access_key_id: e.get("CUBEJS_DB_EXPORT_BUCKET_AWS_KEY")?,
            secret_access_key: e.get("CUBEJS_DB_EXPORT_BUCKET_AWS_SECRET")?,
            region: e.get("CUBEJS_DB_EXPORT_BUCKET_AWS_REGION")?,
            gcs_credentials,
            ..std::mem::take(&mut self.export_bucket)
        };
        Ok(())
    }

    /// Reads everything from the process environment.
    pub fn from_env(data_source: Option<&str>, engine: Engine) -> Result<Self> {
        let mut config = Self::from_driver_config(DriverConfig::from_env(data_source)?, engine);
        config.apply_env()?;
        Ok(config)
    }

    /// `Authorization` header value: bearer token or basic auth.
    pub fn authorization(&self) -> Result<Option<String>> {
        if self.auth_token.is_some() && self.password.is_some() {
            return Err(DriverError::Config(
                "Both user/password and auth token are set. Please remove password or token."
                    .to_string(),
            ));
        }
        if let Some(token) = &self.auth_token {
            return Ok(Some(format!("Bearer {token}")));
        }
        if let Some(password) = &self.password {
            use base64::Engine as _;
            let user = self.user.clone().unwrap_or_default();
            return Ok(Some(format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"))
            )));
        }
        Ok(None)
    }

    /// Options of the protocol client.
    pub fn client_options(&self) -> Result<client::ClientOptions> {
        Ok(client::ClientOptions {
            engine: self.engine,
            host: self.host.clone(),
            port: self.port,
            user: self
                .user
                .clone()
                .or_else(|| std::env::var("USER").ok().filter(|u| !u.is_empty())),
            authorization: self.authorization()?,
            catalog: self.catalog.clone(),
            source: self.source.clone(),
            ssl: self.ssl.clone(),
            headers: self.headers.clone(),
            check_interval: self.check_interval,
            timeout: Some(self.query_timeout).filter(|t| !t.is_zero()),
        })
    }
}

/// Presto (and, with [`Engine::Trino`], Trino) driver.
pub struct PrestoDriver {
    config: PrestoConfig,
    client: StatementClient,
}

impl std::fmt::Debug for PrestoDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrestoDriver")
            .field("engine", &self.config.engine)
            .field("host", &self.config.host)
            .field("port", &self.config.port)
            .field("catalog", &self.config.catalog)
            .finish()
    }
}

impl PrestoDriver {
    /// Creates the driver. Nothing is sent until the first query.
    pub fn new(config: PrestoConfig) -> Result<Self> {
        let what = match config.engine {
            Engine::Presto => "Presto",
            Engine::Trino => "Trino",
        };
        let client = StatementClient::new(config.client_options()?, what)?;
        Ok(Self { config, client })
    }

    /// Creates the driver from `CUBEJS_DB_*`.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Self::new(PrestoConfig::from_env(data_source, Engine::Presto)?)
    }

    /// The driver configuration.
    pub fn presto_config(&self) -> &PrestoConfig {
        &self.config
    }

    /// The protocol client.
    pub fn client(&self) -> &StatementClient {
        &self.client
    }

    /// `prepareQueryWithParams`: ANSI client-side interpolation.
    pub fn prepare_query_with_params(&self, query: &str, values: &[Value]) -> String {
        format_ansi(query, values)
    }

    fn catalog_prefix(&self) -> String {
        match &self.config.catalog {
            Some(c) => format!("{c}."),
            None => String::new(),
        }
    }

    fn statement_options(&self, streaming: bool) -> StatementOptions {
        StatementOptions {
            schema: Some(
                self.config
                    .schema
                    .clone()
                    .unwrap_or_else(|| "default".to_string()),
            ),
            session: if streaming && !self.config.query_timeout.is_zero() {
                Some(format!(
                    "query_max_run_time={}s",
                    self.config.query_timeout.as_secs()
                ))
            } else {
                None
            },
        }
    }

    fn to_columns(&self, columns: &[PrestoColumn]) -> Vec<Column> {
        columns
            .iter()
            .map(|c| Column::new(c.name.clone(), self.to_generic_type(&c.type_, None, None)))
            .collect()
    }

    /// `queryPromised(query, false)`: runs `sql` (already interpolated) and
    /// collects every page.
    pub async fn run(&self, sql: &str) -> Result<QueryResult> {
        let mut statement = self
            .client
            .execute(sql, &self.statement_options(false))
            .await?;
        let mut rows: Vec<Row> = Vec::new();
        while let Some(page) = statement.next_page().await? {
            rows.extend(page.data);
        }
        let columns = statement
            .columns()
            .map(|c| self.to_columns(c))
            .unwrap_or_default();
        Ok(QueryResult::new(columns, rows))
    }

    /// `queryPromised(query, true)`: resolves once the columns are known and
    /// streams the rows with back-pressure (`high_water_mark` rows). Dropping
    /// the stream cancels the query on the server.
    pub async fn run_stream(&self, sql: &str, high_water_mark: usize) -> Result<StreamTableData> {
        let mut statement = self
            .client
            .execute(sql, &self.statement_options(true))
            .await?;
        let mut buffered: Vec<Row> = Vec::new();
        let mut done = false;
        while statement.columns().is_none() {
            match statement.next_page().await? {
                Some(page) => buffered.extend(page.data),
                None => {
                    done = true;
                    break;
                }
            }
        }
        let columns = statement
            .columns()
            .map(|c| self.to_columns(c))
            .unwrap_or_default();

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Row>>(high_water_mark.max(1));
        tokio::spawn(async move {
            for row in buffered {
                if tx.send(Ok(row)).await.is_err() {
                    return;
                }
            }
            if done {
                return;
            }
            loop {
                match statement.next_page().await {
                    Ok(Some(page)) => {
                        for row in page.data {
                            if tx.send(Ok(row)).await.is_err() {
                                // Consumer gone: dropping `statement` cancels it.
                                return;
                            }
                        }
                    }
                    Ok(None) => return,
                    Err(e) => {
                        let _ = tx.send(Err(e)).await;
                        return;
                    }
                }
            }
        });
        let rows = futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        })
        .boxed();
        Ok(StreamTableData { columns, rows })
    }

    /// `testConnectionViaSelect`.
    pub async fn test_connection_via_select(&self) -> Result<()> {
        self.run("SELECT 1").await.map(|_| ())
    }

    fn require_export_bucket(&self) -> Result<&str> {
        self.config
            .export_bucket
            .export_bucket
            .as_deref()
            .ok_or_else(|| DriverError::Config("Export bucket is not configured.".to_string()))
    }

    fn bucket_type(&self) -> Result<&str> {
        match self.config.export_bucket.bucket_type.as_deref() {
            Some(t) if SUPPORTED_BUCKET_TYPES.contains(&t) => Ok(t),
            other => Err(DriverError::Config(format!(
                "Unsupported export bucket type: {}",
                other.unwrap_or("undefined")
            ))),
        }
    }

    /// `generateTableColumnsForExport`.
    pub fn table_columns_for_export(types: &[Column]) -> String {
        types
            .iter()
            .map(|c| format!("CAST({0} AS varchar) {0}", c.name))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// `external_location` of the unload of `schema.table`.
    pub fn external_location(&self, schema: &str, table: &str) -> Result<String> {
        let bucket = self.require_export_bucket()?;
        let protocol = match self.bucket_type()? {
            "s3" if self.config.export_bucket.s3_advanced_fs => "s3a",
            "s3" => "s3",
            _ => "gs",
        };
        Ok(format!("{protocol}://{bucket}/{schema}/{table}"))
    }

    /// The `CREATE TABLE … WITH (external_location …) AS` statement of `unloadGeneric`.
    pub fn unload_create_table_sql(
        &self,
        table_full_name: &str,
        types: &[Column],
        from_sql: &str,
    ) -> Result<(String, String)> {
        let (schema, table) = split_table_full_name(table_full_name);
        let target = format!(
            "{}.{schema}.{table}",
            self.config.catalog.as_deref().unwrap_or("undefined")
        );
        let location = self.external_location(&schema, &table)?;
        let with_params = format!("( external_location = '{location}', format = 'CSV')");
        let select = format!(
            "SELECT {} FROM ({from_sql})",
            Self::table_columns_for_export(types)
        );
        Ok((
            format!("CREATE TABLE {target} WITH {with_params} AS ({select})"),
            format!("DROP TABLE IF EXISTS {target}"),
        ))
    }

    async fn unload_generic(
        &self,
        table_full_name: &str,
        type_sql: &str,
        type_params: &[Value],
        from_sql: &str,
        from_params: &[Value],
    ) -> Result<TableStructure> {
        self.require_export_bucket()?;
        let types = self
            .query_column_types(type_sql, type_params, &QueryOptions::default())
            .await?;
        let (create, drop) = self.unload_create_table_sql(table_full_name, &types, from_sql)?;
        let created = self
            .query(&create, from_params, &QueryOptions::default())
            .await;
        let dropped = self.query(&drop, &[], &QueryOptions::default()).await;
        created?;
        dropped?;
        Ok(types)
    }

    /// `getCsvFiles`: signed URLs of the unloaded files.
    async fn csv_files(&self, table_full_name: &str) -> Result<Vec<String>> {
        let bucket = self.require_export_bucket()?;
        let (schema, table) = split_table_full_name(table_full_name);
        let eb = &self.config.export_bucket;
        match self.bucket_type()? {
            "gcs" => {
                let account = export_bucket::resolve_gcs_account(eb.gcs_credentials.as_deref())?;
                export_bucket::extract_files_from_gcs(
                    self.client.http(),
                    &account,
                    export_bucket::strip_scheme(bucket),
                    &format!("{schema}/{table}/"),
                )
                .await
            }
            _ => {
                let location = export_bucket::S3Location::resolve(
                    bucket,
                    eb.access_key_id.as_deref(),
                    eb.secret_access_key.as_deref(),
                    eb.region.as_deref(),
                    eb.s3_endpoint.as_deref(),
                )?;
                export_bucket::extract_unloaded_files_from_s3(
                    self.client.http(),
                    &location,
                    &format!("{schema}/{table}/"),
                )
                .await
            }
        }
    }
}

/// `splitTableFullName`: `schema.table` (anything after a second dot is dropped).
pub fn split_table_full_name(name: &str) -> (String, String) {
    let mut parts = name.split('.');
    let schema = parts.next().unwrap_or("").to_string();
    let table = parts.next().unwrap_or("undefined").to_string();
    (schema, table)
}

#[async_trait]
impl Driver for PrestoDriver {
    fn config(&self) -> &DriverConfig {
        &self.config.driver
    }

    /// `GET /v1/node` (`client.nodes`), or `SELECT 1` with
    /// `CUBEJS_DB_USE_SELECT_TEST_CONNECTION`. The Trino driver overrides it.
    async fn test_connection(&self) -> Result<()> {
        if self.config.use_select_test_connection {
            return self.test_connection_via_select().await;
        }
        self.client.nodes().await.map(|_| ())
    }

    async fn query(
        &self,
        sql: &str,
        params: &[Value],
        _options: &QueryOptions,
    ) -> Result<QueryResult> {
        self.run(&self.prepare_query_with_params(sql, params)).await
    }

    fn capabilities(&self) -> DriverCapabilities {
        DriverCapabilities {
            unload_without_temp_table: true,
            ..Default::default()
        }
    }

    fn information_schema_query(&self) -> String {
        let schema_filter = match &self.config.schema {
            Some(s) => format!(" AND columns.table_schema = '{s}'"),
            None => String::new(),
        };
        format!(
            r#"
      SELECT columns.column_name as {},
             columns.table_name as {},
             columns.table_schema as {},
             columns.data_type as {}
      FROM {}information_schema.columns
      WHERE columns.table_schema NOT IN ('pg_catalog', 'information_schema', 'mysql', 'performance_schema', 'sys', 'INFORMATION_SCHEMA'){schema_filter}
   "#,
            self.quote_identifier("column_name"),
            self.quote_identifier("table_name"),
            self.quote_identifier("table_schema"),
            self.quote_identifier("data_type"),
            self.catalog_prefix(),
        )
    }

    fn get_schemas_query(&self) -> String {
        format!(
            r#"
      SELECT table_schema as {}
      FROM {}information_schema.tables
      WHERE table_schema NOT IN ('pg_catalog', 'information_schema', 'mysql', 'performance_schema', 'sys', 'INFORMATION_SCHEMA')
      GROUP BY table_schema
    "#,
            self.quote_identifier("schema_name"),
            self.catalog_prefix(),
        )
    }

    fn get_tables_for_specific_schemas_query(&self, schemas_placeholders: &str) -> String {
        format!(
            r#"
      SELECT table_schema as {},
            table_name as {}
      FROM {}information_schema.tables as columns
      WHERE table_schema IN ({schemas_placeholders})
    "#,
            self.quote_identifier("schema_name"),
            self.quote_identifier("table_name"),
            self.catalog_prefix(),
        )
    }

    fn get_columns_for_specific_tables_query(&self, condition_string: &str) -> String {
        format!(
            r#"
      SELECT columns.column_name as {},
             columns.table_name as {},
             columns.table_schema as {},
             columns.data_type as {}
      FROM {}information_schema.columns as columns
      WHERE {condition_string}
    "#,
            self.quote_identifier("column_name"),
            self.quote_identifier("table_name"),
            self.quote_identifier("schema_name"),
            self.quote_identifier("data_type"),
            self.catalog_prefix(),
        )
    }

    /// `CREATE SCHEMA IF NOT EXISTS <catalog>.<schema>`.
    async fn create_schema_if_not_exists(&self, schema_name: &str) -> Result<()> {
        let catalog = self.config.catalog.as_deref().ok_or_else(|| {
            DriverError::Config(
                "Catalog not specified; catalog is required if schema is specified".to_string(),
            )
        })?;
        self.query(
            &format!("CREATE SCHEMA IF NOT EXISTS {catalog}.{schema_name}"),
            &[],
            &QueryOptions::default(),
        )
        .await?;
        Ok(())
    }

    async fn stream(
        &self,
        sql: &str,
        params: &[Value],
        options: &StreamOptions,
    ) -> Result<StreamTableData> {
        self.run_stream(
            &self.prepare_query_with_params(sql, params),
            options.high_water_mark,
        )
        .await
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
        let mut result = self.query(sql, params, &QueryOptions::default()).await?;
        result.columns = detect_types_from_tabular(&result)?;
        Ok(DownloadedData::Memory(result))
    }

    /// `queryColumnTypes`: `<sql> LIMIT 0` streamed for its columns.
    async fn query_column_types(
        &self,
        sql: &str,
        params: &[Value],
        _options: &QueryOptions,
    ) -> Result<TableStructure> {
        let stream = self
            .stream(
                &format!("{sql} LIMIT 0"),
                params,
                &StreamOptions {
                    high_water_mark: 1,
                    request_id: None,
                },
            )
            .await?;
        Ok(stream.columns)
    }

    async fn is_unload_supported(&self, _options: &UnloadOptions) -> Result<bool> {
        Ok(self.config.export_bucket.export_bucket.is_some())
    }

    async fn unload(&self, table: &str, options: &UnloadOptions) -> Result<TableCsvData> {
        self.require_export_bucket()?;
        self.bucket_type()?;
        let types = match &options.query {
            Some(q) => {
                self.unload_generic(table, &q.sql, &q.params, &q.sql, &q.params)
                    .await?
            }
            None => {
                self.unload_generic(table, &format!("SELECT * FROM {table}"), &[], table, &[])
                    .await?
            }
        };
        let csv_file = self.csv_files(table).await?;
        Ok(TableCsvData {
            csv_file,
            types: Some(types),
            csv_no_header: true,
            export_bucket_csv_escape_symbol: self.config.export_bucket.csv_escape_symbol.clone(),
            ..Default::default()
        })
    }
}

#[cfg(test)]
pub(crate) mod mock_http;
#[cfg(test)]
mod tests;
