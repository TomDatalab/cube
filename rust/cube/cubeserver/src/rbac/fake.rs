//! In-memory `Authz` mirroring `rbac/schema.zed`, for tests.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;

use super::authz::{Authz, AuthzError, ObjectRef, Relationship, RelationshipFilter};

/// One term of a permission expression: a direct relation or `relation->perm`.
enum Term {
    Rel(&'static str),
    Arrow(&'static str, &'static str),
}

/// Permission definitions, written from `rbac/schema.zed`.
fn definition(kind: &str, name: &str) -> Option<Vec<Term>> {
    use Term::*;
    Some(match (kind, name) {
        ("platform", "manage") => vec![Rel("admin")],
        ("organization", "admin") => vec![Rel("owner"), Arrow("platform", "manage")],
        ("organization", "view") => vec![Rel("admin"), Rel("member")],
        ("organization", "manage_connections") => vec![Rel("admin")],
        ("workspace", "manage") => vec![Rel("owner"), Arrow("organization", "admin")],
        ("workspace", "edit") => vec![Rel("editor"), Rel("manage")],
        ("workspace", "view") => vec![Rel("viewer"), Rel("edit")],
        ("workspace", "query") => vec![Rel("view")],
        ("workspace", "execute") => vec![Rel("manage")],
        ("connection", "manage") => vec![Arrow("organization", "admin")],
        ("connection", "query") => vec![Arrow("workspace", "query"), Rel("manage")],
        ("object", "read") => vec![Arrow("workspace", "view")],
        ("object", "write") => vec![Arrow("workspace", "edit")],
        _ => return None,
    })
}

#[derive(Debug, Default)]
pub struct FakeAuthz {
    relationships: Mutex<Vec<Relationship>>,
    failing: AtomicBool,
}

impl FakeAuthz {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_failing(&self, failing: bool) {
        self.failing.store(failing, Ordering::SeqCst);
    }

    fn guard(&self) -> Result<(), AuthzError> {
        if self.failing.load(Ordering::SeqCst) {
            Err(AuthzError::Unavailable("fake authz set to fail".into()))
        } else {
            Ok(())
        }
    }

    /// Whether `name` on `kind:id` is granted to `user`. A relation or
    /// permission name is resolved against the relationship store.
    fn has(&self, rels: &[Relationship], kind: &str, id: &str, name: &str, user: &str) -> bool {
        self.has_depth(rels, kind, id, name, user, 0)
    }

    fn has_depth(
        &self,
        rels: &[Relationship],
        kind: &str,
        id: &str,
        name: &str,
        user: &str,
        depth: usize,
    ) -> bool {
        if depth > 16 {
            return false;
        }
        if let Some(terms) = definition(kind, name) {
            return terms.iter().any(|t| match t {
                Term::Rel(r) => self.has_depth(rels, kind, id, r, user, depth + 1),
                Term::Arrow(rel, perm) => rels
                    .iter()
                    .filter(|x| {
                        x.resource.kind == kind && x.resource.id == id && x.relation == *rel
                    })
                    .any(|x| {
                        self.has_depth(rels, x.subject.kind, &x.subject.id, perm, user, depth + 1)
                    }),
            });
        }
        // A plain relation: direct user subject.
        rels.iter().any(|x| {
            x.resource.kind == kind
                && x.resource.id == id
                && x.relation == name
                && x.subject.kind == "user"
                && x.subject.id == user
        })
    }
}

fn matches(f: &RelationshipFilter, r: &Relationship) -> bool {
    r.resource.kind == f.kind
        && f.id.as_ref().is_none_or(|i| *i == r.resource.id)
        && f.relation.is_none_or(|rel| rel == r.relation)
        && f.subject.as_ref().is_none_or(|s| *s == r.subject)
}

#[async_trait]
impl Authz for FakeAuthz {
    async fn check(
        &self,
        resource: &ObjectRef,
        permission: &str,
        user: &str,
    ) -> Result<bool, AuthzError> {
        self.guard()?;
        let rels = self.relationships.lock().unwrap();
        Ok(self.has(&rels, resource.kind, &resource.id, permission, user))
    }

    async fn lookup_resources(
        &self,
        kind: &'static str,
        permission: &str,
        user: &str,
    ) -> Result<Vec<String>, AuthzError> {
        self.guard()?;
        let rels = self.relationships.lock().unwrap();
        let ids: BTreeSet<&str> = rels
            .iter()
            .flat_map(|r| [&r.resource, &r.subject])
            .filter(|o| o.kind == kind)
            .map(|o| o.id.as_str())
            .collect();
        Ok(ids
            .into_iter()
            .filter(|id| self.has(&rels, kind, id, permission, user))
            .map(String::from)
            .collect())
    }

    async fn read(&self, filter: &RelationshipFilter) -> Result<Vec<Relationship>, AuthzError> {
        self.guard()?;
        let rels = self.relationships.lock().unwrap();
        Ok(rels
            .iter()
            .filter(|r| matches(filter, r))
            .cloned()
            .collect())
    }

    async fn touch(&self, relationships: &[Relationship]) -> Result<(), AuthzError> {
        self.guard()?;
        let mut rels = self.relationships.lock().unwrap();
        for r in relationships {
            if !rels.contains(r) {
                rels.push(r.clone());
            }
        }
        Ok(())
    }

    async fn delete(&self, relationships: &[Relationship]) -> Result<(), AuthzError> {
        self.guard()?;
        self.relationships
            .lock()
            .unwrap()
            .retain(|r| !relationships.contains(r));
        Ok(())
    }

    async fn delete_matching(&self, filter: &RelationshipFilter) -> Result<(), AuthzError> {
        self.guard()?;
        self.relationships
            .lock()
            .unwrap()
            .retain(|r| !matches(filter, r));
        Ok(())
    }

    async fn ensure_schema(&self, _schema: &str) -> Result<(), AuthzError> {
        self.guard()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(
        rk: &'static str,
        rid: &str,
        relation: &'static str,
        sk: &'static str,
        sid: &str,
    ) -> Relationship {
        Relationship {
            resource: ObjectRef::new(rk, rid),
            relation,
            subject: ObjectRef::new(sk, sid),
        }
    }

    #[tokio::test]
    async fn org_owner_can_edit_every_workspace_of_the_org() {
        let a = FakeAuthz::new();
        a.touch(&[
            rel("organization", "o1", "owner", "user", "alice"),
            rel("workspace", "w1", "organization", "organization", "o1"),
            rel("workspace", "w2", "organization", "organization", "o2"),
        ])
        .await
        .unwrap();
        assert!(a
            .check(&ObjectRef::new("workspace", "w1"), "edit", "alice")
            .await
            .unwrap());
        assert!(!a
            .check(&ObjectRef::new("workspace", "w2"), "edit", "alice")
            .await
            .unwrap());
        assert_eq!(
            a.lookup_resources("workspace", "edit", "alice")
                .await
                .unwrap(),
            vec!["w1".to_string()]
        );
    }

    #[tokio::test]
    async fn viewer_reads_objects_but_cannot_write() {
        let a = FakeAuthz::new();
        a.touch(&[
            rel("workspace", "w1", "viewer", "user", "bob"),
            rel("object", "d1", "workspace", "workspace", "w1"),
        ])
        .await
        .unwrap();
        let obj = ObjectRef::new("object", "d1");
        assert!(a.check(&obj, "read", "bob").await.unwrap());
        assert!(!a.check(&obj, "write", "bob").await.unwrap());
        assert!(!a
            .check(&ObjectRef::new("workspace", "w1"), "execute", "bob")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn connection_query_follows_release() {
        let a = FakeAuthz::new();
        a.touch(&[
            rel("organization", "o1", "owner", "user", "admin"),
            rel("workspace", "w1", "organization", "organization", "o1"),
            rel("workspace", "w1", "viewer", "user", "bob"),
            rel("workspace", "w2", "viewer", "user", "carol"),
            rel("connection", "c1", "organization", "organization", "o1"),
            rel("connection", "c1", "workspace", "workspace", "w1"),
        ])
        .await
        .unwrap();
        let c = ObjectRef::new("connection", "c1");
        assert!(a.check(&c, "query", "bob").await.unwrap());
        assert!(a.check(&c, "query", "admin").await.unwrap());
        assert!(!a.check(&c, "query", "carol").await.unwrap());
        assert!(!a.check(&c, "manage", "bob").await.unwrap());
    }

    #[tokio::test]
    async fn platform_admin_administers_every_org() {
        let a = FakeAuthz::new();
        a.touch(&[
            rel("platform", "main", "admin", "user", "root"),
            rel("organization", "o1", "platform", "platform", "main"),
        ])
        .await
        .unwrap();
        let o = ObjectRef::new("organization", "o1");
        assert!(a.check(&o, "admin", "root").await.unwrap());
        assert!(a.check(&o, "manage_connections", "root").await.unwrap());
        assert!(!a.check(&o, "admin", "nobody").await.unwrap());
    }

    #[tokio::test]
    async fn failing_returns_unavailable() {
        let a = FakeAuthz::new();
        a.set_failing(true);
        assert!(matches!(
            a.check(&ObjectRef::new("organization", "o"), "admin", "u")
                .await,
            Err(AuthzError::Unavailable(_))
        ));
    }
}
