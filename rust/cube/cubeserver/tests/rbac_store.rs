//! Postgres store for users, organizations, workspaces and objects.
//! Runs only when `CUBE_TEST_PG_URL` points at a Postgres server; each test
//! works in its own throw-away database.

use cubeserver::rbac::store::{NewObject, NewUser, Store, StoreError};
use tokio_postgres::NoTls;
use uuid::Uuid;

struct TestDb {
    admin_url: String,
    name: String,
    pub store: Store,
}

impl TestDb {
    async fn create() -> Option<Self> {
        let Ok(admin_url) = std::env::var("CUBE_TEST_PG_URL") else {
            eprintln!("CUBE_TEST_PG_URL not set; skipping");
            return None;
        };
        let name = format!("cube_app_test_{}", Uuid::new_v4().simple());
        let admin = connect(&admin_url).await;
        admin
            .batch_execute(&format!("CREATE DATABASE {name}"))
            .await
            .unwrap();
        let store = Store::connect_with_pool_size(&with_db(&admin_url, &name), 1)
            .await
            .unwrap();
        Some(Self {
            admin_url,
            name,
            store,
        })
    }

    async fn drop_db(self) {
        let Self {
            admin_url,
            name,
            store,
        } = self;
        drop(store);
        let admin = connect(&admin_url).await;
        admin
            .batch_execute(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"))
            .await
            .unwrap();
    }
}

fn with_db(url: &str, db: &str) -> String {
    let (main, query) = match url.split_once('?') {
        Some((m, q)) => (m, Some(q)),
        None => (url, None),
    };
    let (base, _) = main.rsplit_once('/').unwrap();
    match query {
        Some(q) => format!("{base}/{db}?{q}"),
        None => format!("{base}/{db}"),
    }
}

async fn connect(url: &str) -> tokio_postgres::Client {
    let (client, conn) = tokio_postgres::connect(url, NoTls).await.unwrap();
    tokio::spawn(conn);
    client
}

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

        store.create_connection("c1", o1.id).await.unwrap();
        assert!(matches!(
            store.create_connection("c1", o2.id).await,
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
