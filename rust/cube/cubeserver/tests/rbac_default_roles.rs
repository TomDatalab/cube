//! `default_roles` migration: built-in rules move to their organization's
//! default roles without anyone gaining or losing access.
//! Runs only when `CUBE_TEST_PG_URL` is set.

mod support;

use cubeserver::rbac::authz::{Authz, ObjectRef, Relationship, RelationshipFilter};
use cubeserver::rbac::default_roles::{migrate_default_roles, MARKER};
use cubeserver::rbac::fake::FakeAuthz;
use cubeserver::rbac::store::{NewUser, RuleRole, Store};
use support::TestDb;
use uuid::Uuid;

const USER: &str = "0b6c1d6e-58a5-4c1e-9a43-0d1a2f3b4c5d";

fn rel(kind: &'static str, id: Uuid, relation: &'static str, sk: &'static str, sid: &str) -> Relationship {
    Relationship {
        resource: ObjectRef::new(kind, id.to_string()),
        relation,
        subject: ObjectRef::new(sk, sid),
    }
}

async fn all(authz: &FakeAuthz) -> Vec<String> {
    let mut out = Vec::new();
    for kind in ["organization", "workspace", "rule", "role"] {
        for r in authz
            .read(&RelationshipFilter { kind, id: None, relation: None, subject: None })
            .await
            .unwrap()
        {
            out.push(format!("{r:?}"));
        }
    }
    out.sort();
    out
}

/// An organization with a workspace and one rule on the built-in `key`,
/// its relationships in the old shape, `USER` a member.
async fn old_rule(store: &Store, authz: &FakeAuthz, key: &str) -> (Uuid, Uuid, Uuid) {
    let name = format!("u{}", Uuid::new_v4().simple());
    let u = store
        .create_user(&NewUser {
            username: name.clone(),
            name: None,
            email: None,
            password_hash: "h".into(),
        })
        .await
        .unwrap();
    let o = store.create_organization(&name, &name, &u.username).await.unwrap();
    let w = store.create_workspace(o.id, "w", &u.username).await.unwrap();
    let r = store
        .create_rule(w.id, "r", "member", RuleRole::Builtin(key), &u.username)
        .await
        .unwrap();
    let old = match key {
        "owner" => "owner_rule",
        "editor" => "editor_rule",
        _ => "viewer_rule",
    };
    authz
        .touch(&[
            rel("organization", o.id, "rule", "rule", &r.id.to_string()),
            rel("workspace", w.id, old, "rule", &r.id.to_string()),
            rel("rule", r.id, "member", "user", USER),
        ])
        .await
        .unwrap();
    (o.id, w.id, r.id)
}

async fn can(authz: &FakeAuthz, ws: Uuid, permission: &str) -> bool {
    authz
        .check(&ObjectRef::new("workspace", ws.to_string()), permission, USER)
        .await
        .unwrap()
}

macro_rules! db_test {
    ($name:ident, |$store:ident| $body:block) => {
        #[tokio::test]
        async fn $name() {
            let Some(db) = TestDb::create().await else {
                return;
            };
            let $store = db.store.clone();
            let result = tokio::spawn(async move $body).await;
            db.drop_db().await;
            result.unwrap();
        }
    };
}

db_test!(a_builtin_rule_moves_to_the_default_role_and_keeps_its_permissions, |store| {
    // Per key: the permissions it holds among those checked below.
    const CHECKED: [&str; 6] = ["view", "query", "read_objects", "write_objects", "execute", "manage"];
    let cases: [(&str, &[&str]); 3] = [
        ("owner", &CHECKED),
        ("editor", &["view", "query", "read_objects", "write_objects"]),
        ("viewer", &["view", "query", "read_objects"]),
    ];
    let authz = FakeAuthz::new();
    let mut made = Vec::new();
    for (key, held) in cases {
        let (org, ws, rule) = old_rule(&store, &authz, key).await;
        for p in CHECKED {
            assert_eq!(can(&authz, ws, p).await, held.contains(&p), "{key} before {p}");
        }
        made.push((key, org, ws, rule, held));
    }

    assert_eq!(migrate_default_roles(&store, &authz).await.unwrap(), 3);

    let rels = all(&authz).await.join("\n");
    assert!(!rels.contains("_rule"), "{rels}");
    for (key, org, ws, rule, held) in made {
        for p in CHECKED {
            assert_eq!(can(&authz, ws, p).await, held.contains(&p), "{key} after {p}");
        }

        let role = store.default_role(org, key).await.unwrap().unwrap();
        let moved = store.rule(rule).await.unwrap().unwrap();
        assert_eq!(moved.role_id, Some(role.id));
        assert_eq!(moved.role, None);
        let on_rule = authz
            .read(&RelationshipFilter {
                kind: "rule",
                id: Some(rule.to_string()),
                relation: Some("role"),
                subject: None,
            })
            .await
            .unwrap();
        assert_eq!(on_rule.len(), 1);
        assert_eq!(on_rule[0].subject.id, role.id.to_string());
    }
});

db_test!(a_second_run_changes_nothing, |store| {
    let authz = FakeAuthz::new();
    old_rule(&store, &authz, "editor").await;
    assert_eq!(migrate_default_roles(&store, &authz).await.unwrap(), 1);
    let after = all(&authz).await;
    assert!(store.migration_applied(MARKER).await.unwrap());
    assert_eq!(migrate_default_roles(&store, &authz).await.unwrap(), 0);
    assert_eq!(all(&authz).await, after);
});

db_test!(a_rerun_after_spicedb_moved_but_the_row_did_not_finishes_the_move, |store| {
    let authz = FakeAuthz::new();
    let (org, ws, rule) = old_rule(&store, &authz, "viewer").await;
    // A crashed first run: roles seeded, SpiceDB moved, row and marker not.
    let roles = store.seed_default_roles(org, "system").await.unwrap();
    let viewer = roles.iter().find(|r| r.builtin.as_deref() == Some("viewer")).unwrap();
    authz
        .touch(&[
            rel("workspace", ws, "rule", "rule", &rule.to_string()),
            rel("rule", rule, "role", "role", &viewer.id.to_string()),
        ])
        .await
        .unwrap();
    authz
        .delete(&[rel("workspace", ws, "viewer_rule", "rule", &rule.to_string())])
        .await
        .unwrap();
    assert!(store.rule(rule).await.unwrap().unwrap().role.is_some());

    assert_eq!(migrate_default_roles(&store, &authz).await.unwrap(), 1);

    let r = store.rule(rule).await.unwrap().unwrap();
    assert_eq!(r.role_id, Some(viewer.id));
    assert_eq!(r.role, None);
    assert!(can(&authz, ws, "query").await);
    assert!(!can(&authz, ws, "write_objects").await);
});
