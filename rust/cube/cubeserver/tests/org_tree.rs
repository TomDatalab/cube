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
}

async fn env(store: cubeserver::rbac::store::Store) -> Env {
    let model = tempfile::tempdir().unwrap();
    let rbac = Arc::new(
        Rbac::with_parts(Arc::new(FakeAuthz::new()), store, rbac_config())
            .await
            .expect("rbac starts"),
    );
    let config = ServerConfig {
        schema_path: model.path().to_string_lossy().into_owned(),
        ..ServerConfig::default()
    };
    let router = build_app(AppState {
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
        tenants: None,
        rbac: Some(rbac),
        model_index: Default::default(),
    });
    Env { router, model }
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
        .header(header::HOST, "localhost:4000");
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

fn install_policy(orgs: &[(&str, &[&str])], platform: &[&str]) {
    static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
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
        generation: GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
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
