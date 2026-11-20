//! Postgres store for users, organizations, workspaces and objects.
//! Runs only when `CUBE_TEST_PG_URL` points at a Postgres server; each test
//! works in its own throw-away database.

mod support;

use cubeserver::rbac::restrictions::{Attributes, Restrictions, RowFilter, RowOperator};
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
        let o1 = store.create_organization("o1", "o1", "ann").await.unwrap();
        let o2 = store.create_organization("o2", "o2", "ann").await.unwrap();
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
        let o = store.create_organization("o", "o", "ann").await.unwrap();
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
    let o = store.create_organization("o", "o", "ann").await.unwrap();
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
    let o = tx.create_organization("o", "o", "ann").await.unwrap();
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
    let o2 = tx.create_organization("o2", "o2", "ann").await.unwrap();
    drop(tx);
    assert!(store
        .organizations_by_ids(&[o2.id])
        .await
        .unwrap()
        .is_empty());
    let o4 = store.create_organization("o4", "o4", "ann").await.unwrap();
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
    let o3 = tx.create_organization("o3", "o3", "ann").await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(store.organizations_by_ids(&[o3.id]).await.unwrap().len(), 1);
});

db_test!(delete_organization_in_use_conflicts, |store, _url| {
    store.create_user(&user("ann")).await.unwrap();
    let o = store.create_organization("o", "o", "ann").await.unwrap();
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
        let o = store.create_organization("o", "o", "ann").await.unwrap();
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
        let o = store.create_organization("o", "o", "ann").await.unwrap();
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
        let o = store.create_organization("o", "o", "ann").await.unwrap();
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
    let a = store.create_organization("Alpha", "alpha", "ann").await.unwrap();
    store.create_organization("Beta", "beta", "ann").await.unwrap();
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
        let on = store.create_organization("ON", "on", "ann").await.unwrap();
        let digital = store.create_organization("Digital", "digital", "ann").await.unwrap();
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
        let one = store.create_organization("Same", "same", "ann").await.unwrap();
        let two = store.create_organization("Same", "same-2", "ann").await.unwrap();
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
        let o1 = store.create_organization("o1", "o1", "ann").await.unwrap();
        let o2 = store.create_organization("o2", "o2", "ann").await.unwrap();
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

db_test!(role_restrictions_round_trip, |store, _url| {
    store.create_user(&user("ann")).await.unwrap();
    let o = store.create_organization("o", "o", "ann").await.unwrap();
    let restrictions = Restrictions {
        row_filters: vec![
            RowFilter {
                member: "orders.region".into(),
                operator: RowOperator::Equals,
                attribute: Some("region".into()),
                values: None,
            },
            RowFilter {
                member: "orders.status".into(),
                operator: RowOperator::NotEquals,
                attribute: None,
                values: Some(vec!["internal".into()]),
            },
        ],
        blocked_members: vec!["orders.margin".into()],
    };
    let created = store
        .create_role(
            o.id,
            "analyst",
            &["workspace#query".into()],
            &restrictions,
            "ann",
        )
        .await
        .unwrap();
    assert_eq!(created.restrictions, restrictions);
    assert_eq!(
        store.role(created.id).await.unwrap().unwrap().restrictions,
        restrictions
    );
    let by_ids = store.roles_by_ids(&[created.id]).await.unwrap();
    assert_eq!(by_ids[0].restrictions, restrictions);

    let plain = store
        .update_role(created.id, "analyst", &[], &Restrictions::default())
        .await
        .unwrap()
        .unwrap();
    assert!(plain.restrictions.is_empty());
    let listed = store.roles_of_organization(o.id).await.unwrap();
    assert!(listed[0].role.restrictions.is_empty());
});

db_test!(user_attributes_are_replaced_as_a_set, |store, _url| {
    store.create_user(&user("ann")).await.unwrap();
    assert!(store.user_attributes("ann").await.unwrap().is_empty());
    let first = Attributes::from([
        (
            "region".to_string(),
            vec!["Sul".to_string(), "Sudeste".to_string()],
        ),
        ("cost_center".to_string(), vec!["1020".to_string()]),
    ]);
    store.set_user_attributes("ann", &first).await.unwrap();
    assert_eq!(store.user_attributes("ann").await.unwrap(), first);

    let second = Attributes::from([("region".to_string(), vec!["Norte".to_string()])]);
    store.set_user_attributes("ann", &second).await.unwrap();
    assert_eq!(store.user_attributes("ann").await.unwrap(), second);
});

db_test!(deleting_a_user_deletes_their_attributes, |store, url| {
    store.create_user(&user("ann")).await.unwrap();
    let attributes = Attributes::from([("region".to_string(), vec!["Sul".to_string()])]);
    store.set_user_attributes("ann", &attributes).await.unwrap();
    let raw = connect(&url).await;
    raw.execute("DELETE FROM users WHERE id = 'ann'", &[])
        .await
        .unwrap();
    assert!(store.user_attributes("ann").await.unwrap().is_empty());
    let left: i64 = raw
        .query_one("SELECT count(*) FROM user_attributes", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(left, 0);
});

db_test!(
    moving_a_connection_changes_its_organization_and_origin,
    |store, _url| {
        store.create_user(&user("ann")).await.unwrap();
        let a = store.create_organization("a", "a", "ann").await.unwrap();
        let b = store.create_organization("b", "b", "ann").await.unwrap();
        store
            .create_connection("x", a.id, ConnectionOrigin::Created)
            .await
            .unwrap();
        assert!(store.move_connection("x", a.id, b.id).await.unwrap());
        let row = store.connection("x").await.unwrap().unwrap();
        assert_eq!(row.organization_id, b.id);
        assert_eq!(row.origin, ConnectionOrigin::Assigned);
        assert!(!store.move_connection("nope", a.id, b.id).await.unwrap());
        assert!(store.delete_organization(a.id).await.unwrap());
    }
);

db_test!(
    a_move_from_an_organization_it_left_changes_nothing,
    |store, _url| {
        store.create_user(&user("ann")).await.unwrap();
        let a = store.create_organization("a", "a", "ann").await.unwrap();
        let b = store.create_organization("b", "b", "ann").await.unwrap();
        let c = store.create_organization("c", "c", "ann").await.unwrap();
        store
            .create_connection("x", a.id, ConnectionOrigin::Created)
            .await
            .unwrap();
        // Another move took it to B first: the one from A to C finds nothing.
        assert!(store.move_connection("x", a.id, b.id).await.unwrap());
        assert!(!store.move_connection("x", a.id, c.id).await.unwrap());
        let row = store.connection("x").await.unwrap().unwrap();
        assert_eq!(row.organization_id, b.id);
    }
);

db_test!(data_sources_put_get_list_delete, |store, _url| {
    let settings: std::collections::BTreeMap<String, String> =
        [("CUBEJS_DB_HOST".to_string(), "h".to_string())].into();
    let id = store
        .put_data_source("zeta", "postgres", &settings, Some(b"sealed"))
        .await
        .unwrap();
    store
        .put_data_source("alpha", "sqlite", &Default::default(), None)
        .await
        .unwrap();

    let row = store.data_source("zeta").await.unwrap().unwrap();
    assert_eq!(row.id, id);
    assert_eq!(row.db_type, "postgres");
    assert_eq!(row.settings, settings);
    assert_eq!(row.secrets.as_deref(), Some(&b"sealed"[..]));
    assert!(store.data_source("nobody").await.unwrap().is_none());

    // Putting it again replaces the settings and keeps the id.
    let other: std::collections::BTreeMap<String, String> =
        [("CUBEJS_DB_HOST".to_string(), "h2".to_string())].into();
    let again = store
        .put_data_source("zeta", "postgres", &other, None)
        .await
        .unwrap();
    assert_eq!(again, id);
    let row = store.data_source("zeta").await.unwrap().unwrap();
    assert_eq!(row.settings, other);
    assert!(row.secrets.is_none());

    let names: Vec<String> = store
        .data_sources()
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.name)
        .collect();
    assert_eq!(names, ["alpha", "zeta"]);

    assert!(store.delete_data_source("zeta").await.unwrap());
    assert!(!store.delete_data_source("zeta").await.unwrap());
});

db_test!(
    delete_connection_of_requires_the_organization,
    |store, _url| {
        store.create_user(&user("ann")).await.unwrap();
        let a = store.create_organization("a", "a", "ann").await.unwrap();
        let b = store.create_organization("b", "b", "ann").await.unwrap();
        store
            .create_connection("c1", a.id, ConnectionOrigin::Assigned)
            .await
            .unwrap();

        let tx = store.transaction().await.unwrap();
        assert!(!tx.delete_connection_of("c1", b.id).await.unwrap());
        tx.commit().await.unwrap();
        assert!(store.connection("c1").await.unwrap().is_some());

        let tx = store.transaction().await.unwrap();
        assert!(tx.delete_connection_of("c1", a.id).await.unwrap());
        tx.commit().await.unwrap();
        assert!(store.connection("c1").await.unwrap().is_none());
    }
);

db_test!(save_then_load_decrypts, |store, url| {
    use base64::Engine;
    use cubeserver::rbac::data_sources::{load, load_all, save};
    use cubeserver::rbac::secrets::SecretKey;
    let key = SecretKey::from_base64(&base64::engine::general_purpose::STANDARD.encode([7u8; 32]))
        .unwrap();
    let vars: std::collections::BTreeMap<String, String> = [
        ("CUBEJS_DB_TYPE", "postgres"),
        ("CUBEJS_DB_HOST", "h"),
        ("CUBEJS_DB_PASS", "very-secret-pw"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let id = save(&store, &key, "sales", &vars).await.unwrap();
    let got = load(&store, &key, "sales").await.unwrap().unwrap();
    assert_eq!(got.id, id);
    assert_eq!(got.variables, vars);
    assert_eq!(load_all(&store, &key).await.unwrap(), vec![got]);
    assert!(load(&store, &key, "nobody").await.unwrap().is_none());

    // The password is not in the database in the clear.
    let c = connect(&url).await;
    let row = c
        .query_one(
            "SELECT settings::text, encode(secrets, 'escape') FROM data_sources WHERE name = 'sales'",
            &[],
        )
        .await
        .unwrap();
    let settings: String = row.get(0);
    let secrets: String = row.get(1);
    assert!(!settings.contains("very-secret-pw") && !secrets.contains("very-secret-pw"));
    assert!(settings.contains("CUBEJS_DB_HOST"));

    // Without CUBEJS_DB_TYPE it is refused.
    let mut untyped = vars.clone();
    untyped.remove("CUBEJS_DB_TYPE");
    assert!(save(&store, &key, "x", &untyped).await.is_err());
});

fn pg_vars(password: &str) -> std::collections::BTreeMap<String, String> {
    [("CUBEJS_DB_TYPE", "postgres"), ("CUBEJS_DB_HOST", "h"), ("CUBEJS_DB_PASS", password)]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

db_test!(a_secret_opens_only_for_its_own_data_source, |store, url| {
    use cubeserver::rbac::data_sources::{load, save};
    let key = bootstrap_support::key();
    save(&store, &key, "a", &pg_vars("a-password")).await.unwrap();
    save(&store, &key, "b", &pg_vars("b-password")).await.unwrap();
    // a's sealed secrets copied onto b.
    let c = connect(&url).await;
    c.execute(
        "UPDATE data_sources SET secrets = (SELECT secrets FROM data_sources WHERE name = 'a') WHERE name = 'b'",
        &[],
    )
    .await
    .unwrap();
    let err = load(&store, &key, "b").await.unwrap_err();
    assert!(err.contains('b'), "{err}");
    assert!(load(&store, &key, "a").await.is_ok());
});

db_test!(secrets_sealed_before_the_binding_are_resealed_at_start, |store, _url| {
    use bootstrap_support::*;
    use cubeserver::rbac::bootstrap::data_sources_from_rbac;
    use cubeserver::rbac::data_sources::load;
    // As the first release sealed them: no associated data.
    let secret = serde_json::to_vec(&std::collections::BTreeMap::from([(
        "CUBEJS_DB_PASS".to_string(),
        "old-password".to_string(),
    )]))
    .unwrap();
    let settings = std::collections::BTreeMap::from([("CUBEJS_DB_HOST".to_string(), "h".to_string())]);
    store
        .put_data_source("old", "postgres", &settings, Some(&key().seal(&secret, b"")))
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("cube.yml"), "version: 1\nmodel_path: model\n").unwrap();
    let mut config = cubeconfig::CubeConfig::load_declared(dir.path(), &cubeconfig::Env::new()).unwrap();

    let report = data_sources_from_rbac(&store, &key(), &mut config, None, &cubeconfig::Env::new())
        .await
        .unwrap();
    assert_eq!(report.loaded, 1);
    let stored = load(&store, &key(), "old").await.unwrap().unwrap();
    assert_eq!(stored.variables.get("CUBEJS_DB_PASS").map(String::as_str), Some("old-password"));
    let row = store.data_source("old").await.unwrap().unwrap();
    assert!(key().open(row.secrets.as_deref().unwrap(), b"").is_err(), "still sealed the old way");
});

db_test!(damaged_settings_are_an_error_not_an_empty_data_source, |store, url| {
    use cubeserver::rbac::data_sources::{load, load_each, save};
    let key = bootstrap_support::key();
    save(&store, &key, "bad", &pg_vars("p")).await.unwrap();
    save(&store, &key, "good", &pg_vars("p")).await.unwrap();
    let c = connect(&url).await;
    c.execute("UPDATE data_sources SET settings = '[1, 2]' WHERE name = 'bad'", &[])
        .await
        .unwrap();
    let err = load(&store, &key, "bad").await.unwrap_err();
    assert!(err.contains("bad"), "{err}");
    // One at a time, the good one still loads.
    let each = load_each(&store, &key).await.unwrap();
    assert_eq!(each.len(), 2);
    assert!(each.iter().any(|r| matches!(r, Ok(ds) if ds.name == "good")));
    assert!(each.iter().any(|r| matches!(r, Err(e) if e.name == "bad" && e.db_type == "postgres")));
});

mod bootstrap_support {
    use base64::Engine;
    use cubeserver::rbac::secrets::SecretKey;

    pub fn key() -> SecretKey {
        SecretKey::from_base64(&base64::engine::general_purpose::STANDARD.encode([9u8; 32])).unwrap()
    }

    pub const CUBE_YML: &str = "version: 1\nmodel_path: model\n\ndata_sources:\n  default:\n    type: postgres\n    host: 127.0.0.1\n    database: analytics\n    password: pg-secret\n\n  # Added from the Playground.\n  extra:\n    type: sqlite\n\nscheduled_refresh:\n  enabled: false\n";
    pub const DOT_ENV: &str = "PORT=4000\n# extra\nCUBEJS_DS_EXTRA_DB_NAME=/tmp/x.db\n";

    /// A deployment directory with `cube.yml` and `.env`.
    pub fn deployment() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("cube.yml"), CUBE_YML).unwrap();
        std::fs::write(dir.path().join(".env"), DOT_ENV).unwrap();
        dir
    }

    pub fn env_of(dir: &std::path::Path) -> cubeconfig::Env {
        let mut env = cubeconfig::Env::new();
        env.set("CUBEJS_DS_EXTRA_DB_NAME", "/tmp/x.db");
        // A global knob, not a setting of `default`.
        env.set("CUBEJS_DB_QUERY_TIMEOUT", "30");
        let _ = dir;
        env
    }
}

db_test!(import_moves_file_data_sources_into_rbac, |store, _url| {
    use bootstrap_support::*;
    use cubeserver::rbac::bootstrap::data_sources_from_rbac;
    let dir = deployment();
    let env = env_of(dir.path());
    let file = dir.path().join("cube.yml");
    let mut config = cubeconfig::CubeConfig::load_declared(dir.path(), &env).unwrap();

    let report = data_sources_from_rbac(&store, &key(), &mut config, Some(&file), &env)
        .await
        .unwrap();
    assert_eq!(report.imported, ["default", "extra"]);
    assert_eq!(report.loaded, 2);

    let names: Vec<String> = store.data_sources().await.unwrap().into_iter().map(|r| r.name).collect();
    assert_eq!(names, ["default", "extra"]);
    let yml = std::fs::read_to_string(&file).unwrap();
    assert!(!yml.contains("extra:") && !yml.contains("pg-secret"), "{yml}");
    assert!(yml.contains("scheduled_refresh"), "{yml}");
    let dot_env = std::fs::read_to_string(dir.path().join(".env")).unwrap();
    assert!(!dot_env.contains("CUBEJS_DS_EXTRA_"), "{dot_env}");
    assert!(dot_env.contains("PORT=4000"));
    let backups: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".bak-"))
        .collect();
    assert!(backups.iter().any(|n| n.starts_with("cube.yml.bak-")), "{backups:?}");
    assert!(backups.iter().any(|n| n.starts_with(".env.bak-")), "{backups:?}");

    assert_eq!(config.data_sources["extra"].database.as_deref(), Some("/tmp/x.db"));
    assert_eq!(config.data_sources["default"].password.as_deref(), Some("pg-secret"));
    assert_eq!(report.env.get("CUBEJS_DS_EXTRA_DB_NAME").map(String::as_str), Some("/tmp/x.db"));
    assert_eq!(report.env.get("CUBEJS_DB_PASS").map(String::as_str), Some("pg-secret"));
});

db_test!(import_is_idempotent, |store, _url| {
    use bootstrap_support::*;
    use cubeserver::rbac::bootstrap::data_sources_from_rbac;
    let dir = deployment();
    let env = env_of(dir.path());
    let file = dir.path().join("cube.yml");
    let mut config = cubeconfig::CubeConfig::load_declared(dir.path(), &env).unwrap();
    data_sources_from_rbac(&store, &key(), &mut config, Some(&file), &env).await.unwrap();
    let yml = std::fs::read_to_string(&file).unwrap();

    // The stripped cube.yml loads, with no data source of its own.
    let mut again = cubeconfig::CubeConfig::load_declared(dir.path(), &cubeconfig::Env::new()).unwrap();
    assert!(again.data_sources.is_empty());
    let report = data_sources_from_rbac(&store, &key(), &mut again, Some(&file), &cubeconfig::Env::new())
        .await
        .unwrap();
    assert!(report.imported.is_empty());
    assert_eq!(report.loaded, 2);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), yml);
    assert_eq!(again.data_sources["extra"].database.as_deref(), Some("/tmp/x.db"));
});

db_test!(a_partial_strip_is_finished_on_the_next_start, |store, _url| {
    use bootstrap_support::*;
    use cubeserver::rbac::bootstrap::data_sources_from_rbac;
    use cubeserver::rbac::data_sources::save;
    let dir = deployment();
    let env = env_of(dir.path());
    let file = dir.path().join("cube.yml");
    // RBAC already holds both (an earlier start imported them), but the
    // files were never stripped.
    for (name, vars) in [
        ("default", vec![("CUBEJS_DB_TYPE", "postgres"), ("CUBEJS_DB_HOST", "127.0.0.1")]),
        ("extra", vec![("CUBEJS_DB_TYPE", "sqlite"), ("CUBEJS_DB_NAME", "/tmp/x.db")]),
    ] {
        let vars = vars.into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        save(&store, &key(), name, &vars).await.unwrap();
    }
    let mut config = cubeconfig::CubeConfig::load_declared(dir.path(), &env).unwrap();
    let report = data_sources_from_rbac(&store, &key(), &mut config, Some(&file), &env)
        .await
        .unwrap();
    assert!(report.imported.is_empty());
    let yml = std::fs::read_to_string(&file).unwrap();
    assert!(!yml.contains("extra:") && !yml.contains("pg-secret"), "{yml}");
    let dot_env = std::fs::read_to_string(dir.path().join(".env")).unwrap();
    assert!(!dot_env.contains("CUBEJS_DS_EXTRA_"), "{dot_env}");
});

db_test!(import_keeps_global_knobs_out_of_default, |store, _url| {
    use bootstrap_support::*;
    use cubeserver::rbac::bootstrap::data_sources_from_rbac;
    let dir = deployment();
    let env = env_of(dir.path());
    let file = dir.path().join("cube.yml");
    let mut config = cubeconfig::CubeConfig::load_declared(dir.path(), &env).unwrap();
    data_sources_from_rbac(&store, &key(), &mut config, Some(&file), &env).await.unwrap();
    let row = store.data_source("default").await.unwrap().unwrap();
    assert!(!row.settings.contains_key("CUBEJS_DB_QUERY_TIMEOUT"), "{:?}", row.settings);
    assert_eq!(row.settings.get("CUBEJS_DB_HOST").map(String::as_str), Some("127.0.0.1"));
});

db_test!(a_taken_slug_conflicts_on_the_slug_index, |store, _url| {
    store.create_user(&user("ann")).await.unwrap();
    store.create_organization("A", "same", "ann").await.unwrap();
    match store.create_organization("B", "same", "ann").await {
        Err(StoreError::Conflict(c)) => assert!(c.contains("organizations_slug"), "{c}"),
        other => panic!("expected a slug conflict, got {other:?}"),
    }
});
