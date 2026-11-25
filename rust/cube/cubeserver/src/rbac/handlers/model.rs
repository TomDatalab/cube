//! Reading, editing, deleting, moving and exporting the model files kept in
//! the RBAC database: `/v1/organizations/{id}/model*`, `/v1/platform/model*`
//! and `/v1/model/*`. Every write goes through the `ModelWriter`, which
//! validates the candidate model and serializes writers.

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use cubemodel::ownership::PLATFORM_FOLDER;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::connections::require_platform_admin;
use super::organizations::visible_organization;
use super::{body, caller, resource_id, store_error};
use crate::app::AppState;
use crate::error::ApiError;
use crate::handlers::tree::org_detail_for;
use crate::rbac::model_redact::redact;
use crate::handlers::scaffold::{plan, split_schema, GenerateRequest};
use crate::rbac::model_snapshot::{is_editable, source_path, sources, validate_path};
use crate::rbac::model_sync::{SyncContext, SYNC_MANAGER};
use crate::rbac::model_store::{ModelChange, ModelFile};
use crate::rbac::model_writer::{ModelWriter, WriteError};
use crate::services::RequestContext;

const SYNC_MESSAGE: &str = "Este arquivo é gerado a partir do catálogo do SofiaDB e é reescrito a cada sincronização. Para personalizar uma tabela, escreva o cubo dela em outro arquivo da organização, com outro nome; na próxima sincronização o cubo gerado sai";
const TEMPLATE_MESSAGE: &str = "Arquivos .jinja e .j2 não são editáveis aqui";

/// Whose files a route serves.
pub(crate) struct Scope {
    pub(crate) organization: Option<Uuid>,
    pub(crate) slug: String,
    /// The caller owns the organization without being a platform admin:
    /// nothing returned may reveal another organization's files.
    pub(crate) owner_only: bool,
}

/// `e` as `owner_only` callers may see it.
pub(crate) fn redacted(mut e: WriteError, owner_only: bool, slug: &str) -> WriteError {
    if owner_only && !e.problems.is_empty() {
        e.problems = redact(&e.problems, &e.problems, slug, &e.defs);
    }
    e
}

/// The node's detail; an organization's also lists its models
/// (`models: [{id, name, cubeName}]`, by name).
pub(crate) async fn detail_of(state: &AppState, scope: &Scope) -> Result<Value, ApiError> {
    let mut detail = org_detail_for(state, &scope.slug, scope.owner_only).await?;
    if let Some(org) = scope.organization {
        let models: Vec<Value> = super::rbac_of(state)?
            .store
            .cube_models_of(org)
            .await
            .map_err(|e| store_error(e, "Conflict"))?
            .iter()
            .map(|m| json!({ "id": m.id, "name": m.name, "cubeName": m.cube_name }))
            .collect();
        detail["models"] = json!(models);
    }
    Ok(detail)
}

pub(crate) fn writer(state: &AppState) -> Result<Arc<ModelWriter>, ApiError> {
    state
        .model_writer
        .clone()
        .ok_or_else(|| ApiError::not_found("The model is not stored in the database"))
}

fn file_not_found() -> WriteError {
    WriteError::new(StatusCode::NOT_FOUND, "File not found")
}

/// The file of `id` inside `scope`; a file of another organization is
/// indistinguishable from one that does not exist.
fn find<'a>(files: &'a [ModelFile], scope: &Scope, id: Uuid) -> Result<&'a ModelFile, WriteError> {
    files
        .iter()
        .find(|f| f.id == id && f.organization_id == scope.organization)
        .ok_or_else(file_not_found)
}

/// 400 unless the file can be edited by hand.
fn check_editable(file: &ModelFile) -> Result<(), WriteError> {
    if is_editable(&file.path, file.managed_by.as_deref()) {
        return Ok(());
    }
    Err(WriteError::bad_request(
        if file.managed_by.as_deref().is_some_and(|m| !m.is_empty()) {
            SYNC_MESSAGE
        } else {
            TEMPLATE_MESSAGE
        },
    ))
}

/// The organization `id` as its admins reach it, and the caller's username.
pub(crate) async fn org_scope(
    state: &AppState,
    ctx: &RequestContext,
    id: &str,
) -> Result<(Scope, String), ApiError> {
    let (rbac, user) = caller(state, ctx)?;
    let org = visible_organization(rbac, id, "admin", user).await?;
    let owner_only = !rbac.is_platform_admin(user).await?;
    Ok((
        Scope {
            organization: Some(org.id),
            slug: org.slug,
            owner_only,
        },
        super::actor(ctx)?.to_string(),
    ))
}

fn platform_scope(state: &AppState, ctx: &RequestContext) -> Result<(Scope, String), ApiError> {
    caller(state, ctx)?;
    require_platform_admin(ctx, "reach the platform model")?;
    Ok((
        Scope {
            organization: None,
            slug: PLATFORM_FOLDER.to_string(),
            owner_only: false,
        },
        super::actor(ctx)?.to_string(),
    ))
}

async fn read_file(state: &AppState, scope: Scope, file: &str) -> Result<Json<Value>, ApiError> {
    let rbac = super::rbac_of(state)?;
    let id = resource_id(file, "file")?;
    let row = rbac
        .store
        .model_file(id)
        .await
        .map_err(|e| store_error(e, "Conflict"))?
        .filter(|f| f.organization_id == scope.organization)
        .ok_or_else(|| ApiError::not_found("File not found"))?;
    Ok(Json(json!({
        "id": row.id,
        "path": row.path,
        "content": row.content,
        "version": row.version,
        "editable": is_editable(&row.path, row.managed_by.as_deref()),
        "managedBy": row.managed_by,
        "updatedAt": row.updated_at.to_rfc3339(),
        "updatedBy": row.updated_by,
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PutFile {
    content: String,
    version: i32,
}

#[derive(Deserialize)]
pub struct DeleteParams {
    version: Option<i32>,
}

/// The new detail of the node, with how many old problems the write fixed.
async fn after_write(
    state: &AppState,
    scope: &Scope,
    fixed: usize,
) -> Result<Json<Value>, ApiError> {
    let mut detail = detail_of(state, scope).await?;
    if fixed > 0 {
        detail["fixed"] = json!(fixed);
    }
    Ok(Json(detail))
}

async fn put_file(
    state: &AppState,
    scope: Scope,
    user: String,
    file: &str,
    request: PutFile,
) -> Result<Json<Value>, ApiError> {
    let writer = writer(state)?;
    let id = resource_id(file, "file")?;
    let written = writer
        .write(&user, "edit file", |rows| {
            let row = find(&rows.files, &scope, id)?;
            check_editable(row)?;
            if row.version != request.version {
                return Err(WriteError::new(
                    StatusCode::CONFLICT,
                    "O arquivo mudou desde que você abriu; abra de novo",
                ));
            }
            Ok(vec![ModelChange::Put {
                id: Some(id),
                organization_id: row.organization_id,
                path: row.path.clone(),
                content: request.content,
                expected_version: Some(request.version),
                managed_by: None,
            }])
        })
        .await
        .map_err(|e| redacted(e, scope.owner_only, &scope.slug))?;
    after_write(state, &scope, written.fixed).await
}

async fn delete_file(
    state: &AppState,
    scope: Scope,
    user: String,
    file: &str,
    version: Option<i32>,
) -> Result<StatusCode, ApiError> {
    let version =
        version.ok_or_else(|| ApiError::bad_request("Informe a versão do arquivo (?version=)"))?;
    let writer = writer(state)?;
    let id = resource_id(file, "file")?;
    writer
        .write(&user, "delete file", |rows| {
            let row = find(&rows.files, &scope, id)?;
            check_editable(row)?;
            if version != row.version {
                return Err(WriteError::new(
                    StatusCode::CONFLICT,
                    "O arquivo mudou desde que você abriu; abra de novo",
                ));
            }
            Ok(vec![ModelChange::Delete {
                id,
                expected_version: Some(version),
            }])
        })
        .await
        .map_err(|e| redacted(e, scope.owner_only, &scope.slug))?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /organizations/{id}/model`
pub async fn org_detail(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (scope, _) = org_scope(&state, &ctx, &id).await?;
    Ok(Json(detail_of(&state, &scope).await?))
}

/// `GET /organizations/{id}/model/files/{file}`
pub async fn org_get_file(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path((id, file)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let (scope, _) = org_scope(&state, &ctx, &id).await?;
    read_file(&state, scope, &file).await
}

/// `PUT /organizations/{id}/model/files/{file}` `{content, version}`
pub async fn org_put_file(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path((id, file)): Path<(String, String)>,
    request: Result<Json<PutFile>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (scope, user) = org_scope(&state, &ctx, &id).await?;
    put_file(&state, scope, user, &file, body(request)?).await
}

/// `DELETE /organizations/{id}/model/files/{file}?version=`
pub async fn org_delete_file(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path((id, file)): Path<(String, String)>,
    Query(params): Query<DeleteParams>,
) -> Result<StatusCode, ApiError> {
    let (scope, user) = org_scope(&state, &ctx, &id).await?;
    delete_file(&state, scope, user, &file, params.version).await
}

/// `GET /platform/model`
pub async fn platform_detail(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
) -> Result<Json<Value>, ApiError> {
    let (scope, _) = platform_scope(&state, &ctx)?;
    Ok(Json(detail_of(&state, &scope).await?))
}

/// `GET /platform/model/files/{file}`
pub async fn platform_get_file(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(file): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (scope, _) = platform_scope(&state, &ctx)?;
    read_file(&state, scope, &file).await
}

/// `PUT /platform/model/files/{file}` `{content, version}`
pub async fn platform_put_file(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(file): Path<String>,
    request: Result<Json<PutFile>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (scope, user) = platform_scope(&state, &ctx)?;
    put_file(&state, scope, user, &file, body(request)?).await
}

/// `DELETE /platform/model/files/{file}?version=`
pub async fn platform_delete_file(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(file): Path<String>,
    Query(params): Query<DeleteParams>,
) -> Result<StatusCode, ApiError> {
    let (scope, user) = platform_scope(&state, &ctx)?;
    delete_file(&state, scope, user, &file, params.version).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveFile {
    organization_id: Option<Uuid>,
}

/// `PUT /model/files/{file}/organization` `{organizationId | null}`: admin.
pub async fn move_file(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(file): Path<String>,
    request: Result<Json<MoveFile>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    caller(&state, &ctx)?;
    let user = super::actor(&ctx)?;
    require_platform_admin(&ctx, "move model files")?;
    let request = body(request)?;
    let writer = writer(&state)?;
    let id = resource_id(&file, "file")?;
    let written = writer
        .write(user, "move file", |rows| {
            let row = rows
                .files
                .iter()
                .find(|f| f.id == id)
                .ok_or_else(file_not_found)?;
            if row.organization_id == request.organization_id {
                return Ok(Vec::new());
            }
            Ok(vec![ModelChange::Move {
                id,
                organization_id: request.organization_id,
            }])
        })
        .await?;
    Ok(Json(json!({ "version": written.version })))
}

/// `GET /model/export`: every file under its compile path.
pub async fn export(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, _) = caller(&state, &ctx)?;
    require_platform_admin(&ctx, "export the model")?;
    let rows = rbac
        .store
        .model_rows()
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    let slugs = rbac
        .store
        .organization_slugs()
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    let files: Vec<Value> = sources(&rows, &slugs)
        .into_iter()
        .map(|(path, content)| json!({ "path": path, "content": content }))
        .collect();
    Ok(Json(json!({ "version": rows.version, "files": files })))
}

/// Writes the cubes of `tables` into the model of one owner, all or none:
/// `organization` and `slug` are the organization's (`None` and `_platform`
/// for Platform). `schema` holds the columns of every table, keyed as the
/// data-model page sends them. Answers `[{fileName, content}]`.
pub async fn generate_into_model(
    state: &AppState,
    actor: &str,
    organization: Option<Uuid>,
    slug: &str,
    tables: &[(String, String)],
    schema: &cubedriver::DatabaseStructure,
    owner_only: bool,
) -> Result<Vec<Value>, ApiError> {
    let writer = writer(state)?;
    let rbac = super::rbac_of(state)?;
    let request = GenerateRequest {
        format: Some("yaml".to_string()),
        tables: tables.to_vec(),
        tables_schema: schema.clone(),
    };
    let planned = plan(
        std::path::Path::new(&state.config.schema_path),
        &state.config.data_sources,
        &request,
        Some(slug),
        owner_only,
    )?;
    let slugs = rbac
        .store
        .organization_slugs()
        .await
        .map_err(|e| store_error(e, "Conflict"))?;

    let to_write = planned.clone();
    writer
        .write(actor, "generate", move |rows| {
            let stale = || {
                WriteError::new(
                    StatusCode::CONFLICT,
                    "O modelo mudou enquanto era gerado; tente de novo",
                )
            };
            let mut changes = Vec::new();
            for file in &to_write {
                let mut put = ModelChange::Put {
                    id: None,
                    organization_id: organization,
                    path: file.file.clone(),
                    content: file.content.clone(),
                    expected_version: None,
                    managed_by: None,
                };
                if let Some(from) = &file.adopt_from {
                    let row = rows
                        .files
                        .iter()
                        .find(|f| source_path(f, &slugs).as_deref() == Some(from.as_str()))
                        .ok_or_else(stale)?;
                    if *from == file.name {
                        // Same place: the row stops being the generator's.
                        put = ModelChange::Put {
                            id: Some(row.id),
                            organization_id: row.organization_id,
                            path: row.path.clone(),
                            content: file.content.clone(),
                            expected_version: Some(row.version),
                            managed_by: None,
                        };
                    } else {
                        changes.push(ModelChange::Delete {
                            id: row.id,
                            expected_version: Some(row.version),
                        });
                    }
                }
                changes.push(put);
            }
            Ok(changes)
        })
        .await
        .map_err(|e| redacted(e, owner_only, slug))?;
    Ok(planned
        .into_iter()
        .map(|f| json!({ "fileName": f.name, "content": f.content }))
        .collect())
}

#[derive(Deserialize)]
pub struct GenerateBody {
    #[serde(default)]
    tables: Vec<(String, String)>,
}

/// Reads the columns of `tables` from their connections, after checking that
/// every one is a connection of the scope, then generates into its model.
async fn generate_route(
    state: &AppState,
    scope: Scope,
    user: String,
    tables: Vec<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let rbac = super::rbac_of(state)?;
    let sources = &state.config.data_sources;
    let own: Vec<String> = match scope.organization {
        Some(org) => rbac
            .store
            .connections_of_organization(org)
            .await
            .map_err(|e| store_error(e, "Conflict"))?
            .into_iter()
            .map(|c| c.name)
            .collect(),
        None => {
            let taken: std::collections::HashSet<String> = rbac
                .store
                .all_connections()
                .await
                .map_err(|e| store_error(e, "Conflict"))?
                .into_iter()
                .map(|c| c.name)
                .collect();
            let mut names: Vec<String> = sources.iter().map(|(n, _)| n.clone()).collect();
            if names.is_empty() {
                names.push("default".to_string());
            }
            names.retain(|n| !taken.contains(n));
            names
        }
    };
    let mut involved = std::collections::BTreeSet::new();
    for (schema, table) in &tables {
        let (data_source, _) = split_schema(schema, sources);
        if !own.iter().any(|n| n == data_source) {
            return Err(ApiError::bad_request(format!(
                "A tabela {schema}.{table} não é de uma conexão {}",
                if scope.organization.is_some() { "desta organização" } else { "da Plataforma" }
            )));
        }
        involved.insert(data_source.to_string());
    }

    // Not while a data source is being deleted; see `generate_schema`.
    let _guard = crate::handlers::data_sources::WRITE.lock().await;
    let mut merged = cubedriver::DatabaseStructure::new();
    for name in &involved {
        let columns = crate::handlers::playground::data_source_schema(name).await?;
        merge_columns(&mut merged, name, columns);
    }
    let files =
        generate_into_model(state, &user, scope.organization, &scope.slug, &tables, &merged, scope.owner_only)
            .await?;
    Ok(Json(json!({ "files": files })))
}

/// Adds the columns read from data source `name` to `merged`, keyed as the
/// data-model page sends the tables: `<name>.<schema>`, `default` included
/// (which [`split_schema`] reads back).
fn merge_columns(merged: &mut cubedriver::DatabaseStructure, name: &str, columns: cubedriver::DatabaseStructure) {
    for (schema, tables) in columns {
        merged.entry(format!("{name}.{schema}")).or_default().extend(tables);
    }
}

/// `POST /organizations/{id}/model/generate` `{tables: [[schema, table]]}`
pub async fn org_generate(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
    request: Result<Json<GenerateBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (scope, user) = org_scope(&state, &ctx, &id).await?;
    generate_route(&state, scope, user, body(request)?.tables).await
}

/// `POST /platform/model/generate` `{tables: [[schema, table]]}`
pub async fn platform_generate(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    request: Result<Json<GenerateBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (scope, user) = platform_scope(&state, &ctx)?;
    generate_route(&state, scope, user, body(request)?.tables).await
}

/// The caller must be the sync user: not even an admin uses the sync routes.
fn require_sync_user<'a>(
    state: &'a AppState,
    ctx: &'a RequestContext,
) -> Result<&'a crate::rbac::Rbac, ApiError> {
    // `CUBE_SYNC_USER` is a username.
    let (rbac, _) = caller(state, ctx)?;
    if super::actor(ctx)? != rbac.config.sync_user {
        return Err(ApiError::forbidden(
            "Só o usuário de sincronização usa esta rota",
        ));
    }
    Ok(rbac)
}

/// The context of the sync of `connection` over `rows`: the sync's own rows
/// (any connection) do not count.
fn context_of(
    rows: &crate::rbac::model_store::ModelRows,
    slugs: &std::collections::HashMap<Uuid, String>,
    connection: &str,
    default_db: &str,
) -> SyncContext {
    let managed = rows
        .files
        .iter()
        .filter(|f| f.managed_by.as_deref() == Some(SYNC_MANAGER))
        .filter_map(|f| source_path(f, slugs))
        .collect();
    crate::rbac::model_sync::sync_context(&sources(rows, slugs), &managed, connection, default_db)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncContextParams {
    default_db: Option<String>,
}

/// `GET /model/sync/{connection}/context?defaultDb=`: the sync user.
pub async fn sync_context(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(connection): Path<String>,
    Query(params): Query<SyncContextParams>,
) -> Result<Json<Value>, ApiError> {
    let rbac = require_sync_user(&state, &ctx)?;
    let rows = rbac
        .store
        .model_rows()
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    let slugs = rbac
        .store
        .organization_slugs()
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    let context = context_of(
        &rows,
        &slugs,
        &connection,
        params.default_db.as_deref().unwrap_or("default"),
    );
    Ok(Json(json!(context)))
}

#[derive(Deserialize)]
pub struct SyncFile {
    path: String,
    content: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncBody {
    files: Vec<SyncFile>,
    context: SyncContext,
    default_db: Option<String>,
}

/// `PUT /model/sync/{connection}` `{files, context, defaultDb}`: the sync
/// user. Replaces the rows the sync owns under `<connection>/` with `files`,
/// in the organization that owns the connection, in one validated write.
pub async fn sync_put(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(connection): Path<String>,
    request: Result<Json<SyncBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let rbac = require_sync_user(&state, &ctx)?;
    let user = super::actor(&ctx)?.to_string();
    let request = body(request)?;
    let prefix = format!("{connection}/");
    let mut seen = std::collections::HashSet::new();
    for file in &request.files {
        if !file.path.starts_with(&prefix) {
            return Err(ApiError::bad_request(format!(
                "O arquivo {} não está na pasta {prefix}",
                file.path
            )));
        }
        if let Err(why) = validate_path(&file.path) {
            return Err(ApiError::bad_request(format!(
                "Caminho inválido {}: {why}",
                file.path
            )));
        }
        if !seen.insert(file.path.as_str()) {
            return Err(ApiError::bad_request(format!(
                "O arquivo {} aparece duas vezes",
                file.path
            )));
        }
    }
    let writer = writer(&state)?;
    let slugs = rbac
        .store
        .organization_slugs()
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    let destination = rbac
        .store
        .connection(&connection)
        .await
        .map_err(|e| store_error(e, "Conflict"))?
        .map(|c| c.organization_id);
    let default_db = request.default_db.clone().unwrap_or_else(|| "default".into());

    let (mut added, mut updated, mut removed) = (0usize, 0usize, 0usize);
    let written = writer
        .write(&user, "sync", |rows| {
            if context_of(rows, &slugs, &connection, &default_db) != request.context {
                return Err(WriteError::new(
                    StatusCode::CONFLICT,
                    "O modelo mudou durante a sincronização",
                ));
            }
            let is_sync_row = |f: &ModelFile| {
                f.managed_by.as_deref() == Some(SYNC_MANAGER) && f.path.starts_with(&prefix)
            };
            let mut taken: Vec<&str> = request
                .files
                .iter()
                .filter(|file| {
                    rows.files.iter().any(|f| {
                        f.organization_id == destination && f.path == file.path && !is_sync_row(f)
                    })
                })
                .map(|file| file.path.as_str())
                .collect();
            if !taken.is_empty() {
                taken.sort();
                let message = taken
                    .iter()
                    .map(|p| format!("O caminho {p} já é de um arquivo que não é do sync; renomeie ou mova esse arquivo"))
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(WriteError::bad_request(message));
            }
            // The sync's rows of this connection, in any organization. Of a
            // path found twice, the one already in place stays.
            let mut keep: std::collections::HashMap<&str, &ModelFile> = Default::default();
            let mut changes = Vec::new();
            for row in rows.files.iter().filter(|f| {
                f.managed_by.as_deref() == Some(SYNC_MANAGER) && f.path.starts_with(&prefix)
            }) {
                match keep.get(row.path.as_str()) {
                    Some(old) if old.organization_id == destination => {
                        changes.push(ModelChange::Delete { id: row.id, expected_version: Some(row.version) });
                    }
                    Some(old) => {
                        changes.push(ModelChange::Delete { id: old.id, expected_version: Some(old.version) });
                        keep.insert(row.path.as_str(), row);
                    }
                    None => {
                        keep.insert(row.path.as_str(), row);
                    }
                }
            }
            let mut gone: Vec<&&ModelFile> = keep
                .values()
                .filter(|row| !request.files.iter().any(|f| f.path == row.path))
                .collect();
            gone.sort_by(|a, b| a.path.cmp(&b.path));
            for row in gone {
                removed += 1;
                changes.push(ModelChange::Delete { id: row.id, expected_version: Some(row.version) });
            }
            for file in &request.files {
                match keep.get(file.path.as_str()) {
                    Some(row) if row.organization_id == destination && row.content == file.content => {}
                    Some(row) => {
                        updated += 1;
                        changes.push(ModelChange::Put {
                            id: Some(row.id),
                            organization_id: destination,
                            path: file.path.clone(),
                            content: file.content.clone(),
                            expected_version: Some(row.version),
                            managed_by: Some(SYNC_MANAGER.to_string()),
                        });
                    }
                    None => {
                        added += 1;
                        changes.push(ModelChange::Put {
                            id: None,
                            organization_id: destination,
                            path: file.path.clone(),
                            content: file.content.clone(),
                            expected_version: None,
                            managed_by: Some(SYNC_MANAGER.to_string()),
                        });
                    }
                }
            }
            Ok(changes)
        })
        .await?;
    Ok(Json(json!({
        "changed": written.changed,
        "added": added,
        "updated": updated,
        "removed": removed,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merged_columns_are_keyed_as_the_page_sends_them() {
        let sources = vec![("sofiadb".to_string(), "postgres".to_string())];
        let one = |schema: &str| {
            let mut s = cubedriver::DatabaseStructure::new();
            s.entry(schema.to_string()).or_default().insert("orders".to_string(), Vec::new());
            s
        };
        let mut merged = cubedriver::DatabaseStructure::new();
        merge_columns(&mut merged, "default", one("public"));
        merge_columns(&mut merged, "sofiadb", one("dfos"));
        let keys: Vec<_> = merged.keys().cloned().collect();
        assert_eq!(keys, ["default.public", "sofiadb.dfos"]);
        // Each key reads back as its data source and schema.
        assert_eq!(split_schema("default.public", &sources), ("default", "public"));
    }
}
