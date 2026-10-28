//! `Authz` trait and its SpiceDB HTTP implementation.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

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

#[derive(Debug)]
pub enum AuthzError {
    /// SpiceDB unreachable or failing (maps to 503).
    Unavailable(String),
    /// SpiceDB rejected the request (validation error).
    Rejected(String),
}

impl std::fmt::Display for AuthzError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthzError::Unavailable(m) => write!(f, "authorization unavailable: {m}"),
            AuthzError::Rejected(m) => write!(f, "authorization rejected: {m}"),
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
}

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

fn user_subject(user: &str) -> Value {
    json!({"object": {"objectType": "user", "objectId": user}})
}

impl HttpAuthz {
    pub fn new(endpoint: &str, preshared_key: &str) -> Self {
        Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            key: preshared_key.to_string(),
            client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(2))
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("reqwest client"),
        }
    }

    /// POSTs and returns the raw body; maps transport/5xx to `Unavailable`
    /// and other non-2xx statuses to `Rejected`.
    async fn post(&self, path: &str, body: Value) -> Result<String, AuthzError> {
        let resp = self
            .client
            .post(format!("{}{}", self.endpoint, path))
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await
            .map_err(|e| AuthzError::Unavailable(e.to_string()))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| AuthzError::Unavailable(e.to_string()))?;
        if status.is_success() {
            Ok(text)
        } else if status.is_server_error() || matches!(status.as_u16(), 401 | 403 | 429) {
            Err(AuthzError::Unavailable(format!("{status}: {text}")))
        } else {
            Err(AuthzError::Rejected(format!("{status}: {text}")))
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

    async fn write(&self, operation: &str, rels: &[Relationship]) -> Result<(), AuthzError> {
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
        let text = self.post("/v1/permissions/check", body).await?;
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
        let text = self.post("/v1/permissions/resources", body).await?;
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
        let text = self.post("/v1/relationships/read", body).await?;
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
        self.write("OPERATION_TOUCH", relationships).await
    }

    async fn delete(&self, relationships: &[Relationship]) -> Result<(), AuthzError> {
        self.write("OPERATION_DELETE", relationships).await
    }

    async fn delete_matching(&self, filter: &RelationshipFilter) -> Result<(), AuthzError> {
        self.post(
            "/v1/relationships/delete",
            json!({"relationshipFilter": filter_json(filter)}),
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
    }
}
