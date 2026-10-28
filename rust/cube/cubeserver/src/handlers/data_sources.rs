//! `/playground/data-sources` — the Playground's "Data Source" page: list,
//! test, add, edit and delete the data sources.
//!
//! Settings are written where the deployment already keeps them. A new data
//! source gets its name and type under `data_sources:` in `cube.yml` and its
//! settings in `.env` next to it, as `CUBEJS_DS_<NAME>_*`: the variables
//! `cube.yml` resolves a data source's fields from, so the password never
//! lands in `cube.yml`. Editing changes each setting where its value comes
//! from: the variable a `{ env: ... }` field names (`SOFIADB_HOST`, say), or
//! else `CUBEJS_DS_<NAME>_*`, which wins over a literal in the file.
//!
//! The same handlers serve the REST API as `/v1/data-sources`, behind a
//! token like every `/v1` route and the `data_sources` scope, which no
//! deployment grants unless `CUBEJS_DEFAULT_API_SCOPES` lists it.
//!
//! Data sources are read once, at start-up: the planner picks each one's SQL
//! dialect then, and the drivers read their settings from the environment.
//! So every change restarts the server, as the Node.js dev server does after
//! its connection wizard writes `.env`. The restart replaces the process
//! image in place (same PID, same log), with the changed variables applied to
//! its environment, so whatever supervises the process does not notice.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::extract::{Path as UrlPath, State};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use cubeconfig::data_source::key_by_data_source;
use cubeconfig::{ConfigValue, CubeConfig, CubeConfigSpec, DataSourceSpec, Env};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::error::ApiError;
use crate::handlers::auth::{assert_api_scope, ForContext};
use crate::services::RequestContext;

/// The scope `/v1/data-sources` requires.
const SCOPE: &str = "data_sources";

/// The scope `/v1/execute` requires: it runs whatever
/// statement it is given against the database, so it is kept apart from
/// managing the connection settings.
const EXECUTE_SCOPE: &str = "execute";

/// A statement running longer than this is abandoned.
const EXECUTE_TIMEOUT: Duration = Duration::from_secs(120);

/// At most this many rows of a statement's result are sent back.
const EXECUTE_MAX_ROWS: usize = 1000;

/// A connection test longer than this is reported as a failure.
const TEST_TIMEOUT: Duration = Duration::from_secs(20);

/// The `CUBEJS_DB_*` variable behind each connection field of `cube.yml`.
const FIELDS: &[(&str, &str)] = &[
    ("CUBEJS_DB_URL", "url"),
    ("CUBEJS_DB_HOST", "host"),
    ("CUBEJS_DB_PORT", "port"),
    ("CUBEJS_DB_NAME", "database"),
    ("CUBEJS_DB_SCHEMA", "schema"),
    ("CUBEJS_DB_USER", "user"),
    ("CUBEJS_DB_PASS", "password"),
    ("CUBEJS_DB_SSL", "ssl"),
];

/// One change to the configuration at a time: each one reads the files,
/// checks the result and writes it back.
static WRITE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// `GET /playground/data-sources`
pub async fn list(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let active: Vec<&str> = state
        .config
        .data_sources
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();

    let data_sources: Vec<Value> = match &state.config.config_file {
        Some(path) => load(path, &env_with_file(&path.with_file_name(".env")))?
            .data_sources
            .values()
            .map(|ds| {
                json!({
                    "name": ds.name,
                    "type": ds.db_type,
                    "host": ds.host.as_deref().or(ds.url.as_deref()),
                    "port": ds.port,
                    "database": ds.database,
                    "user": ds.user,
                    "hasPassword": ds.password.as_deref().is_some_and(|p| !p.is_empty()),
                    // In the file but not served yet: written while the
                    // server ran, and it has not restarted since.
                    "active": active.is_empty() || active.contains(&ds.name.as_str()),
                })
            })
            .collect(),
        None => state
            .config
            .data_sources
            .iter()
            .map(|(name, db_type)| json!({ "name": name, "type": db_type, "active": true }))
            .collect(),
    };

    let connectors: Vec<String> = super::connectors::connectors(&[])
        .into_iter()
        .filter(|c| c.implemented)
        .map(|c| c.db_type)
        .collect();

    Ok(Json(json!({
        "dataSources": data_sources,
        "connectors": connectors,
        // Without a `cube.yml` there is nowhere to write a change to.
        "editable": state.config.config_file.is_some(),
    })))
}

/// `GET /playground/data-sources/:name` — the settings the edit form starts
/// from. The password is never sent back, only whether there is one.
pub async fn get(
    State(state): State<AppState>,
    UrlPath(name): UrlPath<String>,
) -> Result<impl IntoResponse, ApiError> {
    let paths = paths(&state)?;
    let env = env_with_file(&paths.env);
    let config = load(&paths.config, &env)?;
    let mut variables = current_variables(&config, &name, &env)?;
    let has_password = variables
        .remove("CUBEJS_DB_PASS")
        .is_some_and(|p| !p.is_empty());
    let db_type = variables.remove("CUBEJS_DB_TYPE").unwrap_or_default();

    Ok(Json(json!({
        "name": name,
        "type": db_type,
        "variables": variables,
        "hasPassword": has_password,
    })))
}

#[derive(Debug, Deserialize)]
pub struct TestRequest {
    /// A configured data source: tested as it is, or, with `variables`, as
    /// it would be after those changes (a password left out is kept).
    #[serde(default, rename = "dataSource")]
    data_source: Option<String>,
    /// `CUBEJS_DB_*` variables, as the form sends them.
    #[serde(default)]
    variables: BTreeMap<String, String>,
}

/// `POST /playground/data-sources/test`
pub async fn test(
    State(state): State<AppState>,
    Json(request): Json<TestRequest>,
) -> Result<impl IntoResponse, ApiError> {
    match (request.data_source, request.variables.is_empty()) {
        (Some(name), true) => {
            let driver = crate::orchestrator_adapter::driver_factory()(name.clone())
                .await
                .map_err(|e| ApiError::bad_request(format!("{name}: {e}")))?;
            run_test(driver).await?;
        }
        (Some(name), false) => {
            let paths = paths(&state)?;
            let env = env_with_file(&paths.env);
            let config = load(&paths.config, &env)?;
            let current = current_variables(&config, &name, &env)?;
            let (set, cleared) = split_changes(&request.variables)?;
            test_variables(&edited(&current, &set, &cleared)).await?;
        }
        (None, _) => test_variables(&clean(&request.variables)?).await?,
    }

    Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
pub struct CreateRequest {
    pub(crate) name: String,
    variables: BTreeMap<String, String>,
}

/// `POST /playground/data-sources`
pub async fn create(
    State(state): State<AppState>,
    Json(request): Json<CreateRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let paths = paths(&state)?;
    let _guard = WRITE.lock().await;

    let name = request.name.trim().to_string();
    let variables = clean(&request.variables)?;
    validate_name(&name)?;
    let db_type = variables
        .get("CUBEJS_DB_TYPE")
        .cloned()
        .ok_or_else(|| ApiError::bad_request("Choose a database type"))?;
    if !cubedriver::DriverFactory::is_implemented(&db_type) {
        return Err(ApiError::bad_request(format!(
            "This server has no {db_type} connector"
        )));
    }

    let current = std::fs::read_to_string(&paths.config).map_err(io_error)?;
    if load(&paths.config, &env_with_file(&paths.env))?
        .data_sources
        .contains_key(&name)
    {
        return Err(ApiError::bad_request(format!(
            "A data source named {name} already exists"
        )));
    }

    // Saved only if it connects: a source that cannot is one /readyz would
    // report as down from the moment the server restarts.
    test_variables(&variables).await?;

    // The settings, as the variables `cube.yml` resolves this source from.
    let set: Vec<(String, String)> = variables
        .iter()
        .filter(|(key, _)| key.as_str() != "CUBEJS_DB_TYPE")
        .map(|(key, value)| (key_by_data_source(key, &name), value.clone()))
        .collect();

    let updated = add_to_config(&current, &name, &db_type);
    check_config(&paths.config, &updated, &set, &[])?;

    update_env(&paths.env, &set, &[], &header(&name), false)?;
    write_atomically(&paths.config, &updated)?;
    tracing::info!(data_source = %name, db_type = %db_type, "data source added from the Playground");

    Ok(Json(json!({
        "name": name,
        "restarting": restart_if_enabled(&state, set, Vec::new()),
    })))
}

#[derive(Debug, Deserialize)]
pub struct UpdateRequest {
    /// The fields the form shows. One left empty is cleared, except the
    /// password, which an empty field keeps.
    variables: BTreeMap<String, String>,
}

/// `PUT /playground/data-sources/:name`
pub async fn update(
    State(state): State<AppState>,
    UrlPath(name): UrlPath<String>,
    Json(request): Json<UpdateRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let paths = paths(&state)?;
    let _guard = WRITE.lock().await;

    let env = env_with_file(&paths.env);
    let config = load(&paths.config, &env)?;
    let current = current_variables(&config, &name, &env)?;
    let (set, cleared) = split_changes(&request.variables)?;
    if set.contains_key("CUBEJS_DB_TYPE") && set.get("CUBEJS_DB_TYPE") != current.get("CUBEJS_DB_TYPE") {
        return Err(ApiError::bad_request(
            "The database type cannot change; add a new data source instead",
        ));
    }

    test_variables(&edited(&current, &set, &cleared)).await?;

    let spec = read_spec(&paths.config)?;
    let spec = spec.data_sources.get(&name);
    let mut to_set = Vec::new();
    for (var, value) in &set {
        if var == "CUBEJS_DB_TYPE" || current.get(var) == Some(value) {
            continue;
        }
        to_set.push((target_var(spec, &name, var), value.clone()));
    }
    let to_remove: Vec<String> = cleared
        .iter()
        .filter(|var| current.contains_key(*var))
        .map(|var| target_var(spec, &name, var))
        .collect();

    if to_set.is_empty() && to_remove.is_empty() {
        return Ok(Json(json!({ "name": name, "restarting": false })));
    }

    let text = std::fs::read_to_string(&paths.config).map_err(io_error)?;
    check_config(&paths.config, &text, &to_set, &to_remove)?;
    update_env(&paths.env, &to_set, &to_remove, &header(&name), false)?;
    tracing::info!(data_source = %name, "data source edited from the Playground");

    Ok(Json(json!({
        "name": name,
        "restarting": restart_if_enabled(&state, to_set, to_remove),
    })))
}

/// `DELETE /playground/data-sources/:name`
pub async fn delete(
    State(state): State<AppState>,
    UrlPath(name): UrlPath<String>,
) -> Result<impl IntoResponse, ApiError> {
    let paths = paths(&state)?;
    let _guard = WRITE.lock().await;

    if name == "default" {
        return Err(ApiError::bad_request(
            "The default data source cannot be deleted: the server plans queries with it",
        ));
    }
    let config = load(&paths.config, &env_with_file(&paths.env))?;
    if !config.data_sources.contains_key(&name) {
        return Err(ApiError::not_found(format!("No data source named {name}")));
    }

    // A cube pointing at a missing data source fails the whole model.
    let users = cubes_using(Path::new(&state.config.schema_path), &name);
    if !users.is_empty() {
        let shown: Vec<&str> = users.iter().take(5).map(String::as_str).collect();
        let more = users.len().saturating_sub(shown.len());
        return Err(ApiError::bad_request(format!(
            "{} cube(s) still use {name} ({}{}). Delete them or point them at another data source first.",
            users.len(),
            shown.join(", "),
            if more > 0 { format!(", and {more} more") } else { String::new() },
        )));
    }

    let text = std::fs::read_to_string(&paths.config).map_err(io_error)?;
    let updated = remove_from_config(&text, &name)
        .ok_or_else(|| ApiError::bad_request(format!("Cannot find {name} in cube.yml")))?;

    // Only the variables that exist for this source alone go; one that a
    // `{ env: ... }` field names may be read by something else too.
    let prefix = key_by_data_source("CUBEJS_", &name);
    let to_remove: Vec<String> = env_keys(&paths.env)
        .into_iter()
        .filter(|key| key.starts_with(&prefix))
        .collect();

    check_config(&paths.config, &updated, &[], &to_remove)?;
    update_env(&paths.env, &[], &to_remove, &header(&name), true)?;
    write_atomically(&paths.config, &updated)?;
    tracing::info!(data_source = %name, "data source deleted from the Playground");

    Ok(Json(json!({
        "name": name,
        "restarting": restart_if_enabled(&state, Vec::new(), to_remove),
    })))
}

async fn v1<R: IntoResponse>(
    state: &AppState,
    ctx: &RequestContext,
    handler: impl std::future::Future<Output = Result<R, ApiError>>,
) -> Response {
    // With RBAC on, data sources are managed per organization
    // (`/v1/organizations/{id}/connections`); these routes see them all.
    if state.rbac.is_some() && !ctx.platform_admin {
        return ApiError::forbidden("Only platform admins manage /v1/data-sources")
            .for_context(state, ctx)
            .into_response();
    }
    v1_scoped(state, ctx, SCOPE, handler).await
}

async fn v1_scoped<R: IntoResponse>(
    state: &AppState,
    ctx: &RequestContext,
    scope: &str,
    handler: impl std::future::Future<Output = Result<R, ApiError>>,
) -> Response {
    if let Err(e) = assert_api_scope(state, ctx, scope).await {
        return e.into_response();
    }
    match handler.await {
        Ok(response) => response.into_response(),
        Err(e) => e.for_context(state, ctx).into_response(),
    }
}

/// `GET /v1/data-sources`
pub async fn v1_list(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
) -> Response {
    v1(&state, &ctx, list(State(state.clone()))).await
}

/// `GET /v1/data-sources/:name`
pub async fn v1_get(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    name: UrlPath<String>,
) -> Response {
    v1(&state, &ctx, get(State(state.clone()), name)).await
}

/// `POST /v1/data-sources/test`
pub async fn v1_test(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    request: Json<TestRequest>,
) -> Response {
    v1(&state, &ctx, test(State(state.clone()), request)).await
}

/// `POST /v1/data-sources`
pub async fn v1_create(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    request: Json<CreateRequest>,
) -> Response {
    v1(&state, &ctx, create(State(state.clone()), request)).await
}

/// `PUT /v1/data-sources/:name`
pub async fn v1_update(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    name: UrlPath<String>,
    request: Json<UpdateRequest>,
) -> Response {
    v1(&state, &ctx, update(State(state.clone()), name, request)).await
}

/// `DELETE /v1/data-sources/:name`
pub async fn v1_delete(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    name: UrlPath<String>,
) -> Response {
    v1(&state, &ctx, delete(State(state.clone()), name)).await
}

/// Where `/v1/execute` runs a statement.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Connection {
    /// A configured data source, by name: run with its settings.
    Name(String),
    /// A connection described by its `CUBEJS_DB_*` variables, for a
    /// database that is not a data source (`CUBEJS_DB_TYPE` required).
    /// Values may be strings, numbers or booleans (`"CUBEJS_DB_PORT": 8201`).
    Settings(BTreeMap<String, Value>),
}

#[derive(Debug, Deserialize)]
pub struct ExecuteRequest {
    #[serde(default)]
    connection: Option<Connection>,
    /// The database to run in, instead of the connection's own:
    /// `CUBEJS_DB_NAME` for this one statement (a file for SQLite).
    #[serde(default)]
    database: Option<String>,
    /// One statement: `CREATE TABLE`, `CREATE VIEW`, `INSERT`, `SELECT`...
    #[serde(default)]
    query: String,
}

/// `POST /v1/execute` — runs one statement on the connection the request
/// names, to create tables, views and the like.
///
/// Cube itself only ever reads; this hands the statement to the
/// connection's driver as it is, on a connection of its own.
pub async fn v1_execute(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Json(request): Json<ExecuteRequest>,
) -> Response {
    v1_scoped(&state, &ctx, EXECUTE_SCOPE, execute(&state, request)).await
}

async fn execute(state: &AppState, request: ExecuteRequest) -> Result<Json<Value>, ApiError> {
    let query = request.query.trim();
    if query.is_empty() {
        return Err(ApiError::bad_request("The query is empty"));
    }

    let (name, mut variables) = match request.connection {
        None => {
            return Err(ApiError::bad_request(
                "Name the connection: a data source name, or its CUBEJS_DB_* settings",
            ))
        }
        Some(Connection::Name(name)) => {
            let Some(config_file) = &state.config.config_file else {
                return Err(ApiError::not_implemented(
                    "This deployment has no cube.yml to read the data source from",
                ));
            };
            let env = env_with_file(&config_file.with_file_name(".env"));
            let variables = current_variables(&load(config_file, &env)?, &name, &env)?;
            (name, variables)
        }
        Some(Connection::Settings(settings)) => {
            let mut text = BTreeMap::new();
            for (key, value) in settings {
                let value = match value {
                    Value::String(s) => s,
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    Value::Null => continue,
                    _ => return Err(ApiError::bad_request(format!("{key} must be a single value"))),
                };
                text.insert(key, value);
            }
            let variables = clean(&text)?;
            match variables.get("CUBEJS_DB_TYPE") {
                Some(db_type) if cubedriver::DriverFactory::is_implemented(db_type) => {}
                Some(db_type) => {
                    return Err(ApiError::bad_request(format!(
                        "This server has no {db_type} connector"
                    )))
                }
                None => return Err(ApiError::bad_request("The connection needs CUBEJS_DB_TYPE")),
            }
            // Named in the log and the response; the settings stay out.
            ("(ad hoc)".to_string(), variables)
        }
    };
    let name = name.as_str();
    if let Some(database) = request.database.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        if database.contains(['\n', '\r', '\0']) {
            return Err(ApiError::bad_request("The database name cannot span several lines"));
        }
        variables.insert("CUBEJS_DB_NAME".to_string(), database.to_string());
    }

    let env: std::collections::HashMap<String, String> = variables.into_iter().collect();
    let config = cubedriver::DriverConfig::from_env_source(&env, None, false)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let db_type = config.data_source.db_type.clone().unwrap_or_default();
    let database = config.data_source.database.clone();
    let driver = cubedriver::DriverFactory::create(&db_type, config)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;

    tracing::info!(
        data_source = %name,
        database = database.as_deref().unwrap_or(""),
        query = %query.chars().take(200).collect::<String>(),
        "executing a statement through /v1/execute"
    );
    let started = std::time::Instant::now();
    let result = tokio::time::timeout(
        EXECUTE_TIMEOUT,
        driver.query(query, &[], &cubedriver::QueryOptions::default()),
    )
    .await;
    let _ = driver.release().await;
    let result = match result {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => return Err(ApiError::bad_request(e.to_string())),
        Err(_) => {
            return Err(ApiError::bad_request(format!(
                "The statement did not finish in {}s",
                EXECUTE_TIMEOUT.as_secs()
            )))
        }
    };

    let row_count = result.rows.len();
    let columns: Vec<&str> = result.columns.iter().map(|c| c.name.as_str()).collect();
    let rows: Vec<_> = result.rows.iter().take(EXECUTE_MAX_ROWS).collect();
    Ok(Json(json!({
        "dataSource": name,
        "database": database,
        "columns": columns,
        "rows": rows,
        "rowCount": row_count,
        "truncated": row_count > EXECUTE_MAX_ROWS,
        "ms": started.elapsed().as_millis() as u64,
    })))
}

/// Each data source's type, by name: as `cube.yml` has them now, or as the
/// server started with when there is no file or it cannot be read.
pub(crate) fn data_source_types(state: &AppState) -> BTreeMap<String, String> {
    let from_file = state.config.config_file.as_ref().and_then(|path| {
        load(path, &env_with_file(&path.with_file_name(".env")))
            .ok()
            .map(|config| {
                config
                    .data_sources
                    .values()
                    .map(|ds| (ds.name.clone(), ds.db_type.clone()))
                    .collect()
            })
    });
    from_file.unwrap_or_else(|| state.config.data_sources.iter().cloned().collect())
}

struct Paths {
    config: PathBuf,
    env: PathBuf,
}

fn paths(state: &AppState) -> Result<Paths, ApiError> {
    let config = state.config.config_file.clone().ok_or_else(|| {
        ApiError::not_implemented(
            "This deployment has no cube.yml, so its data sources cannot be \
             changed from here; set their CUBEJS_DB_* variables and restart.",
        )
    })?;
    let env = config.with_file_name(".env");
    Ok(Paths { config, env })
}

fn load(path: &Path, env: &Env) -> Result<CubeConfig, ApiError> {
    CubeConfig::load_file_with_env(path, env).map_err(|e| ApiError::bad_request(e.to_string()))
}

fn read_spec(path: &Path) -> Result<CubeConfigSpec, ApiError> {
    let text = std::fs::read_to_string(path).map_err(io_error)?;
    serde_yaml::from_str(&text).map_err(|e| ApiError::bad_request(e.to_string()))
}

/// A data source's settings as the `CUBEJS_DB_*` variables a driver reads:
/// the fields `cube.yml` resolves, plus the driver-specific variables set for
/// it alone (`CUBEJS_DS_<NAME>_DB_SNOWFLAKE_ACCOUNT`, say).
fn current_variables(
    config: &CubeConfig,
    name: &str,
    env: &Env,
) -> Result<BTreeMap<String, String>, ApiError> {
    let ds = config
        .data_sources
        .get(name)
        .ok_or_else(|| ApiError::not_found(format!("No data source named {name}")))?;

    let mut out = BTreeMap::new();
    let mut put = |var: &str, value: Option<String>| {
        if let Some(value) = value.filter(|v| !v.is_empty()) {
            out.insert(var.to_string(), value);
        }
    };
    put("CUBEJS_DB_TYPE", Some(ds.db_type.clone()));
    put("CUBEJS_DB_URL", ds.url.clone());
    put("CUBEJS_DB_HOST", ds.host.clone());
    put("CUBEJS_DB_PORT", ds.port.map(|p| p.to_string()));
    put("CUBEJS_DB_NAME", ds.database.clone());
    put("CUBEJS_DB_SCHEMA", ds.schema.clone());
    put("CUBEJS_DB_USER", ds.user.clone());
    put("CUBEJS_DB_PASS", ds.password.clone());
    put("CUBEJS_DB_SSL", ds.ssl.then(|| "true".to_string()));

    if name != "default" {
        let prefix = key_by_data_source("CUBEJS_", name);
        for key in env.names() {
            let value = env.get(key).unwrap_or_default();
            if let Some(rest) = key.strip_prefix(&prefix) {
                let var = format!("CUBEJS_{rest}");
                if !out.contains_key(&var) && !value.is_empty() {
                    out.insert(var, value.to_string());
                }
            }
        }
    }
    Ok(out)
}

/// The form's fields, split into the ones to set and the ones emptied. An
/// empty password means "keep it", not "clear it".
fn split_changes(
    variables: &BTreeMap<String, String>,
) -> Result<(BTreeMap<String, String>, BTreeSet<String>), ApiError> {
    let set = clean(variables)?;
    let cleared = variables
        .iter()
        .filter(|(key, value)| value.trim().is_empty() && key.as_str() != "CUBEJS_DB_PASS")
        .map(|(key, _)| key.clone())
        .collect();
    Ok((set, cleared))
}

fn edited(
    current: &BTreeMap<String, String>,
    set: &BTreeMap<String, String>,
    cleared: &BTreeSet<String>,
) -> BTreeMap<String, String> {
    let mut out = current.clone();
    for key in cleared {
        out.remove(key);
    }
    out.extend(set.iter().map(|(k, v)| (k.clone(), v.clone())));
    out
}

/// The variable a setting of `name` is written to: the one its `cube.yml`
/// field references, so the value stays where it is read from, or else the
/// per-data-source variable, which wins over a literal in the file.
fn target_var(spec: Option<&DataSourceSpec>, name: &str, var: &str) -> String {
    let referenced = FIELDS
        .iter()
        .find(|(v, _)| *v == var)
        .and_then(|(_, field)| spec.and_then(|s| spec_field(s, field)))
        .and_then(|value| match value {
            ConfigValue::EnvRef { name, .. } => Some(name.clone()),
            ConfigValue::Literal(_) => None,
        });
    referenced.unwrap_or_else(|| key_by_data_source(var, name))
}

fn spec_field<'a>(spec: &'a DataSourceSpec, field: &str) -> Option<&'a ConfigValue> {
    match field {
        "url" => spec.url.as_ref(),
        "host" => spec.host.as_ref(),
        "port" => spec.port.as_ref(),
        "database" => spec.database.as_ref(),
        "schema" => spec.schema.as_ref(),
        "user" => spec.user.as_ref(),
        "password" => spec.password.as_ref(),
        "ssl" => spec.ssl.as_ref(),
        _ => None,
    }
}

/// Checks `content` loads as the restarted server will read it, before
/// anything is written: a file it cannot load would keep it from starting.
fn check_config(
    config_file: &Path,
    content: &str,
    set: &[(String, String)],
    remove: &[String],
) -> Result<(), ApiError> {
    let mut vars: BTreeMap<String, String> = env_file_vars(&config_file.with_file_name(".env"));
    for (key, value) in std::env::vars() {
        vars.entry(key).or_insert(value);
    }
    for key in remove {
        vars.remove(key);
    }
    vars.extend(set.iter().cloned());
    let env: Env = vars.into_iter().collect();

    let check = config_file.with_extension("yml.check");
    std::fs::write(&check, content).map_err(io_error)?;
    let loaded = CubeConfig::load_file_with_env(&check, &env);
    let _ = std::fs::remove_file(&check);
    loaded
        .map(|_| ())
        .map_err(|e| ApiError::bad_request(format!("The new configuration would not load: {e}")))
}

async fn test_variables(variables: &BTreeMap<String, String>) -> Result<(), ApiError> {
    let env: std::collections::HashMap<String, String> = variables.clone().into_iter().collect();
    let config = cubedriver::DriverConfig::from_env_source(&env, None, false)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let db_type = config
        .data_source
        .db_type
        .clone()
        .ok_or_else(|| ApiError::bad_request("Choose a database type"))?;
    let driver = cubedriver::DriverFactory::create(&db_type, config)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    run_test(driver).await
}

async fn run_test(driver: std::sync::Arc<dyn cubedriver::Driver>) -> Result<(), ApiError> {
    let result = tokio::time::timeout(TEST_TIMEOUT, driver.test_connection()).await;
    let _ = driver.release().await;
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(ApiError::bad_request(format!("Cannot connect: {e}"))),
        Err(_) => Err(ApiError::bad_request(format!(
            "Cannot connect: no answer in {}s",
            TEST_TIMEOUT.as_secs()
        ))),
    }
}

/// Drops empty fields and refuses anything that is not a `CUBEJS_*` variable
/// or that would break the `.env` line it is written to.
fn clean(variables: &BTreeMap<String, String>) -> Result<BTreeMap<String, String>, ApiError> {
    let mut out = BTreeMap::new();
    for (key, value) in variables {
        let valid_key = key.starts_with("CUBEJS_")
            && !key.starts_with("CUBEJS_DS_")
            && key
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        if !valid_key {
            return Err(ApiError::bad_request(format!("Unexpected setting {key}")));
        }
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        if value.contains(['\n', '\r', '\0']) {
            return Err(ApiError::bad_request(format!(
                "{key} cannot span several lines"
            )));
        }
        out.insert(key.clone(), value.to_string());
    }
    Ok(out)
}

/// Lower case, digits and `_`, starting with a letter: it becomes part of
/// `CUBEJS_DS_<NAME>_*` and of `data_source:` in the model.
pub(crate) fn validate_name(name: &str) -> Result<(), ApiError> {
    let valid = name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && name.len() <= 63;
    if valid {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "The name must start with a letter and hold only lower-case letters, digits and _",
        ))
    }
}

/// The model files with a cube whose `data_source` is `name`.
fn cubes_using(model_dir: &Path, name: &str) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, name: &str, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, name, out);
                continue;
            }
            if !matches!(path.extension().and_then(|e| e.to_str()), Some("yml") | Some("yaml")) {
                continue;
            }
            let Some(doc) = std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| serde_yaml::from_str::<serde_yaml::Value>(&t).ok())
            else {
                continue;
            };
            let cubes = doc.get("cubes").and_then(|c| c.as_sequence());
            for cube in cubes.into_iter().flatten() {
                if cube.get("data_source").and_then(|d| d.as_str()) == Some(name) {
                    let cube_name = cube.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                    let file = path.strip_prefix(root).unwrap_or(&path).display();
                    out.push(format!("{cube_name} in {file}"));
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(model_dir, model_dir, name, &mut out);
    out.sort();
    out
}

/// `cube.yml` with the new data source added at the end of `data_sources:`.
///
/// Edited as text rather than re-serialised, so the comments and layout the
/// file already has survive.
fn add_to_config(current: &str, name: &str, db_type: &str) -> String {
    let block = format!(
        "\n  # Added from the Playground; its settings are CUBEJS_DS_{}_* in .env.\n  {name}:\n    type: {db_type}\n",
        name.to_uppercase()
    );

    let lines: Vec<&str> = current.lines().collect();
    let Some(start) = lines.iter().position(|l| l.trim_end() == "data_sources:") else {
        let mut out = current.trim_end().to_string();
        out.push_str("\n\ndata_sources:");
        out.push_str(&block);
        return out;
    };

    let mut end = section_end(&lines, start);
    // Blank lines and comments just before the next key belong to it.
    while end > start + 1 {
        let previous = lines[end - 1];
        if previous.trim().is_empty() || previous.starts_with('#') {
            end -= 1;
        } else {
            break;
        }
    }

    let mut out = lines[..end].join("\n");
    out.push('\n');
    out.push_str(&block);
    if end < lines.len() {
        if !lines[end].trim().is_empty() {
            out.push('\n');
        }
        out.push_str(&lines[end..].join("\n"));
        out.push('\n');
    }
    out
}

/// `cube.yml` without data source `name`: its entry, the comment lines right
/// above it and one blank line before those. `None` if it is not there.
fn remove_from_config(current: &str, name: &str) -> Option<String> {
    let lines: Vec<&str> = current.lines().collect();
    let start = lines.iter().position(|l| l.trim_end() == "data_sources:")?;
    let end = section_end(&lines, start);

    let indent = |l: &str| l.len() - l.trim_start().len();
    let entry = (start + 1..end).find(|&i| {
        let line = lines[i];
        line.trim_start().trim_end() == format!("{name}:") && indent(line) > 0
    })?;
    let level = indent(lines[entry]);

    let mut stop = entry + 1;
    while stop < end {
        let line = lines[stop];
        if !line.trim().is_empty() && indent(line) <= level {
            break;
        }
        stop += 1;
    }
    // The blank lines that ended the entry stay, to separate what follows.
    while stop > entry + 1 && lines[stop - 1].trim().is_empty() {
        stop -= 1;
    }

    let mut first = entry;
    while first > start + 1 {
        let line = lines[first - 1];
        if line.trim_start().starts_with('#') && indent(line) == level {
            first -= 1;
        } else {
            break;
        }
    }
    if first > start + 1 && lines[first - 1].trim().is_empty() {
        first -= 1;
    }

    let mut out: Vec<&str> = lines[..first].to_vec();
    out.extend_from_slice(&lines[stop..]);
    Some(out.join("\n") + "\n")
}

/// The index of the first line after the section starting at `start`: the
/// next top-level key.
fn section_end(lines: &[&str], start: usize) -> usize {
    lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, line)| !line.is_empty() && !line.starts_with([' ', '\t', '#']))
        .map(|(i, _)| i)
        .unwrap_or(lines.len())
}

fn header(name: &str) -> String {
    format!("# Data source {name}, added from the Playground")
}

/// The process environment with `.env` laid over it: what the server will
/// read once it restarts, and so what the page describes.
fn env_with_file(path: &Path) -> Env {
    let mut vars: BTreeMap<String, String> = std::env::vars().collect();
    vars.extend(env_file_vars(path));
    vars.into_iter().collect()
}

/// The variables `.env` sets, unquoted the way the shell would for the
/// forms written here (`'...'`, `"..."` or bare).
fn env_file_vars(path: &Path) -> BTreeMap<String, String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let Some(key) = env_line_key(line) else {
            continue;
        };
        let raw = line.split_once('=').map(|(_, v)| v.trim()).unwrap_or_default();
        let value = if let Some(inner) = raw.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')) {
            inner.replace(r"'\''", "'")
        } else if let Some(inner) = raw.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
            inner.to_string()
        } else {
            raw.to_string()
        };
        out.insert(key.to_string(), value);
    }
    out
}

/// The variable a `.env` line sets, if it sets one.
fn env_line_key(line: &str) -> Option<&str> {
    let line = line.trim_start();
    if line.starts_with('#') {
        return None;
    }
    let line = line.strip_prefix("export ").unwrap_or(line);
    let (key, _) = line.split_once('=')?;
    let key = key.trim();
    (!key.is_empty()).then_some(key)
}

fn env_keys(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(env_line_key)
        .map(str::to_string)
        .collect()
}

/// Rewrites `.env`, which `start-all.sh` sources with `set -a`: each value
/// single-quoted so the shell takes it literally. A variable already there
/// is changed in place; a new one is appended under `header`.
fn update_env(
    path: &Path,
    set: &[(String, String)],
    remove: &[String],
    header: &str,
    drop_header: bool,
) -> Result<(), ApiError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(io_error(e)),
    };
    let line_for = |key: &str, value: &str| format!("{key}='{}'", value.replace('\'', r"'\''"));

    let mut done = BTreeSet::new();
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        if drop_header && line.trim() == header {
            continue;
        }
        match env_line_key(line) {
            Some(key) if remove.iter().any(|r| r == key) => continue,
            Some(key) => match set.iter().find(|(k, _)| k == key) {
                Some((k, v)) if !done.contains(k) => {
                    done.insert(k.clone());
                    lines.push(line_for(k, v));
                }
                // A second line for the same variable would win when sourced.
                Some(_) => continue,
                None => lines.push(line.to_string()),
            },
            None => lines.push(line.to_string()),
        }
    }
    let new: Vec<&(String, String)> = set.iter().filter(|(k, _)| !done.contains(k)).collect();
    if !new.is_empty() {
        if lines.last().is_some_and(|l| !l.trim().is_empty()) {
            lines.push(String::new());
        }
        if !lines.iter().any(|l| l.trim() == header) {
            lines.push(header.to_string());
        }
        lines.extend(new.iter().map(|(k, v)| line_for(k, v)));
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }

    use std::os::unix::fs::PermissionsExt;
    // It holds passwords: readable by the owner alone unless it already
    // had other permissions.
    let mode = std::fs::metadata(path)
        .map(|m| m.permissions().mode())
        .unwrap_or(0o600);
    let tmp = path.with_file_name(".env.tmp");
    std::fs::write(&tmp, lines.join("\n") + "\n").map_err(io_error)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode)).map_err(io_error)?;
    std::fs::rename(&tmp, path).map_err(io_error)
}

fn write_atomically(path: &Path, content: &str) -> Result<(), ApiError> {
    let tmp: PathBuf = path.with_extension("yml.tmp");
    std::fs::write(&tmp, content).map_err(io_error)?;
    std::fs::rename(&tmp, path).map_err(io_error)
}

fn restart_if_enabled(state: &AppState, set: Vec<(String, String)>, remove: Vec<String>) -> bool {
    if state.config.restart_on_config_change {
        restart_soon(set, remove);
    }
    state.config.restart_on_config_change
}

/// Replaces this process with a fresh copy of itself once the response has
/// gone out, with `set` added to its environment and `remove` taken out.
fn restart_soon(set: Vec<(String, String)>, remove: Vec<String>) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        tracing::info!("restarting to load the changed data sources");

        use std::os::unix::process::CommandExt;
        let mut args = std::env::args_os();
        // argv[0] rather than /proc/self/exe: a rebuilt binary leaves the
        // running one's path ending in " (deleted)".
        let program = args
            .next()
            .filter(|p| Path::new(p).components().count() > 1)
            .map(PathBuf::from)
            .or_else(|| std::env::current_exe().ok())
            .unwrap_or_else(|| PathBuf::from("cube-server"));
        let mut command = std::process::Command::new(&program);
        command.args(args).envs(set);
        for key in remove {
            command.env_remove(key);
        }
        let error = command.exec();
        tracing::error!(error = %error, program = %program.display(), "cannot restart; restart the server by hand");
    });
}

fn io_error(e: std::io::Error) -> ApiError {
    tracing::warn!(error = %e, "cannot write the configuration");
    ApiError::new(
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        format!("Cannot write the configuration: {e}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "version: 1\n\ndata_sources:\n  default:\n    type: postgres\n\n# Refresh\nscheduled_refresh:\n  enabled: false\n";

    #[test]
    fn the_source_goes_at_the_end_of_data_sources() {
        let out = add_to_config(CONFIG, "sales", "mysql");
        assert_eq!(
            out,
            "version: 1\n\ndata_sources:\n  default:\n    type: postgres\n\n  # Added from the Playground; its settings are CUBEJS_DS_SALES_* in .env.\n  sales:\n    type: mysql\n\n# Refresh\nscheduled_refresh:\n  enabled: false\n"
        );
    }

    #[test]
    fn a_file_without_data_sources_gets_the_section() {
        let out = add_to_config("version: 1\n", "sales", "mysql");
        assert!(out.ends_with("data_sources:\n  # Added from the Playground; its settings are CUBEJS_DS_SALES_* in .env.\n  sales:\n    type: mysql\n"), "{out}");
    }

    #[test]
    fn removing_a_source_undoes_adding_it() {
        let added = add_to_config(CONFIG, "sales", "mysql");
        assert_eq!(remove_from_config(&added, "sales").unwrap(), CONFIG);
    }

    #[test]
    fn removing_a_source_in_the_middle_keeps_its_neighbours() {
        let config = "data_sources:\n  default:\n    type: postgres\n\n  # SofiaDB\n  sofiadb:\n    type: clickhouse\n    host: { env: SOFIADB_HOST }\n\n  other:\n    type: mysql\n";
        assert_eq!(
            remove_from_config(config, "sofiadb").unwrap(),
            "data_sources:\n  default:\n    type: postgres\n\n  other:\n    type: mysql\n"
        );
        assert_eq!(remove_from_config(config, "missing"), None);
    }

    #[test]
    fn a_setting_is_written_where_cube_yml_reads_it() {
        let spec: DataSourceSpec =
            serde_yaml::from_str("type: clickhouse\nhost: { env: SOFIADB_HOST }\nport: \"8201\"\n").unwrap();
        assert_eq!(target_var(Some(&spec), "sofiadb", "CUBEJS_DB_HOST"), "SOFIADB_HOST");
        // A literal in the file: the per-source variable wins over it.
        assert_eq!(target_var(Some(&spec), "sofiadb", "CUBEJS_DB_PORT"), "CUBEJS_DS_SOFIADB_DB_PORT");
        assert_eq!(target_var(Some(&spec), "default", "CUBEJS_DB_USER"), "CUBEJS_DB_USER");
    }

    #[test]
    fn env_lines_are_changed_in_place_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        std::fs::write(&path, "PORT=4000\nSOFIADB_HOST=127.0.0.1\n\n# Data source x, added from the Playground\nCUBEJS_DS_X_DB_HOST='a'\n").unwrap();

        update_env(&path, &[("SOFIADB_HOST".into(), "10.0.0.1".into())], &[], "# h", false).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "PORT=4000\nSOFIADB_HOST='10.0.0.1'\n\n# Data source x, added from the Playground\nCUBEJS_DS_X_DB_HOST='a'\n"
        );

        update_env(&path, &[], &["CUBEJS_DS_X_DB_HOST".into()], &header("x"), true).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "PORT=4000\nSOFIADB_HOST='10.0.0.1'\n");
    }

    #[test]
    fn names_must_fit_a_variable() {
        assert!(validate_name("sales_2").is_ok());
        assert!(validate_name("Sales").is_err());
        assert!(validate_name("2sales").is_err());
        assert!(validate_name("sales-eu").is_err());
    }

    #[test]
    fn only_single_line_cubejs_settings_pass() {
        let ok = BTreeMap::from([
            ("CUBEJS_DB_HOST".to_string(), " db ".to_string()),
            ("CUBEJS_DB_PORT".to_string(), "".to_string()),
        ]);
        assert_eq!(
            clean(&ok).unwrap(),
            BTreeMap::from([("CUBEJS_DB_HOST".to_string(), "db".to_string())])
        );
        let path = BTreeMap::from([("PATH".to_string(), "/tmp".to_string())]);
        assert!(clean(&path).is_err());
        let multi = BTreeMap::from([("CUBEJS_DB_PASS".to_string(), "a\nb".to_string())]);
        assert!(clean(&multi).is_err());
    }
}
