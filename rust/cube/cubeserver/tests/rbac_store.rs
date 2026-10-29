//! Postgres store for users, organizations, workspaces and objects.
//! Runs only when `CUBE_TEST_PG_URL` points at a Postgres server; each test
//! works in its own throw-away database.

mod support;

use cubeserver::rbac::store::{ConnectionOrigin, NewObject, NewUser, Search, StoreError};
use support::{connect, with_db, TestDb};
use uuid::Uuid;

fn user(id: &str) -> NewUser {
    NewUser {
        id: id.into(),
        name: Some("Ann".into()),
        email: None,
        password_hash: "hash1".into(),
    }
}

/// Runs `body` against a fresh database and always drops it, even when the
/// body panics.
macro_rules! db_test {
    ($name:ident, |$store:ident, $url:ident| $body:block) => {
        #[tokio::test]
        async fn $name() {
            let Some(db) = TestDb::create().await else {
                return;
            };
            let $store = db.store.clone();
            let $url = with_db(&db.admin_url, &db.name);
            let result = tokio::spawn(async move $body).await;
            db.drop_db().await;
            result.unwrap();
        }
    };
}

db_test!(users_roundtrip_and_conflict, |store, _url| {
    let u = store.create_user(&user("ann")).await.unwrap();
    assert_eq!(u.id, "ann");
    assert!(!u.disabled);
    assert!(matches!(
        store.create_user(&user("ann")).await,
        Err(StoreError::Conflict(_))
    ));
    assert_eq!(
        store.user("ann").await.unwrap().unwrap().name.as_deref(),
        Some("Ann")
    );
    assert!(store.user("nobody").await.unwrap().is_none());
    store.create_user(&user("bob")).await.unwrap();
    assert_eq!(store.users().await.unwrap().len(), 2);

    let updated = store
        .update_user("ann", None, Some("a@x.org".into()), Some(true))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.email.as_deref(), Some("a@x.org"));
    assert_eq!(updated.name.as_deref(), Some("Ann"));
    assert!(updated.disabled);
    assert!(store
        .update_user("nobody", None, None, None)
        .await
        .unwrap()
        .is_none());

    store.set_password_hash("ann", "hash2").await.unwrap();
    let after = store.user("ann").await.unwrap().unwrap();
    assert_eq!(after.password_hash, "hash2");
    assert!(after.password_changed_at > u.password_changed_at);
    assert!(matches!(
        store.set_password_hash("nobody", "x").await,
        Err(StoreError::NotFound)
    ));
});

db_test!(
    workspace_names_are_unique_per_organization,
    |store, _url| {
        store.create_user(&user("ann")).await.unwrap();
        let o1 = store.create_organization("o1", "ann").await.unwrap();
        let o2 = store.create_organization("o2", "ann").await.unwrap();
        store.create_workspace(o1.id, "w", "ann").await.unwrap();
        assert!(matches!(
            store.create_workspace(o1.id, "w", "ann").await,
            Err(StoreError::Conflict(_))
        ));
        let other = store.create_workspace(o2.id, "w", "ann").await.unwrap();
        assert_eq!(store.workspaces_by_ids(&[other.id]).await.unwrap().len(), 1);

        store
            .create_connection("c1", o1.id, ConnectionOrigin::Created)
            .await
            .unwrap();
        assert!(matches!(
            store
                .create_connection("c1", o2.id, ConnectionOrigin::Assigned)
                .await,
            Err(StoreError::Conflict(_))
        ));
        assert_eq!(
            store
                .connection("c1")
                .await
                .unwrap()
                .unwrap()
                .organization_id,
            o1.id
        );
    }
);

db_test!(
    object_update_with_a_stale_version_conflicts,
    |store, _url| {
        store.create_user(&user("ann")).await.unwrap();
        let o = store.create_organization("o", "ann").await.unwrap();
        let w = store.create_workspace(o.id, "w", "ann").await.unwrap();
        let obj = store
            .create_object(&NewObject {
                workspace_id: w.id,
                r#type: "report".into(),
                name: "r".into(),
                content_type: "text/plain".into(),
                content: b"v1".to_vec(),
                created_by: "ann".into(),
            })
            .await
            .unwrap();
        assert_eq!(obj.version, 1);
        let v2 = store
            .update_object(obj.id, 1, None, None, Some(b"v2".to_vec()), "ann")
            .await
            .unwrap();
        assert_eq!(v2.version, 2);
        assert_eq!(v2.content, b"v2");
        assert!(matches!(
            store
                .update_object(obj.id, 1, None, None, Some(b"v3".to_vec()), "ann")
                .await,
            Err(StoreError::Conflict(_))
        ));
        assert!(matches!(
            store
                .update_object(Uuid::new_v4(), 1, None, None, None, "ann")
                .await,
            Err(StoreError::NotFound)
        ));
        assert_eq!(store.object(obj.id).await.unwrap().unwrap().content, b"v2");
    }
);

db_test!(deleting_a_workspace_deletes_its_objects, |store, _url| {
    store.create_user(&user("ann")).await.unwrap();
    let o = store.create_organization("o", "ann").await.unwrap();
    let w = store.create_workspace(o.id, "w", "ann").await.unwrap();
    let obj = store
        .create_object(&NewObject {
            workspace_id: w.id,
            r#type: "report".into(),
            name: "r".into(),
            content_type: "text/plain".into(),
            content: vec![],
            created_by: "ann".into(),
        })
        .await
        .unwrap();
    assert!(store.delete_workspace(w.id).await.unwrap());
    assert!(!store.delete_workspace(w.id).await.unwrap());
    assert!(store.object(obj.id).await.unwrap().is_none());
});

db_test!(a_rolled_back_transaction_leaves_nothing, |store, url| {
    store.create_user(&user("ann")).await.unwrap();

    let tx = store.transaction().await.unwrap();
    let o = tx.create_organization("o", "ann").await.unwrap();
    tx.create_workspace(o.id, "w", "ann").await.unwrap();
    tx.rollback().await.unwrap();
    assert!(store
        .organizations_by_ids(&[o.id])
        .await
        .unwrap()
        .is_empty());

    // Dropping without commit also rolls back. The pool has a single
    // connection, so a leaked open transaction would be reused below.
    let tx = store.transaction().await.unwrap();
    let o2 = tx.create_organization("o2", "ann").await.unwrap();
    drop(tx);
    assert!(store
        .organizations_by_ids(&[o2.id])
        .await
        .unwrap()
        .is_empty());
    let o4 = store.create_organization("o4", "ann").await.unwrap();
    // Visible to a fresh connection only if committed, and no connection may
    // be left idle inside a transaction.
    let fresh = connect(&url).await;
    let n: i64 = fresh
        .query_one(
            "SELECT count(*) FROM organizations WHERE id = $1",
            &[&o4.id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(n, 1);
    let idle: i64 = fresh
        .query_one(
            "SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() \
             AND state LIKE 'idle in transaction%'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(idle, 0);

    let tx = store.transaction().await.unwrap();
    let o3 = tx.create_organization("o3", "ann").await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(store.organizations_by_ids(&[o3.id]).await.unwrap().len(), 1);
});

db_test!(delete_organization_in_use_conflicts, |store, _url| {
    store.create_user(&user("ann")).await.unwrap();
    let o = store.create_organization("o", "ann").await.unwrap();
    let w = store.create_workspace(o.id, "w", "ann").await.unwrap();
    assert!(matches!(
        store.delete_organization(o.id).await,
        Err(StoreError::Conflict(_))
    ));
    store.delete_workspace(w.id).await.unwrap();
    assert!(store.delete_organization(o.id).await.unwrap());
});

db_test!(
    renaming_a_workspace_to_an_existing_name_conflicts,
    |store, _url| {
        store.create_user(&user("ann")).await.unwrap();
        let o = store.create_organization("o", "ann").await.unwrap();
        store.create_workspace(o.id, "a", "ann").await.unwrap();
        let b = store.create_workspace(o.id, "b", "ann").await.unwrap();
        assert!(matches!(
            store.update_workspace(b.id, "a").await,
            Err(StoreError::Conflict(_))
        ));
        assert_eq!(
            store
                .update_workspace(b.id, "c")
                .await
                .unwrap()
                .unwrap()
                .name,
            "c"
        );
    }
);

db_test!(
    connection_origin_migrates_old_rows_as_assigned,
    |store, url| {
        store.create_user(&user("ann")).await.unwrap();
        let o = store.create_organization("o", "ann").await.unwrap();
        store
            .create_connection("made", o.id, ConnectionOrigin::Created)
            .await
            .unwrap();
        // A row from before the column: drop it and add the row back the old way.
        let raw = connect(&url).await;
        raw.batch_execute("ALTER TABLE connections DROP COLUMN origin")
            .await
            .unwrap();
        raw.execute(
            "INSERT INTO connections (name, organization_id) VALUES ('old', $1)",
            &[&o.id],
        )
        .await
        .unwrap();
        // Starting again applies the migration to the existing rows.
        let again = cubeserver::rbac::store::Store::connect_with_pool_size(&url, 1)
            .await
            .unwrap();
        let rows = again.connections_of_organization(o.id).await.unwrap();
        let origins: Vec<(&str, ConnectionOrigin)> =
            rows.iter().map(|r| (r.name.as_str(), r.origin)).collect();
        assert_eq!(
            origins,
            [
                ("made", ConnectionOrigin::Assigned),
                ("old", ConnectionOrigin::Assigned)
            ]
        );
        // And once more: idempotent.
        drop(again);
        cubeserver::rbac::store::Store::connect_with_pool_size(&url, 1)
            .await
            .unwrap();
    }
);

db_test!(
    a_delete_in_a_rolled_back_transaction_is_undone,
    |store, _url| {
        store.create_user(&user("ann")).await.unwrap();
        let o = store.create_organization("o", "ann").await.unwrap();
        let w = store.create_workspace(o.id, "w", "ann").await.unwrap();
        store
            .create_connection("c", o.id, ConnectionOrigin::Assigned)
            .await
            .unwrap();

        let tx = store.transaction().await.unwrap();
        assert!(tx.delete_workspace(w.id).await.unwrap());
        assert!(tx.delete_connection("c").await.unwrap());
        drop(tx);
        assert!(store.workspace(w.id).await.unwrap().is_some());
        assert!(store.connection("c").await.unwrap().is_some());

        // A conflicting delete inside a transaction reports the conflict.
        let tx = store.transaction().await.unwrap();
        assert!(matches!(
            tx.delete_organization(o.id).await,
            Err(StoreError::Conflict(_))
        ));
        drop(tx);

        let tx = store.transaction().await.unwrap();
        assert!(tx.delete_workspace(w.id).await.unwrap());
        assert!(tx.delete_connection("c").await.unwrap());
        assert!(tx.delete_organization(o.id).await.unwrap());
        tx.commit().await.unwrap();
        assert!(store.organization(o.id).await.unwrap().is_none());
    }
);

fn search(q: Option<&str>, limit: Option<i64>, offset: i64) -> Search {
    Search {
        q: q.map(str::to_string),
        limit,
        offset,
    }
}

fn user_with(id: &str, name: Option<&str>, email: Option<&str>) -> NewUser {
    NewUser {
        id: id.into(),
        name: name.map(str::to_string),
        email: email.map(str::to_string),
        password_hash: "h".into(),
    }
}

db_test!(
    search_users_matches_id_name_and_email_case_insensitively,
    |store, _url| {
        store
            .create_user(&user_with("ana", Some("Ana Souza"), None))
            .await
            .unwrap();
        store
            .create_user(&user_with("bia", None, Some("ANA@x.com")))
            .await
            .unwrap();
        store
            .create_user(&user_with("caio", Some("Caio"), None))
            .await
            .unwrap();
        let page = store
            .search_users(&search(Some("ana"), None, 0))
            .await
            .unwrap();
        let ids: Vec<_> = page.items.iter().map(|u| u.id.as_str()).collect();
        assert_eq!(ids, ["ana", "bia"]);
        assert_eq!(page.total, 2);
    }
);

db_test!(search_treats_like_wildcards_literally, |store, _url| {
    store.create_user(&user("a_b")).await.unwrap();
    store.create_user(&user("axb")).await.unwrap();
    let page = store
        .search_users(&search(Some("a_b"), None, 0))
        .await
        .unwrap();
    let ids: Vec<_> = page.items.iter().map(|u| u.id.as_str()).collect();
    assert_eq!(ids, ["a_b"]);
    let page = store
        .search_users(&search(Some("%"), None, 0))
        .await
        .unwrap();
    assert!(page.items.is_empty());
    assert_eq!(page.total, 0);
});

db_test!(search_pages_report_the_full_total, |store, _url| {
    for i in 1..=5 {
        store.create_user(&user(&format!("u{i}"))).await.unwrap();
    }
    let page = store.search_users(&search(None, Some(2), 2)).await.unwrap();
    let ids: Vec<_> = page.items.iter().map(|u| u.id.as_str()).collect();
    assert_eq!(ids, ["u3", "u4"]);
    assert_eq!(page.total, 5);
    let page = store
        .search_users(&search(None, Some(2), 10))
        .await
        .unwrap();
    assert!(page.items.is_empty());
    assert_eq!(page.total, 5);
});

db_test!(search_organizations_by_name_or_id_prefix, |store, _url| {
    store.create_user(&user("ann")).await.unwrap();
    let a = store.create_organization("Alpha", "ann").await.unwrap();
    store.create_organization("Beta", "ann").await.unwrap();
    let prefix = a.id.to_string()[..8].to_uppercase();
    let page = store
        .search_organizations(None, &search(Some(&prefix), None, 0))
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].id, a.id);
    let page = store
        .search_organizations(None, &search(Some("alp"), None, 0))
        .await
        .unwrap();
    assert_eq!(page.total, 1);
    let page = store
        .search_organizations(Some(&[]), &search(None, None, 0))
        .await
        .unwrap();
    assert_eq!(page.total, 0);
    assert!(page.items.is_empty());
    let page = store
        .search_organizations(Some(&[a.id]), &search(None, None, 0))
        .await
        .unwrap();
    assert_eq!(page.total, 1);
});

db_test!(
    search_workspaces_matches_the_organization_name,
    |store, _url| {
        store.create_user(&user("ann")).await.unwrap();
        let on = store.create_organization("ON", "ann").await.unwrap();
        let digital = store.create_organization("Digital", "ann").await.unwrap();
        store
            .create_workspace(on.id, "Vendas", "ann")
            .await
            .unwrap();
        store
            .create_workspace(on.id, "Operations", "ann")
            .await
            .unwrap();
        store
            .create_workspace(digital.id, "Finance", "ann")
            .await
            .unwrap();
        let page = store
            .search_workspaces(None, None, &search(Some("on"), None, 0))
            .await
            .unwrap();
        let names: Vec<_> = page
            .items
            .iter()
            .map(|w| w.workspace.name.as_str())
            .collect();
        assert_eq!(names, ["Operations", "Vendas"]);
        assert!(page.items.iter().all(|w| w.organization_name == "ON"));
        assert_eq!(page.total, 2);
        let page = store
            .search_workspaces(None, Some(digital.id), &search(None, None, 0))
            .await
            .unwrap();
        let names: Vec<_> = page
            .items
            .iter()
            .map(|w| w.workspace.name.as_str())
            .collect();
        assert_eq!(names, ["Finance"]);
        let page = store
            .search_workspaces(Some(&[]), None, &search(None, None, 0))
            .await
            .unwrap();
        assert_eq!(page.total, 0);
    }
);

db_test!(
    search_workspaces_keeps_same_named_organizations_apart,
    |store, _url| {
        store.create_user(&user("ann")).await.unwrap();
        let one = store.create_organization("Same", "ann").await.unwrap();
        let two = store.create_organization("Same", "ann").await.unwrap();
        for org in [&one, &two] {
            for name in ["a", "z"] {
                store.create_workspace(org.id, name, "ann").await.unwrap();
            }
        }
        let page = store
            .search_workspaces(None, None, &search(None, None, 0))
            .await
            .unwrap();
        let orgs: Vec<_> = page
            .items
            .iter()
            .map(|w| w.workspace.organization_id)
            .collect();
        let (first, second) = if one.id < two.id {
            (one.id, two.id)
        } else {
            (two.id, one.id)
        };
        assert_eq!(orgs, [first, first, second, second]);
    }
);

db_test!(
    all_connections_lists_every_tracked_connection,
    |store, _url| {
        store.create_user(&user("ann")).await.unwrap();
        let o1 = store.create_organization("o1", "ann").await.unwrap();
        let o2 = store.create_organization("o2", "ann").await.unwrap();
        store
            .create_connection("c2", o2.id, ConnectionOrigin::Assigned)
            .await
            .unwrap();
        store
            .create_connection("c1", o1.id, ConnectionOrigin::Created)
            .await
            .unwrap();
        let all = store.all_connections().await.unwrap();
        let names: Vec<_> = all.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["c1", "c2"]);
        assert_eq!(all[0].organization_id, o1.id);
        assert_eq!(all[1].origin, ConnectionOrigin::Assigned);
    }
);
