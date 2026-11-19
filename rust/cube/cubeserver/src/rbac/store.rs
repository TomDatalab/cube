//! Postgres (`cube_app`) storage for users, organizations, workspaces,
//! connections and objects. Roles and grants live in SpiceDB, not here.

use chrono::{DateTime, Utc};
use deadpool_postgres::{Manager, ManagerConfig, Object, Pool, RecyclingMethod};
use tokio_postgres::error::SqlState;
use tokio_postgres::{NoTls, Row};
use uuid::Uuid;

use super::restrictions::{Attributes, Restrictions};

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

/// Substring search plus paging. `limit: None` means no limit.
#[derive(Debug, Clone, Default)]
pub struct Search {
    pub q: Option<String>,
    pub limit: Option<i64>,
    pub offset: i64,
}

impl Search {
    /// The term with `\`, `%` and `_` escaped so `LIKE ... ESCAPE '\'` reads
    /// them literally.
    fn escaped(&self) -> Option<String> {
        self.q.as_deref().map(|q| {
            let mut out = String::with_capacity(q.len());
            for ch in q.chars() {
                if matches!(ch, '\\' | '%' | '_') {
                    out.push('\\');
                }
                out.push(ch);
            }
            out
        })
    }

    fn limit(&self) -> Option<i64> {
        self.limit.map(|l| l.max(0))
    }

    fn offset(&self) -> i64 {
        self.offset.max(0)
    }
}

#[derive(Debug, Clone)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: i64,
}

/// A role an organization defines: the workspace permissions it grants.
#[derive(Debug, Clone)]
pub struct Role {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub name: String,
    pub permissions: Vec<String>,
    pub restrictions: Restrictions,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
}

/// A role with the number of rules that use it.
#[derive(Debug, Clone)]
pub struct RoleListing {
    pub role: Role,
    pub rules: i64,
}

/// A named role in a workspace: its members hold a built-in `role`
/// (`owner`, `editor`, `viewer`) or the organization's `role_id` there,
/// exactly one of them. `type` is a free label.
#[derive(Debug, Clone)]
pub struct Rule {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub description: String,
    pub r#type: String,
    pub role: Option<String>,
    pub role_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
}

/// What a rule grants: a built-in role or one of its organization's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleRole<'a> {
    Builtin(&'a str),
    Custom(Uuid),
}

impl<'a> RuleRole<'a> {
    fn columns(self) -> (Option<&'a str>, Option<Uuid>) {
        match self {
            Self::Builtin(r) => (Some(r), None),
            Self::Custom(id) => (None, Some(id)),
        }
    }
}

impl Rule {
    pub fn role_ref(&self) -> RuleRole<'_> {
        match (&self.role, self.role_id) {
            (_, Some(id)) => RuleRole::Custom(id),
            (Some(r), None) => RuleRole::Builtin(r),
            // The table's check rules this out.
            (None, None) => RuleRole::Builtin("viewer"),
        }
    }
}

/// A rule with the workspace and organization it belongs to.
#[derive(Debug, Clone)]
pub struct RuleListing {
    pub rule: Rule,
    pub workspace_name: String,
    pub organization_id: Uuid,
    pub organization_name: String,
    /// The name and permissions of its custom role, if it has one.
    pub custom_role: Option<(String, Vec<String>)>,
}

#[derive(Debug, Clone)]
pub struct WorkspaceListing {
    pub workspace: Workspace,
    pub organization_name: String,
}

/// How a connection came to its organization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionOrigin {
    /// Created by the organization (`POST /v1/organizations/{id}/connections`):
    /// its owners may edit and delete it.
    Created,
    /// A `cube.yml` data source a platform admin assigned to the organization
    /// (`PUT /v1/connections/{name}/organization`): only platform admins may
    /// edit or delete it.
    Assigned,
}

impl ConnectionOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            ConnectionOrigin::Created => "created",
            ConnectionOrigin::Assigned => "assigned",
        }
    }

    /// Anything but `created` is `assigned`: the stricter reading.
    fn parse(s: &str) -> Self {
        if s == "created" {
            ConnectionOrigin::Created
        } else {
            ConnectionOrigin::Assigned
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConnectionRow {
    pub name: String,
    pub organization_id: Uuid,
    pub origin: ConnectionOrigin,
}

const CONNECTION_COLS: &str = "name, organization_id, origin";

fn connection_from(r: &Row) -> ConnectionRow {
    ConnectionRow {
        name: r.get(0),
        organization_id: r.get(1),
        origin: ConnectionOrigin::parse(r.get(2)),
    }
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

const RULE_COLS: &str =
    "id, workspace_id, description, type, role, role_id, created_at, created_by";
const ROLE_COLS: &str =
    "id, organization_id, name, permissions, restrictions, created_at, created_by";

fn rule_from(r: &Row) -> Rule {
    Rule {
        id: r.get(0),
        workspace_id: r.get(1),
        description: r.get(2),
        r#type: r.get(3),
        role: r.get(4),
        role_id: r.get(5),
        created_at: r.get(6),
        created_by: r.get(7),
    }
}

/// `(name, permissions)` at columns `at` and `at + 1`, `None` when NULL.
fn custom_role(r: &Row, at: usize) -> Option<(String, Vec<String>)> {
    let name: Option<String> = r.get(at);
    let permissions: Option<Vec<String>> = r.get(at + 1);
    name.zip(permissions)
}

/// A stored `restrictions` that does not parse is an error, never "no
/// restriction".
fn role_from(r: &Row) -> Result<Role, StoreError> {
    let restrictions: serde_json::Value = r.get(4);
    let restrictions = serde_json::from_value(restrictions)
        .map_err(|e| StoreError::Db(format!("invalid role restrictions: {e}")))?;
    Ok(Role {
        id: r.get(0),
        organization_id: r.get(1),
        name: r.get(2),
        permissions: r.get(3),
        restrictions,
        created_at: r.get(5),
        created_by: r.get(6),
    })
}

fn restrictions_json(restrictions: &Restrictions) -> Result<serde_json::Value, StoreError> {
    serde_json::to_value(restrictions)
        .map_err(|e| StoreError::Db(format!("invalid role restrictions: {e}")))
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
        origin: ConnectionOrigin,
    ) -> Result<ConnectionRow, StoreError> {
        c.execute(
            "INSERT INTO connections (name, organization_id, origin) VALUES ($1, $2, $3)",
            &[&name, &organization_id, &origin.as_str()],
        )
        .await?;
        Ok(ConnectionRow {
            name: name.to_string(),
            organization_id,
            origin,
        })
    }

    pub async fn delete_organization(
        c: &tokio_postgres::Client,
        id: Uuid,
    ) -> Result<bool, StoreError> {
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

    pub async fn delete_workspace(
        c: &tokio_postgres::Client,
        id: Uuid,
    ) -> Result<bool, StoreError> {
        Ok(c.execute("DELETE FROM workspaces WHERE id = $1", &[&id])
            .await?
            > 0)
    }

    /// Hands the connection from `from` to `to`; it is then `assigned` (the
    /// new organization did not create it). `true` when a row changed:
    /// `false` when there is none, or it no longer belongs to `from` (a
    /// concurrent move or delete got there first).
    pub async fn move_connection(
        c: &tokio_postgres::Client,
        name: &str,
        from: Uuid,
        to: Uuid,
    ) -> Result<bool, StoreError> {
        Ok(c
            .execute(
                "UPDATE connections SET organization_id = $3, origin = 'assigned' \
                 WHERE name = $1 AND organization_id = $2",
                &[&name, &from, &to],
            )
            .await?
            > 0)
    }

    pub async fn delete_connection(
        c: &tokio_postgres::Client,
        name: &str,
    ) -> Result<bool, StoreError> {
        Ok(
            c.execute("DELETE FROM connections WHERE name = $1", &[&name])
                .await?
                > 0,
        )
    }

    pub async fn create_rule(
        c: &tokio_postgres::Client,
        workspace_id: Uuid,
        description: &str,
        r#type: &str,
        role: RuleRole<'_>,
        created_by: &str,
    ) -> Result<Rule, StoreError> {
        let sql = format!(
            "INSERT INTO rules (id, workspace_id, description, type, role, role_id, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {RULE_COLS}"
        );
        let (role, role_id) = role.columns();
        let row = c
            .query_one(
                &sql,
                &[
                    &Uuid::new_v4(),
                    &workspace_id,
                    &description,
                    &r#type,
                    &role,
                    &role_id,
                    &created_by,
                ],
            )
            .await?;
        Ok(rule_from(&row))
    }

    pub async fn update_rule(
        c: &tokio_postgres::Client,
        id: Uuid,
        workspace_id: Uuid,
        description: &str,
        r#type: &str,
        role: RuleRole<'_>,
    ) -> Result<Option<Rule>, StoreError> {
        let sql = format!(
            "UPDATE rules SET workspace_id = $2, description = $3, type = $4, role = $5, \
             role_id = $6 WHERE id = $1 RETURNING {RULE_COLS}"
        );
        let (role, role_id) = role.columns();
        let row = c
            .query_opt(
                &sql,
                &[&id, &workspace_id, &description, &r#type, &role, &role_id],
            )
            .await?;
        Ok(row.as_ref().map(rule_from))
    }

    pub async fn delete_rule(c: &tokio_postgres::Client, id: Uuid) -> Result<bool, StoreError> {
        Ok(c.execute("DELETE FROM rules WHERE id = $1", &[&id]).await? > 0)
    }

    pub async fn create_role(
        c: &tokio_postgres::Client,
        organization_id: Uuid,
        name: &str,
        permissions: &[String],
        restrictions: &Restrictions,
        created_by: &str,
    ) -> Result<Role, StoreError> {
        let restrictions = restrictions_json(restrictions)?;
        let sql = format!(
            "INSERT INTO roles (id, organization_id, name, permissions, restrictions, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6) RETURNING {ROLE_COLS}"
        );
        let row = c
            .query_one(
                &sql,
                &[
                    &Uuid::new_v4(),
                    &organization_id,
                    &name,
                    &permissions,
                    &restrictions,
                    &created_by,
                ],
            )
            .await?;
        role_from(&row)
    }

    pub async fn update_role(
        c: &tokio_postgres::Client,
        id: Uuid,
        name: &str,
        permissions: &[String],
        restrictions: &Restrictions,
    ) -> Result<Option<Role>, StoreError> {
        let restrictions = restrictions_json(restrictions)?;
        let sql = format!(
            "UPDATE roles SET name = $2, permissions = $3, restrictions = $4 \
             WHERE id = $1 RETURNING {ROLE_COLS}"
        );
        let row = c
            .query_opt(&sql, &[&id, &name, &permissions, &restrictions])
            .await?;
        row.as_ref().map(role_from).transpose()
    }

    /// Fails with `Conflict` while a rule uses the role.
    pub async fn delete_role(c: &tokio_postgres::Client, id: Uuid) -> Result<bool, StoreError> {
        match c.execute("DELETE FROM roles WHERE id = $1", &[&id]).await {
            Ok(n) => Ok(n > 0),
            Err(e) if e.code() == Some(&SqlState::FOREIGN_KEY_VIOLATION) => {
                Err(StoreError::Conflict("role still used by rules".into()))
            }
            Err(e) => Err(e.into()),
        }
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
            origin: ConnectionOrigin,
        ) -> Result<ConnectionRow, StoreError> {
            let guard = $conn(self).await?;
            let c: &tokio_postgres::Client = &guard;
            q::create_connection(c, name, organization_id, origin).await
        }
        /// `true` when a row was deleted. Fails with `Conflict` while
        /// workspaces or connections still reference the organization.
        pub async fn delete_organization(&self, id: Uuid) -> Result<bool, StoreError> {
            let guard = $conn(self).await?;
            let c: &tokio_postgres::Client = &guard;
            q::delete_organization(c, id).await
        }
        /// Cascades to the workspace's objects. `true` when a row was deleted.
        pub async fn delete_workspace(&self, id: Uuid) -> Result<bool, StoreError> {
            let guard = $conn(self).await?;
            let c: &tokio_postgres::Client = &guard;
            q::delete_workspace(c, id).await
        }
        /// See [`q::move_connection`].
        pub async fn move_connection(
            &self,
            name: &str,
            from: Uuid,
            to: Uuid,
        ) -> Result<bool, StoreError> {
            let guard = $conn(self).await?;
            let c: &tokio_postgres::Client = &guard;
            q::move_connection(c, name, from, to).await
        }
        pub async fn delete_connection(&self, name: &str) -> Result<bool, StoreError> {
            let guard = $conn(self).await?;
            let c: &tokio_postgres::Client = &guard;
            q::delete_connection(c, name).await
        }
        pub async fn create_rule(
            &self,
            workspace_id: Uuid,
            description: &str,
            r#type: &str,
            role: RuleRole<'_>,
            created_by: &str,
        ) -> Result<Rule, StoreError> {
            let guard = $conn(self).await?;
            let c: &tokio_postgres::Client = &guard;
            q::create_rule(c, workspace_id, description, r#type, role, created_by).await
        }
        /// The rule with every field replaced, `None` when it does not exist.
        pub async fn update_rule(
            &self,
            id: Uuid,
            workspace_id: Uuid,
            description: &str,
            r#type: &str,
            role: RuleRole<'_>,
        ) -> Result<Option<Rule>, StoreError> {
            let guard = $conn(self).await?;
            let c: &tokio_postgres::Client = &guard;
            q::update_rule(c, id, workspace_id, description, r#type, role).await
        }
        /// `true` when a row was deleted.
        pub async fn delete_rule(&self, id: Uuid) -> Result<bool, StoreError> {
            let guard = $conn(self).await?;
            let c: &tokio_postgres::Client = &guard;
            q::delete_rule(c, id).await
        }
        pub async fn create_role(
            &self,
            organization_id: Uuid,
            name: &str,
            permissions: &[String],
            restrictions: &Restrictions,
            created_by: &str,
        ) -> Result<Role, StoreError> {
            let guard = $conn(self).await?;
            let c: &tokio_postgres::Client = &guard;
            q::create_role(
                c,
                organization_id,
                name,
                permissions,
                restrictions,
                created_by,
            )
            .await
        }
        pub async fn update_role(
            &self,
            id: Uuid,
            name: &str,
            permissions: &[String],
            restrictions: &Restrictions,
        ) -> Result<Option<Role>, StoreError> {
            let guard = $conn(self).await?;
            let c: &tokio_postgres::Client = &guard;
            q::update_role(c, id, name, permissions, restrictions).await
        }
        /// `true` when a row was deleted; `Conflict` while a rule uses it.
        pub async fn delete_role(&self, id: Uuid) -> Result<bool, StoreError> {
            let guard = $conn(self).await?;
            let c: &tokio_postgres::Client = &guard;
            q::delete_role(c, id).await
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

    pub async fn search_users(&self, s: &Search) -> Result<Page<User>, StoreError> {
        let c = self.conn().await?;
        let q = s.escaped();
        const WHERE: &str = "($1::text IS NULL OR id ILIKE '%' || $1 || '%' ESCAPE '\\' \
             OR name ILIKE '%' || $1 || '%' ESCAPE '\\' \
             OR email ILIKE '%' || $1 || '%' ESCAPE '\\')";
        let sql =
            format!("SELECT {USER_COLS} FROM users WHERE {WHERE} ORDER BY id LIMIT $2 OFFSET $3");
        let items = c
            .query(&sql, &[&q, &s.limit(), &s.offset()])
            .await?
            .iter()
            .map(user_from)
            .collect();
        let total = c
            .query_one(&format!("SELECT count(*) FROM users WHERE {WHERE}"), &[&q])
            .await?
            .get(0);
        Ok(Page { items, total })
    }

    /// `visible: None` lists every organization.
    pub async fn search_organizations(
        &self,
        visible: Option<&[Uuid]>,
        s: &Search,
    ) -> Result<Page<Organization>, StoreError> {
        let c = self.conn().await?;
        let q = s.escaped();
        let visible = visible.map(<[Uuid]>::to_vec);
        const WHERE: &str = "($1::text IS NULL OR name ILIKE '%' || $1 || '%' ESCAPE '\\' \
             OR id::text LIKE lower($1) || '%' ESCAPE '\\') \
             AND ($2::uuid[] IS NULL OR id = ANY($2))";
        let sql = format!(
            "SELECT id, name, created_at, created_by FROM organizations WHERE {WHERE} \
             ORDER BY name, id LIMIT $3 OFFSET $4"
        );
        let items = c
            .query(&sql, &[&q, &visible, &s.limit(), &s.offset()])
            .await?
            .iter()
            .map(org_from)
            .collect();
        let total = c
            .query_one(
                &format!("SELECT count(*) FROM organizations WHERE {WHERE}"),
                &[&q, &visible],
            )
            .await?
            .get(0);
        Ok(Page { items, total })
    }

    /// `visible: None` lists every workspace; `organization` narrows to one.
    pub async fn search_workspaces(
        &self,
        visible: Option<&[Uuid]>,
        organization: Option<Uuid>,
        s: &Search,
    ) -> Result<Page<WorkspaceListing>, StoreError> {
        let c = self.conn().await?;
        let q = s.escaped();
        let visible = visible.map(<[Uuid]>::to_vec);
        const FROM: &str = "FROM workspaces w JOIN organizations o ON o.id = w.organization_id \
             WHERE ($1::text IS NULL OR w.name ILIKE '%' || $1 || '%' ESCAPE '\\' \
             OR w.id::text LIKE lower($1) || '%' ESCAPE '\\' \
             OR o.name ILIKE '%' || $1 || '%' ESCAPE '\\') \
             AND ($2::uuid[] IS NULL OR w.id = ANY($2)) \
             AND ($3::uuid IS NULL OR w.organization_id = $3)";
        let sql = format!(
            "SELECT w.id, w.organization_id, w.name, w.created_at, w.created_by, o.name {FROM} \
             ORDER BY o.name, o.id, w.name, w.id LIMIT $4 OFFSET $5"
        );
        let items = c
            .query(
                &sql,
                &[&q, &visible, &organization, &s.limit(), &s.offset()],
            )
            .await?
            .iter()
            .map(|r| WorkspaceListing {
                workspace: workspace_from(r),
                organization_name: r.get(5),
            })
            .collect();
        let total = c
            .query_one(
                &format!("SELECT count(*) {FROM}"),
                &[&q, &visible, &organization],
            )
            .await?
            .get(0);
        Ok(Page { items, total })
    }

    pub async fn all_connections(&self) -> Result<Vec<ConnectionRow>, StoreError> {
        let c = self.conn().await?;
        let sql = format!("SELECT {CONNECTION_COLS} FROM connections ORDER BY name");
        Ok(c.query(&sql, &[])
            .await?
            .iter()
            .map(connection_from)
            .collect())
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

    // roles
    pub async fn role(&self, id: Uuid) -> Result<Option<Role>, StoreError> {
        let c = self.conn().await?;
        let sql = format!("SELECT {ROLE_COLS} FROM roles WHERE id = $1");
        c.query_opt(&sql, &[&id])
            .await?
            .as_ref()
            .map(role_from)
            .transpose()
    }

    pub async fn roles_by_ids(&self, ids: &[Uuid]) -> Result<Vec<Role>, StoreError> {
        let c = self.conn().await?;
        let sql = format!("SELECT {ROLE_COLS} FROM roles WHERE id = ANY($1) ORDER BY name");
        c.query(&sql, &[&ids])
            .await?
            .iter()
            .map(role_from)
            .collect()
    }

    /// The organization's roles by name, each with how many rules use it.
    pub async fn roles_of_organization(
        &self,
        organization_id: Uuid,
    ) -> Result<Vec<RoleListing>, StoreError> {
        let c = self.conn().await?;
        let sql = format!(
            "SELECT {ROLE_COLS}, (SELECT count(*) FROM rules r WHERE r.role_id = roles.id) \
             FROM roles WHERE organization_id = $1 ORDER BY name, id"
        );
        c.query(&sql, &[&organization_id])
            .await?
            .iter()
            .map(|r| {
                Ok(RoleListing {
                    role: role_from(r)?,
                    rules: r.get(7),
                })
            })
            .collect()
    }

    /// The user's attributes: name to values.
    pub async fn user_attributes(&self, user: &str) -> Result<Attributes, StoreError> {
        let c = self.conn().await?;
        let rows = c
            .query(
                "SELECT name, values FROM user_attributes WHERE user_id = $1",
                &[&user],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Replaces the user's whole set of attributes, in one transaction.
    pub async fn set_user_attributes(
        &self,
        user: &str,
        attributes: &Attributes,
    ) -> Result<(), StoreError> {
        let mut c = self.conn().await?;
        let tx = c.transaction().await?;
        tx.execute("DELETE FROM user_attributes WHERE user_id = $1", &[&user])
            .await?;
        for (name, values) in attributes {
            tx.execute(
                "INSERT INTO user_attributes (user_id, name, values) VALUES ($1, $2, $3)",
                &[&user, name, values],
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// The rules that use the role.
    pub async fn rules_of_role(&self, role_id: Uuid) -> Result<Vec<Rule>, StoreError> {
        let c = self.conn().await?;
        let sql = format!("SELECT {RULE_COLS} FROM rules WHERE role_id = $1");
        Ok(c.query(&sql, &[&role_id])
            .await?
            .iter()
            .map(rule_from)
            .collect())
    }

    // rules
    pub async fn rule(&self, id: Uuid) -> Result<Option<Rule>, StoreError> {
        let c = self.conn().await?;
        let sql = format!("SELECT {RULE_COLS} FROM rules WHERE id = $1");
        Ok(c.query_opt(&sql, &[&id]).await?.as_ref().map(rule_from))
    }

    pub async fn rules_by_ids(&self, ids: &[Uuid]) -> Result<Vec<Rule>, StoreError> {
        let c = self.conn().await?;
        let sql =
            format!("SELECT {RULE_COLS} FROM rules WHERE id = ANY($1) ORDER BY description, id");
        Ok(c.query(&sql, &[&ids])
            .await?
            .iter()
            .map(rule_from)
            .collect())
    }

    pub async fn rules_of_workspace(&self, workspace_id: Uuid) -> Result<Vec<Rule>, StoreError> {
        let c = self.conn().await?;
        let sql = format!(
            "SELECT {RULE_COLS} FROM rules WHERE workspace_id = $1 ORDER BY description, id"
        );
        Ok(c.query(&sql, &[&workspace_id])
            .await?
            .iter()
            .map(rule_from)
            .collect())
    }

    /// Searches the description, type, role, workspace and organization names.
    /// `visible: None` lists the rules of every workspace; `organization`
    /// and `workspace` narrow it.
    pub async fn search_rules(
        &self,
        visible: Option<&[Uuid]>,
        organization: Option<Uuid>,
        workspace: Option<Uuid>,
        s: &Search,
    ) -> Result<Page<RuleListing>, StoreError> {
        let c = self.conn().await?;
        let q = s.escaped();
        let visible = visible.map(<[Uuid]>::to_vec);
        const FROM: &str = "FROM rules r JOIN workspaces w ON w.id = r.workspace_id \
             JOIN organizations o ON o.id = w.organization_id \
             LEFT JOIN roles ro ON ro.id = r.role_id \
             WHERE ($1::text IS NULL OR r.description ILIKE '%' || $1 || '%' ESCAPE '\\' \
             OR r.type ILIKE '%' || $1 || '%' ESCAPE '\\' \
             OR coalesce(r.role, ro.name) ILIKE '%' || $1 || '%' ESCAPE '\\' \
             OR w.name ILIKE '%' || $1 || '%' ESCAPE '\\' \
             OR o.name ILIKE '%' || $1 || '%' ESCAPE '\\') \
             AND ($2::uuid[] IS NULL OR w.id = ANY($2)) \
             AND ($3::uuid IS NULL OR w.organization_id = $3) \
             AND ($4::uuid IS NULL OR w.id = $4)";
        let sql = format!(
            "SELECT r.id, r.workspace_id, r.description, r.type, r.role, r.role_id, \
             r.created_at, r.created_by, w.name, o.id, o.name, ro.name, ro.permissions {FROM} \
             ORDER BY o.name, o.id, w.name, w.id, r.description, r.id LIMIT $5 OFFSET $6"
        );
        let items = c
            .query(
                &sql,
                &[
                    &q,
                    &visible,
                    &organization,
                    &workspace,
                    &s.limit(),
                    &s.offset(),
                ],
            )
            .await?
            .iter()
            .map(|r| RuleListing {
                rule: rule_from(r),
                workspace_name: r.get(8),
                organization_id: r.get(9),
                organization_name: r.get(10),
                custom_role: custom_role(r, 11),
            })
            .collect();
        let total = c
            .query_one(
                &format!("SELECT count(*) {FROM}"),
                &[&q, &visible, &organization, &workspace],
            )
            .await?
            .get(0);
        Ok(Page { items, total })
    }

    // connections
    pub async fn connection(&self, name: &str) -> Result<Option<ConnectionRow>, StoreError> {
        let c = self.conn().await?;
        let sql = format!("SELECT {CONNECTION_COLS} FROM connections WHERE name = $1");
        let row = c.query_opt(&sql, &[&name]).await?;
        Ok(row.as_ref().map(connection_from))
    }

    pub async fn connections_by_names(
        &self,
        names: &[String],
    ) -> Result<Vec<ConnectionRow>, StoreError> {
        let c = self.conn().await?;
        let sql =
            format!("SELECT {CONNECTION_COLS} FROM connections WHERE name = ANY($1) ORDER BY name");
        let rows = c.query(&sql, &[&names]).await?;
        Ok(rows.iter().map(connection_from).collect())
    }

    pub async fn connections_of_organization(
        &self,
        organization_id: Uuid,
    ) -> Result<Vec<ConnectionRow>, StoreError> {
        let c = self.conn().await?;
        let sql = format!(
            "SELECT {CONNECTION_COLS} FROM connections WHERE organization_id = $1 ORDER BY name"
        );
        let rows = c.query(&sql, &[&organization_id]).await?;
        Ok(rows.iter().map(connection_from).collect())
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

    /// The objects of a workspace, optionally of one `type`, by name. The
    /// content is left out (empty): listings carry metadata only.
    pub async fn objects_of_workspace(
        &self,
        workspace_id: Uuid,
        r#type: Option<&str>,
    ) -> Result<Vec<StoredObject>, StoreError> {
        let c = self.conn().await?;
        let sql = "SELECT id, workspace_id, type, name, content_type, ''::bytea, version, \
                   created_at, created_by, updated_at, updated_by FROM objects \
                   WHERE workspace_id = $1 AND ($2::text IS NULL OR type = $2) \
                   ORDER BY name, id";
        Ok(c.query(sql, &[&workspace_id, &r#type])
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

/// A transaction exposing the `create_*` and `delete_*` operations. Dropping it without
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
