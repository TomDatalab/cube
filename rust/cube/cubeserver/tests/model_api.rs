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

/// The cube names of organization `slug`'s model, as a platform admin
/// reads it (`X-Cube-Organization`).
async fn cube_names_in(env: &Env, token: &str, slug: &str) -> Vec<String> {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let request = axum::http::Request::builder()
        .uri("/cube/v1/meta")
        .header(axum::http::header::HOST, "localhost:4000")
        .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
        .header(cubeserver::handlers::auth::ORGANIZATION_HEADER, slug)
        .body(axum::body::Body::empty())
        .unwrap();
    let response = env.router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
    assert_eq!(status, StatusCode::OK, "{body}");
    body["cubes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect()
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

api_test!(delete_of_a_cube_a_view_includes_is_400, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let view = "views:\n  - name: v\n    cubes:\n      - join_path: b\n        includes: \"*\"\n";
    env.writer.clone().unwrap()
        .write("ann", "seed", |_| Ok(vec![put_in(None, "b.yml", &cube("b"), None), put_in(None, "v.yml", view, None)]))
        .await
        .unwrap();
    let (_, detail) = call(&env, Method::GET, &admin, "/cube/v1/platform/model", None).await;
    let (id, version) = file_in(&detail, "_platform/b.yml");
    let (status, body) = call(&env, Method::DELETE, &admin, &format!("/cube/v1/platform/model/files/{id}?version={version}"), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["problems"].to_string().contains("Cube b doesn't exist"), "{body}");
    assert_eq!(call(&env, Method::GET, &admin, &format!("/cube/v1/platform/model/files/{id}"), None).await.0, StatusCode::OK);
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
    assert!(cube_names_in(&env, &admin, &slug).await.contains(&"fatos".to_string()));
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
    assert!(cube_names_in(&env, &admin, &slug).await.contains(&"fatos".to_string()));
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
            username: user.into(),
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
    let (token, on, _) = owner_org(&env, &store, &admin, "olga", "ON").await;
    install_policy(&[("on", &["sofiadb"])], &["default"]);
    env.writer.clone().unwrap()
        .write("ann", "seed", |_| Ok(vec![
            put_in(None, "fatos.yml", &cube("fatos"), None),
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
    assert!(body.to_string().contains("O nome de cubo fatos já está em uso na sua organização ou no Platform; escolha outro"), "{body}");
});

api_test!(owner_generate_does_not_adopt_foreign, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (digital, _) = org(&env, &store, &admin, "Digital").await;
    let (_, on, slug) = owner_org(&env, &store, &admin, "olga", "ON").await;
    // ON reads `default` too, so a cube of its own on it compiles.
    install_policy(&[("digital", &["default"]), ("on", &["sofiadb", "default"])], &["default"]);
    let generated = format!("# Generated by sync\n{}", cube("fatos").replace("sql_table: t", "sql_table: cluster_test.fatos"));
    env.writer.clone().unwrap()
        .write("sync", "seed", |_| Ok(vec![put_in(Some(digital), "fatos.yml", &generated, Some("sofiadb-sync"))]))
        .await
        .unwrap();
    let before = store.model_version().await.unwrap();

    // A name is unique per organization: ON gets a `fatos` of its own, and
    // Digital's file is neither adopted nor named.
    let files = cubeserver::rbac::handlers::model::generate_into_model(
        &env.state, "olga", Some(on), &slug, &pairs(&["fatos"]), &structure(&["fatos"]), true,
    )
    .await
    .unwrap();
    assert_eq!(files[0]["fileName"], "on/fatos.yml");
    assert!(!json!(files).to_string().contains("digital"));
    assert!(store.model_version().await.unwrap() > before);
    let rows = store.model_rows().await.unwrap();
    assert_eq!(rows.files.len(), 2);
    let digital_row = rows.files.iter().find(|f| f.organization_id == Some(digital)).unwrap();
    assert_eq!(digital_row.content, generated);
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
    // ON reads `default` too, so a cube of its own on it compiles.
    install_policy(&[("digital", &["default"]), ("on", &["sofiadb", "default"])], &["default"]);
    let plain = format!("# Generated by playground\n{}", cube("fatos").replace("sql_table: t", "sql_table: cluster_test.fatos"));
    let sync = format!("# Generated by sync-sofiadb-model.py\n{}", cube("dados").replace("sql_table: t", "sql_table: cluster_test.dados"));
    env.writer.clone().unwrap()
        .write("x", "seed", |_| Ok(vec![
            put_in(Some(digital), "fatos.yml", &plain, None),
            put_in(Some(on), "dados.yml", &sync.replace("data_source: default", "data_source: sofiadb"), Some("sofiadb-sync")),
        ]))
        .await
        .unwrap();
    // Its own sync file is not adopted.
    let err = cubeserver::rbac::handlers::model::generate_into_model(
        &env.state, "olga", Some(on), &slug, &pairs(&["dados"]), &structure(&["dados"]), true,
    )
    .await
    .unwrap_err();
    assert_eq!(err.status, StatusCode::BAD_REQUEST, "{err:?}");
    assert_eq!(err.body.error, "O nome de cubo dados já está em uso na sua organização ou no Platform; escolha outro");
    // Digital's generated file is another organization's name: ON gets its
    // own cube and Digital's file stays as it was.
    let files = cubeserver::rbac::handlers::model::generate_into_model(
        &env.state, "olga", Some(on), &slug, &pairs(&["fatos"]), &structure(&["fatos"]), true,
    )
    .await
    .unwrap();
    assert_eq!(files[0]["fileName"], "on/fatos.yml");
    let rows = store.model_rows().await.unwrap();
    let digital_row = rows.files.iter().find(|f| f.organization_id == Some(digital)).unwrap();
    assert_eq!(digital_row.content, plain);
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
    // The reference crosses organizations, so ON's scope never sees it: the
    // delete compiles there and nothing of Digital is mentioned.
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
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
    // Digital's broken file is not in ON's scope: ON's model compiles.
    assert!(errors.is_empty(), "{body}");
    assert!(!body.to_string().contains("digital"), "{body}");
    // Digital's model shows it, with the full message for a platform admin.
    let (_, full) = call(&env, Method::GET, &admin, &format!("/cube/v1/organizations/{digital}/model"), None).await;
    assert!(full.to_string().contains("digital/ruim.yml"), "{full}");

    // A broken `_platform/` file is in ON's scope: the owner gets the
    // generic line, nothing of the file.
    let broken = put_in(None, "ruim.yml", "cubes: [ {", None);
    store.model_write("ann", |_| Ok::<_, WriteError>(vec![broken])).await.unwrap().unwrap();
    env.runtime.clone().unwrap().refresh().await.unwrap();
    let (status, body) = call(&env, Method::GET, &token, &format!("/cube/v1/organizations/{on}/model"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let errors: Vec<&Value> = body["errors"].as_array().unwrap().iter()
        .filter(|e| !e["message"].as_str().unwrap().starts_with("sofiadb:"))
        .collect();
    assert_eq!(
        errors,
        [&json!({ "file": "", "cube": "", "message": "Há um problema no modelo fora da sua organização; avise o administrador" })],
        "{body}"
    );
    assert!(!body.to_string().contains("ruim.yml") && !body.to_string().contains("digital"), "{body}");
    // The admin, on the same node, keeps the full path.
    let (_, full) = call(&env, Method::GET, &admin, &format!("/cube/v1/organizations/{on}/model"), None).await;
    assert!(full.to_string().contains("_platform/ruim.yml"), "{full}");
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
            username: "bia".into(),
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
            username: SYNC_USER.into(),
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

// ---- names are unique per organization ----

api_test!(same_cube_name_in_two_organizations_is_allowed, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (digital, _) = org(&env, &store, &admin, "Digital").await;
    let (on, _) = org(&env, &store, &admin, "ON").await;
    install_policy(&[("digital", &["default"]), ("on", &["sofiadb"])], &["default"]);
    let writer = env.writer.clone().unwrap();
    writer
        .write("ann", "a", |_| Ok(vec![put_in(Some(digital), "a.yml", &cube("a"), None)]))
        .await
        .unwrap();
    let r = writer
        .write("ann", "b", |_| Ok(vec![put_in(Some(on), "a.yml", &cube_on("a", "sofiadb"), None)]))
        .await;
    assert!(r.is_ok(), "{r:?}");
});

api_test!(platform_name_clash_with_an_organization_is_refused, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (digital, _) = org(&env, &store, &admin, "Digital").await;
    install_policy(&[("digital", &["default"])], &["default"]);
    let writer = env.writer.clone().unwrap();
    writer
        .write("ann", "a", |_| Ok(vec![put_in(Some(digital), "a.yml", &cube("a"), None)]))
        .await
        .unwrap();
    let err = writer
        .write("ann", "p", |_| Ok(vec![put_in(None, "a.yml", &cube("a"), None)]))
        .await
        .unwrap_err();
    assert_eq!(err.status, StatusCode::BAD_REQUEST, "{err:?}");
    assert_eq!(err.message, "A mudança não compila");
    assert!(err.problems.iter().any(|p| p.message.contains('a')), "{err:?}");
});

api_test!(duplicate_inside_one_organization_is_still_refused, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (token, on, _) = owner_org(&env, &store, &admin, "olga", "ON").await;
    install_policy(&[("on", &["sofiadb"])], &["default"]);
    env.writer.clone().unwrap()
        .write("ann", "seed", |_| Ok(vec![
            put_in(Some(on), "x.yml", &cube_on("a", "sofiadb"), None),
            put_in(Some(on), "y.yml", &cube_on("b", "sofiadb"), None),
        ]))
        .await
        .unwrap();
    let rows = store.model_rows().await.unwrap();
    let row = rows.files.iter().find(|f| f.path == "y.yml").unwrap();
    let (status, body) = call(&env, Method::PUT, &token,
        &format!("/cube/v1/organizations/{on}/model/files/{}", row.id),
        Some(json!({ "content": cube_on("a", "sofiadb"), "version": row.version }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.to_string().contains(&cubeserver::rbac::model_redact::duplicate_name_message("a")), "{body}");
});

// ---- problems are new scope by scope ----

fn joiner(name: &str, target: &str) -> String {
    format!(
        "cubes:\n  - name: {name}\n    sql_table: t\n    data_source: default\n    joins:\n      - name: {target}\n        sql: \"1=1\"\n        relationship: many_to_one\n    measures:\n      - name: n\n        type: count\n"
    )
}

api_test!(reassigning_a_cube_a_platform_cube_joins_is_refused, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (digital, _) = org(&env, &store, &admin, "Digital").await;
    let (on, _) = org(&env, &store, &admin, "ON").await;
    install_policy(&[("digital", &["sofiadb"]), ("on", &["sofiadb"])], &["default"]);
    // Around the writer: `u` already fails in ON's scope, where `fatos` is missing.
    let seed = vec![
        put_in(Some(digital), "fatos.yml", &cube_on("fatos", "sofiadb"), None),
        put_in(None, "u.yml", &joiner("u", "fatos"), None),
    ];
    store.model_write("ann", |_| Ok::<_, WriteError>(seed)).await.unwrap().unwrap();
    let id = store.model_rows().await.unwrap().files.into_iter().find(|f| f.path == "fatos.yml").unwrap().id;
    let (status, body) = call(&env, Method::PUT, &admin, &format!("/cube/v1/model/files/{id}/organization"),
        Some(json!({ "organizationId": on }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["problems"].to_string().contains("u.yml"), "{body}");
});

api_test!(owner_cannot_join_another_organizations_cube, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (token, digital, _) = owner_org(&env, &store, &admin, "dora", "Digital").await;
    let (on, _) = org(&env, &store, &admin, "ON").await;
    install_policy(&[("digital", &["default"]), ("on", &["sofiadb"])], &["default"]);
    env.writer.clone().unwrap()
        .write("ann", "seed", |_| Ok(vec![
            put_in(Some(on), "base.yml", &cube_on("base_on", "sofiadb"), None),
            put_in(Some(digital), "j.yml", &cube("j"), None),
        ]))
        .await
        .unwrap();
    let row = store.model_rows().await.unwrap().files.into_iter().find(|f| f.organization_id == Some(digital)).unwrap();
    let (status, body) = call(&env, Method::PUT, &token,
        &format!("/cube/v1/organizations/{digital}/model/files/{}", row.id),
        Some(json!({ "content": joiner("j", "base_on"), "version": row.version }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let text = body.to_string();
    // The owner's own file is named; ON's files and paths are not.
    assert!(text.contains("digital/j.yml") && !text.contains("on/"), "{text}");
});

api_test!(owner_deleting_a_cube_a_platform_cube_joins_is_refused, |store| {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (token, on, _) = owner_org(&env, &store, &admin, "olga", "ON").await;
    install_policy(&[("on", &["sofiadb"])], &["default"]);
    env.writer.clone().unwrap()
        .write("ann", "seed", |_| Ok(vec![put_in(Some(on), "base.yml", &cube_on("base_on", "sofiadb"), None)]))
        .await
        .unwrap();
    // Around the writer: the platform cube joins a cube of one organization.
    let seed = vec![put_in(None, "p.yml", &joiner("p", "base_on"), None)];
    store.model_write("ann", |_| Ok::<_, WriteError>(seed)).await.unwrap().unwrap();
    let row = store.model_rows().await.unwrap().files.into_iter().find(|f| f.organization_id == Some(on)).unwrap();
    let (status, body) = call(&env, Method::DELETE, &token,
        &format!("/cube/v1/organizations/{on}/model/files/{}?version={}", row.id, row.version), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.to_string().contains("Sua mudança quebra um cubo de outra organização"), "{body}");
});

// ---- Models: a canvas of joined tables built into one cube ----

const STALE_MODEL: &str = "O model mudou desde que você abriu; abra de novo";

/// As `api_test!`, with the URL of the test's own database: the `sofiadb`
/// data source reads it, so a build loads real columns.
macro_rules! build_test {
    ($name:ident, |$store:ident, $url:ident| $body:block) => {
        #[tokio::test]
        async fn $name() {
            let _serial = SERIAL.lock().await;
            let _clear = ClearPolicy;
            let Some(db) = TestDb::create().await else {
                return;
            };
            install_policy(&[], &["default"]);
            let $store = db.store.clone();
            let $url = support::with_db(&db.admin_url, &db.name);
            let _env = SofiadbEnv::point_at(&$url);
            let result = tokio::spawn(async move $body).await;
            drop(_env);
            db.drop_db().await;
            result.unwrap();
        }
    };
}

const SOFIADB_VARS: [&str; 3] =
    ["CUBEJS_DATASOURCES", "CUBEJS_DS_SOFIADB_DB_TYPE", "CUBEJS_DS_SOFIADB_DB_URL"];

/// Points the `sofiadb` data source at a database for as long as it lives,
/// then puts back what the variables held.
struct SofiadbEnv(Vec<Option<String>>);
impl SofiadbEnv {
    fn point_at(url: &str) -> Self {
        let saved = SOFIADB_VARS.iter().map(|var| std::env::var(var).ok()).collect();
        std::env::set_var(SOFIADB_VARS[0], "default,sofiadb");
        std::env::set_var(SOFIADB_VARS[1], "postgres");
        std::env::set_var(SOFIADB_VARS[2], url);
        SofiadbEnv(saved)
    }
}
impl Drop for SofiadbEnv {
    fn drop(&mut self) {
        for (var, saved) in SOFIADB_VARS.iter().zip(&self.0) {
            match saved {
                Some(value) => std::env::set_var(var, value),
                None => std::env::remove_var(var),
            }
        }
    }
}

/// `shop.orders` and `shop.customers` in the database at `url`.
async fn shop_tables(url: &str) {
    support::connect(url)
        .await
        .batch_execute(
            "CREATE SCHEMA shop; \
             CREATE TABLE shop.customers (id integer PRIMARY KEY, region text); \
             CREATE TABLE shop.orders (id integer PRIMARY KEY, customer_id integer, amount numeric);",
        )
        .await
        .unwrap();
}

/// Orders with their customer, on data source `ds`.
fn model_def(ds: &str) -> Value {
    json!({
        "dataSource": ds,
        "tables": [
            { "alias": "orders", "schema": "shop", "table": "orders", "x": 0, "y": 0 },
            { "alias": "customers", "schema": "shop", "table": "customers", "x": 240, "y": 0 },
        ],
        "primary": "orders",
        "primaryKey": "id",
        "joins": [{
            "from": { "alias": "orders", "column": "customer_id" },
            "to": { "alias": "customers", "column": "id" },
            "relationship": "many_to_one",
            "join": "left",
        }],
    })
}

/// Owner olga of organization ON, which owns connection `sofiadb`: the
/// env, her token, ON's id.
async fn models_env(store: &cubeserver::rbac::store::Store) -> (Env, String, uuid::Uuid) {
    let env = env_live(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (token, on, _) = owner_org(&env, store, &admin, "olga", "ON").await;
    install_policy(&[("on", &["sofiadb"])], &["default"]);
    own_connection(store, on).await;
    (env, token, on)
}

/// Creates model `name` in `org`: its id.
async fn new_model(env: &Env, token: &str, org: uuid::Uuid, name: &str) -> String {
    let (status, body) = call(env, Method::POST, token, &format!("/cube/v1/organizations/{org}/cube-models"),
        Some(json!({ "name": name, "definition": model_def("sofiadb") }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().unwrap().to_string()
}

async fn build(env: &Env, token: &str, id: &str, body: Value) -> (StatusCode, Value) {
    call(env, Method::POST, token, &format!("/cube/v1/cube-models/{id}/build"), Some(body)).await
}

api_test!(cube_models_crud_and_scope, |store| {
    let (env, token, on) = models_env(&store).await;
    let list = format!("/cube/v1/organizations/{on}/cube-models");

    let (status, created) = call(&env, Method::POST, &token, &list,
        Some(json!({ "name": "  Vendas ", "definition": model_def("sofiadb") }))).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["name"], "Vendas");
    assert_eq!(created["organizationId"], on.to_string());
    assert_eq!(created["version"], 1);
    assert_eq!(created["cubeNames"], json!([]));
    assert_eq!(created["definition"]["cubes"][0]["source"]["table"], "orders");
    assert_eq!(created["updatedBy"], "olga");
    assert!(created["updatedAt"].is_string(), "{created}");
    let id = created["id"].as_str().unwrap().to_string();
    let one = format!("/cube/v1/cube-models/{id}");

    let (status, body) = call(&env, Method::GET, &token, &list, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["models"].as_array().unwrap().len(), 1, "{body}");
    assert_eq!(body["models"][0]["id"], id);
    let (status, body) = call(&env, Method::GET, &token, &one, None).await;
    assert_eq!((status, &body["name"]), (StatusCode::OK, &json!("Vendas")), "{body}");

    let (status, body) = call(&env, Method::PUT, &token, &one, Some(json!({ "name": "Pedidos", "version": 1 }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!((&body["name"], &body["version"]), (&json!("Pedidos"), &json!(2)));
    assert_eq!(body["definition"]["cubes"][0]["source"]["table"], "orders");
    let (status, body) = call(&env, Method::PUT, &token, &one, Some(json!({ "name": "Outro", "version": 1 }))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], STALE_MODEL);

    // The shape is checked on every save.
    let mut bad = model_def("sofiadb");
    bad["primary"] = json!("nope");
    let (status, body) = call(&env, Method::POST, &token, &list, Some(json!({ "name": "Ruim", "definition": bad }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "A tabela principal nope não está no model");
    let (status, body) = call(&env, Method::PUT, &token, &one,
        Some(json!({ "definition": { "dataSource": "sofiadb", "tables": [], "joins": [], "extra": 1 }, "version": 2 }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    for name in ["   ", "a\nb", "tab\there", "a\u{2028}b", "a\u{2029}b"] {
        let (status, body) = call(&env, Method::POST, &token, &list,
            Some(json!({ "name": name, "definition": model_def("sofiadb") }))).await;
        assert_eq!((status, &body["error"]), (StatusCode::BAD_REQUEST, &json!("Nome inválido")), "{name:?}");
    }
    let (status, body) = call(&env, Method::PUT, &token, &one, Some(json!({ "name": "x\ny", "version": 2 }))).await;
    assert_eq!((status, &body["error"]), (StatusCode::BAD_REQUEST, &json!("Nome inválido")), "{body}");
    let (status, body) = call(&env, Method::POST, &token, &list,
        Some(json!({ "name": "Pedidos", "definition": model_def("sofiadb") }))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // A connection of no one's: refused before any column is read.
    let foreign = new_model(&env, &token, on, "Fora").await;
    let (status, body) = call(&env, Method::PUT, &token, &format!("/cube/v1/cube-models/{foreign}"),
        Some(json!({ "definition": model_def("outra"), "version": 1 }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = build(&env, &token, &foreign, json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "A conexão outra não é desta organização");

    // Another organization's owner, and a user of none: as if it did not exist.
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (other, beta, _) = owner_org(&env, &store, &admin, "paula", "Beta").await;
    store
        .create_user(&cubeserver::rbac::store::NewUser {
            username: "nina".into(),
            name: None,
            email: None,
            password_hash: cubeserver::rbac::identity::hash_password("nina-password-1").unwrap(),
        })
        .await
        .unwrap();
    let nobody = login(&env, "nina", "nina-password-1").await;
    for who in [&other, &nobody] {
        assert_eq!(call(&env, Method::GET, who, &one, None).await.0, StatusCode::NOT_FOUND);
        assert_eq!(call(&env, Method::PUT, who, &one, Some(json!({ "name": "Meu", "version": 2 }))).await.0, StatusCode::NOT_FOUND);
        assert_eq!(call(&env, Method::DELETE, who, &format!("{one}?version=2"), None).await.0, StatusCode::NOT_FOUND);
        assert_eq!(build(&env, who, &id, json!({})).await.0, StatusCode::NOT_FOUND);
        assert_eq!(call(&env, Method::GET, who, &list, None).await.0, StatusCode::NOT_FOUND);
        assert_eq!(call(&env, Method::POST, who, &list,
            Some(json!({ "name": "Meu", "definition": model_def("sofiadb") }))).await.0, StatusCode::NOT_FOUND);
    }
    // Paula's own organization has none of ON's models.
    let (status, body) = call(&env, Method::GET, &other, &format!("/cube/v1/organizations/{beta}/cube-models"), None).await;
    assert_eq!((status, body["models"].as_array().unwrap().len()), (StatusCode::OK, 0), "{body}");
    assert_eq!(call(&env, Method::GET, &token, "/cube/v1/cube-models/not-a-uuid", None).await.0, StatusCode::NOT_FOUND);

    let (status, body) = call(&env, Method::DELETE, &token, &one, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, body) = call(&env, Method::DELETE, &token, &format!("{one}?version=1"), None).await;
    assert_eq!((status, &body["error"]), (StatusCode::CONFLICT, &json!(STALE_MODEL)), "{body}");
    let (status, body) = call(&env, Method::DELETE, &token, &format!("{one}?version=2"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(call(&env, Method::GET, &token, &one, None).await.0, StatusCode::NOT_FOUND);
});

build_test!(building_a_model_writes_one_cube, |store, url| {
    shop_tables(&url).await;
    let (env, token, on) = models_env(&store).await;
    let id = new_model(&env, &token, on, "Vendas").await;

    let (status, built) = build(&env, &token, &id, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{built}");
    assert_eq!(built["cubes"].as_array().unwrap().len(), 1, "{built}");
    assert_eq!(built["removed"], json!([]));
    let file_name = built["cubes"][0]["fileName"].as_str().unwrap().to_string();
    assert_eq!(file_name, "on/vendas.yml");
    assert_eq!(built["cubes"][0]["cubeName"], "vendas");
    let content = built["cubes"][0]["content"].as_str().unwrap();
    assert!(content.starts_with("# Built from model Vendas ("), "{content}");
    assert!(content.contains("customers_region"), "{content}");
    let detail = &built["detail"];
    file_in(detail, &file_name);
    assert!(detail["cubes"].as_array().unwrap().iter().any(|c| c["name"] == "vendas"), "{detail}");
    assert_eq!(detail["models"], json!([{ "id": id, "name": "Vendas", "cubeNames": ["vendas"] }]));
    let (status, org_detail) = call(&env, Method::GET, &token, &format!("/cube/v1/organizations/{on}/model"), None).await;
    assert_eq!(status, StatusCode::OK, "{org_detail}");
    assert_eq!(org_detail["models"][0]["cubeNames"], json!(["vendas"]));
    let (_, one) = call(&env, Method::GET, &token, &format!("/cube/v1/cube-models/{id}"), None).await;
    assert_eq!((&one["cubeNames"], &one["version"]), (&json!(["vendas"]), &json!(1)), "{one}");

    // Renamed, the model still builds into the same cube and file.
    let (status, body) = call(&env, Method::PUT, &token, &format!("/cube/v1/cube-models/{id}"),
        Some(json!({ "name": "Pedidos", "version": 1 }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, rebuilt) = build(&env, &token, &id, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{rebuilt}");
    assert_eq!(rebuilt["cubes"][0]["fileName"], file_name.as_str());
    assert_eq!(rebuilt["cubes"][0]["cubeName"], "vendas");
    assert!(rebuilt["cubes"][0]["content"].as_str().unwrap().starts_with("# Built from model Pedidos ("));
    let rows = store.model_rows().await.unwrap();
    assert_eq!(rows.files.iter().filter(|f| f.organization_id == Some(on)).count(), 1);
});

build_test!(rebuilding_after_a_hand_edit_needs_force, |store, url| {
    shop_tables(&url).await;
    let (env, token, on) = models_env(&store).await;
    let id = new_model(&env, &token, on, "Vendas").await;
    let (status, built) = build(&env, &token, &id, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{built}");
    let (file, version) = file_in(&built["detail"], "on/vendas.yml");

    let edited = format!("{}# à mão\n", built["cubes"][0]["content"].as_str().unwrap());
    let (status, body) = call(&env, Method::PUT, &token, &format!("/cube/v1/organizations/{on}/model/files/{file}"),
        Some(json!({ "content": edited, "version": version }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = build(&env, &token, &id, json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "O cubo foi editado desde o último build");
    let row = store.model_file(file.parse().unwrap()).await.unwrap().unwrap();
    assert_eq!(row.content, edited);

    let (status, forced) = build(&env, &token, &id, json!({ "force": true })).await;
    assert_eq!(status, StatusCode::OK, "{forced}");
    assert!(forced["cubes"][0]["content"].as_str().unwrap().starts_with("# Built from model"));
    assert_eq!(forced["cubes"][0]["fileName"], "on/vendas.yml");
    let row = store.model_file(file.parse().unwrap()).await.unwrap().unwrap();
    assert_eq!(row.content, forced["cubes"][0]["content"].as_str().unwrap());
    // Built again, with nothing edited since: no force needed.
    assert_eq!(build(&env, &token, &id, json!({})).await.0, StatusCode::OK);
});

build_test!(deleting_the_model_keeps_the_cube, |store, url| {
    shop_tables(&url).await;
    let (env, token, on) = models_env(&store).await;
    let id = new_model(&env, &token, on, "Vendas").await;
    let (status, built) = build(&env, &token, &id, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{built}");
    let (status, body) = call(&env, Method::DELETE, &token, &format!("/cube/v1/cube-models/{id}?version=1"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (status, detail) = call(&env, Method::GET, &token, &format!("/cube/v1/organizations/{on}/model"), None).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["models"], json!([]));
    let (_, version) = file_in(&detail, "on/vendas.yml");
    assert_eq!(version, 1);
    assert!(detail["cubes"].as_array().unwrap().iter().any(|c| c["name"] == "vendas"), "{detail}");
    let row = store.model_rows().await.unwrap().files.into_iter().find(|f| f.path == "vendas.yml").unwrap();
    assert_eq!(row.content, built["cubes"][0]["content"].as_str().unwrap());
    // The cascade drops the records, not the files.
    let db = support::connect(&url).await;
    let left = db.query_one("SELECT count(*) FROM cube_model_cubes", &[]).await.unwrap();
    assert_eq!(left.get::<_, i64>(0), 0);
});

build_test!(a_build_whose_record_was_lost_adopts_its_file, |store, url| {
    shop_tables(&url).await;
    let (env, token, on) = models_env(&store).await;
    let id = new_model(&env, &token, on, "Vendas").await;
    let (status, built) = build(&env, &token, &id, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{built}");
    let db = support::connect(&url).await;
    let forget = |sql: &'static str| {
        let db = &db;
        let id: uuid::Uuid = id.parse().unwrap();
        async move { db.execute(sql, &[&id]).await.unwrap() }
    };

    // Nothing recorded, as after a first build whose record failed: the
    // file is still this model's, and only `force` overwrites it.
    forget("DELETE FROM cube_model_cubes WHERE model_id = $1").await;
    let (status, body) = build(&env, &token, &id, json!({})).await;
    assert_eq!((status, &body["error"]), (StatusCode::CONFLICT, &json!("O cubo foi editado desde o último build")), "{body}");
    let (status, body) = build(&env, &token, &id, json!({ "force": true })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["cubes"][0]["fileName"], "on/vendas.yml");
    assert_eq!(store.model_rows().await.unwrap().files.iter().filter(|f| f.organization_id == Some(on)).count(), 1);
    // Recorded again: a plain rebuild works.
    assert_eq!(build(&env, &token, &id, json!({})).await.0, StatusCode::OK);

    // A file at the path with another model's header keeps the 409.
    let other = new_model(&env, &token, on, "Vendas!").await;
    let (status, body) = build(&env, &token, &other, json!({ "force": true })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "O caminho vendas.yml já é de outro arquivo; renomeie ou mova esse arquivo");
});

// ---- Models with cubes: one file per cube ----

/// A cube of `model_def`'s table `alias`, in auto mode on its `id`.
fn table_cube(id: &str, name: &str, alias: &str) -> Value {
    json!({
        "id": id, "name": name,
        "source": { "kind": "table", "table": alias, "primaryKey": "id" },
        "dimensions": [], "measures": [], "segments": [], "x": 600, "y": 0,
    })
}

/// `model_def` with these cubes instead of `primary`.
fn cubes_def(cubes: &[Value]) -> Value {
    let mut def = model_def("sofiadb");
    let obj = def.as_object_mut().unwrap();
    obj.remove("primary");
    obj.remove("primaryKey");
    obj.insert("cubes".into(), json!(cubes));
    def
}

/// Saves `def` on model `id` at `version`.
async fn save_def(env: &Env, token: &str, id: &str, def: Value, version: i64) {
    let (status, body) = call(env, Method::PUT, token, &format!("/cube/v1/cube-models/{id}"),
        Some(json!({ "definition": def, "version": version }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// The paths of organization `org`'s files, sorted.
async fn org_paths(store: &cubeserver::rbac::store::Store, org: uuid::Uuid) -> Vec<String> {
    let mut paths: Vec<String> = store.model_rows().await.unwrap().files.into_iter()
        .filter(|f| f.organization_id == Some(org)).map(|f| f.path).collect();
    paths.sort();
    paths
}

fn two_cubes() -> Value {
    cubes_def(&[table_cube("c1", "orders", "orders"), table_cube("c2", "customers", "customers")])
}

api_test!(a_model_saved_before_cubes_opens_converted, |store| {
    let (env, token, on) = models_env(&store).await;
    let id = new_model(&env, &token, on, "Vendas Mensais").await;
    let (status, one) = call(&env, Method::GET, &token, &format!("/cube/v1/cube-models/{id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{one}");
    let def = &one["definition"];
    assert!(def.get("primary").is_none() && def.get("primaryKey").is_none(), "{def}");
    let cubes = def["cubes"].as_array().unwrap();
    assert_eq!(cubes.len(), 1, "{def}");
    assert_eq!((&cubes[0]["id"], &cubes[0]["name"]), (&json!("c1"), &json!("vendas_mensais")), "{def}");
    assert_eq!(cubes[0]["source"], json!({ "kind": "joined", "table": "orders", "primaryKey": "id" }));
    assert_eq!(one["cubeNames"], json!([]));
    let (_, list) = call(&env, Method::GET, &token, &format!("/cube/v1/organizations/{on}/cube-models"), None).await;
    assert_eq!(list["models"][0]["definition"]["cubes"][0]["name"], "vendas_mensais", "{list}");
});

build_test!(two_cubes_build_two_files, |store, url| {
    shop_tables(&url).await;
    let (env, token, on) = models_env(&store).await;
    let id = new_model(&env, &token, on, "Vendas").await;
    save_def(&env, &token, &id, two_cubes(), 1).await;

    let (status, built) = build(&env, &token, &id, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{built}");
    let names: Vec<(&Value, &Value)> = built["cubes"].as_array().unwrap().iter()
        .map(|c| (&c["cubeName"], &c["fileName"])).collect();
    assert_eq!(names, vec![(&json!("orders"), &json!("on/orders.yml")), (&json!("customers"), &json!("on/customers.yml"))]);
    assert_eq!(built["removed"], json!([]));
    assert_eq!(org_paths(&store, on).await, vec!["customers.yml", "orders.yml"]);
    assert_eq!(built["detail"]["models"][0]["cubeNames"], json!(["customers", "orders"]));
    let (_, one) = call(&env, Method::GET, &token, &format!("/cube/v1/cube-models/{id}"), None).await;
    assert_eq!(one["cubeNames"], json!(["customers", "orders"]));
    let (_, list) = call(&env, Method::GET, &token, &format!("/cube/v1/organizations/{on}/cube-models"), None).await;
    assert_eq!(list["models"][0]["cubeNames"], json!(["customers", "orders"]));
    // Built again: the same files.
    let (status, again) = build(&env, &token, &id, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(org_paths(&store, on).await, vec!["customers.yml", "orders.yml"]);
});

build_test!(removing_a_cube_removes_its_file, |store, url| {
    shop_tables(&url).await;
    let (env, token, on) = models_env(&store).await;
    let id = new_model(&env, &token, on, "Vendas").await;
    save_def(&env, &token, &id, two_cubes(), 1).await;
    assert_eq!(build(&env, &token, &id, json!({})).await.0, StatusCode::OK);

    save_def(&env, &token, &id, cubes_def(&[table_cube("c1", "orders", "orders")]), 2).await;
    let (status, built) = build(&env, &token, &id, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{built}");
    assert_eq!(built["removed"], json!(["on/customers.yml"]));
    assert_eq!(built["cubes"].as_array().unwrap().len(), 1, "{built}");
    assert_eq!(org_paths(&store, on).await, vec!["orders.yml"]);
    assert!(built["detail"]["files"].as_array().unwrap().iter().all(|f| f["fileName"] != "on/customers.yml"));
    assert_eq!(built["detail"]["models"][0]["cubeNames"], json!(["orders"]));
});

build_test!(removing_an_edited_cube_needs_force, |store, url| {
    shop_tables(&url).await;
    let (env, token, on) = models_env(&store).await;
    let id = new_model(&env, &token, on, "Vendas").await;
    save_def(&env, &token, &id, two_cubes(), 1).await;
    let (status, built) = build(&env, &token, &id, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{built}");
    let (file, version) = file_in(&built["detail"], "on/customers.yml");
    let edited = format!("{}# à mão\n", built["cubes"][1]["content"].as_str().unwrap());
    let (status, body) = call(&env, Method::PUT, &token, &format!("/cube/v1/organizations/{on}/model/files/{file}"),
        Some(json!({ "content": edited, "version": version }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    save_def(&env, &token, &id, cubes_def(&[table_cube("c1", "orders", "orders")]), 2).await;
    let (status, body) = build(&env, &token, &id, json!({})).await;
    assert_eq!((status, &body["error"]), (StatusCode::CONFLICT, &json!("O cubo foi editado desde o último build")), "{body}");
    assert_eq!(org_paths(&store, on).await, vec!["customers.yml", "orders.yml"]);
    let (status, body) = build(&env, &token, &id, json!({ "force": true })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["removed"], json!(["on/customers.yml"]));
    assert_eq!(org_paths(&store, on).await, vec!["orders.yml"]);
});

build_test!(a_cube_named_like_a_foreign_cube_is_refused, |store, url| {
    shop_tables(&url).await;
    let (env, token, on) = models_env(&store).await;
    env.writer.clone().unwrap()
        .write("ann", "seed", |_| Ok(vec![put_in(Some(on), "x.yml", &cube_on("orders", "sofiadb"), None)]))
        .await
        .unwrap();
    let id = new_model(&env, &token, on, "Vendas").await;
    save_def(&env, &token, &id, two_cubes(), 1).await;
    for force in [false, true] {
        let (status, body) = build(&env, &token, &id, json!({ "force": force })).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["error"], "O cubo orders já existe em on/x.yml, fora deste model");
        assert_eq!(org_paths(&store, on).await, vec!["x.yml"]);
    }
});

build_test!(a_legacy_model_with_a_long_name_builds_and_saves, |store, url| {
    shop_tables(&url).await;
    let (env, token, on) = models_env(&store).await;
    let long = "Vendas Mensais Consolidadas Por Regiao E Por Cliente Do Ano Fiscal Corrente";
    let cube = "vendas_mensais_consolidadas_por_regiao_e_por_cliente_do_ano_fiscal_corrente";
    assert!(cube.len() > 63);
    let id = new_model(&env, &token, on, long).await;

    let (status, built) = build(&env, &token, &id, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{built}");
    assert_eq!(built["cubes"][0]["cubeName"], cube);
    assert_eq!(built["cubes"][0]["fileName"], format!("on/{cube}.yml"));
    // Saved as it opens, converted: still valid, and it builds the same.
    let (_, one) = call(&env, Method::GET, &token, &format!("/cube/v1/cube-models/{id}"), None).await;
    assert_eq!(one["definition"]["cubes"][0]["name"], cube, "{one}");
    save_def(&env, &token, &id, one["definition"].clone(), 1).await;
    let (status, again) = build(&env, &token, &id, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["cubes"][0]["content"], built["cubes"][0]["content"]);
    assert_eq!(org_paths(&store, on).await, vec![format!("{cube}.yml")]);
});

build_test!(a_removed_cubes_name_on_a_new_cube_reuses_its_file, |store, url| {
    shop_tables(&url).await;
    let (env, token, on) = models_env(&store).await;
    let id = new_model(&env, &token, on, "Vendas").await;
    save_def(&env, &token, &id, two_cubes(), 1).await;
    let (status, built) = build(&env, &token, &id, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{built}");
    let (file, _) = file_in(&built["detail"], "on/customers.yml");

    // c2 removed, and a new cube c3 takes its name.
    save_def(&env, &token, &id,
        cubes_def(&[table_cube("c1", "orders", "orders"), table_cube("c3", "customers", "customers")]), 2).await;
    let (status, built) = build(&env, &token, &id, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{built}");
    assert_eq!(built["removed"], json!([]));
    assert_eq!(built["cubes"][1]["fileName"], "on/customers.yml");
    assert_eq!(file_in(&built["detail"], "on/customers.yml").0, file);
    assert_eq!(org_paths(&store, on).await, vec!["customers.yml", "orders.yml"]);
});

build_test!(a_new_cube_on_a_renamed_cubes_file_is_refused, |store, url| {
    shop_tables(&url).await;
    let (env, token, on) = models_env(&store).await;
    let id = new_model(&env, &token, on, "Vendas").await;
    save_def(&env, &token, &id, two_cubes(), 1).await;
    assert_eq!(build(&env, &token, &id, json!({})).await.0, StatusCode::OK);

    // c1 renamed to sales stays in orders.yml; a new cube orders wants it.
    save_def(&env, &token, &id, cubes_def(&[
        table_cube("c1", "sales", "orders"),
        table_cube("c2", "customers", "customers"),
        table_cube("c3", "orders", "orders"),
    ]), 2).await;
    let (status, body) = build(&env, &token, &id, json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "O arquivo orders.yml é do cubo sales deste model; renomeie um dos cubos");
    assert_eq!(org_paths(&store, on).await, vec!["customers.yml", "orders.yml"]);
});

build_test!(an_old_build_record_moves_to_cube_model_cubes, |store, url| {
    shop_tables(&url).await;
    let (env, token, on) = models_env(&store).await;
    let id = new_model(&env, &token, on, "Vendas").await;
    let (status, built) = build(&env, &token, &id, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{built}");
    let model: uuid::Uuid = id.parse().unwrap();

    // As the old build left it: the record on the model, none per cube.
    let db = support::connect(&url).await;
    let row = db.query_one("SELECT file_id, built_hash FROM cube_model_cubes WHERE model_id = $1", &[&model]).await.unwrap();
    let (file, hash): (uuid::Uuid, String) = (row.get(0), row.get(1));
    db.execute("DELETE FROM cube_model_cubes WHERE model_id = $1", &[&model]).await.unwrap();
    db.execute("UPDATE cube_models SET cube_name = 'vendas', cube_file_id = $2, cube_built_hash = $3 WHERE id = $1",
        &[&model, &file, &hash]).await.unwrap();

    // The migration runs again, as on the next start.
    cubeserver::rbac::store::Store::connect_with_pool_size(&url, 1).await.unwrap();
    let (status, body) = build(&env, &token, &id, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["cubes"][0]["fileName"], "on/vendas.yml");
    assert_eq!(org_paths(&store, on).await, vec!["vendas.yml"]);
    let row = db.query_one("SELECT cube_file_id, cube_built_hash FROM cube_models WHERE id = $1", &[&model]).await.unwrap();
    assert_eq!((row.get::<_, Option<uuid::Uuid>>(0), row.get::<_, Option<String>>(1)), (None, None));
    let row = db.query_one("SELECT file_id FROM cube_model_cubes WHERE model_id = $1 AND cube_id = 'c1'", &[&model]).await.unwrap();
    assert_eq!(row.get::<_, uuid::Uuid>(0), file);
});

/// `model_def` with a draft table `shop.<table>` (id, name) added.
fn draft_def(table: &str) -> Value {
    let mut def = model_def("sofiadb");
    def["tables"].as_array_mut().unwrap().push(json!({
        "alias": "items", "schema": "shop", "table": table, "x": 480, "y": 0,
        "draft": { "columns": [
            { "name": "id", "type": "integer", "nullable": false, "primaryKey": true },
            { "name": "order", "type": "text", "nullable": true, "primaryKey": false },
        ] },
    }));
    def["joins"].as_array_mut().unwrap().push(json!({
        "from": { "alias": "orders", "column": "id" },
        "to": { "alias": "items", "column": "id" },
        "relationship": "many_to_one",
        "join": "left",
    }));
    def
}

async fn new_draft_model(env: &Env, token: &str, org: uuid::Uuid, table: &str) -> String {
    let (status, body) = call(env, Method::POST, token, &format!("/cube/v1/organizations/{org}/cube-models"),
        Some(json!({ "name": "Rascunho", "definition": draft_def(table) }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().unwrap().to_string()
}

async fn create_tables(env: &Env, token: &str, id: &str, body: Value) -> (StatusCode, Value) {
    call(env, Method::POST, token, &format!("/cube/v1/cube-models/{id}/create-tables"), Some(body)).await
}

async fn table_exists(url: &str, table: &str) -> bool {
    !support::connect(url)
        .await
        .query("SELECT 1 FROM information_schema.tables WHERE table_schema = 'shop' AND table_name = $1", &[&table])
        .await
        .unwrap()
        .is_empty()
}

build_test!(create_tables_dry_run_lists_statements, |store, url| {
    shop_tables(&url).await;
    let (env, token, on) = models_env(&store).await;
    let id = new_draft_model(&env, &token, on, "items").await;
    let (status, body) = create_tables(&env, &token, &id, json!({ "version": 1, "dryRun": true })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["statements"], json!([
        "CREATE TABLE \"shop\".\"items\" (\"id\" integer NOT NULL, \"order\" text, PRIMARY KEY (\"id\"))"
    ]));
    assert!(!table_exists(&url, "items").await);
    let (status, body) = create_tables(&env, &token, &id, json!({ "version": 7, "dryRun": true })).await;
    assert_eq!((status, &body["error"]), (StatusCode::CONFLICT, &json!(STALE_MODEL)), "{body}");
    let plain = new_model(&env, &token, on, "Sem rascunho").await;
    let (status, body) = create_tables(&env, &token, &plain, json!({ "version": 1 })).await;
    assert_eq!((status, &body["error"]), (StatusCode::BAD_REQUEST, &json!("Não há tabelas novas para criar")), "{body}");
});

build_test!(create_tables_creates_and_clears_drafts, |store, url| {
    shop_tables(&url).await;
    let db = support::connect(&url).await;
    db.batch_execute("DROP TABLE IF EXISTS shop.items").await.unwrap();
    let (env, _, on) = models_env(&store).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let id = new_draft_model(&env, &admin, on, "items").await;

    let (status, body) = build(&env, &admin, &id, json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "A tabela shop.items ainda não existe no banco: use Criar no banco");

    let (status, body) = create_tables(&env, &admin, &id, json!({ "version": 1 })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["created"], json!(["shop.items"]));
    assert_eq!(body["error"], Value::Null);
    assert_eq!(body["model"]["version"], 2);
    assert!(body["detail"].is_object(), "{body}");
    assert!(body["model"]["definition"]["tables"][2].get("draft").is_none(), "{body}");
    assert!(table_exists(&url, "items").await);
    let (_, one) = call(&env, Method::GET, &admin, &format!("/cube/v1/cube-models/{id}"), None).await;
    assert!(one["definition"]["tables"][2].get("draft").is_none(), "{one}");
    assert_eq!(one["version"], 2);

    let (status, body) = build(&env, &admin, &id, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    db.batch_execute("DROP TABLE IF EXISTS shop.items").await.unwrap();
});

build_test!(create_tables_refuses_an_existing_table, |store, url| {
    shop_tables(&url).await;
    let (env, _, on) = models_env(&store).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let mut def = draft_def("items");
    def["tables"][2]["table"] = json!("orders");
    let (status, body) = call(&env, Method::POST, &admin, &format!("/cube/v1/organizations/{on}/cube-models"),
        Some(json!({ "name": "Duplicado", "definition": def }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["id"].as_str().unwrap();
    let (status, body) = create_tables(&env, &admin, id, json!({ "version": 1 })).await;
    assert_eq!((status, &body["error"]), (StatusCode::CONFLICT, &json!("A tabela shop.orders já existe no banco")), "{body}");
    let (_, one) = call(&env, Method::GET, &admin, &format!("/cube/v1/cube-models/{id}"), None).await;
    assert_eq!(one["version"], 1);
    assert!(one["definition"]["tables"][2].get("draft").is_some(), "{one}");
});

build_test!(create_tables_needs_execute, |store, url| {
    shop_tables(&url).await;
    support::connect(&url).await.batch_execute("DROP TABLE IF EXISTS shop.items").await.unwrap();
    let (env, token, on) = models_env(&store).await;
    let id = new_draft_model(&env, &token, on, "items").await;
    // Olga owns the organization but has no workspace with `execute`.
    let (status, body) = create_tables(&env, &token, &id, json!({ "version": 1 })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(!table_exists(&url, "items").await);
    let (_, one) = call(&env, Method::GET, &token, &format!("/cube/v1/cube-models/{id}"), None).await;
    assert_eq!(one["version"], 1);
    // The dry run needs no execute.
    assert_eq!(create_tables(&env, &token, &id, json!({ "version": 1, "dryRun": true })).await.0, StatusCode::OK);
    // A user of no organization: as if the model did not exist.
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let (other, _, _) = owner_org(&env, &store, &admin, "paula", "Beta").await;
    assert_eq!(create_tables(&env, &other, &id, json!({ "version": 1 })).await.0, StatusCode::NOT_FOUND);
});

build_test!(create_tables_partial_failure_keeps_the_first, |store, url| {
    shop_tables(&url).await;
    let db = support::connect(&url).await;
    db.batch_execute("DROP TABLE IF EXISTS shop.items").await.unwrap();
    let (env, _, on) = models_env(&store).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let mut def = draft_def("items");
    def["tables"].as_array_mut().unwrap().push(json!({
        "alias": "ghosts", "schema": "nowhere", "table": "ghosts", "x": 0, "y": 80,
        "draft": { "columns": [{ "name": "id", "type": "integer", "nullable": false, "primaryKey": true }] },
    }));
    def["joins"].as_array_mut().unwrap().push(json!({
        "from": { "alias": "orders", "column": "id" },
        "to": { "alias": "ghosts", "column": "id" },
        "relationship": "many_to_one", "join": "left",
    }));
    let (status, body) = call(&env, Method::POST, &admin, &format!("/cube/v1/organizations/{on}/cube-models"),
        Some(json!({ "name": "Parcial", "definition": def }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["id"].as_str().unwrap().to_string();

    let (status, body) = create_tables(&env, &admin, &id, json!({ "version": 1 })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["created"], json!(["shop.items"]));
    assert!(body["error"].as_str().unwrap().contains("nowhere.ghosts"), "{body}");
    assert_eq!(body["model"]["version"], 2);
    let tables = &body["model"]["definition"]["tables"];
    assert!(tables[2].get("draft").is_none(), "{body}");
    assert!(tables[3].get("draft").is_some(), "{body}");
    assert!(table_exists(&url, "items").await);
    db.batch_execute("DROP TABLE IF EXISTS shop.items").await.unwrap();
});

/// Like `call`, with extra headers.
async fn call_h(env: &Env, method: Method, token: &str, path: &str, body: Option<Value>, lock: Option<&str>) -> (StatusCode, Value) {
    use axum::body::Body;
    use axum::http::{header, Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "localhost:4000")
        .header(
            cubeserver::handlers::playground::PLAYGROUND_KEY_HEADER,
            cubeserver::handlers::playground::playground_key(),
        )
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    if let Some(lock) = lock {
        request = request.header("X-Model-Lock", lock);
    }
    let request = match body {
        Some(body) => request.header(header::CONTENT_TYPE, "application/json").body(Body::from(body.to_string())),
        None => request.body(Body::empty()),
    }
    .unwrap();
    let response = env.router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

api_test!(model_edit_lock, |store| {
    let (env, token_a, on) = models_env(&store).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    store
        .create_user(&cubeserver::rbac::store::NewUser {
            username: "bia".into(),
            name: None,
            email: None,
            password_hash: cubeserver::rbac::identity::hash_password("owner-password-1").unwrap(),
        })
        .await
        .unwrap();
    let (status, body) = call(&env, Method::PUT, &admin, &format!("/cube/v1/organizations/{on}/owners/bia"), None).await;
    assert!(status.is_success(), "{body}");
    let token_b = login(&env, "bia", "owner-password-1").await;
    let id = new_model(&env, &token_a, on, "Vendas").await;
    let one = format!("/cube/v1/cube-models/{id}");
    let lock = format!("{one}/lock");
    let (status, body) = call(&env, Method::POST, &token_a, &lock, Some(json!({ "token": "a1" }))).await;
    assert_eq!((status, &body), (StatusCode::OK, &json!({ "held": true, "holder": "olga" })));
    let (status, body) = call(&env, Method::POST, &token_b, &lock, Some(json!({ "token": "b1" }))).await;
    assert_eq!((status, &body), (StatusCode::OK, &json!({ "held": false, "holder": "olga" })));

    let msg = "O model está sendo editado por olga";
    let upd = |v: i64| Some(json!({ "name": "X", "version": v }));
    let (status, body) = call_h(&env, Method::PUT, &token_b, &one, upd(1), Some("b1")).await;
    assert_eq!(status, StatusCode::LOCKED, "{body}");
    assert_eq!(body["error"], msg);
    let (status, _) = call_h(&env, Method::PUT, &token_b, &one, upd(1), None).await;
    assert_eq!(status, StatusCode::LOCKED);
    let (status, body) = call_h(&env, Method::PUT, &token_a, &one, upd(1), Some("a1")).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = call(&env, Method::POST, &token_b, &format!("{lock}/take"), Some(json!({ "token": "b1" }))).await;
    assert_eq!((status, &body), (StatusCode::OK, &json!({ "held": true, "holder": "bia" })));
    let (status, _) = call_h(&env, Method::PUT, &token_a, &one, upd(2), Some("a1")).await;
    assert_eq!(status, StatusCode::LOCKED);
    let (status, body) = call_h(&env, Method::PUT, &token_b, &one, upd(2), Some("b1")).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // A, no longer the holder, is refused by build and create-tables.
    let (status, _) = call_h(&env, Method::POST, &token_a, &format!("{one}/build"), Some(json!({})), Some("a1")).await;
    assert_eq!(status, StatusCode::LOCKED);
    let (status, _) = call_h(&env, Method::POST, &token_a, &format!("{one}/create-tables"),
        Some(json!({ "version": 3, "dryRun": true })), Some("a1")).await;
    assert_eq!(status, StatusCode::LOCKED);
    // Nor may A delete it.
    let (status, body) = call_h(&env, Method::DELETE, &token_a, &format!("{one}?version=3"), None, Some("a1")).await;
    assert_eq!(status, StatusCode::LOCKED, "{body}");
    assert_eq!(body["error"], "O model está sendo editado por bia");

    let (status, _) = call(&env, Method::DELETE, &token_b, &format!("{lock}?token=b1"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = call_h(&env, Method::PUT, &token_a, &one, upd(3), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
});
