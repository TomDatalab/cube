//! Authorization (RBAC) backed by SpiceDB.

pub mod authz;
pub mod config;
pub mod enforce;
pub mod fake;
pub mod handlers;
pub mod identity;
pub mod store;

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
        identity::prepare_dummy_hash();
        let hashing = Arc::new(Semaphore::new(config.max_concurrent_hashes.max(1)));
        let rbac = Self {
            authz,
            store,
            config,
            hashing,
            running: Default::default(),
        };
        rbac.bootstrap_admins().await?;
        Ok(rbac)
    }

    /// `CUBE_RBAC_ADMINS`: creates the missing ones with
    /// `CUBE_RBAC_ADMIN_PASSWORD` and grants `platform:main#admin` to all.
    /// Removing an id from the list revokes nothing.
    async fn bootstrap_admins(&self) -> Result<(), String> {
        let mut grants = Vec::new();
        for id in &self.config.admins {
            let existing = self
                .store
                .user(id)
                .await
                .map_err(|e| format!("Cannot read user {id}: {e}"))?;
            if existing.is_none() {
                let password = self.config.admin_password.as_deref().ok_or_else(|| {
                    format!(
                        "Admin {id} (CUBE_RBAC_ADMINS) does not exist and \
                         CUBE_RBAC_ADMIN_PASSWORD is not set"
                    )
                })?;
                let password_hash = identity::hash_password(password)
                    .map_err(|e| format!("CUBE_RBAC_ADMIN_PASSWORD: {}", e.body.error))?;
                let created = self
                    .store
                    .create_user(&NewUser {
                        id: id.clone(),
                        name: None,
                        email: None,
                        password_hash,
                    })
                    .await;
                match created {
                    // Another instance starting at the same time made it.
                    Ok(_) | Err(StoreError::Conflict(_)) => {}
                    Err(e) => return Err(format!("Cannot create admin {id}: {e}")),
                }
            }
            grants.push(Relationship {
                resource: platform(),
                relation: "admin",
                subject: ObjectRef::new("user", id.clone()),
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

    /// Test hook: the semaphore bounding concurrent hashes, so a test can
    /// hold its permits as an in-flight verification would.
    pub fn hashing_permits(&self) -> Arc<Semaphore> {
        self.hashing.clone()
    }

    pub async fn is_platform_admin(&self, user: &str) -> Result<bool, ApiError> {
        self.check(&platform(), "manage", user).await
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
