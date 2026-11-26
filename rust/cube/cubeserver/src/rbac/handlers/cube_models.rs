//! Models: `/v1/organizations/{id}/cube-models` and `/v1/cube-models/{id}`.
//!
//! A model is a canvas of tables of one connection, linked by their columns,
//! that Build cube turns into ONE cube file of the organization (see
//! `rbac::cube_model`). The organization's admins manage its models, as they
//! do its model files; a model of another organization answers 404. Deleting
//! a model keeps its cube file.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
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
use crate::rbac::model_store::ModelChange;
use crate::rbac::model_writer::WriteError;
use crate::rbac::store::{CubeModel, StoreError};
use crate::services::RequestContext;

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

fn view(model: &CubeModel) -> Value {
    json!({
        "id": model.id,
        "organizationId": model.organization_id,
        "name": model.name,
        "definition": model.definition,
        "cubeName": model.cube_name,
        "version": model.version,
        "updatedAt": model.updated_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "updatedBy": model.updated_by,
    })
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

/// → `{models: [...]}`, by name: `admin` on the organization.
pub async fn list(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(org): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (scope, _) = org_scope(&state, &ctx, &org).await?;
    let rbac = super::rbac_of(&state)?;
    let org = scope.organization.ok_or_else(not_found)?;
    let models: Vec<Value> = rbac
        .store
        .cube_models_of(org)
        .await
        .map_err(|e| store_error(e, "Conflict"))?
        .iter()
        .map(view)
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
    Ok((StatusCode::CREATED, Json(view(&model))))
}

/// `admin` on the organization.
pub async fn get(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (model, _, _) = visible_model(&state, &ctx, &id).await?;
    let mut body = view(&model);
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
    request: Result<Json<ModelUpdate>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (model, _, actor) = visible_model(&state, &ctx, &id).await?;
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
    Ok(Json(view(&updated)))
}

/// `?version=` → 204: `admin` on the organization. The cube file stays.
pub async fn delete(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
    Query(params): Query<DeleteParams>,
) -> Result<StatusCode, ApiError> {
    let (model, _, _) = visible_model(&state, &ctx, &id).await?;
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

/// Whether `content` starts with the header [`cube_model::build`] writes
/// for model `id` (the name in it may have changed since).
fn built_from(content: &str, id: uuid::Uuid) -> bool {
    content.lines().next().is_some_and(|line| {
        line.starts_with("# Built from model ")
            && line.ends_with(&format!(" ({id}): Build cube rewrites this file"))
    })
}

/// `{force?}` → `{fileName, content, cubeName, detail}`: `admin` on the
/// organization. Reads the columns of the model's connection, builds the
/// cube and writes it: over the file of the last build when that file still
/// holds what was built (or with `force`), else 409; at `<cubeName>.yml`
/// when there is no such file. A file at that path that carries this model's
/// header is taken as the last build's, its record lost. `detail` is the
/// organization's new detail.
pub async fn build(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
    request: Result<Json<BuildBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (model, scope, actor) = visible_model(&state, &ctx, &id).await?;
    let force = match request {
        Err(JsonRejection::MissingJsonContentType(_)) => false,
        request => body(request)?.force,
    };
    let rbac = super::rbac_of(&state)?;
    let org = model.organization_id;
    let def: Definition = serde_json::from_value(model.definition.clone())
        .map_err(|e| ApiError::bad_request(format!("Definição inválida: {e}")))?;
    let ds = def.data_source.as_str();
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
    let db_type = state
        .config
        .data_sources
        .iter()
        .find(|(n, _)| n == ds)
        .map(|(_, t)| t.as_str())
        .unwrap_or("");
    let cube_name = model
        .cube_name
        .clone()
        .unwrap_or_else(|| snake(&model.name));
    let content = cube_model::build(&model.name, model.id, &cube_name, &def, &columns, db_type)
        .map_err(ApiError::bad_request)?;

    let mut path = format!("{cube_name}.yml");
    writer
        .write(&actor, "build model", |rows| {
            let built = model
                .cube_file_id
                .and_then(|file| {
                    rows.files
                        .iter()
                        .find(|f| f.id == file && f.organization_id == Some(org))
                })
                // A build whose record was lost: the file at the path still
                // carries this model's header.
                .or_else(|| {
                    rows.files.iter().find(|f| {
                        f.organization_id == Some(org)
                            && f.path == path
                            && built_from(&f.content, model.id)
                    })
                });
            if let Some(row) = built {
                if !force && model.cube_built_hash.as_deref() != Some(&sha256_hex(&row.content)) {
                    return Err(WriteError::new(StatusCode::CONFLICT, EDITED));
                }
                path = row.path.clone();
                return Ok(vec![ModelChange::Put {
                    id: Some(row.id),
                    organization_id: row.organization_id,
                    path: row.path.clone(),
                    content: content.clone(),
                    expected_version: Some(row.version),
                    managed_by: None,
                }]);
            }
            if rows
                .files
                .iter()
                .any(|f| f.organization_id == Some(org) && f.path == path)
            {
                return Err(WriteError::new(
                    StatusCode::CONFLICT,
                    format!(
                        "O caminho {path} já é de outro arquivo; renomeie ou mova esse arquivo"
                    ),
                ));
            }
            Ok(vec![ModelChange::Put {
                id: None,
                organization_id: Some(org),
                path: path.clone(),
                content: content.clone(),
                expected_version: None,
                managed_by: None,
            }])
        })
        .await
        .map_err(|e| redacted(e, scope.owner_only, &scope.slug))?;

    // `write` does not say which row it wrote: read it back by its path.
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
    let row = rows
        .files
        .iter()
        .find(|f| f.organization_id == Some(org) && f.path == path)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::CONFLICT,
                "O modelo mudou durante o build; tente de novo",
            )
        })?;
    rbac.store
        .set_cube_model_build(model.id, &cube_name, row.id, &sha256_hex(&content))
        .await
        .map_err(model_error)?;
    let file_name = source_path(row, &slugs).unwrap_or_else(|| format!("{}/{path}", scope.slug));
    let detail = detail_of(&state, &scope).await?;
    Ok(Json(json!({
        "fileName": file_name,
        "content": content,
        "cubeName": cube_name,
        "detail": detail,
    })))
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
    request: Result<Json<CreateTablesBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (model, scope, actor) = visible_model(&state, &ctx, &id).await?;
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
                if let Some(table) = &statement.table {
                    created.push(table.clone());
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
    let mut out = json!({ "model": view(&model), "created": created, "detail": detail });
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
