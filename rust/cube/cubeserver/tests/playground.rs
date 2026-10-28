//! The Playground as the browser sees it: the page at `/`, its assets, the
//! bootstrap call, and the JSON 404 that must survive underneath.

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use cubeserver::handlers::playground::{playground_key, PLAYGROUND_KEY_HEADER};
use cubeserver::services::{
    AlwaysHealthy, AuthService, AuthenticatedRequest, MetaService, RequestContext,
    UnimplementedQueryService,
};
use cubeserver::{build_app, ApiError, AppState, ServerConfig};
use http_body_util::BodyExt;
use serde_json::{json, Map, Value};
use tower::ServiceExt;

/// Issues a recognisable token, so a test can tell the app got this one.
#[derive(Debug)]
struct TokenIssuingAuth;

#[async_trait]
impl AuthService for TokenIssuingAuth {
    async fn authenticate(&self, _: Option<&str>) -> Result<AuthenticatedRequest, ApiError> {
        Ok(AuthenticatedRequest {
            security_context: json!({}),
            signed_with_playground_auth_secret: false,
            token_verified: true,
        })
    }

    async fn api_scopes(&self, _: &Value) -> Result<Vec<String>, ApiError> {
        Ok(vec!["meta".to_string(), "data".to_string()])
    }

    async fn issue_token(&self, claims: Map<String, Value>) -> Result<String, ApiError> {
        Ok(format!("signed:{}", Value::Object(claims)))
    }
}

/// An authenticator with no secret, like a deployment that cannot sign.
#[derive(Debug)]
struct CannotIssue;

#[async_trait]
impl AuthService for CannotIssue {
    async fn authenticate(&self, _: Option<&str>) -> Result<AuthenticatedRequest, ApiError> {
        Ok(AuthenticatedRequest {
            security_context: json!({}),
            signed_with_playground_auth_secret: false,
            token_verified: true,
        })
    }

    async fn api_scopes(&self, _: &Value) -> Result<Vec<String>, ApiError> {
        Ok(vec![])
    }
}

#[derive(Debug)]
struct NoMeta;

#[async_trait]
impl MetaService for NoMeta {
    async fn meta(&self, _: &RequestContext, _: bool) -> Result<Value, ApiError> {
        Ok(json!({ "cubes": [] }))
    }
}

/// A directory that looks like a Playground build, plus a model directory.
fn fixture() -> (tempfile::TempDir, ServerConfig) {
    let dir = tempfile::tempdir().expect("temp dir");

    let assets = dir.path().join("playground");
    std::fs::create_dir_all(assets.join("assets")).unwrap();
    std::fs::write(
        assets.join("index.html"),
        r#"<html><div id="playground-root"></div></html>"#,
    )
    .unwrap();
    std::fs::write(assets.join("assets/index.js"), "console.log('cube')").unwrap();

    let model = dir.path().join("model");
    std::fs::create_dir_all(model.join("views")).unwrap();
    std::fs::write(model.join("orders.yml"), "cubes: []").unwrap();
    std::fs::write(model.join("views/sales.yml"), "views: []").unwrap();
    std::fs::write(model.join("notes.txt"), "not a model file").unwrap();

    let config = ServerConfig {
        base_path: "/cube".to_string(),
        schema_path: model.to_string_lossy().into_owned(),
        playground_path: Some(assets.to_string_lossy().into_owned()),
        data_sources: vec![("default".to_string(), "postgres".to_string())],
        ..ServerConfig::default()
    };

    (dir, config)
}

fn app(config: ServerConfig, auth: Arc<dyn AuthService>) -> axum::Router {
    build_app(AppState {
        config: Arc::new(config),
        query_config: Arc::new(cubequery::QueryConfig::default()),
        auth,
        meta: Arc::new(NoMeta),
        health: Arc::new(AlwaysHealthy),
        query: Arc::new(UnimplementedQueryService),
        pre_aggregations: Arc::new(cubeserver::services::UnimplementedPreAggregationService),
        sql_conversion: Arc::new(cubeserver::services::UnimplementedSqlConversionService),
        graphql: Default::default(),
        ws: Default::default(),
        tenants: None,
    })
}

/// Every helper sends the key the page gets from `/playground/context`, as
/// the page does; `the_playground_key_is_needed_to_change_anything` goes
/// without it.
async fn get(router: axum::Router, path: &str) -> (StatusCode, String) {
    let request = Request::builder()
        .uri(path)
        .header(PLAYGROUND_KEY_HEADER, playground_key())
        .body(Body::empty())
        .expect("a request");
    let response = router.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();

    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn get_json(router: axum::Router, path: &str) -> (StatusCode, Value) {
    let (status, body) = get(router, path).await;
    let value = serde_json::from_str(&body).unwrap_or(Value::Null);

    (status, value)
}

#[tokio::test]
async fn the_page_is_served_at_the_root() {
    let (_dir, config) = fixture();
    let (status, body) = get(app(config, Arc::new(TokenIssuingAuth)), "/").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("playground-root"), "{body}");
}

#[tokio::test]
async fn assets_are_served_from_the_root_too() {
    let (_dir, config) = fixture();
    let (status, body) = get(app(config, Arc::new(TokenIssuingAuth)), "/assets/index.js").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "console.log('cube')");
}

/// The app derives the REST API address from `basePath`. Sending Node's
/// `/cubejs-api` would point it at nothing on this server.
#[tokio::test]
async fn the_context_names_this_servers_api_prefix_and_carries_a_token() {
    let (_dir, config) = fixture();
    let (status, body) = get_json(
        app(config, Arc::new(TokenIssuingAuth)),
        "/playground/context",
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["basePath"], "/cube");
    assert_eq!(body["cubejsToken"], "signed:{}");
    // The connection wizard writes .env and installs npm packages.
    assert_eq!(body["shouldStartConnectionWizardFlow"], json!(false));
    // Neither outbound analytics nor Cube Cloud live preview.
    assert_eq!(body["telemetry"], json!(false));
    assert_eq!(body["livePreview"], json!(false));
    assert_eq!(body["dbType"], "postgres");
}

#[tokio::test]
async fn a_deployment_that_cannot_sign_says_so_instead_of_serving_a_broken_app() {
    let (_dir, config) = fixture();
    let (status, body) = get_json(app(config, Arc::new(CannotIssue)), "/playground/context").await;

    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{body}");
}

#[tokio::test]
async fn the_file_list_holds_the_model_and_nothing_else() {
    let (_dir, config) = fixture();
    let (status, body) =
        get_json(app(config, Arc::new(TokenIssuingAuth)), "/playground/files").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let names: Vec<&str> = body["files"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|f| f["fileName"].as_str().expect("a name"))
        .collect();

    // Nested files are listed by their path relative to the model directory,
    // and a non-model file is left out.
    assert_eq!(names, vec!["orders.yml", "views/sales.yml"]);
    assert_eq!(body["files"][0]["content"], "cubes: []");
}

/// The landing page reads an empty list as "no data model yet".
#[tokio::test]
async fn a_missing_model_directory_is_an_empty_list_not_an_error() {
    let (_dir, mut config) = fixture();
    config.schema_path = "/no/such/model/directory".to_string();

    let (status, body) =
        get_json(app(config, Arc::new(TokenIssuingAuth)), "/playground/files").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["files"], json!([]));
}

#[tokio::test]
async fn the_connector_status_reflects_what_this_build_carries() {
    let (_dir, config) = fixture();
    let router = || app(config.clone(), Arc::new(TokenIssuingAuth));

    let (status, body) = get_json(router(), "/playground/driver?driver=postgres").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "installed");

    // Known to Cube, no Rust driver.
    let (_, body) = get_json(router(), "/playground/driver?driver=athena").await;
    assert_eq!(body["status"], "error");

    let (status, _) = get_json(router(), "/playground/driver?driver=nonsense").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// Serving the app at the root must not turn every mistyped API path into
/// the page; clients rely on the JSON 404.
#[tokio::test]
async fn an_unknown_path_is_still_a_json_404() {
    let (_dir, config) = fixture();
    let (status, body) = get_json(
        app(config, Arc::new(TokenIssuingAuth)),
        "/cube/v1/no-such-endpoint",
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "Cannot GET /cube/v1/no-such-endpoint");
}

#[tokio::test]
async fn nothing_is_mounted_when_no_assets_are_configured() {
    let (_dir, mut config) = fixture();
    config.playground_path = None;

    let router = || app(config.clone(), Arc::new(TokenIssuingAuth));

    let (status, body) = get_json(router(), "/").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "Cannot GET /");

    let (status, _) = get_json(router(), "/playground/context").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_directory_without_a_build_mounts_nothing() {
    let (dir, mut config) = fixture();
    config.playground_path = Some(dir.path().join("empty").to_string_lossy().into_owned());
    std::fs::create_dir_all(dir.path().join("empty")).unwrap();

    let (status, _) = get_json(app(config, Arc::new(TokenIssuingAuth)), "/").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_actions_that_need_javascript_explain_themselves() {
    let (_dir, config) = fixture();

    let (status, body) = post_json(
        app(config, Arc::new(TokenIssuingAuth)),
        "/playground/env",
        json!({}),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    let message = body["error"].as_str().expect("a message");

    assert!(message.contains("YAML"), "{message}");
}

async fn post_json(router: axum::Router, path: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header(PLAYGROUND_KEY_HEADER, playground_key())
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();

    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// What the data-model page sends for two tables of a second source.
fn generate_request(format: &str) -> Value {
    json!({
        "format": format,
        "tables": [["sofiadb.dfos", "fiberCable"], ["sofiadb.dfos", "fibersegment"]],
        "tablesSchema": {
            "sofiadb.dfos": {
                "fiberCable": [
                    { "name": "id", "type": "UUID", "attributes": ["primaryKey"] },
                    { "name": "fiberCount", "type": "Nullable(Int32)", "attributes": [] },
                    { "name": "tags", "type": "Array(String)", "attributes": [] }
                ],
                "fibersegment": [{ "name": "id", "type": "String", "attributes": [] }]
            }
        }
    })
}

fn two_sources(config: &mut ServerConfig) {
    config
        .data_sources
        .push(("sofiadb".to_string(), "clickhouse".to_string()));
}

/// The app only offers YAML when the server reports Cube 0.31 or later.
#[tokio::test]
async fn the_reported_version_is_a_cube_release_that_has_yaml() {
    let (_dir, config) = fixture();
    let (_, body) = get_json(
        app(config, Arc::new(TokenIssuingAuth)),
        "/playground/context",
    )
    .await;

    let version = body["coreServerVersion"].as_str().expect("a version");
    let major: u32 = version.split('.').next().unwrap().parse().unwrap();
    assert!(major >= 1, "{version}");
}

#[tokio::test]
async fn yaml_cubes_are_written_for_the_picked_tables() {
    let (dir, mut config) = fixture();
    two_sources(&mut config);

    let (status, body) = post_json(
        app(config, Arc::new(TokenIssuingAuth)),
        "/playground/generate-schema",
        generate_request("yaml"),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["files"][0]["fileName"], "dfos_fiber_cable.yml");

    let written = std::fs::read_to_string(dir.path().join("model/dfos_fiber_cable.yml")).unwrap();
    assert!(written.contains("data_source: sofiadb"), "{written}");
    assert!(written.contains("`dfos`.`fiberCable`"), "{written}");
    assert!(written.contains("name: fiber_count"), "{written}");
    assert!(written.contains("primary_key: true"), "{written}");
    let doc: serde_yaml::Value = serde_yaml::from_str(&written).unwrap();
    assert_eq!(
        doc["cubes"][0]["measures"],
        serde_yaml::from_str::<serde_yaml::Value>("[{ name: count, type: count }]").unwrap(),
        "{written}"
    );
    // No Cube type for an array.
    assert!(!written.contains("tags"), "{written}");
    assert!(dir.path().join("model/dfos_fibersegment.yml").exists());
}

#[tokio::test]
async fn javascript_is_refused_and_nothing_is_written() {
    let (dir, mut config) = fixture();
    two_sources(&mut config);

    let (status, body) = post_json(
        app(config, Arc::new(TokenIssuingAuth)),
        "/playground/generate-schema",
        generate_request("js"),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(!dir.path().join("model/dfos_fiber_cable.yml").exists());
}

#[tokio::test]
async fn a_cube_the_user_wrote_is_never_overwritten() {
    let (dir, mut config) = fixture();
    two_sources(&mut config);
    let mine = "cubes:\n- name: dfos_fibersegment\n  sql_table: x\n";
    std::fs::write(dir.path().join("model/mine.yml"), mine).unwrap();

    let (status, body) = post_json(
        app(config, Arc::new(TokenIssuingAuth)),
        "/playground/generate-schema",
        generate_request("yaml"),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("mine.yml"),
        "{body}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("model/mine.yml")).unwrap(),
        mine
    );
    // All or nothing: the other table was not written either.
    assert!(!dir.path().join("model/dfos_fiber_cable.yml").exists());
}

/// A generator's file for the cube is moved out of its hands, joins and all.
#[tokio::test]
async fn a_generated_cube_is_adopted_rather_than_duplicated() {
    let (dir, mut config) = fixture();
    two_sources(&mut config);
    let generated_dir = dir.path().join("model/sofiadb");
    std::fs::create_dir_all(&generated_dir).unwrap();
    std::fs::write(
        generated_dir.join("dfos_fibersegment.yml"),
        "# Generated by sync. Do not edit.\ncubes:\n- name: dfos_fibersegment\n  sql: SELECT * FROM dfos.fibersegment FINAL\n",
    )
    .unwrap();

    let (status, body) = post_json(
        app(config, Arc::new(TokenIssuingAuth)),
        "/playground/generate-schema",
        generate_request("yaml"),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!generated_dir.join("dfos_fibersegment.yml").exists());
    let adopted = std::fs::read_to_string(dir.path().join("model/dfos_fibersegment.yml")).unwrap();
    assert_eq!(
        adopted,
        "cubes:\n- name: dfos_fibersegment\n  sql: SELECT * FROM dfos.fibersegment FINAL\n"
    );
}

#[tokio::test]
async fn a_security_context_token_carries_the_claims_the_user_typed() {
    let (_dir, config) = fixture();

    let request = Request::builder()
        .method("POST")
        .uri("/playground/token")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "payload": { "tenant": "acme" } }).to_string(),
        ))
        .unwrap();

    let response = app(config, Arc::new(TokenIssuingAuth))
        .oneshot(request)
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(body["token"], r#"signed:{"tenant":"acme"}"#);
}

/// A deployment with a `cube.yml` (one Postgres source) and a `.env` beside it.
fn with_config_file(dir: &tempfile::TempDir, config: &mut ServerConfig) -> std::path::PathBuf {
    let file = dir.path().join("cube.yml");
    std::fs::write(
        &file,
        "version: 1\ndata_sources:\n  default:\n    type: postgres\n    host: db\n    password: secret\n\nscheduled_refresh:\n  enabled: false\n",
    )
    .unwrap();
    std::fs::write(dir.path().join(".env"), "PORT=4000\n").unwrap();
    config.config_file = Some(file.clone());
    file
}

#[tokio::test]
async fn the_data_sources_are_listed_without_their_passwords() {
    let (dir, mut config) = fixture();
    with_config_file(&dir, &mut config);

    let (status, body) = get_json(
        app(config, Arc::new(TokenIssuingAuth)),
        "/playground/data-sources",
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["editable"], json!(true));
    assert_eq!(body["dataSources"][0]["name"], "default");
    assert_eq!(body["dataSources"][0]["host"], "db");
    assert_eq!(body["dataSources"][0]["hasPassword"], json!(true));
    assert!(!body.to_string().contains("secret"), "{body}");
    assert!(body["connectors"]
        .as_array()
        .unwrap()
        .contains(&json!("sqlite")));
}

#[tokio::test]
async fn a_data_source_that_connects_is_added_to_cube_yml_and_env() {
    let (dir, mut config) = fixture();
    let file = with_config_file(&dir, &mut config);
    let database = dir.path().join("local.db");

    let (status, body) = post_json(
        app(config, Arc::new(TokenIssuingAuth)),
        "/playground/data-sources",
        json!({
            "name": "local",
            "variables": {
                "CUBEJS_DB_TYPE": "sqlite",
                "CUBEJS_DB_NAME": database.to_string_lossy(),
                "CUBEJS_DB_PASS": "it's",
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["restarting"], json!(false));

    let yml = std::fs::read_to_string(&file).unwrap();
    assert!(yml.contains("  local:\n    type: sqlite\n"), "{yml}");
    // Still before the next top-level key, and that key is intact.
    assert!(
        yml.ends_with("scheduled_refresh:\n  enabled: false\n"),
        "{yml}"
    );

    let env = std::fs::read_to_string(dir.path().join(".env")).unwrap();
    assert!(env.starts_with("PORT=4000\n"), "{env}");
    assert!(
        env.contains("CUBEJS_DS_LOCAL_DB_PASS='it'\\''s'\n"),
        "{env}"
    );
    assert!(!env.contains("CUBEJS_DS_LOCAL_DB_TYPE"), "{env}");
    assert!(!yml.contains("it's"), "{yml}");
}

#[tokio::test]
async fn a_data_source_that_does_not_connect_is_not_saved() {
    let (dir, mut config) = fixture();
    let file = with_config_file(&dir, &mut config);
    let before = std::fs::read_to_string(&file).unwrap();

    let (status, body) = post_json(
        app(config, Arc::new(TokenIssuingAuth)),
        "/playground/data-sources",
        json!({
            "name": "broken",
            "variables": {
                "CUBEJS_DB_TYPE": "postgres",
                "CUBEJS_DB_HOST": "127.0.0.1",
                "CUBEJS_DB_PORT": "1",
                "CUBEJS_DB_NAME": "x",
                "CUBEJS_DB_USER": "x",
            }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("Cannot connect"),
        "{body}"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".env")).unwrap(),
        "PORT=4000\n"
    );
}

#[tokio::test]
async fn a_taken_or_malformed_name_is_refused() {
    let (dir, mut config) = fixture();
    with_config_file(&dir, &mut config);
    let router = || app(config.clone(), Arc::new(TokenIssuingAuth));
    let variables = json!({ "CUBEJS_DB_TYPE": "sqlite", "CUBEJS_DB_NAME": ":memory:" });

    let (status, body) = post_json(
        router(),
        "/playground/data-sources",
        json!({ "name": "default", "variables": variables }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"].as_str().unwrap().contains("already exists"),
        "{body}"
    );

    let (status, _) = post_json(
        router(),
        "/playground/data-sources",
        json!({ "name": "Bad-Name", "variables": variables }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn without_cube_yml_nothing_can_be_added() {
    let (_dir, config) = fixture();

    let (status, _) = post_json(
        app(config, Arc::new(TokenIssuingAuth)),
        "/playground/data-sources",
        json!({ "name": "local", "variables": { "CUBEJS_DB_TYPE": "sqlite" } }),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn a_connection_can_be_tested_without_saving_it() {
    let (dir, config) = fixture();
    let database = dir.path().join("t.db");

    let (status, body) = post_json(
        app(config, Arc::new(TokenIssuingAuth)),
        "/playground/data-sources/test",
        json!({ "variables": { "CUBEJS_DB_TYPE": "sqlite", "CUBEJS_DB_NAME": database.to_string_lossy() } }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], json!(true));
}

async fn send_json(
    router: axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header(PLAYGROUND_KEY_HEADER, playground_key())
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();

    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Adds a SQLite data source `local` to a deployment with a `cube.yml`.
async fn with_local_source(
    dir: &tempfile::TempDir,
    config: &mut ServerConfig,
) -> (std::path::PathBuf, String, String) {
    let file = with_config_file(dir, config);
    let yml_before = std::fs::read_to_string(&file).unwrap();
    let env_before = std::fs::read_to_string(dir.path().join(".env")).unwrap();
    let database = dir.path().join("local.db");

    let (status, body) = post_json(
        app(config.clone(), Arc::new(TokenIssuingAuth)),
        "/playground/data-sources",
        json!({
            "name": "local",
            "variables": {
                "CUBEJS_DB_TYPE": "sqlite",
                "CUBEJS_DB_NAME": database.to_string_lossy(),
                "CUBEJS_DB_PASS": "secret",
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    (file, yml_before, env_before)
}

#[tokio::test]
async fn the_edit_form_gets_the_settings_but_not_the_password() {
    let (dir, mut config) = fixture();
    with_local_source(&dir, &mut config).await;

    let (status, body) = send_json(
        app(config, Arc::new(TokenIssuingAuth)),
        "GET",
        "/playground/data-sources/local",
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["type"], "sqlite");
    assert!(body["variables"]["CUBEJS_DB_NAME"]
        .as_str()
        .unwrap()
        .ends_with("local.db"));
    assert_eq!(body["hasPassword"], json!(true));
    assert_eq!(body["secrets"], json!(["CUBEJS_DB_PASS"]));
    assert!(!body.to_string().contains("\"secret\""), "{body}");
}

#[tokio::test]
async fn an_edit_changes_the_setting_in_place_and_keeps_the_password() {
    let (dir, mut config) = fixture();
    with_local_source(&dir, &mut config).await;
    let other = dir.path().join("other.db");

    let (status, body) = send_json(
        app(config, Arc::new(TokenIssuingAuth)),
        "PUT",
        "/playground/data-sources/local",
        Some(json!({
            "variables": {
                "CUBEJS_DB_NAME": other.to_string_lossy(),
                "CUBEJS_DB_PASS": "",
            }
        })),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let env = std::fs::read_to_string(dir.path().join(".env")).unwrap();
    let expected = format!("CUBEJS_DS_LOCAL_DB_NAME='{}'\n", other.to_string_lossy());
    assert_eq!(env.matches("CUBEJS_DS_LOCAL_DB_NAME=").count(), 1, "{env}");
    assert!(env.contains(&expected), "{env}");
    assert!(!env.contains("local.db"), "{env}");
    // Not the variable the default source reads.
    assert!(!env.contains("\nCUBEJS_DB_NAME="), "{env}");
    assert!(env.contains("CUBEJS_DS_LOCAL_DB_PASS='secret'"), "{env}");
}

#[tokio::test]
async fn the_database_type_cannot_change() {
    let (dir, mut config) = fixture();
    with_local_source(&dir, &mut config).await;

    let (status, _) = send_json(
        app(config, Arc::new(TokenIssuingAuth)),
        "PUT",
        "/playground/data-sources/local",
        Some(json!({ "variables": { "CUBEJS_DB_TYPE": "duckdb" } })),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn deleting_a_source_leaves_the_files_as_they_were_before_it() {
    let (dir, mut config) = fixture();
    let (file, yml_before, env_before) = with_local_source(&dir, &mut config).await;

    let (status, body) = send_json(
        app(config, Arc::new(TokenIssuingAuth)),
        "DELETE",
        "/playground/data-sources/local",
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), yml_before);
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".env")).unwrap(),
        env_before
    );
}

#[tokio::test]
async fn a_source_a_cube_uses_or_the_default_one_is_not_deleted() {
    let (dir, mut config) = fixture();
    let (file, _, _) = with_local_source(&dir, &mut config).await;
    std::fs::write(
        dir.path().join("model/local_orders.yml"),
        "cubes:\n- name: local_orders\n  data_source: local\n  sql_table: orders\n",
    )
    .unwrap();
    let yml = std::fs::read_to_string(&file).unwrap();
    let router = || app(config.clone(), Arc::new(TokenIssuingAuth));

    let (status, body) =
        send_json(router(), "DELETE", "/playground/data-sources/local", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"].as_str().unwrap().contains("local_orders"),
        "{body}"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), yml);

    let (status, _) = send_json(router(), "DELETE", "/playground/data-sources/default", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// Grants `data_sources`, as `CUBEJS_DEFAULT_API_SCOPES` would.
#[derive(Debug)]
struct DataSourcesAuth;

#[async_trait]
impl AuthService for DataSourcesAuth {
    async fn authenticate(&self, _: Option<&str>) -> Result<AuthenticatedRequest, ApiError> {
        Ok(AuthenticatedRequest {
            security_context: json!({}),
            signed_with_playground_auth_secret: false,
            token_verified: true,
        })
    }

    async fn api_scopes(&self, _: &Value) -> Result<Vec<String>, ApiError> {
        Ok(vec!["meta".to_string(), "data_sources".to_string()])
    }
}

#[tokio::test]
async fn the_rest_api_needs_the_data_sources_scope() {
    let (dir, mut config) = fixture();
    with_config_file(&dir, &mut config);

    // TokenIssuingAuth grants meta and data only.
    let (status, body) = send_json(
        app(config, Arc::new(TokenIssuingAuth)),
        "GET",
        "/cube/v1/data-sources",
        None,
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"], "API scope is missing: data_sources");
}

#[tokio::test]
async fn the_rest_api_lists_and_adds_data_sources() {
    let (dir, mut config) = fixture();
    let file = with_config_file(&dir, &mut config);
    let router = || app(config.clone(), Arc::new(DataSourcesAuth));
    let database = dir.path().join("api.db");

    let (status, body) = send_json(
        router(),
        "POST",
        "/cube/v1/data-sources",
        Some(json!({
            "name": "from_api",
            "variables": { "CUBEJS_DB_TYPE": "sqlite", "CUBEJS_DB_NAME": database.to_string_lossy() }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(std::fs::read_to_string(&file)
        .unwrap()
        .contains("  from_api:\n"));

    // What the Playground's Data Source page reads.
    let (status, body) = send_json(router(), "GET", "/playground/data-sources", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let names: Vec<&str> = body["dataSources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["default", "from_api"]);

    let (status, body) = send_json(router(), "GET", "/cube/v1/data-sources/from_api", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["type"], "sqlite");

    let (status, body) =
        send_json(router(), "DELETE", "/cube/v1/data-sources/from_api", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!std::fs::read_to_string(&file).unwrap().contains("from_api"));
}

/// Grants `execute` too, as `CUBEJS_DEFAULT_API_SCOPES` would.
#[derive(Debug)]
struct ExecuteAuth;

#[async_trait]
impl AuthService for ExecuteAuth {
    async fn authenticate(&self, _: Option<&str>) -> Result<AuthenticatedRequest, ApiError> {
        Ok(AuthenticatedRequest {
            security_context: json!({}),
            signed_with_playground_auth_secret: false,
            token_verified: true,
        })
    }

    async fn api_scopes(&self, _: &Value) -> Result<Vec<String>, ApiError> {
        Ok(vec!["execute".to_string()])
    }
}

#[tokio::test]
async fn executing_a_statement_needs_the_execute_scope() {
    let (dir, mut config) = fixture();
    with_local_source(&dir, &mut config).await;

    // data_sources manages connections; it does not run statements.
    let (status, body) = send_json(
        app(config, Arc::new(DataSourcesAuth)),
        "POST",
        "/cube/v1/execute",
        Some(json!({ "connection": "local", "query": "CREATE TABLE t (id INTEGER)" })),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"], "API scope is missing: execute");
}

#[tokio::test]
async fn tables_and_views_are_created_through_execute() {
    let (dir, mut config) = fixture();
    with_local_source(&dir, &mut config).await;
    let router = || app(config.clone(), Arc::new(ExecuteAuth));
    let run = |query: &str| {
        send_json(
            router(),
            "POST",
            "/cube/v1/execute",
            Some(json!({ "connection": "local", "query": query })),
        )
    };

    for statement in [
        "CREATE TABLE produtos (id INTEGER PRIMARY KEY, nome TEXT, preco REAL)",
        "INSERT INTO produtos (nome, preco) VALUES ('Cabo', 10.5), ('Conector', 2)",
        "CREATE VIEW caros AS SELECT nome FROM produtos WHERE preco > 5",
    ] {
        let (status, body) = run(statement).await;
        assert_eq!(status, StatusCode::OK, "{statement}: {body}");
    }

    let (status, body) = run("SELECT nome FROM caros").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["columns"], json!(["nome"]));
    assert_eq!(body["rowCount"], json!(1));
    assert_eq!(body["rows"][0][0], "Cabo");
    assert_eq!(body["dataSource"], "local");

    // A database error is the caller's, not the server's.
    let (status, body) = run("CREATE TABLE produtos (id INTEGER)").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("already exists"),
        "{body}"
    );
}

#[tokio::test]
async fn the_database_parameter_picks_where_the_statement_runs() {
    let (dir, mut config) = fixture();
    with_local_source(&dir, &mut config).await;
    let other = dir.path().join("other.db");

    let (status, body) = send_json(
        app(config, Arc::new(ExecuteAuth)),
        "POST",
        "/cube/v1/execute",
        Some(json!({ "connection": "local", "database": other.to_string_lossy(), "query": "CREATE TABLE em_outro (id INTEGER)" })),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["database"], json!(other.to_string_lossy()));
    // SQLite's database is the file: the table is in the other one.
    assert!(other.exists());
    assert!(
        !dir.path().join("local.db").exists() || {
            let bytes = std::fs::read(dir.path().join("local.db")).unwrap();
            !String::from_utf8_lossy(&bytes).contains("em_outro")
        }
    );
}

#[tokio::test]
async fn a_connection_can_be_given_by_its_settings() {
    let (dir, config) = fixture();
    let database = dir.path().join("adhoc.db");

    let (status, body) = send_json(
        app(config, Arc::new(ExecuteAuth)),
        "POST",
        "/cube/v1/execute",
        Some(json!({
            "connection": { "CUBEJS_DB_TYPE": "sqlite", "CUBEJS_DB_NAME": database.to_string_lossy(), "CUBEJS_DB_MAX_POOL": 1 },
            "query": "CREATE TABLE avulsa (id INTEGER)"
        })),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["dataSource"], "(ad hoc)");
    assert!(database.exists());
}

#[tokio::test]
async fn a_request_without_a_connection_is_refused() {
    let (dir, mut config) = fixture();
    with_config_file(&dir, &mut config);
    let router = || app(config.clone(), Arc::new(ExecuteAuth));

    let (status, body) = send_json(
        router(),
        "POST",
        "/cube/v1/execute",
        Some(json!({ "query": "SELECT 1" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("Name the connection"),
        "{body}"
    );

    let (status, _) = send_json(
        router(),
        "POST",
        "/cube/v1/execute",
        Some(json!({ "connection": "missing", "query": "SELECT 1" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_playground_key_is_needed_to_change_anything() {
    let (dir, mut config) = fixture();
    two_sources(&mut config);
    with_config_file(&dir, &mut config);
    let router = || app(config.clone(), Arc::new(TokenIssuingAuth));

    let (_, context) = get_json(router(), "/playground/context").await;
    assert_eq!(context["playgroundKey"], playground_key());

    for (method, path) in [
        ("POST", "/playground/generate-schema"),
        ("GET", "/playground/data-sources"),
        ("POST", "/playground/data-sources"),
        ("POST", "/playground/data-sources/test"),
        ("GET", "/playground/data-sources/default"),
        ("PUT", "/playground/data-sources/default"),
        ("DELETE", "/playground/data-sources/default"),
    ] {
        for key in [None, Some("not-the-key")] {
            let mut request = Request::builder()
                .method(method)
                .uri(path)
                .header(header::CONTENT_TYPE, "application/json");
            if let Some(key) = key {
                request = request.header(PLAYGROUND_KEY_HEADER, key);
            }
            let request = request
                .body(Body::from(generate_request("yaml").to_string()))
                .unwrap();
            let response = router().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {path}");
        }
    }
    assert!(!dir.path().join("model/dfos_fiber_cable.yml").exists());
}

/// Another site may call the REST API, but gets no CORS answer from the
/// Playground's helper API, so its page can neither read the key nor send
/// the JSON the routes take.
#[tokio::test]
async fn only_the_rest_api_answers_another_site() {
    let (_dir, config) = fixture();
    let preflight = |path: &str| {
        Request::builder()
            .method("OPTIONS")
            .uri(path)
            .header(header::ORIGIN, "https://elsewhere.example")
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
            .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "content-type")
            .body(Body::empty())
            .unwrap()
    };
    let allows = |response: &axum::response::Response| {
        response
            .headers()
            .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN)
    };

    let router = app(config, Arc::new(TokenIssuingAuth));
    let api = router
        .clone()
        .oneshot(preflight("/cube/v1/load"))
        .await
        .unwrap();
    assert!(allows(&api));
    for path in [
        "/playground/context",
        "/playground/data-sources",
        "/cube/v1/data-sources",
        "/cube/v1/data-sources/default",
        "/cube/v1/execute",
    ] {
        let response = router.clone().oneshot(preflight(path)).await.unwrap();
        assert!(!allows(&response), "{path}");
    }
}

/// `customers` in two schemas of the default source.
#[tokio::test]
async fn same_named_tables_of_two_schemas_both_get_a_cube() {
    let (dir, config) = fixture();
    let columns = json!([{ "name": "id", "type": "integer", "attributes": [] }]);

    let (status, body) = post_json(
        app(config, Arc::new(TokenIssuingAuth)),
        "/playground/generate-schema",
        json!({
            "format": "yaml",
            "tables": [["public", "customers"], ["archive", "customers"]],
            "tablesSchema": { "public": { "customers": columns }, "archive": { "customers": columns } },
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let archive = std::fs::read_to_string(dir.path().join("model/archive_customers.yml")).unwrap();
    assert!(archive.contains("\"archive\".\"customers\""), "{archive}");
    let public = std::fs::read_to_string(dir.path().join("model/customers.yml")).unwrap();
    assert!(public.contains("\"public\".\"customers\""), "{public}");
}

/// A generator's cube with the same name but for another table is left
/// alone, not adopted in the picked table's place.
#[tokio::test]
async fn another_tables_generated_cube_is_not_adopted() {
    let (dir, mut config) = fixture();
    two_sources(&mut config);
    let generated_dir = dir.path().join("model/sofiadb");
    std::fs::create_dir_all(&generated_dir).unwrap();
    let generated = "# Generated by sync. Do not edit.\ncubes:\n- name: dfos_fibersegment\n  data_source: sofiadb\n  sql_table: dfos.fiber_segment_v2\n";
    std::fs::write(generated_dir.join("dfos_fibersegment.yml"), generated).unwrap();

    let (status, body) = post_json(
        app(config, Arc::new(TokenIssuingAuth)),
        "/playground/generate-schema",
        generate_request("yaml"),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        std::fs::read_to_string(generated_dir.join("dfos_fibersegment.yml")).unwrap(),
        generated
    );
    assert!(!dir.path().join("model/dfos_fiber_cable.yml").exists());
}

#[tokio::test]
async fn a_password_in_a_url_is_neither_listed_nor_sent_to_the_form() {
    let (dir, mut config) = fixture();
    let file = dir.path().join("cube.yml");
    std::fs::write(
        &file,
        "version: 1\ndata_sources:\n  default:\n    type: postgres\n    url: postgres://reader:hunter2@db:5432/sales\n",
    )
    .unwrap();
    config.config_file = Some(file);
    let router = || app(config.clone(), Arc::new(TokenIssuingAuth));

    let (status, body) = get_json(router(), "/playground/data-sources").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["dataSources"][0]["host"],
        "postgres://reader@db:5432/sales"
    );
    assert!(!body.to_string().contains("hunter2"), "{body}");

    let (status, body) = get_json(router(), "/playground/data-sources/default").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["secrets"], json!(["CUBEJS_DB_URL"]));
    assert!(!body.to_string().contains("hunter2"), "{body}");
}

#[tokio::test]
async fn a_value_written_in_cube_yml_is_not_cleared_from_the_form() {
    let (dir, mut config) = fixture();
    let file = with_config_file(&dir, &mut config);
    let before = std::fs::read_to_string(&file).unwrap();

    let (status, body) = send_json(
        app(config, Arc::new(TokenIssuingAuth)),
        "PUT",
        "/playground/data-sources/default",
        Some(json!({ "variables": { "CUBEJS_DB_HOST": "" } })),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("cube.yml"),
        "{body}"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".env")).unwrap(),
        "PORT=4000\n"
    );
}

#[tokio::test]
async fn a_source_a_tenants_cube_uses_is_not_deleted() {
    let (dir, mut config) = fixture();
    let (file, _, _) = with_local_source(&dir, &mut config).await;
    let tenant_model = dir.path().join("tenants/acme");
    std::fs::create_dir_all(&tenant_model).unwrap();
    std::fs::write(
        tenant_model.join("orders.yml"),
        "cubes:\n- name: acme_orders\n  data_source: local\n  sql_table: orders\n",
    )
    .unwrap();
    let yml = std::fs::read_to_string(&file).unwrap()
        + &format!(
            "\ntenants:\n  claim: tenant\n  rules:\n    - match: acme\n      model_path: {}\n",
            tenant_model.display()
        );
    std::fs::write(&file, &yml).unwrap();

    let (status, body) = send_json(
        app(config, Arc::new(TokenIssuingAuth)),
        "DELETE",
        "/playground/data-sources/local",
        None,
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("acme_orders"),
        "{body}"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), yml);
}

/// Security checks off and no token, as a request from anywhere would be,
/// with default scopes that list `data_sources` and `execute`.
#[derive(Debug)]
struct NoToken;

#[async_trait]
impl AuthService for NoToken {
    async fn authenticate(&self, _: Option<&str>) -> Result<AuthenticatedRequest, ApiError> {
        Ok(AuthenticatedRequest {
            security_context: json!({}),
            signed_with_playground_auth_secret: false,
            token_verified: false,
        })
    }

    async fn api_scopes(&self, _: &Value) -> Result<Vec<String>, ApiError> {
        Ok(vec!["data_sources".to_string(), "execute".to_string()])
    }
}

#[tokio::test]
async fn managing_connections_takes_a_verified_token_whatever_the_default_scopes() {
    let (dir, mut config) = fixture();
    let file = with_config_file(&dir, &mut config);
    let before = std::fs::read_to_string(&file).unwrap();
    let router = || app(config.clone(), Arc::new(NoToken));

    let (status, body) = send_json(router(), "GET", "/cube/v1/data-sources", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    let (status, body) = send_json(
        router(),
        "POST",
        "/cube/v1/execute",
        Some(json!({
            "connection": { "CUBEJS_DB_TYPE": "sqlite", "CUBEJS_DB_NAME": ":memory:" },
            "query": "SELECT 1",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    let (status, body) = send_json(
        router(),
        "POST",
        "/cube/v1/data-sources",
        Some(json!({
            "name": "x",
            "variables": { "CUBEJS_DB_TYPE": "sqlite", "CUBEJS_DB_NAME": ":memory:" }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
}

async fn get_with_host(router: axum::Router, path: &str, host: &str) -> StatusCode {
    let request = Request::builder()
        .uri(path)
        .header(header::HOST, host)
        .header(PLAYGROUND_KEY_HEADER, playground_key())
        .body(Body::empty())
        .unwrap();
    router.oneshot(request).await.unwrap().status()
}

/// A rebound domain would make another site's page the same origin.
#[tokio::test]
async fn the_playground_answers_only_under_names_that_cannot_be_rebound() {
    let (dir, mut config) = fixture();
    with_config_file(&dir, &mut config);
    config.playground_hosts = vec!["cube.lan".to_string()];
    let router = || app(config.clone(), Arc::new(TokenIssuingAuth));

    for host in [
        "localhost:4000",
        "127.0.0.1:4000",
        "[::1]:4000",
        "cube.lan:4000",
    ] {
        let status = get_with_host(router(), "/playground/context", host).await;
        assert_eq!(status, StatusCode::OK, "{host}");
    }
    for path in [
        "/playground/context",
        "/playground/data-sources",
        "/playground/files",
    ] {
        let status = get_with_host(router(), path, "rebound.example:4000").await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
}

#[tokio::test]
async fn a_key_or_a_secret_in_a_jdbc_url_is_not_sent_back() {
    let (dir, mut config) = fixture();
    with_local_source(&dir, &mut config).await;
    let env = dir.path().join(".env");
    let mut text = std::fs::read_to_string(&env).unwrap();
    text.push_str("CUBEJS_DS_LOCAL_DB_SSL_KEY='-----BEGIN PRIVATE KEY-----'\n");
    text.push_str(
        "CUBEJS_DS_LOCAL_DB_URL='jdbc:databricks://h:443/default;UID=token;PWD=dapi123'\n",
    );
    std::fs::write(&env, text).unwrap();
    let router = || app(config.clone(), Arc::new(TokenIssuingAuth));

    let (status, body) = get_json(router(), "/playground/data-sources/local").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let secrets = body["secrets"].as_array().unwrap();
    assert!(secrets.contains(&json!("CUBEJS_DB_SSL_KEY")), "{body}");
    assert!(secrets.contains(&json!("CUBEJS_DB_URL")), "{body}");
    assert!(!body.to_string().contains("BEGIN"), "{body}");
    assert!(!body.to_string().contains("dapi123"), "{body}");

    let (status, body) = get_json(router(), "/playground/data-sources").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!body.to_string().contains("dapi123"), "{body}");
}

/// `CUBEJS_DS_SALES_` is also the start of `CUBEJS_DS_SALES_EU_`.
#[tokio::test]
async fn a_source_named_like_the_start_of_another_keeps_to_its_own_settings() {
    let (dir, mut config) = fixture();
    with_config_file(&dir, &mut config);
    let router = || app(config.clone(), Arc::new(TokenIssuingAuth));
    for name in ["sales", "sales_eu"] {
        let database = dir.path().join(format!("{name}.db"));
        let (status, body) = post_json(
            router(),
            "/playground/data-sources",
            json!({
                "name": name,
                "variables": {
                    "CUBEJS_DB_TYPE": "sqlite",
                    "CUBEJS_DB_NAME": database.to_string_lossy(),
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    let (status, body) = get_json(router(), "/playground/data-sources/sales").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["variables"].as_object().unwrap().len(), 1, "{body}");
    let database = body["variables"]["CUBEJS_DB_NAME"].as_str().unwrap();
    assert!(database.ends_with("sales.db"), "{body}");

    let (status, body) =
        send_json(router(), "DELETE", "/playground/data-sources/sales", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let env = std::fs::read_to_string(dir.path().join(".env")).unwrap();
    assert!(!env.contains("CUBEJS_DS_SALES_DB_NAME"), "{env}");
    assert!(env.contains("CUBEJS_DS_SALES_EU_DB_NAME="), "{env}");
}
