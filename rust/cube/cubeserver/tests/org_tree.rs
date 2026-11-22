//! The Playground's organization tree (`/playground/tree*`) with RBAC on.
//!
//! The ownership policy is process-global, so these tests live in a binary of
//! their own, run one at a time, and clear the policy when they end. They need
//! `CUBE_TEST_PG_URL` (see `support`).

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use cubeauth::AuthConfig;
use cubemodel::ownership::{set_policy, OwnershipPolicy};
use cubeserver::auth_adapter::CubeAuthService;
use cubeserver::rbac::config::RbacConfig;
use cubeserver::rbac::testing::FakeAuthz;
use cubeserver::rbac::Rbac;
use cubeserver::services::{
    AlwaysHealthy, MetaService, RequestContext, UnimplementedQueryService,
};
use cubeserver::{build_app, ApiError, AppState, ServerConfig};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use support::TestDb;
use tower::ServiceExt;

const SECRET: &str = "rbac-test-secret";
const ADMIN: &str = "admin";
const ADMIN_PASSWORD: &str = "admin-password-1";

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Clears the policy when the test ends, panicking or not.
struct ClearPolicy;
impl Drop for ClearPolicy {
    fn drop(&mut self) {
        set_policy(None);
    }
}

#[derive(Debug)]
struct NoMeta;

#[async_trait::async_trait]
impl MetaService for NoMeta {
    async fn meta(&self, _: &RequestContext, _: bool) -> Result<Value, ApiError> {
        Ok(json!({ "cubes": [] }))
    }
}

fn rbac_config() -> RbacConfig {
    RbacConfig {
        spicedb_endpoint: "http://unused".into(),
        preshared_key: "unused".into(),
        db_url: "postgres://unused".into(),
        admins: vec![ADMIN.into()],
        admin_password: Some(ADMIN_PASSWORD.into()),
        token_ttl: Duration::from_secs(3600),
        failure_delay: Duration::ZERO,
        max_concurrent_hashes: 4,
        hash_queue_wait: Duration::from_secs(2),
        secret_key: cubeserver::rbac::secrets::SecretKey::from_base64(
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        )
        .unwrap(),
    }
}

struct Env {
    router: axum::Router,
    model: tempfile::TempDir,
    /// The Playground's assets: `build_app` mounts `/playground/*` only when
    /// they exist, as the deployment does.
    _assets: tempfile::TempDir,
    rbac: Arc<Rbac>,
    state: AppState,
}

async fn env(store: cubeserver::rbac::store::Store) -> Env {
    env_with(store, false).await
}

/// `real_meta`: the model is served by a tenant registry, as in `main`, so a
/// policy change shows in `/cube/v1/meta`.
async fn env_with(store: cubeserver::rbac::store::Store, real_meta: bool) -> Env {
    let model = tempfile::tempdir().unwrap();
    let rbac = Arc::new(
        Rbac::with_parts(Arc::new(FakeAuthz::new()), store, rbac_config())
            .await
            .expect("rbac starts"),
    );
    // As `main` does when it installs the boot policy.
    rbac.enable_ownership();
    let assets = tempfile::tempdir().unwrap();
    std::fs::write(assets.path().join("index.html"), "<html></html>").unwrap();
    let config = ServerConfig {
        schema_path: model.path().to_string_lossy().into_owned(),
        playground_path: Some(assets.path().to_string_lossy().into_owned()),
        data_sources: vec![("sofiadb".into(), "postgres".into())],
        ..ServerConfig::default()
    };
    let tenants = real_meta.then(|| {
        let cube_config = cubeconfig::CubeConfig::load_or_env(
            "/nonexistent-cube-yml-dir",
            &cubeconfig::Env::new()
                .with("CUBEJS_SCHEMA_PATH", &*model.path().to_string_lossy())
                .with("CUBEJS_DB_TYPE", "postgres"),
        )
        .expect("config from the environment");
        Arc::new(cubeserver::tenants::TenantRegistry::new(
            Arc::new(cube_config),
            cubeplanner::Dialect::Postgres,
            Some("postgres".to_string()),
            1,
            false,
        ))
    });
    let state = AppState {
        config: Arc::new(config),
        query_config: Arc::new(cubequery::QueryConfig::default()),
        auth: Arc::new(CubeAuthService::new(
            AuthConfig::builder()
                .api_secret(SECRET)
                .enforce_security_checks(true)
                .default_api_scopes(["graphql", "meta", "data"])
                .build(),
        )),
        meta: Arc::new(NoMeta),
        health: Arc::new(AlwaysHealthy),
        query: Arc::new(UnimplementedQueryService),
        pre_aggregations: Arc::new(cubeserver::services::UnimplementedPreAggregationService),
        sql_conversion: Arc::new(cubeserver::services::UnimplementedSqlConversionService),
        graphql: Default::default(),
        ws: Default::default(),
        tenants,
        rbac: Some(rbac.clone()),
        model_index: Default::default(),
    };
    let router = build_app(state.clone());
    Env { router, model, _assets: assets, rbac, state }
}

impl Env {
    fn write(&self, name: &str, content: &str) {
        let path = self.model.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
}

async fn send(
    router: &axum::Router,
    method: Method,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value, String) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "localhost:4000")
        .header(
            cubeserver::handlers::playground::PLAYGROUND_KEY_HEADER,
            cubeserver::handlers::playground::playground_key(),
        );
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let request = match body {
        Some(body) => request
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string())),
        None => request.body(Body::empty()),
    }
    .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    (status, serde_json::from_str(&text).unwrap_or(Value::Null), text)
}

async fn login(env: &Env, user: &str, password: &str) -> String {
    let (status, body, _) = send(
        &env.router,
        Method::POST,
        "/cube/v1/auth/login",
        None,
        Some(json!({ "username": user, "password": password })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["token"].as_str().unwrap().to_string()
}

async fn create_org(env: &Env, token: &str, name: &str) -> String {
    let (status, body, _) = send(
        &env.router,
        Method::POST,
        "/cube/v1/organizations",
        Some(token),
        Some(json!({ "name": name, "owner": ADMIN })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["slug"].as_str().unwrap().to_string()
}

async fn get(env: &Env, token: &str, path: &str) -> (StatusCode, Value, String) {
    send(&env.router, Method::GET, path, Some(token), None).await
}

/// Installs a policy; `set_policy` gives it a new generation.
fn install_policy(orgs: &[(&str, &[&str])], platform: &[&str]) {
    set_policy(Some(Arc::new(OwnershipPolicy {
        organizations: orgs
            .iter()
            .map(|(slug, ds)| {
                (
                    slug.to_string(),
                    ds.iter().map(|d| d.to_string()).collect::<BTreeSet<_>>(),
                )
            })
            .collect::<BTreeMap<_, _>>(),
        platform: platform.iter().map(|d| d.to_string()).collect(),
        generation: 0,
    })));
}

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
