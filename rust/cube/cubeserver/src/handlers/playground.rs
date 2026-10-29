//! The Playground's helper API (`/playground/*`).
//!
//! The query builder's part is implemented, generating a YAML data model
//! from the database (`scaffold.rs`) and the "Data Source" page
//! (`data_sources.rs`). The Node.js dev server
//! (`cubejs-server-core/src/core/DevServer.ts`) also runs a connection wizard
//! that rewrites the default source in `.env`, and scaffolds a dashboard app
//! through npm; those routes answer 501 with a message naming the
//! alternative.
//!
//! These routes are not behind the auth middleware, because the app fetches
//! `/playground/context` to *obtain* its token. That is why the server mounts
//! the Playground only when authentication is not enforced; see `main.rs`.
//! The routes that write the model or the data sources, or connect to a
//! database, also want the key `/playground/context` hands the page
//! ([`playground_key`]). The server sends these routes no CORS headers (see
//! `app.rs`), so another site the developer visits cannot read the key, nor
//! send the JSON the routes take. Nor can it rebind a domain of its own to
//! this server to become the same origin: every route here answers only
//! under `localhost`, an IP address, or a name in `CUBEJS_PLAYGROUND_HOSTS`.
//!
//! With RBAC on it hands out no token: the context says a login is needed
//! (`authRequired`), `/playground/token` is gone, and every other route wants
//! a platform admin's verified token ([`require_platform_admin`]). The
//! Playground is then mounted whatever `NODE_ENV` says.

use std::net::IpAddr;
use std::sync::OnceLock;
use std::time::Duration;

use axum::extract::{Query, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::app::AppState;
use crate::error::ApiError;
use crate::services::RequestContext;

/// The Cube release whose API this server implements (`lerna.json`).
pub const CUBE_API_VERSION: &str = "1.7.40";

/// The header the page sends [`playground_key`] in.
pub const PLAYGROUND_KEY_HEADER: &str = "x-playground-key";

/// How long one data source may take to describe its tables.
const SCHEMA_TIMEOUT: Duration = Duration::from_secs(30);

/// A secret made when the process starts, which `/playground/context` hands
/// the page. A restart makes a new one; the Data Source page fetches it
/// again once the server is back.
pub fn playground_key() -> &'static str {
    static KEY: OnceLock<String> = OnceLock::new();
    KEY.get_or_init(|| uuid::Uuid::new_v4().simple().to_string())
}

pub fn routes(state: AppState) -> Router<AppState> {
    let keyed = Router::new()
        .route("/playground/generate-schema", post(generate_schema))
        .route(
            "/playground/data-sources",
            get(super::data_sources::list).post(super::data_sources::create_unless_tracked),
        )
        .route(
            "/playground/data-sources/test",
            post(super::data_sources::test),
        )
        .route(
            "/playground/data-sources/:name",
            get(super::data_sources::get)
                .put(super::data_sources::update)
                .delete(super::data_sources::delete_with_its_grants),
        )
        .route_layer(axum::middleware::from_fn(require_key));

    let mut helpers = Router::new()
        .route("/playground/files", get(files))
        .route("/playground/db-schema", get(db_schema))
        .route("/playground/driver", get(driver))
        .merge(keyed)
        // Not ported. Named individually so the message says why, rather
        // than letting the app see an unexplained 404.
        .route("/playground/env", post(not_ported))
        .route("/playground/test-connection", post(not_ported))
        .route("/playground/schema/pre-aggregation", post(not_ported));
    // With RBAC on they read the model and the databases and write files,
    // so only platform admins reach them. The auth middleware runs first.
    if state.rbac.is_some() {
        helpers = helpers
            .route_layer(axum::middleware::from_fn(require_platform_admin))
            .route_layer(axum::middleware::from_fn_with_state(
                state.clone(),
                super::auth::auth_middleware,
            ));
    }

    Router::new()
        .route("/playground/context", get(context))
        .route(
            "/playground/token",
            // A token for any claims would be a login for anyone.
            match state.rbac {
                Some(_) => post(token_gone),
                None => post(token),
            },
        )
        .merge(helpers)
        .route_layer(axum::middleware::from_fn_with_state(state, require_host))
}

/// `GET /playground/context` — the only call the app must have to boot.
///
/// `basePath` tells the app where the REST API lives; it builds
/// `window.location.origin + basePath + "/v1"` from it, so the value must be
/// this server's API prefix rather than Node's `/cubejs-api`.
async fn context(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let mut body = json!({
        "playgroundKey": playground_key(),
        "basePath": state.config.base_path,
        // The wizard writes `.env` and installs npm driver packages, neither
        // of which exists here, so the app must not be sent to it.
        "shouldStartConnectionWizardFlow": false,
        // No outbound analytics from a self-hosted Rust server.
        "telemetry": false,
        // Live preview talks to Cube Cloud.
        "livePreview": false,
        "dbType": state.config.data_sources.first().map(|(_, db_type)| db_type.clone()),
        // Namespaces the app's `localStorage` keys, so two deployments in one
        // browser keep their own query tabs.
        "identifier": state.config.app_identifier(),
        "anonymousId": Value::Null,
        // The app gates features on this (YAML generation needs 0.31,
        // GraphQL 0.29), reading it as a Cube release. This crate's own
        // version would turn them all off, so the Cube release whose API
        // this server implements is sent instead.
        "coreServerVersion": CUBE_API_VERSION,
        "dockerVersion": Value::Null,
        "projectFingerprint": Value::Null,
        "isDocker": false,
        "previewFeatures": false,
    });

    if state.rbac.is_some() {
        // No token: the user logs in. The key goes with the flag saying so,
        // and opens nothing without an admin's token.
        body["authRequired"] = json!(true);
        body["rbac"] = json!(true);
    } else {
        // `jwt.sign({}, apiSecret, { expiresIn: '1d' })` in `DevServer.ts:52`.
        body["cubejsToken"] = json!(state.auth.issue_token(Map::new()).await?);
    }

    Ok(Json(body))
}

/// `GET /playground/files` — the data model files on disk.
///
/// The landing page reads this to decide where to send the user: an empty
/// list means "no model yet" and routes to the schema page, anything else
/// routes to the query builder.
async fn files(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let root = std::path::Path::new(&state.config.schema_path);

    let mut files = Vec::new();
    collect_model_files(root, root, &mut files)?;
    files.sort_by(|a, b| a["fileName"].as_str().cmp(&b["fileName"].as_str()));

    Ok(Json(json!({ "files": files })))
}

/// Walks the model directory, listing YAML files by their path relative to
/// it. Node returns `fileName`, `content` and `absPath`.
fn collect_model_files(
    root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<Value>,
) -> Result<(), ApiError> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        // A missing model directory is not an error here: the app reads an
        // empty list as "no model yet" and says so in the interface.
        return Ok(());
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_model_files(root, &path, out)?;
            continue;
        }

        let is_model = matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("yml") | Some("yaml")
        );
        if !is_model {
            continue;
        }

        let name = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();

        out.push(json!({
            "fileName": name,
            "content": std::fs::read_to_string(&path).unwrap_or_default(),
            "absPath": path.canonicalize().unwrap_or(path.clone()).to_string_lossy(),
            "readOnly": true,
        }));
    }

    Ok(())
}

#[derive(Debug, Deserialize)]
struct TokenRequest {
    #[serde(default)]
    payload: Map<String, Value>,
}

/// `POST /playground/token` — a token carrying the security context the user
/// typed into the Playground, so they can preview row-level security.
async fn token(
    State(state): State<AppState>,
    Json(request): Json<TokenRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let token = state.auth.issue_token(request.payload).await?;

    Ok(Json(json!({ "token": token })))
}

/// `POST /playground/token` with RBAC on.
async fn token_gone(request: Request) -> ApiError {
    ApiError::not_found(format!(
        "Cannot {} {}",
        request.method(),
        request.uri().path()
    ))
}

#[derive(Debug, Deserialize)]
struct DriverQuery {
    driver: Option<String>,
}

/// `GET /playground/driver` — whether a connector is available.
///
/// Node.js answers whether the npm package is installed, and can install it
/// on demand. Here the set is fixed when the binary is built, so a connector
/// is either compiled in or it is not.
async fn driver(Query(query): Query<DriverQuery>) -> Result<impl IntoResponse, ApiError> {
    let name = query.driver.unwrap_or_default();

    if !cubedriver::DriverFactory::is_known(&name) {
        return Err(ApiError::bad_request("Wrong driver"));
    }

    Ok(Json(json!({
        "status": if cubedriver::DriverFactory::is_implemented(&name) {
            "installed"
        } else {
            // Not an error: the app renders this as "not available".
            "error"
        }
    })))
}

/// `GET /playground/db-schema` — the tables of the data sources, for the
/// data-model page.
///
/// The driver is built the same way the orchestrator builds it, from the
/// environment, so the page describes the database queries actually run
/// against.
///
/// With `?dataSource=` it answers for that source alone, as Node.js does.
/// Without it, Node.js describes only `default`, and the app never sends the
/// parameter: a second source's tables would never show. So every configured
/// source is listed, the schemas of all but `default` named
/// `<data source>.<schema>` so two sources' schemas cannot collide (and a
/// `default` schema that would read as one of those, `default.<schema>`). A
/// source that cannot be read is left out rather than hiding the others, and
/// named in `errors`.
async fn db_schema(
    State(state): State<AppState>,
    Query(query): Query<DataSourceQuery>,
) -> Result<impl IntoResponse, ApiError> {
    if let Some(data_source) = query.data_source {
        let structure = data_source_schema(&data_source).await?;
        return Ok(Json(json!({ "tablesSchema": structure })));
    }

    let mut names: Vec<String> = state
        .config
        .data_sources
        .iter()
        .map(|(n, _)| n.clone())
        .collect();
    if names.is_empty() {
        names.push("default".to_string());
    }

    let results = futures::future::join_all(names.iter().map(|n| data_source_schema(n))).await;

    let mut merged = cubedriver::DatabaseStructure::new();
    let mut errors = Vec::new();
    for (name, result) in names.iter().zip(results) {
        match result {
            Ok(structure) => {
                for (schema, tables) in structure {
                    let key = if name != "default" {
                        format!("{name}.{schema}")
                    } else if super::scaffold::is_prefixed(&schema, &state.config.data_sources) {
                        format!("default.{schema}")
                    } else {
                        schema
                    };
                    merged.entry(key).or_default().extend(tables);
                }
            }
            Err(e) => {
                tracing::warn!(data_source = %name, error = %e.body.error, "cannot list the tables of a data source");
                errors.push(e.body.error);
            }
        }
    }

    if merged.is_empty() && !errors.is_empty() {
        return Err(ApiError::bad_request(errors.join("; ")));
    }
    Ok(Json(json!({ "tablesSchema": merged, "errors": errors })))
}

async fn data_source_schema(data_source: &str) -> Result<cubedriver::DatabaseStructure, ApiError> {
    // A database that never answers, while connecting or once connected,
    // would otherwise hold up every other source's tables.
    let no_answer = || {
        ApiError::bad_request(format!(
            "{data_source}: no answer in {}s",
            SCHEMA_TIMEOUT.as_secs()
        ))
    };
    let driver = tokio::time::timeout(
        SCHEMA_TIMEOUT,
        crate::orchestrator_adapter::driver_factory()(data_source.to_string()),
    )
    .await
    .map_err(|_| no_answer())?
    .map_err(|e| ApiError::bad_request(format!("{data_source}: {e}")))?;

    let result = tokio::time::timeout(SCHEMA_TIMEOUT, driver.tables_schema()).await;
    let _ = driver.release().await;
    match result {
        Ok(Ok(structure)) => Ok(structure),
        Ok(Err(e)) => Err(ApiError::bad_request(format!("{data_source}: {e}"))),
        Err(_) => Err(no_answer()),
    }
}

#[derive(Debug, Deserialize)]
struct DataSourceQuery {
    #[serde(rename = "dataSource")]
    data_source: Option<String>,
}

/// `POST /playground/generate-schema` — writes YAML cubes for the tables
/// picked on the data-model page; see `scaffold.rs`.
async fn generate_schema(
    State(state): State<AppState>,
    Json(request): Json<super::scaffold::GenerateRequest>,
) -> Result<impl IntoResponse, ApiError> {
    // Not while a data source is being deleted: the delete checks no cube
    // uses it, and a cube written meanwhile would.
    let _guard = super::data_sources::WRITE.lock().await;
    let files = super::scaffold::generate(
        std::path::Path::new(&state.config.schema_path),
        &state.config.data_sources,
        &request,
    )?;

    Ok(Json(json!({ "files": files })))
}

/// Refuses a request whose host is a name the deployment did not list, or
/// that names none. A DNS-rebinding page points a domain of its own at this
/// server, which makes it the same origin as the Playground; an IP address
/// or `localhost` cannot be rebound like that. The host is the `Host`
/// header, or over HTTP/2 the URI's authority.
async fn require_host(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| request.uri().authority().map(|a| a.to_string()));
    match host {
        Some(host) if host_allowed(&host, &state.config.playground_hosts) => {
            next.run(request).await
        }
        Some(host) => ApiError::forbidden(format!(
            "The Playground answers only under localhost, an IP address or a name \
             in CUBEJS_PLAYGROUND_HOSTS, not {host}"
        ))
        .into_response(),
        None => ApiError::forbidden("The Playground needs to know the host it is called under")
            .into_response(),
    }
}

fn host_allowed(host: &str, listed: &[String]) -> bool {
    // `name:port`, `[v6]:port`, or either without the port.
    let name = match host.rsplit_once(':') {
        Some((name, port)) if port.chars().all(|c| c.is_ascii_digit()) && !name.ends_with(':') => {
            name
        }
        _ => host,
    };
    let name = name.to_ascii_lowercase();
    let bare = name.trim_start_matches('[').trim_end_matches(']');
    name == "localhost"
        || name.ends_with(".localhost")
        || bare.parse::<IpAddr>().is_ok()
        || listed.contains(&name)
}

/// Compares in time that does not depend on where the two first differ.
fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0, |acc, (x, y)| acc | (x ^ y))
            == 0
}

async fn require_key(request: Request, next: Next) -> Response {
    let key = request
        .headers()
        .get(PLAYGROUND_KEY_HEADER)
        .and_then(|v| v.to_str().ok());
    if !key.is_some_and(|key| same_secret(key, playground_key())) {
        return ApiError::forbidden(
            "This Playground action needs the key the page gets from /playground/context; \
             reload the Playground.",
        )
        .into_response();
    }
    next.run(request).await
}

/// With RBAC on: a platform admin's token, verified. Runs after the auth
/// middleware, which answers 401 for a missing or invalid token; one that
/// went through unverified (security checks off) is no login either.
async fn require_platform_admin(request: Request, next: Next) -> Response {
    let Some(ctx) = request.extensions().get::<RequestContext>() else {
        return crate::rbac::unauthenticated().into_response();
    };
    if !ctx.token_verified || ctx.user.is_none() {
        return crate::rbac::unauthenticated()
            .with_request_id(&ctx.request_id)
            .into_response();
    }
    if !ctx.platform_admin {
        return ApiError::forbidden("Permission denied: the Playground is for platform admins")
            .with_request_id(&ctx.request_id)
            .into_response();
    }
    next.run(request).await
}

async fn not_ported() -> ApiError {
    ApiError::not_implemented(
        "This Playground action is not available on the Rust server. \
         Data sources are added on the Data Source page, and the \
         dashboard-app scaffolding runs JavaScript, which this deployment \
         does not have. Edit the YAML model files directly; the server \
         reloads them while it runs.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_names_that_cannot_be_rebound_or_are_listed_pass() {
        let listed = vec!["cube.lan".to_string()];
        for host in [
            "localhost",
            "localhost:4000",
            "app.localhost:4000",
            "127.0.0.1:4000",
            "10.0.0.5",
            "[::1]:4000",
            "::1",
            "CUBE.LAN:4000",
        ] {
            assert!(host_allowed(host, &listed), "{host}");
        }
        for host in [
            "evil.example",
            "evil.example:4000",
            "localhost.evil.example",
            "127.0.0.1.nip.io",
        ] {
            assert!(!host_allowed(host, &listed), "{host}");
        }
    }

    #[test]
    fn the_key_comparison_is_exact() {
        assert!(same_secret("abc", "abc"));
        assert!(!same_secret("abc", "abd"));
        assert!(!same_secret("abc", "ab"));
    }
}
