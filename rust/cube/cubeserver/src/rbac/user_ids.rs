//! `spicedb_user_uuid`: SpiceDB named a user by their username. Every
//! relationship whose subject is `user:<username>` is rewritten to
//! `user:<uuid>` once, at start-up, after the `users_uuid` store migration.

use async_trait::async_trait;

use super::authz::{Authz, ObjectRef, Relationship, RelationshipFilter};
use super::store::{is_uuid, Store};

/// The marker recorded in `rbac_migrations` once a full pass completed.
pub const MARKER: &str = "spicedb_user_uuid";

/// Every relation whose subject is a user. `role#can_*@user:*` is a
/// wildcard, not a user, and stays as it is.
pub const USER_RELATIONS: &[(&str, &str)] = &[
    ("platform", "admin"),
    ("organization", "owner"),
    ("organization", "member"),
    ("workspace", "owner"),
    ("workspace", "editor"),
    ("workspace", "viewer"),
    ("rule", "member"),
];

/// Finds the id of the user with a username.
#[async_trait]
pub trait UserIds: Send + Sync {
    async fn id_of(&self, username: &str) -> Result<Option<String>, String>;
}

#[async_trait]
impl UserIds for Store {
    async fn id_of(&self, username: &str) -> Result<Option<String>, String> {
        self.user_by_username(username)
            .await
            .map(|u| u.map(|u| u.id))
            .map_err(|e| format!("Cannot read user {username}: {e}"))
    }
}

/// Rewrites the user subjects to UUIDs unless the marker says it was done,
/// then records the marker. Returns how many relationships it rewrote.
pub async fn migrate_user_subjects(store: &Store, authz: &dyn Authz) -> Result<usize, String> {
    let applied = store
        .migration_applied(MARKER)
        .await
        .map_err(|e| format!("Cannot read the migration marker {MARKER}: {e}"))?;
    if applied {
        return Ok(0);
    }
    let rewritten = rewrite_user_subjects(store, authz).await?;
    store
        .record_migration(MARKER)
        .await
        .map_err(|e| format!("Cannot record the migration marker {MARKER}: {e}"))?;
    tracing::info!("{MARKER}: {rewritten} SpiceDB relationships now name users by UUID");
    Ok(rewritten)
}

/// One pass over [`USER_RELATIONS`]: a subject that is already a UUID is
/// skipped; one naming a user by username gets its UUID relationship
/// touched, then the username one deleted (a crash in between leaves both,
/// and the next pass deletes the username one). A subject that names no
/// user is left and logged.
pub async fn rewrite_user_subjects(
    users: &dyn UserIds,
    authz: &dyn Authz,
) -> Result<usize, String> {
    let spicedb = |e| format!("Cannot rewrite SpiceDB user subjects (SPICEDB_ENDPOINT): {e}");
    let mut rewritten = 0;
    for &(kind, relation) in USER_RELATIONS {
        // SpiceDB streams every match when no limit is set: one read is all.
        let found = authz
            .read(&RelationshipFilter {
                kind,
                id: None,
                relation: Some(relation),
                subject: None,
            })
            .await
            .map_err(spicedb)?;
        for old in found {
            if old.subject.kind != "user" || is_uuid(&old.subject.id) {
                continue;
            }
            let Some(id) = users.id_of(&old.subject.id).await? else {
                tracing::warn!(
                    "{MARKER}: {kind}:{}#{relation}@user:{} names no user; left as it is",
                    old.resource.id,
                    old.subject.id
                );
                continue;
            };
            let new = Relationship {
                subject: ObjectRef::new("user", id),
                ..old.clone()
            };
            authz
                .touch(std::slice::from_ref(&new))
                .await
                .map_err(spicedb)?;
            authz
                .delete(std::slice::from_ref(&old))
                .await
                .map_err(spicedb)?;
            tracing::info!(
                "{MARKER}: {kind}:{}#{relation}@user:{} -> user:{}",
                old.resource.id,
                old.subject.id,
                new.subject.id
            );
            rewritten += 1;
        }
    }
    Ok(rewritten)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::rbac::fake::FakeAuthz;

    struct Users(HashMap<String, String>);

    #[async_trait]
    impl UserIds for Users {
        async fn id_of(&self, username: &str) -> Result<Option<String>, String> {
            Ok(self.0.get(username).cloned())
        }
    }

    const ANN: &str = "0b6c1d6e-58a5-4c1e-9a43-0d1a2f3b4c5d";
    const BOB: &str = "7f3e2d1c-0b9a-4876-a543-210fedcba987";

    fn users() -> Users {
        Users(HashMap::from([
            ("ann".to_string(), ANN.to_string()),
            ("bob".to_string(), BOB.to_string()),
        ]))
    }

    fn rel(kind: &'static str, id: &str, relation: &'static str, user: &str) -> Relationship {
        Relationship {
            resource: ObjectRef::new(kind, id),
            relation,
            subject: ObjectRef::new("user", user),
        }
    }

    async fn all(authz: &FakeAuthz) -> Vec<Relationship> {
        let mut out = Vec::new();
        for kind in ["platform", "organization", "workspace", "rule", "role"] {
            out.extend(
                authz
                    .read(&RelationshipFilter {
                        kind,
                        id: None,
                        relation: None,
                        subject: None,
                    })
                    .await
                    .unwrap(),
            );
        }
        out.sort_by_key(|r| format!("{r:?}"));
        out
    }

    fn sorted(mut v: Vec<Relationship>) -> Vec<Relationship> {
        v.sort_by_key(|r| format!("{r:?}"));
        v
    }

    #[tokio::test]
    async fn subjects_named_by_username_move_to_the_uuid() {
        let authz = FakeAuthz::new();
        let before: Vec<Relationship> = USER_RELATIONS
            .iter()
            .map(|(kind, relation)| rel(kind, "x", relation, "ann"))
            .chain([rel("workspace", "w2", "viewer", "bob")])
            .collect();
        authz.touch(&before).await.unwrap();

        let n = rewrite_user_subjects(&users(), &authz).await.unwrap();

        assert_eq!(n, USER_RELATIONS.len() + 1);
        let expected: Vec<Relationship> = USER_RELATIONS
            .iter()
            .map(|(kind, relation)| rel(kind, "x", relation, ANN))
            .chain([rel("workspace", "w2", "viewer", BOB)])
            .collect();
        assert_eq!(all(&authz).await, sorted(expected));
    }

    #[tokio::test]
    async fn a_second_run_changes_nothing() {
        let authz = FakeAuthz::new();
        authz
            .touch(&[
                rel("organization", "o", "owner", "ann"),
                rel("rule", "r", "member", BOB),
            ])
            .await
            .unwrap();
        assert_eq!(rewrite_user_subjects(&users(), &authz).await.unwrap(), 1);
        let after_first = all(&authz).await;

        assert_eq!(rewrite_user_subjects(&users(), &authz).await.unwrap(), 0);
        assert_eq!(all(&authz).await, after_first);
    }

    #[tokio::test]
    async fn a_crash_between_touch_and_delete_converges() {
        // Both twins present: the pass deletes the username one.
        let authz = FakeAuthz::new();
        authz
            .touch(&[
                rel("workspace", "w", "editor", "ann"),
                rel("workspace", "w", "editor", ANN),
            ])
            .await
            .unwrap();
        assert_eq!(rewrite_user_subjects(&users(), &authz).await.unwrap(), 1);
        assert_eq!(
            all(&authz).await,
            vec![rel("workspace", "w", "editor", ANN)]
        );
    }

    #[tokio::test]
    async fn a_subject_with_no_user_row_is_left_and_logged() {
        let authz = FakeAuthz::new();
        let ghost = rel("organization", "o", "member", "ghost");
        authz
            .touch(&[ghost.clone(), rel("organization", "o", "member", "ann")])
            .await
            .unwrap();

        assert_eq!(rewrite_user_subjects(&users(), &authz).await.unwrap(), 1);
        assert_eq!(
            all(&authz).await,
            sorted(vec![ghost, rel("organization", "o", "member", ANN)])
        );
    }

    #[tokio::test]
    async fn role_wildcards_are_untouched() {
        let authz = FakeAuthz::new();
        let wildcards: Vec<Relationship> = ["can_view", "can_query", "can_manage"]
            .into_iter()
            .map(|relation| rel("role", "viewer", relation, "*"))
            .collect();
        authz.touch(&wildcards).await.unwrap();
        // A username that happens to match a role id or `*` changes nothing.
        let users = Users(HashMap::from([("*".to_string(), ANN.to_string())]));

        assert_eq!(rewrite_user_subjects(&users, &authz).await.unwrap(), 0);
        assert_eq!(all(&authz).await, sorted(wildcards));
    }

    #[tokio::test]
    async fn an_unavailable_spicedb_is_an_error() {
        let authz = FakeAuthz::new();
        authz.set_failing(true);
        assert!(rewrite_user_subjects(&users(), &authz).await.is_err());
    }
}
