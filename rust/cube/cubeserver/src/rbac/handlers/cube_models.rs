//! Models: `/v1/organizations/{id}/cube-models` and `/v1/cube-models/{id}`.
//!
//! A model is a canvas of tables of one connection, linked by their columns,
//! and of cubes on them, that Build cube turns into one cube file per cube
//! of the organization (see `rbac::cube_model`); `cube_model_cubes` records
//! which file each cube's last build wrote. The organization's admins manage
//! its models, as they do its model files; a model of another organization
//! answers 404. Deleting a model keeps its cube files.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::model::{detail_of, org_scope, redacted, writer, Scope};
use super::{body, caller, store_error};
use crate::app::AppState;
use crate::error::ApiError;
use crate::handlers::scaffold::snake;
use crate::rbac::cube_model::{self, Definition};
use crate::rbac::model_snapshot::source_path;
use crate::rbac::model_store::{ModelChange, ModelFile};
use crate::rbac::model_writer::WriteError;
use crate::rbac::store::{BuiltRecord, CubeModel, ModelLock, Store, StoreError};
use crate::services::RequestContext;
use std::collections::HashMap;

const DUPLICATE: &str = "Já existe um model com este nome nesta organização";
const STALE: &str = "O model mudou desde que você abriu; abra de novo";
const EDITED: &str = "O cubo foi editado desde o último build";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewModel {
    name: String,
    definition: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelUpdate {
    name: Option<String>,
    definition: Option<Value>,
    version: i32,
}

#[derive(Deserialize)]
pub struct DeleteParams {
    version: Option<i32>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct BuildBody {
    #[serde(default)]
    force: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CreateTablesBody {
    version: i32,
    #[serde(default)]
    dry_run: bool,
}

const STATEMENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

fn not_found() -> ApiError {
    ApiError::not_found("Model não encontrado")
}

/// A store failure of a model write: a stale version and a name taken are
/// both `Conflict`, told apart by the store's message.
fn model_error(e: StoreError) -> ApiError {
    match e {
        StoreError::NotFound => not_found(),
        StoreError::Conflict(m) if m == "stale model version" => {
            ApiError::new(StatusCode::CONFLICT, STALE)
        }
        e => store_error(e, DUPLICATE),
    }
}

/// The name goes into a comment line of the cube file: trimmed, not empty,
/// no control characters (a newline would end the comment), and neither
/// U+2028 nor U+2029, which libyaml also takes as line breaks.
fn model_name(name: &str) -> Result<String, ApiError> {
    let name = name.trim();
    if name.is_empty() || name.chars().any(|c| c.is_control() || c == '\u{2028}' || c == '\u{2029}') {
        return Err(ApiError::bad_request("Nome inválido"));
    }
    Ok(name.to_string())
}

/// The type of the configured data source `ds`, or "" when there is none.
fn db_type_of<'a>(state: &'a AppState, ds: &str) -> &'a str {
    state
        .config
        .data_sources
        .iter()
        .find(|(n, _)| n == ds)
        .map(|(_, t)| t.as_str())
        .unwrap_or("")
}

/// `value` as a [`Definition`] whose shape [`cube_model::validate`] accepts,
/// its draft types those of its connection's dialect, as stored.
fn definition(state: &AppState, value: Value) -> Result<Value, ApiError> {
    let def: Definition = serde_json::from_value(value)
        .map_err(|e| ApiError::bad_request(format!("Definição inválida: {e}")))?;
    cube_model::validate(&def).map_err(ApiError::bad_request)?;
    cube_model::validate_types(&def, db_type_of(state, &def.data_source))
        .map_err(ApiError::bad_request)?;
    serde_json::to_value(&def).map_err(|e| ApiError::bad_request(e.to_string()))
}

/// The name of the cube a definition without cubes converts to: the
/// model's `cube_name`, else the name cube `c1` was last built under, else
/// the model's name in snake case.
fn conversion_name(model: &CubeModel, records: &[BuiltRecord]) -> String {
    model
        .cube_name
        .clone()
        .or_else(|| {
            records
                .iter()
                .find(|r| r.cube_id == "c1")
                .map(|r| r.cube_name.clone())
        })
        .unwrap_or_else(|| snake(&model.name))
}

/// The model's definition with [`cube_model::upgrade`] applied; as stored
/// when it does not read as a [`Definition`].
fn upgraded(model: &CubeModel, records: &[BuiltRecord]) -> Value {
    let Ok(mut def) = serde_json::from_value::<Definition>(model.definition.clone()) else {
        return model.definition.clone();
    };
    cube_model::upgrade(&mut def, &conversion_name(model, records));
    serde_json::to_value(&def).unwrap_or_else(|_| model.definition.clone())
}

/// The build records of the organization's models, by model.
pub(crate) async fn records_of(
    store: &Store,
    org: uuid::Uuid,
) -> Result<HashMap<uuid::Uuid, Vec<BuiltRecord>>, ApiError> {
    let mut out: HashMap<uuid::Uuid, Vec<BuiltRecord>> = HashMap::new();
    for (model, record) in store
        .cube_model_cubes_of(org)
        .await
        .map_err(|e| store_error(e, "Conflict"))?
    {
        out.entry(model).or_default().push(record);
    }
    Ok(out)
}

/// The cube names of a model's last build, by name.
pub(crate) fn cube_names(records: Option<&Vec<BuiltRecord>>) -> Vec<String> {
    let mut names: Vec<String> = records
        .map(|rs| rs.iter().map(|r| r.cube_name.clone()).collect())
        .unwrap_or_default();
    names.sort();
    names
}

fn view(model: &CubeModel, records: &[BuiltRecord]) -> Value {
    let mut names: Vec<&str> = records.iter().map(|r| r.cube_name.as_str()).collect();
    names.sort();
    json!({
        "id": model.id,
        "organizationId": model.organization_id,
        "name": model.name,
        "definition": upgraded(model, records),
        "cubeNames": names,
        "version": model.version,
        "updatedAt": model.updated_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "updatedBy": model.updated_by,
    })
}

/// [`view`] of `model` with its build records read from the store.
async fn view_of(store: &Store, model: &CubeModel) -> Result<Value, ApiError> {
    let records = store
        .cube_model_cubes(model.id)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    Ok(view(model, &records))
}

/// The model in the path, its organization's scope and the caller's
/// username, after `admin` is checked on the organization: 404 when the
/// caller cannot view it, as when the model does not exist.
async fn visible_model(
    state: &AppState,
    ctx: &RequestContext,
    id: &str,
) -> Result<(CubeModel, Scope, String), ApiError> {
    let (rbac, _) = caller(state, ctx)?;
    let id = id.parse().map_err(|_| not_found())?;
    let model = rbac
        .store
        .cube_model(id)
        .await
        .map_err(|e| store_error(e, "Conflict"))?
        .ok_or_else(not_found)?;
    let (scope, actor) = org_scope(state, ctx, &model.organization_id.to_string())
        .await
        .map_err(|e| {
            if e.status == StatusCode::NOT_FOUND {
                not_found()
            } else {
                e
            }
        })?;
    Ok((model, scope, actor))
}

/// Seconds without a heartbeat after which an edit lock is free.
pub const LOCK_TTL_SECS: i64 = 120;

/// The header that carries the caller's lock token.
const LOCK_HEADER: &str = "X-Model-Lock";

/// 423 when a live lock of the model is not the one in `X-Model-Lock`
/// (or the header is missing); free when nobody holds the model.
async fn check_lock(store: &Store, model_id: uuid::Uuid, headers: &HeaderMap) -> Result<(), ApiError> {
    let live = store
        .live_cube_model_lock(model_id, LOCK_TTL_SECS)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    let Some(live) = live else { return Ok(()) };
    let token = headers.get(LOCK_HEADER).and_then(|v| v.to_str().ok());
    if token == Some(live.token.as_str()) {
        Ok(())
    } else {
        Err(ApiError::locked(format!(
            "O model está sendo editado por {}",
            live.user_name
        )))
    }
}

#[derive(Deserialize)]
pub struct LockBody {
    token: String,
}

#[derive(Deserialize)]
pub struct ReleaseParams {
    token: Option<String>,
}

fn lock_view(lock: Option<ModelLock>, token: &str) -> Json<Value> {
    Json(json!({
        "held": lock.as_ref().is_some_and(|l| l.token == token),
        "holder": lock.map(|l| l.user_name),
    }))
}

async fn claim(
    state: AppState,
    ctx: RequestContext,
    id: String,
    request: Result<Json<LockBody>, JsonRejection>,
    take: bool,
) -> Result<Json<Value>, ApiError> {
    let (model, _, actor) = visible_model(&state, &ctx, &id).await?;
    let request = body(request)?;
    let lock = super::rbac_of(&state)?
        .store
        .claim_cube_model_lock(model.id, &actor, &request.token, take, LOCK_TTL_SECS)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    Ok(lock_view(lock, &request.token))
}

/// `{token}` → `{held, holder}`: claims the model's edit lock, or renews it
/// when `token` holds it. `admin` on the organization.
pub async fn lock(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
    request: Result<Json<LockBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    claim(state, ctx, id, request, false).await
}

/// `{token}` → `{held, holder}`: takes the lock over from its holder.
pub async fn take_lock(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
    request: Result<Json<LockBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    claim(state, ctx, id, request, true).await
}

/// `?token=` → 204: frees the lock when `token` holds it.
pub async fn release_lock(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
    Query(params): Query<ReleaseParams>,
) -> Result<StatusCode, ApiError> {
    let (model, _, _) = visible_model(&state, &ctx, &id).await?;
    let token = params
        .token
        .ok_or_else(|| ApiError::bad_request("Informe o token (?token=)"))?;
    super::rbac_of(&state)?
        .store
        .release_cube_model_lock(model.id, &token)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    Ok(StatusCode::NO_CONTENT)
}

/// → `{models: [...]}`, by name: `admin` on the organization.
pub async fn list(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(org): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (scope, _) = org_scope(&state, &ctx, &org).await?;
    let rbac = super::rbac_of(&state)?;
    let org = scope.organization.ok_or_else(not_found)?;
    let records = records_of(&rbac.store, org).await?;
    let models: Vec<Value> = rbac
        .store
        .cube_models_of(org)
        .await
        .map_err(|e| store_error(e, "Conflict"))?
        .iter()
        .map(|m| view(m, records.get(&m.id).map_or(&[][..], Vec::as_slice)))
        .collect();
    Ok(Json(json!({ "models": models })))
}

/// `{name, definition}` → 201: `admin` on the organization.
pub async fn create(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(org): Path<String>,
    request: Result<Json<NewModel>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let (scope, actor) = org_scope(&state, &ctx, &org).await?;
    let rbac = super::rbac_of(&state)?;
    let org = scope.organization.ok_or_else(not_found)?;
    let request = body(request)?;
    let name = model_name(&request.name)?;
    let definition = definition(&state, request.definition)?;
    let model = rbac
        .store
        .create_cube_model(org, &name, &definition, &actor)
        .await
        .map_err(model_error)?;
    Ok((StatusCode::CREATED, Json(view(&model, &[]))))
}

/// `admin` on the organization.
pub async fn get(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (model, _, _) = visible_model(&state, &ctx, &id).await?;
    let mut body = view_of(&super::rbac_of(&state)?.store, &model).await?;
    // The dialect each connection designs tables in, for the editor's types.
    let dialects: serde_json::Map<String, Value> = state
        .config
        .data_sources
        .iter()
        .map(|(name, db_type)| (name.clone(), json!(cube_model::dialect(db_type))))
        .collect();
    body["dialects"] = Value::Object(dialects);
    Ok(Json(body))
}

/// `{name?, definition?, version}`: `admin` on the organization; 409 when
/// the version is not the model's.
pub async fn update(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
    request: Result<Json<ModelUpdate>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (model, _, actor) = visible_model(&state, &ctx, &id).await?;
    check_lock(&super::rbac_of(&state)?.store, model.id, &headers).await?;
    let rbac = super::rbac_of(&state)?;
    let request = body(request)?;
    let name = request.name.as_deref().map(model_name).transpose()?;
    let definition = request
        .definition
        .map(|value| definition(&state, value))
        .transpose()?;
    let updated = rbac
        .store
        .update_cube_model(
            model.id,
            name.as_deref(),
            definition.as_ref(),
            request.version,
            &actor,
        )
        .await
        .map_err(model_error)?;
    Ok(Json(view_of(&rbac.store, &updated).await?))
}

/// `?version=` → 204: `admin` on the organization. The cube file stays.
pub async fn delete(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(params): Query<DeleteParams>,
) -> Result<StatusCode, ApiError> {
    let (model, _, _) = visible_model(&state, &ctx, &id).await?;
    check_lock(&super::rbac_of(&state)?.store, model.id, &headers).await?;
    let version = params
        .version
        .ok_or_else(|| ApiError::bad_request("Informe a versão do model (?version=)"))?;
    super::rbac_of(&state)?
        .store
        .delete_cube_model(model.id, version)
        .await
        .map_err(model_error)?;
    Ok(StatusCode::NO_CONTENT)
}

fn sha256_hex(content: &str) -> String {
    Sha256::digest(content.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Whether `content` starts with the header [`cube_model::build_all`] writes
/// for model `id` (the name in it may have changed since).
fn built_from(content: &str, id: uuid::Uuid) -> bool {
    content.lines().next().is_some_and(|line| {
        line.starts_with("# Built from model ")
            && line.ends_with(&format!(" ({id}): Build cube rewrites this file"))
    })
}

/// `{force?}` → `{cubes: [{cubeName, fileName, content}], removed, detail}`:
/// `admin` on the organization. Reads the columns of the model's connection,
/// builds every cube and writes each over the file of its last build, or at
/// `<cubeName>.yml` when there is none (a file there that carries this
/// model's header is taken as the last build's, its record lost). The file
/// of a cube no longer in the model is deleted; `removed` names those files.
/// 409 when a file was edited since the build that wrote it (unless
/// `force`), when another file of the organization declares a cube of the
/// same name, or when a new cube's path is another file's. `detail` is the
/// organization's new detail.
pub async fn build(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
    request: Result<Json<BuildBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (model, scope, actor) = visible_model(&state, &ctx, &id).await?;
    check_lock(&super::rbac_of(&state)?.store, model.id, &headers).await?;
    let force = match request {
        Err(JsonRejection::MissingJsonContentType(_)) => false,
        request => body(request)?.force,
    };
    let rbac = super::rbac_of(&state)?;
    let org = model.organization_id;
    let mut def: Definition = serde_json::from_value(model.definition.clone())
        .map_err(|e| ApiError::bad_request(format!("Definição inválida: {e}")))?;
    let ds = def.data_source.clone();
    let ds = ds.as_str();
    let own = rbac
        .store
        .connections_of_organization(org)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    if !own.iter().any(|c| c.name == ds) {
        return Err(ApiError::bad_request(format!(
            "A conexão {ds} não é desta organização"
        )));
    }
    let writer = writer(&state)?;

    // Not while a data source is being deleted; see `generate_schema`.
    let _guard = crate::handlers::data_sources::WRITE.lock().await;
    let columns = crate::handlers::playground::data_source_schema(ds).await?;
    let db_type = db_type_of(&state, ds);
    let records = rbac
        .store
        .cube_model_cubes(model.id)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    cube_model::upgrade(&mut def, &conversion_name(&model, &records));
    let built = cube_model::build_all(&model.name, model.id, &def, &columns, db_type)
        .map_err(ApiError::bad_request)?;
    if built.is_empty() && records.is_empty() {
        return Err(ApiError::bad_request("Crie um cubo antes do build"));
    }
    let slugs = rbac
        .store
        .organization_slugs()
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    let name_of = |f: &ModelFile| {
        source_path(f, &slugs).unwrap_or_else(|| format!("{}/{}", scope.slug, f.path))
    };

    // The path each built cube is written at, and the names of the files removed.
    let mut paths: Vec<String> = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    writer
        .write(&actor, "build model", |rows| {
            let files: Vec<&ModelFile> = rows
                .files
                .iter()
                .filter(|f| f.organization_id == Some(org))
                .collect();
            let by_id = |id: uuid::Uuid| files.iter().copied().find(|f| f.id == id);
            // A file holds what its build wrote: the hash of the record of
            // that file. A file no record names was not recorded: edited.
            let edited = |f: &ModelFile| {
                !force
                    && !records
                        .iter()
                        .any(|r| r.file_id == f.id && r.built_hash == sha256_hex(&f.content))
            };
            let kept: Vec<uuid::Uuid> = records
                .iter()
                .filter(|r| built.iter().any(|b| b.cube_id == r.cube_id))
                .map(|r| r.file_id)
                .collect();

            // Each built cube's file: its record's, or an unrecorded one at
            // its path with this model's header.
            let mut found: Vec<Option<&ModelFile>> = Vec::new();
            for b in &built {
                let path = format!("{}.yml", b.cube_name);
                let row = records
                    .iter()
                    .find(|r| r.cube_id == b.cube_id)
                    .and_then(|r| by_id(r.file_id))
                    .or_else(|| {
                        files.iter().copied().find(|f| {
                            f.path == path && !kept.contains(&f.id) && built_from(&f.content, model.id)
                        })
                    });
                if row.is_some_and(edited) {
                    return Err(WriteError::new(StatusCode::CONFLICT, EDITED));
                }
                found.push(row);
            }
            // The files of the cubes no longer in the model; one whose file
            // is gone is just dropped.
            let gone: Vec<&ModelFile> = records
                .iter()
                .filter(|r| !built.iter().any(|b| b.cube_id == r.cube_id))
                .filter_map(|r| by_id(r.file_id))
                .filter(|f| !found.iter().any(|row| row.is_some_and(|row| row.id == f.id)))
                .collect();
            if gone.iter().any(|f| edited(f)) {
                return Err(WriteError::new(StatusCode::CONFLICT, EDITED));
            }

            let mut changes = Vec::new();
            for (b, row) in built.iter().zip(&found) {
                match row {
                    Some(row) => {
                        paths.push(row.path.clone());
                        changes.push(ModelChange::Put {
                            id: Some(row.id),
                            organization_id: row.organization_id,
                            path: row.path.clone(),
                            content: b.content.clone(),
                            expected_version: Some(row.version),
                            managed_by: None,
                        });
                    }
                    None => {
                        let path = format!("{}.yml", b.cube_name);
                        // The file of another cube of this model, renamed
                        // since it was built there.
                        let sibling = built.iter().zip(&found).find_map(|(other, row)| {
                            row.filter(|row| row.path == path).map(|_| &other.cube_name)
                        });
                        if let Some(other) = sibling {
                            return Err(WriteError::new(
                                StatusCode::CONFLICT,
                                format!(
                                    "O arquivo {path} é do cubo {other} deste model; renomeie um dos cubos"
                                ),
                            ));
                        }
                        if files.iter().any(|f| f.path == path) {
                            return Err(WriteError::new(
                                StatusCode::CONFLICT,
                                format!(
                                    "O caminho {path} já é de outro arquivo; renomeie ou mova esse arquivo"
                                ),
                            ));
                        }
                        paths.push(path.clone());
                        changes.push(ModelChange::Put {
                            id: None,
                            organization_id: Some(org),
                            path,
                            content: b.content.clone(),
                            expected_version: None,
                            managed_by: None,
                        });
                    }
                }
            }
            // A cube name another file of the organization declares, after the
            // paths: a new cube's path taken by another file says so first.
            let ours = |f: &ModelFile| {
                found.iter().any(|row| row.is_some_and(|row| row.id == f.id))
                    || gone.iter().any(|g| g.id == f.id)
            };
            let declared: Vec<(&ModelFile, Vec<String>)> = files
                .iter()
                .copied()
                .filter(|f| !ours(f))
                .map(|f| (f, cubes_declared(&f.content)))
                .collect();
            for b in &built {
                if let Some((f, _)) = declared.iter().find(|(_, names)| names.contains(&b.cube_name)) {
                    return Err(WriteError::new(
                        StatusCode::CONFLICT,
                        format!("O cubo {} já existe em {}, fora deste model", b.cube_name, name_of(f)),
                    ));
                }
            }

            for f in &gone {
                removed.push(name_of(f));
                changes.push(ModelChange::Delete {
                    id: f.id,
                    expected_version: Some(f.version),
                });
            }
            Ok(changes)
        })
        .await
        .map_err(|e| redacted(e, scope.owner_only, &scope.slug))?;

    // `write` does not say which rows it wrote: read them back by path.
    let rows = rbac
        .store
        .model_rows()
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    let mut new_records = Vec::new();
    let mut cubes = Vec::new();
    for (b, path) in built.iter().zip(&paths) {
        let row = rows
            .files
            .iter()
            .find(|f| f.organization_id == Some(org) && &f.path == path)
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::CONFLICT,
                    "O modelo mudou durante o build; tente de novo",
                )
            })?;
        new_records.push(BuiltRecord {
            cube_id: b.cube_id.clone(),
            cube_name: b.cube_name.clone(),
            file_id: row.id,
            built_hash: sha256_hex(&b.content),
        });
        cubes.push(json!({
            "cubeName": b.cube_name,
            "fileName": name_of(row),
            "content": b.content,
        }));
    }
    rbac.store
        .set_cube_model_cubes(model.id, &new_records)
        .await
        .map_err(model_error)?;
    let detail = detail_of(&state, &scope).await?;
    Ok(Json(json!({
        "cubes": cubes,
        "removed": removed,
        "detail": detail,
    })))
}

/// The names of the cubes the YAML `content` declares (`cubes[].name`);
/// none when it does not parse.
fn cubes_declared(content: &str) -> Vec<String> {
    let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(content) else {
        return Vec::new();
    };
    doc.get("cubes")
        .and_then(|v| v.as_sequence())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("name").and_then(|v| v.as_str()))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Drops `draft` from the tables of `def` named in `created` (`schema.table`).
fn clear_drafts(def: &mut Definition, created: &[String]) {
    for table in def.tables.iter_mut() {
        if created.contains(&format!("{}.{}", table.schema, table.table)) {
            table.draft = None;
        }
    }
}

/// `{version, dryRun?}`: `admin` on the organization. `dryRun` answers
/// `{statements}` and runs nothing. Otherwise, with the permission
/// `/v1/execute` asks, runs one `CREATE TABLE` per draft table, stopping at
/// the first error, clears `draft` on the tables created and saves the model:
/// `{model, created, error?, detail}`. 409 when the version is stale or a
/// draft names a table the database already has (checked before any
/// statement runs).
pub async fn create_tables(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
    request: Result<Json<CreateTablesBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (model, scope, actor) = visible_model(&state, &ctx, &id).await?;
    check_lock(&super::rbac_of(&state)?.store, model.id, &headers).await?;
    let request = body(request)?;
    let rbac = super::rbac_of(&state)?;
    let mut def: Definition = serde_json::from_value(model.definition.clone())
        .map_err(|e| ApiError::bad_request(format!("Definição inválida: {e}")))?;
    if request.version != model.version {
        return Err(ApiError::new(StatusCode::CONFLICT, STALE));
    }
    let ds = def.data_source.clone();
    let ds = ds.as_str();
    let own = rbac
        .store
        .connections_of_organization(model.organization_id)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    if !own.iter().any(|c| c.name == ds) {
        return Err(ApiError::bad_request(format!(
            "A conexão {ds} não é desta organização"
        )));
    }
    let db_type = state
        .config
        .data_sources
        .iter()
        .find(|(n, _)| n == ds)
        .map(|(_, t)| t.as_str())
        .unwrap_or("");
    let statements = cube_model::create_statements(&def, db_type).map_err(ApiError::bad_request)?;
    if statements.is_empty() {
        return Err(ApiError::bad_request("Não há tabelas novas para criar"));
    }
    if request.dry_run {
        let sql: Vec<&str> = statements.iter().map(|s| s.sql.as_str()).collect();
        return Ok(Json(json!({ "statements": sql })));
    }
    crate::rbac::enforce::authorize_execute(&state, &ctx, Some(ds)).await?;

    // Not while a data source is being deleted; see `generate_schema`.
    let _guard = crate::handlers::data_sources::WRITE.lock().await;
    // The lock may have been waited for: a request already stale runs nothing.
    let current = rbac
        .store
        .cube_model(model.id)
        .await
        .map_err(|e| store_error(e, "Conflict"))?
        .ok_or_else(not_found)?;
    if current.version != request.version {
        return Err(ApiError::new(StatusCode::CONFLICT, STALE));
    }
    let existing = crate::handlers::playground::data_source_schema(ds).await?;
    for table in def.tables.iter().filter(|t| t.draft.is_some()) {
        if existing
            .get(&table.schema)
            .is_some_and(|tables| tables.contains_key(&table.table))
        {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                format!("A tabela {}.{} já existe no banco", table.schema, table.table),
            ));
        }
    }

    let mut created: Vec<String> = Vec::new();
    let mut error: Option<String> = None;
    let mut ran = 0;
    for statement in &statements {
        let sql = &statement.sql;
        // A draft's CREATE TABLE names its table; anything else, its first line.
        let name = statement
            .table
            .clone()
            .unwrap_or_else(|| sql.lines().next().unwrap_or("").to_string());
        let driver = match crate::orchestrator_adapter::driver_factory()(ds.to_string()).await {
            Ok(driver) => driver,
            Err(e) => {
                error = Some(format!("{name}: {e}"));
                break;
            }
        };
        tracing::info!(data_source = %ds, table = %name, "creating a model's table");
        let result = tokio::time::timeout(
            STATEMENT_TIMEOUT,
            driver.query(sql, &[], &cubedriver::QueryOptions::default()),
        )
        .await;
        let _ = driver.release().await;
        match result {
            Ok(Ok(_)) => {
                ran += 1;
                if statement.creates {
                    if let Some(table) = &statement.table {
                        created.push(table.clone());
                    }
                }
            }
            Ok(Err(e)) => {
                error = Some(format!("{name}: {e}"));
                break;
            }
            Err(_) => {
                error = Some(format!(
                    "{name}: o comando não terminou em {}s",
                    STATEMENT_TIMEOUT.as_secs()
                ));
                break;
            }
        }
    }

    // A created table's draft is cleared even when one of its indexes, a
    // script or a foreign key then failed: the table exists. What did not
    // run is listed, to run by hand.
    if let Some(message) = error.as_mut() {
        let rest: Vec<&str> = statements
            .iter()
            .skip(ran + 1)
            .map(|s| s.sql.as_str())
            .collect();
        if !rest.is_empty() {
            message.push_str(&format!(
                "\nNão rodaram ({}):\n{};",
                rest.len(),
                rest.join(";\n")
            ));
        }
    }

    // The model may have been edited or deleted while the statements ran:
    // clear the drafts on its current definition, and retry on a stale save.
    let mut model = model;
    if !created.is_empty() {
        let mut saved = false;
        for _ in 0..5 {
            let Some(current) = rbac
                .store
                .cube_model(model.id)
                .await
                .map_err(|e| store_error(e, "Conflict"))?
            else {
                break;
            };
            let mut def: Definition = serde_json::from_value(current.definition.clone())
                .map_err(|e| ApiError::bad_request(format!("Definição inválida: {e}")))?;
            clear_drafts(&mut def, &created);
            let definition =
                serde_json::to_value(&def).map_err(|e| ApiError::bad_request(e.to_string()))?;
            match rbac
                .store
                .update_cube_model(current.id, None, Some(&definition), current.version, &actor)
                .await
            {
                Ok(updated) => {
                    model = updated;
                    saved = true;
                    break;
                }
                Err(StoreError::Conflict(m)) if m == "stale model version" => continue,
                Err(StoreError::NotFound) => break,
                Err(e) => return Err(model_error(e)),
            }
        }
        if !saved {
            let note = format!(
                "Tabelas criadas ({}), mas o model não pôde ser atualizado (foi apagado ou mudou); abra de novo",
                created.join(", ")
            );
            error = Some(match error {
                Some(e) => format!("{e}; {note}"),
                None => note,
            });
        }
    }
    let detail = detail_of(&state, &scope).await?;
    let mut out = json!({ "model": view_of(&rbac.store, &model).await?, "created": created, "detail": detail });
    if let Some(error) = error {
        out["error"] = json!(error);
    }
    Ok(Json(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_drafts_acts_on_the_current_definition() {
        let table = |name: &str| json!({
            "alias": name, "schema": "shop", "table": name, "x": 0, "y": 0,
            "draft": { "columns": [{ "name": "id", "type": "integer", "nullable": false, "primaryKey": true }] },
        });
        // The model as edited meanwhile: `a` moved and `b` is new.
        let mut def: Definition = serde_json::from_value(json!({
            "dataSource": "d", "tables": [table("a"), table("b")], "joins": [],
        })).unwrap();
        clear_drafts(&mut def, &["shop.a".to_string(), "shop.gone".to_string()]);
        assert!(def.tables[0].draft.is_none());
        assert!(def.tables[1].draft.is_some());
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SqlOptions {
    #[serde(default)]
    pub drop: bool,
    #[serde(default)]
    pub only_new: bool,
    #[serde(default)]
    pub only: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SqlBody {
    pub definition: Value,
    #[serde(default)]
    pub options: SqlOptions,
}

/// `{definition, options}` → `{script}`: the SQL of the definition as sent
/// (it may not be saved yet) in its connection's dialect; runs nothing.
/// `admin` on the organization.
pub async fn sql(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
    request: Result<Json<SqlBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (model, _, _) = visible_model(&state, &ctx, &id).await?;
    let request = body(request)?;
    let rbac = super::rbac_of(&state)?;
    let def: Definition = serde_json::from_value(request.definition)
        .map_err(|e| ApiError::bad_request(format!("Definição inválida: {e}")))?;
    cube_model::validate(&def).map_err(ApiError::bad_request)?;
    let ds = def.data_source.as_str();
    let own = rbac
        .store
        .connections_of_organization(model.organization_id)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    if !own.iter().any(|c| c.name == ds) {
        return Err(ApiError::bad_request(format!(
            "A conexão {ds} não é desta organização"
        )));
    }
    let db_type = db_type_of(&state, ds);
    if cube_model::dialect(db_type).is_none() {
        return Err(ApiError::bad_request(
            "Gerar SQL só para conexões Postgres ou MySQL",
        ));
    }
    cube_model::validate_types(&def, db_type).map_err(ApiError::bad_request)?;
    let columns = crate::handlers::playground::data_source_schema(ds).await?;
    let comments = def
        .settings
        .as_ref()
        .and_then(|s| s.sql_generation.as_ref())
        .and_then(|g| g.comments)
        .unwrap_or(true);
    let options = crate::rbac::ddl::ScriptOptions {
        drop: request.options.drop,
        only_new: request.options.only_new,
        only: request.options.only,
        comments,
    };
    let script = crate::rbac::ddl::script(&def, &columns, db_type, &options)
        .map_err(ApiError::bad_request)?;
    Ok(Json(json!({ "script": script })))
}
