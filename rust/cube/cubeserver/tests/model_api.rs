//! Validated, transactional model writes (`ModelWriter`) with RBAC on.
//!
//! The ownership policy is process-global, so these tests live in a binary of
//! their own, run one at a time, and clear the policy when they end. They need
//! `CUBE_TEST_PG_URL` (see `support`).

mod support;
#[path = "support/tree_env.rs"]
mod tree_env;

use axum::http::{Method, StatusCode};
use cubeserver::rbac::model_store::ModelChange;
use cubeserver::rbac::model_writer::WriteError;
use support::TestDb;
use tree_env::*;

fn cube(name: &str) -> String {
    format!(
        "cubes:\n  - name: {name}\n    sql_table: t\n    data_source: default\n    measures:\n      - name: n\n        type: count\n"
    )
}

fn put(path: &str, content: &str) -> ModelChange {
    ModelChange::Put {
        id: None,
        organization_id: None,
        path: path.into(),
        content: content.into(),
        expected_version: None,
        managed_by: None,
    }
}

macro_rules! api_test {
    ($name:ident, |$store:ident| $body:block) => {
        #[tokio::test]
        async fn $name() {
            let _serial = SERIAL.lock().await;
            let _clear = ClearPolicy;
            let Some(db) = TestDb::create().await else {
                return;
            };
            install_policy(&[], &["default"]);
            let $store = db.store.clone();
            let result = tokio::spawn(async move $body).await;
            db.drop_db().await;
            result.unwrap();
        }
    };
}

async fn cube_names(env: &Env, token: &str) -> Vec<String> {
    let (status, body, _) = get(env, token, "/cube/v1/meta").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["cubes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect()
}

api_test!(write_applies_and_reloads, |store| {
    let env = env_live(store.clone()).await;
    let writer = env.writer.clone().unwrap();
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    // A model with no file cannot be planned, so start from one.
    writer
        .write("ann", "seed", |_| Ok(vec![put("base.yml", &cube("base"))]))
        .await
        .unwrap();
    assert_eq!(cube_names(&env, &admin).await, vec!["base".to_string()]);

    let written = writer
        .write("ann", "test", |_| Ok(vec![put("novo.yml", &cube("novo"))]))
        .await
        .unwrap();
    assert!(written.changed);
    assert_eq!(written.version, store.model_version().await.unwrap());
    // No waiting for the reload loop.
    let mut names = cube_names(&env, &admin).await;
    names.sort();
    assert_eq!(names, ["base", "novo"]);
});

api_test!(invalid_write_changes_nothing, |store| {
    let env = env_live(store.clone()).await;
    let writer = env.writer.clone().unwrap();
    let runtime = env.runtime.clone().unwrap();
    let before = store.model_version().await.unwrap();
    let live_before = std::fs::read_link(runtime.live_path()).unwrap();

    let err: WriteError = writer
        .write("ann", "test", |_| Ok(vec![put("ruim.yml", "cubes: [ {")]))
        .await
        .unwrap_err();
    assert_eq!(err.status, StatusCode::BAD_REQUEST);
    assert!(err.problems[0].file.ends_with(".yml"), "{err:?}");

    assert_eq!(store.model_version().await.unwrap(), before);
    assert!(store.model_is_empty().await.unwrap());
    assert_eq!(std::fs::read_link(runtime.live_path()).unwrap(), live_before);
    assert_eq!(runtime.version(), before);
    // The HTTP shape carries the problems.
    let api: cubeserver::ApiError = err.into();
    assert_eq!(api.status, StatusCode::BAD_REQUEST);
    assert_eq!(api.body.problems.as_ref().unwrap()[0]["file"], "_platform/ruim.yml");
    let _ = Method::GET;
});

api_test!(concurrent_writes_both_validate, |store| {
    let env = env_live(store.clone()).await;
    let writer = env.writer.clone().unwrap();
    let before = store.model_version().await.unwrap();

    let (a, b) = tokio::join!(
        writer.write("ann", "a", |_| Ok(vec![put("a.yml", &cube("a"))])),
        writer.write("bob", "b", |_| Ok(vec![put("b.yml", &cube("b"))])),
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(store.model_version().await.unwrap(), before + 2);
    let rows = store.model_rows().await.unwrap();
    let mut paths: Vec<_> = rows.files.iter().map(|f| f.path.as_str()).collect();
    paths.sort();
    assert_eq!(paths, ["a.yml", "b.yml"]);
});
