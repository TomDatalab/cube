//! The Playground's helper API (`/playground/*`).
//!
//! Only the part the query builder needs is implemented. The Node.js dev
//! server (`cubejs-server-core/src/core/DevServer.ts`) also generates a data
//! model from a database, edits `.env` and scaffolds a dashboard app through
//! npm; none of that belongs in a deployment with no Node.js, so those routes
//! answer 501 with a message naming the alternative.
//!
//! These routes are not behind the auth middleware, because the app fetches
//! `/playground/context` to *obtain* its token. That is why the server mounts
//! the Playground only when authentication is not enforced; see `main.rs`.

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::app::AppState;
use crate::error::ApiError;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/playground/context", get(context))
        .route("/playground/files", get(files))
        .route("/playground/token", post(token))
        .route("/playground/db-schema", get(db_schema))
        .route("/playground/driver", get(driver))
        // Not ported. Named individually so the message says why, rather
        // than letting the app see an unexplained 404.
        .route("/playground/generate-schema", post(not_ported))
        .route("/playground/env", post(not_ported))
        .route("/playground/test-connection", post(not_ported))
        .route("/playground/schema/pre-aggregation", post(not_ported))
}

/// `GET /playground/context` — the only call the app must have to boot.
///
/// `basePath` tells the app where the REST API lives; it builds
/// `window.location.origin + basePath + "/v1"` from it, so the value must be
/// this server's API prefix rather than Node's `/cubejs-api`.
async fn context(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    // `jwt.sign({}, apiSecret, { expiresIn: '1d' })` in `DevServer.ts:52`.
    let token = state.auth.issue_token(Map::new()).await?;

    Ok(Json(json!({
        "cubejsToken": token,
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
        "coreServerVersion": env!("CARGO_PKG_VERSION"),
        "dockerVersion": Value::Null,
        "projectFingerprint": Value::Null,
        "isDocker": false,
        "previewFeatures": false,
    })))
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

/// `GET /playground/db-schema` — the tables of the data source, for the
/// data-model page.
///
/// The driver is built the same way the orchestrator builds it, from the
/// environment, so the page describes the database queries actually run
/// against.
async fn db_schema(Query(query): Query<DataSourceQuery>) -> Result<impl IntoResponse, ApiError> {
    let data_source = query.data_source.unwrap_or_else(|| "default".to_string());

    let driver = crate::orchestrator_adapter::driver_factory()(data_source.clone())
        .await
        .map_err(|e| ApiError::bad_request(format!("{data_source}: {e}")))?;

    let structure = driver
        .tables_schema()
        .await
        .map_err(|e| ApiError::bad_request(format!("{data_source}: {e}")))?;

    Ok(Json(json!({ "tablesSchema": structure })))
}

#[derive(Debug, Deserialize)]
struct DataSourceQuery {
    #[serde(rename = "dataSource")]
    data_source: Option<String>,
}

async fn not_ported() -> ApiError {
    ApiError::not_implemented(
        "This Playground action is not available on the Rust server. \
         Generating a data model from the database, editing .env and the \
         dashboard-app scaffolding all run JavaScript, which this deployment \
         does not have. Edit the YAML model files directly; the server \
         reloads them while it runs.",
    )
}
