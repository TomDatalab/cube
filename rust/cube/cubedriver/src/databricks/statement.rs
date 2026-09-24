//! Client for the Databricks SQL Statement Execution API
//! (`POST /api/2.0/sql/statements`).
//!
//! * A statement is submitted with `wait_timeout` + `on_wait_timeout:
//!   CONTINUE`, then polled (`GET /api/2.0/sql/statements/{id}`) with a
//!   back-off capped at `CUBEJS_DB_POLL_MAX_INTERVAL` until it reaches a
//!   terminal state. Past the query timeout it is cancelled
//!   (`POST .../cancel`).
//! * Results are always `JSON_ARRAY` (every value is a string or `null`,
//!   hydrated against the manifest column types). Two dispositions are used:
//!   `INLINE` for ordinary queries (limited to 25 MiB by Databricks), and
//!   `EXTERNAL_LINKS` for pre-aggregation downloads and streams, where each
//!   chunk is a pre-signed cloud storage URL that is fetched *without* the
//!   workspace credentials.
//! * Further chunks are fetched with
//!   `GET /api/2.0/sql/statements/{id}/result/chunks/{index}`.
//!
//! `ARROW_STREAM` is not used: `JSON_ARRAY` works with both dispositions and
//! yields the same textual values in both, so the hydration logic is shared
//! and no Arrow IPC decoder has to be linked in.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{DriverError, Result};

/// `disposition` of a statement request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Disposition {
    Inline,
    ExternalLinks,
}

/// `POST /api/2.0/sql/statements` body.
#[derive(Debug, Clone, Serialize)]
pub struct StatementRequest {
    pub statement: String,
    pub warehouse_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub disposition: Disposition,
    pub format: &'static str,
    pub wait_timeout: String,
    pub on_wait_timeout: &'static str,
}

/// `status.error`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct ServiceError {
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

/// `status`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct StatementStatus {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub error: Option<ServiceError>,
}

/// `manifest.schema.columns[]`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct ColumnInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub type_text: Option<String>,
    #[serde(default)]
    pub type_name: Option<String>,
    #[serde(default)]
    pub position: Option<i64>,
    #[serde(default)]
    pub type_precision: Option<i64>,
    #[serde(default)]
    pub type_scale: Option<i64>,
}

impl ColumnInfo {
    /// `type_text` lower-cased (`decimal(10,2)`), falling back to `type_name`.
    pub fn type_text(&self) -> String {
        self.type_text
            .clone()
            .or_else(|| self.type_name.clone())
            .unwrap_or_else(|| "string".to_string())
            .to_lowercase()
    }

    /// `type_name` upper-cased (`DECIMAL`), falling back to `type_text`.
    pub fn type_name(&self) -> String {
        match &self.type_name {
            Some(t) => t.to_uppercase(),
            None => {
                let text = self.type_text();
                text.split(['(', '<'])
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_uppercase()
            }
        }
    }
}

/// `manifest.schema`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct ResultSchema {
    #[serde(default)]
    pub column_count: Option<i64>,
    #[serde(default)]
    pub columns: Vec<ColumnInfo>,
}

/// `manifest`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct ResultManifest {
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub schema: ResultSchema,
    #[serde(default)]
    pub total_chunk_count: Option<i64>,
    #[serde(default)]
    pub total_row_count: Option<i64>,
    #[serde(default)]
    pub truncated: Option<bool>,
}

/// `result.external_links[]`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct ExternalLink {
    #[serde(default)]
    pub chunk_index: Option<i64>,
    pub external_link: String,
    #[serde(default)]
    pub next_chunk_index: Option<i64>,
    #[serde(default)]
    pub http_headers: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default)]
    pub row_count: Option<i64>,
}

/// `result` (and the body of `GET .../result/chunks/{index}`).
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct ResultData {
    #[serde(default)]
    pub chunk_index: Option<i64>,
    #[serde(default)]
    pub row_count: Option<i64>,
    #[serde(default)]
    pub data_array: Option<Vec<Vec<Value>>>,
    #[serde(default)]
    pub external_links: Option<Vec<ExternalLink>>,
    #[serde(default)]
    pub next_chunk_index: Option<i64>,
    #[serde(default)]
    pub next_chunk_internal_link: Option<String>,
}

impl ResultData {
    /// The chunk that follows this one, if any.
    pub fn next_chunk(&self) -> Option<i64> {
        self.next_chunk_index.or_else(|| {
            self.external_links
                .as_ref()
                .and_then(|links| links.last())
                .and_then(|l| l.next_chunk_index)
        })
    }
}

/// A statement response.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct StatementResponse {
    #[serde(default)]
    pub statement_id: String,
    #[serde(default)]
    pub status: StatementStatus,
    #[serde(default)]
    pub manifest: Option<ResultManifest>,
    #[serde(default)]
    pub result: Option<ResultData>,
}

/// How requests are authenticated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credentials {
    /// Personal access token (`CUBEJS_DB_DATABRICKS_TOKEN` / `PWD`).
    Bearer(String),
    /// `UID`/`PWD` other than `token` (JDBC `AuthMech=3` with a user name).
    Basic { user: String, password: String },
}

/// Terminal state helpers.
pub fn is_terminal(state: &str) -> bool {
    matches!(state, "SUCCEEDED" | "FAILED" | "CANCELED" | "CLOSED")
}

/// Turns a non-successful terminal response into an error.
pub fn check_status(response: &StatementResponse) -> Result<()> {
    match response.status.state.as_str() {
        "SUCCEEDED" => Ok(()),
        "FAILED" => {
            let error = response.status.error.clone().unwrap_or_default();
            Err(DriverError::Database {
                message: error
                    .message
                    .unwrap_or_else(|| "Databricks statement failed".to_string()),
                code: error.error_code,
            })
        }
        other => Err(DriverError::Query(format!(
            "Databricks statement {} ended in state {other}{}",
            response.statement_id,
            response
                .status
                .error
                .as_ref()
                .and_then(|e| e.message.as_ref())
                .map(|m| format!(": {m}"))
                .unwrap_or_default()
        ))),
    }
}

/// Unwraps the `{ "error_code": ..., "message": ... }` error envelope.
pub fn api_error(status: reqwest::StatusCode, body: &str) -> DriverError {
    #[derive(Deserialize)]
    struct ErrorBody {
        error_code: Option<String>,
        message: Option<String>,
    }
    match serde_json::from_str::<ErrorBody>(body) {
        Ok(ErrorBody {
            error_code,
            message: Some(message),
        }) => DriverError::Database {
            message,
            code: error_code.or_else(|| Some(status.as_u16().to_string())),
        },
        _ => DriverError::Database {
            message: format!(
                "Databricks API error: {} {}",
                status.as_u16(),
                if body.trim().is_empty() {
                    status.canonical_reason().unwrap_or("").to_string()
                } else {
                    body.trim().to_string()
                }
            ),
            code: Some(status.as_u16().to_string()),
        },
    }
}

/// Low level Statement Execution API client.
#[derive(Debug, Clone)]
pub struct StatementClient {
    pub(crate) http: reqwest::Client,
    /// `https://<host>` (overridable for tests).
    pub(crate) base_url: String,
}

impl StatementClient {
    pub fn new(http: reqwest::Client, base_url: String) -> Self {
        Self { http, base_url }
    }

    fn authorize(
        &self,
        request: reqwest::RequestBuilder,
        credentials: &Credentials,
    ) -> reqwest::RequestBuilder {
        match credentials {
            Credentials::Bearer(token) => request.bearer_auth(token),
            Credentials::Basic { user, password } => request.basic_auth(user, Some(password)),
        }
    }

    async fn send_json<T: for<'de> Deserialize<'de>>(
        &self,
        request: reqwest::RequestBuilder,
        credentials: &Credentials,
    ) -> Result<T> {
        let response = self
            .authorize(request, credentials)
            .header("Accept", "application/json")
            .header("User-Agent", "CubeDev_Cube")
            .send()
            .await
            .map_err(|e| DriverError::Connection {
                pool_name: "databricks".to_string(),
                message: e.to_string(),
            })?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| DriverError::Query(e.to_string()))?;
        if !status.is_success() {
            return Err(api_error(status, &body));
        }
        serde_json::from_str(&body)
            .map_err(|e| DriverError::Query(format!("Unexpected Databricks response: {e}: {body}")))
    }

    /// `GET /api/2.0/sql/warehouses/{id}`.
    pub async fn get_warehouse(
        &self,
        warehouse_id: &str,
        credentials: &Credentials,
    ) -> Result<Value> {
        let url = format!("{}/api/2.0/sql/warehouses/{warehouse_id}", self.base_url);
        self.send_json(self.http.get(url), credentials).await
    }

    /// Submits `request` and waits until it reaches a terminal state.
    pub async fn execute(
        &self,
        request: &StatementRequest,
        credentials: &Credentials,
        poll_max_interval: Duration,
        timeout: Duration,
    ) -> Result<StatementResponse> {
        let url = format!("{}/api/2.0/sql/statements", self.base_url);
        let started = Instant::now();
        let mut response: StatementResponse = self
            .send_json(self.http.post(&url).json(request), credentials)
            .await?;

        let mut i = 0u32;
        while !is_terminal(&response.status.state) {
            if response.statement_id.is_empty() {
                return Err(DriverError::Query(format!(
                    "Databricks returned state {} without a statement id",
                    response.status.state
                )));
            }
            if started.elapsed() > timeout {
                self.cancel(&response.statement_id, credentials).await.ok();
                return Err(DriverError::Query(format!(
                    "Databricks statement timeout reached {}ms",
                    timeout.as_millis()
                )));
            }
            i += 1;
            let pause = Duration::from_millis(100 * u64::from(i)).min(poll_max_interval);
            tokio::time::sleep(pause).await;
            response = self
                .get_statement(&response.statement_id, credentials)
                .await?;
        }
        check_status(&response)?;
        Ok(response)
    }

    /// `GET /api/2.0/sql/statements/{id}`.
    pub async fn get_statement(
        &self,
        statement_id: &str,
        credentials: &Credentials,
    ) -> Result<StatementResponse> {
        let url = format!("{}/api/2.0/sql/statements/{statement_id}", self.base_url);
        self.send_json(self.http.get(url), credentials).await
    }

    /// `POST /api/2.0/sql/statements/{id}/cancel`.
    pub async fn cancel(&self, statement_id: &str, credentials: &Credentials) -> Result<()> {
        let url = format!(
            "{}/api/2.0/sql/statements/{statement_id}/cancel",
            self.base_url
        );
        let _: Value = self
            .send_json(
                self.http.post(url).json(&serde_json::json!({})),
                credentials,
            )
            .await?;
        Ok(())
    }

    /// `GET /api/2.0/sql/statements/{id}/result/chunks/{index}`.
    pub async fn get_chunk(
        &self,
        statement_id: &str,
        chunk_index: i64,
        credentials: &Credentials,
    ) -> Result<ResultData> {
        let url = format!(
            "{}/api/2.0/sql/statements/{statement_id}/result/chunks/{chunk_index}",
            self.base_url
        );
        self.send_json(self.http.get(url), credentials).await
    }

    /// Downloads one `EXTERNAL_LINKS` chunk. The pre-signed URL must be
    /// fetched without the workspace `Authorization` header.
    pub async fn download_external_link(&self, link: &ExternalLink) -> Result<Vec<Vec<Value>>> {
        let mut request = self.http.get(&link.external_link);
        if let Some(headers) = &link.http_headers {
            for (k, v) in headers {
                request = request.header(k, v);
            }
        }
        let response = request.send().await.map_err(|e| DriverError::Connection {
            pool_name: "databricks".to_string(),
            message: format!("Unable to download a result chunk: {e}"),
        })?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|e| DriverError::Query(e.to_string()))?;
        if !status.is_success() {
            return Err(DriverError::Query(format!(
                "Unable to download a Databricks result chunk: HTTP {}: {}",
                status.as_u16(),
                String::from_utf8_lossy(&body)
            )));
        }
        serde_json::from_slice(&body)
            .map_err(|e| DriverError::Query(format!("Unexpected Databricks result chunk: {e}")))
    }

    /// Rows of one [`ResultData`] (inline or external links).
    pub async fn chunk_rows(&self, data: &ResultData) -> Result<Vec<Vec<Value>>> {
        if let Some(rows) = &data.data_array {
            return Ok(rows.clone());
        }
        let mut rows = Vec::new();
        if let Some(links) = &data.external_links {
            for link in links {
                rows.extend(self.download_external_link(link).await?);
            }
        }
        Ok(rows)
    }
}
