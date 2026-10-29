//! `Authz` trait and its SpiceDB HTTP implementation.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObjectRef {
    pub kind: &'static str,
    pub id: String,
}

impl ObjectRef {
    pub fn new(kind: &'static str, id: impl Into<String>) -> Self {
        Self {
            kind,
            id: id.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Relationship {
    pub resource: ObjectRef,
    pub relation: &'static str,
    pub subject: ObjectRef,
}

#[derive(Debug, Clone)]
pub struct RelationshipFilter {
    pub kind: &'static str,
    pub id: Option<String>,
    pub relation: Option<&'static str>,
    pub subject: Option<ObjectRef>,
}

/// One change in an atomic [`Authz::write`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    Touch(Relationship),
    Delete(Relationship),
}

/// A condition an atomic [`Authz::write`] requires, checked at write time.
#[derive(Debug, Clone)]
pub enum Precondition {
    /// At least one relationship matches the filter.
    MustMatch(RelationshipFilter),
    /// No relationship matches the filter.
    MustNotMatch(RelationshipFilter),
}

#[derive(Debug)]
pub enum AuthzError {
    /// SpiceDB unreachable or failing (maps to 503).
    Unavailable(String),
    /// SpiceDB rejected the request (validation error).
    Rejected(String),
    /// A write precondition did not hold; nothing was written (maps to 409).
    PreconditionFailed(String),
}

impl std::fmt::Display for AuthzError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthzError::Unavailable(m) => write!(f, "authorization unavailable: {m}"),
            AuthzError::Rejected(m) => write!(f, "authorization rejected: {m}"),
            AuthzError::PreconditionFailed(m) => write!(f, "write precondition failed: {m}"),
        }
    }
}

impl std::error::Error for AuthzError {}

#[async_trait]
pub trait Authz: Send + Sync + std::fmt::Debug {
    async fn check(
        &self,
        resource: &ObjectRef,
        permission: &str,
        user: &str,
    ) -> Result<bool, AuthzError>;
    async fn lookup_resources(
        &self,
        kind: &'static str,
        permission: &str,
        user: &str,
    ) -> Result<Vec<String>, AuthzError>;
    async fn read(&self, filter: &RelationshipFilter) -> Result<Vec<Relationship>, AuthzError>;
    /// OPERATION_TOUCH
    async fn touch(&self, relationships: &[Relationship]) -> Result<(), AuthzError>;
    /// OPERATION_DELETE
    async fn delete(&self, relationships: &[Relationship]) -> Result<(), AuthzError>;
    async fn delete_matching(&self, filter: &RelationshipFilter) -> Result<(), AuthzError>;
    /// Applies all `updates` atomically, only if every precondition holds
    /// (else [`AuthzError::PreconditionFailed`] and nothing changes). A
    /// relationship may appear in at most one update.
    async fn write(
        &self,
        updates: &[Update],
        preconditions: &[Precondition],
    ) -> Result<(), AuthzError>;
    async fn ensure_schema(&self, schema: &str) -> Result<(), AuthzError>;
}

/// Leaks each distinct string once, so SpiceDB answers can use `&'static str`.
fn intern(s: &str) -> &'static str {
    static CACHE: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let mut map = CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap();
    if let Some(v) = map.get(s) {
        return v;
    }
    let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
    map.insert(s.to_owned(), leaked);
    leaked
}

#[derive(Debug)]
pub struct HttpAuthz {
    endpoint: String,
    key: String,
    client: reqwest::Client,
    /// Per attempt of a read-only call (`check`, `lookup_resources`,
    /// `read`); such a call is retried once on a timeout or connection
    /// error. Writes are never retried and only bounded by the client's
    /// total timeout.
    read_timeout: Duration,
}

/// How a failed SpiceDB call ended, so read-only calls know whether to retry.
enum PostError {
    /// The request never got an answer (connection, timeout, body read):
    /// a read-only call may be retried.
    Transport(String),
    /// SpiceDB answered with an error.
    Answered(AuthzError),
}

impl From<PostError> for AuthzError {
    fn from(e: PostError) -> Self {
        match e {
            PostError::Transport(m) => AuthzError::Unavailable(m),
            PostError::Answered(e) => e,
        }
    }
}

/// Default per-attempt timeout of read-only calls.
const READ_TIMEOUT: Duration = Duration::from_secs(3);
/// Total timeout of any call (writes, schema).
const TOTAL_TIMEOUT: Duration = Duration::from_secs(10);

fn object_json(o: &ObjectRef) -> Value {
    json!({"objectType": o.kind, "objectId": o.id})
}

fn relationship_json(r: &Relationship) -> Value {
    json!({
        "resource": object_json(&r.resource),
        "relation": r.relation,
        "subject": {"object": object_json(&r.subject)},
    })
}

fn filter_json(f: &RelationshipFilter) -> Value {
    let mut v = json!({"resourceType": f.kind});
    if let Some(id) = &f.id {
        v["optionalResourceId"] = json!(id);
    }
    if let Some(rel) = f.relation {
        v["optionalRelation"] = json!(rel);
    }
    if let Some(s) = &f.subject {
        v["optionalSubjectFilter"] = json!({"subjectType": s.kind, "optionalSubjectId": s.id});
    }
    v
}

/// The gRPC status code SpiceDB answers when a write precondition fails.
const FAILED_PRECONDITION: i64 = 9;

fn update_json(u: &Update) -> Value {
    let (operation, r) = match u {
        Update::Touch(r) => ("OPERATION_TOUCH", r),
        Update::Delete(r) => ("OPERATION_DELETE", r),
    };
    json!({"operation": operation, "relationship": relationship_json(r)})
}

fn precondition_json(p: &Precondition) -> Value {
    let (operation, f) = match p {
        Precondition::MustMatch(f) => ("OPERATION_MUST_MATCH", f),
        Precondition::MustNotMatch(f) => ("OPERATION_MUST_NOT_MATCH", f),
    };
    json!({"operation": operation, "filter": filter_json(f)})
}

fn user_subject(user: &str) -> Value {
    json!({"object": {"objectType": "user", "objectId": user}})
}

impl HttpAuthz {
    pub fn new(endpoint: &str, preshared_key: &str) -> Self {
        Self::with_timeouts(endpoint, preshared_key, READ_TIMEOUT, TOTAL_TIMEOUT)
    }

    /// [`HttpAuthz::new`] with explicit timeouts (tests use short ones).
    pub fn with_timeouts(
        endpoint: &str,
        preshared_key: &str,
        read_timeout: Duration,
        total_timeout: Duration,
    ) -> Self {
        Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            key: preshared_key.to_string(),
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(2))
                .timeout(total_timeout)
                // A pooled connection SpiceDB (or a proxy) silently dropped
                // is not reused after this long idle.
                .pool_idle_timeout(Duration::from_secs(30))
                .build()
                .expect("reqwest client"),
            read_timeout,
        }
    }

    /// POSTs and returns the raw body; maps transport/5xx to `Unavailable`
    /// and other non-2xx statuses to `Rejected`. Never retried.
    async fn post(&self, path: &str, body: Value) -> Result<String, AuthzError> {
        self.send(path, &body).await.map_err(AuthzError::from)
    }

    /// [`HttpAuthz::post`] for read-only calls: each attempt is bounded by
    /// `read_timeout`, and one that timed out or could not reach SpiceDB is
    /// retried once (a stuck pooled connection then costs one attempt, not
    /// the whole request).
    async fn post_read(&self, path: &str, body: Value) -> Result<String, AuthzError> {
        let mut last = String::new();
        for attempt in 0..2 {
            match tokio::time::timeout(self.read_timeout, self.send(path, &body)).await {
                Ok(Ok(text)) => return Ok(text),
                Ok(Err(PostError::Answered(e))) => return Err(e),
                Ok(Err(PostError::Transport(m))) => last = m,
                Err(_) => last = format!("no answer within {:?}", self.read_timeout),
            }
            if attempt == 0 {
                tracing::warn!("SpiceDB {path} failed ({last}); retrying once");
            }
        }
        Err(AuthzError::Unavailable(last))
    }

    async fn send(&self, path: &str, body: &Value) -> Result<String, PostError> {
        let resp = self
            .client
            .post(format!("{}{}", self.endpoint, path))
            .bearer_auth(&self.key)
            .json(body)
            .send()
            .await
            .map_err(|e| PostError::Transport(e.to_string()))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| PostError::Transport(e.to_string()))?;
        let answered = |e| Err(PostError::Answered(e));
        if status.is_success() {
            Ok(text)
        } else if serde_json::from_str::<Value>(&text)
            .is_ok_and(|v| v["code"] == json!(FAILED_PRECONDITION))
        {
            answered(AuthzError::PreconditionFailed(text))
        } else if status.is_server_error() || matches!(status.as_u16(), 401 | 403 | 429) {
            answered(AuthzError::Unavailable(format!("{status}: {text}")))
        } else {
            answered(AuthzError::Rejected(format!("{status}: {text}")))
        }
    }

    /// Parses a newline-delimited stream, returning each line's `result`.
    fn stream_results(body: &str) -> Result<Vec<Value>, AuthzError> {
        let mut out = Vec::new();
        for line in body.lines().filter(|l| !l.trim().is_empty()) {
            let v: Value = serde_json::from_str(line)
                .map_err(|e| AuthzError::Unavailable(format!("bad response: {e}")))?;
            if let Some(err) = v.get("error") {
                return Err(AuthzError::Unavailable(err.to_string()));
            }
            if let Some(r) = v.get("result") {
                out.push(r.clone());
            }
        }
        Ok(out)
    }

    async fn write_all(&self, operation: &str, rels: &[Relationship]) -> Result<(), AuthzError> {
        if rels.is_empty() {
            return Ok(());
        }
        let updates: Vec<Value> = rels
            .iter()
            .map(|r| json!({"operation": operation, "relationship": relationship_json(r)}))
            .collect();
        self.post("/v1/relationships/write", json!({"updates": updates}))
            .await
            .map(|_| ())
    }
}

#[async_trait]
impl Authz for HttpAuthz {
    async fn check(
        &self,
        resource: &ObjectRef,
        permission: &str,
        user: &str,
    ) -> Result<bool, AuthzError> {
        let body = json!({
            "consistency": {"fullyConsistent": true},
            "resource": object_json(resource),
            "permission": permission,
            "subject": user_subject(user),
        });
        let text = self.post_read("/v1/permissions/check", body).await?;
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| AuthzError::Unavailable(format!("bad response: {e}")))?;
        Ok(v["permissionship"] == "PERMISSIONSHIP_HAS_PERMISSION")
    }

    async fn lookup_resources(
        &self,
        kind: &'static str,
        permission: &str,
        user: &str,
    ) -> Result<Vec<String>, AuthzError> {
        let body = json!({
            "consistency": {"fullyConsistent": true},
            "resourceObjectType": kind,
            "permission": permission,
            "subject": user_subject(user),
        });
        let text = self.post_read("/v1/permissions/resources", body).await?;
        Ok(Self::stream_results(&text)?
            .iter()
            .filter(|r| r["permissionship"] != "LOOKUP_PERMISSIONSHIP_CONDITIONAL_PERMISSION")
            .filter_map(|r| r["resourceObjectId"].as_str().map(String::from))
            .collect())
    }

    async fn read(&self, filter: &RelationshipFilter) -> Result<Vec<Relationship>, AuthzError> {
        let body = json!({
            "consistency": {"fullyConsistent": true},
            "relationshipFilter": filter_json(filter),
        });
        let text = self.post_read("/v1/relationships/read", body).await?;
        let mut out = Vec::new();
        for r in Self::stream_results(&text)? {
            let rel = &r["relationship"];
            let get = |v: &Value, k: &str| v[k].as_str().map(String::from);
            let (Some(rk), Some(rid), Some(relation), Some(sk), Some(sid)) = (
                get(&rel["resource"], "objectType"),
                get(&rel["resource"], "objectId"),
                get(rel, "relation"),
                get(&rel["subject"]["object"], "objectType"),
                get(&rel["subject"]["object"], "objectId"),
            ) else {
                return Err(AuthzError::Unavailable(format!(
                    "malformed relationship row: {rel}"
                )));
            };
            out.push(Relationship {
                resource: ObjectRef {
                    kind: intern(&rk),
                    id: rid,
                },
                relation: intern(&relation),
                subject: ObjectRef {
                    kind: intern(&sk),
                    id: sid,
                },
            });
        }
        Ok(out)
    }

    async fn touch(&self, relationships: &[Relationship]) -> Result<(), AuthzError> {
        self.write_all("OPERATION_TOUCH", relationships).await
    }

    async fn delete(&self, relationships: &[Relationship]) -> Result<(), AuthzError> {
        self.write_all("OPERATION_DELETE", relationships).await
    }

    async fn delete_matching(&self, filter: &RelationshipFilter) -> Result<(), AuthzError> {
        self.post(
            "/v1/relationships/delete",
            json!({"relationshipFilter": filter_json(filter)}),
        )
        .await
        .map(|_| ())
    }

    async fn write(
        &self,
        updates: &[Update],
        preconditions: &[Precondition],
    ) -> Result<(), AuthzError> {
        if updates.is_empty() {
            return Ok(());
        }
        let updates: Vec<Value> = updates.iter().map(update_json).collect();
        let preconditions: Vec<Value> = preconditions.iter().map(precondition_json).collect();
        self.post(
            "/v1/relationships/write",
            json!({"updates": updates, "optionalPreconditions": preconditions}),
        )
        .await
        .map(|_| ())
    }

    async fn ensure_schema(&self, schema: &str) -> Result<(), AuthzError> {
        match self.post("/v1/schema/read", json!({})).await {
            Ok(text) => {
                let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                if v["schemaText"].as_str().map(str::trim) == Some(schema.trim()) {
                    return Ok(());
                }
            }
            // Only "No schema has been defined" means absent; anything else
            // is an error and must never be followed by a write.
            Err(AuthzError::Rejected(m)) | Err(AuthzError::Unavailable(m))
                if m.contains("No schema has been defined") => {}
            Err(e) => return Err(e),
        }
        self.post("/v1/schema/write", json!({"schema": schema}))
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A fake SpiceDB: the first `stall` connections are accepted and never
    /// answered; later ones answer every request with a permitted check.
    /// Returns its URL and the count of accepted connections.
    async fn mock_spicedb(stall: usize) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let accepted = Arc::new(AtomicUsize::new(0));
        let count = accepted.clone();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let n = count.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    if n < stall {
                        // Read and hold the connection open without answering.
                        while socket.read(&mut buf).await.unwrap_or(0) > 0 {}
                        return;
                    }
                    let _ = socket.read(&mut buf).await;
                    let body = r#"{"permissionship":"PERMISSIONSHIP_HAS_PERMISSION"}"#;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                         content-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });
        (url, accepted)
    }

    fn short(url: &str) -> HttpAuthz {
        HttpAuthz::with_timeouts(
            url,
            "key",
            Duration::from_millis(300),
            Duration::from_millis(600),
        )
    }

    #[tokio::test]
    async fn read_only_call_retries_a_stuck_attempt_once() {
        let (url, accepted) = mock_spicedb(1).await;
        let org = ObjectRef::new("organization", "o");
        let started = std::time::Instant::now();
        assert!(short(&url).check(&org, "admin", "u").await.unwrap());
        assert_eq!(accepted.load(Ordering::SeqCst), 2);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn read_only_call_gives_up_after_two_attempts() {
        let (url, accepted) = mock_spicedb(usize::MAX).await;
        let org = ObjectRef::new("organization", "o");
        let result = short(&url).check(&org, "admin", "u").await;
        assert!(
            matches!(result, Err(AuthzError::Unavailable(_))),
            "{result:?}"
        );
        assert_eq!(accepted.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn writes_are_never_retried() {
        let (url, accepted) = mock_spicedb(usize::MAX).await;
        let rel = Relationship {
            resource: ObjectRef::new("organization", "o"),
            relation: "owner",
            subject: ObjectRef::new("user", "u"),
        };
        let result = short(&url).touch(&[rel]).await;
        assert!(
            matches!(result, Err(AuthzError::Unavailable(_))),
            "{result:?}"
        );
        // Give a (wrong) retry the time to connect before counting.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(accepted.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_refused_connection_is_retried_once_then_unavailable() {
        // Nothing listens on this port any more.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let org = ObjectRef::new("organization", "o");
        let result = short(&url).check(&org, "admin", "u").await;
        assert!(
            matches!(result, Err(AuthzError::Unavailable(_))),
            "{result:?}"
        );
    }

    #[tokio::test]
    #[ignore]
    async fn http_authz_roundtrip() {
        let endpoint = std::env::var("SPICEDB_ENDPOINT").unwrap();
        let key = std::env::var("SPICEDB_PRESHARED_KEY").unwrap();
        let authz = HttpAuthz::new(&endpoint, &key);
        let schema = include_str!("../../rbac/schema.zed");
        authz.ensure_schema(schema).await.unwrap();
        let rel = Relationship {
            resource: ObjectRef::new("organization", "roundtrip-org"),
            relation: "owner",
            subject: ObjectRef::new("user", "roundtrip-user"),
        };
        authz.touch(std::slice::from_ref(&rel)).await.unwrap();
        let org = ObjectRef::new("organization", "roundtrip-org");
        assert!(authz.check(&org, "admin", "roundtrip-user").await.unwrap());
        let found = authz
            .lookup_resources("organization", "admin", "roundtrip-user")
            .await
            .unwrap();
        assert!(found.contains(&"roundtrip-org".to_string()));
        let read = authz
            .read(&RelationshipFilter {
                kind: "organization",
                id: Some("roundtrip-org".into()),
                relation: None,
                subject: None,
            })
            .await
            .unwrap();
        assert_eq!(read, vec![rel.clone()]);
        authz.delete(std::slice::from_ref(&rel)).await.unwrap();
        assert!(!authz.check(&org, "admin", "roundtrip-user").await.unwrap());

        // An atomic mixed write, then one whose precondition fails.
        let ws_rel = |relation| Relationship {
            resource: ObjectRef::new("workspace", "roundtrip-ws"),
            relation,
            subject: ObjectRef::new("user", "roundtrip-user"),
        };
        let ws_filter = |relation| RelationshipFilter {
            kind: "workspace",
            id: Some("roundtrip-ws".into()),
            relation: Some(relation),
            subject: Some(ObjectRef::new("user", "roundtrip-user")),
        };
        let ws = ObjectRef::new("workspace", "roundtrip-ws");
        authz.touch(&[ws_rel("editor")]).await.unwrap();
        authz
            .write(
                &[
                    Update::Delete(ws_rel("editor")),
                    Update::Touch(ws_rel("viewer")),
                ],
                &[Precondition::MustMatch(ws_filter("editor"))],
            )
            .await
            .unwrap();
        assert!(authz.check(&ws, "view", "roundtrip-user").await.unwrap());
        assert!(!authz.check(&ws, "edit", "roundtrip-user").await.unwrap());
        let failed = authz
            .write(
                &[
                    Update::Delete(ws_rel("viewer")),
                    Update::Touch(ws_rel("owner")),
                ],
                &[Precondition::MustMatch(ws_filter("editor"))],
            )
            .await;
        assert!(
            matches!(failed, Err(AuthzError::PreconditionFailed(_))),
            "{failed:?}"
        );
        // Nothing of the failed write was applied.
        assert!(authz.check(&ws, "view", "roundtrip-user").await.unwrap());
        assert!(!authz.check(&ws, "manage", "roundtrip-user").await.unwrap());
        authz.delete(&[ws_rel("viewer")]).await.unwrap();
    }
}
