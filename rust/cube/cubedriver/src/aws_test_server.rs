//! A tiny HTTP/1.1 server for unit-testing the AWS drivers (Athena, Aurora
//! Serverless Data API) without an AWS account.
//!
//! The AWS SDK is pointed at it with an endpoint override. Every request is
//! recorded and answered by a closure, so a test can script the JSON protocol
//! (`X-Amz-Target` for Athena's awsJson1.1, the URI path for the Data API's
//! restJson1) and then assert on what the SDK sent.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A request as received by [`MockServer`].
#[derive(Debug, Clone)]
pub struct MockRequest {
    pub method: String,
    /// Path plus query string.
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl MockRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
    }
}

/// A scripted answer.
#[derive(Debug, Clone)]
pub struct MockResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl MockResponse {
    pub fn json(status: u16, content_type: &str, body: serde_json::Value) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".to_string(), content_type.to_string())],
            body: body.to_string().into_bytes(),
        }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }
}

type Handler = dyn Fn(&MockRequest) -> MockResponse + Send + Sync;

/// Local HTTP server answering with a closure.
pub struct MockServer {
    pub endpoint: String,
    requests: Arc<Mutex<Vec<MockRequest>>>,
    task: tokio::task::JoinHandle<()>,
}

impl MockServer {
    pub async fn start(
        handler: impl Fn(&MockRequest) -> MockResponse + Send + Sync + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<Handler> = Arc::new(handler);
        let recorded = requests.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let handler = handler.clone();
                let recorded = recorded.clone();
                tokio::spawn(async move {
                    let _ = serve_connection(stream, handler, recorded).await;
                });
            }
        });
        Self {
            endpoint,
            requests,
            task,
        }
    }

    /// Every request received so far.
    pub fn requests(&self) -> Vec<MockRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve_connection(
    mut stream: TcpStream,
    handler: Arc<Handler>,
    recorded: Arc<Mutex<Vec<MockRequest>>>,
) -> std::io::Result<()> {
    let mut buffer: Vec<u8> = Vec::new();
    loop {
        // Headers.
        let header_end = loop {
            if let Some(pos) = find(&buffer, b"\r\n\r\n") {
                break pos;
            }
            let mut chunk = [0u8; 8192];
            let n = stream.read(&mut chunk).await?;
            if n == 0 {
                return Ok(());
            }
            buffer.extend_from_slice(&chunk[..n]);
        };
        let head = String::from_utf8_lossy(&buffer[..header_end]).to_string();
        let mut lines = head.split("\r\n");
        let request_line = lines.next().unwrap_or_default();
        let mut parts = request_line.split(' ');
        let method = parts.next().unwrap_or_default().to_string();
        let target = parts.next().unwrap_or_default().to_string();
        let headers: Vec<(String, String)> = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
            .collect();
        let content_length = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, v)| v.parse::<usize>().ok())
            .unwrap_or(0);
        let body_start = header_end + 4;
        while buffer.len() < body_start + content_length {
            let mut chunk = [0u8; 8192];
            let n = stream.read(&mut chunk).await?;
            if n == 0 {
                return Ok(());
            }
            buffer.extend_from_slice(&chunk[..n]);
        }
        let body = buffer[body_start..body_start + content_length].to_vec();
        buffer.drain(..body_start + content_length);

        let request = MockRequest {
            method,
            target,
            headers,
            body,
        };
        recorded.lock().unwrap().push(request.clone());
        let response = handler(&request);

        let mut out = format!("HTTP/1.1 {} Mock\r\n", response.status);
        for (k, v) in &response.headers {
            out.push_str(&format!("{k}: {v}\r\n"));
        }
        out.push_str(&format!("Content-Length: {}\r\n\r\n", response.body.len()));
        stream.write_all(out.as_bytes()).await?;
        stream.write_all(&response.body).await?;
        stream.flush().await?;
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
