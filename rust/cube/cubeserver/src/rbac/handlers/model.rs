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
use crate::handlers::tree::org_detail as detail_of;
use crate::rbac::model_snapshot::{is_editable, sources};
use crate::rbac::model_store::{ModelChange, ModelFile};
use crate::rbac::model_writer::{ModelWriter, WriteError};
use crate::services::RequestContext;

const SYNC_MESSAGE: &str = "Este arquivo é gerado a partir do catálogo do SofiaDB e é reescrito a cada sincronização. Para personalizar uma tabela, escreva o cubo dela em outro arquivo da organização, com outro nome; na próxima sincronização o cubo gerado sai";
const TEMPLATE_MESSAGE: &str = "Arquivos .jinja e .j2 não são editáveis aqui";

/// Whose files a route serves.
struct Scope {
    organization: Option<Uuid>,
    slug: String,
}

fn writer(state: &AppState) -> Result<Arc<ModelWriter>, ApiError> {
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

async fn org_scope(
    state: &AppState,
    ctx: &RequestContext,
    id: &str,
) -> Result<(Scope, String), ApiError> {
    let (rbac, user) = caller(state, ctx)?;
    let org = visible_organization(rbac, id, "admin", user).await?;
    Ok((
        Scope {
            organization: Some(org.id),
            slug: org.slug,
        },
        user.to_string(),
    ))
}

fn platform_scope(state: &AppState, ctx: &RequestContext) -> Result<(Scope, String), ApiError> {
    let (_, user) = caller(state, ctx)?;
    require_platform_admin(ctx, "reach the platform model")?;
    Ok((
        Scope {
            organization: None,
            slug: PLATFORM_FOLDER.to_string(),
        },
        user.to_string(),
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
    let mut detail = detail_of(state, &scope.slug).await?;
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
        .await?;
    after_write(state, &scope, written.fixed).await
}

async fn delete_file(
    state: &AppState,
    scope: Scope,
    user: String,
    file: &str,
    version: Option<i32>,
) -> Result<StatusCode, ApiError> {
    let writer = writer(state)?;
    let id = resource_id(file, "file")?;
    writer
        .write(&user, "delete file", |rows| {
            let row = find(&rows.files, &scope, id)?;
            check_editable(row)?;
            if version.is_some_and(|v| v != row.version) {
                return Err(WriteError::new(
                    StatusCode::CONFLICT,
                    "O arquivo mudou desde que você abriu; abra de novo",
                ));
            }
            Ok(vec![ModelChange::Delete {
                id,
                expected_version: version,
            }])
        })
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /organizations/{id}/model`
pub async fn org_detail(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (scope, _) = org_scope(&state, &ctx, &id).await?;
    Ok(Json(detail_of(&state, &scope.slug).await?))
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
    Ok(Json(detail_of(&state, &scope.slug).await?))
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
    let (_, user) = caller(&state, &ctx)?;
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
