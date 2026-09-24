//! Druid driver: port of `@cubejs-backend/druid-driver` over Druid's SQL HTTP
//! endpoint (`POST /druid/v2/sql/`) with `reqwest` (rustls).
//!
//! The Node driver posts the same JSON body through axios; this port keeps the
//! request shape (`header`, `sqlTypesHeader`, `resultFormat: object`,
//! `VARCHAR` parameters) and the SQL identical.

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::config::DriverConfig;
use crate::driver::Driver;
use crate::error::{DriverError, Result};
use crate::types::{
    Column, DownloadQueryResultsOptions, DownloadedData, GenericType, QueryOptions, QueryResult,
    Row,
};

/// `getDefaultConcurrency`.
pub const DEFAULT_CONCURRENCY: usize = 2;
/// Default database (`CUBEJS_DB_NAME`).
pub const DEFAULT_DATABASE: &str = "default";

/// Configuration of [`DruidDriver`] (`DruidClientConfiguration`).
#[derive(Debug, Clone)]
pub struct DruidConfig {
    /// Connection settings and global driver knobs.
    pub driver: DriverConfig,
    /// `CUBEJS_DB_URL` (or `http[s]://<host>:<port>`).
    pub url: String,
    /// `CUBEJS_DB_USER`.
    pub user: Option<String>,
    /// `CUBEJS_DB_PASS`.
    pub password: Option<String>,
    /// `CUBEJS_DB_NAME` (default `default`).
    pub database: String,
    /// `CUBEJS_DB_QUERY_TIMEOUT`.
    pub request_timeout: Duration,
}

impl DruidConfig {
    /// Builds the Druid configuration from a generic [`DriverConfig`].
    ///
    /// Mirrors the Node constructor: `CUBEJS_DB_URL`, else host + port (+ SSL),
    /// else an error.
    pub fn from_driver_config(driver: DriverConfig) -> Result<Self> {
        let ds = &driver.data_source;
        let url = match ds.url.clone().filter(|u| !u.is_empty()) {
            Some(url) => url,
            None => match (ds.host.clone().filter(|h| !h.is_empty()), ds.port) {
                (Some(host), Some(port)) => {
                    let protocol = if ds.ssl.is_some() { "https" } else { "http" };
                    format!("{protocol}://{host}:{port}")
                }
                _ => {
                    return Err(DriverError::Config(
                        "Please specify CUBEJS_DB_URL".to_string(),
                    ))
                }
            },
        };
        let database = ds
            .database
            .clone()
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| DEFAULT_DATABASE.to_string());
        Ok(Self {
            url: url.trim_end_matches('/').to_string(),
            user: ds.user.clone(),
            password: ds.password.clone(),
            database,
            request_timeout: ds.query_timeout,
            driver,
        })
    }

    /// Reads the configuration of `data_source` from the environment.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Self::from_driver_config(DriverConfig::from_env(data_source)?)
    }

    /// Reads the configuration from a broker URL, for tests and tooling.
    pub fn from_url(url: &str) -> Result<Self> {
        let mut driver = DriverConfig::default();
        driver.data_source.url = Some(url.to_string());
        Self::from_driver_config(driver)
    }
}

/// A JSON object whose keys keep their document order.
///
/// `serde_json::Map` sorts its keys (the crate is built without
/// `preserve_order`), which would reorder the result columns: Druid sends both
/// the header and the rows as objects, and their order *is* the column order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OrderedMap(pub Vec<(String, Value)>);

impl OrderedMap {
    /// Value of `key`, if present.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// The keys, in order.
    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.0.iter().map(|(k, _)| k)
    }
}

impl<'de> Deserialize<'de> for OrderedMap {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct OrderedMapVisitor;

        impl<'de> serde::de::Visitor<'de> for OrderedMapVisitor {
            type Value = OrderedMap;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON object")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut entries = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, Value>()? {
                    entries.push((key, value));
                }
                Ok(OrderedMap(entries))
            }
        }

        deserializer.deserialize_map(OrderedMapVisitor)
    }
}

/// Column metadata of the `sqlTypesHeader` row.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnMeta {
    #[serde(default)]
    pub sql_type: Option<String>,
    #[serde(rename = "type", default)]
    pub native_type: Option<String>,
}

/// Druid driver.
pub struct DruidDriver {
    config: DruidConfig,
    client: reqwest::Client,
}

impl std::fmt::Debug for DruidDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DruidDriver")
            .field("url", &self.config.url)
            .finish()
    }
}

impl DruidDriver {
    /// Creates the driver and its HTTP client.
    pub fn new(config: DruidConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(format!("CubeDev_Cube/{}", env!("CARGO_PKG_VERSION")))
            .timeout(config.request_timeout)
            .build()
            .map_err(|e| DriverError::Config(format!("Unable to build the HTTP client: {e}")))?;
        Ok(Self { config, client })
    }

    /// Creates the driver from `CUBEJS_DB_*`.
    pub fn from_env(data_source: Option<&str>) -> Result<Self> {
        Self::new(DruidConfig::from_env(data_source)?)
    }

    /// The driver configuration.
    pub fn druid_config(&self) -> &DruidConfig {
        &self.config
    }

    /// `normalizeQueryValues`: every parameter is sent as a `VARCHAR`.
    pub fn normalize_query_values(params: &[Value]) -> Vec<Value> {
        params
            .iter()
            .map(|value| json!({ "value": value, "type": "VARCHAR" }))
            .collect()
    }

    /// `DruidClient.query`: returns the column metadata and the rows.
    async fn client_query(
        &self,
        sql: &str,
        params: &[Value],
    ) -> Result<(Option<Vec<(String, ColumnMeta)>>, Vec<OrderedMap>)> {
        let body = json!({
            "query": sql,
            "parameters": Self::normalize_query_values(params),
            "header": true,
            "sqlTypesHeader": true,
            "resultFormat": "object",
        });

        let mut request = self
            .client
            .post(format!("{}/druid/v2/sql/", self.config.url))
            .header("Content-Type", "application/json");
        if let (Some(user), Some(password)) = (&self.config.user, &self.config.password) {
            request = request.basic_auth(user, Some(password));
        }

        let response = request
            .json(&body)
            .send()
            .await
            .map_err(|e| DriverError::Connection {
                pool_name: self.config.url.clone(),
                message: e.to_string(),
            })?;

        let status = response.status();
        let header_included = response
            .headers()
            .get("x-druid-sql-header-included")
            .is_some();
        let text = response
            .text()
            .await
            .map_err(|e| DriverError::Query(e.to_string()))?;

        if !status.is_success() {
            return Err(druid_error(status, &text));
        }

        let parsed: Vec<OrderedMap> = serde_json::from_str(&text)
            .map_err(|e| DriverError::Query(format!("Unexpected Druid response: {e}: {text}")))?;

        let mut rows: Vec<OrderedMap> = Vec::new();
        let mut columns = None;
        for (i, object) in parsed.into_iter().enumerate() {
            if i == 0 && header_included {
                columns = Some(
                    object
                        .0
                        .into_iter()
                        .map(|(name, meta)| {
                            let meta: ColumnMeta = serde_json::from_value(meta).unwrap_or_default();
                            (name, meta)
                        })
                        .collect::<Vec<_>>(),
                );
                continue;
            }
            rows.push(object);
        }

        Ok((columns, rows))
    }

    /// Builds the positional result out of the header and the object rows.
    fn build_result(
        &self,
        columns: Option<Vec<(String, ColumnMeta)>>,
        rows: Vec<OrderedMap>,
    ) -> QueryResult {
        let names: Vec<String> = match &columns {
            Some(columns) => columns.iter().map(|(name, _)| name.clone()).collect(),
            // Old Druid versions do not send a header; fall back to the keys
            // of the first row.
            None => rows
                .first()
                .map(|row| row.keys().cloned().collect())
                .unwrap_or_default(),
        };
        let types: Vec<GenericType> = match &columns {
            Some(columns) => columns
                .iter()
                .map(|(_, meta)| {
                    self.to_generic_type(
                        &meta.sql_type.clone().unwrap_or_default().to_lowercase(),
                        None,
                        None,
                    )
                })
                .collect(),
            None => names.iter().map(|_| GenericType::Text).collect(),
        };

        let columns: Vec<Column> = names
            .iter()
            .zip(types)
            .map(|(name, type_)| Column::new(name.clone(), type_))
            .collect();
        let rows: Vec<Row> = rows
            .into_iter()
            .map(|row| {
                names
                    .iter()
                    .map(|name| row.get(name).cloned().unwrap_or(Value::Null))
                    .collect()
            })
            .collect();
        QueryResult::new(columns, rows)
    }
}

/// Unwraps `{ "errorMessage": … }`.
fn druid_error(status: reqwest::StatusCode, body: &str) -> DriverError {
    #[derive(Deserialize)]
    struct ErrorBody {
        #[serde(rename = "errorMessage")]
        error_message: Option<String>,
    }
    let message = serde_json::from_str::<ErrorBody>(body)
        .ok()
        .and_then(|e| e.error_message)
        .unwrap_or_else(|| body.trim().to_string());
    DriverError::Database {
        message,
        code: Some(status.as_u16().to_string()),
    }
}

#[async_trait]
impl Driver for DruidDriver {
    fn config(&self) -> &DriverConfig {
        &self.config.driver
    }

    /// The Node driver does nothing here.
    async fn test_connection(&self) -> Result<()> {
        Ok(())
    }

    async fn query(
        &self,
        sql: &str,
        params: &[Value],
        _options: &QueryOptions,
    ) -> Result<QueryResult> {
        let (columns, rows) = self.client_query(sql, params).await?;
        Ok(self.build_result(columns, rows))
    }

    fn read_only(&self) -> bool {
        true
    }

    fn information_schema_query(&self) -> String {
        format!(
            "
        SELECT
            COLUMN_NAME as {},
            TABLE_NAME as {},
            TABLE_SCHEMA as {},
            DATA_TYPE as {}
        FROM INFORMATION_SCHEMA.COLUMNS
        WHERE TABLE_SCHEMA NOT IN ('INFORMATION_SCHEMA', 'sys')
    ",
            self.quote_identifier("column_name"),
            self.quote_identifier("table_name"),
            self.quote_identifier("table_schema"),
            self.quote_identifier("data_type"),
        )
    }

    async fn create_schema_if_not_exists(&self, _schema_name: &str) -> Result<()> {
        Err(DriverError::Query(
            "Unable to create schema, Druid does not support it".to_string(),
        ))
    }

    async fn get_tables_query(&self, schema_name: &str) -> Result<Vec<String>> {
        let result = self
            .query(
                "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = ?",
                &[Value::from(schema_name)],
                &QueryOptions::default(),
            )
            .await?;
        Ok((0..result.len())
            .filter_map(|i| {
                result
                    .get_string(i, "TABLE_NAME")
                    .or_else(|| result.get_string(i, "table_name"))
            })
            .collect())
    }

    async fn download_query_results(
        &self,
        sql: &str,
        params: &[Value],
        _options: &DownloadQueryResultsOptions,
    ) -> Result<DownloadedData> {
        let (columns, rows) = self.client_query(sql, params).await?;
        if columns.is_none() {
            return Err(DriverError::Query(
                "You are using an old version of Druid. Unable to detect column types in readOnly mode."
                    .to_string(),
            ));
        }
        Ok(DownloadedData::Memory(self.build_result(columns, rows)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn driver() -> DruidDriver {
        DruidDriver::new(DruidConfig::from_url("http://localhost:8888").unwrap()).unwrap()
    }

    #[test]
    fn url_resolution() {
        let mut driver_config = DriverConfig::default();
        driver_config.data_source.host = Some("broker".into());
        driver_config.data_source.port = Some(8082);
        let config = DruidConfig::from_driver_config(driver_config.clone()).unwrap();
        assert_eq!(config.url, "http://broker:8082");
        assert_eq!(config.database, "default");

        driver_config.data_source.ssl = Some(crate::config::SslConfig::default());
        let config = DruidConfig::from_driver_config(driver_config).unwrap();
        assert_eq!(config.url, "https://broker:8082");

        let mut driver_config = DriverConfig::default();
        driver_config.data_source.url = Some("http://druid.local:8888/".into());
        let config = DruidConfig::from_driver_config(driver_config).unwrap();
        assert_eq!(config.url, "http://druid.local:8888");

        let err = DruidConfig::from_driver_config(DriverConfig::default()).unwrap_err();
        assert_eq!(err.to_string(), "Please specify CUBEJS_DB_URL");
    }

    #[test]
    fn parameters_are_varchar() {
        let values = DruidDriver::normalize_query_values(&[Value::from("x"), Value::from(1)]);
        assert_eq!(
            values,
            vec![
                json!({ "value": "x", "type": "VARCHAR" }),
                json!({ "value": 1, "type": "VARCHAR" }),
            ]
        );
    }

    #[tokio::test]
    async fn sql_contract() {
        let driver = driver();
        assert_eq!(driver.param(0), "?");
        assert_eq!(driver.quote_identifier("a"), "\"a\"");
        assert!(driver.read_only());

        let q = driver.information_schema_query();
        assert!(q.contains("COLUMN_NAME as \"column_name\""));
        assert!(q.contains("FROM INFORMATION_SCHEMA.COLUMNS"));
        assert!(q.contains("WHERE TABLE_SCHEMA NOT IN ('INFORMATION_SCHEMA', 'sys')"));

        let err = driver.create_schema_if_not_exists("x").await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "Unable to create schema, Druid does not support it"
        );
    }

    #[test]
    fn results_are_built_from_the_header() {
        let driver = driver();
        let columns = Some(vec![
            (
                "__time".to_string(),
                ColumnMeta {
                    sql_type: Some("TIMESTAMP".into()),
                    native_type: Some("LONG".into()),
                },
            ),
            (
                "cnt".to_string(),
                ColumnMeta {
                    sql_type: Some("BIGINT".into()),
                    native_type: Some("LONG".into()),
                },
            ),
        ]);
        let rows =
            vec![
                serde_json::from_value(json!({ "__time": "2020-01-01T00:00:00.000Z", "cnt": 3 }))
                    .unwrap(),
            ];
        let result = driver.build_result(columns, rows);
        assert_eq!(
            result.columns,
            vec![
                Column::new("__time", "timestamp"),
                Column::new("cnt", "bigint"),
            ]
        );
        assert_eq!(
            result.rows,
            vec![vec![json!("2020-01-01T00:00:00.000Z"), json!(3)]]
        );
    }

    #[test]
    fn results_without_a_header_fall_back_to_row_keys() {
        let driver = driver();
        let rows = vec![serde_json::from_value(json!({ "a": 1 })).unwrap()];
        let result = driver.build_result(None, rows);
        assert_eq!(result.columns, vec![Column::new("a", "text")]);
        assert_eq!(result.rows, vec![vec![json!(1)]]);
    }

    #[test]
    fn errors_are_unwrapped() {
        let err = druid_error(
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"error":"SQL parse failed","errorMessage":"Encountered \"FROM\""}"#,
        );
        assert_eq!(err.to_string(), "Encountered \"FROM\"");
    }
}
