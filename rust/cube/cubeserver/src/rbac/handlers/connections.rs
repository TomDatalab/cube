//! Organization connections and their release to workspaces:
//! `/v1/organizations/{id}/connections`, `/v1/connections/{name}` and
//! `/v1/workspaces/{id}/connections`.
//!
//! A connection is a data source of `cube.yml` that belongs to one
//! organization (a `connections` row and `connection#organization`). The
//! organization's admins edit it with the data-source handlers and release
//! it to the organization's workspaces (`connection#workspace`). A data
//! source without an organization is for platform admins only.
//!
//! A connection the caller can neither query nor manage answers 404.

use std::collections::BTreeSet;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::organizations::{organization_ref, visible_organization};
use super::workspaces::{visible_workspace, workspace_ref};
use super::{body, caller, resource_id, store_error};
use crate::app::AppState;
use crate::error::ApiError;
use crate::handlers::data_sources::{self, data_source_types, CreateRequest, UpdateRequest};
use crate::rbac::store::{ConnectionRow, StoreError, Workspace};
use crate::rbac::{
    authz_error, unavailable, ObjectRef, Precondition, Rbac, Relationship, RelationshipFilter,
    Update,
};
use crate::services::RequestContext;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssignOrganization {
    #[serde(rename = "organizationId")]
    organization_id: String,
}

fn connection_ref(name: &str) -> ObjectRef {
    ObjectRef::new("connection", name)
}

fn organization_relationship(name: &str, org: Uuid) -> Relationship {
    Relationship {
        resource: connection_ref(name),
        relation: "organization",
        subject: organization_ref(org),
    }
}

fn release_relationship(name: &str, ws: Uuid) -> Relationship {
    Relationship {
        resource: connection_ref(name),
        relation: "workspace",
        subject: workspace_ref(ws),
    }
}

fn not_found() -> ApiError {
    ApiError::not_found("connection not found")
}

/// A connection name from the path. One SpiceDB could not hold as an object
/// id cannot exist: 404.
fn connection_name(name: &str) -> Result<(), ApiError> {
    let valid = !name.is_empty()
        && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if valid {
        Ok(())
    } else {
        Err(not_found())
    }
}

/// 503 for any RBAC database failure: these reads only grant access.
fn read_error(e: StoreError) -> ApiError {
    tracing::error!("RBAC store failed: {e}");
    unavailable()
}

/// The names of the connections released to `workspace`: the
/// `connection:<name>#workspace@workspace:<id>` relationships whose
/// connection belongs to the workspace's organization (a release from
/// another organization's connection never counts). Empty for an unknown
/// workspace; 503 when SpiceDB or the RBAC database cannot answer.
pub async fn released_connections(
    rbac: &Rbac,
    workspace: &str,
) -> Result<BTreeSet<String>, ApiError> {
    let Ok(id) = workspace.parse::<Uuid>() else {
        return Ok(BTreeSet::new());
    };
    let Some(ws) = rbac.store.workspace(id).await.map_err(read_error)? else {
        return Ok(BTreeSet::new());
    };
    let releases = rbac
        .authz
        .read(&RelationshipFilter {
            kind: "connection",
            id: None,
            relation: Some("workspace"),
            subject: Some(workspace_ref(id)),
        })
        .await
        .map_err(authz_error)?;
    let names: Vec<String> = releases.into_iter().map(|r| r.resource.id).collect();
    let rows = rbac
        .store
        .connections_by_names(&names)
        .await
        .map_err(read_error)?;
    Ok(rows
        .into_iter()
        .filter(|c| c.organization_id == ws.organization_id)
        .map(|c| c.name)
        .collect())
}

/// The workspaces `name` is released to, as relationships.
async fn releases_of(rbac: &Rbac, name: &str) -> Result<Vec<Relationship>, ApiError> {
    rbac.authz
        .read(&RelationshipFilter {
            kind: "connection",
            id: Some(name.to_string()),
            relation: Some("workspace"),
            subject: None,
        })
        .await
        .map_err(authz_error)
}

/// The connection in the path, for `connection#manage`: 404 when the caller
/// can neither query nor manage it, 403 when they can only query it.
/// Platform admins pass for any name; the row is `None` for a data source
/// without an organization (or none at all).
async fn managed_connection(
    rbac: &Rbac,
    ctx: &RequestContext,
    user: &str,
    name: &str,
) -> Result<Option<ConnectionRow>, ApiError> {
    connection_name(name)?;
    if !ctx.platform_admin {
        rbac.require_visible(&connection_ref(name), "query", "manage", user)
            .await?;
    }
    let row = rbac
        .store
        .connection(name)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    if row.is_none() && !ctx.platform_admin {
        return Err(not_found());
    }
    Ok(row)
}

/// → `{connections: [{name, type, workspaces}]}`: `admin` on the
/// organization. `workspaces` are the ids it is released to.
pub async fn list(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let org = visible_organization(rbac, &id, "admin", user).await?;
    let rows = rbac
        .store
        .connections_of_organization(org.id)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    let types = data_source_types(&state);
    let mut connections = Vec::with_capacity(rows.len());
    for row in &rows {
        let mut workspaces: Vec<String> = releases_of(rbac, &row.name)
            .await?
            .into_iter()
            .map(|r| r.subject.id)
            .collect();
        workspaces.sort();
        connections.push(json!({
            "name": row.name,
            "type": types.get(&row.name),
            "workspaces": workspaces,
        }));
    }
    Ok(Json(json!({ "connections": connections })))
}

/// The body of `POST /v1/data-sources` → 201: `manage_connections` on the
/// organization. The row and `connection#organization` are written first,
/// so the data source never exists without its organization; both are
/// undone when it cannot be added.
pub async fn create(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
    request: Result<Json<CreateRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let org = visible_organization(rbac, &id, "manage_connections", user).await?;
    let request = body(request)?;
    let name = request.name.trim().to_string();
    data_sources::validate_name(&name)?;
    // A data source without an organization is not taken over.
    if data_source_types(&state).contains_key(&name) {
        return Err(ApiError::bad_request(format!(
            "A data source named {name} already exists"
        )));
    }

    let conflict = "A connection with this name already exists";
    let tx = rbac
        .store
        .transaction()
        .await
        .map_err(|e| store_error(e, conflict))?;
    tx.create_connection(&name, org.id)
        .await
        .map_err(|e| store_error(e, conflict))?;
    let relationship = organization_relationship(&name, org.id);
    rbac.authz
        .touch(std::slice::from_ref(&relationship))
        .await
        // Dropping `tx` rolls the row back.
        .map_err(authz_error)?;
    let undo = || async {
        if let Err(e) = rbac.authz.delete(std::slice::from_ref(&relationship)).await {
            tracing::error!(connection = %name, "Cannot undo connection#organization: {e}");
        }
    };
    let response = match data_sources::create(State(state.clone()), Json(request)).await {
        Ok(response) => response.into_response(),
        Err(e) => {
            undo().await;
            return Err(e);
        }
    };
    if let Err(e) = tx.commit().await {
        // The data source stays, without an organization: platform admins
        // can assign it.
        undo().await;
        return Err(store_error(e, conflict));
    }
    Ok((StatusCode::CREATED, response).into_response())
}

/// The body of `PUT /v1/data-sources/{name}`: `connection#manage`.
pub async fn update(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(name): Path<String>,
    request: Result<Json<UpdateRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    managed_connection(rbac, &ctx, user, &name).await?;
    let request = body(request)?;
    data_sources::update(State(state.clone()), Path(name), Json(request))
        .await
        .map(IntoResponse::into_response)
}

/// `connection#manage`: [`delete_with_grants`].
pub async fn delete(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(name): Path<String>,
) -> Result<Response, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    managed_connection(rbac, &ctx, user, &name).await?;
    delete_with_grants(&state, rbac, &name).await
}

/// Deletes data source `name` with everything RBAC holds about it, so a
/// later data source of the same name inherits no grant: its relationships
/// first (restored if the data source cannot be deleted), then the data
/// source, then its `connections` row. Also behind `DELETE /v1/data-sources`.
pub(crate) async fn delete_with_grants(
    state: &AppState,
    rbac: &Rbac,
    name: &str,
) -> Result<Response, ApiError> {
    // A name SpiceDB could not hold never got a grant or a row.
    let tracked = connection_name(name).is_ok();
    let mut existing = Vec::new();
    if tracked {
        let filter = RelationshipFilter {
            kind: "connection",
            id: Some(name.to_string()),
            relation: None,
            subject: None,
        };
        existing = rbac.authz.read(&filter).await.map_err(authz_error)?;
        rbac.authz
            .delete_matching(&filter)
            .await
            .map_err(authz_error)?;
    }
    let response = match data_sources::delete(State(state.clone()), Path(name.to_string())).await {
        Ok(response) => response.into_response(),
        Err(e) => {
            if !existing.is_empty() {
                if let Err(err) = rbac.authz.touch(&existing).await {
                    tracing::error!(connection = %name, "Cannot restore the connection's relationships: {err}");
                }
            }
            return Err(e);
        }
    };
    if tracked {
        rbac.store
            .delete_connection(name)
            .await
            .map_err(|e| store_error(e, "Conflict"))?;
    }
    Ok(response)
}

/// 409 when `name` (as `POST /v1/data-sources` trims it) still has a
/// `connections` row: a new data source never adopts an old one's
/// organization and releases.
pub(crate) async fn refuse_tracked_name(rbac: &Rbac, name: &str) -> Result<(), ApiError> {
    let row = rbac
        .store
        .connection(name.trim())
        .await
        .map_err(read_error)?;
    if row.is_some() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "A connection with this name still belongs to an organization",
        ));
    }
    Ok(())
}

/// `{organizationId}`: platform admins. Assigns a data source of
/// `cube.yml` (`default`, say) to an organization. Moving it to another
/// organization withdraws its releases to the old one's workspaces.
pub async fn assign_organization(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(name): Path<String>,
    request: Result<Json<AssignOrganization>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let (rbac, _) = caller(&state, &ctx)?;
    if !ctx.platform_admin {
        return Err(ApiError::forbidden(
            "Only platform admins assign connections to organizations",
        ));
    }
    connection_name(&name)?;
    let request = body(request)?;
    let org_id = resource_id(&request.organization_id, "organization")?;
    let org = rbac
        .store
        .organization(org_id)
        .await
        .map_err(|e| store_error(e, "Conflict"))?
        .ok_or_else(|| ApiError::not_found("organization not found"))?;
    if !data_source_types(&state).contains_key(&name) {
        return Err(not_found());
    }

    let row = rbac
        .store
        .connection(&name)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    let mut updates = Vec::new();
    if let Some(row) = row.as_ref().filter(|r| r.organization_id != org.id) {
        updates.push(Update::Delete(organization_relationship(
            &name,
            row.organization_id,
        )));
        updates.extend(
            releases_of(rbac, &name)
                .await?
                .into_iter()
                .map(Update::Delete),
        );
    }
    updates.push(Update::Touch(organization_relationship(&name, org.id)));

    let conflict = "The connection changed concurrently; try again";
    match row {
        None => {
            let tx = rbac
                .store
                .transaction()
                .await
                .map_err(|e| store_error(e, conflict))?;
            tx.create_connection(&name, org.id)
                .await
                .map_err(|e| store_error(e, conflict))?;
            rbac.authz.write(&updates, &[]).await.map_err(authz_error)?;
            tx.commit().await.map_err(|e| store_error(e, conflict))?;
        }
        Some(_) => {
            rbac.authz.write(&updates, &[]).await.map_err(authz_error)?;
            rbac.store
                .update_connection(&name, org.id)
                .await
                .map_err(|e| store_error(e, conflict))?
                .ok_or_else(not_found)?;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

/// The workspace and the connection of a release, after the checks:
/// 404 when the caller cannot view the workspace, 403 unless they administer
/// its organization, 404 when the connection is not of that organization.
async fn release_target(
    rbac: &Rbac,
    user: &str,
    id: &str,
    name: &str,
) -> Result<Workspace, ApiError> {
    let ws = visible_workspace(rbac, id, "view", user).await?;
    rbac.require(&organization_ref(ws.organization_id), "admin", user)
        .await?;
    connection_name(name)?;
    rbac.store
        .connection(name)
        .await
        .map_err(|e| store_error(e, "Conflict"))?
        .filter(|c| c.organization_id == ws.organization_id)
        .ok_or_else(not_found)?;
    Ok(ws)
}

/// `organization#admin`: releases the connection to the workspace.
pub async fn release(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path((id, name)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let ws = release_target(rbac, user, &id, &name).await?;
    // Only while the connection is still the organization's.
    rbac.authz
        .write(
            &[Update::Touch(release_relationship(&name, ws.id))],
            &[Precondition::MustMatch(RelationshipFilter {
                kind: "connection",
                id: Some(name.clone()),
                relation: Some("organization"),
                subject: Some(organization_ref(ws.organization_id)),
            })],
        )
        .await
        .map_err(authz_error)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `organization#admin`: withdraws the release.
pub async fn withdraw(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path((id, name)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let ws = release_target(rbac, user, &id, &name).await?;
    rbac.authz
        .delete(&[release_relationship(&name, ws.id)])
        .await
        .map_err(authz_error)?;
    Ok(StatusCode::NO_CONTENT)
}

/// → `{connections: [{name, type}]}`: `view` on the workspace. The
/// connections released to it, without their settings.
pub async fn of_workspace(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let ws = visible_workspace(rbac, &id, "view", user).await?;
    let types = data_source_types(&state);
    let connections: Vec<Value> = released_connections(rbac, &ws.id.to_string())
        .await?
        .into_iter()
        .map(|name| json!({ "type": types.get(&name), "name": name }))
        .collect();
    Ok(Json(json!({ "connections": connections })))
}
