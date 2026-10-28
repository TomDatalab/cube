//! Postgres (`cube_app`) storage for users, organizations, workspaces,
//! connections and objects. Roles and grants live in SpiceDB, not here.

use chrono::{DateTime, Utc};
use deadpool_postgres::{Manager, ManagerConfig, Object, Pool, RecyclingMethod};
use tokio_postgres::error::SqlState;
use tokio_postgres::{NoTls, Row};
use uuid::Uuid;

const MIGRATIONS: &str = include_str!("migrations.sql");

#[derive(Debug)]
pub enum StoreError {
    Conflict(String),
    NotFound,
    Db(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict(m) => write!(f, "conflict: {m}"),
            Self::NotFound => write!(f, "not found"),
            Self::Db(m) => write!(f, "database error: {m}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<tokio_postgres::Error> for StoreError {
    fn from(e: tokio_postgres::Error) -> Self {
        match e.code() {
            Some(c) if *c == SqlState::UNIQUE_VIOLATION => Self::Conflict(
                e.as_db_error()
                    .and_then(|d| d.constraint())
                    .unwrap_or("unique violation")
                    .to_string(),
            ),
            _ => Self::Db(e.to_string()),
        }
    }
}

impl From<deadpool_postgres::PoolError> for StoreError {
    fn from(e: deadpool_postgres::PoolError) -> Self {
        Self::Db(e.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct NewUser {
    pub id: String,
    pub name: Option<String>,
    pub email: Option<String>,
    pub password_hash: String,
}

#[derive(Debug, Clone)]
pub struct User {
    pub id: String,
    pub name: Option<String>,
    pub email: Option<String>,
    pub password_hash: String,
    pub disabled: bool,
    pub created_at: DateTime<Utc>,
    pub password_changed_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct Organization {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
}

#[derive(Debug, Clone)]
pub struct Workspace {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
}

#[derive(Debug, Clone)]
pub struct ConnectionRow {
    pub name: String,
    pub organization_id: Uuid,
}

#[derive(Debug, Clone)]
pub struct NewObject {
    pub workspace_id: Uuid,
    pub r#type: String,
    pub name: String,
    pub content_type: String,
    pub content: Vec<u8>,
    pub created_by: String,
}

#[derive(Debug, Clone)]
pub struct StoredObject {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub r#type: String,
    pub name: String,
    pub content_type: String,
    pub content: Vec<u8>,
    pub version: i32,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,
}

const USER_COLS: &str = "id, name, email, password_hash, disabled, created_at, password_changed_at";
const OBJECT_COLS: &str = "id, workspace_id, type, name, content_type, content, version, \
                           created_at, created_by, updated_at, updated_by";

fn user_from(r: &Row) -> User {
    User {
        id: r.get(0),
        name: r.get(1),
        email: r.get(2),
        password_hash: r.get(3),
        disabled: r.get(4),
        created_at: r.get(5),
        password_changed_at: r.get(6),
    }
}

fn org_from(r: &Row) -> Organization {
    Organization {
        id: r.get(0),
        name: r.get(1),
        created_at: r.get(2),
        created_by: r.get(3),
    }
}

fn workspace_from(r: &Row) -> Workspace {
    Workspace {
        id: r.get(0),
        organization_id: r.get(1),
        name: r.get(2),
        created_at: r.get(3),
        created_by: r.get(4),
    }
}

fn object_from(r: &Row) -> StoredObject {
    StoredObject {
        id: r.get(0),
        workspace_id: r.get(1),
        r#type: r.get(2),
        name: r.get(3),
        content_type: r.get(4),
        content: r.get(5),
        version: r.get(6),
        created_at: r.get(7),
        created_by: r.get(8),
        updated_at: r.get(9),
        updated_by: r.get(10),
    }
}

/// The queries, shared by [`Store`] (pooled connection) and [`StoreTx`].
mod q {
    use super::*;

    pub async fn create_user(c: &tokio_postgres::Client, u: &NewUser) -> Result<User, StoreError> {
        let sql = format!(
            "INSERT INTO users (id, name, email, password_hash) VALUES ($1, $2, $3, $4) \
             RETURNING {USER_COLS}"
        );
        let row = c
            .query_one(&sql, &[&u.id, &u.name, &u.email, &u.password_hash])
            .await?;
        Ok(user_from(&row))
    }

    pub async fn create_organization(
        c: &tokio_postgres::Client,
        name: &str,
        created_by: &str,
    ) -> Result<Organization, StoreError> {
        let row = c
            .query_one(
                "INSERT INTO organizations (id, name, created_by) VALUES ($1, $2, $3) \
                 RETURNING id, name, created_at, created_by",
                &[&Uuid::new_v4(), &name, &created_by],
            )
            .await?;
        Ok(org_from(&row))
    }

    pub async fn create_workspace(
        c: &tokio_postgres::Client,
        organization_id: Uuid,
        name: &str,
        created_by: &str,
    ) -> Result<Workspace, StoreError> {
        let row = c
            .query_one(
                "INSERT INTO workspaces (id, organization_id, name, created_by) \
                 VALUES ($1, $2, $3, $4) \
                 RETURNING id, organization_id, name, created_at, created_by",
                &[&Uuid::new_v4(), &organization_id, &name, &created_by],
            )
            .await?;
        Ok(workspace_from(&row))
    }

    pub async fn create_connection(
        c: &tokio_postgres::Client,
        name: &str,
        organization_id: Uuid,
    ) -> Result<ConnectionRow, StoreError> {
        c.execute(
            "INSERT INTO connections (name, organization_id) VALUES ($1, $2)",
            &[&name, &organization_id],
        )
        .await?;
        Ok(ConnectionRow {
            name: name.to_string(),
            organization_id,
        })
    }

    pub async fn create_object(
        c: &tokio_postgres::Client,
        o: &NewObject,
    ) -> Result<StoredObject, StoreError> {
        let sql = format!(
            "INSERT INTO objects (id, workspace_id, type, name, content_type, content, \
             created_by, updated_by) VALUES ($1, $2, $3, $4, $5, $6, $7, $7) \
             RETURNING {OBJECT_COLS}"
        );
        let row = c
            .query_one(
                &sql,
                &[
                    &Uuid::new_v4(),
                    &o.workspace_id,
                    &o.r#type,
                    &o.name,
                    &o.content_type,
                    &o.content,
                    &o.created_by,
                ],
            )
            .await?;
        Ok(object_from(&row))
    }
}

/// Expands to the same method set on [`Store`] and [`StoreTx`]; `$conn` is the
/// expression yielding a `GenericClient` inside each method.
macro_rules! shared_creates {
    ($conn:expr) => {
        pub async fn create_user(&self, u: &NewUser) -> Result<User, StoreError> {
            let guard = $conn(self).await?;
            let c: &tokio_postgres::Client = &guard;
            q::create_user(c, u).await
        }
        pub async fn create_organization(
            &self,
            name: &str,
            created_by: &str,
        ) -> Result<Organization, StoreError> {
            let guard = $conn(self).await?;
            let c: &tokio_postgres::Client = &guard;
            q::create_organization(c, name, created_by).await
        }
        pub async fn create_workspace(
            &self,
            organization_id: Uuid,
            name: &str,
            created_by: &str,
        ) -> Result<Workspace, StoreError> {
            let guard = $conn(self).await?;
            let c: &tokio_postgres::Client = &guard;
            q::create_workspace(c, organization_id, name, created_by).await
        }
        pub async fn create_connection(
            &self,
            name: &str,
            organization_id: Uuid,
        ) -> Result<ConnectionRow, StoreError> {
            let guard = $conn(self).await?;
            let c: &tokio_postgres::Client = &guard;
            q::create_connection(c, name, organization_id).await
        }
        pub async fn create_object(&self, o: &NewObject) -> Result<StoredObject, StoreError> {
            let guard = $conn(self).await?;
            let c: &tokio_postgres::Client = &guard;
            q::create_object(c, o).await
        }
    };
}

#[derive(Clone)]
pub struct Store {
    pool: Pool,
}

impl Store {
    /// Connects and applies the (idempotent) migrations.
    pub async fn connect(url: &str) -> Result<Self, StoreError> {
        Self::connect_with_pool_size(url, 8).await
    }

    /// Like [`Store::connect`] with an explicit pool size (tests use 1).
    pub async fn connect_with_pool_size(url: &str, max_size: usize) -> Result<Self, StoreError> {
        let config: tokio_postgres::Config = url
            .parse()
            .map_err(|e: tokio_postgres::Error| StoreError::Db(e.to_string()))?;
        let manager = Manager::from_config(
            config,
            NoTls,
            ManagerConfig {
                recycling_method: RecyclingMethod::Fast,
            },
        );
        let pool = Pool::builder(manager)
            .max_size(max_size)
            .build()
            .map_err(|e| StoreError::Db(e.to_string()))?;
        let store = Self { pool };
        let c = store.pool.get().await?;
        // Serialize concurrent starts: CREATE TABLE IF NOT EXISTS can race.
        c.batch_execute("SELECT pg_advisory_lock(7263541)").await?;
        let applied = c.batch_execute(MIGRATIONS).await;
        if let Err(e) = c.batch_execute("SELECT pg_advisory_unlock(7263541)").await {
            // Still holding the lock: close the connection instead of pooling it.
            let _ = Object::take(c);
            return Err(e.into());
        }
        applied?;
        drop(c);
        Ok(store)
    }

    async fn conn(&self) -> Result<Object, StoreError> {
        Ok(self.pool.get().await?)
    }

    shared_creates!(Self::conn);

    pub async fn user(&self, id: &str) -> Result<Option<User>, StoreError> {
        let c = self.conn().await?;
        let sql = format!("SELECT {USER_COLS} FROM users WHERE id = $1");
        Ok(c.query_opt(&sql, &[&id]).await?.as_ref().map(user_from))
    }

    pub async fn users(&self) -> Result<Vec<User>, StoreError> {
        let c = self.conn().await?;
        let sql = format!("SELECT {USER_COLS} FROM users ORDER BY id");
        Ok(c.query(&sql, &[]).await?.iter().map(user_from).collect())
    }

    /// `None` fields are left unchanged. `Ok(None)` when the user is unknown.
    pub async fn update_user(
        &self,
        id: &str,
        name: Option<String>,
        email: Option<String>,
        disabled: Option<bool>,
    ) -> Result<Option<User>, StoreError> {
        let c = self.conn().await?;
        let sql = format!(
            "UPDATE users SET name = COALESCE($2, name), email = COALESCE($3, email), \
             disabled = COALESCE($4, disabled) WHERE id = $1 RETURNING {USER_COLS}"
        );
        Ok(c.query_opt(&sql, &[&id, &name, &email, &disabled])
            .await?
            .as_ref()
            .map(user_from))
    }

    /// Also bumps `password_changed_at`, invalidating older tokens.
    pub async fn set_password_hash(&self, id: &str, hash: &str) -> Result<(), StoreError> {
        let c = self.conn().await?;
        let n = c
            .execute(
                "UPDATE users SET password_hash = $2, password_changed_at = now() WHERE id = $1",
                &[&id, &hash],
            )
            .await?;
        if n == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    // organizations
    pub async fn organization(&self, id: Uuid) -> Result<Option<Organization>, StoreError> {
        let c = self.conn().await?;
        let row = c
            .query_opt(
                "SELECT id, name, created_at, created_by FROM organizations WHERE id = $1",
                &[&id],
            )
            .await?;
        Ok(row.as_ref().map(org_from))
    }

    pub async fn organizations_by_ids(
        &self,
        ids: &[Uuid],
    ) -> Result<Vec<Organization>, StoreError> {
        let c = self.conn().await?;
        let rows = c
            .query(
                "SELECT id, name, created_at, created_by FROM organizations \
                 WHERE id = ANY($1) ORDER BY name",
                &[&ids],
            )
            .await?;
        Ok(rows.iter().map(org_from).collect())
    }

    pub async fn update_organization(
        &self,
        id: Uuid,
        name: &str,
    ) -> Result<Option<Organization>, StoreError> {
        let c = self.conn().await?;
        let row = c
            .query_opt(
                "UPDATE organizations SET name = $2 WHERE id = $1 \
                 RETURNING id, name, created_at, created_by",
                &[&id, &name],
            )
            .await?;
        Ok(row.as_ref().map(org_from))
    }

    /// `true` when a row was deleted. Fails with `Conflict` while workspaces
    /// or connections still reference the organization.
    pub async fn delete_organization(&self, id: Uuid) -> Result<bool, StoreError> {
        let c = self.conn().await?;
        match c
            .execute("DELETE FROM organizations WHERE id = $1", &[&id])
            .await
        {
            Ok(n) => Ok(n > 0),
            Err(e) if e.code() == Some(&SqlState::FOREIGN_KEY_VIOLATION) => Err(
                StoreError::Conflict("organization still has workspaces or connections".into()),
            ),
            Err(e) => Err(e.into()),
        }
    }

    // workspaces
    pub async fn workspace(&self, id: Uuid) -> Result<Option<Workspace>, StoreError> {
        let c = self.conn().await?;
        let row = c
            .query_opt(
                "SELECT id, organization_id, name, created_at, created_by \
                 FROM workspaces WHERE id = $1",
                &[&id],
            )
            .await?;
        Ok(row.as_ref().map(workspace_from))
    }

    pub async fn workspaces_by_ids(&self, ids: &[Uuid]) -> Result<Vec<Workspace>, StoreError> {
        let c = self.conn().await?;
        let rows = c
            .query(
                "SELECT id, organization_id, name, created_at, created_by \
                 FROM workspaces WHERE id = ANY($1) ORDER BY name",
                &[&ids],
            )
            .await?;
        Ok(rows.iter().map(workspace_from).collect())
    }

    pub async fn workspaces_of_organization(
        &self,
        organization_id: Uuid,
    ) -> Result<Vec<Workspace>, StoreError> {
        let c = self.conn().await?;
        let rows = c
            .query(
                "SELECT id, organization_id, name, created_at, created_by \
                 FROM workspaces WHERE organization_id = $1 ORDER BY name",
                &[&organization_id],
            )
            .await?;
        Ok(rows.iter().map(workspace_from).collect())
    }

    pub async fn update_workspace(
        &self,
        id: Uuid,
        name: &str,
    ) -> Result<Option<Workspace>, StoreError> {
        let c = self.conn().await?;
        let row = c
            .query_opt(
                "UPDATE workspaces SET name = $2 WHERE id = $1 \
                 RETURNING id, organization_id, name, created_at, created_by",
                &[&id, &name],
            )
            .await?;
        Ok(row.as_ref().map(workspace_from))
    }

    /// Cascades to the workspace's objects. `true` when a row was deleted.
    pub async fn delete_workspace(&self, id: Uuid) -> Result<bool, StoreError> {
        let c = self.conn().await?;
        Ok(c.execute("DELETE FROM workspaces WHERE id = $1", &[&id])
            .await?
            > 0)
    }

    // connections
    pub async fn connection(&self, name: &str) -> Result<Option<ConnectionRow>, StoreError> {
        let c = self.conn().await?;
        let row = c
            .query_opt(
                "SELECT name, organization_id FROM connections WHERE name = $1",
                &[&name],
            )
            .await?;
        Ok(row.map(|r| ConnectionRow {
            name: r.get(0),
            organization_id: r.get(1),
        }))
    }

    pub async fn connections_by_names(
        &self,
        names: &[String],
    ) -> Result<Vec<ConnectionRow>, StoreError> {
        let c = self.conn().await?;
        let rows = c
            .query(
                "SELECT name, organization_id FROM connections WHERE name = ANY($1) ORDER BY name",
                &[&names],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| ConnectionRow {
                name: r.get(0),
                organization_id: r.get(1),
            })
            .collect())
    }

    pub async fn connections_of_organization(
        &self,
        organization_id: Uuid,
    ) -> Result<Vec<ConnectionRow>, StoreError> {
        let c = self.conn().await?;
        let rows = c
            .query(
                "SELECT name, organization_id FROM connections \
                 WHERE organization_id = $1 ORDER BY name",
                &[&organization_id],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| ConnectionRow {
                name: r.get(0),
                organization_id: r.get(1),
            })
            .collect())
    }

    /// Reassigns a connection to another organization.
    pub async fn update_connection(
        &self,
        name: &str,
        organization_id: Uuid,
    ) -> Result<Option<ConnectionRow>, StoreError> {
        let c = self.conn().await?;
        let n = c
            .execute(
                "UPDATE connections SET organization_id = $2 WHERE name = $1",
                &[&name, &organization_id],
            )
            .await?;
        Ok((n > 0).then(|| ConnectionRow {
            name: name.to_string(),
            organization_id,
        }))
    }

    pub async fn delete_connection(&self, name: &str) -> Result<bool, StoreError> {
        let c = self.conn().await?;
        Ok(
            c.execute("DELETE FROM connections WHERE name = $1", &[&name])
                .await?
                > 0,
        )
    }

    // objects
    pub async fn object(&self, id: Uuid) -> Result<Option<StoredObject>, StoreError> {
        let c = self.conn().await?;
        let sql = format!("SELECT {OBJECT_COLS} FROM objects WHERE id = $1");
        Ok(c.query_opt(&sql, &[&id]).await?.as_ref().map(object_from))
    }

    pub async fn objects_by_ids(&self, ids: &[Uuid]) -> Result<Vec<StoredObject>, StoreError> {
        let c = self.conn().await?;
        let sql = format!("SELECT {OBJECT_COLS} FROM objects WHERE id = ANY($1) ORDER BY name");
        Ok(c.query(&sql, &[&ids])
            .await?
            .iter()
            .map(object_from)
            .collect())
    }

    /// Optimistic update: succeeds only when the stored version equals
    /// `expected_version`, then increments it. `Conflict` on a stale version,
    /// `NotFound` when the object does not exist. `None` fields are kept.
    pub async fn update_object(
        &self,
        id: Uuid,
        expected_version: i32,
        name: Option<String>,
        content_type: Option<String>,
        content: Option<Vec<u8>>,
        updated_by: &str,
    ) -> Result<StoredObject, StoreError> {
        let c = self.conn().await?;
        let sql = format!(
            "UPDATE objects SET name = COALESCE($3, name), \
             content_type = COALESCE($4, content_type), content = COALESCE($5, content), \
             version = version + 1, updated_at = now(), updated_by = $6 \
             WHERE id = $1 AND version = $2 RETURNING {OBJECT_COLS}"
        );
        let row = c
            .query_opt(
                &sql,
                &[
                    &id,
                    &expected_version,
                    &name,
                    &content_type,
                    &content,
                    &updated_by,
                ],
            )
            .await?;
        match row {
            Some(r) => Ok(object_from(&r)),
            None => {
                let exists = c
                    .query_opt("SELECT 1 FROM objects WHERE id = $1", &[&id])
                    .await?
                    .is_some();
                if exists {
                    Err(StoreError::Conflict("stale object version".into()))
                } else {
                    Err(StoreError::NotFound)
                }
            }
        }
    }

    pub async fn delete_object(&self, id: Uuid) -> Result<bool, StoreError> {
        let c = self.conn().await?;
        Ok(c.execute("DELETE FROM objects WHERE id = $1", &[&id])
            .await?
            > 0)
    }

    /// Starts a transaction on a dedicated connection.
    pub async fn transaction(&self) -> Result<StoreTx<'_>, StoreError> {
        let c = self.conn().await?;
        c.batch_execute("BEGIN").await?;
        Ok(StoreTx {
            conn: Some(c),
            _store: std::marker::PhantomData,
        })
    }
}

/// A transaction exposing the `create_*` operations. Dropping it without
/// `commit` discards the connection, which rolls the transaction back.
pub struct StoreTx<'a> {
    conn: Option<Object>,
    _store: std::marker::PhantomData<&'a Store>,
}

impl StoreTx<'_> {
    async fn conn(&self) -> Result<&Object, StoreError> {
        self.conn
            .as_ref()
            .ok_or_else(|| StoreError::Db("transaction already finished".into()))
    }

    shared_creates!(Self::conn);

    pub async fn commit(mut self) -> Result<(), StoreError> {
        let c = self.conn.take().ok_or(StoreError::NotFound)?;
        match c.batch_execute("COMMIT").await {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = Object::take(c);
                Err(e.into())
            }
        }
    }

    pub async fn rollback(mut self) -> Result<(), StoreError> {
        let c = self.conn.take().ok_or(StoreError::NotFound)?;
        match c.batch_execute("ROLLBACK").await {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = Object::take(c);
                Err(e.into())
            }
        }
    }
}

impl Drop for StoreTx<'_> {
    fn drop(&mut self) {
        if let Some(c) = self.conn.take() {
            // Detach so the open transaction dies with the connection instead
            // of going back to the pool.
            let _ = Object::take(c);
        }
    }
}
