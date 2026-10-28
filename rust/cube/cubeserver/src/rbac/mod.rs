//! Authorization (RBAC) backed by SpiceDB.

pub mod authz;
pub mod fake;
pub mod store;

pub use authz::{Authz, AuthzError, HttpAuthz, ObjectRef, Relationship, RelationshipFilter};

/// In-memory implementation for tests, also usable from integration tests.
pub mod testing {
    pub use super::fake::FakeAuthz;
}
