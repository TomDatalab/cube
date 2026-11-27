//! Authorization (RBAC) backed by SpiceDB.

pub mod access;
pub mod authz;
pub mod bootstrap;
pub mod config;
pub mod cube_model;
pub mod ddl;
pub mod dialects;
pub mod dependencies;
pub mod enforce;
pub mod fake;
pub mod handlers;
pub mod identity;
pub mod model_import;
pub mod model_sync;
pub mod model_redact;
pub mod model_runtime;
pub mod model_snapshot;
pub mod model_validate;
pub mod model_writer;
pub mod model_store;
pub mod ownership;
pub mod restrictions;
pub mod data_sources;
pub mod scope;
pub mod secrets;
pub mod slug;
pub mod store;
pub mod user_ids;
pub mod default_roles;

use std::sync::Arc;

use axum::http::StatusCode;
use tokio::sync::Semaphore;

pub use authz::{
    Authz, AuthzError, HttpAuthz, ObjectRef, Precondition, Relationship, RelationshipFilter, Update,
};
pub use config::RbacConfig;

use crate::error::ApiError;
use store::{NewUser, Store, StoreError};

/// In-memory implementation for tests, also usable from integration tests.
pub mod testing {
    pub use super::fake::FakeAuthz;
}

const SCHEMA: &str = include_str!("../../rbac/schema.zed");

/// The single `platform` object holding the platform admins.
pub fn platform() -> ObjectRef {
    ObjectRef::new("platform", "main")
}

/// RBAC is on: who users are (Postgres) and what they may do (SpiceDB).
pub struct Rbac {
    pub authz: Arc<dyn Authz>,
    pub store: Store,
    pub config: RbacConfig,
    /// Bounds concurrent argon2 runs (`config.max_concurrent_hashes`).
    hashing: Arc<Semaphore>,
    /// Who runs each load, so only they (or a platform admin) cancel it.
    pub running: enforce::RunningRequests,
    /// The data sources `cube.yml` and RBAC declare, for the ownership
    /// policy: those no connection claims belong to the platform.
    data_source_names: std::sync::RwLock<Vec<String>>,
    /// Serializes policy refreshes, so an older policy never replaces a newer.
    pub(crate) refresh_lock: tokio::sync::Mutex<()>,
    /// A background retry of a failed refresh is running.
    pub(crate) refresh_retry: Arc<ownership::RetryState>,
    /// The folder ownership policy is in force: set by `main` when it
    /// installs the boot policy (and by tests that want it). Off, a change to
    /// organizations or connections never installs a policy, so a process
    /// (a test binary) that did not ask for one keeps a model without owners.
    ownership_enabled: std::sync::atomic::AtomicBool,
}

impl std::fmt::Debug for Rbac {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rbac")
            .field("authz", &self.authz)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl Rbac {
    /// Connects to SpiceDB and Postgres, writes the schema and makes the
    /// configured admins platform admins. Any failure stops the start-up.
    pub async fn start(config: RbacConfig) -> Result<Self, String> {
        let authz: Arc<dyn Authz> = Arc::new(HttpAuthz::new(
            &config.spicedb_endpoint,
            &config.preshared_key,
        ));
        let store = Store::connect(&config.db_url)
            .await
            .map_err(|e| format!("Cannot open the RBAC database (CUBE_APP_DB_URL): {e}"))?;
        store
            .backfill_slugs()
            .await
            .map_err(|e| format!("Cannot fill the organization slugs: {e}"))?;
        Self::with_parts(authz, store, config).await
    }

    /// [`Rbac::start`] with the clients already built (tests pass fakes).
    pub async fn with_parts(
        authz: Arc<dyn Authz>,
        store: Store,
        config: RbacConfig,
    ) -> Result<Self, String> {
        authz
            .ensure_schema(SCHEMA)
            .await
            .map_err(|e| format!("Cannot write the SpiceDB schema (SPICEDB_ENDPOINT): {e}"))?;
        // Warm-up: the first request does not pay for opening the
        // connection, and a SpiceDB that cannot answer checks stops the start.
        authz
            .check(&platform(), "manage", "startup-check")
            .await
            .map_err(|e| {
                format!("SpiceDB cannot answer permission checks (SPICEDB_ENDPOINT): {e}")
            })?;
        // Before the admins are granted: SpiceDB names users by UUID.
        user_ids::migrate_user_subjects(&store, authz.as_ref()).await?;
        default_roles::migrate_default_roles(&store, authz.as_ref()).await?;
        identity::prepare_dummy_hash();
        let hashing = Arc::new(Semaphore::new(config.max_concurrent_hashes.max(1)));
        let rbac = Self {
            authz,
            store,
            config,
            hashing,
            running: Default::default(),
            data_source_names: Default::default(),
            refresh_lock: Default::default(),
            refresh_retry: Default::default(),
            ownership_enabled: Default::default(),
        };
        rbac.bootstrap_admins().await?;
        Ok(rbac)
    }

    /// `CUBE_RBAC_ADMINS` (usernames): creates the missing ones, with a fresh
    /// UUID and `CUBE_RBAC_ADMIN_PASSWORD`, and grants `platform:main#admin`
    /// to each by UUID. Removing a name from the list revokes nothing.
    async fn bootstrap_admins(&self) -> Result<(), String> {
        let mut grants = Vec::new();
        for username in &self.config.admins {
            let read = || async {
                self.store
                    .user_by_username(username)
                    .await
                    .map_err(|e| format!("Cannot read user {username}: {e}"))
            };
            let user = match read().await? {
                Some(user) => user,
                None => {
                    let password = self.config.admin_password.as_deref().ok_or_else(|| {
                        format!(
                            "Admin {username} (CUBE_RBAC_ADMINS) does not exist and \
                             CUBE_RBAC_ADMIN_PASSWORD is not set"
                        )
                    })?;
                    let password_hash = identity::hash_password(password)
                        .map_err(|e| format!("CUBE_RBAC_ADMIN_PASSWORD: {}", e.body.error))?;
                    let created = self
                        .store
                        .create_user(&NewUser {
                            username: username.clone(),
                            name: None,
                            email: None,
                            password_hash,
                        })
                        .await;
                    match created {
                        Ok(user) => user,
                        // Another instance starting at the same time made it.
                        Err(StoreError::Conflict(_)) => read().await?.ok_or_else(|| {
                            format!("Admin {username} conflicts but cannot be read")
                        })?,
                        Err(e) => return Err(format!("Cannot create admin {username}: {e}")),
                    }
                }
            };
            grants.push(Relationship {
                resource: platform(),
                relation: "admin",
                subject: ObjectRef::new("user", user.id),
            });
        }
        if !grants.is_empty() {
            self.authz
                .touch(&grants)
                .await
                .map_err(|e| format!("Cannot grant the platform admins in SpiceDB: {e}"))?;
        }
        Ok(())
    }

    /// Runs a password hash or verification on a blocking thread, at most
    /// `max_concurrent_hashes` at once. 503 when no slot frees up within
    /// `hash_queue_wait`: a login flood is refused instead of exhausting
    /// memory. The slot is held until the hash finishes, even if the request
    /// is dropped.
    pub async fn run_hashing<T, F>(&self, work: F) -> Result<T, ApiError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let permit = tokio::time::timeout(
            self.config.hash_queue_wait,
            self.hashing.clone().acquire_owned(),
        )
        .await
        .map_err(|_| {
            tracing::warn!("Too many password checks at once; answering 503");
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "Too many login attempts, try again shortly",
            )
        })?
        .map_err(|_| ApiError::internal())?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            work()
        })
        .await
        .map_err(|_| ApiError::internal())
    }

    pub fn set_data_source_names(&self, names: Vec<String>) {
        *self
            .data_source_names
            .write()
            .unwrap_or_else(|e| e.into_inner()) = names;
    }

    pub fn data_source_names(&self) -> Vec<String> {
        self.data_source_names
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Turns the folder ownership policy on: from now on organization and
    /// connection changes rebuild and install it.
    pub fn enable_ownership(&self) {
        self.ownership_enabled
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn ownership_enabled(&self) -> bool {
        self.ownership_enabled
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Test hook: the semaphore bounding concurrent hashes, so a test can
    /// hold its permits as an in-flight verification would.
    pub fn hashing_permits(&self) -> Arc<Semaphore> {
        self.hashing.clone()
    }

    pub async fn is_platform_admin(&self, user: &str) -> Result<bool, ApiError> {
        self.check(&platform(), "manage", user).await
    }

    /// The organizations `user` owns (organization permission `admin`),
    /// sorted by name. Ids SpiceDB returns that are not UUIDs are ignored.
    /// 503 when SpiceDB or the RBAC database cannot answer.
    pub async fn owned_organizations(
        &self,
        user: &str,
    ) -> Result<Vec<store::Organization>, ApiError> {
        let ids: Vec<uuid::Uuid> = self
            .authz
            .lookup_resources("organization", "admin", user)
            .await
            .map_err(authz_error)?
            .iter()
            .filter_map(|id| id.parse().ok())
            .collect();
        let mut organizations = self.store.organizations_by_ids(&ids).await.map_err(|e| {
            tracing::error!("Cannot read the owned organizations: {e}");
            unavailable()
        })?;
        organizations.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(organizations)
    }

    /// 403 unless `user` has `permission` on `resource`.
    pub async fn require(
        &self,
        resource: &ObjectRef,
        permission: &str,
        user: &str,
    ) -> Result<(), ApiError> {
        if self.check(resource, permission, user).await? {
            Ok(())
        } else {
            Err(ApiError::forbidden(format!(
                "Permission denied: {permission} on {} {}",
                resource.kind, resource.id
            )))
        }
    }

    /// 404 when `user` cannot see `resource` (so its existence is not
    /// revealed), 403 when they can but lack `permission`.
    pub async fn require_visible(
        &self,
        resource: &ObjectRef,
        view_permission: &str,
        permission: &str,
        user: &str,
    ) -> Result<(), ApiError> {
        if !self.check(resource, view_permission, user).await? {
            return Err(ApiError::not_found(format!("{} not found", resource.kind)));
        }
        if permission == view_permission {
            return Ok(());
        }
        self.require(resource, permission, user).await
    }

    /// Fail-closed: an unreachable SpiceDB is 503, never an allow.
    async fn check(
        &self,
        resource: &ObjectRef,
        permission: &str,
        user: &str,
    ) -> Result<bool, ApiError> {
        self.authz
            .check(resource, permission, user)
            .await
            .map_err(authz_error)
    }
}

/// 503 for an unreachable SpiceDB, 500 for a request it refused, 409 for a
/// write whose precondition no longer held.
pub fn authz_error(err: AuthzError) -> ApiError {
    match err {
        AuthzError::Unavailable(m) => {
            tracing::error!("SpiceDB unavailable: {m}");
            unavailable()
        }
        AuthzError::Rejected(m) => {
            tracing::error!("SpiceDB rejected a request: {m}");
            ApiError::internal()
        }
        AuthzError::PreconditionFailed(m) => {
            tracing::warn!("SpiceDB write precondition failed: {m}");
            ApiError::new(
                StatusCode::CONFLICT,
                "The resource changed concurrently; try again",
            )
        }
    }
}

/// 401: no usable identity.
pub fn unauthenticated() -> ApiError {
    ApiError::new(StatusCode::UNAUTHORIZED, "Unauthorized")
}

/// 503: SpiceDB or the RBAC database cannot answer.
pub fn unavailable() -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "Authorization is temporarily unavailable",
    )
}
