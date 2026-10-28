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
use cubeserver::rbac::config::{check_rbac_compatible, RbacConfig};
use cubeserver::rbac::identity::{hash_password, verify_password};
use cubeserver::rbac::store::Store;
use cubeserver::rbac::testing::FakeAuthz;
use cubeserver::rbac::{Authz, ObjectRef, Rbac};
use cubeserver::services::{AlwaysHealthy, MetaService, RequestContext, UnimplementedQueryService};
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
        // `data_sources` too, so a 403 there comes from the RBAC check.
        .default_api_scopes(["graphql", "meta", "data", "sql", "data_sources"])
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

/// A scenario whose server reads a `cube.yml` (with the data source
/// `default`) and an `.env` from `dir`.
async fn connections_scenario(store: Store, dir: &tempfile::TempDir) -> Scenario {
    let file = dir.path().join("cube.yml");
    std::fs::write(
        &file,
        "version: 1\ndata_sources:\n  default:\n    type: postgres\n    host: db\n    password: secret\n",
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
        json!({ "connections": [{ "name": "x", "type": "sqlite", "workspaces": [w1] }] })
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

db_test!(assigning_an_existing_data_source_to_an_org, |store| {
    let dir = tempfile::tempdir().unwrap();
    let s = connections_scenario(store, &dir).await;
    let env = &s.env;
    let assign = |token: String, name: &'static str, org: String| {
        let router = env.router.clone();
        async move {
            send(
                &router,
                Method::PUT,
                &format!("/cube/v1/connections/{name}/organization"),
                Some(&token),
                Some(json!({ "organizationId": org })),
            )
            .await
            .0
        }
    };
    // Platform admins only.
    assert_eq!(
        assign(s.ana.clone(), "default", s.org_a.clone()).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        assign(s.admin.clone(), "nope", s.org_a.clone()).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        assign(
            s.admin.clone(),
            "default",
            "00000000-0000-0000-0000-000000000000".into()
        )
        .await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        assign(s.admin.clone(), "default", s.org_a.clone()).await,
        StatusCode::NO_CONTENT
    );

    let row = env.rbac.store.connection("default").await.unwrap().unwrap();
    assert_eq!(row.organization_id.to_string(), s.org_a);
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
        json!({ "connections": [{ "name": "default", "type": "postgres", "workspaces": [] }] })
    );
    let w1 = workspace_id(env, &s.ana, &s.org_a, "W1").await;
    assert_eq!(
        release(env, &s.ana, Method::PUT, &w1, "default").await,
        StatusCode::NO_CONTENT
    );

    // Moving it to another organization withdraws the old releases.
    let org_b = create_org(env, &s.admin, "B", "bia").await;
    assert_eq!(
        assign(s.admin.clone(), "default", org_b.clone()).await,
        StatusCode::NO_CONTENT
    );
    assert!(released(env, &w1).await.is_empty());
    let connection = ObjectRef::new("connection", "default");
    assert!(!env.authz.check(&connection, "manage", "ana").await.unwrap());
    assert!(env.authz.check(&connection, "manage", "bia").await.unwrap());
    let row = env.rbac.store.connection("default").await.unwrap().unwrap();
    assert_eq!(row.organization_id.to_string(), org_b);
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
