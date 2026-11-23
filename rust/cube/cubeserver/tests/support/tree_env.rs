//! Shared setup of the tests that run the app over a real RBAC store with the
//! folder ownership policy (`org_tree.rs`, `model_api.rs`). Included with
//! `#[path]`, so each binary has its own copy (and its own `SERIAL`).
#![allow(dead_code)]

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
use tower::ServiceExt;

pub const SECRET: &str = "rbac-test-secret";
pub const ADMIN: &str = "admin";
pub const ADMIN_PASSWORD: &str = "admin-password-1";

pub static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Clears the policy when the test ends, panicking or not.
pub struct ClearPolicy;
impl Drop for ClearPolicy {
    fn drop(&mut self) {
        set_policy(None);
    }
}

#[derive(Debug)]
pub struct NoMeta;

#[async_trait::async_trait]
impl MetaService for NoMeta {
    async fn meta(&self, _: &RequestContext, _: bool) -> Result<Value, ApiError> {
        Ok(json!({ "cubes": [] }))
    }
}

pub fn rbac_config() -> RbacConfig {
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
        sync_user: "sofiadb-sync".into(),
    }
}

pub struct Env {
    pub router: axum::Router,
    pub model: tempfile::TempDir,
    /// The Playground's assets: `build_app` mounts `/playground/*` only when
    /// they exist, as the deployment does.
    pub _assets: tempfile::TempDir,
    pub rbac: Arc<Rbac>,
    pub state: AppState,
    /// With `env_live`: the model served from the database, and its writer.
    pub runtime: Option<Arc<cubeserver::rbac::model_runtime::ModelRuntime>>,
    pub writer: Option<Arc<cubeserver::rbac::model_writer::ModelWriter>>,
    pub _cache: Option<tempfile::TempDir>,
}

pub async fn env(store: cubeserver::rbac::store::Store) -> Env {
    env_with(store, false).await
}

/// `real_meta`: the model is served by a tenant registry, as in `main`, so a
/// policy change shows in `/cube/v1/meta`.
pub async fn env_with(store: cubeserver::rbac::store::Store, real_meta: bool) -> Env {
    build_env(store, real_meta, false).await
}

/// Real meta over a `ModelRuntime` of the store: the schema path and the
/// tenant registry read the runtime's `live` folder, and `state.model_writer`
/// is set, so a write shows in `/cube/v1/meta` right away.
pub async fn env_live(store: cubeserver::rbac::store::Store) -> Env {
    build_env(store, true, true).await
}

async fn build_env(store: cubeserver::rbac::store::Store, real_meta: bool, live: bool) -> Env {
    use std::os::unix::fs::PermissionsExt;
    let model = tempfile::tempdir().unwrap();
    let (cache, runtime) = if live {
        let cache = tempfile::tempdir().unwrap();
        std::fs::set_permissions(cache.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = cubeserver::rbac::model_runtime::ModelRuntime::start(
            store.clone(),
            cache.path().to_path_buf(),
        )
        .await
        .expect("runtime starts");
        (Some(cache), Some(runtime))
    } else {
        (None, None)
    };
    let model_dir: String = match &runtime {
        Some(r) => r.live_path().to_string_lossy().into_owned(),
        None => model.path().to_string_lossy().into_owned(),
    };
    let writer_store = store.clone();
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
        schema_path: model_dir.clone(),
        playground_path: Some(assets.path().to_string_lossy().into_owned()),
        data_sources: vec![("sofiadb".into(), "postgres".into())],
        ..ServerConfig::default()
    };
    let tenants = real_meta.then(|| {
        let cube_config = cubeconfig::CubeConfig::load_or_env(
            "/nonexistent-cube-yml-dir",
            &cubeconfig::Env::new()
                .with("CUBEJS_SCHEMA_PATH", &*model_dir)
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
    let writer = runtime.as_ref().map(|runtime| {
        Arc::new(cubeserver::rbac::model_writer::ModelWriter::new(
            writer_store,
            runtime.clone(),
            tenants.clone().expect("live env has a tenant registry"),
            cubemodel::TemplateContext::default(),
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
        model_runtime: runtime.clone(),
        model_writer: writer.clone(),
    };
    let router = build_app(state.clone());
    Env { router, model, _assets: assets, rbac, state, runtime, writer, _cache: cache }
}

impl Env {
    pub fn write(&self, name: &str, content: &str) {
        let path = self.model.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
}

pub async fn send(
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

pub async fn login(env: &Env, user: &str, password: &str) -> String {
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

pub async fn create_org(env: &Env, token: &str, name: &str) -> String {
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

pub async fn get(env: &Env, token: &str, path: &str) -> (StatusCode, Value, String) {
    send(&env.router, Method::GET, path, Some(token), None).await
}

/// Installs a policy; `set_policy` gives it a new generation.
pub fn install_policy(orgs: &[(&str, &[&str])], platform: &[&str]) {
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
