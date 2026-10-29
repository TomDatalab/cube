//! Organizations and their owners: `/v1/organizations`.
//!
//! An organization the caller cannot `view` answers 404, as if it did not
//! exist; one they can see but not administer answers 403.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{body, caller, existing_user, resource_id, resource_name, store_error};
use crate::app::AppState;
use crate::error::ApiError;
use crate::rbac::identity::validate_user_id;
use crate::rbac::store::{Organization, StoreError};
use crate::rbac::{
    authz_error, platform, ObjectRef, Precondition, Rbac, Relationship, RelationshipFilter, Update,
};
use crate::services::RequestContext;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateOrganization {
    name: String,
    owner: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateOrganization {
    name: String,
}

pub(crate) fn organization_ref(id: Uuid) -> ObjectRef {
    ObjectRef::new("organization", id.to_string())
}

pub(crate) fn user_ref(id: &str) -> ObjectRef {
    ObjectRef::new("user", id)
}

fn view(org: &Organization) -> Value {
    json!({
        "id": org.id,
        "name": org.name,
        "createdAt": org.created_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "createdBy": org.created_by,
    })
}

fn not_found() -> ApiError {
    ApiError::not_found("organization not found")
}

/// The organization in the path, after `permission` is checked (404 when the
/// caller cannot view it, 403 when they lack `permission`).
pub(crate) async fn visible_organization(
    rbac: &Rbac,
    id: &str,
    permission: &str,
    user: &str,
) -> Result<Organization, ApiError> {
    let id = resource_id(id, "organization")?;
    rbac.require_visible(&organization_ref(id), "view", permission, user)
        .await?;
    rbac.store
        .organization(id)
        .await
        .map_err(|e| store_error(e, "Conflict"))?
        .ok_or_else(not_found)
}

/// `{name, owner}` → 201. Platform admins only; the owner must be an
/// existing, enabled user.
pub async fn create(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    request: Result<Json<CreateOrganization>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    if !ctx.platform_admin {
        return Err(ApiError::forbidden(
            "Only platform admins create organizations",
        ));
    }
    let request = body(request)?;
    let name = resource_name(&request.name)?;
    validate_user_id(&request.owner)?;
    let owner = rbac
        .store
        .user(&request.owner)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    if !owner.is_some_and(|o| !o.disabled) {
        return Err(ApiError::bad_request(
            "The owner must be an existing, enabled user",
        ));
    }

    let tx = rbac
        .store
        .transaction()
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    let org = tx
        .create_organization(&name, user)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    let resource = organization_ref(org.id);
    rbac.authz
        .touch(&[
            Relationship {
                resource: resource.clone(),
                relation: "platform",
                subject: platform(),
            },
            Relationship {
                resource,
                relation: "owner",
                subject: user_ref(&request.owner),
            },
        ])
        .await
        // Dropping `tx` rolls the row back.
        .map_err(authz_error)?;
    tx.commit().await.map_err(|e| store_error(e, "Conflict"))?;
    Ok((StatusCode::CREATED, Json(view(&org))))
}

/// → `{organizations: [...]}`: those the caller can view.
pub async fn list(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let ids: Vec<Uuid> = rbac
        .authz
        .lookup_resources("organization", "view", user)
        .await
        .map_err(authz_error)?
        .iter()
        .filter_map(|id| id.parse().ok())
        .collect();
    let orgs = rbac
        .store
        .organizations_by_ids(&ids)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    let orgs: Vec<Value> = orgs.iter().map(view).collect();
    Ok(Json(json!({ "organizations": orgs })))
}

/// `view`.
pub async fn get(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let org = visible_organization(rbac, &id, "view", user).await?;
    Ok(Json(view(&org)))
}

/// `{name}`: `admin`.
pub async fn update(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
    request: Result<Json<UpdateOrganization>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let org = visible_organization(rbac, &id, "admin", user).await?;
    let name = resource_name(&body(request)?.name)?;
    let org = rbac
        .store
        .update_organization(org.id, &name)
        .await
        .map_err(|e| store_error(e, "Conflict"))?
        .ok_or_else(not_found)?;
    Ok(Json(view(&org)))
}

/// `admin`. Refused (400) while the organization has workspaces or
/// connections. The row and its relationships go together: the row delete
/// is committed only after SpiceDB's.
pub async fn delete(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let org = visible_organization(rbac, &id, "admin", user).await?;
    let in_use =
        || ApiError::bad_request("Delete the organization's workspaces and connections first");
    let workspaces = rbac
        .store
        .workspaces_of_organization(org.id)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    let connections = rbac
        .store
        .connections_of_organization(org.id)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    if !workspaces.is_empty() || !connections.is_empty() {
        return Err(in_use());
    }

    // The row goes in a transaction committed only once SpiceDB forgot the
    // organization: a failure leaves both or neither.
    let tx = rbac
        .store
        .transaction()
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    match tx.delete_organization(org.id).await {
        Ok(_) => {}
        // A workspace or connection was added meanwhile.
        Err(StoreError::Conflict(_)) => return Err(in_use()),
        Err(e) => return Err(store_error(e, "Conflict")),
    }
    rbac.authz
        .delete_matching(&RelationshipFilter {
            kind: "organization",
            id: Some(org.id.to_string()),
            relation: None,
            subject: None,
        })
        .await
        // Dropping `tx` rolls the row delete back.
        .map_err(authz_error)?;
    tx.commit().await.map_err(|e| store_error(e, "Conflict"))?;
    Ok(StatusCode::NO_CONTENT)
}

/// `admin`: makes an existing user an owner.
pub async fn add_owner(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path((id, owner)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let org = visible_organization(rbac, &id, "admin", user).await?;
    let owner = existing_user(rbac, &owner).await?;
    rbac.authz
        .touch(&[owner_relationship(org.id, &owner.id)])
        .await
        .map_err(authz_error)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `admin`. The last owner cannot be removed (400); a user who is not an
/// owner answers 404.
pub async fn remove_owner(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path((id, owner)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let org = visible_organization(rbac, &id, "admin", user).await?;
    validate_user_id(&owner)?;
    let owners = rbac
        .authz
        .read(&RelationshipFilter {
            kind: "organization",
            id: Some(org.id.to_string()),
            relation: Some("owner"),
            subject: None,
        })
        .await
        .map_err(authz_error)?;
    let owners: Vec<&str> = owners
        .iter()
        .filter(|r| r.subject.kind == "user")
        .map(|r| r.subject.id.as_str())
        .collect();
    if !owners.contains(&owner.as_str()) {
        return Err(ApiError::not_found(format!(
            "{owner} is not an owner of this organization"
        )));
    }
    // Remove only while another owner is still there: two concurrent
    // removals cannot leave the organization without owners.
    let Some(other) = owners.iter().find(|o| **o != owner.as_str()) else {
        return Err(ApiError::bad_request(
            "The last owner of an organization cannot be removed",
        ));
    };
    rbac.authz
        .write(
            &[Update::Delete(owner_relationship(org.id, &owner))],
            &[Precondition::MustMatch(RelationshipFilter {
                kind: "organization",
                id: Some(org.id.to_string()),
                relation: Some("owner"),
                subject: Some(user_ref(other)),
            })],
        )
        .await
        .map_err(authz_error)?;
    Ok(StatusCode::NO_CONTENT)
}

fn owner_relationship(org: Uuid, user: &str) -> Relationship {
    Relationship {
        resource: organization_ref(org),
        relation: "owner",
        subject: user_ref(user),
    }
}
