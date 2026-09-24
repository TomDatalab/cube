//! Amazon Athena driver: port of `@cubejs-backend/athena-driver` on the
//! official AWS SDK for Rust (`aws-sdk-athena`, `aws-sdk-s3`, rustls).
//!
//! Queries go through `StartQueryExecution`, are polled with
//! `GetQueryExecution` (exponential-ish back-off capped by
//! `CUBEJS_DB_POLL_MAX_INTERVAL`, bounded by `CUBEJS_DB_POLL_TIMEOUT` /
//! `CUBEJS_DB_QUERY_TIMEOUT`) and read with `GetQueryResults` pages. Every cell
//! comes back as a string (Athena's `VarCharValue`), exactly like the Node
//! driver. Parameters are interpolated client side with the ANSI escaper
//! (`formatAnsi`), because `StartQueryExecution` is sent a single SQL text.
//!
//! Cancellation: the Node driver returns cancelable promises that call
//! `StopQueryExecution`. Here a dropped future (or a dropped row stream) stops
//! the running query on the Tokio runtime, best effort.
//!
//! Export bucket (`CUBEJS_DB_EXPORT_BUCKET`): `UNLOAD … TO 's3://…'` as
//! gzip-compressed TEXTFILE, then the files are listed with `ListObjectsV2`
//! and handed over as presigned `GetObject` URLs (one hour), as in
//! `BaseDriver.extractUnloadedFilesFromS3`.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use aws_config::sts::AssumeRoleProvider;
use aws_config::{BehaviorVersion, Region, SdkConfig};
use aws_sdk_athena::config::Credentials;
use aws_sdk_athena::error::{DisplayErrorContext, ProvideErrorMetadata, SdkError};
use aws_sdk_athena::operation::get_query_results::GetQueryResultsOutput;
use aws_sdk_athena::types::{
    ColumnInfo as AthenaColumnInfo, QueryExecutionContext, QueryExecutionState, ResultConfiguration,
};
use futures::StreamExt;
use serde_json::Value;
use tokio::sync::OnceCell;

use crate::config::{self, DataSourceConfig, DriverConfig, EnvSource, ProcessEnv};
use crate::driver::Driver;
use crate::error::{DriverError, Result};
use crate::escape::format_ansi;
use crate::types::{
    Column, DatabaseStructure, DownloadQueryResultsOptions, DownloadedData, DriverCapabilities,
    QueryOptions, QueryResult, Row, SchemaColumn, StreamOptions, StreamTableData, TableCsvData,
    TableStructure, UnloadOptions, UnloadQuery,
};

#[cfg(test)]
mod tests;

/// `AthenaDriver.getDefaultConcurrency()`.
pub const DEFAULT_CONCURRENCY: usize = 10;
/// Default `CUBEJS_AWS_ATHENA_WORKGROUP`.
pub const DEFAULT_WORKGROUP: &str = "primary";
/// Lifetime of the presigned URLs of unloaded files (`expiresIn: 3600`).
pub const PRESIGNED_URL_TTL: Duration = Duration::from_secs(3600);
/// Delimiter Athena writes for `format = 'TEXTFILE'` (Cube Store notation for `\x01`).
pub const UNLOAD_CSV_DELIMITER: &str = "^A";

/// `applyParams`: ANSI client-side interpolation of `?` placeholders.
pub fn apply_params(query: &str, params: &[Value]) -> String {
    format_ansi(query, params)
}

/// `AthenaDriver.normalizeS3Path`: strips trailing `/` and prepends `s3://`.
pub fn normalize_s3_path(path: &str) -> String {
    let path = path.trim_end_matches('/');
    if path.starts_with("s3://") {
        path.to_string()
    } else {
        format!("s3://{path}")
    }
}

/// `AthenaDriver.splitS3Path`: `(bucket, prefix)`, the prefix keeping its
/// leading `/` like `URL.pathname`.
pub fn split_s3_path(path: &str) -> Result<(String, String)> {
    let url = url::Url::parse(path)
        .map_err(|e| DriverError::Config(format!("Invalid S3 path {path}: {e}")))?;
    let bucket = url.host_str().unwrap_or_default().to_string();
    let prefix = url.path().to_string();
    Ok((bucket, prefix))
}

/// Configuration of [`AthenaDriver`] (`AthenaDriverOptions`).
#[derive(Debug, Clone)]
pub struct AthenaConfig {
    /// Shared driver knobs.
    pub driver: DriverConfig,
    /// `CUBEJS_AWS_KEY`.
    pub access_key_id: Option<String>,
    /// `CUBEJS_AWS_SECRET`.
    pub secret_access_key: Option<String>,
    /// `CUBEJS_AWS_REGION` (`None`: the SDK's region chain, `AWS_REGION`, …).
    pub region: Option<String>,
    /// `CUBEJS_AWS_S3_OUTPUT_LOCATION`.
    pub s3_output_location: Option<String>,
    /// `CUBEJS_AWS_ATHENA_WORKGROUP` (default `primary`).
    pub work_group: String,
    /// `CUBEJS_AWS_ATHENA_CATALOG`.
    pub catalog: Option<String>,
    /// `CUBEJS_DB_NAME`: the `QueryExecutionContext.Database`.
    pub database: Option<String>,
    /// `CUBEJS_DB_NAME` or (deprecated) `CUBEJS_DB_SCHEMA`: restricts introspection.
    pub schema: Option<String>,
    /// `CUBEJS_DB_EXPORT_BUCKET`, normalised to `s3://bucket[/prefix]`.
    pub export_bucket: Option<String>,
    /// `CUBEJS_DB_EXPORT_BUCKET_CSV_ESCAPE_SYMBOL`.
    pub export_bucket_csv_escape_symbol: Option<String>,
    /// `pollTimeout`: `CUBEJS_DB_POLL_TIMEOUT`, else `CUBEJS_DB_QUERY_TIMEOUT`.
    pub poll_timeout: Duration,
    /// `pollMaxInterval`: `CUBEJS_DB_POLL_MAX_INTERVAL` (default 5 s).
    pub poll_max_interval: Duration,
    /// `CUBEJS_AWS_ATHENA_ASSUME_ROLE_ARN`.
    pub assume_role_arn: Option<String>,
    /// `CUBEJS_AWS_ATHENA_ASSUME_ROLE_EXTERNAL_ID`.
    pub assume_role_external_id: Option<String>,
    /// `readOnly` option. The Node constructor defaults it to
    /// `!this.isUnloadSupported()`, but that method is `async`, so the
    /// negated promise is always `false`: the effective default is `false`,
    /// which is what this port keeps.
    pub read_only: bool,
    /// Endpoint override for Athena, STS and S3 (tests, VPC endpoints). The
    /// SDK also honours `AWS_ENDPOINT_URL` / `AWS_ENDPOINT_URL_ATHENA` /
    /// `AWS_ENDPOINT_URL_S3` on its own, as the Node SDK does.
    pub endpoint_url: Option<String>,
    /// Separate endpoint override for S3 only (defaults to `endpoint_url`).
    pub s3_endpoint_url: Option<String>,
    /// Use path-style S3 addressing (implied by an endpoint override).
    pub s3_force_path_style: bool,
}

impl AthenaConfig {
    /// Builds the Athena configuration from a generic [`DriverConfig`]
    /// (no Athena specific variables read yet; see [`AthenaConfig::apply_env`]).
    pub fn from_driver_config(driver: DriverConfig) -> Self {
        let ds = &driver.data_source;
        let poll_timeout = ds.poll_timeout.unwrap_or(ds.query_timeout);
        let poll_max_interval = ds.poll_max_interval;
        let database = ds.database.clone();
        let schema = ds.database.clone().or_else(|| ds.schema.clone());
        let export_bucket_csv_escape_symbol = ds.export_bucket_csv_escape_symbol.clone();
        Self {
            access_key_id: None,
            secret_access_key: None,
            region: None,
            s3_output_location: None,
            work_group: DEFAULT_WORKGROUP.to_string(),
            catalog: None,
            database,
            schema,
            export_bucket: None,
            export_bucket_csv_escape_symbol,
            poll_timeout,
            poll_max_interval,
            assume_role_arn: None,
            assume_role_external_id: None,
            read_only: false,
            endpoint_url: None,
            s3_endpoint_url: None,
            s3_force_path_style: false,
            driver,
        }
    }

    /// Reads the configuration of `data_source` from the process environment.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        let mut config = Self::from_driver_config(DriverConfig::from_env(data_source)?);
        config.apply_env()?;
        Ok(config)
    }

    /// Reads the `CUBEJS_AWS_*` / `CUBEJS_DB_EXPORT_BUCKET` variables of this
    /// data source from the process environment.
    pub fn apply_env(&mut self) -> Result<()> {
        self.apply_env_source(&ProcessEnv)
    }

    /// [`AthenaConfig::apply_env`] against an arbitrary [`EnvSource`].
    pub fn apply_env_source(&mut self, env: &dyn EnvSource) -> Result<()> {
        let ds = self.driver.data_source.clone();
        let get = |key: &str| read_env(env, &ds, key);
        self.access_key_id = get("CUBEJS_AWS_KEY")?;
        self.secret_access_key = get("CUBEJS_AWS_SECRET")?;
        self.region = get("CUBEJS_AWS_REGION")?;
        self.s3_output_location = get("CUBEJS_AWS_S3_OUTPUT_LOCATION")?;
        self.work_group =
            get("CUBEJS_AWS_ATHENA_WORKGROUP")?.unwrap_or_else(|| DEFAULT_WORKGROUP.to_string());
        self.catalog = get("CUBEJS_AWS_ATHENA_CATALOG")?;
        self.assume_role_arn = get("CUBEJS_AWS_ATHENA_ASSUME_ROLE_ARN")?;
        self.assume_role_external_id = get("CUBEJS_AWS_ATHENA_ASSUME_ROLE_EXTERNAL_ID")?;
        self.export_bucket = get("CUBEJS_DB_EXPORT_BUCKET")?.map(|b| normalize_s3_path(&b));
        Ok(())
    }
}

/// `getEnv(<key>, { dataSource, preAggregations })`; empty values are unset.
pub(crate) fn read_env(
    env: &dyn EnvSource,
    ds: &DataSourceConfig,
    key: &str,
) -> Result<Option<String>> {
    let declared = config::data_sources(env);
    let data_source = if ds.data_source.is_empty() {
        None
    } else {
        Some(ds.data_source.as_str())
    };
    let key = config::env_key(key, &declared, data_source, ds.pre_aggregations)?;
    Ok(env.get(&key).filter(|v| !v.is_empty()))
}

/// The SDK clients, created on first use (loading the AWS configuration is
/// asynchronous, the driver constructor is not).
#[derive(Clone)]
struct Clients {
    athena: aws_sdk_athena::Client,
    s3: aws_sdk_s3::Client,
}

/// Amazon Athena driver.
pub struct AthenaDriver {
    config: AthenaConfig,
    clients: OnceCell<Clients>,
}

impl std::fmt::Debug for AthenaDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AthenaDriver")
            .field("region", &self.config.region)
            .field("work_group", &self.config.work_group)
            .field("catalog", &self.config.catalog)
            .field("database", &self.config.database)
            .finish()
    }
}

/// Converts an SDK error into a [`DriverError`], keeping the service message
/// (what the Node SDK puts into `Error.message`) and error code.
pub(crate) fn sdk_error<E, R>(err: SdkError<E, R>) -> DriverError
where
    E: ProvideErrorMetadata + std::error::Error + Send + Sync + 'static,
    R: std::fmt::Debug,
{
    match (&err, err.message()) {
        (SdkError::ServiceError(_), Some(message)) => DriverError::Database {
            message: message.to_string(),
            code: err.code().map(str::to_string),
        },
        (SdkError::ServiceError(_), None) => DriverError::Database {
            message: err
                .code()
                .map(str::to_string)
                .unwrap_or_else(|| DisplayErrorContext(&err).to_string()),
            code: err.code().map(str::to_string),
        },
        _ => DriverError::Connection {
            pool_name: "athena".to_string(),
            message: DisplayErrorContext(&err).to_string(),
        },
    }
}

/// Stops a query when dropped while armed: the Rust counterpart of the
/// cancelable promise's `cancel()` → `stopQuery`.
struct StopOnDrop {
    client: aws_sdk_athena::Client,
    query_execution_id: String,
    armed: bool,
}

impl StopOnDrop {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let client = self.client.clone();
            let id = std::mem::take(&mut self.query_execution_id);
            handle.spawn(async move { stop_query(&client, &id).await });
        }
    }
}

/// `stopQuery`: best effort, a failure is logged and never raised.
async fn stop_query(client: &aws_sdk_athena::Client, query_execution_id: &str) {
    if let Err(e) = client
        .stop_query_execution()
        .query_execution_id(query_execution_id)
        .send()
        .await
    {
        log::warn!(
            "Failed to stop Athena query {query_execution_id}: {}",
            sdk_error(e)
        );
    }
}

/// One `GetQueryResults` page.
async fn fetch_page(
    client: &aws_sdk_athena::Client,
    query_execution_id: &str,
    next_token: Option<String>,
) -> Result<GetQueryResultsOutput> {
    client
        .get_query_results()
        .query_execution_id(query_execution_id)
        .set_next_token(next_token)
        .send()
        .await
        .map_err(sdk_error)
}

/// Column names used to key the rows of a result (`lazyRowIterator`):
/// `SHOW COLUMNS` yields one unnamed column, reported as `column`.
fn result_column_names(query: &str, info: &[AthenaColumnInfo]) -> Vec<String> {
    if query.contains("SHOW COLUMNS") {
        vec!["column".to_string()]
    } else {
        info.iter().map(|c| c.name().to_string()).collect()
    }
}

/// Converts one page into rows. On the first page Athena repeats the column
/// names as the first row, which is skipped (`rows.slice(1)`).
fn page_rows(page: &GetQueryResultsOutput, width: usize, first_page: bool) -> Vec<Row> {
    let rows = page.result_set().map(|r| r.rows()).unwrap_or_default();
    let skip = usize::from(first_page);
    rows.iter()
        .skip(skip)
        .map(|row| {
            let data = row.data();
            (0..width)
                .map(|j| {
                    data.get(j)
                        .and_then(|d| d.var_char_value())
                        .map(|v| Value::String(v.to_string()))
                        .unwrap_or(Value::Null)
                })
                .collect()
        })
        .collect()
}

fn page_column_info(page: &GetQueryResultsOutput) -> &[AthenaColumnInfo] {
    page.result_set()
        .and_then(|r| r.result_set_metadata())
        .map(|m| m.column_info())
        .unwrap_or_default()
}

impl AthenaDriver {
    /// Creates the driver. No AWS call is made until the first query.
    pub fn new(config: AthenaConfig) -> Result<Self> {
        Ok(Self {
            config,
            clients: OnceCell::new(),
        })
    }

    /// Creates the driver from `CUBEJS_AWS_*` / `CUBEJS_DB_*`.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Self::new(AthenaConfig::from_env(data_source)?)
    }

    /// The driver configuration.
    pub fn athena_config(&self) -> &AthenaConfig {
        &self.config
    }

    /// Credentials, region and endpoint as the Node constructor sets them up:
    /// assume-role (with the static keys as source credentials when given),
    /// else the static keys, else the SDK's default chain.
    async fn sdk_config(&self) -> SdkConfig {
        let cfg = &self.config;
        let mut loader = aws_config::defaults(BehaviorVersion::latest());
        if let Some(region) = &cfg.region {
            loader = loader.region(Region::new(region.clone()));
        }
        if let Some(endpoint) = &cfg.endpoint_url {
            loader = loader.endpoint_url(endpoint.clone());
        }
        let static_credentials = match (&cfg.access_key_id, &cfg.secret_access_key) {
            (Some(key), Some(secret)) => Some(Credentials::new(
                key.clone(),
                secret.clone(),
                None,
                None,
                "cube-athena-driver",
            )),
            _ => None,
        };
        if let Some(role_arn) = &cfg.assume_role_arn {
            let base = aws_config::defaults(BehaviorVersion::latest());
            let base = match &cfg.region {
                Some(region) => base.region(Region::new(region.clone())),
                None => base,
            };
            let base = match &cfg.endpoint_url {
                Some(endpoint) => base.endpoint_url(endpoint.clone()),
                None => base,
            };
            let base = base.load().await;
            let mut builder = AssumeRoleProvider::builder(role_arn.clone())
                .session_name(format!(
                    "cube-athena-{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis())
                        .unwrap_or(0)
                ))
                .configure(&base);
            if let Some(external_id) = &cfg.assume_role_external_id {
                builder = builder.external_id(external_id.clone());
            }
            let provider = match static_credentials {
                Some(credentials) => builder.build_from_provider(credentials).await,
                None => builder.build().await,
            };
            loader = loader.credentials_provider(provider);
        } else if let Some(credentials) = static_credentials {
            loader = loader.credentials_provider(credentials);
        }
        loader.load().await
    }

    async fn clients(&self) -> Result<&Clients> {
        self.clients
            .get_or_try_init(|| async {
                let sdk_config = self.sdk_config().await;
                let athena = aws_sdk_athena::Client::new(&sdk_config);
                let mut s3_config = aws_sdk_s3::config::Builder::from(&sdk_config)
                    .force_path_style(
                        self.config.s3_force_path_style
                            || self.config.endpoint_url.is_some()
                            || self.config.s3_endpoint_url.is_some(),
                    );
                if let Some(endpoint) = &self.config.s3_endpoint_url {
                    s3_config = s3_config.endpoint_url(endpoint.clone());
                }
                let s3_config = s3_config.build();
                let s3 = aws_sdk_s3::Client::from_conf(s3_config);
                Ok::<_, DriverError>(Clients { athena, s3 })
            })
            .await
    }

    async fn athena(&self) -> Result<&aws_sdk_athena::Client> {
        Ok(&self.clients().await?.athena)
    }

    /// `startQuery`: interpolates the parameters and submits the query.
    async fn start_query(&self, query: &str, params: &[Value]) -> Result<StopOnDrop> {
        let client = self.athena().await?;
        let query_string = apply_params(query, params);
        let mut request = client
            .start_query_execution()
            .query_string(query_string)
            .work_group(self.config.work_group.clone())
            .result_configuration(
                ResultConfiguration::builder()
                    .set_output_location(self.config.s3_output_location.clone())
                    .build(),
            );
        if self.config.catalog.is_some() || self.config.database.is_some() {
            request = request.query_execution_context(
                QueryExecutionContext::builder()
                    .set_catalog(self.config.catalog.clone())
                    .set_database(self.config.database.clone())
                    .build(),
            );
        }
        let output = request.send().await.map_err(sdk_error)?;
        let id = output
            .query_execution_id()
            .ok_or_else(|| DriverError::Query("StartQueryExecution is not defined".to_string()))?;
        Ok(StopOnDrop {
            client: client.clone(),
            query_execution_id: id.to_string(),
            armed: true,
        })
    }

    /// `checkStatus`: `true` once the query has succeeded.
    async fn check_status(&self, query_execution_id: &str) -> Result<bool> {
        let output = self
            .athena()
            .await?
            .get_query_execution()
            .query_execution_id(query_execution_id)
            .send()
            .await
            .map_err(sdk_error)?;
        let status = output.query_execution().and_then(|q| q.status());
        match status.and_then(|s| s.state()) {
            Some(QueryExecutionState::Failed) => Err(DriverError::Database {
                message: status
                    .and_then(|s| s.state_change_reason())
                    .unwrap_or_default()
                    .to_string(),
                code: None,
            }),
            Some(QueryExecutionState::Cancelled) => {
                Err(DriverError::Query("Query has been cancelled".to_string()))
            }
            Some(QueryExecutionState::Succeeded) => Ok(true),
            _ => Ok(false),
        }
    }

    /// `waitForSuccess`: polls with a `500 ms * i` pause capped by
    /// `pollMaxInterval`; stops the query once `pollTimeout` is exceeded.
    async fn wait_for_success(&self, query: &StopOnDrop) -> Result<()> {
        let started = Instant::now();
        let mut i: u32 = 0;
        while started.elapsed() <= self.config.poll_timeout {
            if self.check_status(&query.query_execution_id).await? {
                return Ok(());
            }
            let pause =
                Duration::from_millis(500 * u64::from(i)).min(self.config.poll_max_interval);
            tokio::time::sleep(pause).await;
            i += 1;
        }
        stop_query(self.athena().await?, &query.query_execution_id).await;
        Err(DriverError::Query(format!(
            "Athena job timeout reached {}ms",
            self.config.poll_timeout.as_millis()
        )))
    }

    /// Starts `query`, waits for it and returns the guard that owns its id.
    async fn execute(&self, query: &str, params: &[Value]) -> Result<StopOnDrop> {
        let mut guard = self.start_query(query, params).await?;
        match self.wait_for_success(&guard).await {
            Ok(()) => Ok(guard),
            Err(e) => {
                // Already stopped (timeout) or terminal (failed / cancelled).
                guard.disarm();
                Err(e)
            }
        }
    }

    /// `mapTypes`: Athena column metadata to generic types
    /// (`toGenericType(field.Type || 'text')`).
    pub fn map_types(&self, fields: &[AthenaColumnInfo]) -> TableStructure {
        fields
            .iter()
            .map(|field| {
                let db_type = if field.r#type().is_empty() {
                    "text"
                } else {
                    field.r#type()
                };
                Column::new(field.name(), self.to_generic_type(db_type, None, None))
            })
            .collect()
    }

    /// Runs `query` and reads every result page (`query` / `memory`).
    async fn run(&self, query: &str, params: &[Value]) -> Result<QueryResult> {
        let mut guard = self.execute(query, params).await?;
        let client = self.athena().await?;
        let first = fetch_page(client, &guard.query_execution_id, None).await?;
        let info = page_column_info(&first);
        let types = self.map_types(info);
        let names = result_column_names(query, info);
        let mut rows = page_rows(&first, names.len(), true);
        let mut next = first.next_token().map(str::to_string);
        while let Some(token) = next {
            let page = fetch_page(client, &guard.query_execution_id, Some(token)).await?;
            rows.extend(page_rows(&page, names.len(), false));
            next = page.next_token().map(str::to_string);
        }
        guard.disarm();
        // Rows are keyed by `names` (which differ from the metadata for
        // `SHOW COLUMNS`); the types come from the metadata when it matches.
        let same_width = names.len() == types.len();
        let columns = names
            .into_iter()
            .zip(types.into_iter().map(Some).chain(std::iter::repeat(None)))
            .map(|(name, t)| match t {
                Some(t) if same_width => Column::new(name, t.type_),
                _ => Column::new(name, "text"),
            })
            .collect();
        Ok(QueryResult::new(columns, rows))
    }

    /// `unloadWithSql`.
    async fn unload_with_sql(
        &self,
        table_name: &str,
        query: &UnloadQuery,
    ) -> Result<TableStructure> {
        let columns = self
            .query_column_types(&query.sql, &query.params, &QueryOptions::default())
            .await?;
        let unload_sql = self.unload_sql(&query.sql, table_name);
        let mut guard = self.execute(&unload_sql, &query.params).await?;
        fetch_page(self.athena().await?, &guard.query_execution_id, None).await?;
        guard.disarm();
        Ok(columns)
    }

    /// `unloadWithTable`.
    async fn unload_with_table(&self, table_name: &str) -> Result<TableStructure> {
        let types = self.table_column_types(table_name).await?;
        let columns = types
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let unload_sql =
            self.unload_sql(&format!("SELECT {columns} FROM {table_name}"), table_name);
        let mut guard = self.execute(&unload_sql, &[]).await?;
        guard.disarm();
        Ok(types)
    }

    fn unload_sql(&self, select: &str, table_name: &str) -> String {
        format!(
            "
      UNLOAD ({select})
      TO '{}/{table_name}'
      WITH (
        format = 'TEXTFILE',
        compression='GZIP'
      )",
            self.config.export_bucket.as_deref().unwrap_or_default()
        )
    }

    /// `getCsvFiles`: presigned URLs of every object under
    /// `<export bucket>/<table name>`.
    async fn get_csv_files(&self, table_name: &str) -> Result<Vec<String>> {
        let bucket_path = format!(
            "{}/{table_name}",
            self.config.export_bucket.as_deref().unwrap_or_default()
        );
        let (bucket, prefix) = split_s3_path(&bucket_path)?;
        let prefix = prefix.strip_prefix('/').unwrap_or(&prefix).to_string();
        self.extract_unloaded_files_from_s3(&bucket, &prefix).await
    }

    /// `extractUnloadedFilesFromS3`. Unlike the Node helper, which reads only
    /// the first `ListObjectsV2` page (1000 keys), every page is listed.
    async fn extract_unloaded_files_from_s3(
        &self,
        bucket: &str,
        prefix: &str,
    ) -> Result<Vec<String>> {
        let s3 = &self.clients().await?.s3;
        let bucket = strip_scheme(bucket);
        let mut keys: Vec<String> = Vec::new();
        let mut continuation: Option<String> = None;
        loop {
            let page = s3
                .list_objects_v2()
                .bucket(bucket)
                .prefix(prefix)
                .set_continuation_token(continuation.take())
                .send()
                .await
                .map_err(|e| {
                    s3_error(
                        e,
                        "Unable to retrieve list of files from S3 storage after unloading",
                    )
                })?;
            keys.extend(
                page.contents()
                    .iter()
                    .filter_map(|o| o.key().map(str::to_string)),
            );
            match page.next_continuation_token() {
                Some(token) if page.is_truncated() == Some(true) => {
                    continuation = Some(token.to_string())
                }
                _ => break,
            }
        }
        let presigning = aws_sdk_s3::presigning::PresigningConfig::expires_in(PRESIGNED_URL_TTL)
            .map_err(|e| DriverError::Other(e.to_string()))?;
        let mut urls = Vec::with_capacity(keys.len());
        for key in keys {
            let request = s3
                .get_object()
                .bucket(bucket)
                .key(key)
                .presigned(presigning.clone())
                .await
                .map_err(|e| s3_error(e, "Unable to presign an unloaded file"))?;
            urls.push(request.uri().to_string());
        }
        Ok(urls)
    }

    /// `getAllTables`.
    async fn get_all_tables(&self) -> Result<Vec<(String, String)>> {
        let mut query = "
      SELECT table_schema AS schema, table_name AS name
      FROM information_schema.tables
      WHERE tables.table_schema NOT IN ('information_schema', 'mysql', 'performance_schema', 'sys')
    "
        .to_string();
        if let Some(schema) = &self.config.schema {
            query = format!("{query} AND tables.table_schema = '{schema}'");
        }
        let result = self.query(&query, &[], &QueryOptions::default()).await?;
        Ok((0..result.len())
            .filter_map(|i| {
                Some((
                    result.get_string(i, "schema")?,
                    result.get_string(i, "name")?,
                ))
            })
            .collect())
    }

    /// `getColumns`: `SHOW COLUMNS` of one table (used for views, which
    /// `information_schema.columns` does not list on Athena).
    async fn get_columns(&self, schema: &str, name: &str) -> Result<DatabaseStructure> {
        let data = self
            .query(
                &format!("SHOW COLUMNS IN `{schema}`.`{name}`"),
                &[],
                &QueryOptions::default(),
            )
            .await?;
        let columns = (0..data.len())
            .filter_map(|i| data.get_string(i, "column"))
            .map(|column| {
                let mut parts = column.split('\t');
                let name = parts.next().unwrap_or_default().to_string();
                let type_ = parts.next().unwrap_or_default().to_string();
                SchemaColumn {
                    name,
                    type_,
                    attributes: Vec::new(),
                    foreign_keys: Vec::new(),
                }
            })
            .collect();
        let mut structure = DatabaseStructure::new();
        structure
            .entry(schema.to_string())
            .or_default()
            .insert(name.to_string(), columns);
        Ok(structure)
    }

    /// `viewsSchema`: columns of every table that `information_schema.columns`
    /// did not report.
    async fn views_schema(&self, tables_schema: &DatabaseStructure) -> Result<DatabaseStructure> {
        let views: Vec<(String, String)> = self
            .get_all_tables()
            .await?
            .into_iter()
            .filter(|(schema, name)| {
                !tables_schema
                    .get(schema)
                    .map(|t| t.contains_key(name))
                    .unwrap_or(false)
            })
            .collect();
        let structures = futures::future::try_join_all(
            views
                .iter()
                .map(|(schema, name)| self.get_columns(schema, name)),
        )
        .await?;
        Ok(merge_schemas(structures))
    }
}

/// `mergeSchemas`: the first definition of a table wins.
pub fn merge_schemas(schemas: Vec<DatabaseStructure>) -> DatabaseStructure {
    let mut result = DatabaseStructure::new();
    for structure in schemas {
        for (schema, tables) in structure {
            let target = result.entry(schema).or_default();
            for (name, columns) in tables {
                target.entry(name).or_insert(columns);
            }
        }
    }
    result
}

/// `bucketName.replace(/^[a-zA-Z]+:\/\//, '')`.
fn strip_scheme(bucket: &str) -> &str {
    match bucket.find("://") {
        Some(idx) if idx > 0 && bucket[..idx].chars().all(|c| c.is_ascii_alphabetic()) => {
            &bucket[idx + 3..]
        }
        _ => bucket,
    }
}

fn s3_error<E, R>(err: SdkError<E, R>, context: &str) -> DriverError
where
    E: ProvideErrorMetadata + std::error::Error + Send + Sync + 'static,
    R: std::fmt::Debug,
{
    DriverError::Query(format!("{context}: {}", DisplayErrorContext(&err)))
}

#[async_trait]
impl Driver for AthenaDriver {
    fn config(&self) -> &DriverConfig {
        &self.config.driver
    }

    /// `getWorkGroup` of the configured work group.
    async fn test_connection(&self) -> Result<()> {
        self.athena()
            .await?
            .get_work_group()
            .work_group(self.config.work_group.clone())
            .send()
            .await
            .map_err(sdk_error)?;
        Ok(())
    }

    async fn query(
        &self,
        sql: &str,
        params: &[Value],
        _options: &QueryOptions,
    ) -> Result<QueryResult> {
        self.run(sql, params).await
    }

    fn read_only(&self) -> bool {
        self.config.read_only
    }

    fn capabilities(&self) -> DriverCapabilities {
        DriverCapabilities {
            unload_without_temp_table: true,
            incremental_schema_loading: true,
            ..Default::default()
        }
    }

    fn information_schema_query(&self) -> String {
        let base = crate::sql::information_schema_query(&|i| self.quote_identifier(i));
        match &self.config.schema {
            Some(schema) => format!("{base} AND columns.table_schema = '{schema}'"),
            None => base,
        }
    }

    async fn tables_schema(&self) -> Result<DatabaseStructure> {
        let query = self.information_schema_query();
        let data = self.query(&query, &[], &QueryOptions::default()).await?;
        let tables_schema = crate::driver::information_columns_to_structure(&data);
        let views_schema = self.views_schema(&tables_schema).await?;
        Ok(merge_schemas(vec![tables_schema, views_schema]))
    }

    async fn load_pre_aggregation_into_table(
        &self,
        _pre_aggregation_table_name: &str,
        load_sql: &str,
        params: &[Value],
        _options: &QueryOptions,
    ) -> Result<QueryResult> {
        if self.config.s3_output_location.is_none() {
            return Err(DriverError::Config(
                "Unload is not configured. Please define CUBEJS_AWS_S3_OUTPUT_LOCATION env var "
                    .to_string(),
            ));
        }
        let mut guard = self.execute(load_sql, params).await?;
        guard.disarm();
        Ok(QueryResult::default())
    }

    async fn download_query_results(
        &self,
        sql: &str,
        params: &[Value],
        options: &DownloadQueryResultsOptions,
    ) -> Result<DownloadedData> {
        if options.stream_import {
            Ok(DownloadedData::Stream(
                self.stream(sql, params, &options.stream).await?,
            ))
        } else {
            Ok(DownloadedData::Memory(self.run(sql, params).await?))
        }
    }

    /// Rows are fetched page by page as the stream is polled; dropping the
    /// stream before its end stops the query (`release`).
    async fn stream(
        &self,
        sql: &str,
        params: &[Value],
        _options: &StreamOptions,
    ) -> Result<StreamTableData> {
        let guard = self.execute(sql, params).await?;
        let client = self.athena().await?.clone();
        let first = fetch_page(&client, &guard.query_execution_id, None).await?;
        let info = page_column_info(&first);
        let columns = self.map_types(info);
        let width = result_column_names(sql, info).len();

        struct State {
            client: aws_sdk_athena::Client,
            guard: StopOnDrop,
            page: Option<(GetQueryResultsOutput, bool)>,
        }
        let state = State {
            client,
            guard,
            page: Some((first, true)),
        };
        let rows = futures::stream::try_unfold(state, move |mut state| async move {
            let Some((page, first_page)) = state.page.take() else {
                state.guard.disarm();
                return Ok(None);
            };
            let rows = page_rows(&page, width, first_page);
            if let Some(token) = page.next_token() {
                let next = fetch_page(
                    &state.client,
                    &state.guard.query_execution_id,
                    Some(token.to_string()),
                )
                .await?;
                state.page = Some((next, false));
            }
            Ok(Some((rows, state)))
        })
        .map(|chunk: Result<Vec<Row>>| {
            futures::stream::iter(match chunk {
                Ok(rows) => rows.into_iter().map(Ok).collect::<Vec<_>>(),
                Err(e) => vec![Err(e)],
            })
        })
        .flatten()
        .boxed();

        Ok(StreamTableData { columns, rows })
    }

    async fn is_unload_supported(&self, _options: &UnloadOptions) -> Result<bool> {
        Ok(self.config.export_bucket.is_some())
    }

    async fn unload(&self, table: &str, options: &UnloadOptions) -> Result<TableCsvData> {
        if self.config.export_bucket.is_none() {
            return Err(DriverError::Config(
                "Export bucket is not configured.".to_string(),
            ));
        }
        let types = match &options.query {
            Some(query) => self.unload_with_sql(table, query).await?,
            None => self.unload_with_table(table).await?,
        };
        let csv_file = self.get_csv_files(table).await?;
        Ok(TableCsvData {
            csv_file,
            types: Some(types),
            csv_no_header: true,
            csv_delimiter: Some(UNLOAD_CSV_DELIMITER.to_string()),
            csv_disable_quoting: true,
            export_bucket_csv_escape_symbol: self.config.export_bucket_csv_escape_symbol.clone(),
        })
    }

    /// Not defined by the Node driver (the Node orchestrator then downloads
    /// the rows instead); the Rust orchestrator calls it whenever unload is
    /// supported, so the query is unloaded under a unique prefix.
    async fn unload_from_query(
        &self,
        sql: &str,
        params: &[Value],
        options: &UnloadOptions,
    ) -> Result<TableCsvData> {
        let prefix = format!(
            "unload_from_query_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        self.unload(
            &prefix,
            &UnloadOptions {
                query: Some(UnloadQuery {
                    sql: sql.to_string(),
                    params: params.to_vec(),
                }),
                ..options.clone()
            },
        )
        .await
    }

    /// `queryColumnTypes`: runs `<sql> LIMIT 0` and maps the metadata.
    async fn query_column_types(
        &self,
        sql: &str,
        params: &[Value],
        _options: &QueryOptions,
    ) -> Result<TableStructure> {
        let mut guard = self.execute(&format!("{sql} LIMIT 0"), params).await?;
        let page = fetch_page(self.athena().await?, &guard.query_execution_id, None).await?;
        guard.disarm();
        Ok(self.map_types(page_column_info(&page)))
    }
}
