//! Workspaces and their members: `/v1/organizations/{id}/workspaces` and
//! `/v1/workspaces`.
//!
//! A member holds exactly one of the roles `owner`, `editor` and `viewer`.
//! Any role also makes the user an `organization#member`, which is removed
//! when they hold no role in any workspace of that organization.

use std::collections::HashSet;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::organizations::{organization_ref, user_ref, visible_organization};
use super::{body, caller, existing_user, resource_id, resource_name, store_error};
use crate::app::AppState;
use crate::error::ApiError;
use crate::rbac::identity::validate_user_id;
use crate::rbac::store::Workspace;
use crate::rbac::{authz_error, ObjectRef, Rbac, Relationship, RelationshipFilter, Update};
use crate::services::RequestContext;

/// The workspace roles, i.e. the relations a member can hold.
pub const ROLES: [&str; 3] = ["owner", "editor", "viewer"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceName {
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetMember {
    role: String,
}

pub(crate) fn workspace_ref(id: Uuid) -> ObjectRef {
    ObjectRef::new("workspace", id.to_string())
}

fn view(ws: &Workspace) -> Value {
    json!({
        "id": ws.id,
        "organizationId": ws.organization_id,
        "name": ws.name,
        "createdAt": ws.created_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "createdBy": ws.created_by,
    })
}

fn not_found() -> ApiError {
    ApiError::not_found("workspace not found")
}

/// The workspace in the path, after `permission` is checked (404 when the
/// caller cannot view it, 403 when they lack `permission`).
pub(crate) async fn visible_workspace(
    rbac: &Rbac,
    id: &str,
    permission: &str,
    user: &str,
) -> Result<Workspace, ApiError> {
    let id = resource_id(id, "workspace")?;
    rbac.require_visible(&workspace_ref(id), "view", permission, user)
        .await?;
    rbac.store
        .workspace(id)
        .await
        .map_err(|e| store_error(e, "Conflict"))?
        .ok_or_else(not_found)
}

fn role_relationships(ws: Uuid, user: &str) -> Vec<Relationship> {
    ROLES
        .iter()
        .map(|role| Relationship {
            resource: workspace_ref(ws),
            relation: role,
            subject: user_ref(user),
        })
        .collect()
}

fn membership(org: Uuid, user: &str) -> Relationship {
    Relationship {
        resource: organization_ref(org),
        relation: "member",
        subject: user_ref(user),
    }
}

/// Removes `organization#member` from `user` unless they still hold a role
/// in a workspace of `org` other than `except`.
async fn drop_membership_if_roleless(
    rbac: &Rbac,
    org: Uuid,
    except: Uuid,
    user: &str,
) -> Result<(), ApiError> {
    let workspaces: HashSet<String> = rbac
        .store
        .workspaces_of_organization(org)
        .await
        .map_err(|e| store_error(e, "Conflict"))?
        .iter()
        .filter(|w| w.id != except)
        .map(|w| w.id.to_string())
        .collect();
    let roles = rbac
        .authz
        .read(&RelationshipFilter {
            kind: "workspace",
            id: None,
            relation: None,
            subject: Some(user_ref(user)),
        })
        .await
        .map_err(authz_error)?;
    let has_role = roles
        .iter()
        .any(|r| ROLES.contains(&r.relation) && workspaces.contains(&r.resource.id));
    if !has_role {
        rbac.authz
            .delete(&[membership(org, user)])
            .await
            .map_err(authz_error)?;
    }
    Ok(())
}

/// `{name}` → 201: `organization#admin`.
pub async fn create(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(org): Path<String>,
    request: Result<Json<WorkspaceName>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let org = visible_organization(rbac, &org, "admin", user).await?;
    let name = resource_name(&body(request)?.name)?;

    let conflict = "A workspace with this name already exists in the organization";
    let tx = rbac
        .store
        .transaction()
        .await
        .map_err(|e| store_error(e, conflict))?;
    let ws = tx
        .create_workspace(org.id, &name, user)
        .await
        .map_err(|e| store_error(e, conflict))?;
    rbac.authz
        .touch(&[Relationship {
            resource: workspace_ref(ws.id),
            relation: "organization",
            subject: organization_ref(org.id),
        }])
        .await
        // Dropping `tx` rolls the row back.
        .map_err(authz_error)?;
    tx.commit().await.map_err(|e| store_error(e, conflict))?;
    Ok((StatusCode::CREATED, Json(view(&ws))))
}

/// → `{workspaces: [...]}`: those of the organization the caller can view.
pub async fn list(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(org): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let org = visible_organization(rbac, &org, "view", user).await?;
    let ids: Vec<Uuid> = rbac
        .authz
        .lookup_resources("workspace", "view", user)
        .await
        .map_err(authz_error)?
        .iter()
        .filter_map(|id| id.parse().ok())
        .collect();
    let workspaces = rbac
        .store
        .workspaces_by_ids(&ids)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    let workspaces: Vec<Value> = workspaces
        .iter()
        .filter(|w| w.organization_id == org.id)
        .map(view)
        .collect();
    Ok(Json(json!({ "workspaces": workspaces })))
}

/// `view`.
pub async fn get(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let ws = visible_workspace(rbac, &id, "view", user).await?;
    Ok(Json(view(&ws)))
}

/// `{name}`: `manage`.
pub async fn update(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
    request: Result<Json<WorkspaceName>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let ws = visible_workspace(rbac, &id, "manage", user).await?;
    let name = resource_name(&body(request)?.name)?;
    let ws = rbac
        .store
        .update_workspace(ws.id, &name)
        .await
        .map_err(|e| {
            store_error(
                e,
                "A workspace with this name already exists in the organization",
            )
        })?
        .ok_or_else(not_found)?;
    Ok(Json(view(&ws)))
}

/// `organization#admin`. The row (which takes the objects with it) and the
/// relationships of the workspace, its objects and its connection releases
/// go together: the row delete is committed only after SpiceDB's.
pub async fn delete(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let ws = visible_workspace(rbac, &id, "view", user).await?;
    rbac.require(&organization_ref(ws.organization_id), "admin", user)
        .await?;

    let members = read_members(rbac, ws.id).await?;
    // The row (and its objects) go in a transaction committed only once
    // SpiceDB forgot the workspace; a SpiceDB failure rolls it back. The
    // workspace's own relationships go last, so a failure midway leaves it
    // reachable, and its delete can be retried.
    let tx = rbac
        .store
        .transaction()
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    tx.delete_workspace(ws.id)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    let subject = Some(workspace_ref(ws.id));
    let filters = [
        RelationshipFilter {
            kind: "object",
            id: None,
            relation: Some("workspace"),
            subject: subject.clone(),
        },
        RelationshipFilter {
            kind: "connection",
            id: None,
            relation: Some("workspace"),
            subject,
        },
        RelationshipFilter {
            kind: "workspace",
            id: Some(ws.id.to_string()),
            relation: None,
            subject: None,
        },
    ];
    for filter in &filters {
        rbac.authz
            .delete_matching(filter)
            .await
            // Dropping `tx` rolls the row delete back.
            .map_err(authz_error)?;
    }
    tx.commit().await.map_err(|e| store_error(e, "Conflict"))?;
    // Only after the commit: a member's organization membership is dropped
    // when this workspace gave their last role in it.
    for (member, _) in &members {
        drop_membership_if_roleless(rbac, ws.organization_id, ws.id, member).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `(user, role)` pairs, sorted by user.
async fn read_members(rbac: &Rbac, ws: Uuid) -> Result<Vec<(String, &'static str)>, ApiError> {
    let rels = rbac
        .authz
        .read(&RelationshipFilter {
            kind: "workspace",
            id: Some(ws.to_string()),
            relation: None,
            subject: None,
        })
        .await
        .map_err(authz_error)?;
    let mut members: Vec<(String, &'static str)> = rels
        .into_iter()
        .filter(|r| ROLES.contains(&r.relation) && r.subject.kind == "user")
        .map(|r| (r.subject.id, r.relation))
        .collect();
    members.sort();
    Ok(members)
}

/// → `{members: [{user, role}]}`: `view`.
pub async fn members(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let ws = visible_workspace(rbac, &id, "view", user).await?;
    let members: Vec<Value> = read_members(rbac, ws.id)
        .await?
        .into_iter()
        .map(|(user, role)| json!({ "user": user, "role": role }))
        .collect();
    Ok(Json(json!({ "members": members })))
}

/// `{role}`: `manage`. Replaces the user's previous role in the workspace.
pub async fn set_member(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path((id, member)): Path<(String, String)>,
    request: Result<Json<SetMember>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let ws = visible_workspace(rbac, &id, "manage", user).await?;
    let request = body(request)?;
    let role = ROLES
        .iter()
        .copied()
        .find(|r| *r == request.role)
        .ok_or_else(|| ApiError::bad_request("The role must be owner, editor or viewer"))?;
    let member = existing_user(rbac, &member).await?;

    // One atomic write, so concurrent changes cannot leave two roles.
    // SpiceDB refuses the same relationship twice in a request: delete only
    // the other roles.
    let mut updates: Vec<Update> = role_relationships(ws.id, &member.id)
        .into_iter()
        .filter(|r| r.relation != role)
        .map(Update::Delete)
        .collect();
    updates.push(Update::Touch(Relationship {
        resource: workspace_ref(ws.id),
        relation: role,
        subject: user_ref(&member.id),
    }));
    updates.push(Update::Touch(membership(ws.organization_id, &member.id)));
    rbac.authz.write(&updates, &[]).await.map_err(authz_error)?;
    Ok(Json(json!({ "user": member.id, "role": role })))
}

/// `manage`. Removes the user's role, and their organization membership when
/// it was their last role there.
pub async fn remove_member(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path((id, member)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let ws = visible_workspace(rbac, &id, "manage", user).await?;
    validate_user_id(&member)?;
    rbac.authz
        .delete(&role_relationships(ws.id, &member))
        .await
        .map_err(authz_error)?;
    drop_membership_if_roleless(rbac, ws.organization_id, ws.id, &member).await?;
    Ok(StatusCode::NO_CONTENT)
}
