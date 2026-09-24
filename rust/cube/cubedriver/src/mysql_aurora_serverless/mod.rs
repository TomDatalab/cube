//! Aurora Serverless (MySQL) driver: port of
//! `@cubejs-backend/mysql-aurora-serverless-driver` on the RDS Data API
//! (`aws-sdk-rdsdata`, rustls).
//!
//! The Node driver sits on `data-api-client`, which turns positional values
//! into named Data API parameters, hydrates records into objects and retries
//! while a paused cluster resumes; [`data_api`] ports those pieces.
//!
//! Differences from Node, both fixes of paths that could not work there:
//! * `downloadQueryResults` binds the query's values in the temporary-table
//!   statement (Node passed the raw `?` text plus a values array that
//!   `data-api-client` rejects).
//! * a `DESCRIBE` type returned as a blob (MySQL 8 reports `Type` as binary)
//!   is decoded as UTF-8 text instead of breaking the type mapping.

pub mod data_api;

use async_trait::async_trait;
use aws_config::{BehaviorVersion, Region};
use aws_sdk_rdsdata::error::{DisplayErrorContext, ProvideErrorMetadata, SdkError};
use serde_json::Value;
use sha2::Digest;
use tokio::sync::OnceCell;

use crate::config::{DriverConfig, EnvSource, ProcessEnv};
use crate::driver::Driver;
use crate::error::{DriverError, Result};
use crate::types::{
    Column, DownloadQueryResultsOptions, DownloadedData, ExternalCreateTableOptions, GenericType,
    IndexSql, QueryOptions, QueryResult, Row, TableMemoryData,
};

pub use data_api::RetryOptions;

#[cfg(test)]
mod tests;

/// `AuroraServerlessMySqlDriver.getDefaultConcurrency()`.
pub const DEFAULT_CONCURRENCY: usize = 2;
/// Rows per `INSERT` in `uploadTableWithIndexes`.
pub const UPLOAD_BATCH_SIZE: usize = 1000;

/// Configuration of [`AuroraServerlessMySqlDriver`] (`ConnectionOptions`).
#[derive(Debug, Clone)]
pub struct AuroraServerlessMySqlConfig {
    /// Shared driver knobs.
    pub driver: DriverConfig,
    /// `CUBEJS_DATABASE_SECRET_ARN`.
    pub secret_arn: Option<String>,
    /// `CUBEJS_DATABASE_CLUSTER_ARN`.
    pub resource_arn: Option<String>,
    /// `CUBEJS_DB_NAME`, else the deprecated `CUBEJS_DATABASE`.
    pub database: Option<String>,
    /// `loadPreAggregationWithoutMetaLock` option (no environment variable).
    pub load_pre_aggregation_without_meta_lock: bool,
    /// `options.region` of the Node driver; `None` uses the SDK's chain
    /// (`AWS_REGION`, profile, IMDS).
    pub region: Option<String>,
    /// `options.credentials` (static keys; no environment variable in Node
    /// either). `None` uses the SDK's default credential chain.
    pub credentials: Option<(String, String)>,
    /// `options.endpoint`; the SDK also honours `AWS_ENDPOINT_URL` /
    /// `AWS_ENDPOINT_URL_RDS_DATA`.
    pub endpoint_url: Option<String>,
    /// `data-api-client` retry options.
    pub retry: RetryOptions,
}

impl AuroraServerlessMySqlConfig {
    /// Builds the configuration from a generic [`DriverConfig`].
    pub fn from_driver_config(driver: DriverConfig) -> Self {
        let database = driver.data_source.database.clone();
        Self {
            driver,
            secret_arn: None,
            resource_arn: None,
            database,
            load_pre_aggregation_without_meta_lock: false,
            region: None,
            credentials: None,
            endpoint_url: None,
            retry: RetryOptions::default(),
        }
    }

    /// Reads the configuration of `data_source` from the process environment.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        let mut config = Self::from_driver_config(DriverConfig::from_env(data_source)?);
        config.apply_env()?;
        Ok(config)
    }

    /// Reads `CUBEJS_DATABASE_SECRET_ARN`, `CUBEJS_DATABASE_CLUSTER_ARN` and
    /// `CUBEJS_DATABASE` (data-source aware) from the process environment.
    pub fn apply_env(&mut self) -> Result<()> {
        self.apply_env_source(&ProcessEnv)
    }

    /// [`AuroraServerlessMySqlConfig::apply_env`] against any [`EnvSource`].
    pub fn apply_env_source(&mut self, env: &dyn EnvSource) -> Result<()> {
        let ds = self.driver.data_source.clone();
        let get = |key: &str| read_env(env, &ds, key);
        self.secret_arn = get("CUBEJS_DATABASE_SECRET_ARN")?;
        self.resource_arn = get("CUBEJS_DATABASE_CLUSTER_ARN")?;
        if self.database.is_none() {
            self.database = get("CUBEJS_DATABASE")?;
        }
        Ok(())
    }
}

/// `getEnv(<key>, { dataSource, preAggregations })`; empty values are unset.
fn read_env(
    env: &dyn EnvSource,
    ds: &crate::config::DataSourceConfig,
    key: &str,
) -> Result<Option<String>> {
    let declared = crate::config::data_sources(env);
    let data_source = (!ds.data_source.is_empty()).then_some(ds.data_source.as_str());
    let key = crate::config::env_key(key, &declared, data_source, ds.pre_aggregations)?;
    Ok(env.get(&key).filter(|v| !v.is_empty()))
}

/// A hydrated `ExecuteStatement` result.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StatementResult {
    /// `(label, typeName)` per column.
    pub columns: Vec<(String, Option<String>)>,
    /// `None` when the statement returned no records (DDL, DML).
    pub rows: Option<Vec<Row>>,
}

/// Converts an SDK error; the Data API reports SQL errors as
/// `BadRequestException` with the server message.
fn sdk_error<E, R>(err: &SdkError<E, R>) -> DriverError
where
    E: ProvideErrorMetadata + std::error::Error + Send + Sync + 'static,
    R: std::fmt::Debug,
{
    match err {
        SdkError::ServiceError(_) => DriverError::Database {
            message: err
                .message()
                .map(str::to_string)
                .or_else(|| err.code().map(str::to_string))
                .unwrap_or_else(|| DisplayErrorContext(err).to_string()),
            code: err.code().map(str::to_string),
        },
        _ => DriverError::Connection {
            pool_name: "mysqlauroraserverless".to_string(),
            message: DisplayErrorContext(err).to_string(),
        },
    }
}

/// Aurora Serverless MySQL driver (RDS Data API).
pub struct AuroraServerlessMySqlDriver {
    config: AuroraServerlessMySqlConfig,
    secret_arn: String,
    resource_arn: String,
    client: OnceCell<aws_sdk_rdsdata::Client>,
}

impl std::fmt::Debug for AuroraServerlessMySqlDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuroraServerlessMySqlDriver")
            .field("resource_arn", &self.resource_arn)
            .field("database", &self.config.database)
            .finish()
    }
}

impl AuroraServerlessMySqlDriver {
    /// Creates the driver. Like `data-api-client`'s constructor, it fails
    /// when the secret or cluster ARN is missing.
    pub fn new(config: AuroraServerlessMySqlConfig) -> Result<Self> {
        let secret_arn = config.secret_arn.clone().ok_or_else(|| {
            DriverError::Config(
                "'secretArn' string value required (set CUBEJS_DATABASE_SECRET_ARN)".to_string(),
            )
        })?;
        let resource_arn = config.resource_arn.clone().ok_or_else(|| {
            DriverError::Config(
                "'resourceArn' string value required (set CUBEJS_DATABASE_CLUSTER_ARN)".to_string(),
            )
        })?;
        Ok(Self {
            config,
            secret_arn,
            resource_arn,
            client: OnceCell::new(),
        })
    }

    /// Creates the driver from the environment.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Self::new(AuroraServerlessMySqlConfig::from_env(data_source)?)
    }

    /// The driver configuration.
    pub fn aurora_config(&self) -> &AuroraServerlessMySqlConfig {
        &self.config
    }

    async fn client(&self) -> &aws_sdk_rdsdata::Client {
        self.client
            .get_or_init(|| async {
                let mut loader = aws_config::defaults(BehaviorVersion::latest());
                if let Some(region) = &self.config.region {
                    loader = loader.region(Region::new(region.clone()));
                }
                if let Some(endpoint) = &self.config.endpoint_url {
                    loader = loader.endpoint_url(endpoint.clone());
                }
                if let Some((key, secret)) = &self.config.credentials {
                    loader =
                        loader.credentials_provider(aws_sdk_rdsdata::config::Credentials::new(
                            key.clone(),
                            secret.clone(),
                            None,
                            None,
                            "cube-aurora-serverless-driver",
                        ));
                }
                aws_sdk_rdsdata::Client::new(&loader.load().await)
            })
            .await
    }

    /// `withRetry` around one Data API call.
    async fn with_retry<T, E, R, F, Fut>(&self, mut call: F) -> Result<T>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = std::result::Result<T, SdkError<E, R>>>,
        E: ProvideErrorMetadata + std::error::Error + Send + Sync + 'static,
        R: std::fmt::Debug,
    {
        let mut attempt = 0usize;
        loop {
            match call().await {
                Ok(v) => return Ok(v),
                Err(e) => {
                    let code = e.code().unwrap_or_default().to_string();
                    let message = e
                        .message()
                        .map(str::to_string)
                        .unwrap_or_else(|| DisplayErrorContext(&e).to_string());
                    match data_api::retry_delay(&self.config.retry, attempt, &code, &message) {
                        Some(delay) => {
                            log::debug!(
                                "Retrying Data API call after {code}: {message} (attempt {})",
                                attempt + 1
                            );
                            tokio::time::sleep(delay).await;
                            attempt += 1;
                        }
                        None => return Err(sdk_error(&e)),
                    }
                }
            }
        }
    }

    /// `ExecuteStatement` with `?` placeholders and positional values.
    pub async fn execute(
        &self,
        sql: &str,
        values: &[Value],
        transaction_id: Option<&str>,
    ) -> Result<StatementResult> {
        self.execute_with(sql, values, transaction_id, false).await
    }

    /// [`Self::execute`]; `blob_as_text` decodes blob cells as UTF-8 text
    /// (MySQL 8 reports `DESCRIBE`'s `Type` column as binary).
    async fn execute_with(
        &self,
        sql: &str,
        values: &[Value],
        transaction_id: Option<&str>,
        blob_as_text: bool,
    ) -> Result<StatementResult> {
        let (sql, parameters) = data_api::build_statement(sql, values)?;
        let client = self.client().await;
        let output = self
            .with_retry(|| {
                client
                    .execute_statement()
                    .resource_arn(self.resource_arn.clone())
                    .secret_arn(self.secret_arn.clone())
                    .sql(sql.clone())
                    .set_database(self.config.database.clone())
                    .set_transaction_id(transaction_id.map(str::to_string))
                    .set_parameters((!parameters.is_empty()).then(|| parameters.clone()))
                    .include_result_metadata(true)
                    .send()
            })
            .await?;

        let columns: Vec<(String, Option<String>)> = output
            .column_metadata()
            .iter()
            .map(|c| {
                (
                    c.label().or(c.name()).unwrap_or_default().to_string(),
                    c.type_name().map(str::to_string),
                )
            })
            .collect();
        // `records` is absent (not empty) for statements without a result set.
        let rows = match output.records.as_ref() {
            None => None,
            Some(records) => Some(
                records
                    .iter()
                    .map(|record| {
                        record
                            .iter()
                            .enumerate()
                            .map(|(i, field)| {
                                data_api::format_record_value(
                                    field,
                                    columns.get(i).and_then(|c| c.1.as_deref()),
                                    blob_as_text,
                                )
                            })
                            .collect::<Result<Row>>()
                    })
                    .collect::<Result<Vec<Row>>>()?,
            ),
        };
        Ok(StatementResult { columns, rows })
    }

    fn to_query_result(&self, result: StatementResult) -> QueryResult {
        let columns = result
            .columns
            .iter()
            .map(|(label, type_name)| {
                Column::new(
                    label.clone(),
                    self.to_generic_type(type_name.as_deref().unwrap_or("text"), None, None),
                )
            })
            .collect();
        QueryResult::new(columns, result.rows.unwrap_or_default())
    }

    /// `dataApi.transaction()…commit()`: runs `statements` in one transaction
    /// and rolls back when one of them fails.
    async fn transaction(
        &self,
        statements: &[(String, Vec<Value>)],
    ) -> Result<Vec<StatementResult>> {
        let client = self.client().await;
        let begin = self
            .with_retry(|| {
                client
                    .begin_transaction()
                    .resource_arn(self.resource_arn.clone())
                    .secret_arn(self.secret_arn.clone())
                    .set_database(self.config.database.clone())
                    .send()
            })
            .await?;
        let transaction_id = begin.transaction_id().unwrap_or_default().to_string();

        let mut results = Vec::with_capacity(statements.len());
        for (sql, values) in statements {
            match self
                .execute_with(sql, values, Some(&transaction_id), true)
                .await
            {
                Ok(result) => results.push(result),
                Err(e) => {
                    if let Err(rollback) = client
                        .rollback_transaction()
                        .resource_arn(self.resource_arn.clone())
                        .secret_arn(self.secret_arn.clone())
                        .transaction_id(transaction_id.clone())
                        .send()
                        .await
                    {
                        log::warn!("Data API rollback failed: {}", sdk_error(&rollback));
                    }
                    return Err(e);
                }
            }
        }

        self.with_retry(|| {
            client
                .commit_transaction()
                .resource_arn(self.resource_arn.clone())
                .secret_arn(self.secret_arn.clone())
                .transaction_id(transaction_id.clone())
                .send()
        })
        .await?;
        Ok(results)
    }
}

/// Random name for the type-probing temporary table
/// (`crypto.randomBytes(10).toString('hex')`).
fn random_table_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seed = format!(
        "{:?}-{}-{}",
        std::time::SystemTime::now(),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let digest = sha2::Sha256::digest(seed.as_bytes());
    digest[..10].iter().map(|b| format!("{b:02x}")).collect()
}

/// `loadSql.replace(/^CREATE TABLE (\S+) AS/i, 'INSERT INTO $1')`.
pub fn create_table_as_to_insert(load_sql: &str) -> String {
    const PREFIX: &str = "CREATE TABLE ";
    if load_sql.len() < PREFIX.len() || !load_sql[..PREFIX.len()].eq_ignore_ascii_case(PREFIX) {
        return load_sql.to_string();
    }
    let rest = &load_sql[PREFIX.len()..];
    let name_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    if name_end == 0 {
        return load_sql.to_string();
    }
    let (name, after) = rest.split_at(name_end);
    match after.get(..3) {
        Some(s) if s.eq_ignore_ascii_case(" AS") => format!("INSERT INTO {name}{}", &after[3..]),
        _ => load_sql.to_string(),
    }
}

#[async_trait]
impl Driver for AuroraServerlessMySqlDriver {
    fn config(&self) -> &DriverConfig {
        &self.config.driver
    }

    async fn test_connection(&self) -> Result<()> {
        self.execute("SELECT 1", &[], None).await?;
        Ok(())
    }

    async fn query(
        &self,
        sql: &str,
        params: &[Value],
        _options: &QueryOptions,
    ) -> Result<QueryResult> {
        let result = self.execute(sql, params, None).await?;
        Ok(self.to_query_result(result))
    }

    fn quote_identifier(&self, identifier: &str) -> String {
        format!("`{identifier}`")
    }

    /// `GenericTypeToMySql`: strings are `varchar(255)` in utf8mb4.
    fn from_generic_type(&self, generic: &GenericType) -> String {
        match generic {
            GenericType::String | GenericType::Text => {
                "varchar(255) CHARACTER SET utf8mb4".to_string()
            }
            other => other.to_string(),
        }
    }

    /// The Node driver interpolates `this.config.database` unconditionally,
    /// so a missing database filters on the literal `'undefined'`.
    fn information_schema_query(&self) -> String {
        format!(
            "{} AND columns.table_schema = '{}'",
            crate::sql::information_schema_query(&|i| self.quote_identifier(i)),
            self.config.database.as_deref().unwrap_or("undefined")
        )
    }

    /// `toColumnValue`: timestamps lose their `Z`, `'true'`/`'false'`
    /// strings become booleans.
    fn to_column_value(&self, value: &Value, generic_type: &GenericType) -> Value {
        match (generic_type, value) {
            (GenericType::Timestamp, Value::String(s)) => Value::String(s.replacen('Z', "", 1)),
            (GenericType::Boolean, Value::String(s)) if s.eq_ignore_ascii_case("true") => {
                Value::Bool(true)
            }
            (GenericType::Boolean, Value::String(s)) if s.eq_ignore_ascii_case("false") => {
                Value::Bool(false)
            }
            _ => value.clone(),
        }
    }

    async fn load_pre_aggregation_into_table(
        &self,
        _pre_aggregation_table_name: &str,
        load_sql: &str,
        params: &[Value],
        options: &QueryOptions,
    ) -> Result<QueryResult> {
        if self.config.load_pre_aggregation_without_meta_lock {
            self.query(&format!("{load_sql} LIMIT 0"), params, options)
                .await?;
            return self
                .query(&create_table_as_to_insert(load_sql), params, options)
                .await;
        }
        self.query(load_sql, params, options).await
    }

    /// Column types come from `DESCRIBE` of a `LIMIT 0` temporary copy of
    /// the query, created and dropped in one transaction.
    async fn download_query_results(
        &self,
        sql: &str,
        params: &[Value],
        _options: &DownloadQueryResultsOptions,
    ) -> Result<DownloadedData> {
        let database = self.config.database.clone().ok_or_else(|| {
            DriverError::Config(
                "Default database should be defined to be used for temporary tables during \
                 query results downloads"
                    .to_string(),
            )
        })?;
        let table = format!("`{database}`.t_{}", random_table_suffix());
        let results = self
            .transaction(&[
                (
                    format!("CREATE TEMPORARY TABLE {table} AS {sql} LIMIT 0"),
                    params.to_vec(),
                ),
                (format!("DESCRIBE {table}"), Vec::new()),
                (format!("DROP TEMPORARY TABLE {table}"), Vec::new()),
            ])
            .await?;
        let describe = self.to_query_result(results.get(1).cloned().unwrap_or_default());
        let types: Vec<Column> = (0..describe.len())
            .filter_map(|i| {
                let name = describe.get_string(i, "Field")?;
                let type_ = describe.get_string(i, "Type")?;
                Some(Column::new(name, self.to_generic_type(&type_, None, None)))
            })
            .collect();

        let data = self.query(sql, params, &QueryOptions::default()).await?;
        // `rows` are objects in Node; align them with the described columns.
        let rows = data
            .rows
            .iter()
            .map(|row| {
                types
                    .iter()
                    .map(|c| {
                        data.column_index(&c.name)
                            .and_then(|idx| row.get(idx).cloned())
                            .unwrap_or(Value::Null)
                    })
                    .collect()
            })
            .collect();
        Ok(DownloadedData::Memory(QueryResult::new(types, rows)))
    }

    /// Batched multi-row `INSERT`s (1000 rows each), then the indexes; the
    /// table is dropped when anything fails.
    async fn upload_table_with_indexes(
        &self,
        table: &str,
        columns: &[Column],
        table_data: &TableMemoryData,
        indexes_sql: &[IndexSql],
        _unique_key_columns: &[String],
        _external_options: &ExternalCreateTableOptions,
    ) -> Result<()> {
        self.create_table(table, columns).await?;
        let upload = async {
            let column_list = columns
                .iter()
                .map(|c| self.quote_identifier(&c.name))
                .collect::<Vec<_>>()
                .join(", ");
            for batch in table_data.rows.chunks(UPLOAD_BATCH_SIZE) {
                let placeholders = (0..batch.len())
                    .map(|i| {
                        let row = (0..columns.len())
                            .map(|c| self.param(c + i * columns.len()))
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!("({row})")
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let params: Vec<Value> = batch
                    .iter()
                    .flat_map(|row| {
                        columns.iter().map(move |c| {
                            let idx = if table_data.columns.is_empty() {
                                columns.iter().position(|x| x.name == c.name)
                            } else {
                                table_data.column_index(&c.name)
                            };
                            let value =
                                idx.and_then(|i| row.get(i).cloned()).unwrap_or(Value::Null);
                            self.to_column_value(&value, &c.type_)
                        })
                    })
                    .collect();
                self.query(
                    &format!(
                        "INSERT INTO {table}\n            ({column_list})\n          VALUES {placeholders}"
                    ),
                    &params,
                    &QueryOptions::default(),
                )
                .await?;
            }
            for index in indexes_sql {
                self.query(&index.sql, &index.params, &QueryOptions::default())
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
}
