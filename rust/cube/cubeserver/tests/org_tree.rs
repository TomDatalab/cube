//! The Playground's organization tree (`/playground/tree*`) with RBAC on.
//!
//! The ownership policy is process-global, so these tests live in a binary of
//! their own, run one at a time, and clear the policy when they end. They need
//! `CUBE_TEST_PG_URL` (see `support`).

mod support;

use axum::http::{Method, StatusCode};
use cubemodel::ownership::set_policy;
use serde_json::{json, Value};
use support::TestDb;
use std::collections::BTreeSet;
use std::sync::Arc;

#[path = "support/tree_env.rs"]
mod tree_env;
use tree_env::*;


/// Runs `body` against a fresh database, alone, and always drops it and
/// clears the policy.
macro_rules! tree_test {
    ($name:ident, |$store:ident| $body:block) => {
        #[tokio::test]
        async fn $name() {
            let _serial = SERIAL.lock().await;
            let _clear = ClearPolicy;
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

const CUBE_A: &str = "cubes:\n  - name: a\n    sql_table: t\n    data_source: sofiadb\n    measures:\n      - name: n\n        type: count\n    dimensions:\n      - name: id\n        sql: id\n        type: number\n";
const CUBE_B: &str = "cubes:\n  - name: b\n    sql_table: t\n    data_source: sofiadb\n    measures:\n      - name: n\n        type: count\n";
const CUBE_U: &str = "cubes:\n  - name: u\n    sql_table: t\n    data_source: default\n    measures:\n      - name: n\n        type: count\n";
const CUBE_ORDERS: &str = "cubes:\n  - name: orders\n    sql_table: t\n    data_source: default\n    measures:\n      - name: n\n        type: count\n";

tree_test!(tree_is_for_platform_admins, |store| {
    let env = env(store).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    let slug = create_org(&env, &admin, "Digital").await;
    let (status, body, _) = send(
        &env.router,
        Method::POST,
        "/cube/v1/users",
        Some(&admin),
        Some(json!({ "id": "ann", "name": "Ann", "email": "a@example.org",
                     "password": "ann-password-1" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let ann = login(&env, "ann", "ann-password-1").await;

    for path in ["/playground/tree".to_string(), format!("/playground/tree/{slug}")] {
        assert_eq!(get(&env, &ann, &path).await.0, StatusCode::FORBIDDEN, "{path}");
        let (status, _, _) = send(&env.router, Method::GET, &path, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
    }
});

tree_test!(tree_groups_by_folder, |store| {
    let env = env(store).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    assert_eq!(create_org(&env, &admin, "Digital").await, "digital");
    assert_eq!(create_org(&env, &admin, "ON").await, "on");
    env.write("digital/a.yml", CUBE_A);
    env.write("on/b.yml", CUBE_B);
    env.write("_platform/u.yml", CUBE_U);
    env.write("orders.yml", CUBE_ORDERS);
    install_policy(&[("digital", &["sofiadb"]), ("on", &[])], &["default"]);

    let (status, body, _) = get(&env, &admin, "/playground/tree").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let orgs = body["organizations"].as_array().unwrap();
    let by = |slug: &str| orgs.iter().find(|o| o["slug"] == slug).unwrap().clone();
    assert_eq!(by("digital")["counts"]["cubes"], 1, "{body}");
    assert_eq!(by("digital")["counts"]["files"], 1);
    assert_eq!(by("digital")["counts"]["connections"], 1);
    assert_eq!(by("digital")["errors"], 0);
    assert_eq!(by("on")["counts"]["cubes"], 0);
    assert_eq!(by("on")["errors"], 1);
    assert_eq!(by("_platform")["name"], "Platform");
    assert_eq!(by("_platform")["counts"]["cubes"], 1);
    // The root-level file is shown under `_platform`, with its rejection.
    assert_eq!(by("_platform")["counts"]["files"], 2);
    assert_eq!(by("_platform")["errors"], 1);
    // `_platform` closes the list and is not counted.
    assert_eq!(orgs.last().unwrap()["slug"], "_platform");
    assert_eq!(body["total"], 2);

    let (status, body, _) = get(&env, &admin, "/playground/tree/on").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["errors"][0]["message"],
        "data source sofiadb is not a connection of on"
    );
    assert_eq!(body["files"], json!([{ "fileName": "on/b.yml" }]));
    assert_eq!(body["cubes"], json!([]));

    let (status, body, _) = get(&env, &admin, "/playground/tree/_platform").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let files: Vec<&str> = body["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["fileName"].as_str().unwrap())
        .collect();
    assert!(files.contains(&"orders.yml") && files.contains(&"_platform/u.yml"), "{body}");
    assert!(
        body["errors"].as_array().unwrap().iter().any(|e| e["file"] == "orders.yml"
            && e["message"] == "the folder / does not belong to any organization"),
        "{body}"
    );

    let (_, body, _) = get(&env, &admin, "/playground/tree/digital").await;
    assert_eq!(body["cubes"][0]["name"], "a");
    assert_eq!(body["cubes"][0]["type"], "cube");
    assert_eq!(body["cubes"][0]["dataSource"], "sofiadb");
    assert_eq!(body["cubes"][0]["file"], "digital/a.yml");
    assert_eq!(body["cubes"][0]["members"], json!({ "measures": 1, "dimensions": 1 }));

    let (status, body, _) = get(&env, &admin, "/playground/tree/digital/files/digital/a.yml").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["fileName"], "digital/a.yml");
    assert_eq!(body["content"], CUBE_A);
    assert!(body["absPath"].as_str().unwrap().ends_with("digital/a.yml"));
});

tree_test!(tree_search_and_paging, |store| {
    let env = env(store).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    for name in ["Digital", "ON", "Zeta"] {
        create_org(&env, &admin, name).await;
    }
    install_policy(&[("digital", &[]), ("on", &[]), ("zeta", &[])], &[]);

    let (_, body, _) = get(&env, &admin, "/playground/tree?q=dig").await;
    let slugs: Vec<&str> = body["organizations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["slug"].as_str().unwrap())
        .collect();
    assert_eq!(slugs, ["digital"], "{body}");

    let (_, body, _) = get(&env, &admin, "/playground/tree?limit=1&offset=1").await;
    assert_eq!(body["organizations"].as_array().unwrap().len(), 1, "{body}");
    assert_eq!(body["organizations"][0]["slug"], "on");
    assert_eq!(body["total"], 3);

    // The limit stops at 200 and the request still answers.
    let (status, body, _) = get(&env, &admin, "/playground/tree?limit=100000").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 3);
});

tree_test!(tree_unknown_slug_is_404, |store| {
    let env = env(store).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    for path in [
        "/playground/tree/nope",
        "/playground/tree/nope/files/nope/a.yml",
    ] {
        assert_eq!(get(&env, &admin, path).await.0, StatusCode::NOT_FOUND, "{path}");
    }
});

tree_test!(file_paths_cannot_escape, |store| {
    let env = env(store).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    create_org(&env, &admin, "Digital").await;
    create_org(&env, &admin, "ON").await;
    env.write("digital/a.yml", CUBE_A);
    env.write("on/b.yml", "SECRET_B_CONTENT: 1\n");
    std::os::unix::fs::symlink("../on/b.yml", env.model.path().join("digital/link.yml")).unwrap();
    install_policy(&[("digital", &["sofiadb"]), ("on", &[])], &[]);

    for path in [
        "files/../on/b.yml",
        "files/%2e%2e/on/b.yml",
        "files//etc/passwd",
        "files/on/b.yml",
        "files/digital/link.yml",
        "files/digital/../on/b.yml",
        "files/digital/a.txt",
    ] {
        let (status, _, text) = get(&env, &admin, &format!("/playground/tree/digital/{path}")).await;
        assert!(
            status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND,
            "{path}: {status}"
        );
        assert!(!text.contains("SECRET_B_CONTENT"), "{path}: {text}");
    }
});

tree_test!(policy_follows_connections, |store| {
    use cubeserver::rbac::store::ConnectionOrigin;
    let env = env(store.clone()).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    create_org(&env, &admin, "Digital").await;
    create_org(&env, &admin, "ON").await;
    let digital = store.organization_by_slug("digital").await.unwrap().unwrap();
    store
        .create_connection("sofiadb", digital.id, ConnectionOrigin::Assigned)
        .await
        .unwrap();

    let policy = cubeserver::rbac::ownership::build_policy(
        &store,
        &["default".to_string(), "sofiadb".to_string()],
    )
    .await
    .unwrap();
    let set = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<BTreeSet<_>>();
    assert_eq!(policy.organizations["digital"], set(&["sofiadb"]));
    assert_eq!(policy.organizations["on"], set(&[]));
    assert_eq!(policy.platform, set(&["default"]));

    // Every installed policy is a new generation.
    set_policy(Some(Arc::new(policy)));
    let first = cubemodel::ownership::current_policy().unwrap().generation;
    let again = cubeserver::rbac::ownership::build_policy(&store, &[]).await.unwrap();
    set_policy(Some(Arc::new(again)));
    assert!(cubemodel::ownership::current_policy().unwrap().generation > first);
});

tree_test!(moving_a_connection_rejects_its_cubes_without_restart, |store| {
    use cubeserver::rbac::store::ConnectionOrigin;
    let env = env_with(store.clone(), true).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    create_org(&env, &admin, "Digital").await;
    create_org(&env, &admin, "ON").await;
    let digital = store.organization_by_slug("digital").await.unwrap().unwrap();
    let on = store.organization_by_slug("on").await.unwrap().unwrap();
    store
        .create_connection("sofiadb", digital.id, ConnectionOrigin::Assigned)
        .await
        .unwrap();
    env.rbac.set_data_source_names(vec!["default".into(), "sofiadb".into()]);
    env.write("digital/a.yml", CUBE_A);
    cubeserver::rbac::ownership::refresh_policy(&env.state).await.unwrap();
    let cubes = |body: &Value| -> Vec<String> {
        body["cubes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap().to_string())
            .collect()
    };

    let (status, body, _) = get(&env, &admin, "/cube/v1/meta").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(cubes(&body).contains(&"a".to_string()), "{body}");

    let (status, _, text) = send(
        &env.router,
        Method::PUT,
        "/cube/v1/connections/sofiadb/organization",
        Some(&admin),
        Some(json!({ "organizationId": on.id })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{text}");

    let (status, body, _) = get(&env, &admin, "/playground/tree/digital").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["errors"][0]["message"],
        "data source sofiadb is not a connection of digital"
    );
    let (status, body, _) = get(&env, &admin, "/cube/v1/meta").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!cubes(&body).contains(&"a".to_string()), "{body}");
});

fn generate_body(tables: Value) -> Value {
    let cols = json!([{ "name": "id", "type": "String", "attributes": ["primaryKey"] }]);
    json!({
        "format": "yaml",
        "tables": tables,
        "tablesSchema": {
            "sofiadb.public": { "orders": cols },
            "public": { "users": cols }
        }
    })
}

tree_test!(generated_cubes_go_to_the_owners_folder, |store| {
    let env = env(store).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    install_policy(&[("digital", &["sofiadb"])], &["default"]);

    let (status, body, _) = send(
        &env.router,
        Method::POST,
        "/playground/generate-schema",
        Some(&admin),
        Some(generate_body(json!([["sofiadb.public", "orders"]]))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["files"][0]["fileName"], "digital/public_orders.yml");
    assert!(env.model.path().join("digital/public_orders.yml").exists());
    assert!(!env.model.path().join("public_orders.yml").exists());

    let (status, body, _) = send(
        &env.router,
        Method::POST,
        "/playground/generate-schema",
        Some(&admin),
        Some(generate_body(json!([["public", "users"]]))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(env.model.path().join("_platform/users.yml").exists());
});

tree_test!(generate_refuses_mixed_owners, |store| {
    let env = env(store).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    install_policy(&[("digital", &["sofiadb"])], &["default"]);

    let (status, body, _) = send(
        &env.router,
        Method::POST,
        "/playground/generate-schema",
        Some(&admin),
        Some(generate_body(json!([["public", "users"], ["sofiadb.public", "orders"]]))),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "The tables belong to more than one organization");
    assert_eq!(std::fs::read_dir(env.model.path()).unwrap().count(), 0);
});

tree_test!(files_carry_their_organization, |store| {
    let env = env(store).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    install_policy(&[("digital", &["sofiadb"])], &["default"]);
    env.write("digital/a.yml", CUBE_A);
    env.write("_platform/u.yml", CUBE_U);
    env.write("stray/x.yml", CUBE_B);

    let (status, body, _) = get(&env, &admin, "/playground/files").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let org = |name: &str| {
        body["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["fileName"] == name)
            .unwrap()["organization"]
            .clone()
    };
    assert_eq!(org("digital/a.yml"), "digital");
    assert_eq!(org("_platform/u.yml"), "_platform");
    assert_eq!(org("stray/x.yml"), Value::Null);
});

tree_test!(meta_groups_split_the_visible_cubes_by_organization, |store| {
    let (env, admin, digital, on) = two_orgs(&store).await;
    env.writer
        .clone()
        .unwrap()
        .write("admin", "seed", |_| {
            Ok(vec![
                put_in(None, "u.yml", CUBE_U),
                put_in(Some(digital), "a.yml", CUBE_A),
                put_in(Some(on), "b.yml", &titled("b", "B", "ondb")),
            ])
        })
        .await
        .unwrap();
    // ondb is no longer ON's: its cube is rejected, counted on ON's group.
    assign(&env, &admin, "ondb", digital).await;

    let (status, _, _) = send(&env.router, Method::GET, "/cube/v1/meta/groups", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let platform = json!({ "slug": "_platform", "id": null, "name": "Platform", "errors": 0,
                           "dataSources": [{ "name": "default", "cubes": ["u"] }] });
    let groups = |org: Option<&'static str>| {
        let (env, admin) = (&env, admin.clone());
        async move {
            let headers: Vec<(&str, &str)> =
                org.map(|o| ("x-cube-organization", o)).into_iter().collect();
            let (status, body, _) = get_with(env, &admin, &headers, "/cube/v1/meta/groups").await;
            assert_eq!(status, StatusCode::OK, "{org:?}: {body}");
            body
        }
    };
    // Each organization's groups: the Platform's and its own.
    assert_eq!(
        groups(Some("digital")).await,
        json!({ "groups": [
            platform,
            { "slug": "digital", "id": digital, "name": "Digital", "errors": 0,
              "dataSources": [{ "name": "sofiadb", "cubes": ["a"] }] },
        ] })
    );
    assert_eq!(
        groups(Some("on")).await,
        json!({ "groups": [
            platform,
            { "slug": "on", "id": on, "name": "ON", "errors": 1, "dataSources": [] },
        ] })
    );
    // Without a header the admin is served the Platform's model.
    assert_eq!(groups(None).await, json!({ "groups": [platform] }));
});

tree_test!(meta_groups_show_a_member_only_their_workspace, |store| {
    let env = env_with(store.clone(), true).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    create_org(&env, &admin, "Digital").await;
    create_org(&env, &admin, "ON").await;
    let digital = store.organization_by_slug("digital").await.unwrap().unwrap();
    env.rbac.set_data_source_names(vec!["default".into(), "sofiadb".into()]);
    let (status, _, text) = send(
        &env.router,
        Method::PUT,
        "/cube/v1/connections/sofiadb/organization",
        Some(&admin),
        Some(json!({ "organizationId": digital.id })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{text}");
    env.write("digital/a.yml", CUBE_A);
    env.write("_platform/u.yml", CUBE_U);
    env.write("on/b.yml", CUBE_B);
    cubeserver::rbac::ownership::refresh_policy(&env.state).await.unwrap();

    let (status, body, _) = send(
        &env.router,
        Method::POST,
        "/cube/v1/users",
        Some(&admin),
        Some(json!({ "id": "ann", "name": "Ann", "email": "a@example.org",
                     "password": "ann-password-1" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body, _) = send(
        &env.router,
        Method::POST,
        &format!("/cube/v1/organizations/{}/workspaces", digital.id),
        Some(&admin),
        Some(json!({ "name": "Ops" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let ws = body["id"].as_str().unwrap().to_string();
    for (method, path, payload) in [
        (Method::PUT, format!("/cube/v1/workspaces/{ws}/members/ann"), Some(json!({ "role": "viewer" }))),
        (Method::PUT, format!("/cube/v1/workspaces/{ws}/connections/sofiadb"), None),
    ] {
        let (status, _, text) = send(&env.router, method, &path, Some(&admin), payload).await;
        assert!(status.is_success(), "{path}: {status} {text}");
    }
    let ann = login(&env, "ann", "ann-password-1").await;

    let (meta_status, meta, _) = get_in_workspace(&env, &ann, &ws, "/cube/v1/meta").await;
    assert_eq!(meta_status, StatusCode::OK, "{meta}");
    let (status, body, _) = get_in_workspace(&env, &ann, &ws, "/cube/v1/meta/groups").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Only what the workspace's meta shows, and no error counts.
    assert_eq!(
        body,
        json!({ "groups": [
            { "slug": "digital", "id": digital.id, "name": "Digital", "errors": null,
              "dataSources": [{ "name": "sofiadb", "cubes": ["a"] }] },
        ] }),
        "meta: {meta}"
    );

    // Without a workspace header: whatever `/meta` answers, the groups hold
    // the same cubes.
    let (status, body, _) = get(&env, &ann, "/cube/v1/meta/groups").await;
    let (meta_status, meta, _) = get(&env, &ann, "/cube/v1/meta").await;
    assert_eq!(status, meta_status, "{body}");
    if status.is_success() {
        let mut shown: Vec<&str> = meta["cubes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        let mut grouped: Vec<&str> = body["groups"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|g| g["dataSources"].as_array().unwrap())
            .flat_map(|d| d["cubes"].as_array().unwrap())
            .map(|c| c.as_str().unwrap())
            .collect();
        shown.sort();
        grouped.sort();
        assert_eq!(grouped, shown);
        assert!(body["groups"].as_array().unwrap().iter().all(|g| g["errors"].is_null()));
    }
});

/// `GET path` as `token` with extra headers.
async fn get_with(
    env: &Env,
    token: &str,
    headers: &[(&str, &str)],
    path: &str,
) -> (StatusCode, Value, String) {
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let mut request = axum::http::Request::builder()
        .method(Method::GET)
        .uri(path)
        .header(axum::http::header::HOST, "localhost:4000")
        .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"));
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = env.router.clone().oneshot(request.body(Body::empty()).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    (status, serde_json::from_str(&text).unwrap_or(Value::Null), text)
}

fn titled(name: &str, title: &str, data_source: &str) -> String {
    format!(
        "cubes:\n  - name: {name}\n    title: {title}\n    sql_table: t\n    data_source: {data_source}\n    measures:\n      - name: n\n        type: count\n"
    )
}

fn put_in(org: Option<uuid::Uuid>, path: &str, content: &str) -> cubeserver::rbac::model_store::ModelChange {
    cubeserver::rbac::model_store::ModelChange::Put {
        id: None,
        organization_id: org,
        path: path.into(),
        content: content.into(),
        expected_version: None,
        managed_by: None,
    }
}

/// The titles of the cubes named `name` in a meta answer.
fn titles_of(meta: &Value, name: &str) -> Vec<String> {
    meta["cubes"]
        .as_array()
        .unwrap_or_else(|| panic!("{meta}"))
        .iter()
        .filter(|c| c["name"] == name)
        .map(|c| c["title"].as_str().unwrap_or_default().to_string())
        .collect()
}

async fn ok(env: &Env, admin: &str, method: Method, path: &str, body: Option<Value>) -> Value {
    let (status, body, text) = send(&env.router, method, path, Some(admin), body).await;
    assert!(status.is_success(), "{path}: {status} {text}");
    body
}

/// A workspace of `org` with `connection` released to it and `members`.
async fn workspace_with(
    env: &Env,
    admin: &str,
    org: uuid::Uuid,
    connection: &str,
    members: &[&str],
) -> String {
    let body = ok(
        env,
        admin,
        Method::POST,
        &format!("/cube/v1/organizations/{org}/workspaces"),
        Some(json!({ "name": "Ops" })),
    )
    .await;
    let ws = body["id"].as_str().unwrap().to_string();
    ok(env, admin, Method::PUT, &format!("/cube/v1/workspaces/{ws}/connections/{connection}"), None).await;
    for member in members {
        ok(
            env,
            admin,
            Method::PUT,
            &format!("/cube/v1/workspaces/{ws}/members/{member}"),
            Some(json!({ "role": "viewer" })),
        )
        .await;
    }
    ws
}

async fn user(env: &Env, admin: &str, id: &str) -> String {
    let password = format!("{id}-password-1");
    ok(
        env,
        admin,
        Method::POST,
        "/cube/v1/users",
        Some(json!({ "id": id, "name": id, "email": format!("{id}@example.org"),
                     "password": password })),
    )
    .await;
    login(env, id, &password).await
}

async fn assign(env: &Env, admin: &str, connection: &str, org: uuid::Uuid) {
    ok(
        env,
        admin,
        Method::PUT,
        &format!("/cube/v1/connections/{connection}/organization"),
        Some(json!({ "organizationId": org })),
    )
    .await;
}

/// Digital (connection `sofiadb`) and ON (connection `ondb`), with their
/// ids, on a model served from the database.
async fn two_orgs(store: &cubeserver::rbac::store::Store) -> (Env, String, uuid::Uuid, uuid::Uuid) {
    let mut env = env_live(store.clone()).await;
    let mut config = (*env.state.config).clone();
    config.data_sources.push(("ondb".into(), "postgres".into()));
    env.state.config = Arc::new(config);
    env.router = cubeserver::build_app(env.state.clone());
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    create_org(&env, &admin, "Digital").await;
    create_org(&env, &admin, "ON").await;
    let digital = store.organization_by_slug("digital").await.unwrap().unwrap().id;
    let on = store.organization_by_slug("on").await.unwrap().unwrap().id;
    env.rbac
        .set_data_source_names(vec!["default".into(), "sofiadb".into(), "ondb".into()]);
    assign(&env, &admin, "sofiadb", digital).await;
    assign(&env, &admin, "ondb", on).await;
    (env, admin, digital, on)
}

tree_test!(two_organizations_serve_the_same_cube_name, |store| {
    let (env, admin, digital, on) = two_orgs(&store).await;
    env.writer
        .clone()
        .unwrap()
        .write("admin", "seed", |_| {
            Ok(vec![
                put_in(None, "u.yml", CUBE_U),
                put_in(Some(digital), "a.yml", &titled("a", "A Digital", "sofiadb")),
                put_in(Some(on), "a.yml", &titled("a", "A ON", "ondb")),
            ])
        })
        .await
        .unwrap();
    let ann = user(&env, &admin, "ann").await;
    let bia = user(&env, &admin, "bia").await;
    let ws_digital = workspace_with(&env, &admin, digital, "sofiadb", &["ann"]).await;
    let ws_on = workspace_with(&env, &admin, on, "ondb", &["bia"]).await;

    let (status, meta, _) = get_in_workspace(&env, &ann, &ws_digital, "/cube/v1/meta").await;
    assert_eq!(status, StatusCode::OK, "{meta}");
    assert_eq!(titles_of(&meta, "a"), ["A Digital"], "{meta}");
    let (status, meta, _) = get_in_workspace(&env, &bia, &ws_on, "/cube/v1/meta").await;
    assert_eq!(status, StatusCode::OK, "{meta}");
    assert_eq!(titles_of(&meta, "a"), ["A ON"], "{meta}");
    // A member of one organization's workspaces needs no header.
    let (status, meta, _) = get(&env, &bia, "/cube/v1/meta").await;
    assert_eq!(status, StatusCode::OK, "{meta}");
    assert_eq!(titles_of(&meta, "a"), ["A ON"], "{meta}");
    // ... and naming their own organization changes nothing.
    let (status, meta, _) =
        get_with(&env, &bia, &[("x-cube-organization", "on")], "/cube/v1/meta").await;
    assert_eq!(status, StatusCode::OK, "{meta}");
    assert_eq!(titles_of(&meta, "a"), ["A ON"], "{meta}");

    // A platform admin picks the organization, or gets the Platform.
    let (status, meta, _) =
        get_with(&env, &admin, &[("x-cube-organization", "on")], "/cube/v1/meta").await;
    assert_eq!(status, StatusCode::OK, "{meta}");
    assert_eq!(titles_of(&meta, "a"), ["A ON"], "{meta}");
    let (status, meta, _) =
        get_with(&env, &admin, &[("x-cube-organization", "digital")], "/cube/v1/meta").await;
    assert_eq!(status, StatusCode::OK, "{meta}");
    assert_eq!(titles_of(&meta, "a"), ["A Digital"], "{meta}");
    let (status, meta, _) = get(&env, &admin, "/cube/v1/meta").await;
    assert_eq!(status, StatusCode::OK, "{meta}");
    assert!(titles_of(&meta, "a").is_empty(), "{meta}");
    assert_eq!(titles_of(&meta, "u").len(), 1, "{meta}");
    let (status, meta, _) =
        get_with(&env, &admin, &[("x-cube-organization", "_platform")], "/cube/v1/meta").await;
    assert_eq!(status, StatusCode::OK, "{meta}");
    assert!(titles_of(&meta, "a").is_empty(), "{meta}");
    let (status, _, text) =
        get_with(&env, &admin, &[("x-cube-organization", "nope")], "/cube/v1/meta").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{text}");
    // An admin in a workspace gets the workspace's organization.
    let (status, meta, _) = get_in_workspace(&env, &admin, &ws_on, "/cube/v1/meta").await;
    assert_eq!(status, StatusCode::OK, "{meta}");
    assert_eq!(titles_of(&meta, "a"), ["A ON"], "{meta}");
});

tree_test!(member_with_workspaces_in_two_orgs_needs_a_workspace, |store| {
    let (env, admin, digital, on) = two_orgs(&store).await;
    env.writer
        .clone()
        .unwrap()
        .write("admin", "seed", |_| {
            Ok(vec![
                put_in(Some(digital), "a.yml", &titled("a", "A Digital", "sofiadb")),
                put_in(Some(on), "a.yml", &titled("a", "A ON", "ondb")),
            ])
        })
        .await
        .unwrap();
    let ann = user(&env, &admin, "ann").await;
    let ws_digital = workspace_with(&env, &admin, digital, "sofiadb", &["ann"]).await;
    workspace_with(&env, &admin, on, "ondb", &["ann"]).await;

    let (status, body, text) = get(&env, &ann, "/cube/v1/meta").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    assert_eq!(body["error"], cubeserver::rbac::enforce::WORKSPACE_REQUIRED);
    let (status, _, text) = get_with(
        &env,
        &ann,
        &[("x-cube-workspace", &ws_digital), ("x-cube-organization", "on")],
        "/cube/v1/meta",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    // In a workspace the request is served by that organization.
    let (status, meta, _) = get_in_workspace(&env, &ann, &ws_digital, "/cube/v1/meta").await;
    assert_eq!(status, StatusCode::OK, "{meta}");
    assert_eq!(titles_of(&meta, "a"), ["A Digital"], "{meta}");
});

tree_test!(moving_a_connection_moves_its_cubes_between_scopes, |store| {
    let (env, admin, digital, on) = two_orgs(&store).await;
    let writer = env.writer.clone().unwrap();
    writer
        .write("admin", "seed", |_| {
            Ok(vec![
                put_in(None, "u.yml", CUBE_U),
                put_in(Some(digital), "b.yml", CUBE_B),
            ])
        })
        .await
        .unwrap();
    let names = |meta: &Value| -> Vec<String> {
        meta["cubes"]
            .as_array()
            .unwrap_or_else(|| panic!("{meta}"))
            .iter()
            .map(|c| c["name"].as_str().unwrap().to_string())
            .collect()
    };
    let meta_of = |slug: &'static str| {
        let (env, admin) = (&env, admin.clone());
        async move {
            let (status, meta, _) =
                get_with(env, &admin, &[("x-cube-organization", slug)], "/cube/v1/meta").await;
            assert_eq!(status, StatusCode::OK, "{slug}: {meta}");
            names(&meta)
        }
    };
    assert!(meta_of("digital").await.contains(&"b".to_string()));
    assert!(!meta_of("on").await.contains(&"b".to_string()));

    assign(&env, &admin, "sofiadb", on).await;
    let id = store
        .model_rows()
        .await
        .unwrap()
        .files
        .iter()
        .find(|f| f.path == "b.yml")
        .unwrap()
        .id;
    writer
        .write("admin", "move", |_| {
            Ok(vec![cubeserver::rbac::model_store::ModelChange::Move {
                id,
                organization_id: Some(on),
            }])
        })
        .await
        .unwrap();

    // No restart: both scopes show the move.
    let on_names = meta_of("on").await;
    assert!(on_names.contains(&"b".to_string()), "{on_names:?}");
    assert!(on_names.contains(&"u".to_string()), "{on_names:?}");
    let digital_names = meta_of("digital").await;
    assert!(!digital_names.contains(&"b".to_string()), "{digital_names:?}");
});

tree_test!(an_organization_without_files_serves_an_empty_model, |store| {
    let (env, admin, digital, on) = two_orgs(&store).await;
    // Every file belongs to Digital: ON and the Platform have none, and there
    // is no `_platform/` file either.
    env.writer
        .clone()
        .unwrap()
        .write("admin", "seed", |_| {
            Ok(vec![put_in(Some(digital), "a.yml", &titled("a", "A Digital", "sofiadb"))])
        })
        .await
        .unwrap();
    let bia = user(&env, &admin, "bia").await;
    let ws_on = workspace_with(&env, &admin, on, "ondb", &["bia"]).await;
    let cache = env._cache.as_ref().unwrap().path().to_string_lossy().into_owned();

    let mut answers = vec![
        ("member in workspace", get_in_workspace(&env, &bia, &ws_on, "/cube/v1/meta").await),
        ("member", get(&env, &bia, "/cube/v1/meta").await),
        (
            "admin of on",
            get_with(&env, &admin, &[("x-cube-organization", "on")], "/cube/v1/meta").await,
        ),
        ("admin", get(&env, &admin, "/cube/v1/meta").await),
    ];
    for (who, (status, body, text)) in &answers {
        assert_eq!(*status, StatusCode::OK, "{who}: {text}");
        assert_eq!(body["cubes"], json!([]), "{who}: {text}");
    }

    // A query naming a cube the scope does not have is the usual error: the
    // one a scope with files gives for a cube it does not have.
    let load = |cube: &str| {
        format!("/cube/v1/load?query=%7B%22measures%22%3A%5B%22{cube}.n%22%5D%7D")
    };
    let (status, _, text) = get_in_workspace(&env, &bia, &ws_on, &load("a")).await;
    assert_ne!(status, StatusCode::SERVICE_UNAVAILABLE, "{text}");
    answers.push(("member load", (status, Value::Null, text)));
    let (usual_status, usual, _) =
        get_with(&env, &admin, &[("x-cube-organization", "digital")], &load("zz")).await;
    let (status, body, text) =
        get_with(&env, &admin, &[("x-cube-organization", "on")], &load("zz")).await;
    assert_eq!(status, usual_status, "{text}");
    assert_eq!(body, usual, "{text}");
    answers.push(("admin load", (status, body, text)));
    for (who, (_, _, text)) in &answers {
        assert!(!text.contains(&cache), "{who} shows the cache dir: {text}");
        assert!(!text.contains("scopes/"), "{who} shows a scope folder: {text}");
    }
});

/// Digital's `a` (on sofiadb, with a dimension only it has) and ON's `a` (on
/// ondb), written to the database.
async fn same_name_in_two_orgs(env: &Env, digital: uuid::Uuid, on: uuid::Uuid) {
    let digital_a = format!(
        "{}    dimensions:\n      - name: only_digital\n        sql: x\n        type: string\n",
        titled("a", "A Digital", "sofiadb")
    );
    env.writer
        .clone()
        .unwrap()
        .write("admin", "seed", |_| {
            Ok(vec![
                put_in(None, "u.yml", CUBE_U),
                put_in(Some(digital), "a.yml", &digital_a),
                put_in(Some(on), "a.yml", &titled("a", "A ON", "ondb")),
            ])
        })
        .await
        .unwrap();
}

tree_test!(tree_lists_same_name_in_two_orgs, |store| {
    let (env, admin, digital, on) = two_orgs(&store).await;
    same_name_in_two_orgs(&env, digital, on).await;

    let (status, body, _) = get(&env, &admin, "/playground/tree").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let orgs = body["organizations"].as_array().unwrap();
    let by = |slug: &str| orgs.iter().find(|o| o["slug"] == slug).unwrap().clone();
    for slug in ["digital", "on", "_platform"] {
        assert_eq!(by(slug)["counts"]["cubes"], 1, "{slug}: {body}");
        assert_eq!(by(slug)["errors"], 0, "{slug}: {body}");
    }

    for (slug, source) in [("digital", "sofiadb"), ("on", "ondb")] {
        let (status, body, _) = get(&env, &admin, &format!("/playground/tree/{slug}")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let cubes = body["cubes"].as_array().unwrap();
        assert_eq!(cubes.len(), 1, "{slug}: {body}");
        assert_eq!(cubes[0]["name"], "a");
        assert_eq!(cubes[0]["dataSource"], source);
        assert_eq!(cubes[0]["file"], format!("{slug}/a.yml"));
        // Only the databases' answers may fail here (no database runs).
        assert!(
            body["errors"].as_array().unwrap().iter().all(|e| e["file"] == ""),
            "{slug}: {body}"
        );
    }
});

tree_test!(meta_groups_in_org_scope, |store| {
    let (env, admin, digital, on) = two_orgs(&store).await;
    same_name_in_two_orgs(&env, digital, on).await;

    let (status, body, _) =
        get_with(&env, &admin, &[("x-cube-organization", "on")], "/cube/v1/meta/groups").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({ "groups": [
            { "slug": "_platform", "id": null, "name": "Platform", "errors": 0,
              "dataSources": [{ "name": "default", "cubes": ["u"] }] },
            { "slug": "on", "id": on, "name": "ON", "errors": 0,
              "dataSources": [{ "name": "ondb", "cubes": ["a"] }] },
        ] })
    );
});

tree_test!(model_members_read_the_organizations_scope, |store| {
    let (env, admin, digital, on) = two_orgs(&store).await;
    same_name_in_two_orgs(&env, digital, on).await;

    let dimensions = |body: &Value| -> Vec<String> {
        let cubes = body["cubes"].as_array().unwrap_or_else(|| panic!("{body}"));
        assert_eq!(cubes.len(), 1, "{body}");
        assert_eq!(cubes[0]["name"], "a", "{body}");
        cubes[0]["dimensions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["name"].as_str().unwrap().to_string())
            .collect()
    };
    let (status, body, _) =
        get(&env, &admin, &format!("/cube/v1/organizations/{digital}/model-members")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(dimensions(&body).contains(&"a.only_digital".to_string()), "{body}");
    let (status, body, _) =
        get(&env, &admin, &format!("/cube/v1/organizations/{on}/model-members")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!dimensions(&body).contains(&"a.only_digital".to_string()), "{body}");
});

tree_test!(generate_reuses_a_name_taken_in_another_org, |store| {
    let (env, admin, digital, _on) = two_orgs(&store).await;
    env.writer
        .clone()
        .unwrap()
        .write("admin", "seed", |_| {
            Ok(vec![put_in(
                Some(digital),
                "dfos_schema_meta.yml",
                &titled("dfos_schema_meta", "Digital's", "sofiadb"),
            )])
        })
        .await
        .unwrap();
    let cols = json!([{ "name": "id", "type": "String", "attributes": ["primaryKey"] }]);
    let (status, body, _) = send(
        &env.router,
        Method::POST,
        "/playground/generate-schema",
        Some(&admin),
        Some(json!({
            "format": "yaml",
            "tables": [["ondb.dfos", "schema_meta"]],
            "tablesSchema": { "ondb.dfos": { "schema_meta": cols } }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["files"][0]["fileName"], "on/dfos_schema_meta.yml", "{body}");
    let rows = store.model_rows().await.unwrap();
    assert_eq!(rows.files.iter().filter(|f| f.path == "dfos_schema_meta.yml").count(), 2);
});

tree_test!(generate_into_platform_refuses_a_name_any_org_has, |store| {
    let env = env(store).await;
    let admin = login(&env, ADMIN, ADMIN_PASSWORD).await;
    install_policy(&[("digital", &["sofiadb"])], &["default"]);
    // A `_platform/` cube is compiled in every organization's scope.
    env.write("digital/users.yml", &titled("users", "Digital's", "sofiadb"));

    let (status, body, _) = send(
        &env.router,
        Method::POST,
        "/playground/generate-schema",
        Some(&admin),
        Some(generate_body(json!([["public", "users"]]))),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(!env.model.path().join("_platform/users.yml").exists());
});

tree_test!(boot_services_compile_the_platform_scope_when_organizations_share_a_name, |store| {
    use cubeserver::meta_adapter::ModelMetaService;
    use cubeserver::rbac::model_runtime::boot_model_path;
    use cubeserver::rbac::scope::ModelScope;
    let (env, _admin, digital, on) = two_orgs(&store).await;
    env.writer
        .clone()
        .unwrap()
        .write("admin", "seed", |_| {
            Ok(vec![
                put_in(None, "u.yml", CUBE_U),
                put_in(Some(digital), "a.yml", &titled("a", "A Digital", "sofiadb")),
                put_in(Some(on), "a.yml", &titled("a", "A ON", "ondb")),
            ])
        })
        .await
        .unwrap();
    let runtime = env.runtime.clone().unwrap();
    let live = runtime.live_path().to_string_lossy().into_owned();
    // The whole model has two cubes named `a`: it does not compile.
    assert!(ModelMetaService::load(&live, false).is_err());

    // What `main` compiles at boot with RBAC on: the Platform scope.
    let platform = boot_model_path(Some(&*runtime), &live, &ModelScope::Platform)
        .await
        .unwrap();
    assert_ne!(platform, live);
    ModelMetaService::load(&platform, false).expect("the Platform scope compiles");
    // The SQL API may serve one organization's scope instead.
    let on_path = boot_model_path(Some(&*runtime), &live, &ModelScope::Organization("on".into()))
        .await
        .unwrap();
    ModelMetaService::load(&on_path, false).expect("ON's scope compiles");
    // An organization that does not exist is refused, not served empty.
    let err = boot_model_path(Some(&*runtime), &live, &ModelScope::Organization("nope".into()))
        .await
        .unwrap_err();
    assert!(err.contains("nope"), "{err}");
});
