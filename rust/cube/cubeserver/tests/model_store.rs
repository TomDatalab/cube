//! Model files and model version in the RBAC store. Runs only when
//! `CUBE_TEST_PG_URL` points at a Postgres server.

mod support;

use cubeserver::rbac::model_store::{ModelChange, ModelRows, WriteOutcome};
use cubeserver::rbac::store::{Store, StoreError};
use support::TestDb;
use uuid::Uuid;

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

fn put(org: Option<Uuid>, path: &str, content: &str) -> ModelChange {
    ModelChange::Put {
        id: None,
        organization_id: org,
        path: path.into(),
        content: content.into(),
        expected_version: None,
        managed_by: None,
    }
}

async fn write(store: &Store, changes: Vec<ModelChange>) -> Result<WriteOutcome, StoreError> {
    store
        .model_write("ann", move |_: &ModelRows| Ok::<_, ()>(changes))
        .await
        .map(|r| r.unwrap())
}

db_test!(empty_model_has_version_zero, |store| {
    let rows = store.model_rows().await.unwrap();
    assert_eq!(rows.version, 0);
    assert!(rows.files.is_empty());
    assert!(store.model_is_empty().await.unwrap());
});

db_test!(write_creates_and_bumps_version, |store| {
    let org = store.create_organization("o", "o", "ann").await.unwrap();
    let out = write(&store, vec![put(Some(org.id), "fatos.yml", "a")])
        .await
        .unwrap();
    assert_eq!(
        out,
        WriteOutcome {
            version: 1,
            changed: true
        }
    );
    let rows = store.model_rows().await.unwrap();
    assert_eq!(rows.version, 1);
    assert_eq!(rows.files[0].path, "fatos.yml");
    assert_eq!(rows.files[0].version, 1);
    assert_eq!(rows.files[0].created_by, "ann");
    assert!(!store.model_is_empty().await.unwrap());
    let f = store.model_file(rows.files[0].id).await.unwrap().unwrap();
    assert_eq!(f.content, "a");
    assert!(store.model_file(Uuid::new_v4()).await.unwrap().is_none());
});

db_test!(stale_version_is_a_conflict, |store| {
    let org = store.create_organization("o", "o", "ann").await.unwrap();
    write(&store, vec![put(Some(org.id), "fatos.yml", "a")])
        .await
        .unwrap();
    let f = store.model_rows().await.unwrap().files[0].id;
    let edit = |content: &str| ModelChange::Put {
        id: Some(f),
        organization_id: Some(org.id),
        path: "fatos.yml".into(),
        content: content.into(),
        expected_version: Some(1),
        managed_by: None,
    };
    write(&store, vec![edit("first")]).await.unwrap();
    match write(&store, vec![edit("second")]).await {
        Err(StoreError::Conflict(m)) => assert_eq!(m, "stale"),
        other => panic!("expected stale conflict, got {other:?}"),
    }
    let file = store.model_file(f).await.unwrap().unwrap();
    assert_eq!(file.content, "first");
    assert_eq!(file.version, 2);
    assert_eq!(store.model_version().await.unwrap(), 2);
});

db_test!(plan_error_rolls_back, |store| {
    let r = store
        .model_write("ann", |_: &ModelRows| Err::<Vec<ModelChange>, _>("no"))
        .await
        .unwrap();
    assert_eq!(r.unwrap_err(), "no");
    assert_eq!(store.model_version().await.unwrap(), 0);
});

db_test!(empty_plan_does_not_bump, |store| {
    let org = store.create_organization("o", "o", "ann").await.unwrap();
    write(&store, vec![put(Some(org.id), "a.yml", "a")])
        .await
        .unwrap();
    let out = write(&store, vec![]).await.unwrap();
    assert_eq!(
        out,
        WriteOutcome {
            version: 1,
            changed: false
        }
    );
    assert_eq!(store.model_version().await.unwrap(), 1);
});

db_test!(same_path_in_two_organizations, |store| {
    let a = store.create_organization("a", "a", "ann").await.unwrap();
    let b = store.create_organization("b", "b", "ann").await.unwrap();
    write(&store, vec![put(Some(a.id), "fatos.yml", "a")])
        .await
        .unwrap();
    write(&store, vec![put(Some(b.id), "fatos.yml", "b")])
        .await
        .unwrap();
    assert_eq!(store.model_rows().await.unwrap().files.len(), 2);
    match write(&store, vec![put(Some(a.id), "fatos.yml", "x")]).await {
        Err(StoreError::Conflict(m)) => assert_eq!(m, "path"),
        other => panic!("expected path conflict, got {other:?}"),
    }
    write(&store, vec![put(None, "fatos.yml", "p")])
        .await
        .unwrap();
    match write(&store, vec![put(None, "fatos.yml", "q")]).await {
        Err(StoreError::Conflict(m)) => assert_eq!(m, "path"),
        other => panic!("expected path conflict, got {other:?}"),
    }
    assert_eq!(store.model_rows().await.unwrap().files.len(), 3);
});

db_test!(move_changes_owner, |store| {
    let org = store.create_organization("o", "o", "ann").await.unwrap();
    write(&store, vec![put(Some(org.id), "fatos.yml", "a")])
        .await
        .unwrap();
    let f = store.model_rows().await.unwrap().files[0].id;
    write(
        &store,
        vec![ModelChange::Move {
            id: f,
            organization_id: None,
        }],
    )
    .await
    .unwrap();
    let file = store.model_file(f).await.unwrap().unwrap();
    assert_eq!(file.organization_id, None);
    assert_eq!(file.version, 2);
});

db_test!(organization_with_files_cannot_be_deleted, |store| {
    let org = store.create_organization("o", "o", "ann").await.unwrap();
    write(&store, vec![put(Some(org.id), "fatos.yml", "a")])
        .await
        .unwrap();
    assert!(matches!(
        store.delete_organization(org.id).await,
        Err(StoreError::Conflict(_))
    ));
});
