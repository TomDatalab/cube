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
use cubeserver::rbac::Rbac;
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
    AppState {
        config: Arc::new(ServerConfig::default()),
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
    let authz = Arc::new(FakeAuthz::new());
    let rbac = Arc::new(
        Rbac::with_parts(authz.clone(), store, config)
            .await
            .expect("rbac starts"),
    );
    Env {
        router: build_app(state(Some(rbac.clone()))),
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
