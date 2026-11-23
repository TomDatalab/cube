//! The one path of every model write: lock the model version, build the
//! sources before and after the change, refuse a change that adds problems,
//! commit, then reload what serves the model.

use std::sync::Arc;

use axum::http::StatusCode;
use uuid::Uuid;

use super::model_snapshot::sources;
use super::model_store::{ModelChange, ModelFile, ModelRows};
use super::model_runtime::ModelRuntime;
use super::model_validate::{check, fixed_count, new_problems, Problem};
use super::store::{Store, StoreError};
use crate::error::ApiError;
use crate::tenants::TenantRegistry;

#[derive(Debug, Clone)]
pub struct WriteError {
    pub status: StatusCode,
    pub message: String,
    pub problems: Vec<Problem>,
    /// Cube and view names of the candidate model, with their source files:
    /// what `model_redact::redact` needs.
    pub defs: std::collections::BTreeMap<String, String>,
}

impl WriteError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            problems: Vec::new(),
            defs: Default::default(),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }
}

impl From<WriteError> for ApiError {
    fn from(e: WriteError) -> Self {
        let problems = serde_json::to_value(&e.problems).unwrap_or_default();
        ApiError::new(e.status, e.message).with_problems(problems)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    pub version: i64,
    pub changed: bool,
    /// Problems of the old model that this change removed.
    pub fixed: usize,
}

pub struct ModelWriter {
    store: Store,
    runtime: Arc<ModelRuntime>,
    tenants: Arc<TenantRegistry>,
    ctx: cubemodel::TemplateContext,
    lock: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for ModelWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelWriter").finish_non_exhaustive()
    }
}

/// The rows as they will be once `changes` are applied, mirroring what
/// `Store::model_write` writes.
pub fn apply_in_memory(rows: &ModelRows, changes: &[ModelChange]) -> ModelRows {
    let mut out = rows.clone();
    for change in changes {
        match change {
            ModelChange::Put {
                id: None,
                organization_id,
                path,
                content,
                managed_by,
                ..
            } => {
                let now = chrono::Utc::now();
                out.files.push(ModelFile {
                    id: Uuid::new_v4(),
                    organization_id: *organization_id,
                    path: path.clone(),
                    content: content.clone(),
                    version: 1,
                    managed_by: managed_by.clone(),
                    created_at: now,
                    created_by: String::new(),
                    updated_at: now,
                    updated_by: String::new(),
                });
            }
            ModelChange::Put {
                id: Some(id),
                organization_id,
                path,
                content,
                managed_by,
                ..
            } => {
                if let Some(f) = out.files.iter_mut().find(|f| f.id == *id) {
                    f.organization_id = *organization_id;
                    f.path = path.clone();
                    f.content = content.clone();
                    f.managed_by = managed_by.clone();
                    f.version += 1;
                }
            }
            ModelChange::Delete { id, .. } => out.files.retain(|f| f.id != *id),
            ModelChange::Move { id, organization_id } => {
                if let Some(f) = out.files.iter_mut().find(|f| f.id == *id) {
                    f.organization_id = *organization_id;
                    f.version += 1;
                }
            }
        }
    }
    out
}

impl ModelWriter {
    pub fn new(
        store: Store,
        runtime: Arc<ModelRuntime>,
        tenants: Arc<TenantRegistry>,
        ctx: cubemodel::TemplateContext,
    ) -> Self {
        Self {
            store,
            runtime,
            tenants,
            ctx,
            lock: tokio::sync::Mutex::new(()),
        }
    }

    pub async fn write(
        &self,
        actor: &str,
        what: &str,
        changes: impl FnOnce(&ModelRows) -> Result<Vec<ModelChange>, WriteError>,
    ) -> Result<Written, WriteError> {
        let _guard = self.lock.lock().await;
        let slugs = self.store.organization_slugs().await.map_err(internal)?;
        let policy = cubemodel::ownership::current_policy();
        let mut fixed = 0usize;
        let outcome = self
            .store
            .model_write(actor, |rows| {
                let changes = changes(rows)?;
                if changes.is_empty() {
                    return Ok(changes);
                }
                for change in &changes {
                    let org = match change {
                        ModelChange::Put { organization_id, .. }
                        | ModelChange::Move { organization_id, .. } => *organization_id,
                        ModelChange::Delete { .. } => None,
                    };
                    if org.is_some_and(|o| !slugs.contains_key(&o)) {
                        return Err(WriteError::bad_request("Organização desconhecida"));
                    }
                }
                let before = check(&sources(rows, &slugs), &self.ctx, policy.as_deref());
                let after_rows = apply_in_memory(rows, &changes);
                let after_sources = sources(&after_rows, &slugs);
                let after = check(&after_sources, &self.ctx, policy.as_deref());
                let problems = new_problems(&before, &after);
                if !problems.is_empty() {
                    return Err(WriteError {
                        status: StatusCode::BAD_REQUEST,
                        message: "A mudança não compila".into(),
                        problems,
                        defs: crate::rbac::model_validate::definitions(&after_sources),
                    });
                }
                fixed = fixed_count(&before, &after);
                Ok(changes)
            })
            .await;
        let outcome = match outcome {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(e)) => return Err(e),
            Err(StoreError::Conflict(c)) if c == "stale" => {
                return Err(WriteError::new(
                    StatusCode::CONFLICT,
                    "O arquivo mudou desde que você abriu; abra de novo",
                ))
            }
            Err(StoreError::Conflict(c)) if c == "path" => {
                return Err(WriteError::bad_request(
                    "Já existe um arquivo com esse caminho",
                ))
            }
            Err(e) => return Err(internal(e)),
        };
        if outcome.changed {
            // The write is committed; a failed reload is picked up by the loop.
            if let Err(e) = self.runtime.refresh().await {
                tracing::error!(error = %e, "model refresh after a write failed");
            }
            for (tenant, err) in self.tenants.reload_all().await {
                tracing::error!(%tenant, error = %err.body.error, "model reload after a write failed");
            }
            tracing::info!(actor, what, version = outcome.version, "model change");
        }
        Ok(Written {
            version: outcome.version,
            changed: outcome.changed,
            fixed,
        })
    }
}

fn internal(e: StoreError) -> WriteError {
    tracing::error!(error = %e, "model write failed");
    WriteError::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(org: Option<Uuid>, path: &str, content: &str) -> ModelFile {
        let now = chrono::Utc::now();
        ModelFile {
            id: Uuid::new_v4(),
            organization_id: org,
            path: path.into(),
            content: content.into(),
            version: 1,
            managed_by: None,
            created_at: now,
            created_by: "x".into(),
            updated_at: now,
            updated_by: "x".into(),
        }
    }

    #[test]
    fn applies_put_delete_and_move() {
        let (o1, o2) = (Uuid::new_v4(), Uuid::new_v4());
        let (a, b, c) = (file(Some(o1), "a.yml", "A"), file(Some(o1), "b.yml", "B"), file(None, "c.yml", "C"));
        let rows = ModelRows {
            version: 3,
            files: vec![a.clone(), b.clone(), c.clone()],
        };
        let out = apply_in_memory(
            &rows,
            &[
                ModelChange::Put {
                    id: None,
                    organization_id: Some(o2),
                    path: "n.yml".into(),
                    content: "N".into(),
                    expected_version: None,
                    managed_by: None,
                },
                ModelChange::Put {
                    id: Some(a.id),
                    organization_id: Some(o1),
                    path: "a2.yml".into(),
                    content: "A2".into(),
                    expected_version: Some(1),
                    managed_by: None,
                },
                ModelChange::Delete { id: b.id, expected_version: None },
                ModelChange::Move { id: c.id, organization_id: Some(o2) },
            ],
        );
        assert_eq!(out.files.len(), 3);
        let by = |p: &str| out.files.iter().find(|f| f.path == p);
        assert_eq!(by("n.yml").unwrap().organization_id, Some(o2));
        let a2 = by("a2.yml").unwrap();
        assert_eq!((a2.id, a2.content.as_str(), a2.version), (a.id, "A2", 2));
        assert!(by("b.yml").is_none() && by("a.yml").is_none());
        let moved = by("c.yml").unwrap();
        assert_eq!((moved.organization_id, moved.version), (Some(o2), 2));
        // The input is untouched.
        assert_eq!(rows.files.len(), 3);
    }
}
