//! Workspace objects (pipelines, dashboards, ...): `/v1/workspaces/{id}/objects`
//! and `/v1/objects/{id}`.
//!
//! The row lives in Postgres; `object:<id>#workspace@workspace:<ws>` in
//! SpiceDB grants `read` (workspace `view`) and `write` (workspace `edit`).
//! An object the caller cannot read answers 404, whoever owns it.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::workspaces::{visible_workspace, workspace_ref};
use super::{body, caller, resource_id, resource_name};
use crate::app::AppState;
use crate::error::ApiError;
use crate::rbac::store::{NewObject, StoreError, StoredObject};
use crate::rbac::{authz_error, unavailable, ObjectRef, Relationship, RelationshipFilter};
use crate::services::RequestContext;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateObject {
    r#type: String,
    name: String,
    #[serde(rename = "contentType")]
    content_type: String,
    content: String,
    encoding: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateObject {
    name: Option<String>,
    #[serde(rename = "contentType")]
    content_type: Option<String>,
    content: Option<String>,
    encoding: Option<String>,
    version: i32,
}

#[derive(Deserialize)]
pub struct ListParams {
    r#type: Option<String>,
}

fn object_ref(id: Uuid) -> ObjectRef {
    ObjectRef::new("object", id.to_string())
}

fn not_found() -> ApiError {
    ApiError::not_found("object not found")
}

fn store_failed(e: StoreError) -> ApiError {
    tracing::error!("RBAC store failed: {e}");
    unavailable()
}

/// The object type: a short identifier (`pipeline`, `dashboard`).
fn object_type(value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    let valid = !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if valid {
        Ok(value.to_string())
    } else {
        Err(ApiError::bad_request(
            "The type must be 1-64 letters, digits, '_', '-' or '.'",
        ))
    }
}

/// A media type that can be sent back as a `Content-Type` header.
fn content_type(value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    let essence = value.split(';').next().unwrap_or_default().trim();
    let valid = value.len() <= 255
        && value.chars().all(|c| c.is_ascii_graphic() || c == ' ')
        && essence
            .split_once('/')
            .is_some_and(|(kind, sub)| !kind.is_empty() && !sub.is_empty())
        && HeaderValue::from_str(value).is_ok();
    if valid {
        Ok(value.to_string())
    } else {
        Err(ApiError::bad_request(
            "contentType must be a media type such as application/json",
        ))
    }
}

/// The bytes of `content`: as is, or decoded with `"encoding": "base64"`.
fn content_bytes(content: String, encoding: Option<&str>) -> Result<Vec<u8>, ApiError> {
    match encoding {
        None | Some("utf8") | Some("utf-8") => Ok(content.into_bytes()),
        Some("base64") => base64::engine::general_purpose::STANDARD
            .decode(content.trim())
            .map_err(|_| ApiError::bad_request("content is not valid base64")),
        Some(other) => Err(ApiError::bad_request(format!(
            "Unknown encoding {other}; use base64 or leave it out"
        ))),
    }
}

fn metadata(o: &StoredObject) -> serde_json::Map<String, Value> {
    let time =
        |t: &chrono::DateTime<chrono::Utc>| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let value = json!({
        "id": o.id,
        "workspaceId": o.workspace_id,
        "type": o.r#type,
        "name": o.name,
        "contentType": o.content_type,
        "version": o.version,
        "createdAt": time(&o.created_at),
        "createdBy": o.created_by,
        "updatedAt": time(&o.updated_at),
        "updatedBy": o.updated_by,
    });
    match value {
        Value::Object(map) => map,
        _ => unreachable!(),
    }
}

/// Metadata and content: text when it is UTF-8, base64 otherwise.
fn with_content(o: StoredObject) -> Value {
    let mut map = metadata(&o);
    let (encoding, content) = match String::from_utf8(o.content) {
        Ok(text) => ("utf8", text),
        Err(e) => (
            "base64",
            base64::engine::general_purpose::STANDARD.encode(e.into_bytes()),
        ),
    };
    map.insert("encoding".into(), json!(encoding));
    map.insert("content".into(), json!(content));
    Value::Object(map)
}

/// The essence of a media type (`type/subtype`, lowercase).
fn essence(media_type: &str) -> String {
    media_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

/// Whether `Accept` names the object's own media type (wildcards do not).
fn accepts_raw(headers: &HeaderMap, content_type: &str) -> bool {
    let wanted = essence(content_type);
    headers
        .get_all(header::ACCEPT)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|range| essence(range) == wanted)
}

/// The object after `permission` on it is checked: 404 when the caller
/// cannot read it (or it does not exist), 403 when they lack `permission`.
async fn visible_object(
    state: &AppState,
    ctx: &RequestContext,
    id: &str,
    permission: &str,
) -> Result<StoredObject, ApiError> {
    let (rbac, user) = caller(state, ctx)?;
    let id = resource_id(id, "object")?;
    rbac.require_visible(&object_ref(id), "read", permission, user)
        .await?;
    rbac.store
        .object(id)
        .await
        .map_err(store_failed)?
        .ok_or_else(not_found)
}

/// `POST /v1/workspaces/{id}/objects`: `edit` on the workspace.
pub async fn create(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
    request: Result<Json<CreateObject>, JsonRejection>,
) -> Result<Response, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let ws = visible_workspace(rbac, &id, "edit", user).await?;
    let request = body(request)?;
    let new = NewObject {
        workspace_id: ws.id,
        r#type: object_type(&request.r#type)?,
        name: resource_name(&request.name)?,
        content_type: content_type(&request.content_type)?,
        content: content_bytes(request.content, request.encoding.as_deref())?,
        created_by: user.to_string(),
    };

    // The row and the grant together: the transaction rolls back unless
    // SpiceDB took the relationship.
    let tx = rbac.store.transaction().await.map_err(store_failed)?;
    let object = tx.create_object(&new).await.map_err(store_failed)?;
    let grant = Relationship {
        resource: object_ref(object.id),
        relation: "workspace",
        subject: workspace_ref(ws.id),
    };
    rbac.authz
        .touch(std::slice::from_ref(&grant))
        .await
        .map_err(authz_error)?;
    if let Err(e) = tx.commit().await {
        // The grant names an object that does not exist; drop it.
        if let Err(cleanup) = rbac.authz.delete(&[grant]).await {
            tracing::warn!("Cannot remove the grant of an uncommitted object: {cleanup}");
        }
        return Err(store_failed(e));
    }
    Ok((StatusCode::CREATED, Json(Value::Object(metadata(&object)))).into_response())
}

/// `GET /v1/workspaces/{id}/objects?type=`: `view` on the workspace.
/// Metadata only.
pub async fn list(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
    Query(params): Query<ListParams>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let ws = visible_workspace(rbac, &id, "view", user).await?;
    let objects = rbac
        .store
        .objects_of_workspace(ws.id, params.r#type.as_deref())
        .await
        .map_err(store_failed)?;
    let objects: Vec<Value> = objects.iter().map(|o| Value::Object(metadata(o))).collect();
    Ok(Json(json!({ "objects": objects })))
}

/// `GET /v1/objects/{id}`: `read`. With `Accept` naming the object's content
/// type, the content itself.
pub async fn get(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let object = visible_object(&state, &ctx, &id, "read").await?;
    if accepts_raw(&headers, &object.content_type) {
        if let Ok(content_type) = HeaderValue::from_str(&object.content_type) {
            return Ok((
                [
                    (header::CONTENT_TYPE, content_type),
                    // Never rendered as anything but what it says it is.
                    (
                        header::X_CONTENT_TYPE_OPTIONS,
                        HeaderValue::from_static("nosniff"),
                    ),
                    (
                        header::CONTENT_SECURITY_POLICY,
                        HeaderValue::from_static("sandbox; default-src 'none'"),
                    ),
                ],
                object.content,
            )
                .into_response());
        }
    }
    Ok(Json(with_content(object)).into_response())
}

/// `PUT /v1/objects/{id}`: `write`. 409 unless `version` is the current one.
pub async fn update(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
    request: Result<Json<UpdateObject>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let object = visible_object(&state, &ctx, &id, "write").await?;
    let (rbac, user) = caller(&state, &ctx)?;
    let request = body(request)?;
    let name = request.name.as_deref().map(resource_name).transpose()?;
    let content_type = request
        .content_type
        .as_deref()
        .map(content_type)
        .transpose()?;
    let content = request
        .content
        .map(|c| content_bytes(c, request.encoding.as_deref()))
        .transpose()?;
    let updated = rbac
        .store
        .update_object(
            object.id,
            request.version,
            name,
            content_type,
            content,
            user,
        )
        .await
        .map_err(|e| match e {
            StoreError::Conflict(_) => ApiError::new(
                StatusCode::CONFLICT,
                "The object changed since that version; read it again",
            ),
            StoreError::NotFound => not_found(),
            e => store_failed(e),
        })?;
    Ok(Json(Value::Object(metadata(&updated))))
}

/// `DELETE /v1/objects/{id}`: `write`.
pub async fn delete(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let object = visible_object(&state, &ctx, &id, "write").await?;
    let (rbac, _) = caller(&state, &ctx)?;
    if !rbac
        .store
        .delete_object(object.id)
        .await
        .map_err(store_failed)?
    {
        return Err(not_found());
    }
    // A leftover grant names an id that is never reused: harmless.
    if let Err(e) = rbac
        .authz
        .delete_matching(&RelationshipFilter {
            kind: "object",
            id: Some(object.id.to_string()),
            relation: None,
            subject: None,
        })
        .await
    {
        tracing::warn!(
            "Cannot remove the grants of deleted object {}: {e}",
            object.id
        );
    }
    Ok(StatusCode::NO_CONTENT)
}
