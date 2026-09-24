//! Presto / Trino client REST protocol (`POST /v1/statement` + `nextUri`
//! polling), a port of `presto-client` 1.2 as the Node.js drivers use it.
//!
//! Behaviour kept from the Node client (and from the monkey patch in
//! `PrestoDriver.applyCustomHeadersToAllRequests`):
//!
//! * headers are `X-Presto-*` or `X-Trino-*` depending on the engine;
//! * custom headers go on **every** request, including the `nextUri` polls,
//!   which follow the host named in `nextUri`; the protocol headers (`User`,
//!   `Source`, `User-Agent`, `Authorization`, and on the POST `Catalog`,
//!   `Schema`, `Session`) are set after them and therefore win;
//! * `502`, `503` and `504` are retried after a random 50–100 ms;
//! * the query is polled every `checkInterval` (800 ms) while it is queued,
//!   planning, starting or running without data;
//! * a whole-query timeout (`CUBEJS_DB_QUERY_TIMEOUT`) cancels the query on the
//!   server and fails with `execution error:query timed out`;
//! * the error messages are the ones `presto-client` produces.
//!
//! Deliberate differences: a page that carried data is followed by the next
//! poll immediately rather than after `checkInterval` (the server long-polls,
//! the fixed sleep only slowed large results down), and a query whose
//! consumer goes away is cancelled on the server with `DELETE nextUri`.

use std::time::{Duration, Instant};

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::Deserialize;
use serde_json::Value;

use crate::config::SslConfig;
use crate::error::{DriverError, Result};

/// `QUERY_STATE_CHECK_INTERVAL` of `presto-client`.
pub const DEFAULT_CHECK_INTERVAL: Duration = Duration::from_millis(800);
/// `source` default of `presto-client` (kept so that resource-group selectors
/// matching the Node.js client keep matching).
pub const DEFAULT_SOURCE: &str = "nodejs-client";

/// Which header family the coordinator speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Engine {
    #[default]
    Presto,
    Trino,
}

impl Engine {
    /// `X-Presto-<suffix>` / `X-Trino-<suffix>`.
    pub fn header(self, suffix: &str) -> String {
        match self {
            Engine::Presto => format!("X-Presto-{suffix}"),
            Engine::Trino => format!("X-Trino-{suffix}"),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Engine::Presto => "presto",
            Engine::Trino => "trino",
        }
    }
}

/// One entry of `columns` in a statement response.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PrestoColumn {
    pub name: String,
    #[serde(rename = "type")]
    pub type_: String,
}

#[derive(Debug, Default, Deserialize)]
struct Stats {
    #[serde(default)]
    state: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueryError {
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    error_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatementResponse {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    next_uri: Option<String>,
    #[serde(default)]
    info_uri: Option<String>,
    #[serde(default)]
    stats: Stats,
    #[serde(default)]
    columns: Option<Vec<PrestoColumn>>,
    #[serde(default)]
    data: Option<Vec<Vec<Value>>>,
    #[serde(default)]
    error: Option<QueryError>,
}

/// Settings of a [`StatementClient`].
#[derive(Debug, Clone)]
pub struct ClientOptions {
    pub engine: Engine,
    pub host: String,
    pub port: u16,
    pub user: Option<String>,
    /// Value of the `Authorization` header (`Basic …` or `Bearer …`).
    pub authorization: Option<String>,
    pub catalog: Option<String>,
    pub source: String,
    pub ssl: Option<SslConfig>,
    /// Custom headers sent on every request.
    pub headers: Vec<(String, String)>,
    pub check_interval: Duration,
    /// Whole-query timeout; `None` disables it (`timeout: 0`).
    pub timeout: Option<Duration>,
}

/// Builds a `reqwest` client (rustls) honouring `CUBEJS_DB_SSL_*`, the way
/// the Node client hands `ssl` to an `https.Agent`.
pub fn build_http_client(ssl: Option<&SslConfig>, what: &str) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .user_agent(format!("CubeDev_Cube/{}", env!("CARGO_PKG_VERSION")));
    if let Some(ssl) = ssl {
        if ssl.passphrase.is_some() {
            return Err(DriverError::NotImplemented(format!(
                "CUBEJS_DB_SSL_PASSPHRASE is not supported by the {what} driver: \
                 encrypted private keys cannot be loaded by rustls. Decrypt the key instead."
            )));
        }
        if ssl.servername.is_some() {
            return Err(DriverError::NotImplemented(format!(
                "CUBEJS_DB_SSL_SERVERNAME is not supported by the {what} driver: \
                 the HTTP client verifies the certificate against the host name of the URL."
            )));
        }
        if ssl.ciphers.is_some() {
            log::warn!(
                "CUBEJS_DB_SSL_CIPHERS is ignored by the {what} driver: rustls does not accept OpenSSL cipher lists"
            );
        }
        if !ssl.reject_unauthorized {
            builder = builder.danger_accept_invalid_certs(true);
        }
        if let Some(ca) = &ssl.ca {
            for cert in reqwest::Certificate::from_pem_bundle(ca.as_bytes()).map_err(|e| {
                DriverError::Config(format!("Invalid CUBEJS_DB_SSL_CA for {what}: {e}"))
            })? {
                builder = builder.add_root_certificate(cert);
            }
        }
        match (&ssl.cert, &ssl.key) {
            (Some(cert), Some(key)) => {
                let identity = reqwest::Identity::from_pem(format!("{cert}\n{key}").as_bytes())
                    .map_err(|e| {
                        DriverError::Config(format!(
                            "Invalid CUBEJS_DB_SSL_CERT / CUBEJS_DB_SSL_KEY for {what}: {e}"
                        ))
                    })?;
                builder = builder.identity(identity);
            }
            (None, None) => {}
            _ => {
                return Err(DriverError::Config(format!(
                    "{what}: CUBEJS_DB_SSL_CERT and CUBEJS_DB_SSL_KEY must be set together"
                )))
            }
        }
    }
    builder
        .build()
        .map_err(|e| DriverError::Config(format!("Unable to build the HTTP client: {e}")))
}

/// The statement protocol client.
#[derive(Debug, Clone)]
pub struct StatementClient {
    http: reqwest::Client,
    options: ClientOptions,
    /// Custom headers, validated once.
    custom: HeaderMap,
}

/// Per-statement settings (`execute({ schema, session, ... })`).
#[derive(Debug, Clone, Default)]
pub struct StatementOptions {
    pub schema: Option<String>,
    /// `X-*-Session` (e.g. `query_max_run_time=600s`).
    pub session: Option<String>,
}

impl StatementClient {
    pub fn new(options: ClientOptions, what: &str) -> Result<Self> {
        let http = build_http_client(options.ssl.as_ref(), what)?;
        let mut custom = HeaderMap::new();
        for (name, value) in &options.headers {
            custom.insert(header_name(name)?, header_value(name, value)?);
        }
        Ok(Self {
            http,
            options,
            custom,
        })
    }

    pub fn options(&self) -> &ClientOptions {
        &self.options
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// `http://host:port` or `https://host:port`.
    pub fn base_url(&self) -> String {
        let protocol = if self.options.ssl.is_some() {
            "https"
        } else {
            "http"
        };
        format!("{protocol}://{}:{}", self.options.host, self.options.port)
    }

    /// Headers of `Client.prototype.request`: custom headers first, then
    /// `extra` (the POST's catalog/schema/session), then user, source,
    /// user agent and authorization.
    pub fn request_headers(&self, extra: &[(String, String)]) -> Result<HeaderMap> {
        let mut headers = self.custom.clone();
        for (name, value) in extra {
            headers.insert(header_name(name)?, header_value(name, value)?);
        }
        let engine = self.options.engine;
        if let Some(user) = &self.options.user {
            headers.insert(
                header_name(&engine.header("User"))?,
                header_value("User", user)?,
            );
        }
        if !self.options.source.is_empty() {
            headers.insert(
                header_name(&engine.header("Source"))?,
                header_value("Source", &self.options.source)?,
            );
        }
        if let Some(authorization) = &self.options.authorization {
            let mut value = header_value("Authorization", authorization)?;
            value.set_sensitive(true);
            headers.insert(reqwest::header::AUTHORIZATION, value);
        }
        Ok(headers)
    }

    /// `client.nodes()`: `GET /v1/node`.
    pub async fn nodes(&self) -> Result<Value> {
        let response = self
            .http
            .get(format!("{}/v1/node", self.base_url()))
            .headers(self.request_headers(&[])?)
            .send()
            .await
            .map_err(|e| DriverError::Connection {
                pool_name: self.options.engine.name().to_string(),
                message: format!("node list api returns error: {e}"),
            })?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if status.as_u16() != 200 {
            return Err(DriverError::Connection {
                pool_name: self.options.engine.name().to_string(),
                message: if body.is_empty() {
                    format!("node list api returns error (HTTP {})", status.as_u16())
                } else {
                    format!("node list api returns error:{body}")
                },
            });
        }
        serde_json::from_str(&body).map_err(|_| DriverError::Connection {
            pool_name: self.options.engine.name().to_string(),
            message: "node list api returns error:execution error:could not parse response"
                .to_string(),
        })
    }

    /// Submits `query` (`POST /v1/statement`) and returns the running
    /// statement; pages are pulled with [`Statement::next_page`].
    pub async fn execute(&self, query: &str, options: &StatementOptions) -> Result<Statement> {
        let engine = self.options.engine;
        if options.schema.is_some() && self.options.catalog.is_none() {
            return Err(DriverError::Config(
                "Catalog not specified; catalog is required if schema is specified".to_string(),
            ));
        }
        let mut extra = Vec::new();
        if let Some(catalog) = &self.options.catalog {
            extra.push((engine.header("Catalog"), catalog.clone()));
        }
        if let Some(schema) = &options.schema {
            extra.push((engine.header("Schema"), schema.clone()));
        }
        if let Some(session) = &options.session {
            extra.push((engine.header("Session"), session.clone()));
        }
        let headers = self.request_headers(&extra)?;

        let mut statement = Statement {
            client: self.clone(),
            budget: self.options.timeout,
            query_id: None,
            next_uri: None,
            columns: None,
            finished: false,
            pending: None,
        };
        let request = self
            .http
            .post(format!("{}/v1/statement", self.base_url()))
            .headers(headers)
            .body(query.to_string());
        let started = Instant::now();
        let response = statement.send_with_budget(request, true).await;
        statement.charge(started);
        let response = response?;

        if response.id.is_none() || response.next_uri.is_none() || response.info_uri.is_none() {
            let message = if response.id.is_none() {
                "query id missing in response for POST /v1/statement"
            } else if response.next_uri.is_none() {
                "nextUri missing in response for POST /v1/statement"
            } else {
                "infoUri missing in response for POST /v1/statement"
            };
            statement.finished = true;
            return Err(DriverError::Query(message.to_string()));
        }
        statement.pending = Some(response);
        Ok(statement)
    }
}

/// A page of rows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Page {
    pub columns: Option<Vec<PrestoColumn>>,
    pub data: Vec<Vec<Value>>,
}

/// A submitted statement.
pub struct Statement {
    client: StatementClient,
    /// Time left of the whole-query timeout. Only time spent talking to the
    /// server is charged, so a slow stream consumer does not time a query out.
    budget: Option<Duration>,
    query_id: Option<String>,
    next_uri: Option<String>,
    columns: Option<Vec<PrestoColumn>>,
    finished: bool,
    pending: Option<StatementResponse>,
}

impl std::fmt::Debug for Statement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Statement")
            .field("query_id", &self.query_id)
            .field("finished", &self.finished)
            .finish()
    }
}

enum Outcome {
    Timeout,
}

impl Statement {
    /// The query id assigned by the coordinator.
    pub fn query_id(&self) -> Option<&str> {
        self.query_id.as_deref()
    }

    /// The columns, once the coordinator has sent them.
    pub fn columns(&self) -> Option<&[PrestoColumn]> {
        self.columns.as_deref()
    }

    fn charge(&mut self, started: Instant) {
        if let Some(budget) = self.budget.as_mut() {
            *budget = budget.saturating_sub(started.elapsed());
        }
    }

    /// Sends `request`, retrying 502/503/504, bounded by the timeout budget.
    async fn send_with_budget(
        &mut self,
        request: reqwest::RequestBuilder,
        first: bool,
    ) -> Result<StatementResponse> {
        let result = match self.budget {
            Some(budget) => {
                match tokio::time::timeout(budget, send_with_retries(request, first)).await {
                    Ok(r) => Ok(r),
                    Err(_) => Err(Outcome::Timeout),
                }
            }
            None => Ok(send_with_retries(request, first).await),
        };
        match result {
            Ok(r) => r,
            Err(Outcome::Timeout) => {
                self.cancel().await;
                Err(DriverError::Query(
                    "execution error:query timed out".to_string(),
                ))
            }
        }
    }

    /// Returns the next page of data, `None` when the query has finished.
    pub async fn next_page(&mut self) -> Result<Option<Page>> {
        loop {
            if self.finished {
                return Ok(None);
            }
            let response = match self.pending.take() {
                Some(r) => r,
                None => {
                    let Some(uri) = self.next_uri.clone() else {
                        self.finished = true;
                        return Ok(None);
                    };
                    let headers = self.client.request_headers(&[])?;
                    let request = self.client.http.get(uri).headers(headers);
                    let started = Instant::now();
                    let r = self.send_with_budget(request, false).await;
                    self.charge(started);
                    match r {
                        Ok(r) => r,
                        Err(e) => {
                            self.finished = true;
                            return Err(e);
                        }
                    }
                }
            };

            if let Some(error) = response.error {
                self.finished = true;
                return Err(DriverError::Database {
                    message: error
                        .message
                        .unwrap_or_else(|| "execution error".to_string()),
                    code: error.error_name,
                });
            }

            if response.id.is_some() {
                self.query_id = response.id.clone();
            }
            if self.columns.is_none() {
                if let Some(columns) = &response.columns {
                    self.columns = Some(columns.clone());
                }
            }
            self.next_uri = response.next_uri.clone();

            let state = response.stats.state.as_str();
            let has_data = response.data.is_some();
            if matches!(state, "QUEUED" | "PLANNING" | "STARTING")
                || (state == "RUNNING" && !has_data)
            {
                if self.next_uri.is_none() {
                    // A well-behaved coordinator never does this; the Node
                    // client would poll `undefined` and fail.
                    self.finished = true;
                    return Err(DriverError::Query(format!(
                        "nextUri missing in response while the query is {state}"
                    )));
                }
                self.wait(self.client.options.check_interval).await?;
                // A page that carries columns but no data yet still tells the
                // caller the result shape.
                if response.columns.is_some() && self.columns.is_some() {
                    return Ok(Some(Page {
                        columns: response.columns,
                        data: Vec::new(),
                    }));
                }
                continue;
            }

            if self.next_uri.is_none() {
                self.finished = true;
            }

            match response.data {
                Some(data) => {
                    return Ok(Some(Page {
                        columns: response.columns,
                        data,
                    }))
                }
                None => {
                    if !self.finished {
                        self.wait(self.client.options.check_interval).await?;
                    }
                    if response.columns.is_some() {
                        return Ok(Some(Page {
                            columns: response.columns,
                            data: Vec::new(),
                        }));
                    }
                }
            }
        }
    }

    /// Sleeps for the poll interval (charged to the timeout budget).
    async fn wait(&mut self, interval: Duration) -> Result<()> {
        if let Some(budget) = self.budget {
            if interval >= budget {
                tokio::time::sleep(budget).await;
                self.budget = Some(Duration::ZERO);
                self.cancel().await;
                self.finished = true;
                return Err(DriverError::Query(
                    "execution error:query timed out".to_string(),
                ));
            }
            self.budget = Some(budget - interval);
        }
        tokio::time::sleep(interval).await;
        Ok(())
    }

    /// Cancels the query on the server (`DELETE nextUri`, or
    /// `DELETE /v1/query/<id>` like `client.kill`). Failures are ignored.
    pub async fn cancel(&mut self) {
        self.finished = true;
        if let Some(request) = self.cancel_request() {
            let _ = tokio::time::timeout(Duration::from_secs(10), request.send()).await;
        }
        self.next_uri = None;
    }

    fn cancel_request(&self) -> Option<reqwest::RequestBuilder> {
        let headers = self.client.request_headers(&[]).ok()?;
        let url = match (&self.next_uri, &self.query_id) {
            (Some(uri), _) => uri.clone(),
            (None, Some(id)) => format!("{}/v1/query/{id}", self.client.base_url()),
            _ => return None,
        };
        Some(self.client.http.delete(url).headers(headers))
    }
}

impl Drop for Statement {
    fn drop(&mut self) {
        if self.finished || self.next_uri.is_none() {
            return;
        }
        // The consumer went away mid-query: free the coordinator's resources.
        if let (Some(request), Ok(handle)) =
            (self.cancel_request(), tokio::runtime::Handle::try_current())
        {
            handle.spawn(async move {
                let _ = tokio::time::timeout(Duration::from_secs(10), request.send()).await;
            });
        }
    }
}

/// One round trip with the retry and error rules of `presto-client`.
async fn send_with_retries(
    request: reqwest::RequestBuilder,
    first: bool,
) -> Result<StatementResponse> {
    loop {
        let Some(attempt) = request.try_clone() else {
            return Err(DriverError::Query(
                "execution error:unable to clone the request".to_string(),
            ));
        };
        let response = match attempt.send().await {
            Ok(r) => r,
            Err(e) => {
                return Err(DriverError::Query(if first {
                    format!("execution error\n{e}")
                } else {
                    e.to_string()
                }))
            }
        };
        let status = response.status().as_u16();
        if matches!(status, 502..=504) {
            tokio::time::sleep(Duration::from_millis(50 + jitter(51))).await;
            continue;
        }
        let body = response.text().await.map_err(|e| {
            DriverError::Query(if first {
                format!("execution error\n{e}")
            } else {
                e.to_string()
            })
        })?;
        if status != 200 {
            return Err(DriverError::Query(if body.is_empty() {
                format!("execution error:invalid response code ({status})")
            } else {
                format!("execution error:{body}")
            }));
        }
        if !(body.starts_with('{') || body.starts_with('[')) {
            return Err(DriverError::Query(
                "execution error:could not parse response".to_string(),
            ));
        }
        return serde_json::from_str(&body).map_err(|_| {
            DriverError::Query("execution error:could not parse response".to_string())
        });
    }
}

/// A pseudo-random number in `0..n` (retry jitter; no need for a RNG crate).
fn jitter(n: u64) -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    nanos % n
}

fn header_name(name: &str) -> Result<HeaderName> {
    HeaderName::from_bytes(name.as_bytes())
        .map_err(|e| DriverError::Config(format!("Invalid HTTP header name \"{name}\": {e}")))
}

fn header_value(name: &str, value: &str) -> Result<HeaderValue> {
    HeaderValue::from_str(value)
        .map_err(|e| DriverError::Config(format!("Invalid value of HTTP header \"{name}\": {e}")))
}
