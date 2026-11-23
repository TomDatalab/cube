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
    assert_eq!(err.message, "A mudança não compila");
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

api_test!(write_to_unknown_organization_is_refused, |store| {
    let env = env_live(store.clone()).await;
    let writer = env.writer.clone().unwrap();
    let before = store.model_version().await.unwrap();
    let err = writer
        .write("ann", "test", |_| {
            Ok(vec![ModelChange::Put {
                id: None,
                organization_id: Some(uuid::Uuid::new_v4()),
                path: "x.yml".into(),
                content: cube("x"),
                expected_version: None,
                managed_by: None,
            }])
        })
        .await
        .unwrap_err();
    assert_eq!(err.status, StatusCode::BAD_REQUEST);
    assert_eq!(err.message, "Organização desconhecida");
    assert_eq!(store.model_version().await.unwrap(), before);
});

// ---- routes of Task 7 ----

use serde_json::{json, Value};

const SYNC_MESSAGE: &str = "Este arquivo é gerado a partir do catálogo do SofiaDB e é reescrito a cada sincronização. Para personalizar uma tabela, escreva o cubo dela em outro arquivo da organização, com outro nome; na próxima sincronização o cubo gerado sai";
const STALE: &str = "O arquivo mudou desde que você abriu; abra de novo";

fn cube_on(name: &str, source: &str) -> String {
    cube(name).replace("data_source: default", &format!("data_source: {source}"))
}

/// An organization of the admin's: its id and slug.
async fn org(env: &Env, store: &cubeserver::rbac::store::Store, admin: &str, name: &str) -> (uuid::Uuid, String) {
    let slug = create_org(env, admin, name).await;
    let org = store.organization_by_slug(&slug).await.unwrap().unwrap();
    (org.id, slug)
}

fn put_in(org: Option<uuid::Uuid>, path: &str, content: &str, managed_by: Option<&str>) -> ModelChange {
    ModelChange::Put {
        id: None,
        organization_id: org,
        path: path.into(),
        content: content.into(),
        expected_version: None,
        managed_by: managed_by.map(str::to_string),
    }
}

/// `(id, version)` of the file `file_name` in a detail.
fn file_in(detail: &Value, file_name: &str) -> (String, i64) {
    let f = detail["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["fileName"] == file_name)
        .unwrap_or_else(|| panic!("{file_name} not in {detail}"));
    (f["id"].as_str().unwrap().to_string(), f["version"].as_i64().unwrap())
}

async fn call(env: &Env, method: Method, token: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let (status, body, text) = send(&env.router, method, path, Some(token), body).await;
    assert!(!matches!(status, StatusCode::SERVICE_UNAVAILABLE | StatusCode::INTERNAL_SERVER_ERROR), "{text}");
    (status, body)
}

api_test!(admin_edits_a_file, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (org_id, _) = org(&env, &store, &admin, "Digital").await;
    install_policy(&[("digital", &["default"])], &["default"]);
    env.writer.clone().unwrap()
        .write("ann", "seed", |_| Ok(vec![put_in(Some(org_id), "fatos.yml", &cube("fatos"), None)]))
        .await
        .unwrap();
    let base = format!("/cube/v1/organizations/{org_id}/model");
    let (status, detail) = call(&env, Method::GET, &admin, &base, None).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    let (id, version) = file_in(&detail, "digital/fatos.yml");
    let f = detail["files"].as_array().unwrap().iter().find(|f| f["id"] == id).unwrap();
    assert_eq!((f["path"].as_str(), f["editable"].as_bool()), (Some("fatos.yml"), Some(true)));

    let new_content = cube("fatos2");
    let (status, detail) = call(&env, Method::PUT, &admin, &format!("{base}/files/{id}"),
        Some(json!({ "content": new_content, "version": version }))).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(file_in(&detail, "digital/fatos.yml").1, version + 1);

    let (status, file) = call(&env, Method::GET, &admin, &format!("{base}/files/{id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{file}");
    assert_eq!(file["content"], new_content);
    assert_eq!(file["version"], version + 1);
    assert_eq!(file["editable"], true);
    assert_eq!(file["updatedBy"], ADMIN);
});

api_test!(stale_put_is_409, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (org_id, _) = org(&env, &store, &admin, "Digital").await;
    install_policy(&[("digital", &["default"])], &["default"]);
    env.writer.clone().unwrap()
        .write("ann", "seed", |_| Ok(vec![put_in(Some(org_id), "a.yml", &cube("a"), None)]))
        .await
        .unwrap();
    let base = format!("/cube/v1/organizations/{org_id}/model");
    let (_, detail) = call(&env, Method::GET, &admin, &base, None).await;
    let (id, version) = file_in(&detail, "digital/a.yml");
    let url = format!("{base}/files/{id}");
    let (first, _) = call(&env, Method::PUT, &admin, &url, Some(json!({ "content": cube("a"), "version": version }))).await;
    assert_eq!(first, StatusCode::OK);
    let (second, body) = call(&env, Method::PUT, &admin, &url, Some(json!({ "content": cube("a"), "version": version }))).await;
    assert_eq!(second, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], STALE);
    let (status, body) = call(&env, Method::DELETE, &admin, &format!("{url}?version={version}"), None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
});

api_test!(sync_file_is_read_only, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (org_id, _) = org(&env, &store, &admin, "Digital").await;
    install_policy(&[("digital", &["default"])], &["default"]);
    env.writer.clone().unwrap()
        .write("sync", "seed", |_| Ok(vec![
            put_in(Some(org_id), "gerado.yml", &cube("gerado"), Some("sofiadb-sync")),
            put_in(Some(org_id), "t.jinja", "cubes: []\n", None),
        ]))
        .await
        .unwrap();
    let base = format!("/cube/v1/organizations/{org_id}/model");
    let (_, detail) = call(&env, Method::GET, &admin, &base, None).await;
    let (id, version) = file_in(&detail, "digital/gerado.yml");
    let url = format!("{base}/files/{id}");
    let (status, body) = call(&env, Method::PUT, &admin, &url, Some(json!({ "content": cube("gerado"), "version": version }))).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::BAD_REQUEST, Some(SYNC_MESSAGE)));
    let (status, body) = call(&env, Method::DELETE, &admin, &format!("{url}?version={version}"), None).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::BAD_REQUEST, Some(SYNC_MESSAGE)));
    // Readable, and not editable.
    let (status, file) = call(&env, Method::GET, &admin, &url, None).await;
    assert_eq!((status, file["editable"].clone()), (StatusCode::OK, json!(false)));

    let (id, version) = file_in(&detail, "digital/t.jinja");
    let (status, body) = call(&env, Method::PUT, &admin, &format!("{base}/files/{id}"), Some(json!({ "content": "y", "version": version }))).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::BAD_REQUEST, Some("Arquivos .jinja e .j2 não são editáveis aqui")));
});

api_test!(delete_of_depended_file_is_400, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let a = "cubes:\n  - name: a\n    sql_table: t\n    data_source: default\n    joins:\n      - name: b\n        sql: \"{CUBE}.id = {b}.id\"\n        relationship: many_to_one\n    measures:\n      - name: n\n        type: count\n    dimensions:\n      - name: id\n        sql: id\n        type: number\n";
    let b = "cubes:\n  - name: b\n    sql_table: t\n    data_source: default\n    measures:\n      - name: n\n        type: count\n    dimensions:\n      - name: id\n        sql: id\n        type: number\n        primary_key: true\n";
    env.writer.clone().unwrap()
        .write("ann", "seed", |_| Ok(vec![put_in(None, "a.yml", a, None), put_in(None, "b.yml", b, None)]))
        .await
        .unwrap();
    let (status, detail) = call(&env, Method::GET, &admin, "/cube/v1/platform/model", None).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    let (id, version) = file_in(&detail, "_platform/b.yml");
    let (status, body) = call(&env, Method::DELETE, &admin, &format!("/cube/v1/platform/model/files/{id}?version={version}"), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(!body["problems"].as_array().unwrap().is_empty(), "{body}");
    // Nothing was deleted.
    assert_eq!(call(&env, Method::GET, &admin, &format!("/cube/v1/platform/model/files/{id}"), None).await.0, StatusCode::OK);
    // `a` lives in Platform, so an organization route does not see it.
    let (org_id, _) = org(&env, &store, &admin, "Digital").await;
    let (status, _) = call(&env, Method::GET, &admin, &format!("/cube/v1/organizations/{org_id}/model/files/{id}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
});

api_test!(file_of_other_org_is_404, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (a, _) = org(&env, &store, &admin, "Alfa").await;
    let (b, _) = org(&env, &store, &admin, "Beta").await;
    install_policy(&[("alfa", &["default"]), ("beta", &["default"])], &["default"]);
    env.writer.clone().unwrap()
        .write("ann", "seed", |_| Ok(vec![put_in(Some(b), "x.yml", &cube("x"), None)]))
        .await
        .unwrap();
    let (_, detail) = call(&env, Method::GET, &admin, &format!("/cube/v1/organizations/{b}/model"), None).await;
    let (id, version) = file_in(&detail, "beta/x.yml");
    let url = format!("/cube/v1/organizations/{a}/model/files/{id}");
    assert_eq!(call(&env, Method::GET, &admin, &url, None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&env, Method::PUT, &admin, &url, Some(json!({ "content": cube("x"), "version": version }))).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&env, Method::DELETE, &admin, &format!("{url}?version={version}"), None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&env, Method::GET, &admin, &format!("/cube/v1/organizations/{a}/model/files/not-a-uuid"), None).await.0, StatusCode::NOT_FOUND);
});

api_test!(move_revalidates, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (digital, _) = org(&env, &store, &admin, "Digital").await;
    let (on, _) = org(&env, &store, &admin, "ON").await;
    install_policy(&[("digital", &["sofiadb"]), ("on", &[])], &["default"]);
    env.writer.clone().unwrap()
        .write("ann", "seed", |_| Ok(vec![put_in(Some(digital), "a.yml", &cube_on("a", "sofiadb"), None)]))
        .await
        .unwrap();
    let (_, detail) = call(&env, Method::GET, &admin, &format!("/cube/v1/organizations/{digital}/model"), None).await;
    let (id, _) = file_in(&detail, "digital/a.yml");
    let url = format!("/cube/v1/model/files/{id}/organization");
    let (status, body) = call(&env, Method::PUT, &admin, &url, Some(json!({ "organizationId": on }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(!body["problems"].as_array().unwrap().is_empty(), "{body}");
    let (status, body) = call(&env, Method::PUT, &admin, &url, Some(json!({ "organizationId": null }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // Once the connection is `on`'s, the file can go to its owner.
    install_policy(&[("digital", &[]), ("on", &["sofiadb"])], &["default"]);
    let before = store.model_version().await.unwrap();
    let (status, body) = call(&env, Method::PUT, &admin, &url, Some(json!({ "organizationId": on }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["version"], before + 1);
    let (_, detail) = call(&env, Method::GET, &admin, &format!("/cube/v1/organizations/{on}/model"), None).await;
    assert_eq!(file_in(&detail, "on/a.yml").0, id);
});

api_test!(export_lists_every_file, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (digital, _) = org(&env, &store, &admin, "Digital").await;
    install_policy(&[("digital", &["other"])], &["default"]);
    env.writer.clone().unwrap()
        .write("ann", "seed", |_| Ok(vec![
            put_in(Some(digital), "fatos.yml", &cube_on("fatos", "other"), None),
            put_in(None, "orders.yml", &cube("orders"), None),
        ]))
        .await
        .unwrap();
    let (status, body) = call(&env, Method::GET, &admin, "/cube/v1/model/export", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["version"], store.model_version().await.unwrap());
    let files = body["files"].as_array().unwrap();
    let content = |p: &str| files.iter().find(|f| f["path"] == p).map(|f| f["content"].clone());
    assert_eq!(content("digital/fatos.yml"), Some(json!(cube_on("fatos", "other"))));
    assert_eq!(content("_platform/orders.yml"), Some(json!(cube("orders"))));
});

api_test!(non_admin_cannot_reach_platform, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (status, body) = call(&env, Method::POST, &admin, "/cube/v1/users", Some(json!({
        "id": "ann", "name": "Ann", "email": "a@example.org", "password": "ann-password-1" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let ann = login(&env, "ann", "ann-password-1").await;
    assert_eq!(call(&env, Method::GET, &ann, "/cube/v1/platform/model", None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&env, Method::GET, &ann, "/cube/v1/model/export", None).await.0, StatusCode::FORBIDDEN);
    let id = uuid::Uuid::new_v4();
    assert_eq!(call(&env, Method::PUT, &ann, &format!("/cube/v1/model/files/{id}/organization"), Some(json!({ "organizationId": null }))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&env, Method::GET, &ann, &format!("/cube/v1/platform/model/files/{id}"), None).await.0, StatusCode::FORBIDDEN);
});

api_test!(organization_with_files_is_400, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (digital, _) = org(&env, &store, &admin, "Digital").await;
    install_policy(&[("digital", &["default"])], &["default"]);
    env.writer.clone().unwrap()
        .write("ann", "seed", |_| Ok(vec![put_in(Some(digital), "a.yml", &cube("a"), None)]))
        .await
        .unwrap();
    let url = format!("/cube/v1/organizations/{digital}");
    let (status, body) = call(&env, Method::DELETE, &admin, &url, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "Mova ou apague os arquivos do modelo antes");

    let (_, detail) = call(&env, Method::GET, &admin, &format!("{url}/model"), None).await;
    let (id, version) = file_in(&detail, "digital/a.yml");
    let (status, _) = call(&env, Method::DELETE, &admin, &format!("{url}/model/files/{id}?version={version}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = call(&env, Method::DELETE, &admin, &url, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
});

api_test!(delete_without_version_is_400, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (org_id, _) = org(&env, &store, &admin, "Digital").await;
    install_policy(&[("digital", &["default"])], &["default"]);
    env.writer.clone().unwrap()
        .write("ann", "seed", |_| Ok(vec![put_in(Some(org_id), "a.yml", &cube("a"), None)]))
        .await
        .unwrap();
    let base = format!("/cube/v1/organizations/{org_id}/model");
    let (_, detail) = call(&env, Method::GET, &admin, &base, None).await;
    let (id, _) = file_in(&detail, "digital/a.yml");
    let before = store.model_version().await.unwrap();
    let (status, body) = call(&env, Method::DELETE, &admin, &format!("{base}/files/{id}"), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "Informe a versão do arquivo (?version=)");
    assert_eq!(call(&env, Method::GET, &admin, &format!("{base}/files/{id}"), None).await.0, StatusCode::OK);
    assert_eq!(store.model_version().await.unwrap(), before);
});

// ---- Generate (Task 8) ----

use cubeserver::rbac::store::ConnectionOrigin;

/// `cluster_test.<table>` with an id and a name column.
fn structure(tables: &[&str]) -> cubedriver::DatabaseStructure {
    let column = |name: &str, type_: &str, attributes: &[&str]| cubedriver::SchemaColumn {
        name: name.into(),
        type_: type_.into(),
        attributes: attributes.iter().map(|a| a.to_string()).collect(),
        foreign_keys: Vec::new(),
    };
    let mut out = cubedriver::DatabaseStructure::new();
    let schema = out.entry("cluster_test".to_string()).or_default();
    for table in tables {
        schema.insert(
            table.to_string(),
            vec![column("id", "integer", &["primaryKey"]), column("nome", "text", &[])],
        );
    }
    out
}

fn pairs(tables: &[&str]) -> Vec<(String, String)> {
    tables.iter().map(|t| ("cluster_test".to_string(), t.to_string())).collect()
}

api_test!(generate_writes_rows, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (org_id, slug) = org(&env, &store, &admin, "Digital").await;
    install_policy(&[("digital", &["default"])], &["default"]);
    env.writer.clone().unwrap()
        .write("ann", "seed", |_| Ok(vec![put_in(Some(org_id), "base.yml", &cube("base"), None)]))
        .await
        .unwrap();

    let files = cubeserver::rbac::handlers::model::generate_into_model(
        &env.state, "ann", Some(org_id), &slug, &pairs(&["fatos"]), &structure(&["fatos"]), false,
    )
    .await
    .unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["fileName"], "digital/fatos.yml");

    let rows = store.model_rows().await.unwrap();
    let row = rows.files.iter().find(|f| f.path == "fatos.yml").expect("a new row");
    assert_eq!(row.organization_id, Some(org_id));
    assert_eq!(row.content, files[0]["content"].as_str().unwrap());
    assert_eq!(rows.files.len(), 2);
    assert!(cube_names(&env, &admin).await.contains(&"fatos".to_string()));
});

api_test!(generate_from_a_default_schema_key, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (org_id, slug) = org(&env, &store, &admin, "Digital").await;
    install_policy(&[("digital", &["default"])], &["default"]);
    env.writer.clone().unwrap()
        .write("ann", "seed", |_| Ok(vec![put_in(Some(org_id), "base.yml", &cube("base"), None)]))
        .await
        .unwrap();
    // The page names a `default` schema `default.<schema>`.
    let mut schema = cubedriver::DatabaseStructure::new();
    schema.insert("default.cluster_test".to_string(), structure(&["fatos"]).remove("cluster_test").unwrap());
    let tables = vec![("default.cluster_test".to_string(), "fatos".to_string())];
    let files = cubeserver::rbac::handlers::model::generate_into_model(
        &env.state, "ann", Some(org_id), &slug, &tables, &schema, false,
    )
    .await
    .unwrap();
    assert_eq!(files[0]["fileName"], "digital/fatos.yml");
    assert!(cube_names(&env, &admin).await.contains(&"fatos".to_string()));
});

api_test!(generate_is_all_or_nothing, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (org_id, slug) = org(&env, &store, &admin, "Digital").await;
    install_policy(&[("digital", &["default"])], &["default"]);
    env.writer.clone().unwrap()
        .write("ann", "seed", |_| Ok(vec![put_in(Some(org_id), "usado.yml", &cube("usado"), None)]))
        .await
        .unwrap();
    let before = store.model_version().await.unwrap();

    let err = cubeserver::rbac::handlers::model::generate_into_model(
        &env.state, "ann", Some(org_id), &slug, &pairs(&["fatos", "usado"]), &structure(&["fatos", "usado"]), false,
    )
    .await
    .unwrap_err();
    assert_eq!(err.status, StatusCode::BAD_REQUEST, "{err:?}");
    assert_eq!(store.model_version().await.unwrap(), before);
    assert_eq!(store.model_rows().await.unwrap().files.len(), 1);
});

api_test!(generate_refuses_foreign_connection, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (a, _) = org(&env, &store, &admin, "Alfa").await;
    let (b, _) = org(&env, &store, &admin, "Beta").await;
    install_policy(&[("alfa", &[]), ("beta", &["sofiadb"])], &["default"]);
    store.create_connection("sofiadb", b, ConnectionOrigin::Assigned).await.unwrap();
    let before = store.model_version().await.unwrap();
    let (status, body) = call(&env, Method::POST, &admin,
        &format!("/cube/v1/organizations/{a}/model/generate"),
        Some(json!({ "tables": [["sofiadb.cluster_test", "fatos"]] }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "A tabela sofiadb.cluster_test.fatos não é de uma conexão desta organização");
    assert_eq!(store.model_version().await.unwrap(), before);
});

api_test!(generate_adopts_a_generated_file, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (org_id, slug) = org(&env, &store, &admin, "Digital").await;
    install_policy(&[("digital", &["default"])], &["default"]);
    let generated = format!("# Generated by sync\n{}", cube("fatos").replace("sql_table: t", "sql_table: cluster_test.fatos"));
    env.writer.clone().unwrap()
        .write("sync", "seed", |_| Ok(vec![put_in(Some(org_id), "fatos.yml", &generated, Some("sofiadb-sync"))]))
        .await
        .unwrap();
    let id = store.model_rows().await.unwrap().files[0].id;

    cubeserver::rbac::handlers::model::generate_into_model(
        &env.state, "ann", Some(org_id), &slug, &pairs(&["fatos"]), &structure(&["fatos"]), false,
    )
    .await
    .unwrap();
    let rows = store.model_rows().await.unwrap();
    assert_eq!(rows.files.len(), 1);
    assert_eq!((rows.files[0].id, rows.files[0].managed_by.clone()), (id, None));
    assert!(!rows.files[0].content.starts_with("# Generated by"));
});

// ---- what an owner sees (Task 11) ----

/// A plain user who owns a new organization named `name`: their token, the
/// organization's id and slug.
async fn owner_org(
    env: &Env,
    store: &cubeserver::rbac::store::Store,
    admin: &str,
    user: &str,
    name: &str,
) -> (String, uuid::Uuid, String) {
    store
        .create_user(&cubeserver::rbac::store::NewUser {
            id: user.into(),
            name: None,
            email: None,
            password_hash: cubeserver::rbac::identity::hash_password("owner-password-1").unwrap(),
        })
        .await
        .unwrap();
    let (status, body) = call(env, Method::POST, admin, "/cube/v1/organizations",
        Some(json!({ "name": name, "owner": user }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let slug = body["slug"].as_str().unwrap().to_string();
    let org = store.organization_by_slug(&slug).await.unwrap().unwrap();
    (login(env, user, "owner-password-1").await, org.id, slug)
}

api_test!(owner_sees_no_foreign_path_on_duplicate, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (digital, _) = org(&env, &store, &admin, "Digital").await;
    let (token, on, _) = owner_org(&env, &store, &admin, "olga", "ON").await;
    install_policy(&[("digital", &["default"]), ("on", &["sofiadb"])], &["default"]);
    env.writer.clone().unwrap()
        .write("ann", "seed", |_| Ok(vec![
            put_in(Some(digital), "fatos.yml", &cube("fatos"), None),
            put_in(Some(on), "x.yml", &cube_on("outro", "sofiadb"), None),
        ]))
        .await
        .unwrap();
    let rows = store.model_rows().await.unwrap();
    let row = rows.files.iter().find(|f| f.organization_id == Some(on)).unwrap();

    let (status, body) = call(&env, Method::PUT, &token,
        &format!("/cube/v1/organizations/{on}/model/files/{}", row.id),
        Some(json!({ "content": cube_on("fatos", "sofiadb"), "version": row.version }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(!body.to_string().contains("digital"), "{body}");
    assert!(body.to_string().contains("O nome de cubo fatos já está em uso na plataforma; escolha outro"), "{body}");
});

api_test!(owner_generate_does_not_adopt_foreign, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (digital, _) = org(&env, &store, &admin, "Digital").await;
    let (_, on, slug) = owner_org(&env, &store, &admin, "olga", "ON").await;
    install_policy(&[("digital", &["default"]), ("on", &["sofiadb"])], &["default"]);
    let generated = format!("# Generated by sync\n{}", cube("fatos").replace("sql_table: t", "sql_table: cluster_test.fatos"));
    env.writer.clone().unwrap()
        .write("sync", "seed", |_| Ok(vec![put_in(Some(digital), "fatos.yml", &generated, Some("sofiadb-sync"))]))
        .await
        .unwrap();
    let before = store.model_version().await.unwrap();

    let err = cubeserver::rbac::handlers::model::generate_into_model(
        &env.state, "olga", Some(on), &slug, &pairs(&["fatos"]), &structure(&["fatos"]), true,
    )
    .await
    .unwrap_err();
    assert_eq!(err.status, StatusCode::BAD_REQUEST, "{err:?}");
    assert_eq!(err.body.error, "O nome de cubo fatos já está em uso na plataforma; escolha outro");
    assert!(!err.body.error.contains("digital"));
    assert_eq!(store.model_version().await.unwrap(), before);
    assert_eq!(store.model_rows().await.unwrap().files.len(), 1);
});

api_test!(owner_adopts_own_generated_file, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (_, on, slug) = owner_org(&env, &store, &admin, "olga", "ON").await;
    install_policy(&[("on", &["default"])], &[]);
    let generated = format!("# Generated by playground\n{}", cube("fatos").replace("sql_table: t", "sql_table: cluster_test.fatos"));
    env.writer.clone().unwrap()
        .write("olga", "seed", |_| Ok(vec![put_in(Some(on), "fatos.yml", &generated, None)]))
        .await
        .unwrap();
    cubeserver::rbac::handlers::model::generate_into_model(
        &env.state, "olga", Some(on), &slug, &pairs(&["fatos"]), &structure(&["fatos"]), true,
    )
    .await
    .unwrap();
    let rows = store.model_rows().await.unwrap();
    assert_eq!(rows.files.len(), 1);
    assert!(!rows.files[0].content.starts_with("# Generated by"));
});

api_test!(owner_does_not_adopt_foreign_generated_or_own_sync_file, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (digital, _) = org(&env, &store, &admin, "Digital").await;
    let (_, on, slug) = owner_org(&env, &store, &admin, "olga", "ON").await;
    install_policy(&[("digital", &["default"]), ("on", &["sofiadb"])], &["default"]);
    let plain = format!("# Generated by playground\n{}", cube("fatos").replace("sql_table: t", "sql_table: cluster_test.fatos"));
    let sync = format!("# Generated by sync-sofiadb-model.py\n{}", cube("dados").replace("sql_table: t", "sql_table: cluster_test.dados"));
    env.writer.clone().unwrap()
        .write("x", "seed", |_| Ok(vec![
            put_in(Some(digital), "fatos.yml", &plain, None),
            put_in(Some(on), "dados.yml", &sync.replace("data_source: default", "data_source: sofiadb"), Some("sofiadb-sync")),
        ]))
        .await
        .unwrap();
    for name in ["fatos", "dados"] {
        let err = cubeserver::rbac::handlers::model::generate_into_model(
            &env.state, "olga", Some(on), &slug, &pairs(&[name]), &structure(&[name]), true,
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST, "{err:?}");
        assert_eq!(err.body.error, format!("O nome de cubo {name} já está em uso na plataforma; escolha outro"));
    }
});

api_test!(owner_breaking_a_foreign_join_sees_no_names, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (digital, _) = org(&env, &store, &admin, "Digital").await;
    let (token, on, _) = owner_org(&env, &store, &admin, "olga", "ON").await;
    install_policy(&[("digital", &["default"]), ("on", &["sofiadb"])], &["default"]);
    let joiner = "cubes:\n  - name: segredo\n    sql_table: t\n    data_source: default\n    joins:\n      - name: base_on\n        sql: \"1=1\"\n        relationship: many_to_one\n    measures:\n      - name: n\n        type: count\n";
    // The ownership rules refuse this reference, so it is seeded around the
    // writer: it stands for a dependency the rules do not see.
    let seed = vec![
        put_in(Some(on), "base.yml", &cube_on("base_on", "sofiadb"), None),
        put_in(Some(digital), "j.yml", joiner, None),
    ];
    store.model_write("ann", |_| Ok::<_, WriteError>(seed)).await.unwrap().unwrap();
    let row = store.model_rows().await.unwrap().files.into_iter().find(|f| f.organization_id == Some(on)).unwrap();
    let (status, body) = call(&env, Method::DELETE, &token,
        &format!("/cube/v1/organizations/{on}/model/files/{}?version={}", row.id, row.version), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let text = body.to_string();
    assert!(!text.contains("segredo") && !text.contains("digital"), "{text}");
});

api_test!(owner_sees_only_own_errors_in_detail, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (digital, _) = org(&env, &store, &admin, "Digital").await;
    let (token, on, _) = owner_org(&env, &store, &admin, "olga", "ON").await;
    install_policy(&[("digital", &["default"]), ("on", &["sofiadb"])], &["default"]);
    // Written around the writer, which would refuse it.
    let broken = put_in(Some(digital), "ruim.yml", "cubes: [ {", None);
    store.model_write("ann", |_| Ok::<_, WriteError>(vec![broken])).await.unwrap().unwrap();
    env.runtime.clone().unwrap().refresh().await.unwrap();

    let (status, body) = call(&env, Method::GET, &token, &format!("/cube/v1/organizations/{on}/model"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // The test environment's own data source cannot list tables: not a model problem.
    let errors: Vec<&Value> = body["errors"].as_array().unwrap().iter()
        .filter(|e| !e["message"].as_str().unwrap().starts_with("sofiadb:"))
        .collect();
    assert_eq!(errors.len(), 1, "{body}");
    assert_eq!(errors[0]["message"], "Há um problema no modelo fora da sua organização; avise o administrador");
    assert!(!body.to_string().contains("digital"), "{body}");
    // A platform admin keeps the full message.
    let (_, full) = call(&env, Method::GET, &admin, &format!("/cube/v1/organizations/{on}/model"), None).await;
    assert!(full.to_string().contains("digital/ruim.yml"), "{full}");
});

// ---- the permission matrix of the spec ----

api_test!(owner_reaches_only_own_model, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (olga, a, _) = owner_org(&env, &store, &admin, "olga", "Alfa").await;
    let (b, _) = org(&env, &store, &admin, "Beta").await;
    install_policy(&[("alfa", &["default"]), ("beta", &["default"])], &[]);
    env.writer.clone().unwrap()
        .write("ann", "seed", |_| Ok(vec![
            put_in(Some(a), "a.yml", &cube("a"), None),
            put_in(Some(b), "b.yml", &cube("b"), None),
        ]))
        .await
        .unwrap();
    let rows = store.model_rows().await.unwrap();
    let row = |org| rows.files.iter().find(|f| f.organization_id == Some(org)).unwrap();
    let (fa, fb) = (row(a), row(b));
    let before = store.model_version().await.unwrap();
    let edit = |v: i32| Some(json!({ "content": cube("z"), "version": v }));
    let generate = Some(json!({ "tables": [["default.cluster_test", "fatos"]] }));

    // The owner of A cannot see B: B's routes are 404.
    let ob = format!("/cube/v1/organizations/{b}/model");
    let fb_url = format!("{ob}/files/{}", fb.id);
    assert_eq!(call(&env, Method::GET, &olga, &ob, None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&env, Method::GET, &olga, &fb_url, None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&env, Method::PUT, &olga, &fb_url, edit(fb.version)).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&env, Method::DELETE, &olga, &format!("{fb_url}?version={}", fb.version), None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&env, Method::POST, &olga, &format!("{ob}/generate"), generate.clone()).await.0, StatusCode::NOT_FOUND);

    // A's id with a file of B: 404 too.
    let oa = format!("/cube/v1/organizations/{a}/model");
    let cross = format!("{oa}/files/{}", fb.id);
    assert_eq!(call(&env, Method::GET, &olga, &cross, None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&env, Method::PUT, &olga, &cross, edit(fb.version)).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&env, Method::DELETE, &olga, &format!("{cross}?version={}", fb.version), None).await.0, StatusCode::NOT_FOUND);
    // Its own model is open to it.
    assert_eq!(call(&env, Method::GET, &olga, &oa, None).await.0, StatusCode::OK);
    assert_eq!(call(&env, Method::GET, &olga, &format!("{oa}/files/{}", fa.id), None).await.0, StatusCode::OK);

    // A workspace member of A who is not its owner: 403 on A's model routes.
    store
        .create_user(&cubeserver::rbac::store::NewUser {
            id: "bia".into(),
            name: None,
            email: None,
            password_hash: cubeserver::rbac::identity::hash_password("member-password-1").unwrap(),
        })
        .await
        .unwrap();
    let (status, ws) = call(&env, Method::POST, &olga, &format!("/cube/v1/organizations/{a}/workspaces"),
        Some(json!({ "name": "W1" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{ws}");
    let ws = ws["id"].as_str().unwrap().to_string();
    let (status, body) = call(&env, Method::PUT, &olga, &format!("/cube/v1/workspaces/{ws}/members/bia"),
        Some(json!({ "role": "editor" }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let bia = login(&env, "bia", "member-password-1").await;
    let fa_url = format!("{oa}/files/{}", fa.id);
    assert_eq!(call(&env, Method::GET, &bia, &oa, None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&env, Method::GET, &bia, &fa_url, None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&env, Method::PUT, &bia, &fa_url, edit(fa.version)).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&env, Method::DELETE, &bia, &format!("{fa_url}?version={}", fa.version), None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&env, Method::POST, &bia, &format!("{oa}/generate"), generate).await.0, StatusCode::FORBIDDEN);

    assert_eq!(store.model_version().await.unwrap(), before);
});

// ---------------------------------------------------------------- SofiaDB sync

const SYNC_USER: &str = "sofiadb-sync";
const SYNC_PASSWORD: &str = "sync-password-1";

/// The sync user, logged in, plus the admin's token.
async fn sync_tokens(env: &Env, store: &cubeserver::rbac::store::Store) -> (String, String) {
    store
        .create_user(&cubeserver::rbac::store::NewUser {
            id: SYNC_USER.into(),
            name: None,
            email: None,
            password_hash: cubeserver::rbac::identity::hash_password(SYNC_PASSWORD).unwrap(),
        })
        .await
        .unwrap();
    (
        login(env, ADMIN, ADMIN_PASSWORD).await,
        login(env, SYNC_USER, SYNC_PASSWORD).await,
    )
}

async fn own_connection(store: &cubeserver::rbac::store::Store, org: uuid::Uuid) {
    store
        .create_connection("sofiadb", org, cubeserver::rbac::store::ConnectionOrigin::Created)
        .await
        .unwrap();
}

/// Asks for the context, then sends `files` with it.
async fn sync_put(env: &Env, token: &str, files: &[(&str, &str)]) -> (StatusCode, Value) {
    let (status, context) = call(env, Method::GET, token, "/cube/v1/model/sync/sofiadb/context?defaultDb=dfos", None).await;
    assert_eq!(status, StatusCode::OK, "{context}");
    let files: Vec<Value> = files.iter().map(|(p, c)| json!({ "path": p, "content": c })).collect();
    call(env, Method::PUT, token, "/cube/v1/model/sync/sofiadb",
        Some(json!({ "files": files, "context": context, "defaultDb": "dfos" }))).await
}

async fn sync_rows(store: &cubeserver::rbac::store::Store) -> Vec<(Option<uuid::Uuid>, String, Option<String>)> {
    let mut rows: Vec<_> = store.model_rows().await.unwrap().files
        .into_iter().map(|f| (f.organization_id, f.path, f.managed_by)).collect();
    rows.sort_by(|a, b| a.1.cmp(&b.1));
    rows
}

api_test!(sync_replaces_only_managed_rows, |store| {
    let env = env_live(store.clone()).await;
    let (admin, sync) = sync_tokens(&env, &store).await;
    let (org_id, _) = org(&env, &store, &admin, "Digital").await;
    own_connection(&store, org_id).await;
    install_policy(&[("digital", &["sofiadb"])], &["default"]);
    env.writer.clone().unwrap().write("sync", "seed", |_| Ok(vec![
        put_in(Some(org_id), "mao.yml", &cube_on("mao", "sofiadb"), None),
        put_in(Some(org_id), "sofiadb/velho.yml", &cube_on("velho", "sofiadb"), Some("sofiadb-sync")),
        put_in(Some(org_id), "sofiadb/fica.yml", &cube_on("fica", "sofiadb"), Some("sofiadb-sync")),
        put_in(Some(org_id), "sofiadb/muda.yml", &cube_on("muda", "sofiadb"), Some("sofiadb-sync")),
        put_in(Some(org_id), "outra/x.yml", &cube_on("outrox", "sofiadb"), Some("sofiadb-sync")),
    ])).await.unwrap();
    let (status, body) = sync_put(&env, &sync, &[
        ("sofiadb/fica.yml", &cube_on("fica", "sofiadb")),
        ("sofiadb/muda.yml", &cube_on("muda2", "sofiadb")),
        ("sofiadb/novo.yml", &cube_on("novo", "sofiadb")),
    ]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!((body["changed"].clone(), body["added"].clone(), body["updated"].clone(), body["removed"].clone()),
        (json!(true), json!(1), json!(1), json!(1)));
    let paths: Vec<String> = sync_rows(&store).await.into_iter().map(|r| r.1).collect();
    assert_eq!(paths, ["mao.yml", "outra/x.yml", "sofiadb/fica.yml", "sofiadb/muda.yml", "sofiadb/novo.yml"]);
    let rows = store.model_rows().await.unwrap();
    let muda = rows.files.iter().find(|f| f.path == "sofiadb/muda.yml").unwrap();
    assert!(muda.content.contains("muda2") && muda.managed_by.as_deref() == Some("sofiadb-sync"));
});

api_test!(same_set_is_no_change, |store| {
    let env = env_live(store.clone()).await;
    let (admin, sync) = sync_tokens(&env, &store).await;
    let (org_id, _) = org(&env, &store, &admin, "Digital").await;
    own_connection(&store, org_id).await;
    install_policy(&[("digital", &["sofiadb"])], &["default"]);
    let files = [("sofiadb/a.yml", cube_on("a", "sofiadb")), ("sofiadb/b.yml", cube_on("b", "sofiadb"))];
    let files: Vec<(&str, &str)> = files.iter().map(|(p, c)| (*p, c.as_str())).collect();
    let (status, body) = sync_put(&env, &sync, &files).await;
    assert_eq!((status, body["changed"].clone(), body["added"].clone()), (StatusCode::OK, json!(true), json!(2)), "{body}");
    let version = store.model_version().await.unwrap();
    let (status, body) = sync_put(&env, &sync, &files).await;
    assert_eq!((status, body["changed"].clone()), (StatusCode::OK, json!(false)), "{body}");
    assert_eq!(store.model_version().await.unwrap(), version);
});

api_test!(sync_moves_rows_with_the_connection, |store| {
    let env = env_live(store.clone()).await;
    let (admin, sync) = sync_tokens(&env, &store).await;
    let (a, _) = org(&env, &store, &admin, "Alfa").await;
    let (b, _) = org(&env, &store, &admin, "Beta").await;
    install_policy(&[("alfa", &["sofiadb"]), ("beta", &[])], &["default"]);
    env.writer.clone().unwrap().write("sync", "seed", |_| Ok(vec![
        put_in(Some(a), "sofiadb/a.yml", &cube_on("a", "sofiadb"), Some("sofiadb-sync")),
    ])).await.unwrap();
    let id = store.model_rows().await.unwrap().files[0].id;
    // The connection now belongs to Beta.
    own_connection(&store, b).await;
    install_policy(&[("alfa", &[]), ("beta", &["sofiadb"])], &["default"]);
    let (status, body) = sync_put(&env, &sync, &[("sofiadb/a.yml", &cube_on("a", "sofiadb"))]).await;
    assert_eq!((status, body["changed"].clone()), (StatusCode::OK, json!(true)), "{body}");
    let rows = store.model_rows().await.unwrap();
    assert_eq!(rows.files.len(), 1);
    assert_eq!((rows.files[0].id, rows.files[0].organization_id), (id, Some(b)));
});

api_test!(broken_sync_file_changes_nothing, |store| {
    let env = env_live(store.clone()).await;
    let (admin, sync) = sync_tokens(&env, &store).await;
    let (org_id, _) = org(&env, &store, &admin, "Digital").await;
    own_connection(&store, org_id).await;
    install_policy(&[("digital", &["sofiadb"])], &["default"]);
    env.writer.clone().unwrap().write("sync", "seed", |_| Ok(vec![
        put_in(Some(org_id), "sofiadb/a.yml", &cube_on("a", "sofiadb"), Some("sofiadb-sync")),
    ])).await.unwrap();
    let version = store.model_version().await.unwrap();
    let (status, body) = sync_put(&env, &sync, &[
        ("sofiadb/a.yml", &cube_on("a", "sofiadb")),
        ("sofiadb/ruim.yml", "cubes: [ {"),
    ]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, body) = sync_put(&env, &sync, &[("outro/x.yml", &cube_on("x", "sofiadb"))]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].as_str().unwrap().contains("outro/x.yml"), "{body}");
    assert_eq!(store.model_version().await.unwrap(), version);
    assert_eq!(sync_rows(&store).await.len(), 1);
});

api_test!(stale_context_is_409, |store| {
    let env = env_live(store.clone()).await;
    let (admin, sync) = sync_tokens(&env, &store).await;
    let (org_id, _) = org(&env, &store, &admin, "Digital").await;
    own_connection(&store, org_id).await;
    install_policy(&[("digital", &["sofiadb"])], &["default"]);
    let (_, context) = call(&env, Method::GET, &sync, "/cube/v1/model/sync/sofiadb/context?defaultDb=dfos", None).await;
    // A hand-written cube shows up after the context was read.
    env.writer.clone().unwrap().write("ann", "seed", |_| Ok(vec![
        put_in(Some(org_id), "mao.yml", &cube_on("mao", "sofiadb"), None),
    ])).await.unwrap();
    let version = store.model_version().await.unwrap();
    let (status, body) = call(&env, Method::PUT, &sync, "/cube/v1/model/sync/sofiadb", Some(json!({
        "files": [{ "path": "sofiadb/a.yml", "content": cube_on("a", "sofiadb") }],
        "context": context,
        "defaultDb": "dfos",
    }))).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::CONFLICT, Some("O modelo mudou durante a sincronização")), "{body}");
    assert_eq!(store.model_version().await.unwrap(), version);
});

api_test!(only_the_sync_user, |store| {
    let env = env_live(store.clone()).await;
    let (admin, sync) = sync_tokens(&env, &store).await;
    let (org_id, _) = org(&env, &store, &admin, "Digital").await;
    own_connection(&store, org_id).await;
    install_policy(&[("digital", &["sofiadb"])], &["default"]);
    let message = Some("Só o usuário de sincronização usa esta rota");
    let (status, body) = call(&env, Method::GET, &admin, "/cube/v1/model/sync/sofiadb/context", None).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::FORBIDDEN, message), "{body}");
    let (status, body) = call(&env, Method::PUT, &admin, "/cube/v1/model/sync/sofiadb", Some(json!({
        "files": [], "context": { "taken": [], "covered": {} }, "defaultDb": "dfos" }))).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::FORBIDDEN, message), "{body}");
    let (status, body) = sync_put(&env, &sync, &[("sofiadb/a.yml", &cube_on("a", "sofiadb"))]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
});

api_test!(sync_path_taken_by_a_hand_file_names_it, |store| {
    let env = env_live(store.clone()).await;
    let (admin, sync) = sync_tokens(&env, &store).await;
    let (org_id, _) = org(&env, &store, &admin, "Digital").await;
    own_connection(&store, org_id).await;
    install_policy(&[("digital", &["sofiadb"])], &["default"]);
    env.writer.clone().unwrap().write("ann", "seed", |_| Ok(vec![
        put_in(Some(org_id), "sofiadb/a.yml", &cube_on("mao", "sofiadb"), None),
    ])).await.unwrap();
    let version = store.model_version().await.unwrap();
    let (status, body) = sync_put(&env, &sync, &[
        ("sofiadb/a.yml", &cube_on("a", "sofiadb")),
        ("sofiadb/b.yml", &cube_on("b", "sofiadb")),
    ]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].as_str().unwrap().contains("sofiadb/a.yml"), "{body}");
    assert!(!body["error"].as_str().unwrap().contains("sofiadb/b.yml"), "{body}");
    assert_eq!(store.model_version().await.unwrap(), version);
});
