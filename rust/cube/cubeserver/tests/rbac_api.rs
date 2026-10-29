//! RBAC through the HTTP API: login, password changes and the identity every
//! authenticated request carries.
//!
//! The tests that need users run only when `CUBE_TEST_PG_URL` points at a
//! Postgres server (each in its own throw-away database); SpiceDB is the
//! in-memory fake.

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use cubeauth::AuthConfig;
use cubeserver::auth_adapter::CubeAuthService;
use cubeserver::planner_adapter::DataSources;
use cubeserver::rbac::config::{check_rbac_compatible, RbacConfig};
use cubeserver::rbac::enforce::WORKSPACE_REQUIRED;
use cubeserver::rbac::identity::{hash_password, verify_password};
use cubeserver::rbac::store::{ConnectionOrigin, Store};
use cubeserver::rbac::testing::FakeAuthz;
use cubeserver::rbac::{Authz, ObjectRef, Rbac, Relationship, RelationshipFilter};
use cubeserver::services::{
    AlwaysHealthy, MetaService, NormalizedRequest, QueryService, RequestContext,
    UnimplementedQueryService,
};
use cubeserver::{build_app, ApiError, AppState, ServerConfig};
use http_body_util::BodyExt;
use serde_json::{json, Map, Value};
use support::TestDb;
use tower::ServiceExt;

const SECRET: &str = "rbac-test-secret";
const ADMIN: &str = "admin";
const ADMIN_PASSWORD: &str = "admin-password-1";

#[derive(Debug)]
struct NoMeta;

#[async_trait]
impl MetaService for NoMeta {
    async fn meta(&self, _: &RequestContext, _: bool) -> Result<Value, ApiError> {
        Ok(json!({ "cubes": [] }))
    }
}

fn auth_config() -> AuthConfig {
    AuthConfig::builder()
        .api_secret(SECRET)
        .enforce_security_checks(true)
        // `data_sources`, `execute` and `jobs` too, so a 403 there comes from
        // the RBAC check.
        .default_api_scopes([
            "graphql",
            "meta",
            "data",
            "sql",
            "data_sources",
            "execute",
            "jobs",
        ])
        .build()
}

fn rbac_config(failure_delay: Duration) -> RbacConfig {
    RbacConfig {
        spicedb_endpoint: "http://unused".into(),
        preshared_key: "unused".into(),
        db_url: "postgres://unused".into(),
        admins: vec![ADMIN.into()],
        admin_password: Some(ADMIN_PASSWORD.into()),
        token_ttl: Duration::from_secs(3600),
        failure_delay,
        max_concurrent_hashes: 4,
        hash_queue_wait: Duration::from_secs(2),
    }
}

fn state(rbac: Option<Arc<Rbac>>) -> AppState {
    state_with(rbac, ServerConfig::default())
}

fn state_with(rbac: Option<Arc<Rbac>>, config: ServerConfig) -> AppState {
    AppState {
        config: Arc::new(config),
        query_config: Arc::new(cubequery::QueryConfig::default()),
        auth: Arc::new(CubeAuthService::new(auth_config())),
        meta: Arc::new(NoMeta),
        health: Arc::new(AlwaysHealthy),
        query: Arc::new(UnimplementedQueryService),
        pre_aggregations: Arc::new(cubeserver::services::UnimplementedPreAggregationService),
        sql_conversion: Arc::new(cubeserver::services::UnimplementedSqlConversionService),
        graphql: Default::default(),
        ws: Default::default(),
        tenants: None,
        rbac,
    }
}

struct Env {
    router: axum::Router,
    rbac: Arc<Rbac>,
    authz: Arc<FakeAuthz>,
}

async fn env(store: Store, failure_delay: Duration) -> Env {
    env_with(store, rbac_config(failure_delay)).await
}

async fn env_with(store: Store, config: RbacConfig) -> Env {
    env_with_server(store, config, ServerConfig::default()).await
}

async fn env_with_server(store: Store, config: RbacConfig, server: ServerConfig) -> Env {
    let authz = Arc::new(FakeAuthz::new());
    let rbac = Arc::new(
        Rbac::with_parts(authz.clone(), store, config)
            .await
            .expect("rbac starts"),
    );
    Env {
        router: build_app(state_with(Some(rbac.clone()), server)),
        rbac,
        authz,
    }
}

async fn send(
    router: &axum::Router,
    method: Method,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(path);
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
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

async fn login(router: &axum::Router, username: &str, password: &str) -> (StatusCode, Value) {
    send(
        router,
        Method::POST,
        "/cube/v1/auth/login",
        None,
        Some(json!({ "username": username, "password": password })),
    )
    .await
}

async fn login_token(router: &axum::Router, username: &str, password: &str) -> String {
    let (status, body) = login(router, username, password).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["token"].as_str().expect("a token").to_string()
}

async fn meta(router: &axum::Router, token: Option<&str>) -> StatusCode {
    send(router, Method::GET, "/cube/v1/meta", token, None)
        .await
        .0
}

/// Runs `body` against a fresh database and always drops it.
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

db_test!(login_returns_a_token_that_authenticates, |store| {
    let env = env(store, Duration::ZERO).await;
    let (status, body) = login(&env.router, ADMIN, ADMIN_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["expiresAt"].is_string(), "{body}");
    assert_eq!(body["user"]["id"], ADMIN);
    assert_eq!(body["user"]["platformAdmin"], json!(true));
    // Never the hash.
    assert!(!body.to_string().contains("argon2"), "{body}");

    let token = body["token"].as_str().unwrap();
    assert_eq!(meta(&env.router, Some(token)).await, StatusCode::OK);
    // No token at all.
    assert_eq!(meta(&env.router, None).await, StatusCode::UNAUTHORIZED);
});

db_test!(
    wrong_password_and_unknown_user_get_the_same_answer,
    |store| {
        let delay = Duration::from_millis(300);
        let env = env(store, delay).await;

        let started = Instant::now();
        let wrong = login(&env.router, ADMIN, "not-the-password").await;
        assert!(started.elapsed() >= delay, "a failure is slowed down");
        let unknown = login(&env.router, "nobody", ADMIN_PASSWORD).await;

        assert_eq!(wrong.0, StatusCode::UNAUTHORIZED);
        assert_eq!(wrong, unknown);
        assert_eq!(wrong.1, json!({ "error": "Invalid username or password" }));
    }
);

db_test!(short_passwords_are_refused, |store| {
    let env = env(store, Duration::ZERO).await;
    // Nine characters.
    let err = hash_password("123456789").unwrap_err();
    assert_eq!(err.status, StatusCode::BAD_REQUEST);
    assert!(hash_password("1234567890").is_ok());

    let token = login_token(&env.router, ADMIN, ADMIN_PASSWORD).await;
    let (status, body) = send(
        &env.router,
        Method::PUT,
        "/cube/v1/auth/password",
        Some(&token),
        Some(json!({ "currentPassword": ADMIN_PASSWORD, "newPassword": "123456789" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    // The old password still works.
    login_token(&env.router, ADMIN, ADMIN_PASSWORD).await;
});

db_test!(
    a_token_issued_before_a_password_change_stops_working,
    |store| {
        let env = env(store, Duration::ZERO).await;
        let old = login_token(&env.router, ADMIN, ADMIN_PASSWORD).await;

        // A wrong current password changes nothing.
        let (status, _) = send(
            &env.router,
            Method::PUT,
            "/cube/v1/auth/password",
            Some(&old),
            Some(
                json!({ "currentPassword": "not-the-password", "newPassword": "a-new-password-2" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(meta(&env.router, Some(&old)).await, StatusCode::OK);

        let (status, body) = send(
            &env.router,
            Method::PUT,
            "/cube/v1/auth/password",
            Some(&old),
            Some(json!({ "currentPassword": ADMIN_PASSWORD, "newPassword": "a-new-password-2" })),
        )
        .await;
        assert!(status.is_success(), "{status} {body}");

        assert_eq!(
            meta(&env.router, Some(&old)).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            login(&env.router, ADMIN, ADMIN_PASSWORD).await.0,
            StatusCode::UNAUTHORIZED
        );
        // A login right after the change, even within the same second, works.
        let new = login_token(&env.router, ADMIN, "a-new-password-2").await;
        assert_eq!(meta(&env.router, Some(&new)).await, StatusCode::OK);
    }
);

db_test!(a_disabled_user_is_locked_out_at_once, |store| {
    let env = env(store, Duration::ZERO).await;
    let user = cubeserver::rbac::store::NewUser {
        id: "ann".into(),
        name: Some("Ann".into()),
        email: None,
        password_hash: hash_password("ann-password-1").unwrap(),
    };
    env.rbac.store.create_user(&user).await.unwrap();
    let token = login_token(&env.router, "ann", "ann-password-1").await;
    assert_eq!(meta(&env.router, Some(&token)).await, StatusCode::OK);

    // What `PATCH /v1/users/ann {disabled: true}` does.
    env.rbac
        .store
        .update_user("ann", None, None, Some(true))
        .await
        .unwrap();

    assert_eq!(
        meta(&env.router, Some(&token)).await,
        StatusCode::UNAUTHORIZED
    );
    let (status, body) = login(&env.router, "ann", "ann-password-1").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body, json!({ "error": "Invalid username or password" }));
});

db_test!(a_token_without_sub_is_refused, |store| {
    let env = env(store, Duration::ZERO).await;
    let mut claims = Map::new();
    claims.insert("iat".into(), json!(chrono::Utc::now().timestamp()));
    let no_sub = cubeauth::issue_token(&auth_config(), claims, 3600).unwrap();
    assert_eq!(
        meta(&env.router, Some(&no_sub)).await,
        StatusCode::UNAUTHORIZED
    );

    // A `sub` naming nobody is no better.
    let mut claims = Map::new();
    claims.insert("sub".into(), json!("nobody"));
    claims.insert("iat".into(), json!(chrono::Utc::now().timestamp()));
    let unknown = cubeauth::issue_token(&auth_config(), claims, 3600).unwrap();
    assert_eq!(
        meta(&env.router, Some(&unknown)).await,
        StatusCode::UNAUTHORIZED
    );

    // Nor a token signed with another secret.
    let other = AuthConfig::builder().api_secret("another").build();
    let mut claims = Map::new();
    claims.insert("sub".into(), json!(ADMIN));
    claims.insert("iat".into(), json!(chrono::Utc::now().timestamp()));
    let forged = cubeauth::issue_token(&other, claims, 3600).unwrap();
    assert_eq!(
        meta(&env.router, Some(&forged)).await,
        StatusCode::UNAUTHORIZED
    );
});

db_test!(spicedb_outages_fail_closed, |store| {
    let env = env(store, Duration::ZERO).await;
    let token = login_token(&env.router, ADMIN, ADMIN_PASSWORD).await;
    env.authz.set_failing(true);

    assert_eq!(
        meta(&env.router, Some(&token)).await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        login(&env.router, ADMIN, ADMIN_PASSWORD).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
});

db_test!(the_admins_are_bootstrapped_once, |store| {
    let env = env(store.clone(), Duration::ZERO).await;
    assert!(env.rbac.is_platform_admin(ADMIN).await.unwrap());
    let first = store.user(ADMIN).await.unwrap().expect("admin created");
    assert!(verify_password(&first.password_hash, ADMIN_PASSWORD));

    // A restart with another bootstrap password keeps the existing one.
    let mut config = rbac_config(Duration::ZERO);
    config.admin_password = Some("another-password-9".into());
    Rbac::with_parts(env.authz.clone(), store.clone(), config)
        .await
        .unwrap();
    let again = store.user(ADMIN).await.unwrap().unwrap();
    assert_eq!(again.password_hash, first.password_hash);
});

#[tokio::test]
async fn without_rbac_the_auth_routes_do_not_exist() {
    let router = build_app(state(None));
    let (status, _) = login(&router, ADMIN, ADMIN_PASSWORD).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[test]
fn passwords_are_hashed_with_argon2id() {
    let hash = hash_password("a-long-password").unwrap();
    assert!(hash.starts_with("$argon2id$"), "{hash}");
    assert!(verify_password(&hash, "a-long-password"));
    assert!(!verify_password(&hash, "a-long-passworD"));
    assert!(!verify_password("not a hash", "a-long-password"));
}

#[test]
fn startup_refuses_tenants_with_rbac() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("cube.yml"),
        "version: 1\ndata_sources:\n  default:\n    type: postgres\ntenants:\n  claim: tenant_id\n  rules:\n    - match: acme\n      app_id: acme\n",
    )
    .unwrap();
    let with_tenants = cubeconfig::CubeConfig::load_with_env(dir.path(), &cubeconfig::Env::new())
        .expect("a valid cube.yml");
    assert!(with_tenants.has_tenants());
    let err = check_rbac_compatible(&with_tenants).unwrap_err();
    assert!(err.contains("tenants"), "{err}");

    let plain_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        plain_dir.path().join("cube.yml"),
        "version: 1\ndata_sources:\n  default:\n    type: postgres\n",
    )
    .unwrap();
    let plain =
        cubeconfig::CubeConfig::load_with_env(plain_dir.path(), &cubeconfig::Env::new()).unwrap();
    assert!(check_rbac_compatible(&plain).is_ok());
}

db_test!(
    an_outage_answers_alike_for_right_and_wrong_passwords,
    |store| {
        let env = env(store, Duration::ZERO).await;
        env.authz.set_failing(true);
        let right = login(&env.router, ADMIN, ADMIN_PASSWORD).await;
        let wrong = login(&env.router, ADMIN, "not-the-password").await;
        assert_eq!(right.0, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(right, wrong);
    }
);

db_test!(
    a_login_flood_is_refused_instead_of_queued_forever,
    |store| {
        let mut config = rbac_config(Duration::ZERO);
        config.max_concurrent_hashes = 1;
        config.hash_queue_wait = Duration::from_millis(200);
        let env = env_with(store, config).await;

        // A verification in flight holds the only slot.
        let held = env.rbac.hashing_permits().acquire_owned().await.unwrap();
        let (status, body) = login(&env.router, ADMIN, ADMIN_PASSWORD).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");

        drop(held);
        login_token(&env.router, ADMIN, ADMIN_PASSWORD).await;
    }
);

db_test!(names_that_are_not_user_ids_get_the_usual_answer, |store| {
    let delay = Duration::from_millis(200);
    let env = env(store, delay).await;
    let unknown = login(&env.router, "nobody", ADMIN_PASSWORD).await;
    for name in ["Admin", "admin.x", "*", "a b", ""] {
        let started = Instant::now();
        let answer = login(&env.router, name, ADMIN_PASSWORD).await;
        assert!(started.elapsed() >= delay, "{name}");
        assert_eq!(answer, unknown, "{name}");
    }
});

db_test!(a_token_whose_sub_is_not_a_user_id_is_refused, |store| {
    let env = env(store, Duration::ZERO).await;
    for sub in ["Admin", "admin.x", "*"] {
        let mut claims = Map::new();
        claims.insert("sub".into(), json!(sub));
        claims.insert("iat".into(), json!(chrono::Utc::now().timestamp() + 60));
        let token = cubeauth::issue_token(&auth_config(), claims, 3600).unwrap();
        assert_eq!(
            meta(&env.router, Some(&token)).await,
            StatusCode::UNAUTHORIZED,
            "{sub}"
        );
    }
});

#[tokio::test]
async fn a_non_ascii_authorization_header_is_401_with_rbac() {
    let Some(db) = TestDb::create().await else {
        return;
    };
    let env = env(db.store.clone(), Duration::ZERO).await;
    let request = Request::builder()
        .uri("/cube/v1/meta")
        .header(
            header::AUTHORIZATION,
            axum::http::HeaderValue::from_bytes(b"Bearer \xff\xfe").unwrap(),
        )
        .body(Body::empty())
        .unwrap();
    let status = env.router.clone().oneshot(request).await.unwrap().status();
    db.drop_db().await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// Records what the WebSocket server sent.
#[derive(Debug, Default)]
struct Recorder {
    sent: tokio::sync::Mutex<Vec<Value>>,
}

#[async_trait]
impl cubeserver::ws::WsConnection for Recorder {
    async fn send(&self, _connection_id: &str, message: Value) {
        self.sent.lock().await.push(message);
    }
}

impl Recorder {
    async fn last(&self) -> Value {
        self.sent.lock().await.last().cloned().expect("a message")
    }
}

db_test!(a_websocket_loses_its_user_at_the_next_message, |store| {
    let env = env(store, Duration::ZERO).await;
    let user = cubeserver::rbac::store::NewUser {
        id: "ann".into(),
        name: None,
        email: None,
        password_hash: hash_password("ann-password-1").unwrap(),
    };
    env.rbac.store.create_user(&user).await.unwrap();
    let token = login_token(&env.router, "ann", "ann-password-1").await;

    let recorder = Arc::new(Recorder::default());
    let server =
        cubeserver::ws::SubscriptionServer::new(state(Some(env.rbac.clone())), recorder.clone());
    let meta_message = |id: u32| json!({ "method": "meta", "messageId": id }).to_string();
    server
        .process_message("c1", &json!({ "authorization": token }).to_string())
        .await;
    assert_eq!(recorder.last().await, json!({ "handshake": true }));
    server.process_message("c1", &meta_message(1)).await;
    assert_eq!(recorder.last().await, json!({ "messageProcessedId": "1" }));

    // Disabled: the next method (as every subscription re-run) is refused,
    // and the connection loses its authorization and subscriptions.
    env.rbac
        .store
        .update_user("ann", None, None, Some(true))
        .await
        .unwrap();
    server.process_message("c1", &meta_message(2)).await;
    assert_eq!(recorder.last().await["status"], json!(401));
    env.rbac
        .store
        .update_user("ann", None, None, Some(false))
        .await
        .unwrap();
    server.process_message("c1", &meta_message(3)).await;
    assert_eq!(recorder.last().await["status"], json!(403));
});

// ---- user management (platform admins) ----

async fn admin_token(env: &Env) -> String {
    login_token(&env.router, ADMIN, ADMIN_PASSWORD).await
}

async fn create_user_as(env: &Env, token: &str, id: &str, password: &str) -> (StatusCode, Value) {
    send(
        &env.router,
        Method::POST,
        "/cube/v1/users",
        Some(token),
        Some(
            json!({ "id": id, "name": "Some One", "email": "x@example.org", "password": password }),
        ),
    )
    .await
}

db_test!(only_platform_admins_manage_users, |store| {
    let env = env(store, Duration::ZERO).await;
    let admin = admin_token(&env).await;
    let (status, body) = create_user_as(&env, &admin, "ann", "ann-password-1").await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let ann = login_token(&env.router, "ann", "ann-password-1").await;

    let calls = [
        (
            Method::POST,
            "/cube/v1/users",
            Some(json!({ "id": "bob", "password": "bob-password-1" })),
        ),
        (Method::GET, "/cube/v1/users", None),
        (
            Method::PATCH,
            "/cube/v1/users/ann",
            Some(json!({ "name": "Ann" })),
        ),
        (
            Method::PUT,
            "/cube/v1/users/ann/password",
            Some(json!({ "password": "another-pass-1" })),
        ),
        (Method::POST, "/cube/v1/users/ann/tokens", None),
    ];
    for (method, path, body) in calls {
        let (status, answer) =
            send(&env.router, method.clone(), path, Some(&ann), body.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path} {answer}");
        let (status, _) = send(&env.router, method.clone(), path, None, body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
    }
    // Nothing happened.
    assert!(env.rbac.store.user("bob").await.unwrap().is_none());
    login_token(&env.router, "ann", "ann-password-1").await;
});

db_test!(user_listing_never_shows_password_hashes, |store| {
    let env = env(store, Duration::ZERO).await;
    let admin = admin_token(&env).await;
    let (status, created) = create_user_as(&env, &admin, "ann", "ann-password-1").await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["id"], "ann");
    assert_eq!(created["disabled"], json!(false));

    let (status, list) = send(
        &env.router,
        Method::GET,
        "/cube/v1/users",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let ids: Vec<&str> = list["users"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["admin", "ann"]);

    let (status, patched) = send(
        &env.router,
        Method::PATCH,
        "/cube/v1/users/ann",
        Some(&admin),
        Some(json!({ "name": "Ann Lee", "disabled": false })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched}");
    assert_eq!(patched["name"], "Ann Lee");

    for text in [created.to_string(), list.to_string(), patched.to_string()] {
        assert!(!text.contains("argon2"), "{text}");
        assert!(!text.to_lowercase().contains("password"), "{text}");
    }

    // Duplicates and unknown users.
    let (status, _) = create_user_as(&env, &admin, "ann", "ann-password-1").await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = send(
        &env.router,
        Method::PATCH,
        "/cube/v1/users/nobody",
        Some(&admin),
        Some(json!({ "name": "x" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
});

db_test!(a_service_token_authenticates_as_its_user, |store| {
    let env = env(store, Duration::ZERO).await;
    let admin = admin_token(&env).await;
    create_user_as(&env, &admin, "etl", "etl-password-1").await;

    let (status, body) = send(
        &env.router,
        Method::POST,
        "/cube/v1/users/etl/tokens",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["expiresAt"].is_string());
    let token = body["token"].as_str().unwrap();
    assert_eq!(meta(&env.router, Some(token)).await, StatusCode::OK);

    // Disabled users get no token (409), and the old one stops working.
    send(
        &env.router,
        Method::PATCH,
        "/cube/v1/users/etl",
        Some(&admin),
        Some(json!({ "disabled": true })),
    )
    .await;
    assert_eq!(
        meta(&env.router, Some(token)).await,
        StatusCode::UNAUTHORIZED
    );
    let (status, _) = send(
        &env.router,
        Method::POST,
        "/cube/v1/users/etl/tokens",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = send(
        &env.router,
        Method::POST,
        "/cube/v1/users/nobody/tokens",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
});

db_test!(invalid_user_ids_are_refused, |store| {
    let env = env(store, Duration::ZERO).await;
    let admin = admin_token(&env).await;
    for id in ["Ana", "a b", "joao.silva", "", "*"] {
        let (status, body) = create_user_as(&env, &admin, id, "long-password-1").await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{id:?} {body}");
    }
    for path in ["/cube/v1/users/Ana", "/cube/v1/users/joao.silva"] {
        let (status, _) = send(
            &env.router,
            Method::PATCH,
            path,
            Some(&admin),
            Some(json!({ "name": "x" })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}");
    }
    let (status, _) = send(
        &env.router,
        Method::PUT,
        "/cube/v1/users/Ana/password",
        Some(&admin),
        Some(json!({ "password": "long-password-1" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // A short password is refused too, and nothing was created.
    let (status, _) = create_user_as(&env, &admin, "ann", "short").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(env.rbac.store.user("ann").await.unwrap().is_none());
});

db_test!(an_admin_password_reset_invalidates_old_tokens, |store| {
    let env = env(store, Duration::ZERO).await;
    let admin = admin_token(&env).await;
    create_user_as(&env, &admin, "ann", "ann-password-1").await;
    let old = login_token(&env.router, "ann", "ann-password-1").await;
    assert_eq!(meta(&env.router, Some(&old)).await, StatusCode::OK);

    let (status, _) = send(
        &env.router,
        Method::PUT,
        "/cube/v1/users/ann/password",
        Some(&admin),
        Some(json!({ "password": "reset-password-2" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        meta(&env.router, Some(&old)).await,
        StatusCode::UNAUTHORIZED
    );
    let new = login_token(&env.router, "ann", "reset-password-2").await;
    assert_eq!(meta(&env.router, Some(&new)).await, StatusCode::OK);

    let (status, _) = send(
        &env.router,
        Method::PUT,
        "/cube/v1/users/nobody/password",
        Some(&admin),
        Some(json!({ "password": "reset-password-2" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
});

db_test!(patch_guards_and_bad_requests, |store| {
    let env = env(store, Duration::ZERO).await;
    let admin = admin_token(&env).await;

    // An admin cannot disable themselves, and can still log in.
    let (status, body) = send(
        &env.router,
        Method::PATCH,
        "/cube/v1/users/admin",
        Some(&admin),
        Some(json!({ "disabled": true })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    login_token(&env.router, ADMIN, ADMIN_PASSWORD).await;

    // An empty patch is refused.
    let (status, _) = send(
        &env.router,
        Method::PATCH,
        "/cube/v1/users/admin",
        Some(&admin),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Malformed JSON.
    let request = Request::builder()
        .method(Method::POST)
        .uri("/cube/v1/users")
        .header(header::AUTHORIZATION, format!("Bearer {admin}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{not json"))
        .unwrap();
    let response = env.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // A bad id on the token route.
    let (status, _) = send(
        &env.router,
        Method::POST,
        "/cube/v1/users/Ana/tokens",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
});

// ---- organizations and workspaces ----

/// The admin, `ana` (owner of organization A), `bia` and `caio`, all logged in.
struct Scenario {
    env: Env,
    admin: String,
    ana: String,
    bia: String,
    caio: String,
    org_a: String,
}

async fn scenario(store: Store) -> Scenario {
    scenario_with(store, ServerConfig::default()).await
}

async fn scenario_with(store: Store, server: ServerConfig) -> Scenario {
    let env = env_with_server(store, rbac_config(Duration::ZERO), server).await;
    scenario_from_env(env).await
}

async fn scenario_from_env(env: Env) -> Scenario {
    let admin = admin_token(&env).await;
    for id in ["ana", "bia", "caio"] {
        let (status, body) = create_user_as(&env, &admin, id, &format!("{id}-password-1")).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    let org_a = create_org(&env, &admin, "A", "ana").await;
    let ana = login_token(&env.router, "ana", "ana-password-1").await;
    let bia = login_token(&env.router, "bia", "bia-password-1").await;
    let caio = login_token(&env.router, "caio", "caio-password-1").await;
    Scenario {
        env,
        admin,
        ana,
        bia,
        caio,
        org_a,
    }
}

async fn create_org(env: &Env, token: &str, name: &str, owner: &str) -> String {
    let (status, body) = send(
        &env.router,
        Method::POST,
        "/cube/v1/organizations",
        Some(token),
        Some(json!({ "name": name, "owner": owner })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().expect("an organization id").to_string()
}

async fn create_workspace(env: &Env, token: &str, org: &str, name: &str) -> (StatusCode, Value) {
    send(
        &env.router,
        Method::POST,
        &format!("/cube/v1/organizations/{org}/workspaces"),
        Some(token),
        Some(json!({ "name": name })),
    )
    .await
}

async fn workspace_id(env: &Env, token: &str, org: &str, name: &str) -> String {
    let (status, body) = create_workspace(env, token, org, name).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().expect("a workspace id").to_string()
}

async fn set_role(env: &Env, token: &str, ws: &str, user: &str, role: &str) -> StatusCode {
    send(
        &env.router,
        Method::PUT,
        &format!("/cube/v1/workspaces/{ws}/members/{user}"),
        Some(token),
        Some(json!({ "role": role })),
    )
    .await
    .0
}

fn names(list: &Value, key: &str) -> Vec<String> {
    list[key]
        .as_array()
        .unwrap_or_else(|| panic!("no {key} in {list}"))
        .iter()
        .map(|o| o["name"].as_str().unwrap().to_string())
        .collect()
}

db_test!(org_owner_creates_workspaces_and_sees_them, |store| {
    let s = scenario(store).await;
    let env = &s.env;
    let w1 = workspace_id(env, &s.ana, &s.org_a, "W1").await;
    workspace_id(env, &s.ana, &s.org_a, "W2").await;
    // Same name in the same organization.
    let (status, _) = create_workspace(env, &s.ana, &s.org_a, "W1").await;
    assert_eq!(status, StatusCode::CONFLICT);
    // Only an organization admin creates workspaces.
    let (status, _) = create_workspace(env, &s.bia, &s.org_a, "W3").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, list) = send(
        &env.router,
        Method::GET,
        &format!("/cube/v1/organizations/{}/workspaces", s.org_a),
        Some(&s.ana),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(names(&list, "workspaces"), ["W1", "W2"]);

    let (status, ws) = send(
        &env.router,
        Method::GET,
        &format!("/cube/v1/workspaces/{w1}"),
        Some(&s.ana),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ws}");
    assert_eq!(ws["name"], "W1");
    assert_eq!(ws["organizationId"], json!(s.org_a));

    let (status, orgs) = send(
        &env.router,
        Method::GET,
        "/cube/v1/organizations",
        Some(&s.ana),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{orgs}");
    assert_eq!(names(&orgs, "organizations"), ["A"]);

    // Rename, then delete a workspace.
    let (status, renamed) = send(
        &env.router,
        Method::PATCH,
        &format!("/cube/v1/workspaces/{w1}"),
        Some(&s.ana),
        Some(json!({ "name": "W1b" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(renamed["name"], "W1b");
    let (status, _) = send(
        &env.router,
        Method::DELETE,
        &format!("/cube/v1/workspaces/{w1}"),
        Some(&s.ana),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(
        &env.router,
        Method::GET,
        &format!("/cube/v1/workspaces/{w1}"),
        Some(&s.ana),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(env
        .rbac
        .store
        .workspace(w1.parse().unwrap())
        .await
        .unwrap()
        .is_none());
});

async fn org_names(env: &Env, token: &str) -> Vec<String> {
    let (status, body) = send(
        &env.router,
        Method::GET,
        "/cube/v1/organizations",
        Some(token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    names(&body, "organizations")
}

db_test!(organizations_list_only_what_the_user_can_view, |store| {
    let s = scenario(store).await;
    let env = &s.env;
    create_org(env, &s.admin, "B", "bia").await;

    assert!(org_names(env, &s.caio).await.is_empty());
    assert_eq!(org_names(env, &s.ana).await, ["A"]);
    assert_eq!(org_names(env, &s.bia).await, ["B"]);
    assert_eq!(org_names(env, &s.admin).await, ["A", "B"]);

    // Only platform admins create organizations; the owner must exist.
    let (status, _) = send(
        &env.router,
        Method::POST,
        "/cube/v1/organizations",
        Some(&s.ana),
        Some(json!({ "name": "C", "owner": "ana" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &env.router,
        Method::POST,
        "/cube/v1/organizations",
        Some(&s.admin),
        Some(json!({ "name": "C", "owner": "nobody" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // `caio` cannot see A at all.
    let (status, _) = send(
        &env.router,
        Method::GET,
        &format!("/cube/v1/organizations/{}", s.org_a),
        Some(&s.caio),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
});

db_test!(a_member_of_one_workspace_does_not_see_the_others, |store| {
    let s = scenario(store).await;
    let env = &s.env;
    let w1 = workspace_id(env, &s.ana, &s.org_a, "W1").await;
    let w2 = workspace_id(env, &s.ana, &s.org_a, "W2").await;
    assert_eq!(
        set_role(env, &s.ana, &w1, "bia", "viewer").await,
        StatusCode::OK
    );

    let (status, list) = send(
        &env.router,
        Method::GET,
        &format!("/cube/v1/organizations/{}/workspaces", s.org_a),
        Some(&s.bia),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(names(&list, "workspaces"), ["W1"]);

    let (status, _) = send(
        &env.router,
        Method::GET,
        &format!("/cube/v1/workspaces/{w2}"),
        Some(&s.bia),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // A viewer sees W1 but cannot manage it.
    let (status, _) = send(
        &env.router,
        Method::PATCH,
        &format!("/cube/v1/workspaces/{w1}"),
        Some(&s.bia),
        Some(json!({ "name": "mine" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        set_role(env, &s.bia, &w1, "caio", "viewer").await,
        StatusCode::FORBIDDEN
    );
});

db_test!(role_change_replaces_the_old_role, |store| {
    let s = scenario(store).await;
    let env = &s.env;
    let w1 = workspace_id(env, &s.ana, &s.org_a, "W1").await;
    let ws = ObjectRef::new("workspace", w1.clone());
    assert_eq!(
        set_role(env, &s.ana, &w1, "bia", "editor").await,
        StatusCode::OK
    );
    assert!(env.authz.check(&ws, "edit", "bia").await.unwrap());
    assert_eq!(
        set_role(env, &s.ana, &w1, "bia", "viewer").await,
        StatusCode::OK
    );

    let (status, members) = send(
        &env.router,
        Method::GET,
        &format!("/cube/v1/workspaces/{w1}/members"),
        Some(&s.bia),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{members}");
    assert_eq!(
        members,
        json!({ "members": [{ "user": "bia", "role": "viewer" }] })
    );
    assert!(!env.authz.check(&ws, "edit", "bia").await.unwrap());
    assert!(env.authz.check(&ws, "view", "bia").await.unwrap());

    // Bad role, bad user id, unknown user.
    assert_eq!(
        set_role(env, &s.ana, &w1, "bia", "god").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        set_role(env, &s.ana, &w1, "Bia", "viewer").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        set_role(env, &s.ana, &w1, "nobody", "viewer").await,
        StatusCode::NOT_FOUND
    );

    // Removing the last role also removes the organization membership.
    let org = ObjectRef::new("organization", s.org_a.clone());
    assert!(env.authz.check(&org, "view", "bia").await.unwrap());
    let (status, _) = send(
        &env.router,
        Method::DELETE,
        &format!("/cube/v1/workspaces/{w1}/members/bia"),
        Some(&s.ana),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!env.authz.check(&ws, "view", "bia").await.unwrap());
    assert!(!env.authz.check(&org, "view", "bia").await.unwrap());
});

db_test!(
    membership_survives_while_another_workspace_role_remains,
    |store| {
        let s = scenario(store).await;
        let env = &s.env;
        let w1 = workspace_id(env, &s.ana, &s.org_a, "W1").await;
        let w2 = workspace_id(env, &s.ana, &s.org_a, "W2").await;
        set_role(env, &s.ana, &w1, "bia", "viewer").await;
        set_role(env, &s.ana, &w2, "bia", "owner").await;
        let (status, _) = send(
            &env.router,
            Method::DELETE,
            &format!("/cube/v1/workspaces/{w1}/members/bia"),
            Some(&s.ana),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let org = ObjectRef::new("organization", s.org_a.clone());
        assert!(env.authz.check(&org, "view", "bia").await.unwrap());
        // Deleting W2 takes the last role, and the membership, with it.
        let (status, _) = send(
            &env.router,
            Method::DELETE,
            &format!("/cube/v1/workspaces/{w2}"),
            Some(&s.ana),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(!env.authz.check(&org, "view", "bia").await.unwrap());
    }
);

db_test!(another_orgs_workspace_is_404, |store| {
    let s = scenario(store).await;
    let env = &s.env;
    let org_b = create_org(env, &s.admin, "B", "bia").await;
    let wb = workspace_id(env, &s.bia, &org_b, "WB").await;

    for (method, path) in [
        (Method::GET, format!("/cube/v1/workspaces/{wb}")),
        (Method::GET, format!("/cube/v1/workspaces/{wb}/members")),
        (Method::DELETE, format!("/cube/v1/workspaces/{wb}")),
        (Method::GET, format!("/cube/v1/organizations/{org_b}")),
        (
            Method::GET,
            format!("/cube/v1/organizations/{org_b}/workspaces"),
        ),
        (Method::GET, "/cube/v1/workspaces/not-a-uuid".to_string()),
    ] {
        let (status, _) = send(&env.router, method.clone(), &path, Some(&s.ana), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}");
    }
    assert_eq!(
        set_role(env, &s.ana, &wb, "ana", "owner").await,
        StatusCode::NOT_FOUND
    );
    let (status, _) = create_workspace(env, &s.ana, &org_b, "mine").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
});

db_test!(the_last_org_owner_cannot_be_removed, |store| {
    let s = scenario(store).await;
    let env = &s.env;
    let owners = format!("/cube/v1/organizations/{}/owners", s.org_a);
    let (status, _) = send(
        &env.router,
        Method::DELETE,
        &format!("{owners}/ana"),
        Some(&s.ana),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // With a second owner, the first can go.
    let (status, _) = send(
        &env.router,
        Method::PUT,
        &format!("{owners}/bia"),
        Some(&s.ana),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(
        &env.router,
        Method::DELETE,
        &format!("{owners}/ana"),
        Some(&s.bia),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(
        &env.router,
        Method::DELETE,
        &format!("{owners}/bia"),
        Some(&s.bia),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = send(
        &env.router,
        Method::PUT,
        &format!("{owners}/nobody"),
        Some(&s.bia),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // `ana` is no longer an owner.
    let (status, _) = send(
        &env.router,
        Method::GET,
        &format!("/cube/v1/organizations/{}", s.org_a),
        Some(&s.ana),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
});

db_test!(deleting_an_org_with_workspaces_is_refused, |store| {
    let s = scenario(store).await;
    let env = &s.env;
    let w1 = workspace_id(env, &s.ana, &s.org_a, "W1").await;
    let org = format!("/cube/v1/organizations/{}", s.org_a);
    let (status, body) = send(&env.router, Method::DELETE, &org, Some(&s.ana), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    // Nothing was revoked.
    let (status, _) = send(&env.router, Method::GET, &org, Some(&s.ana), None).await;
    assert_eq!(status, StatusCode::OK);

    send(
        &env.router,
        Method::DELETE,
        &format!("/cube/v1/workspaces/{w1}"),
        Some(&s.ana),
        None,
    )
    .await;
    let (status, renamed) = send(
        &env.router,
        Method::PATCH,
        &org,
        Some(&s.ana),
        Some(json!({ "name": "A2" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(renamed["name"], "A2");
    let (status, _) = send(&env.router, Method::DELETE, &org, Some(&s.ana), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(&env.router, Method::GET, &org, Some(&s.admin), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(env
        .rbac
        .store
        .organization(s.org_a.parse().unwrap())
        .await
        .unwrap()
        .is_none());
});

db_test!(a_failed_spicedb_write_leaves_no_row, |store| {
    let s = scenario(store).await;
    let env = &s.env;
    // SpiceDB down altogether: 503.
    env.authz.set_failing(true);
    let (status, _) = create_workspace(env, &s.ana, &s.org_a, "W1").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    env.authz.set_failing(false);
    // Checks pass but the relationship write fails: the row is rolled back.
    env.authz.set_failing_writes(true);
    let (status, _) = create_workspace(env, &s.ana, &s.org_a, "W1").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let (status, _) = send(
        &env.router,
        Method::POST,
        "/cube/v1/organizations",
        Some(&s.admin),
        Some(json!({ "name": "Z", "owner": "ana" })),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    env.authz.set_failing_writes(false);

    let (status, list) = send(
        &env.router,
        Method::GET,
        &format!("/cube/v1/organizations/{}/workspaces", s.org_a),
        Some(&s.ana),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert!(names(&list, "workspaces").is_empty());
    // The name is free: no row was left behind.
    workspace_id(env, &s.ana, &s.org_a, "W1").await;
    assert_eq!(org_names(env, &s.admin).await, ["A"]);
});

db_test!(a_workspace_owner_cannot_act_as_org_owner, |store| {
    let s = scenario(store).await;
    let env = &s.env;
    let w1 = workspace_id(env, &s.ana, &s.org_a, "W1").await;
    assert_eq!(
        set_role(env, &s.ana, &w1, "bia", "owner").await,
        StatusCode::OK
    );
    let org = format!("/cube/v1/organizations/{}", s.org_a);

    // Organization-level actions: visible (bia is a member), but forbidden.
    let (status, _) = send(
        &env.router,
        Method::PUT,
        &format!("{org}/owners/bia"),
        Some(&s.bia),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &env.router,
        Method::DELETE,
        &format!("{org}/owners/ana"),
        Some(&s.bia),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &env.router,
        Method::PATCH,
        &org,
        Some(&s.bia),
        Some(json!({ "name": "mine" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(&env.router, Method::DELETE, &org, Some(&s.bia), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &env.router,
        Method::DELETE,
        &format!("/cube/v1/workspaces/{w1}"),
        Some(&s.bia),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = create_workspace(env, &s.bia, &s.org_a, "W9").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let org_ref = ObjectRef::new("organization", s.org_a.clone());
    assert!(!env.authz.check(&org_ref, "admin", "bia").await.unwrap());

    // Workspace-level management is hers.
    let (status, _) = send(
        &env.router,
        Method::PATCH,
        &format!("/cube/v1/workspaces/{w1}"),
        Some(&s.bia),
        Some(json!({ "name": "W1b" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        set_role(env, &s.bia, &w1, "caio", "editor").await,
        StatusCode::OK
    );
    let (status, _) = send(
        &env.router,
        Method::DELETE,
        &format!("/cube/v1/workspaces/{w1}/members/caio"),
        Some(&s.bia),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
});

db_test!(a_viewer_cannot_remove_members, |store| {
    let s = scenario(store).await;
    let env = &s.env;
    let w1 = workspace_id(env, &s.ana, &s.org_a, "W1").await;
    set_role(env, &s.ana, &w1, "bia", "viewer").await;
    set_role(env, &s.ana, &w1, "caio", "editor").await;
    for target in ["caio", "bia"] {
        let (status, _) = send(
            &env.router,
            Method::DELETE,
            &format!("/cube/v1/workspaces/{w1}/members/{target}"),
            Some(&s.bia),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{target}");
    }
    // An editor cannot manage members either.
    let (status, _) = send(
        &env.router,
        Method::DELETE,
        &format!("/cube/v1/workspaces/{w1}/members/bia"),
        Some(&s.caio),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let ws = ObjectRef::new("workspace", w1.clone());
    assert!(env.authz.check(&ws, "edit", "caio").await.unwrap());
    assert!(env.authz.check(&ws, "view", "bia").await.unwrap());
});

db_test!(workspace_listings_stay_within_their_organization, |store| {
    let s = scenario(store).await;
    let env = &s.env;
    let org_b = create_org(env, &s.admin, "B", "bia").await;
    let wa = workspace_id(env, &s.ana, &s.org_a, "WA").await;
    let wb = workspace_id(env, &s.bia, &org_b, "WB").await;
    workspace_id(env, &s.ana, &s.org_a, "WA2").await;
    set_role(env, &s.ana, &wa, "caio", "viewer").await;
    set_role(env, &s.bia, &wb, "caio", "editor").await;

    assert_eq!(org_names(env, &s.caio).await, ["A", "B"]);
    for (org, expected) in [(&s.org_a, "WA"), (&org_b, "WB")] {
        let (status, list) = send(
            &env.router,
            Method::GET,
            &format!("/cube/v1/organizations/{org}/workspaces"),
            Some(&s.caio),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{list}");
        assert_eq!(names(&list, "workspaces"), [expected]);
    }
    // The platform admin sees every workspace, still split by organization.
    let (_, list) = send(
        &env.router,
        Method::GET,
        &format!("/cube/v1/organizations/{}/workspaces", s.org_a),
        Some(&s.admin),
        None,
    )
    .await;
    assert_eq!(names(&list, "workspaces"), ["WA", "WA2"]);
});

db_test!(a_spicedb_outage_fails_listings_closed, |store| {
    let s = scenario(store).await;
    let env = &s.env;
    workspace_id(env, &s.ana, &s.org_a, "W1").await;
    env.authz.set_failing(true);
    for path in [
        "/cube/v1/organizations".to_string(),
        format!("/cube/v1/organizations/{}/workspaces", s.org_a),
    ] {
        let (status, _) = send(&env.router, Method::GET, &path, Some(&s.ana), None).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{path}");
    }
    env.authz.set_failing(false);
});

db_test!(disabled_users_get_no_roles, |store| {
    let s = scenario(store).await;
    let env = &s.env;
    let w1 = workspace_id(env, &s.ana, &s.org_a, "W1").await;
    let (status, _) = send(
        &env.router,
        Method::PATCH,
        "/cube/v1/users/caio",
        Some(&s.admin),
        Some(json!({ "disabled": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        set_role(env, &s.ana, &w1, "caio", "viewer").await,
        StatusCode::BAD_REQUEST
    );
    let (status, body) = send(
        &env.router,
        Method::PUT,
        &format!("/cube/v1/organizations/{}/owners/caio", s.org_a),
        Some(&s.ana),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "User is disabled");
    let org = ObjectRef::new("organization", s.org_a.clone());
    assert!(!env.authz.check(&org, "view", "caio").await.unwrap());
});

// ---- connections ----

/// A scenario whose server reads a `cube.yml` (with the data sources
/// `default` and `shared`, a platform one to assign) and an `.env` from `dir`.
async fn connections_scenario(store: Store, dir: &tempfile::TempDir) -> Scenario {
    let file = dir.path().join("cube.yml");
    std::fs::write(
        &file,
        "version: 1\ndata_sources:\n  default:\n    type: postgres\n    host: db\n    password: secret\n  \
         shared:\n    type: postgres\n    host: db2\n    password: secret2\n",
    )
    .unwrap();
    std::fs::write(dir.path().join(".env"), "PORT=4000\n").unwrap();
    let server = ServerConfig {
        config_file: Some(file),
        schema_path: dir.path().join("model").to_string_lossy().into_owned(),
        ..ServerConfig::default()
    };
    scenario_with(store, server).await
}

/// `POST /v1/organizations/{org}/connections`: a SQLite data source in `dir`.
async fn add_connection(
    env: &Env,
    token: &str,
    org: &str,
    name: &str,
    dir: &tempfile::TempDir,
) -> (StatusCode, Value) {
    let database = dir.path().join(format!("{name}.db"));
    send(
        &env.router,
        Method::POST,
        &format!("/cube/v1/organizations/{org}/connections"),
        Some(token),
        Some(json!({
            "name": name,
            "variables": {
                "CUBEJS_DB_TYPE": "sqlite",
                "CUBEJS_DB_NAME": database.to_string_lossy(),
            }
        })),
    )
    .await
}

async fn release(env: &Env, token: &str, method: Method, ws: &str, name: &str) -> StatusCode {
    send(
        &env.router,
        method,
        &format!("/cube/v1/workspaces/{ws}/connections/{name}"),
        Some(token),
        None,
    )
    .await
    .0
}

async fn workspace_connections(env: &Env, token: &str, ws: &str) -> (StatusCode, Value) {
    send(
        &env.router,
        Method::GET,
        &format!("/cube/v1/workspaces/{ws}/connections"),
        Some(token),
        None,
    )
    .await
}

async fn released(env: &Env, ws: &str) -> Vec<String> {
    cubeserver::rbac::handlers::connections::released_connections(&env.rbac, ws)
        .await
        .unwrap()
        .into_iter()
        .collect()
}

db_test!(org_owner_adds_a_connection_and_releases_it, |store| {
    let dir = tempfile::tempdir().unwrap();
    let s = connections_scenario(store, &dir).await;
    let env = &s.env;
    let (status, body) = add_connection(env, &s.ana, &s.org_a, "x", &dir).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["name"], "x");
    let yml = std::fs::read_to_string(dir.path().join("cube.yml")).unwrap();
    assert!(yml.contains("  x:\n    type: sqlite\n"), "{yml}");
    // The same name again, as `POST /v1/data-sources` answers it.
    let (status, body) = add_connection(env, &s.ana, &s.org_a, "x", &dir).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("already exists"),
        "{body}"
    );
    // A data source that exists without an organization is not taken over.
    let (status, _) = add_connection(env, &s.ana, &s.org_a, "default", &dir).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(env
        .rbac
        .store
        .connection("default")
        .await
        .unwrap()
        .is_none());

    let w1 = workspace_id(env, &s.ana, &s.org_a, "W1").await;
    let w2 = workspace_id(env, &s.ana, &s.org_a, "W2").await;
    assert_eq!(
        release(env, &s.ana, Method::PUT, &w1, "x").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        set_role(env, &s.ana, &w1, "bia", "viewer").await,
        StatusCode::OK
    );

    for token in [&s.ana, &s.bia] {
        let (status, list) = workspace_connections(env, token, &w1).await;
        assert_eq!(status, StatusCode::OK, "{list}");
        assert_eq!(
            list,
            json!({ "connections": [{ "name": "x", "type": "sqlite" }] })
        );
    }
    let (status, list) = workspace_connections(env, &s.ana, &w2).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list, json!({ "connections": [] }));
    assert_eq!(released(env, &w1).await, ["x"]);

    let (status, list) = send(
        &env.router,
        Method::GET,
        &format!("/cube/v1/organizations/{}/connections", s.org_a),
        Some(&s.ana),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(
        list,
        json!({ "connections": [{ "name": "x", "type": "sqlite", "origin": "created", "workspaces": [w1] }] })
    );

    // Withdrawn, released again, then the connection is deleted.
    assert_eq!(
        release(env, &s.ana, Method::DELETE, &w1, "x").await,
        StatusCode::NO_CONTENT
    );
    assert!(released(env, &w1).await.is_empty());
    assert_eq!(
        release(env, &s.ana, Method::PUT, &w1, "x").await,
        StatusCode::NO_CONTENT
    );
    let (status, body) = send(
        &env.router,
        Method::DELETE,
        "/cube/v1/connections/x",
        Some(&s.ana),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let yml = std::fs::read_to_string(dir.path().join("cube.yml")).unwrap();
    assert!(!yml.contains("  x:"), "{yml}");
    assert!(env.rbac.store.connection("x").await.unwrap().is_none());
    assert!(released(env, &w1).await.is_empty());
    let connection = ObjectRef::new("connection", "x");
    assert!(!env.authz.check(&connection, "manage", "ana").await.unwrap());
});

db_test!(
    a_connection_cannot_be_released_to_another_orgs_workspace,
    |store| {
        let dir = tempfile::tempdir().unwrap();
        let s = connections_scenario(store, &dir).await;
        let env = &s.env;
        let (status, body) = add_connection(env, &s.ana, &s.org_a, "x", &dir).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let org_b = create_org(env, &s.admin, "B", "bia").await;
        let wb = workspace_id(env, &s.bia, &org_b, "WB").await;

        // Even a platform admin, who administers both organizations.
        for token in [&s.admin, &s.ana, &s.bia] {
            assert_eq!(
                release(env, token, Method::PUT, &wb, "x").await,
                StatusCode::NOT_FOUND
            );
        }
        assert!(released(env, &wb).await.is_empty());
    }
);

db_test!(workspace_members_cannot_manage_connections, |store| {
    let dir = tempfile::tempdir().unwrap();
    let s = connections_scenario(store, &dir).await;
    let env = &s.env;
    let (status, body) = add_connection(env, &s.ana, &s.org_a, "x", &dir).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let w1 = workspace_id(env, &s.ana, &s.org_a, "W1").await;
    assert_eq!(
        release(env, &s.ana, Method::PUT, &w1, "x").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        set_role(env, &s.ana, &w1, "bia", "owner").await,
        StatusCode::OK
    );

    // `bia` owns W1 and sees x through it, but administers nothing.
    let (status, list) = workspace_connections(env, &s.bia, &w1).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    for method in [Method::PUT, Method::DELETE] {
        assert_eq!(
            release(env, &s.bia, method.clone(), &w1, "x").await,
            StatusCode::FORBIDDEN,
            "{method}"
        );
    }
    let (status, _) = add_connection(env, &s.bia, &s.org_a, "y", &dir).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    for (method, path, body) in [
        (
            Method::GET,
            format!("/cube/v1/organizations/{}/connections", s.org_a),
            None,
        ),
        (
            Method::PUT,
            "/cube/v1/connections/x".to_string(),
            Some(json!({ "variables": {} })),
        ),
        (Method::DELETE, "/cube/v1/connections/x".to_string(), None),
    ] {
        let (status, _) = send(&env.router, method.clone(), &path, Some(&s.bia), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}");
    }
    assert!(env.rbac.store.connection("x").await.unwrap().is_some());
    assert_eq!(released(env, &w1).await, ["x"]);
});

async fn assign(env: &Env, token: &str, name: &str, org: &str) -> StatusCode {
    send(
        &env.router,
        Method::PUT,
        &format!("/cube/v1/connections/{name}/organization"),
        Some(token),
        Some(json!({ "organizationId": org })),
    )
    .await
    .0
}

async fn unassign(env: &Env, token: &str, name: &str) -> StatusCode {
    send(
        &env.router,
        Method::DELETE,
        &format!("/cube/v1/connections/{name}/organization"),
        Some(token),
        None,
    )
    .await
    .0
}

db_test!(assigning_an_existing_data_source_to_an_org, |store| {
    let dir = tempfile::tempdir().unwrap();
    let s = connections_scenario(store, &dir).await;
    let env = &s.env;
    // Platform admins only.
    assert_eq!(
        assign(env, &s.ana, "shared", &s.org_a).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        assign(env, &s.admin, "nope", &s.org_a).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        assign(
            env,
            &s.admin,
            "shared",
            "00000000-0000-0000-0000-000000000000"
        )
        .await,
        StatusCode::NOT_FOUND
    );
    // `default` is the platform's: never assigned.
    assert_eq!(
        assign(env, &s.admin, "default", &s.org_a).await,
        StatusCode::BAD_REQUEST
    );
    assert!(env
        .rbac
        .store
        .connection("default")
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        assign(env, &s.admin, "shared", &s.org_a).await,
        StatusCode::NO_CONTENT
    );
    // Again: a no-op.
    assert_eq!(
        assign(env, &s.admin, "shared", &s.org_a).await,
        StatusCode::NO_CONTENT
    );

    let row = env.rbac.store.connection("shared").await.unwrap().unwrap();
    assert_eq!(row.organization_id.to_string(), s.org_a);
    assert_eq!(row.origin, ConnectionOrigin::Assigned);
    let (status, list) = send(
        &env.router,
        Method::GET,
        &format!("/cube/v1/organizations/{}/connections", s.org_a),
        Some(&s.ana),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(
        list,
        json!({ "connections": [{
            "name": "shared", "type": "postgres", "origin": "assigned", "workspaces": []
        }] })
    );
    let w1 = workspace_id(env, &s.ana, &s.org_a, "W1").await;
    assert_eq!(
        release(env, &s.ana, Method::PUT, &w1, "shared").await,
        StatusCode::NO_CONTENT
    );

    // Owned by A: another organization gets it only after an unassign.
    let org_b = create_org(env, &s.admin, "B", "bia").await;
    assert_eq!(
        assign(env, &s.admin, "shared", &org_b).await,
        StatusCode::CONFLICT
    );
    assert_eq!(released(env, &w1).await, ["shared"]);
    let row = env.rbac.store.connection("shared").await.unwrap().unwrap();
    assert_eq!(row.organization_id.to_string(), s.org_a);
    assert_eq!(
        unassign(env, &s.admin, "shared").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        assign(env, &s.admin, "shared", &org_b).await,
        StatusCode::NO_CONTENT
    );
    assert!(released(env, &w1).await.is_empty());
    let connection = ObjectRef::new("connection", "shared");
    assert!(!env.authz.check(&connection, "manage", "ana").await.unwrap());
    assert!(env.authz.check(&connection, "manage", "bia").await.unwrap());
    let row = env.rbac.store.connection("shared").await.unwrap().unwrap();
    assert_eq!(row.organization_id.to_string(), org_b);
});

db_test!(
    org_owners_cannot_edit_or_delete_an_assigned_connection,
    |store| {
        let dir = tempfile::tempdir().unwrap();
        let s = connections_scenario(store, &dir).await;
        let env = &s.env;
        assert_eq!(
            assign(env, &s.admin, "shared", &s.org_a).await,
            StatusCode::NO_CONTENT
        );
        let yml = std::fs::read_to_string(dir.path().join("cube.yml")).unwrap();

        // `ana` administers A and could manage the connection in SpiceDB, but
        // its settings are the platform's.
        let connection = ObjectRef::new("connection", "shared");
        assert!(env.authz.check(&connection, "manage", "ana").await.unwrap());
        let (status, body) = send(
            &env.router,
            Method::PUT,
            "/cube/v1/connections/shared",
            Some(&s.ana),
            Some(json!({ "variables": { "CUBEJS_DB_HOST": "attacker.example" } })),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        let (status, body) = send(
            &env.router,
            Method::DELETE,
            "/cube/v1/connections/shared",
            Some(&s.ana),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("cube.yml")).unwrap(),
            yml
        );
        assert!(env.rbac.store.connection("shared").await.unwrap().is_some());

        // She still releases and withdraws it.
        let w1 = workspace_id(env, &s.ana, &s.org_a, "W1").await;
        for method in [Method::PUT, Method::DELETE] {
            assert_eq!(
                release(env, &s.ana, method.clone(), &w1, "shared").await,
                StatusCode::NO_CONTENT,
                "{method}"
            );
        }

        // A connection A created stays hers to delete; platform admins delete
        // the assigned one.
        let (status, body) = add_connection(env, &s.ana, &s.org_a, "x", &dir).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!(
            env.rbac
                .store
                .connection("x")
                .await
                .unwrap()
                .unwrap()
                .origin,
            ConnectionOrigin::Created
        );
        for (token, name) in [(&s.ana, "x"), (&s.admin, "shared")] {
            let (status, body) = send(
                &env.router,
                Method::DELETE,
                &format!("/cube/v1/connections/{name}"),
                Some(token),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{name}: {body}");
            assert!(env.rbac.store.connection(name).await.unwrap().is_none());
        }
    }
);

db_test!(unassigning_frees_the_connection_and_the_org, |store| {
    let dir = tempfile::tempdir().unwrap();
    let s = connections_scenario(store, &dir).await;
    let env = &s.env;
    let org_b = create_org(env, &s.admin, "B", "bia").await;
    let bia = login_token(&env.router, "bia", "bia-password-1").await;
    assert_eq!(
        assign(env, &s.admin, "shared", &org_b).await,
        StatusCode::NO_CONTENT
    );
    let wb = workspace_id(env, &bia, &org_b, "WB").await;
    assert_eq!(
        release(env, &bia, Method::PUT, &wb, "shared").await,
        StatusCode::NO_CONTENT
    );

    // Platform admins only; a data source without an organization is 404.
    assert_eq!(unassign(env, &bia, "shared").await, StatusCode::FORBIDDEN);
    assert_eq!(
        unassign(env, &s.admin, "default").await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(unassign(env, &s.admin, "nope").await, StatusCode::NOT_FOUND);

    let delete_org = || async {
        send(
            &env.router,
            Method::DELETE,
            &format!("/cube/v1/organizations/{org_b}"),
            Some(&bia),
            None,
        )
        .await
        .0
    };
    let (status, _) = send(
        &env.router,
        Method::DELETE,
        &format!("/cube/v1/workspaces/{wb}"),
        Some(&bia),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    // The connection still holds B.
    assert_eq!(delete_org().await, StatusCode::BAD_REQUEST);

    let wb2 = workspace_id(env, &bia, &org_b, "WB2").await;
    assert_eq!(
        release(env, &bia, Method::PUT, &wb2, "shared").await,
        StatusCode::NO_CONTENT
    );
    // A failing SpiceDB write keeps the row: nothing changed.
    env.authz.set_failing_writes(true);
    assert_eq!(
        unassign(env, &s.admin, "shared").await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    env.authz.set_failing_writes(false);
    assert!(env.rbac.store.connection("shared").await.unwrap().is_some());
    assert_eq!(released(env, &wb2).await, ["shared"]);

    assert_eq!(
        unassign(env, &s.admin, "shared").await,
        StatusCode::NO_CONTENT
    );
    assert!(env.rbac.store.connection("shared").await.unwrap().is_none());
    assert!(released(env, &wb2).await.is_empty());
    let all = env
        .authz
        .read(&RelationshipFilter {
            kind: "connection",
            id: Some("shared".into()),
            relation: None,
            subject: None,
        })
        .await
        .unwrap();
    assert!(all.is_empty(), "{all:?}");
    // The data source stays in cube.yml.
    let yml = std::fs::read_to_string(dir.path().join("cube.yml")).unwrap();
    assert!(yml.contains("  shared:"), "{yml}");

    let (status, _) = send(
        &env.router,
        Method::DELETE,
        &format!("/cube/v1/workspaces/{wb2}"),
        Some(&bia),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(delete_org().await, StatusCode::NO_CONTENT);
});

db_test!(data_sources_routes_are_admin_only_with_rbac, |store| {
    let dir = tempfile::tempdir().unwrap();
    let s = connections_scenario(store, &dir).await;
    let env = &s.env;
    let (status, body) = send(
        &env.router,
        Method::GET,
        "/cube/v1/data-sources",
        Some(&s.ana),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    for (method, path) in [
        (Method::GET, "/cube/v1/data-sources/default"),
        (Method::DELETE, "/cube/v1/data-sources/default"),
    ] {
        let (status, _) = send(&env.router, method.clone(), path, Some(&s.ana), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}");
    }
    let (status, body) = send(
        &env.router,
        Method::GET,
        "/cube/v1/data-sources",
        Some(&s.admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
});

db_test!(another_orgs_connection_is_404, |store| {
    let dir = tempfile::tempdir().unwrap();
    let s = connections_scenario(store, &dir).await;
    let env = &s.env;
    let (status, body) = add_connection(env, &s.ana, &s.org_a, "x", &dir).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let w1 = workspace_id(env, &s.ana, &s.org_a, "W1").await;
    assert_eq!(
        release(env, &s.ana, Method::PUT, &w1, "x").await,
        StatusCode::NO_CONTENT
    );
    let org_b = create_org(env, &s.admin, "B", "bia").await;
    let wb = workspace_id(env, &s.bia, &org_b, "WB").await;

    for (method, path, body) in [
        (
            Method::GET,
            format!("/cube/v1/organizations/{}/connections", s.org_a),
            None,
        ),
        (
            Method::POST,
            format!("/cube/v1/organizations/{}/connections", s.org_a),
            Some(json!({ "name": "y", "variables": { "CUBEJS_DB_TYPE": "sqlite" } })),
        ),
        (
            Method::PUT,
            "/cube/v1/connections/x".to_string(),
            Some(json!({ "variables": {} })),
        ),
        (Method::DELETE, "/cube/v1/connections/x".to_string(), None),
        (
            Method::PUT,
            "/cube/v1/connections/nope".to_string(),
            Some(json!({ "variables": {} })),
        ),
        (
            Method::GET,
            format!("/cube/v1/workspaces/{w1}/connections"),
            None,
        ),
        (
            Method::PUT,
            format!("/cube/v1/workspaces/{w1}/connections/x"),
            None,
        ),
        (
            Method::DELETE,
            format!("/cube/v1/workspaces/{w1}/connections/x"),
            None,
        ),
        (
            Method::PUT,
            format!("/cube/v1/workspaces/{wb}/connections/x"),
            None,
        ),
        (
            Method::DELETE,
            format!("/cube/v1/workspaces/{wb}/connections/x"),
            None,
        ),
    ] {
        let (status, _) = send(&env.router, method.clone(), &path, Some(&s.bia), body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}");
    }
    // Listings never show it.
    let (status, list) = send(
        &env.router,
        Method::GET,
        &format!("/cube/v1/organizations/{org_b}/connections"),
        Some(&s.bia),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list, json!({ "connections": [] }));
    let (status, list) = workspace_connections(env, &s.bia, &wb).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list, json!({ "connections": [] }));
    assert_eq!(released(env, &w1).await, ["x"]);
});

db_test!(a_failed_spicedb_write_adds_no_connection, |store| {
    let dir = tempfile::tempdir().unwrap();
    let s = connections_scenario(store, &dir).await;
    let env = &s.env;
    let before = std::fs::read_to_string(dir.path().join("cube.yml")).unwrap();
    env.authz.set_failing_writes(true);
    let (status, _) = add_connection(env, &s.ana, &s.org_a, "x", &dir).await;
    env.authz.set_failing_writes(false);
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(env.rbac.store.connection("x").await.unwrap().is_none());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("cube.yml")).unwrap(),
        before
    );

    // A connection that does not connect leaves nothing either.
    let (status, _) = send(
        &env.router,
        Method::POST,
        &format!("/cube/v1/organizations/{}/connections", s.org_a),
        Some(&s.ana),
        Some(json!({
            "name": "broken",
            "variables": {
                "CUBEJS_DB_TYPE": "postgres",
                "CUBEJS_DB_HOST": "127.0.0.1",
                "CUBEJS_DB_PORT": "1",
                "CUBEJS_DB_NAME": "x",
                "CUBEJS_DB_USER": "x",
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(env.rbac.store.connection("broken").await.unwrap().is_none());
    let connection = ObjectRef::new("connection", "broken");
    assert!(!env.authz.check(&connection, "manage", "ana").await.unwrap());
});

db_test!(a_recreated_data_source_inherits_no_grant, |store| {
    let dir = tempfile::tempdir().unwrap();
    let s = connections_scenario(store, &dir).await;
    let env = &s.env;
    let (status, body) = add_connection(env, &s.ana, &s.org_a, "x", &dir).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let w1 = workspace_id(env, &s.ana, &s.org_a, "W1").await;
    assert_eq!(
        release(env, &s.ana, Method::PUT, &w1, "x").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        set_role(env, &s.ana, &w1, "bia", "viewer").await,
        StatusCode::OK
    );

    // A platform admin deletes x through /v1/data-sources and makes a new x.
    let (status, body) = send(
        &env.router,
        Method::DELETE,
        "/cube/v1/data-sources/x",
        Some(&s.admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(env.rbac.store.connection("x").await.unwrap().is_none());
    let database = dir.path().join("x2.db");
    let (status, body) = send(
        &env.router,
        Method::POST,
        "/cube/v1/data-sources",
        Some(&s.admin),
        Some(json!({
            "name": "x",
            "variables": {
                "CUBEJS_DB_TYPE": "sqlite",
                "CUBEJS_DB_NAME": database.to_string_lossy(),
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    assert!(released(env, &w1).await.is_empty());
    let (status, _) = send(
        &env.router,
        Method::PUT,
        "/cube/v1/connections/x",
        Some(&s.ana),
        Some(json!({ "variables": {} })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, list) = workspace_connections(env, &s.bia, &w1).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list, json!({ "connections": [] }));
    let connection = ObjectRef::new("connection", "x");
    assert!(!env.authz.check(&connection, "query", "bia").await.unwrap());
});

db_test!(a_data_source_never_adopts_a_stale_row, |store| {
    let dir = tempfile::tempdir().unwrap();
    let s = connections_scenario(store, &dir).await;
    let env = &s.env;
    // A row left behind (its data source removed by hand, say).
    env.rbac
        .store
        .create_connection("z", s.org_a.parse().unwrap(), ConnectionOrigin::Assigned)
        .await
        .unwrap();
    let (status, body) = send(
        &env.router,
        Method::POST,
        "/cube/v1/data-sources",
        Some(&s.admin),
        Some(json!({
            "name": "z",
            "variables": {
                "CUBEJS_DB_TYPE": "sqlite",
                "CUBEJS_DB_NAME": dir.path().join("z.db").to_string_lossy(),
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let yml = std::fs::read_to_string(dir.path().join("cube.yml")).unwrap();
    assert!(!yml.contains("  z:"), "{yml}");
});

db_test!(another_orgs_release_is_never_listed, |store| {
    let dir = tempfile::tempdir().unwrap();
    let s = connections_scenario(store, &dir).await;
    let env = &s.env;
    let w1 = workspace_id(env, &s.ana, &s.org_a, "W1").await;
    let org_b = create_org(env, &s.admin, "B", "bia").await;
    let (status, _) = send(
        &env.router,
        Method::PUT,
        "/cube/v1/connections/shared/organization",
        Some(&s.admin),
        Some(json!({ "organizationId": org_b })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    // A release that should never have been written: B's connection on
    // A's workspace.
    env.authz
        .touch(&[Relationship {
            resource: ObjectRef::new("connection", "shared"),
            relation: "workspace",
            subject: ObjectRef::new("workspace", w1.clone()),
        }])
        .await
        .unwrap();

    assert!(released(env, &w1).await.is_empty());
    let (status, list) = workspace_connections(env, &s.ana, &w1).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list, json!({ "connections": [] }));

    // Failures answer 503, never a partial list.
    env.authz.set_failing(true);
    let err = cubeserver::rbac::handlers::connections::released_connections(&env.rbac, &w1)
        .await
        .unwrap_err();
    env.authz.set_failing(false);
    assert_eq!(err.status, StatusCode::SERVICE_UNAVAILABLE);
});

db_test!(a_release_needs_the_connection_still_in_the_org, |store| {
    let dir = tempfile::tempdir().unwrap();
    let s = connections_scenario(store, &dir).await;
    let env = &s.env;
    let (status, body) = add_connection(env, &s.ana, &s.org_a, "x", &dir).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let w1 = workspace_id(env, &s.ana, &s.org_a, "W1").await;
    // SpiceDB already moved x away (a reassignment midway): the row still
    // says A, the relationship no longer does.
    let org_rel = Relationship {
        resource: ObjectRef::new("connection", "x"),
        relation: "organization",
        subject: ObjectRef::new("organization", s.org_a.clone()),
    };
    env.authz.delete(&[org_rel]).await.unwrap();

    assert_eq!(
        release(env, &s.ana, Method::PUT, &w1, "x").await,
        StatusCode::CONFLICT
    );
    let releases = env
        .authz
        .read(&RelationshipFilter {
            kind: "connection",
            id: Some("x".into()),
            relation: Some("workspace"),
            subject: None,
        })
        .await
        .unwrap();
    assert!(releases.is_empty(), "{releases:?}");
});

// ---- objects and per-workspace data access ----

const DATA_MODEL: &str = r#"
cubes:
  - name: a_orders
    data_source: a_sqlite
    sql_table: orders
    joins:
      - name: b_orders
        sql: "{CUBE}.id = {b_orders}.id"
        relationship: many_to_one
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
      - name: status
        sql: status
        type: string
    measures:
      - name: count
        type: count

  - name: b_orders
    data_source: b_sqlite
    sql_table: orders
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
      - name: status
        sql: status
        type: string
    measures:
      - name: count
        type: count

  - name: c_orders
    data_source: c_sqlite
    sql_table: orders
    measures:
      - name: count
        type: count

  # No data_source: the `default` one, released like any other.
  - name: plain_orders
    sql_table: orders
    measures:
      - name: count
        type: count

views:
  - name: b_view
    cubes:
      - join_path: b_orders
        includes: [status, count]

  # Spans a_sqlite and b_sqlite.
  - name: mixed_view
    cubes:
      - join_path: a_orders
        includes: [count]
      - join_path: a_orders.b_orders
        prefix: true
        includes: [status]
"#;

/// Answers every query with no rows and keeps what it was asked. Its
/// data-source snapshot stands for the planner's.
#[derive(Debug)]
struct RecordingQuery {
    seen: std::sync::Mutex<Vec<Value>>,
    sources: Arc<DataSources>,
}

impl RecordingQuery {
    fn new(model: &std::path::Path) -> Self {
        Self {
            seen: Default::default(),
            sources: Arc::new(
                DataSources::load(model, &cubemodel::TemplateContext::default()).unwrap(),
            ),
        }
    }

    fn record(&self, request: &NormalizedRequest) -> Value {
        let queries: Vec<Value> = request
            .queries
            .iter()
            .map(|q| serde_json::to_value(q).unwrap())
            .collect();
        self.seen.lock().unwrap().extend(queries.iter().cloned());
        queries.into_iter().next().unwrap_or(Value::Null)
    }

    fn seen(&self) -> Vec<Value> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait]
impl QueryService for RecordingQuery {
    fn data_sources(&self) -> Option<Arc<DataSources>> {
        Some(self.sources.clone())
    }

    async fn load(
        &self,
        _: &RequestContext,
        request: NormalizedRequest,
    ) -> Result<Value, ApiError> {
        let query = self.record(&request);
        Ok(json!({ "data": [], "query": query }))
    }

    async fn sql(&self, _: &RequestContext, request: NormalizedRequest) -> Result<Value, ApiError> {
        self.record(&request);
        Ok(json!({ "sql": { "sql": ["SELECT 1", []] } }))
    }

    async fn dry_run(
        &self,
        _: &RequestContext,
        request: NormalizedRequest,
    ) -> Result<Value, ApiError> {
        self.record(&request);
        Ok(json!({ "queryType": "regularQuery" }))
    }
}

struct DataScenario {
    s: Scenario,
    _dir: tempfile::TempDir,
    /// Owned by ana (organization A); `a_sqlite` is released to it; bia is
    /// a viewer and caio an editor.
    w1: String,
    /// Organization A too; `b_sqlite` is released to it; nobody else is in it.
    w2: String,
    queries: Arc<RecordingQuery>,
    /// The server's state, for the WebSocket transport.
    state: AppState,
}

/// Organization A with the connections `a_sqlite` and `b_sqlite`, a server
/// whose model has `a_orders` (on `a_sqlite`), `b_orders` and `b_view` (on
/// `b_sqlite`), and a query service that records what reaches it.
async fn data_scenario(store: Store) -> DataScenario {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("cube.yml");
    std::fs::write(
        &file,
        "version: 1\ndata_sources:\n  default:\n    type: postgres\n    host: db\n    password: secret\n",
    )
    .unwrap();
    std::fs::write(dir.path().join(".env"), "PORT=4000\n").unwrap();
    let model = dir.path().join("model");
    std::fs::create_dir(&model).unwrap();
    std::fs::write(model.join("model.yml"), DATA_MODEL).unwrap();
    let server = ServerConfig {
        config_file: Some(file),
        schema_path: model.to_string_lossy().into_owned(),
        ..ServerConfig::default()
    };

    let queries = Arc::new(RecordingQuery::new(&model));
    let authz = Arc::new(FakeAuthz::new());
    let rbac = Arc::new(
        Rbac::with_parts(authz.clone(), store, rbac_config(Duration::ZERO))
            .await
            .expect("rbac starts"),
    );
    let mut state = state_with(Some(rbac.clone()), server);
    state.query = queries.clone();
    state.meta = Arc::new(cubeserver::meta_adapter::ModelMetaService::load(&model, false).unwrap());
    let env = Env {
        router: build_app(state.clone()),
        rbac,
        authz,
    };
    let s = scenario_from_env(env).await;
    let env = &s.env;
    for name in ["a_sqlite", "b_sqlite"] {
        let (status, body) = add_connection(env, &s.ana, &s.org_a, name, &dir).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    let w1 = workspace_id(env, &s.ana, &s.org_a, "W1").await;
    let w2 = workspace_id(env, &s.ana, &s.org_a, "W2").await;
    assert_eq!(
        release(env, &s.ana, Method::PUT, &w1, "a_sqlite").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        release(env, &s.ana, Method::PUT, &w2, "b_sqlite").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        set_role(env, &s.ana, &w1, "bia", "viewer").await,
        StatusCode::OK
    );
    assert_eq!(
        set_role(env, &s.ana, &w1, "caio", "editor").await,
        StatusCode::OK
    );
    DataScenario {
        s,
        _dir: dir,
        w1,
        w2,
        queries,
        state,
    }
}

struct Answer {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    bytes: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.bytes).unwrap_or(Value::Null)
    }
}

/// A request with extra headers, answered with its raw body.
async fn call(
    router: &axum::Router,
    method: Method,
    path: &str,
    token: Option<&str>,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> Answer {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    for (name, value) in headers {
        request = request.header(*name, *value);
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
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    Answer {
        status,
        headers,
        bytes,
    }
}

/// `POST /v1/load` in `workspace` (no header when `None`).
async fn load_in(
    env: &Env,
    token: &str,
    workspace: Option<&str>,
    query: Value,
) -> (StatusCode, Value) {
    let headers: Vec<(&str, &str)> = workspace
        .map(|ws| vec![("x-cube-workspace", ws)])
        .unwrap_or_default();
    let answer = call(
        &env.router,
        Method::POST,
        "/cube/v1/load",
        Some(token),
        &headers,
        Some(json!({ "query": query })),
    )
    .await;
    (answer.status, answer.json())
}

async fn meta_cubes(env: &Env, token: &str, workspace: Option<&str>) -> (StatusCode, Vec<String>) {
    let headers: Vec<(&str, &str)> = workspace
        .map(|ws| vec![("x-cube-workspace", ws)])
        .unwrap_or_default();
    let answer = call(
        &env.router,
        Method::GET,
        "/cube/v1/meta",
        Some(token),
        &headers,
        None,
    )
    .await;
    let body = answer.json();
    let mut cubes: Vec<String> = body["cubes"]
        .as_array()
        .map(|cubes| {
            cubes
                .iter()
                .map(|c| c["name"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default();
    cubes.sort();
    (answer.status, cubes)
}

async fn create_object(env: &Env, token: &str, ws: &str, body: Value) -> (StatusCode, Value) {
    send(
        &env.router,
        Method::POST,
        &format!("/cube/v1/workspaces/{ws}/objects"),
        Some(token),
        Some(body),
    )
    .await
}

fn pipeline(name: &str, content: &str) -> Value {
    json!({ "type": "pipeline", "name": name, "contentType": "application/json", "content": content })
}

db_test!(
    editor_creates_and_updates_objects_viewer_only_reads,
    |store| {
        let d = data_scenario(store).await;
        let (env, w1) = (&d.s.env, &d.w1);

        let (status, _) = create_object(env, &d.s.bia, w1, pipeline("p", "{}")).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, created) = create_object(env, &d.s.caio, w1, pipeline("p", "{\"a\":1}")).await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        assert_eq!(created["version"], json!(1));
        assert_eq!(created["workspaceId"], json!(w1));
        assert_eq!(created["createdBy"], "caio");
        let id = created["id"].as_str().unwrap().to_string();
        let path = format!("/cube/v1/objects/{id}");

        let (status, read) = send(&env.router, Method::GET, &path, Some(&d.s.bia), None).await;
        assert_eq!(status, StatusCode::OK, "{read}");
        assert_eq!(read["content"], "{\"a\":1}");
        assert_eq!(read["name"], "p");

        let update = json!({ "content": "{\"a\":2}", "version": 1 });
        let (status, _) = send(
            &env.router,
            Method::PUT,
            &path,
            Some(&d.s.bia),
            Some(update.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = send(&env.router, Method::DELETE, &path, Some(&d.s.bia), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, updated) = send(
            &env.router,
            Method::PUT,
            &path,
            Some(&d.s.caio),
            Some(update),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{updated}");
        assert_eq!(updated["version"], json!(2));
        assert_eq!(updated["updatedBy"], "caio");
        let (_, read) = send(&env.router, Method::GET, &path, Some(&d.s.bia), None).await;
        assert_eq!(read["content"], "{\"a\":2}");

        let (status, _) = send(&env.router, Method::DELETE, &path, Some(&d.s.caio), None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = send(&env.router, Method::GET, &path, Some(&d.s.caio), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(env
            .rbac
            .store
            .object(id.parse().unwrap())
            .await
            .unwrap()
            .is_none());
    }
);

db_test!(stale_version_is_409, |store| {
    let d = data_scenario(store).await;
    let env = &d.s.env;
    let (_, created) = create_object(env, &d.s.caio, &d.w1, pipeline("p", "1")).await;
    let path = format!("/cube/v1/objects/{}", created["id"].as_str().unwrap());
    let (status, _) = send(
        &env.router,
        Method::PUT,
        &path,
        Some(&d.s.caio),
        Some(json!({ "content": "2", "version": 1 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // A second writer still holding version 1.
    let (status, body) = send(
        &env.router,
        Method::PUT,
        &path,
        Some(&d.s.ana),
        Some(json!({ "content": "3", "version": 1 })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (_, read) = send(&env.router, Method::GET, &path, Some(&d.s.caio), None).await;
    assert_eq!(read["content"], "2");
    assert_eq!(read["version"], json!(2));
    // `version` is required.
    let (status, _) = send(
        &env.router,
        Method::PUT,
        &path,
        Some(&d.s.caio),
        Some(json!({ "content": "4" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
});

db_test!(object_listing_filters_by_type_and_omits_content, |store| {
    let d = data_scenario(store).await;
    let env = &d.s.env;
    create_object(env, &d.s.caio, &d.w1, pipeline("p1", "secret-1")).await;
    create_object(env, &d.s.caio, &d.w1, pipeline("p2", "secret-2")).await;
    let (status, _) = create_object(
        env,
        &d.s.caio,
        &d.w1,
        json!({ "type": "dashboard", "name": "d", "contentType": "text/plain", "content": "secret-3" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    // Another workspace's object never shows up.
    create_object(env, &d.s.ana, &d.w2, pipeline("other", "secret-4")).await;

    let (status, list) = send(
        &env.router,
        Method::GET,
        &format!("/cube/v1/workspaces/{}/objects?type=pipeline", d.w1),
        Some(&d.s.bia),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(names(&list, "objects"), ["p1", "p2"]);
    assert!(!list.to_string().contains("secret"), "{list}");
    assert!(list["objects"][0]["content"].is_null(), "{list}");

    let (_, list) = send(
        &env.router,
        Method::GET,
        &format!("/cube/v1/workspaces/{}/objects", d.w1),
        Some(&d.s.bia),
        None,
    )
    .await;
    assert_eq!(names(&list, "objects"), ["d", "p1", "p2"]);
    // bia is not in W2.
    let (status, _) = send(
        &env.router,
        Method::GET,
        &format!("/cube/v1/workspaces/{}/objects", d.w2),
        Some(&d.s.bia),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
});

db_test!(raw_content_with_accept_header, |store| {
    let d = data_scenario(store).await;
    let env = &d.s.env;
    let xml = "<pipeline><step name=\"a\"/></pipeline>";
    let (status, created) = create_object(
        env,
        &d.s.caio,
        &d.w1,
        json!({ "type": "pipeline", "name": "x", "contentType": "application/xml", "content": xml }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let path = format!("/cube/v1/objects/{}", created["id"].as_str().unwrap());

    let raw = call(
        &env.router,
        Method::GET,
        &path,
        Some(&d.s.bia),
        &[("accept", "application/xml")],
        None,
    )
    .await;
    assert_eq!(raw.status, StatusCode::OK);
    assert_eq!(raw.headers[header::CONTENT_TYPE], "application/xml");
    assert_eq!(raw.bytes, xml.as_bytes());

    // Any other Accept gets the JSON envelope.
    let wrapped = call(
        &env.router,
        Method::GET,
        &path,
        Some(&d.s.bia),
        &[("accept", "application/json")],
        None,
    )
    .await;
    assert_eq!(wrapped.status, StatusCode::OK);
    assert_eq!(wrapped.json()["content"], xml);
    assert_eq!(wrapped.json()["contentType"], "application/xml");

    // A content type that could not be a header value is refused.
    let (status, _) = create_object(
        env,
        &d.s.caio,
        &d.w1,
        json!({ "type": "pipeline", "name": "y", "contentType": "text/plain\r\nX-Evil: 1", "content": "" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
});

db_test!(base64_content_roundtrips_binary, |store| {
    use base64::Engine;
    let d = data_scenario(store).await;
    let env = &d.s.env;
    let bytes: Vec<u8> = vec![0, 159, 146, 150, 255, 10, 13];
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let (status, created) = create_object(
        env,
        &d.s.caio,
        &d.w1,
        json!({ "type": "model", "name": "bin", "contentType": "application/octet-stream",
                "content": encoded, "encoding": "base64" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id: uuid::Uuid = created["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        env.rbac.store.object(id).await.unwrap().unwrap().content,
        bytes
    );
    let path = format!("/cube/v1/objects/{id}");

    let (_, read) = send(&env.router, Method::GET, &path, Some(&d.s.bia), None).await;
    assert_eq!(read["encoding"], "base64");
    assert_eq!(read["content"], json!(encoded));

    let raw = call(
        &env.router,
        Method::GET,
        &path,
        Some(&d.s.bia),
        &[("accept", "application/octet-stream")],
        None,
    )
    .await;
    assert_eq!(raw.bytes, bytes);

    // Malformed base64 is a 400, not stored garbage.
    let (status, _) = send(
        &env.router,
        Method::PUT,
        &path,
        Some(&d.s.caio),
        Some(json!({ "content": "not base64!", "encoding": "base64", "version": 1 })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
});

db_test!(another_orgs_object_is_404, |store| {
    let d = data_scenario(store).await;
    let env = &d.s.env;
    let (status, body) = create_user_as(env, &d.s.admin, "dan", "dan-password-1").await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let dan = login_token(&env.router, "dan", "dan-password-1").await;
    let org_b = create_org(env, &d.s.admin, "B", "dan").await;
    let wb = workspace_id(env, &dan, &org_b, "WB").await;
    let (status, created) = create_object(env, &dan, &wb, pipeline("theirs", "secret")).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let path = format!("/cube/v1/objects/{}", created["id"].as_str().unwrap());

    // Neither a member of organization A nor its owner sees it.
    for token in [&d.s.caio, &d.s.ana] {
        let (status, body) = send(&env.router, Method::GET, &path, Some(token), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        assert!(!body.to_string().contains("secret"));
        let (status, _) = send(
            &env.router,
            Method::PUT,
            &path,
            Some(token),
            Some(json!({ "content": "mine", "version": 1 })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = send(&env.router, Method::DELETE, &path, Some(token), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = create_object(env, token, &wb, pipeline("intruder", "x")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    // Unknown and malformed ids look the same.
    for path in [
        format!("/cube/v1/objects/{}", uuid::Uuid::new_v4()),
        "/cube/v1/objects/not-a-uuid".to_string(),
    ] {
        let (status, _) = send(&env.router, Method::GET, &path, Some(&d.s.caio), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
    // The platform admin sees it.
    let (status, _) = send(&env.router, Method::GET, &path, Some(&d.s.admin), None).await;
    assert_eq!(status, StatusCode::OK);
    let (_, read) = send(&env.router, Method::GET, &path, Some(&dan), None).await;
    assert_eq!(read["content"], "secret");
});

db_test!(a_query_on_a_released_connection_runs, |store| {
    let d = data_scenario(store).await;
    let env = &d.s.env;
    let query = json!({ "measures": ["a_orders.count"], "dimensions": ["a_orders.status"] });
    let (status, body) = load_in(env, &d.s.bia, Some(&d.w1), query.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(d.queries.seen().len(), 1);
    assert_eq!(d.queries.seen()[0]["measures"], json!(["a_orders.count"]));
    for path in ["/cube/v1/sql", "/cube/v1/dry-run", "/cube/v1/subscribe"] {
        let answer = call(
            &env.router,
            Method::POST,
            path,
            Some(&d.s.bia),
            &[("x-cube-workspace", &d.w1)],
            Some(json!({ "query": query })),
        )
        .await;
        assert_eq!(answer.status, StatusCode::OK, "{path} {}", answer.json());
    }
    assert_eq!(d.queries.seen().len(), 4);
});

db_test!(a_query_touching_an_unreleased_cube_is_403, |store| {
    let d = data_scenario(store).await;
    let env = &d.s.env;
    let query = json!({ "measures": ["a_orders.count", "b_orders.count"] });
    let (status, body) = load_in(env, &d.s.bia, Some(&d.w1), query.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("b_orders"),
        "{body}"
    );
    // Not even an organization owner: the workspace decides.
    let (status, _) = load_in(env, &d.s.ana, Some(&d.w1), query.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    for path in ["/cube/v1/sql", "/cube/v1/dry-run"] {
        let answer = call(
            &env.router,
            Method::POST,
            path,
            Some(&d.s.bia),
            &[("x-cube-workspace", &d.w1)],
            Some(json!({ "query": query })),
        )
        .await;
        assert_eq!(answer.status, StatusCode::FORBIDDEN, "{path}");
    }
    // A member not in the model fails closed.
    let (status, _) = load_in(
        env,
        &d.s.bia,
        Some(&d.w1),
        json!({ "measures": ["nowhere.count"] }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // A member of the workspace only: bia is not in W2.
    let (status, _) = load_in(
        env,
        &d.s.bia,
        Some(&d.w2),
        json!({ "measures": ["b_orders.count"] }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(d.queries.seen().is_empty(), "{:?}", d.queries.seen());
});

db_test!(
    a_forbidden_member_hidden_in_a_nested_or_filter_is_403,
    |store| {
        let d = data_scenario(store).await;
        let env = &d.s.env;
        let query = json!({
            "measures": ["a_orders.count"],
            "filters": [{ "or": [
                { "member": "a_orders.status", "operator": "equals", "values": ["x"] },
                { "and": [
                    { "member": "a_orders.status", "operator": "set" },
                    { "member": "b_orders.status", "operator": "equals", "values": ["shipped"] },
                ] },
            ] }],
        });
        let (status, body) = load_in(env, &d.s.bia, Some(&d.w1), query).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert!(
            body["error"].as_str().unwrap().contains("b_orders"),
            "{body}"
        );
        // Ordering and time dimensions count as well.
        for query in [
            json!({ "measures": ["a_orders.count"], "order": [["b_orders.status", "asc"]] }),
            json!({ "measures": ["a_orders.count"], "segments": ["b_orders.some_segment"] }),
            json!({ "measures": ["a_orders.count"],
                "timeDimensions": [{ "dimension": "b_orders.status", "granularity": "day" }] }),
        ] {
            let (status, body) = load_in(env, &d.s.bia, Some(&d.w1), query.clone()).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{query} {body}");
        }
        assert!(d.queries.seen().is_empty(), "{:?}", d.queries.seen());
    }
);

db_test!(a_view_over_an_unreleased_cube_is_403, |store| {
    let d = data_scenario(store).await;
    let env = &d.s.env;
    let (status, body) = load_in(
        env,
        &d.s.bia,
        Some(&d.w1),
        json!({ "measures": ["b_view.count"] }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body["error"].as_str().unwrap().contains("b_view"), "{body}");
    assert!(d.queries.seen().is_empty());
    // In W2, where b_sqlite is released, the view runs.
    let (status, body) = load_in(
        env,
        &d.s.ana,
        Some(&d.w2),
        json!({ "measures": ["b_view.count"] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
});

db_test!(meta_shows_only_released_cubes, |store| {
    let d = data_scenario(store).await;
    let env = &d.s.env;
    assert_eq!(
        meta_cubes(env, &d.s.bia, Some(&d.w1)).await,
        (StatusCode::OK, vec!["a_orders".to_string()])
    );
    assert_eq!(
        meta_cubes(env, &d.s.ana, Some(&d.w2)).await,
        (
            StatusCode::OK,
            vec!["b_orders".to_string(), "b_view".to_string()]
        )
    );
    // Without a workspace: the connections the user can query somewhere.
    assert_eq!(
        meta_cubes(env, &d.s.bia, None).await,
        (StatusCode::OK, vec!["a_orders".to_string()])
    );
    assert_eq!(
        meta_cubes(env, &d.s.ana, None).await.1,
        ["a_orders", "b_orders", "b_view", "mixed_view"]
    );
    // A workspace the user is not in shows nothing of it.
    let (status, cubes) = meta_cubes(env, &d.s.bia, Some(&d.w2)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(cubes.is_empty());
    // The platform admin sees everything, with or without a workspace.
    assert_eq!(
        meta_cubes(env, &d.s.admin, None).await.1,
        [
            "a_orders",
            "b_orders",
            "b_view",
            "c_orders",
            "mixed_view",
            "plain_orders"
        ]
    );
    // A user with no workspace sees no cube.
    let (status, body) = create_user_as(env, &d.s.admin, "dan", "dan-password-1").await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let dan = login_token(&env.router, "dan", "dan-password-1").await;
    assert_eq!(meta_cubes(env, &dan, None).await, (StatusCode::OK, vec![]));
});

db_test!(
    data_requests_without_a_workspace_are_400_for_members_and_fine_for_admins,
    |store| {
        let d = data_scenario(store).await;
        let env = &d.s.env;
        let query = json!({ "measures": ["a_orders.count"] });
        for path in [
            "/cube/v1/load",
            "/cube/v1/sql",
            "/cube/v1/dry-run",
            "/cube/v1/subscribe",
        ] {
            let answer = call(
                &env.router,
                Method::POST,
                path,
                Some(&d.s.bia),
                &[],
                Some(json!({ "query": query })),
            )
            .await;
            assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{path}");
            assert_eq!(answer.json()["error"], "X-Cube-Workspace is required");
        }
        assert!(d.queries.seen().is_empty());
        let (status, body) = load_in(
            env,
            &d.s.admin,
            None,
            json!({ "measures": ["a_orders.count", "b_orders.count"] }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(d.queries.seen().len(), 1);
    }
);

db_test!(graphql_goes_through_the_same_check, |store| {
    let d = data_scenario(store).await;
    let env = &d.s.env;
    let graphql = |token: String, workspace: Option<String>, query: &'static str| {
        let router = env.router.clone();
        async move {
            let headers: Vec<(&str, &str)> = workspace
                .as_deref()
                .map(|ws| vec![("x-cube-workspace", ws)])
                .unwrap_or_default();
            call(
                &router,
                Method::POST,
                "/cube/graphql",
                Some(&token),
                &headers,
                Some(json!({ "query": query })),
            )
            .await
            .json()
        }
    };
    let b = "{ cube { b_orders { count } } }";
    let a = "{ cube { a_orders { count } } }";

    let body = graphql(d.s.bia.clone(), Some(d.w1.clone()), b).await;
    assert!(!body["errors"].as_array().unwrap().is_empty(), "{body}");
    // Without a workspace the query check refuses even a released cube.
    let body = graphql(d.s.bia.clone(), None, a).await;
    let errors = body["errors"].to_string();
    assert!(errors.contains("X-Cube-Workspace is required"), "{body}");
    // ana sees b_orders in the schema without a workspace (she manages the
    // connection), but W1 has not released it: the query check refuses.
    let body = graphql(d.s.ana.clone(), Some(d.w1.clone()), b).await;
    assert!(!body["errors"].as_array().unwrap().is_empty(), "{body}");
    assert!(d.queries.seen().is_empty(), "{:?}", d.queries.seen());

    let body = graphql(d.s.bia.clone(), Some(d.w1.clone()), a).await;
    assert!(body["errors"].is_null(), "{body}");
    assert_eq!(d.queries.seen().len(), 1);
});

const ADMIN_ONLY: &str = "Permission denied: platform admins only";

db_test!(cubesql_is_admin_only, |store| {
    let d = data_scenario(store).await;
    let env = &d.s.env;
    let routes = [
        (
            Method::POST,
            "/cube/v1/cubesql",
            Some(json!({ "query": "SELECT 1" })),
        ),
        (
            Method::POST,
            "/cube/v1/convert-query",
            Some(json!({ "query": "SELECT 1" })),
        ),
        (
            Method::POST,
            "/cube/v1/pre-aggregations/can-use",
            Some(json!({})),
        ),
        (
            Method::POST,
            "/cube/v1/pre-aggregations/jobs",
            Some(json!({})),
        ),
        (Method::GET, "/cube-system/v1/context", None),
        (Method::GET, "/cube-system/v1/pre-aggregations", None),
        (
            Method::POST,
            "/cube-system/v1/pre-aggregations/build",
            Some(json!({})),
        ),
    ];
    for (method, path, body) in routes {
        let answer = call(
            &env.router,
            method.clone(),
            path,
            Some(&d.s.ana),
            &[("x-cube-workspace", &d.w1)],
            body.clone(),
        )
        .await;
        assert_eq!(answer.status, StatusCode::FORBIDDEN, "{method} {path}");
        assert_eq!(answer.json()["error"], ADMIN_ONLY, "{method} {path}");
        // The admin gets past the RBAC check (the route's own checks, such
        // as the Playground signature of /cube-system, still apply).
        let answer = call(
            &env.router,
            method.clone(),
            path,
            Some(&d.s.admin),
            &[],
            body,
        )
        .await;
        assert_ne!(answer.json()["error"], ADMIN_ONLY, "{method} {path}");
    }
});

db_test!(
    execute_needs_workspace_owner_and_a_released_connection,
    |store| {
        let d = data_scenario(store).await;
        let env = &d.s.env;
        let execute = |token: String, workspace: Option<String>, connection: Value| {
            let router = env.router.clone();
            async move {
                let headers: Vec<(&str, &str)> = workspace
                    .as_deref()
                    .map(|ws| vec![("x-cube-workspace", ws)])
                    .unwrap_or_default();
                let answer = call(
                    &router,
                    Method::POST,
                    "/cube/v1/execute",
                    Some(&token),
                    &headers,
                    Some(json!({ "connection": connection, "query": "SELECT 1 AS one" })),
                )
                .await;
                (answer.status, answer.json())
            }
        };
        let w1 = Some(d.w1.clone());

        // An editor cannot execute; an owner can, on a released connection only.
        let (status, _) = execute(d.s.caio.clone(), w1.clone(), json!("a_sqlite")).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(
            set_role(env, &d.s.ana, &d.w1, "caio", "owner").await,
            StatusCode::OK
        );
        let (status, body) = execute(d.s.caio.clone(), None, json!("a_sqlite")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["error"], "X-Cube-Workspace is required");
        let (status, body) = execute(d.s.caio.clone(), w1.clone(), json!("b_sqlite")).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        let (status, _) = execute(d.s.caio.clone(), w1.clone(), json!("default")).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, body) = execute(d.s.caio.clone(), w1.clone(), json!("a_sqlite")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["rows"], json!([[1]]), "{body}");

        // Ad hoc settings: platform admins only.
        let database = d._dir.path().join("adhoc.db");
        let settings =
            json!({ "CUBEJS_DB_TYPE": "sqlite", "CUBEJS_DB_NAME": database.to_string_lossy() });
        let (status, _) = execute(d.s.caio.clone(), w1.clone(), settings.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, body) = execute(d.s.admin.clone(), None, settings).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, body) = execute(d.s.admin.clone(), None, json!("b_sqlite")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
);

db_test!(spicedb_down_means_503_not_data, |store| {
    let d = data_scenario(store).await;
    let env = &d.s.env;
    let (_, created) = create_object(env, &d.s.caio, &d.w1, pipeline("p", "secret")).await;
    let path = format!("/cube/v1/objects/{}", created["id"].as_str().unwrap());

    env.authz.set_failing(true);
    let (status, body) = load_in(
        env,
        &d.s.bia,
        Some(&d.w1),
        json!({ "measures": ["a_orders.count"] }),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    let (status, _) = login(&env.router, "bia", "bia-password-1").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let (status, body) = send(&env.router, Method::GET, &path, Some(&d.s.bia), None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(!body.to_string().contains("secret"));
    let (status, _) = meta_cubes(env, &d.s.bia, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    env.authz.set_failing(false);
    assert!(d.queries.seen().is_empty());
});

// The middleware already answers 503 when SpiceDB is down (it resolves the
// identity there), so this calls the RBAC services directly with an
// identity in hand: they must fail closed on their own.
db_test!(rbac_services_fail_closed_when_spicedb_is_down, |store| {
    use cubeserver::rbac::enforce::{RbacMetaService, RbacQueryService};
    let d = data_scenario(store).await;
    let env = &d.s.env;
    let query = RbacQueryService::new(d.queries.clone(), env.rbac.clone());
    let meta = RbacMetaService::new(
        Arc::new(
            cubeserver::meta_adapter::ModelMetaService::load(d._dir.path().join("model"), false)
                .unwrap(),
        ),
        env.rbac.clone(),
        d.queries.data_sources(),
    );
    let ctx = RequestContext {
        user: Some("bia".into()),
        workspace: Some(d.w1.clone()),
        ..RequestContext::default()
    };
    let request = |measure: &str| {
        let value = json!({ "measures": [measure] });
        let (query_type, queries) = cubequery::get_normalized_queries(
            &value,
            false,
            None,
            &cubequery::QueryConfig::default(),
        )
        .unwrap();
        let pivot_query = cubequery::get_pivot_query(query_type, &queries).unwrap();
        NormalizedRequest {
            query_type,
            queries,
            pivot_query,
        }
    };

    query.load(&ctx, request("a_orders.count")).await.unwrap();
    assert_eq!(d.queries.seen().len(), 1);
    let err = query
        .load(&ctx, request("b_orders.count"))
        .await
        .unwrap_err();
    assert_eq!(err.status, StatusCode::FORBIDDEN);
    let cubes = meta.meta(&ctx, false).await.unwrap();
    assert_eq!(cubes["cubes"].as_array().unwrap().len(), 1, "{cubes}");

    env.authz.set_failing(true);
    let err = query
        .load(&ctx, request("a_orders.count"))
        .await
        .unwrap_err();
    assert_eq!(err.status, StatusCode::SERVICE_UNAVAILABLE);
    for ctx in [
        ctx.clone(),
        RequestContext {
            workspace: None,
            ..ctx.clone()
        },
    ] {
        let err = meta.meta(&ctx, false).await.unwrap_err();
        assert_eq!(err.status, StatusCode::SERVICE_UNAVAILABLE);
    }
    env.authz.set_failing(false);
    assert_eq!(d.queries.seen().len(), 1);
});

db_test!(
    the_websocket_names_its_workspace_when_authenticating,
    |store| {
        let d = data_scenario(store).await;
        let load = |id: u32, measure: &str| {
            json!({ "method": "load", "messageId": id, "params": { "query": { "measures": [measure] } } })
            .to_string()
        };

        // No workspace: data methods are refused like the HTTP ones.
        let recorder = Arc::new(Recorder::default());
        let server = cubeserver::ws::SubscriptionServer::new(d.state.clone(), recorder.clone());
        server
            .process_message("c1", &json!({ "authorization": d.s.bia }).to_string())
            .await;
        server
            .process_message("c1", &load(1, "a_orders.count"))
            .await;
        let answer = recorder.last().await;
        assert_eq!(answer["status"], json!(400), "{answer}");
        assert_eq!(answer["message"]["error"], WORKSPACE_REQUIRED);

        // With W1: a_orders runs, b_orders is refused.
        let recorder = Arc::new(Recorder::default());
        let server = cubeserver::ws::SubscriptionServer::new(d.state.clone(), recorder.clone());
        server
            .process_message(
                "c2",
                &json!({ "authorization": d.s.bia, "workspace": d.w1 }).to_string(),
            )
            .await;
        assert_eq!(recorder.last().await, json!({ "handshake": true }));
        server
            .process_message("c2", &load(2, "b_orders.count"))
            .await;
        assert_eq!(recorder.last().await["status"], json!(403));
        assert!(d.queries.seen().is_empty());
        server
            .process_message("c2", &load(3, "a_orders.count"))
            .await;
        assert_eq!(recorder.last().await, json!({ "messageProcessedId": "3" }));
        assert_eq!(d.queries.seen().len(), 1);
    }
);

/// A connection made directly: its row in `org`, `connection#organization`,
/// and a release to each of `workspaces`.
async fn grant_connection(env: &Env, name: &str, org: &str, workspaces: &[&str]) {
    env.rbac
        .store
        .create_connection(name, org.parse().unwrap(), ConnectionOrigin::Assigned)
        .await
        .unwrap();
    let mut relationships = vec![Relationship {
        resource: ObjectRef::new("connection", name.to_string()),
        relation: "organization",
        subject: ObjectRef::new("organization", org.to_string()),
    }];
    for ws in workspaces {
        relationships.push(Relationship {
            resource: ObjectRef::new("connection", name.to_string()),
            relation: "workspace",
            subject: ObjectRef::new("workspace", ws.to_string()),
        });
    }
    env.authz.touch(&relationships).await.unwrap();
}

db_test!(
    a_view_spanning_an_unreleased_source_is_403_member_by_member,
    |store| {
        let d = data_scenario(store).await;
        let env = &d.s.env;
        // mixed_view.count is an a_orders member (released to W1), but the view
        // also spans b_sqlite: refused, as /v1/meta hides the view.
        let (status, body) = load_in(
            env,
            &d.s.bia,
            Some(&d.w1),
            json!({ "measures": ["mixed_view.count"] }),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert!(
            body["error"].as_str().unwrap().contains("mixed_view"),
            "{body}"
        );
        assert!(d.queries.seen().is_empty());
        // Released both: the view runs.
        let w3 = workspace_id(env, &d.s.ana, &d.s.org_a, "W3").await;
        for name in ["a_sqlite", "b_sqlite"] {
            assert_eq!(
                release(env, &d.s.ana, Method::PUT, &w3, name).await,
                StatusCode::NO_CONTENT
            );
        }
        let (status, body) = load_in(
        env,
        &d.s.ana,
        Some(&w3),
        json!({ "measures": ["mixed_view.count"], "dimensions": ["mixed_view.b_orders_status"] }),
    )
    .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            meta_cubes(env, &d.s.ana, Some(&w3)).await.1,
            ["a_orders", "b_orders", "b_view", "mixed_view"]
        );
    }
);

db_test!(the_default_data_source_must_be_released_too, |store| {
    let d = data_scenario(store).await;
    let env = &d.s.env;
    let query = json!({ "measures": ["plain_orders.count"] });
    let (status, body) = load_in(env, &d.s.bia, Some(&d.w1), query.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("plain_orders"),
        "{body}"
    );
    assert!(!meta_cubes(env, &d.s.bia, Some(&d.w1))
        .await
        .1
        .contains(&"plain_orders".to_string()));

    grant_connection(env, "default", &d.s.org_a, &[&d.w1]).await;
    let (status, body) = load_in(env, &d.s.bia, Some(&d.w1), query).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        meta_cubes(env, &d.s.bia, Some(&d.w1)).await.1,
        ["a_orders", "plain_orders"]
    );
});

db_test!(
    meta_without_a_workspace_ignores_other_orgs_connections,
    |store| {
        let d = data_scenario(store).await;
        let env = &d.s.env;
        let (status, body) = create_user_as(env, &d.s.admin, "dan", "dan-password-1").await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let dan = login_token(&env.router, "dan", "dan-password-1").await;
        let org_b = create_org(env, &d.s.admin, "B", "dan").await;
        // c_sqlite belongs to B, but a stale relationship releases it to A's W1.
        grant_connection(env, "c_sqlite", &org_b, &[&d.w1]).await;

        assert_eq!(meta_cubes(env, &d.s.bia, None).await.1, ["a_orders"]);
        assert_eq!(meta_cubes(env, &d.s.bia, Some(&d.w1)).await.1, ["a_orders"]);
        // B's owner sees it.
        assert_eq!(meta_cubes(env, &dan, None).await.1, ["c_orders"]);
    }
);

/// Serves `/v1/meta?extended` from the model, as a deployment with it would.
#[derive(Debug)]
struct ExtendedMeta(cubeserver::meta_adapter::ModelMetaService);

#[async_trait]
impl MetaService for ExtendedMeta {
    async fn meta(&self, ctx: &RequestContext, only_views: bool) -> Result<Value, ApiError> {
        self.0.meta(ctx, only_views).await
    }

    async fn meta_extended(
        &self,
        ctx: &RequestContext,
        only_views: bool,
    ) -> Result<Value, ApiError> {
        let mut meta = self.0.meta(ctx, only_views).await?;
        meta["extended"] = json!(true);
        Ok(meta)
    }
}

db_test!(meta_extended_is_filtered_too, |store| {
    use cubeserver::rbac::enforce::RbacMetaService;
    let d = data_scenario(store).await;
    let env = &d.s.env;
    let meta = RbacMetaService::new(
        Arc::new(ExtendedMeta(
            cubeserver::meta_adapter::ModelMetaService::load(d._dir.path().join("model"), false)
                .unwrap(),
        )),
        env.rbac.clone(),
        d.queries.data_sources(),
    );
    let ctx = RequestContext {
        user: Some("bia".into()),
        workspace: Some(d.w1.clone()),
        ..RequestContext::default()
    };
    let extended = meta.meta_extended(&ctx, false).await.unwrap();
    assert_eq!(extended["extended"], json!(true));
    let names: Vec<&str> = extended["cubes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["a_orders"]);
});

db_test!(a_query_service_without_a_snapshot_is_503, |store| {
    use cubeserver::rbac::enforce::{RbacMetaService, RbacQueryService};
    let d = data_scenario(store).await;
    let env = &d.s.env;
    let query = RbacQueryService::new(Arc::new(UnimplementedQueryService), env.rbac.clone());
    let ctx = RequestContext {
        user: Some("bia".into()),
        workspace: Some(d.w1.clone()),
        ..RequestContext::default()
    };
    let value = json!({ "measures": ["a_orders.count"] });
    let (query_type, queries) =
        cubequery::get_normalized_queries(&value, false, None, &cubequery::QueryConfig::default())
            .unwrap();
    let pivot_query = cubequery::get_pivot_query(query_type, &queries).unwrap();
    let err = query
        .sql(
            &ctx,
            NormalizedRequest {
                query_type,
                queries,
                pivot_query,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err.status, StatusCode::SERVICE_UNAVAILABLE);

    let meta = RbacMetaService::new(
        Arc::new(
            cubeserver::meta_adapter::ModelMetaService::load(d._dir.path().join("model"), false)
                .unwrap(),
        ),
        env.rbac.clone(),
        None,
    );
    let err = meta.meta(&ctx, false).await.unwrap_err();
    assert_eq!(err.status, StatusCode::SERVICE_UNAVAILABLE);
});

// The real tenant runtime (planner included), so the check reads the
// planner's snapshot: a model edited on disk changes nothing until the
// runtime reloads, and then the check and the planner move together.
db_test!(the_check_follows_the_planner_not_the_files, |store| {
    let dir = tempfile::tempdir().unwrap();
    let model = dir.path().join("model");
    std::fs::create_dir(&model).unwrap();
    let file = model.join("model.yml");
    std::fs::write(&file, DATA_MODEL).unwrap();
    std::fs::write(
        dir.path().join("cube.yml"),
        "version: 1\ndata_sources:\n  default:\n    type: postgres\n  a_sqlite:\n    type: sqlite\n  b_sqlite:\n    type: sqlite\n  c_sqlite:\n    type: sqlite\n",
    )
    .unwrap();
    let config = cubeconfig::CubeConfig::load_with_env(
        dir.path(),
        &cubeconfig::Env::new().with("CUBEJS_SCHEMA_PATH", model.to_string_lossy()),
    )
    .expect("valid cube.yml");
    let registry = Arc::new(cubeserver::tenants::TenantRegistry::new(
        Arc::new(config),
        cubeplanner::Dialect::Postgres,
        Some("postgres".to_string()),
        1,
        false,
    ));
    let authz = Arc::new(FakeAuthz::new());
    let rbac = Arc::new(
        Rbac::with_parts(authz.clone(), store, rbac_config(Duration::ZERO))
            .await
            .expect("rbac starts"),
    );
    let mut state = state_with(Some(rbac.clone()), ServerConfig::default());
    state.tenants = Some(registry.clone());
    let env = Env {
        router: build_app(state),
        rbac,
        authz,
    };
    let s = scenario_from_env(env).await;
    let env = &s.env;
    let w1 = workspace_id(env, &s.ana, &s.org_a, "W1").await;
    assert_eq!(
        set_role(env, &s.ana, &w1, "bia", "viewer").await,
        StatusCode::OK
    );
    grant_connection(env, "a_sqlite", &s.org_a, &[&w1]).await;
    grant_connection(env, "b_sqlite", &s.org_a, &[]).await;

    let sql = |measure: &'static str| {
        let router = env.router.clone();
        let token = s.bia.clone();
        let w1 = w1.clone();
        async move {
            let answer = call(
                &router,
                Method::POST,
                "/cube/v1/sql",
                Some(&token),
                &[("x-cube-workspace", &w1)],
                Some(json!({ "query": { "measures": [measure] } })),
            )
            .await;
            (answer.status, answer.json())
        }
    };
    let (status, body) = sql("a_orders.count").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(sql("b_orders.count").await.0, StatusCode::FORBIDDEN);

    // a_orders moves to b_sqlite on disk, and b_orders to a_sqlite. The
    // runtime has not reloaded: its planner still runs a_orders on a_sqlite,
    // so the check still says the same.
    let swapped = DATA_MODEL
        .replacen("data_source: a_sqlite", "data_source: TMP", 1)
        .replacen("data_source: b_sqlite", "data_source: a_sqlite", 1)
        .replacen("data_source: TMP", "data_source: b_sqlite", 1);
    std::fs::write(&file, swapped).unwrap();
    let (status, body) = sql("a_orders.count").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(sql("b_orders.count").await.0, StatusCode::FORBIDDEN);
    assert_eq!(meta_cubes(env, &s.bia, Some(&w1)).await.1, ["a_orders"]);

    // After the reload both move together.
    assert!(registry.reload_all().await.is_empty());
    assert_eq!(sql("a_orders.count").await.0, StatusCode::FORBIDDEN);
    let (status, body) = sql("b_orders.count").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        meta_cubes(env, &s.bia, Some(&w1)).await.1,
        ["b_orders", "b_view"]
    );
});

// ---- the Playground ----

/// A server that mounts the Playground, with a `cube.yml` (the data source
/// `default`), an `.env` and a model directory in `dir`.
fn playground_server(dir: &tempfile::TempDir) -> ServerConfig {
    let assets = dir.path().join("playground");
    std::fs::create_dir_all(assets.join("assets")).unwrap();
    std::fs::write(
        assets.join("index.html"),
        r#"<html><div id="playground-root"></div></html>"#,
    )
    .unwrap();
    let model = dir.path().join("model");
    std::fs::create_dir_all(&model).unwrap();
    std::fs::write(model.join("orders.yml"), "cubes: []").unwrap();
    let file = dir.path().join("cube.yml");
    std::fs::write(
        &file,
        "version: 1\ndata_sources:\n  default:\n    type: postgres\n    host: db\n    password: secret\n",
    )
    .unwrap();
    std::fs::write(dir.path().join(".env"), "PORT=4000\n").unwrap();
    ServerConfig {
        config_file: Some(file),
        schema_path: model.to_string_lossy().into_owned(),
        playground_path: Some(assets.to_string_lossy().into_owned()),
        data_sources: vec![("default".to_string(), "postgres".to_string())],
        ..ServerConfig::default()
    }
}

/// A Playground call as the page makes it: under `localhost`, with the key
/// `/playground/context` hands out.
async fn playground(
    router: &axum::Router,
    method: Method,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    use cubeserver::handlers::playground::{playground_key, PLAYGROUND_KEY_HEADER};
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "localhost:4000")
        .header(PLAYGROUND_KEY_HEADER, playground_key());
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
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Every helper route but the context, with a body where it takes one.
fn playground_helpers() -> Vec<(Method, &'static str, Option<Value>)> {
    vec![
        (Method::GET, "/playground/files", None),
        (Method::GET, "/playground/db-schema", None),
        (Method::GET, "/playground/driver?driver=postgres", None),
        (
            Method::POST,
            "/playground/generate-schema",
            Some(json!({ "format": "yaml", "tables": [] })),
        ),
        (Method::GET, "/playground/data-sources", None),
        (
            Method::POST,
            "/playground/data-sources",
            Some(json!({ "name": "y", "variables": { "CUBEJS_DB_TYPE": "sqlite" } })),
        ),
        (
            Method::POST,
            "/playground/data-sources/test",
            Some(json!({ "variables": { "CUBEJS_DB_TYPE": "sqlite" } })),
        ),
        (Method::GET, "/playground/data-sources/default", None),
        (
            Method::PUT,
            "/playground/data-sources/default",
            Some(json!({ "variables": {} })),
        ),
        (Method::DELETE, "/playground/data-sources/default", None),
        (Method::POST, "/playground/env", Some(json!({}))),
        (Method::POST, "/playground/test-connection", Some(json!({}))),
        (
            Method::POST,
            "/playground/schema/pre-aggregation",
            Some(json!({})),
        ),
    ]
}

db_test!(playground_context_hands_out_no_token_with_rbac, |store| {
    let dir = tempfile::tempdir().unwrap();
    let env = env_with_server(store, rbac_config(Duration::ZERO), playground_server(&dir)).await;
    let (status, body) =
        playground(&env.router, Method::GET, "/playground/context", None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.get("cubejsToken").is_none(), "{body}");
    assert_eq!(body["authRequired"], json!(true), "{body}");
    assert_eq!(body["rbac"], json!(true), "{body}");
    // The key comes only with the flag that says a login is needed: alone
    // it opens nothing.
    assert!(body["playgroundKey"].is_string(), "{body}");
    assert_eq!(body["basePath"], "/cube", "{body}");
});

db_test!(playground_token_minting_is_gone_with_rbac, |store| {
    let dir = tempfile::tempdir().unwrap();
    let s = scenario_with(store, playground_server(&dir)).await;
    for token in [None, Some(s.admin.as_str()), Some(s.ana.as_str())] {
        let (status, body) = playground(
            &s.env.router,
            Method::POST,
            "/playground/token",
            token,
            Some(json!({ "payload": { "sub": "admin" } })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        assert!(body.get("token").is_none(), "{body}");
    }
});

/// Authenticates as the real service does, but reports the token as not
/// verified: the dev-mode fallthrough that lets a request through anyway.
#[derive(Debug)]
struct Unverified(CubeAuthService);

#[async_trait]
impl cubeserver::services::AuthService for Unverified {
    async fn authenticate(
        &self,
        authorization: Option<&str>,
    ) -> Result<cubeserver::services::AuthenticatedRequest, ApiError> {
        let mut authenticated = self.0.authenticate(authorization).await?;
        authenticated.token_verified = false;
        Ok(authenticated)
    }

    async fn api_scopes(&self, security_context: &Value) -> Result<Vec<String>, ApiError> {
        self.0.api_scopes(security_context).await
    }
}

db_test!(playground_helpers_need_a_platform_admin, |store| {
    let dir = tempfile::tempdir().unwrap();
    let s = scenario_with(store, playground_server(&dir)).await;
    let router = &s.env.router;
    for (method, path, body) in playground_helpers() {
        for token in [None, Some("not-a-token")] {
            let (status, answer) =
                playground(router, method.clone(), path, token, body.clone()).await;
            assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "{method} {path}: {answer}"
            );
        }
        let (status, answer) =
            playground(router, method.clone(), path, Some(&s.ana), body.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}: {answer}");
    }

    // A platform admin gets through.
    let (status, body) = playground(
        router,
        Method::GET,
        "/playground/files",
        Some(&s.admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["files"][0]["fileName"], "orders.yml", "{body}");
    let (status, body) = playground(
        router,
        Method::GET,
        "/playground/driver?driver=postgres",
        Some(&s.admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = playground(
        router,
        Method::GET,
        "/playground/data-sources",
        Some(&s.admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Still only with the key, and only under a name that cannot be rebound.
    let request = |key: bool, host: &str| {
        let mut request = Request::builder()
            .uri("/playground/data-sources")
            .header(header::HOST, host)
            .header(header::AUTHORIZATION, format!("Bearer {}", s.admin));
        if key {
            request = request.header(
                cubeserver::handlers::playground::PLAYGROUND_KEY_HEADER,
                cubeserver::handlers::playground::playground_key(),
            );
        }
        request.body(Body::empty()).unwrap()
    };
    let status = router
        .clone()
        .oneshot(request(false, "localhost"))
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::FORBIDDEN);
    let status = router
        .clone()
        .oneshot(request(true, "evil.example"))
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::FORBIDDEN);

    // A token the authenticator did not verify is no login, even the
    // admin's.
    let mut state = state_with(Some(s.env.rbac.clone()), playground_server(&dir));
    state.auth = Arc::new(Unverified(CubeAuthService::new(auth_config())));
    let unverified = build_app(state);
    let (status, body) = playground(
        &unverified,
        Method::GET,
        "/playground/files",
        Some(&s.admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
});

#[tokio::test]
async fn playground_unchanged_without_rbac() {
    let dir = tempfile::tempdir().unwrap();
    let router = build_app(state_with(None, playground_server(&dir)));
    let (status, body) = playground(&router, Method::GET, "/playground/context", None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["cubejsToken"].is_string(), "{body}");
    assert!(body.get("authRequired").is_none(), "{body}");
    assert!(body.get("rbac").is_none(), "{body}");
    let (status, body) = playground(
        &router,
        Method::POST,
        "/playground/token",
        None,
        Some(json!({ "payload": { "tenant": "a" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["token"].is_string(), "{body}");
    // The helpers take no login, as before.
    let (status, body) = playground(&router, Method::GET, "/playground/files", None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

db_test!(playground_delete_drops_the_connections_grants, |store| {
    let dir = tempfile::tempdir().unwrap();
    let s = scenario_with(store, playground_server(&dir)).await;
    let env = &s.env;
    let (status, body) = add_connection(env, &s.ana, &s.org_a, "x", &dir).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let w1 = workspace_id(env, &s.ana, &s.org_a, "W1").await;
    assert_eq!(
        release(env, &s.ana, Method::PUT, &w1, "x").await,
        StatusCode::NO_CONTENT
    );

    let (status, body) = playground(
        &env.router,
        Method::DELETE,
        "/playground/data-sources/x",
        Some(&s.admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(env.rbac.store.connection("x").await.unwrap().is_none());
    assert!(released(env, &w1).await.is_empty());
});

db_test!(playground_create_refuses_a_tracked_name, |store| {
    let dir = tempfile::tempdir().unwrap();
    let s = scenario_with(store, playground_server(&dir)).await;
    let env = &s.env;
    env.rbac
        .store
        .create_connection("z", s.org_a.parse().unwrap(), ConnectionOrigin::Assigned)
        .await
        .unwrap();
    let (status, body) = playground(
        &env.router,
        Method::POST,
        "/playground/data-sources",
        Some(&s.admin),
        Some(json!({
            "name": "z",
            "variables": {
                "CUBEJS_DB_TYPE": "sqlite",
                "CUBEJS_DB_NAME": dir.path().join("z.db").to_string_lossy(),
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let yml = std::fs::read_to_string(dir.path().join("cube.yml")).unwrap();
    assert!(!yml.contains("  z:"), "{yml}");
});
