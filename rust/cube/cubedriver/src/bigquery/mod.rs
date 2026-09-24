//! BigQuery driver: port of `@cubejs-backend/bigquery-driver` on top of the
//! BigQuery REST API (`reqwest` with rustls).
//!
//! The Node driver delegates to `@google-cloud/bigquery`, which is itself a
//! thin REST client: it signs a service-account JWT, exchanges it for an
//! access token and then drives `jobs.insert` / `jobs.get` /
//! `jobs.getQueryResults`. This port speaks the same endpoints directly, so no
//! Google SDK (and no C toolchain) is pulled into the build.
//!
//! Not supported yet: export-bucket unloads (`CUBEJS_DB_EXPORT_BUCKET`), which
//! need a Cloud Storage client and signed URLs.

pub mod auth;
pub mod rest;

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};

use crate::config::DriverConfig;
use crate::driver::{information_columns_to_structure, Driver};
use crate::error::{DriverError, Result};
use crate::types::{
    Column, DatabaseStructure, DownloadQueryResultsOptions, DownloadedData, DriverCapabilities,
    GenericType, QueryOptions, QueryResult, Row, SchemaName, SchemaTable, StreamOptions,
    StreamTableData, TableStructure, UnloadOptions,
};

pub use auth::{ServiceAccount, TokenProvider};
pub use rest::{quote_identifier, to_generic_type};

/// `getDefaultConcurrency`.
pub const DEFAULT_CONCURRENCY: usize = 10;
/// Rows requested per `getQueryResults` page.
pub const PAGE_SIZE: u64 = 100_000;

/// Configuration of [`BigQueryDriver`] (`BigQueryDriverOptions`).
#[derive(Debug, Clone)]
pub struct BigQueryConfig {
    /// Connection settings and global driver knobs.
    pub driver: DriverConfig,
    /// `CUBEJS_DB_BQ_PROJECT_ID`.
    pub project_id: Option<String>,
    /// `CUBEJS_DB_BQ_CREDENTIALS` (base64 JSON) / `CUBEJS_DB_BQ_KEY_FILE`.
    pub credentials: Option<ServiceAccount>,
    /// `CUBEJS_DB_BQ_LOCATION`.
    pub location: Option<String>,
    /// `CUBEJS_DB_EXPORT_BUCKET` / `CUBEJS_DB_BQ_EXPORT_BUCKET` (unload is not
    /// implemented yet; the value is kept so that the configuration round-trips).
    pub export_bucket: Option<String>,
    /// `pollTimeout` (`CUBEJS_DB_POLL_TIMEOUT` or `CUBEJS_DB_QUERY_TIMEOUT`).
    pub poll_timeout: Duration,
    /// `pollMaxInterval` (`CUBEJS_DB_POLL_MAX_INTERVAL`).
    pub poll_max_interval: Duration,
    /// `readOnly` (default `false`).
    pub read_only: bool,
}

impl BigQueryConfig {
    /// Builds the BigQuery configuration from a generic [`DriverConfig`].
    pub fn from_driver_config(driver: DriverConfig) -> Self {
        let ds = &driver.data_source;
        let poll_timeout = ds.poll_timeout.unwrap_or(ds.query_timeout);
        let poll_max_interval = ds.poll_max_interval;
        Self {
            project_id: None,
            credentials: None,
            location: None,
            export_bucket: None,
            poll_timeout,
            poll_max_interval,
            read_only: false,
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

    /// Reads the `CUBEJS_DB_BQ_*` variables into an existing configuration.
    pub fn apply_env(&mut self) -> Result<()> {
        let config = self;
        config.project_id = env_var("CUBEJS_DB_BQ_PROJECT_ID");
        config.location = env_var("CUBEJS_DB_BQ_LOCATION");
        config.export_bucket =
            env_var("CUBEJS_DB_EXPORT_BUCKET").or_else(|| env_var("CUBEJS_DB_BQ_EXPORT_BUCKET"));
        config.credentials = match env_var("CUBEJS_DB_BQ_CREDENTIALS") {
            Some(encoded) => Some(ServiceAccount::from_base64(&encoded)?),
            None => match env_var("CUBEJS_DB_BQ_KEY_FILE") {
                Some(path) => Some(ServiceAccount::from_key_file(&path)?),
                None => None,
            },
        };
        Ok(())
    }

    /// Effective project: the configured one, else the one of the key file.
    pub fn effective_project_id(&self) -> Option<String> {
        self.project_id
            .clone()
            .or_else(|| self.credentials.as_ref().and_then(|c| c.project_id.clone()))
    }
}

fn env_var(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// BigQuery driver.
pub struct BigQueryDriver {
    config: BigQueryConfig,
    client: reqwest::Client,
    token: Option<Arc<TokenProvider>>,
    project_id: Option<String>,
}

impl std::fmt::Debug for BigQueryDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BigQueryDriver")
            .field("project_id", &self.project_id)
            .field("location", &self.config.location)
            .finish()
    }
}

impl BigQueryDriver {
    /// Creates the driver. No request is made until the first query, so a
    /// driver without credentials can be constructed (and fails on use).
    pub fn new(config: BigQueryConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(format!("CubeDev_Cube/{}", env!("CARGO_PKG_VERSION")))
            .timeout(config.poll_timeout.max(Duration::from_secs(60)))
            .build()
            .map_err(|e| DriverError::Config(format!("Unable to build the HTTP client: {e}")))?;
        let token = config
            .credentials
            .clone()
            .map(|account| Arc::new(TokenProvider::new(account, client.clone())));
        let project_id = config.effective_project_id();
        Ok(Self {
            config,
            client,
            token,
            project_id,
        })
    }

    /// Creates the driver from `CUBEJS_DB_*` / `CUBEJS_DB_BQ_*`.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Self::new(BigQueryConfig::from_env(data_source)?)
    }

    /// The driver configuration.
    pub fn bigquery_config(&self) -> &BigQueryConfig {
        &self.config
    }

    fn project(&self) -> Result<&str> {
        self.project_id.as_deref().ok_or_else(|| {
            DriverError::Config(
                "BigQuery project is not configured: set CUBEJS_DB_BQ_PROJECT_ID or use a key file \
                 that carries a project_id."
                    .to_string(),
            )
        })
    }

    fn token_provider(&self) -> Result<&TokenProvider> {
        self.token.as_deref().ok_or_else(|| {
            DriverError::Config(
                "BigQuery credentials are not configured: set CUBEJS_DB_BQ_CREDENTIALS \
                 (base64 encoded service account JSON) or CUBEJS_DB_BQ_KEY_FILE."
                    .to_string(),
            )
        })
    }

    /// `https://bigquery.googleapis.com/bigquery/v2/projects/<project><path>`.
    fn url(&self, path: &str) -> Result<String> {
        Ok(format!(
            "{}/projects/{}{path}",
            rest::API_BASE,
            self.project()?
        ))
    }

    async fn send<T: DeserializeOwned>(&self, request: reqwest::RequestBuilder) -> Result<T> {
        let token = self.token_provider()?.access_token().await?;
        let response =
            request
                .bearer_auth(token)
                .send()
                .await
                .map_err(|e| DriverError::Connection {
                    pool_name: "bigquery".to_string(),
                    message: e.to_string(),
                })?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| DriverError::Query(e.to_string()))?;
        if !status.is_success() {
            return Err(rest::api_error(status, &body));
        }
        rest::parse_json(&body)
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> Result<T> {
        let mut request = self.client.get(self.url(path)?);
        for (key, value) in query {
            request = request.query(&[(key, value)]);
        }
        self.send(request).await
    }

    async fn post<T: DeserializeOwned, B: Serialize>(&self, path: &str, body: &B) -> Result<T> {
        self.send(self.client.post(self.url(path)?).json(body))
            .await
    }

    /// `buildQueryLabels`.
    fn query_labels(&self, request_id: Option<&str>) -> Option<serde_json::Map<String, Value>> {
        let request_id = request_id?;
        let uuid = match request_id.find("-span-") {
            Some(idx) => &request_id[..idx],
            None => request_id,
        };
        let value: String = uuid
            .to_lowercase()
            .chars()
            .map(|c| {
                if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .take(63)
            .collect();
        if value.is_empty() {
            return None;
        }
        let mut labels = serde_json::Map::new();
        labels.insert("cube_request_id".to_string(), Value::String(value));
        Some(labels)
    }

    /// `createQueryJob`: submits the job and returns its id and location.
    async fn create_query_job(
        &self,
        query: &str,
        params: &[Value],
        options: &QueryOptions,
        extra_query_config: Value,
    ) -> Result<(String, Option<String>)> {
        let mut query_config = json!({
            "query": query,
            "useLegacySql": false,
            "parameterMode": "positional",
        });
        if !params.is_empty() {
            query_config["queryParameters"] = Value::Array(
                params
                    .iter()
                    .map(|p| {
                        serde_json::to_value(rest::to_query_parameter(p)).unwrap_or(Value::Null)
                    })
                    .collect(),
            );
        }
        if let Value::Object(extra) = extra_query_config {
            if let Value::Object(target) = &mut query_config {
                target.extend(extra);
            }
        }

        let mut configuration = json!({ "query": query_config });
        if let Some(labels) = self.query_labels(options.request_id.as_deref()) {
            configuration["labels"] = Value::Object(labels);
        }
        let mut body = json!({ "configuration": configuration });
        if let Some(location) = &self.config.location {
            body["jobReference"] = json!({
                "projectId": self.project()?,
                "location": location,
            });
        }

        let job: rest::Job = self.post("/jobs", &body).await?;
        let reference = job.job_reference.unwrap_or_default();
        let job_id = reference
            .job_id
            .ok_or_else(|| DriverError::Query("BigQuery did not return a job id".to_string()))?;
        Ok((job_id, reference.location.or(self.config.location.clone())))
    }

    /// `waitForJobResult`: polls `jobs.get` until the job is `DONE`.
    async fn wait_for_job(&self, job_id: &str, location: Option<&str>) -> Result<()> {
        let started = Instant::now();
        let mut i = 0u32;
        while started.elapsed() <= self.config.poll_timeout {
            let job: rest::Job = self
                .get(&format!("/jobs/{job_id}"), &location_query(location))
                .await?;
            let status = job.status.unwrap_or_default();
            if status.state.as_deref() == Some("DONE") {
                if let Some(error) = status.error_result {
                    return Err(DriverError::Database {
                        message: error.message.unwrap_or_else(|| {
                            error.reason.unwrap_or_else(|| "Unknown error".to_string())
                        }),
                        code: None,
                    });
                }
                return Ok(());
            }
            let pause =
                Duration::from_millis(200 * u64::from(i)).min(self.config.poll_max_interval);
            tokio::time::sleep(pause).await;
            i += 1;
        }

        // Best effort, exactly like `job.cancel()` in the Node driver.
        let _: Result<Value> = self
            .post(&format!("/jobs/{job_id}/cancel"), &location_body(location))
            .await;
        Err(DriverError::Query(format!(
            "BigQuery job timeout reached {}ms",
            self.config.poll_timeout.as_millis()
        )))
    }

    /// One page of `jobs.getQueryResults`.
    async fn results_page(
        &self,
        job_id: &str,
        location: Option<&str>,
        page_token: Option<&str>,
    ) -> Result<rest::QueryResultsResponse> {
        let mut query = location_query(location);
        query.push(("maxResults", PAGE_SIZE.to_string()));
        if let Some(token) = page_token {
            query.push(("pageToken", token.to_string()));
        }
        self.get(&format!("/queries/{job_id}"), &query).await
    }

    /// Runs a query job and collects every result page.
    async fn run_query(
        &self,
        query: &str,
        params: &[Value],
        options: &QueryOptions,
        extra_query_config: Value,
    ) -> Result<QueryResult> {
        let (job_id, location) = self
            .create_query_job(query, params, options, extra_query_config)
            .await?;
        self.wait_for_job(&job_id, location.as_deref()).await?;

        let mut columns: Vec<Column> = Vec::new();
        let mut fields = Vec::new();
        let mut rows: Vec<Row> = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let page = self
                .results_page(&job_id, location.as_deref(), page_token.as_deref())
                .await?;
            if let Some(schema) = &page.schema {
                if columns.is_empty() {
                    columns =
                        rest::schema_to_columns(schema, &|t, p, s| self.to_generic_type(t, p, s));
                    fields = schema.fields.clone();
                }
            }
            rows.extend(page.rows.iter().map(|r| rest::convert_row(r, &fields)));
            match page.page_token {
                Some(token) if !token.is_empty() => page_token = Some(token),
                _ => break,
            }
        }
        Ok(QueryResult::new(columns, rows))
    }

    /// All datasets of the project (`bigquery.getDatasets`).
    pub async fn datasets(&self) -> Result<Vec<String>> {
        let mut result = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut query = vec![("maxResults", "1000".to_string())];
            if let Some(token) = &page_token {
                query.push(("pageToken", token.clone()));
            }
            let list: rest::DatasetList = self.get("/datasets", &query).await?;
            result.extend(
                list.datasets
                    .into_iter()
                    .filter_map(|d| d.dataset_reference.and_then(|r| r.dataset_id)),
            );
            match list.next_page_token {
                Some(token) if !token.is_empty() => page_token = Some(token),
                _ => break,
            }
        }
        Ok(result)
    }

    /// `loadTablesForDataset`: the information-schema query of one dataset.
    async fn load_tables_for_dataset(&self, dataset: &str) -> Result<DatabaseStructure> {
        let default_dataset = json!({
            "defaultDataset": {
                "datasetId": dataset,
                "projectId": self.project()?,
            }
        });
        let result = self
            .run_query(
                &self.information_schema_query(),
                &[],
                &QueryOptions::default(),
                default_dataset,
            )
            .await;
        match result {
            Ok(data) => Ok(information_columns_to_structure(&data)),
            Err(e)
                if e.to_string()
                    .contains("Permission bigquery.tables.get denied on table") =>
            {
                Ok(DatabaseStructure::new())
            }
            Err(e) => Err(e),
        }
    }

    /// Table schema of `schema.table` (`tables.get`).
    async fn table_metadata(&self, schema: &str, name: &str) -> Result<rest::TableSchema> {
        let metadata: rest::TableMetadata = self
            .get(&format!("/datasets/{schema}/tables/{name}"), &[])
            .await?;
        Ok(metadata.schema.unwrap_or_default())
    }
}

fn location_query(location: Option<&str>) -> Vec<(&'static str, String)> {
    match location {
        Some(location) => vec![("location", location.to_string())],
        None => Vec::new(),
    }
}

fn location_body(location: Option<&str>) -> Value {
    match location {
        Some(location) => json!({ "location": location }),
        None => json!({}),
    }
}

#[async_trait]
impl Driver for BigQueryDriver {
    fn config(&self) -> &DriverConfig {
        &self.config.driver
    }

    /// Listing datasets is free of charge, which is why the Node driver uses
    /// it as the connection test.
    async fn test_connection(&self) -> Result<()> {
        self.datasets().await?;
        Ok(())
    }

    async fn query(
        &self,
        sql: &str,
        params: &[Value],
        options: &QueryOptions,
    ) -> Result<QueryResult> {
        self.run_query(sql, params, options, Value::Null).await
    }

    fn quote_identifier(&self, identifier: &str) -> String {
        rest::quote_identifier(identifier)
    }

    fn to_generic_type(
        &self,
        db_type: &str,
        precision: Option<i64>,
        scale: Option<i64>,
    ) -> GenericType {
        rest::to_generic_type(
            db_type,
            precision,
            scale,
            self.config.driver.precise_decimal_in_cubestore,
        )
    }

    fn read_only(&self) -> bool {
        self.config.read_only
    }

    fn capabilities(&self) -> DriverCapabilities {
        DriverCapabilities {
            incremental_schema_loading: true,
            ..Default::default()
        }
    }

    /// The per-dataset query of `loadTablesForDataset` (run with the dataset
    /// as the default one).
    fn information_schema_query(&self) -> String {
        format!(
            "
        SELECT
          columns.column_name as {},
          columns.table_name as {},
          columns.table_schema as {},
          columns.data_type as {}
        FROM INFORMATION_SCHEMA.COLUMNS
      ",
            self.quote_identifier("column_name"),
            self.quote_identifier("table_name"),
            self.quote_identifier("table_schema"),
            self.quote_identifier("data_type"),
        )
    }

    async fn tables_schema(&self) -> Result<DatabaseStructure> {
        let datasets = self.datasets().await?;
        let mut structure = DatabaseStructure::new();
        for dataset in datasets {
            structure.extend(self.load_tables_for_dataset(&dataset).await?);
        }
        Ok(structure)
    }

    async fn get_schemas(&self) -> Result<Vec<SchemaName>> {
        Ok(self
            .datasets()
            .await?
            .into_iter()
            .map(|schema_name| SchemaName { schema_name })
            .collect())
    }

    async fn get_tables_query(&self, schema_name: &str) -> Result<Vec<String>> {
        let mut result = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut query = vec![("maxResults", "1000".to_string())];
            if let Some(token) = &page_token {
                query.push(("pageToken", token.clone()));
            }
            let list: Result<rest::TableList> = self
                .get(&format!("/datasets/{schema_name}/tables"), &query)
                .await;
            let list = match list {
                Ok(list) => list,
                // `getTablesQuery` swallows "Not found" in the Node driver.
                Err(e) if e.to_string().contains("Not found") => return Ok(result),
                Err(e) => return Err(e),
            };
            result.extend(
                list.tables
                    .into_iter()
                    .filter_map(|t| t.table_reference.and_then(|r| r.table_id)),
            );
            match list.next_page_token {
                Some(token) if !token.is_empty() => page_token = Some(token),
                _ => break,
            }
        }
        Ok(result)
    }

    async fn get_tables_for_specific_schemas(
        &self,
        schemas: &[SchemaName],
    ) -> Result<Vec<SchemaTable>> {
        let mut tables = Vec::new();
        for schema in schemas {
            for table_name in self.get_tables_query(&schema.schema_name).await? {
                tables.push(SchemaTable {
                    schema_name: schema.schema_name.clone(),
                    table_name,
                });
            }
        }
        Ok(tables)
    }

    async fn get_columns_for_specific_tables(
        &self,
        tables: &[SchemaTable],
    ) -> Result<Vec<crate::types::ColumnInfo>> {
        let mut columns = Vec::new();
        for table in tables {
            let types = self
                .table_column_types(&format!("{}.{}", table.schema_name, table.table_name))
                .await?;
            for column in types {
                columns.push(crate::types::ColumnInfo {
                    schema_name: table.schema_name.clone(),
                    table_name: table.table_name.clone(),
                    column_name: column.name,
                    data_type: column.type_.to_string(),
                    attributes: Vec::new(),
                    foreign_keys: Vec::new(),
                });
            }
        }
        Ok(columns)
    }

    async fn table_column_types(&self, table: &str) -> Result<TableStructure> {
        let name = crate::types::TableName::split(table);
        let schema = self
            .table_metadata(&name.schema, name.name.split('.').next().unwrap_or(""))
            .await?;
        Ok(rest::table_schema_to_columns(&schema, &|t, p, s| {
            self.to_generic_type(t, p, s)
        }))
    }

    async fn create_schema_if_not_exists(&self, schema_name: &str) -> Result<()> {
        // `dataset.get({ autoCreate: true })`.
        let exists: Result<Value> = self.get(&format!("/datasets/{schema_name}"), &[]).await;
        if exists.is_ok() {
            return Ok(());
        }
        let mut body = json!({
            "datasetReference": {
                "datasetId": schema_name,
                "projectId": self.project()?,
            }
        });
        if let Some(location) = &self.config.location {
            body["location"] = Value::String(location.clone());
        }
        let _: Value = self.post("/datasets", &body).await?;
        Ok(())
    }

    async fn load_pre_aggregation_into_table(
        &self,
        pre_aggregation_table_name: &str,
        load_sql: &str,
        params: &[Value],
        options: &QueryOptions,
    ) -> Result<QueryResult> {
        let name = crate::types::TableName::split(pre_aggregation_table_name);
        let destination = json!({
            "destinationTable": {
                "projectId": self.project()?,
                "datasetId": name.schema,
                "tableId": name.name,
            },
            "createDisposition": "CREATE_IF_NEEDED",
        });
        self.run_query(load_sql, params, options, destination).await
    }

    async fn stream(
        &self,
        sql: &str,
        params: &[Value],
        _options: &StreamOptions,
    ) -> Result<StreamTableData> {
        let (job_id, location) = self
            .create_query_job(sql, params, &QueryOptions::default(), Value::Null)
            .await?;
        self.wait_for_job(&job_id, location.as_deref()).await?;

        let first = self
            .results_page(&job_id, location.as_deref(), None)
            .await?;
        let schema = first.schema.clone().unwrap_or_default();
        let columns = rest::schema_to_columns(&schema, &|t, p, s| self.to_generic_type(t, p, s));
        let fields = schema.fields.clone();

        // Paging needs the driver's HTTP client and token, so the stream owns
        // its own lightweight copy of both.
        let pager = PageFetcher {
            client: self.client.clone(),
            token: self.token.clone(),
            project: self.project()?.to_string(),
            job_id,
            location,
        };

        let rows = futures::stream::try_unfold(
            (Some(first), pager, fields),
            |(page, pager, fields)| async move {
                let page = match page {
                    Some(page) => page,
                    None => return Ok(None),
                };
                let rows: Vec<Row> = page
                    .rows
                    .iter()
                    .map(|r| rest::convert_row(r, &fields))
                    .collect();
                let next = match page.page_token.filter(|t| !t.is_empty()) {
                    Some(token) => Some(pager.fetch(Some(&token)).await?),
                    None => None,
                };
                Ok(Some((rows, (next, pager, fields))))
            },
        )
        .map(|chunk| {
            futures::stream::iter(match chunk {
                Ok(rows) => rows.into_iter().map(Ok).collect::<Vec<_>>(),
                Err(e) => vec![Err(e)],
            })
        })
        .flatten()
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
        Ok(DownloadedData::Memory(
            self.query(sql, params, &QueryOptions::default()).await?,
        ))
    }

    async fn is_unload_supported(&self, _options: &UnloadOptions) -> Result<bool> {
        // Unloading needs a Cloud Storage client (extract job + signed URLs).
        Ok(false)
    }

    async fn unload(
        &self,
        _table: &str,
        _options: &UnloadOptions,
    ) -> Result<crate::types::TableCsvData> {
        Err(DriverError::NotImplemented(
            "BigQuery unload to an export bucket is not implemented in the Rust driver yet."
                .to_string(),
        ))
    }
}

/// Fetches result pages for [`BigQueryDriver::stream`].
struct PageFetcher {
    client: reqwest::Client,
    token: Option<Arc<TokenProvider>>,
    project: String,
    job_id: String,
    location: Option<String>,
}

impl PageFetcher {
    async fn fetch(&self, page_token: Option<&str>) -> Result<rest::QueryResultsResponse> {
        let token = self
            .token
            .as_ref()
            .ok_or_else(|| DriverError::Config("BigQuery credentials are gone".to_string()))?
            .access_token()
            .await?;
        let mut request = self
            .client
            .get(format!(
                "{}/projects/{}/queries/{}",
                rest::API_BASE,
                self.project,
                self.job_id
            ))
            .query(&[("maxResults", PAGE_SIZE.to_string())]);
        if let Some(location) = &self.location {
            request = request.query(&[("location", location)]);
        }
        if let Some(page_token) = page_token {
            request = request.query(&[("pageToken", page_token)]);
        }
        let response =
            request
                .bearer_auth(token)
                .send()
                .await
                .map_err(|e| DriverError::Connection {
                    pool_name: "bigquery".to_string(),
                    message: e.to_string(),
                })?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| DriverError::Query(e.to_string()))?;
        if !status.is_success() {
            return Err(rest::api_error(status, &body));
        }
        rest::parse_json(&body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DataSourceConfig;

    fn driver() -> BigQueryDriver {
        let config = BigQueryConfig {
            project_id: Some("my-project".to_string()),
            ..BigQueryConfig::from_driver_config(DriverConfig::default())
        };
        BigQueryDriver::new(config).unwrap()
    }

    #[test]
    fn config_defaults_to_the_query_timeout() {
        let driver_config = DriverConfig {
            data_source: DataSourceConfig {
                query_timeout: Duration::from_secs(120),
                poll_max_interval: Duration::from_secs(5),
                ..DataSourceConfig::default()
            },
            ..DriverConfig::default()
        };
        let config = BigQueryConfig::from_driver_config(driver_config);
        assert_eq!(config.poll_timeout, Duration::from_secs(120));
        assert_eq!(config.poll_max_interval, Duration::from_secs(5));

        let mut driver_config = DriverConfig::default();
        driver_config.data_source.poll_timeout = Some(Duration::from_secs(30));
        let config = BigQueryConfig::from_driver_config(driver_config);
        assert_eq!(config.poll_timeout, Duration::from_secs(30));
    }

    #[test]
    fn project_falls_back_to_the_key_file() {
        let mut config = BigQueryConfig::from_driver_config(DriverConfig::default());
        config.credentials = Some(ServiceAccount {
            project_id: Some("from-key".into()),
            client_email: "a@b.com".into(),
            private_key: "x".into(),
            private_key_id: None,
            token_uri: None,
        });
        assert_eq!(config.effective_project_id().as_deref(), Some("from-key"));
        config.project_id = Some("explicit".into());
        assert_eq!(config.effective_project_id().as_deref(), Some("explicit"));
    }

    #[tokio::test]
    async fn sql_and_type_mapping() {
        let driver = driver();
        assert_eq!(driver.param(0), "?");
        assert_eq!(driver.quote_identifier("orders"), "orders");
        assert_eq!(driver.quote_identifier("Orders"), "`Orders`");
        assert_eq!(
            driver.to_generic_type("BIGNUMERIC", None, None),
            GenericType::Decimal(None)
        );
        assert_eq!(
            driver.to_generic_type("STRING", None, None),
            GenericType::Text
        );
        assert!(!driver.read_only());
        assert!(driver.capabilities().incremental_schema_loading);

        let q = driver.information_schema_query();
        assert!(q.contains("columns.column_name as column_name"));
        assert!(q.contains("FROM INFORMATION_SCHEMA.COLUMNS"));
        // no `WHERE`: the query is scoped by the default dataset
        assert!(!q.contains("WHERE"));

        assert_eq!(
            driver.create_table_sql(
                "s.t",
                &[Column::new("a", "int"), Column::new("b", "string")]
            ),
            "CREATE TABLE s.t (a int, b string)"
        );
        assert_eq!(
            driver.wrap_query_with_limit("SELECT 1", 10),
            "SELECT * FROM (SELECT 1) AS t LIMIT 10"
        );
        assert!(driver.primary_keys_query(None).is_none());
        assert!(driver.foreign_keys_query(None).is_none());
    }

    #[test]
    fn query_labels_are_sanitised() {
        let driver = driver();
        let labels = driver
            .query_labels(Some("6A27D4B2-3B3F-4C4C-9D4E-000000000000-span-1"))
            .unwrap();
        assert_eq!(
            labels["cube_request_id"],
            "6a27d4b2-3b3f-4c4c-9d4e-000000000000"
        );
        let labels = driver.query_labels(Some("a b!c")).unwrap();
        assert_eq!(labels["cube_request_id"], "a_b_c");
        assert!(driver.query_labels(None).is_none());
    }

    #[tokio::test]
    async fn fails_without_credentials() {
        let without_project =
            BigQueryDriver::new(BigQueryConfig::from_driver_config(DriverConfig::default()))
                .unwrap();
        let err = without_project.test_connection().await.unwrap_err();
        assert!(err.to_string().contains("CUBEJS_DB_BQ_PROJECT_ID"));

        let driver = driver();
        let err = driver.test_connection().await.unwrap_err();
        assert!(err.to_string().contains("CUBEJS_DB_BQ_CREDENTIALS"));
        assert!(!driver
            .is_unload_supported(&UnloadOptions::default())
            .await
            .unwrap());
    }
}
