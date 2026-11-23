//! The Cube data model's files, kept in the RBAC database. Every write goes
//! through [`Store::model_write`]: one transaction that locks the model
//! version, lets the caller plan the changes against the current rows, applies
//! them and bumps the version.

use chrono::{DateTime, Utc};
use tokio_postgres::Row;
use uuid::Uuid;

use super::store::{Store, StoreError};

#[derive(Debug, Clone)]
pub struct ModelFile {
    pub id: Uuid,
    pub organization_id: Option<Uuid>,
    pub path: String,
    pub content: String,
    pub version: i32,
    pub managed_by: Option<String>,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,
}

#[derive(Debug, Clone)]
pub struct ModelRows {
    pub version: i64,
    pub files: Vec<ModelFile>,
}

#[derive(Debug, Clone)]
pub enum ModelChange {
    /// `id: None` creates the file.
    Put {
        id: Option<Uuid>,
        organization_id: Option<Uuid>,
        path: String,
        content: String,
        expected_version: Option<i32>,
        managed_by: Option<String>,
    },
    Delete {
        id: Uuid,
        expected_version: Option<i32>,
    },
    Move {
        id: Uuid,
        organization_id: Option<Uuid>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteOutcome {
    pub version: i64,
    pub changed: bool,
}

const FILE_COLS: &str = "id, organization_id, path, content, version, managed_by, \
     created_at, created_by, updated_at, updated_by";

fn file_from(r: &Row) -> ModelFile {
    ModelFile {
        id: r.get(0),
        organization_id: r.get(1),
        path: r.get(2),
        content: r.get(3),
        version: r.get(4),
        managed_by: r.get(5),
        created_at: r.get(6),
        created_by: r.get(7),
        updated_at: r.get(8),
        updated_by: r.get(9),
    }
}

/// A repeated path hits the unique index; the routes want a plain "path".
fn path_conflict(e: tokio_postgres::Error) -> StoreError {
    match StoreError::from(e) {
        StoreError::Conflict(c) if c == "model_files_path_idx" => {
            StoreError::Conflict("path".into())
        }
        other => other,
    }
}

impl Store {
    pub async fn model_rows(&self) -> Result<ModelRows, StoreError> {
        let mut c = self.conn().await?;
        // One snapshot for the version and the rows.
        let tx = c
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .read_only(true)
            .start()
            .await?;
        let version: i64 = tx
            .query_one("SELECT version FROM model_state", &[])
            .await?
            .get(0);
        let sql = format!(
            "SELECT {FILE_COLS} FROM model_files ORDER BY organization_id NULLS FIRST, path"
        );
        let files = tx.query(&sql, &[]).await?.iter().map(file_from).collect();
        tx.commit().await?;
        Ok(ModelRows { version, files })
    }

    /// Every organization's slug, by id.
    pub async fn organization_slugs(
        &self,
    ) -> Result<std::collections::HashMap<Uuid, String>, StoreError> {
        let c = self.conn().await?;
        Ok(c
            .query("SELECT id, slug FROM organizations WHERE slug IS NOT NULL", &[])
            .await?
            .iter()
            .map(|r| (r.get(0), r.get(1)))
            .collect())
    }

    pub async fn model_version(&self) -> Result<i64, StoreError> {
        let c = self.conn().await?;
        Ok(c.query_one("SELECT version FROM model_state", &[])
            .await?
            .get(0))
    }

    pub async fn model_file(&self, id: Uuid) -> Result<Option<ModelFile>, StoreError> {
        let c = self.conn().await?;
        let sql = format!("SELECT {FILE_COLS} FROM model_files WHERE id = $1");
        Ok(c.query_opt(&sql, &[&id]).await?.as_ref().map(file_from))
    }

    pub async fn model_is_empty(&self) -> Result<bool, StoreError> {
        let c = self.conn().await?;
        Ok(!c
            .query_one("SELECT EXISTS (SELECT 1 FROM model_files)", &[])
            .await?
            .get::<_, bool>(0))
    }

    /// Plans and applies a model change atomically. `plan` sees the current
    /// rows (the version row is locked, so writers are serialized) and
    /// returns the changes, or an error that rolls everything back. An empty
    /// list changes nothing and leaves the version alone. A change against a
    /// file whose version moved on is `Conflict("stale")`.
    pub async fn model_write<F, E>(
        &self,
        actor: &str,
        plan: F,
    ) -> Result<Result<WriteOutcome, E>, StoreError>
    where
        F: FnOnce(&ModelRows) -> Result<Vec<ModelChange>, E>,
    {
        let mut c = self.conn().await?;
        let tx = c.transaction().await?;
        let version: i64 = tx
            .query_one("SELECT version FROM model_state FOR UPDATE", &[])
            .await?
            .get(0);
        let sql = format!(
            "SELECT {FILE_COLS} FROM model_files ORDER BY organization_id NULLS FIRST, path"
        );
        let files = tx.query(&sql, &[]).await?.iter().map(file_from).collect();
        let rows = ModelRows { version, files };
        let changes = match plan(&rows) {
            Ok(changes) => changes,
            Err(e) => return Ok(Err(e)), // `tx` rolls back on drop
        };
        if changes.is_empty() {
            return Ok(Ok(WriteOutcome {
                version,
                changed: false,
            }));
        }
        for change in &changes {
            let n = match change {
                ModelChange::Put {
                    id: None,
                    organization_id,
                    path,
                    content,
                    managed_by,
                    ..
                } => tx
                    .execute(
                        "INSERT INTO model_files (id, organization_id, path, content, managed_by, \
                         created_by, updated_by) VALUES ($1, $2, $3, $4, $5, $6, $6)",
                        &[&Uuid::new_v4(), organization_id, path, content, managed_by, &actor],
                    )
                    .await
                    .map_err(path_conflict)?,
                ModelChange::Put {
                    id: Some(id),
                    organization_id,
                    path,
                    content,
                    expected_version,
                    managed_by,
                } => tx
                    .execute(
                        "UPDATE model_files SET organization_id = $2, path = $3, content = $4, \
                         managed_by = $5, version = version + 1, updated_at = now(), updated_by = $6 \
                         WHERE id = $1 AND ($7::int IS NULL OR version = $7)",
                        &[id, organization_id, path, content, managed_by, &actor, expected_version],
                    )
                    .await
                    .map_err(path_conflict)?,
                ModelChange::Delete { id, expected_version } => tx
                    .execute(
                        "DELETE FROM model_files WHERE id = $1 AND ($2::int IS NULL OR version = $2)",
                        &[id, expected_version],
                    )
                    .await?,
                ModelChange::Move { id, organization_id } => tx
                    .execute(
                        "UPDATE model_files SET organization_id = $2, version = version + 1, \
                         updated_at = now(), updated_by = $3 WHERE id = $1",
                        &[id, organization_id, &actor],
                    )
                    .await
                    .map_err(path_conflict)?,
            };
            if n == 0 {
                return Err(StoreError::Conflict("stale".into()));
            }
        }
        let version: i64 = tx
            .query_one(
                "UPDATE model_state SET version = version + 1 RETURNING version",
                &[],
            )
            .await?
            .get(0);
        tx.commit().await?;
        Ok(Ok(WriteOutcome {
            version,
            changed: true,
        }))
    }
}
