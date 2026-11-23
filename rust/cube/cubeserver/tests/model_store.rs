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

mod import {
    use super::*;
    use cubeserver::rbac::model_import::{import_if_empty, import_if_empty_with, verify_import};
    use std::fs;
    use std::path::Path;

    fn cube(name: &str) -> String {
        format!("cubes:\n  - name: {name}\n    sql_table: t\n    dimensions:\n      - name: id\n        sql: id\n        type: number\n        primary_key: true\n")
    }

    fn put_file(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let m = dir.path().join("model");
        put_file(&m, "digital/fatos.yml", &cube("fatos"));
        put_file(&m, "_platform/orders.yml", &cube("orders"));
        put_file(&m, "solto.yml", &cube("solto"));
        let synced = format!("# Generated by sync-sofiadb-model.py\n{}", cube("dfos_a"));
        put_file(&m, "digital/sofiadb/dfos_a.yml", &synced);
        dir
    }

    db_test!(import_moves_folders_into_rows, |store| {
        let org = store.create_organization("Digital", "digital", "ann").await.unwrap();
        let dir = tree();
        let model = dir.path().join("model");
        let report = import_if_empty(&store, &model, "20261122").await.unwrap();
        assert_eq!(report.imported, 4);
        assert_eq!(report.unowned, vec!["solto.yml".to_string()]);
        assert_eq!(report.renamed_to, Some(dir.path().join("model.imported-20261122")));
        let rows = store.model_rows().await.unwrap();
        assert_eq!(rows.version, 1);
        let got: Vec<(Option<Uuid>, String, Option<String>)> = {
            let mut v: Vec<_> = rows
                .files
                .iter()
                .map(|f| (f.organization_id, f.path.clone(), f.managed_by.clone()))
                .collect();
            v.sort_by(|a, b| a.1.cmp(&b.1));
            v
        };
        assert_eq!(
            got,
            vec![
                (Some(org.id), "fatos.yml".to_string(), None),
                (None, "orders.yml".to_string(), None),
                (Some(org.id), "sofiadb/dfos_a.yml".to_string(), Some("sofiadb-sync".to_string())),
                (None, "solto.yml".to_string(), None),
            ]
        );
        assert!(dir.path().join("model.imported-20261122").is_dir());
        assert!(model.join("README").is_file());
        assert!(!model.join("digital").exists());
    });

    db_test!(import_is_not_repeated_when_the_folder_survives, |store| {
        store.create_organization("Digital", "digital", "ann").await.unwrap();
        let dir = tree();
        let model = dir.path().join("model");
        import_if_empty(&store, &model, "20261122").await.unwrap();
        let before = store.model_rows().await.unwrap();
        put_file(&model, "digital/x.yml", &cube("x"));
        let report = import_if_empty(&store, &model, "20261123").await.unwrap();
        assert_eq!(report.imported, 0);
        assert_eq!(report.renamed_to, None);
        let after = store.model_rows().await.unwrap();
        assert_eq!(after.version, before.version);
        assert_eq!(after.files.len(), before.files.len());
        assert!(after.files.iter().all(|f| f.path != "x.yml"));
    });

    db_test!(import_rejects_an_invalid_path_and_changes_nothing, |store| {
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("model");
        put_file(&model, "_platform/bad name.yml", &cube("a"));
        let err = import_if_empty(&store, &model, "20261122").await.unwrap_err();
        assert!(err.contains("bad name.yml"), "{err}");
        assert!(store.model_is_empty().await.unwrap());
        assert!(model.join("_platform").is_dir());
    });

    db_test!(marker_in_the_middle_is_not_the_syncs, |store| {
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("model");
        let body = format!("# my notes\n# Generated by sync-sofiadb-model.py\n{}", cube("a"));
        put_file(&model, "_platform/a.yml", &body);
        import_if_empty(&store, &model, "20261122").await.unwrap();
        let rows = store.model_rows().await.unwrap();
        assert_eq!(rows.files.len(), 1);
        assert_eq!(rows.files[0].managed_by, None);
    });

    db_test!(mismatch_writes_nothing, |store| {
        store.create_organization("Digital", "digital", "ann").await.unwrap();
        let dir = tree();
        let model = dir.path().join("model");
        let err = import_if_empty_with(&store, &model, "20261122", |_, _| {
            Err("the imported model lacks cubes: b".to_string())
        })
        .await
        .unwrap_err();
        assert!(err.contains("b"), "{err}");
        assert!(store.model_is_empty().await.unwrap());
        assert_eq!(store.model_version().await.unwrap(), 0);
        assert!(model.join("digital/fatos.yml").is_file());
        assert!(model.join("_platform/orders.yml").is_file());
        assert!(!dir.path().join("model.imported-20261122").exists());
        // The next start retries and succeeds.
        let report = import_if_empty(&store, &model, "20261122").await.unwrap();
        assert_eq!(report.imported, 4);
    });

    #[test]
    fn failed_compare_keeps_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("model");
        put_file(&model, "_platform/a.yml", &cube("a"));
        put_file(&model, "_platform/b.yml", &cube("b"));
        let folder = vec![
            ("_platform/a.yml".to_string(), cube("a")),
            ("_platform/b.yml".to_string(), cube("b")),
        ];
        let db = vec![("_platform/a.yml".to_string(), cube("a"))];
        let err = verify_import(&folder, &db).unwrap_err();
        assert_eq!(err, "the imported model lacks cubes: b");
        // verify_import never touches the disk.
        assert!(model.is_dir());
        assert!(!dir.path().join("model.imported-20261122").exists());
    }
}

// ---- ModelRuntime ----

use cubeserver::rbac::model_runtime::ModelRuntime;

fn cache_dir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    d
}

db_test!(runtime_serves_rows_from_live, |store| {
    let org = store.create_organization("Digital", "digital", "ann").await.unwrap();
    write(&store, vec![put(Some(org.id), "fatos.yml", "a")]).await.unwrap();
    let cache = cache_dir();
    let rt = ModelRuntime::start(store.clone(), cache.path().to_path_buf())
        .await
        .unwrap();
    assert_eq!(rt.version(), 1);
    assert_eq!(
        std::fs::read_to_string(rt.live_path().join("digital/fatos.yml")).unwrap(),
        "a"
    );
});

db_test!(refresh_sees_writes_of_other_processes, |store| {
    let org = store.create_organization("Digital", "digital", "ann").await.unwrap();
    let cache = cache_dir();
    let rt = ModelRuntime::start(store.clone(), cache.path().to_path_buf())
        .await
        .unwrap();
    assert!(!rt.refresh().await.unwrap());
    write(&store, vec![put(Some(org.id), "novo.yml", "n")]).await.unwrap();
    assert!(rt.refresh().await.unwrap());
    assert_eq!(
        std::fs::read_to_string(rt.live_path().join("digital/novo.yml")).unwrap(),
        "n"
    );
    assert!(!rt.refresh().await.unwrap());
    // A new organization's files get its slug.
    let o2 = store.create_organization("Outra", "outra", "ann").await.unwrap();
    write(&store, vec![put(Some(o2.id), "x.yml", "x")]).await.unwrap();
    assert!(rt.refresh().await.unwrap());
    assert!(rt.live_path().join("outra/x.yml").exists());
});

db_test!(disk_folder_is_not_read, |store| {
    let model = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(model.path().join("digital")).unwrap();
    std::fs::write(model.path().join("digital/novo.yml"), "x").unwrap();
    let cache = cache_dir();
    let rt = ModelRuntime::start(store.clone(), cache.path().to_path_buf())
        .await
        .unwrap();
    assert!(!rt.live_path().join("digital/novo.yml").exists());
});

/// Every file of `rows` under `live`, with its content.
fn live_is_complete(rt: &ModelRuntime, rounds: usize, count: usize) {
    let target = std::fs::read_link(rt.live_path()).unwrap();
    assert_eq!(target, std::path::PathBuf::from(format!("v{}", rt.version())));
    for (round, i) in (0..rounds).flat_map(|r| (0..count).map(move |i| (r, i))) {
        let p = rt.live_path().join(format!("digital/r{round}/f{i:04}.yml"));
        assert_eq!(std::fs::read_to_string(&p).unwrap_or_default(), format!("c{i}"), "{}", p.display());
    }
}

db_test!(concurrent_refreshes_leave_a_complete_live, |store| {
    let org = store.create_organization("Digital", "digital", "ann").await.unwrap();
    let cache = cache_dir();
    let rt = ModelRuntime::start(store.clone(), cache.path().to_path_buf())
        .await
        .unwrap();
    let count = 400;
    for round in 0..3 {
        let changes = (0..count)
            .map(|i| put(Some(org.id), &format!("r{round}/f{i:04}.yml"), &format!("c{i}")))
            .collect();
        write(&store, changes).await.unwrap();
        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let rt = rt.clone();
                tokio::spawn(async move { rt.refresh().await })
            })
            .collect();
        for t in tasks {
            t.await.unwrap().unwrap();
        }
        assert_eq!(rt.version(), store.model_version().await.unwrap());
        live_is_complete(&rt, round + 1, count);
    }
    // No temporary folder is left behind.
    let left: Vec<_> = std::fs::read_dir(cache.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".tmp"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
});

db_test!(refresh_never_moves_live_back, |store| {
    let org = store.create_organization("Digital", "digital", "ann").await.unwrap();
    let cache = cache_dir();
    let rt = ModelRuntime::start(store.clone(), cache.path().to_path_buf())
        .await
        .unwrap();
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // Stops the watcher even when the test panics first.
    struct Stop(std::sync::Arc<std::sync::atomic::AtomicBool>);
    impl Drop for Stop {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
    let stop = Stop(done.clone());
    // Watches `live` and the version while writes and refreshes race.
    let watcher = {
        let (rt, done, live) = (rt.clone(), done.clone(), rt.live_path().to_path_buf());
        tokio::task::spawn_blocking(move || {
            let (mut last_link, mut last_seen) = (0i64, 0i64);
            while !done.load(std::sync::atomic::Ordering::SeqCst) {
                if let Ok(t) = std::fs::read_link(&live) {
                    let v: i64 = t.to_string_lossy().trim_start_matches('v').parse().unwrap();
                    assert!(v >= last_link, "live went from v{last_link} back to v{v}");
                    last_link = v;
                }
                let seen = rt.version();
                assert!(seen >= last_seen, "version went from {last_seen} back to {seen}");
                last_seen = seen;
            }
        })
    };
    let mut refreshes = Vec::new();
    for round in 0..20 {
        let changes = (0..50)
            .map(|i| put(Some(org.id), &format!("r{round}/f{i:04}.yml"), "x"))
            .collect();
        write(&store, changes).await.unwrap();
        for _ in 0..3 {
            let rt = rt.clone();
            refreshes.push(tokio::spawn(async move { rt.refresh().await }));
        }
    }
    for r in refreshes {
        r.await.unwrap().unwrap();
    }
    drop(stop);
    watcher.await.unwrap();
    let last = store.model_version().await.unwrap();
    assert_eq!(rt.version(), last);
    assert_eq!(std::fs::read_link(rt.live_path()).unwrap(), std::path::PathBuf::from(format!("v{last}")));
});
