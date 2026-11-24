//! User management, for platform admins: `/v1/users`.
//!
//! Responses carry `{id, name, email, disabled, createdAt}`, never the
//! password hash.

use std::collections::BTreeSet;

use axum::extract::rejection::JsonRejection;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use futures::future::try_join_all;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::organizations::user_ref;
use super::workspaces::workspace_ref;
use super::{body, rbac_of, store_error, SearchParams};
use crate::app::AppState;
use crate::error::ApiError;
use crate::handlers::data_sources::data_source_types;
use crate::rbac::access::{self, AccessInput};
use crate::rbac::identity::{hash_password, user_token_claims, validate_user_id};
use crate::rbac::restrictions::{validated_attributes, Attributes};
use crate::rbac::store::{NewUser, StoreError, User};
use crate::rbac::{authz_error, unauthenticated, Rbac, RelationshipFilter};
use crate::services::RequestContext;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateUser {
    id: String,
    name: Option<String>,
    email: Option<String>,
    password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateUser {
    name: Option<String>,
    email: Option<String>,
    disabled: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetPassword {
    password: String,
}

fn view(user: &User) -> Value {
    json!({
        "id": user.id,
        "name": user.name,
        "email": user.email,
        "disabled": user.disabled,
        "createdAt": user.created_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    })
}

/// 401 without an identity, 403 unless the caller is a platform admin.
fn require_admin<'a>(state: &'a AppState, ctx: &RequestContext) -> Result<&'a Rbac, ApiError> {
    let rbac = rbac_of(state)?;
    if ctx.user.is_none() {
        return Err(unauthenticated());
    }
    if !ctx.platform_admin {
        return Err(ApiError::forbidden("Only platform admins manage users"));
    }
    Ok(rbac)
}

/// `{id, name?, email?, password}` → 201 with the user.
pub async fn create(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    request: Result<Json<CreateUser>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let rbac = require_admin(&state, &ctx)?;
    let request = body(request)?;
    validate_user_id(&request.id)?;

    let password = request.password;
    let password_hash = rbac.run_hashing(move || hash_password(&password)).await??;
    let user = rbac
        .store
        .create_user(&NewUser {
            id: request.id,
            name: request.name,
            email: request.email,
            password_hash,
        })
        .await
        .map_err(|e| store_error(e, "A user with this id already exists"))?;
    Ok((StatusCode::CREATED, Json(view(&user))))
}

/// → `{users: [...]}`, plus `total` when `q`, `limit` or `offset` came.
pub(crate) async fn list(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Query(params): Query<SearchParams>,
) -> Result<Json<Value>, ApiError> {
    let rbac = require_admin(&state, &ctx)?;
    let search = params.search()?;
    let page = rbac
        .store
        .search_users(&search)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    let users: Vec<Value> = page.items.iter().map(view).collect();
    if params.has_paging() {
        Ok(Json(json!({ "users": users, "total": page.total })))
    } else {
        Ok(Json(json!({ "users": users })))
    }
}

/// `{name?, email?, disabled?}`: disabling locks the user out at once.
pub async fn update(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
    request: Result<Json<UpdateUser>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let rbac = require_admin(&state, &ctx)?;
    let request = body(request)?;
    validate_user_id(&id)?;
    if request.name.is_none() && request.email.is_none() && request.disabled.is_none() {
        return Err(ApiError::bad_request("Nothing to update"));
    }
    // Disabling the only admin would lock user management out for good.
    if request.disabled == Some(true) && ctx.user.as_deref() == Some(id.as_str()) {
        return Err(ApiError::bad_request("You cannot disable your own account"));
    }
    let user = rbac
        .store
        .update_user(&id, request.name, request.email, request.disabled)
        .await
        .map_err(|e| store_error(e, "This email is already in use"))?
        .ok_or_else(user_not_found)?;
    Ok(Json(view(&user)))
}

/// `{password}`: an admin reset. Every token the user holds stops working.
pub async fn set_password(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
    request: Result<Json<SetPassword>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let rbac = require_admin(&state, &ctx)?;
    let request = body(request)?;
    validate_user_id(&id)?;

    let password = request.password;
    let hash = rbac.run_hashing(move || hash_password(&password)).await??;
    rbac.store
        .set_password_hash(&id, &hash)
        .await
        .map_err(|e| match e {
            StoreError::NotFound => user_not_found(),
            e => store_error(e, "Conflict"),
        })?;
    Ok(StatusCode::NO_CONTENT)
}

/// A service token for the user, issued like a login token. A disabled user
/// gets 409: the token could not authenticate anyway.
pub async fn issue_token(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let rbac = require_admin(&state, &ctx)?;
    validate_user_id(&id)?;
    let user = rbac
        .store
        .user(&id)
        .await
        .map_err(|e| store_error(e, "Conflict"))?
        .ok_or_else(user_not_found)?;
    if user.disabled {
        return Err(ApiError::new(StatusCode::CONFLICT, "The user is disabled"));
    }
    let (claims, expires_at) = user_token_claims(&user.id, rbac.config.token_ttl);
    let token = state.auth.issue_token(claims).await?;
    Ok(Json(json!({
        "token": token,
        "expiresAt": expires_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    })))
}

/// The user in the path, or 404.
async fn known_user(rbac: &Rbac, id: &str) -> Result<(), ApiError> {
    validate_user_id(id)?;
    rbac.store
        .user(id)
        .await
        .map_err(|e| store_error(e, "Conflict"))?
        .map(|_| ())
        .ok_or_else(user_not_found)
}

/// → `{attributes: {name: [values]}}`. Platform admins only.
pub(crate) async fn attributes(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let rbac = require_admin(&state, &ctx)?;
    known_user(rbac, &id).await?;
    let attributes = rbac
        .store
        .user_attributes(&id)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    Ok(Json(json!({ "attributes": attributes })))
}

/// `{attributes: {name: [values]}}` replaces the whole set → the saved set.
/// Platform admins only.
pub(crate) async fn set_attributes(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
    request: Result<Json<AttributesBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let rbac = require_admin(&state, &ctx)?;
    known_user(rbac, &id).await?;
    let attributes = validated_attributes(body(request)?.attributes)?;
    rbac.store
        .set_user_attributes(&id, &attributes)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    Ok(Json(json!({ "attributes": attributes })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AttributesBody {
    attributes: Attributes,
}

/// → the user's access graph (`rbac::access::build`). Platform admins only.
pub(crate) async fn access(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<access::AccessGraph>, ApiError> {
    let rbac = require_admin(&state, &ctx)?;
    validate_user_id(&id)?;
    let user = rbac
        .store
        .user(&id)
        .await
        .map_err(|e| store_error(e, "Conflict"))?
        .ok_or_else(user_not_found)?;
    let platform_admin = rbac.is_platform_admin(&id).await?;
    let mut input = AccessInput {
        user,
        platform_admin,
        direct: vec![],
        organizations: vec![],
        workspaces: vec![],
        rules: vec![],
        roles: vec![],
        connections: vec![],
        releases: vec![],
        configured: BTreeSet::new(),
        platform_sources: vec![],
        cubes: vec![],
    };
    if platform_admin {
        return Ok(Json(access::build(&input)));
    }

    let mut direct = Vec::new();
    for kind in ["organization", "workspace"] {
        direct.extend(
            rbac.authz
                .read(&RelationshipFilter {
                    kind,
                    id: None,
                    relation: None,
                    subject: Some(user_ref(&id)),
                })
                .await
                .map_err(authz_error)?,
        );
    }
    let ids_of = |kind: &str| -> Vec<Uuid> {
        direct
            .iter()
            .filter(|r| r.resource.kind == kind)
            .filter_map(|r| r.resource.id.parse().ok())
            .collect()
    };
    // The rules the user is a member of give a role in their workspace.
    let rule_ids: Vec<Uuid> = rbac
        .authz
        .read(&RelationshipFilter {
            kind: "rule",
            id: None,
            relation: Some("member"),
            subject: Some(user_ref(&id)),
        })
        .await
        .map_err(authz_error)?
        .iter()
        .filter_map(|r| r.resource.id.parse().ok())
        .collect();
    let rules = rbac
        .store
        .rules_by_ids(&rule_ids)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    let role_ids: Vec<Uuid> = rules.iter().filter_map(|r| r.role_id).collect();
    let roles = rbac
        .store
        .roles_by_ids(&role_ids)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    let mut workspace_ids = ids_of("workspace");
    workspace_ids.extend(rules.iter().map(|r| r.workspace_id));
    let mut organization_ids = ids_of("organization");
    let owned: Vec<Uuid> = direct
        .iter()
        .filter(|r| r.resource.kind == "organization" && r.relation == "owner")
        .filter_map(|r| r.resource.id.parse().ok())
        .collect();

    // Every workspace of an organization the user owns, whatever its roles.
    let owned_workspaces = try_join_all(
        owned
            .iter()
            .map(|org| rbac.store.workspaces_of_organization(*org)),
    )
    .await
    .map_err(|e| store_error(e, "Conflict"))?;
    workspace_ids.extend(owned_workspaces.into_iter().flatten().map(|ws| ws.id));
    workspace_ids.sort();
    workspace_ids.dedup();
    let workspaces = rbac
        .store
        .workspaces_by_ids(&workspace_ids)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    organization_ids.extend(workspaces.iter().map(|w| w.organization_id));
    organization_ids.sort();
    organization_ids.dedup();
    let organizations = rbac
        .store
        .organizations_by_ids(&organization_ids)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;

    // One read per organization and per workspace, all at once.
    let connections: Vec<_> = try_join_all(
        organizations
            .iter()
            .map(|org| rbac.store.connections_of_organization(org.id)),
    )
    .await
    .map_err(|e| store_error(e, "Conflict"))?
    .into_iter()
    .flatten()
    .collect();
    let release_filters: Vec<RelationshipFilter> = workspaces
        .iter()
        .map(|ws| RelationshipFilter {
            kind: "connection",
            id: None,
            relation: Some("workspace"),
            subject: Some(workspace_ref(ws.id)),
        })
        .collect();
    let releases: Vec<(String, Uuid)> =
        try_join_all(release_filters.iter().map(|f| rbac.authz.read(f)))
            .await
            .map_err(authz_error)?
            .into_iter()
            .zip(&workspaces)
            .flat_map(|(rels, ws)| rels.into_iter().map(move |r| (r.resource.id, ws.id)))
            .collect();

    input.direct = direct;
    input.organizations = organizations;
    input.workspaces = workspaces;
    input.rules = rules;
    input.roles = roles;
    input.connections = connections;
    input.releases = releases;
    input.configured = data_source_types(&state).await?.into_keys().collect();

    // Configured data sources that are no organization's connection.
    let rows: BTreeSet<String> = rbac
        .store
        .all_connections()
        .await
        .map_err(|e| store_error(e, "Conflict"))?
        .into_iter()
        .map(|c| c.name)
        .collect();
    input.platform_sources = input
        .configured
        .iter()
        .filter(|name| !rows.contains(*name))
        .cloned()
        .collect();
    input.cubes = access_cubes(&*crate::handlers::tree::index(&state).await?);
    Ok(Json(access::build(&input)))
}

/// The cubes and views of the model index, each with its folder and the data
/// sources it reads. Files outside every folder are left out.
///
/// A cube reads its own data source. A view reads, as enforcement does
/// (`member_to_data_source`: the data source of the cube owning each included
/// member), the data sources of its member cubes, the last segment of each
/// `join_path`; the cubes a path only passes through add nothing. They are
/// looked up in the view's folder, then in Platform's. Unlike enforcement,
/// which maps member by member, the index lists the cubes per `join_path`, so
/// a path whose include brings no member still counts; when a member cube is
/// not found the view's own `data_source` stands in.
fn access_cubes(index: &crate::model_index::ModelIndex) -> Vec<access::AccessCube> {
    use cubemodel::ownership::{Owner, PLATFORM_FOLDER};
    let folder_of = |owner: &Owner| match owner {
        Owner::Platform => Some(PLATFORM_FOLDER.to_string()),
        Owner::Organization(slug) => Some(slug.clone()),
        Owner::Unowned(_) => None,
    };
    let owned: Vec<(String, &crate::model_index::IndexedCube)> = index
        .cubes
        .iter()
        .filter_map(|c| folder_of(&c.owner).map(|f| (f, c)))
        .collect();
    // Names are unique per folder, not across them: a view's member cubes are
    // looked up in its own folder, then in Platform's.
    let find = |folder: &str, name: &str| {
        [folder, PLATFORM_FOLDER].into_iter().find_map(|f| {
            owned
                .iter()
                .find(|(of, c)| of == f && c.kind == "cube" && c.name == name)
                .map(|(_, c)| c)
        })
    };
    owned
        .iter()
        .map(|(folder, cube)| {
            let own = BTreeSet::from([cube.data_source.clone()]);
            let data_sources = if cube.kind == "view" && !cube.member_cubes.is_empty() {
                let found: Option<BTreeSet<String>> = cube
                    .member_cubes
                    .iter()
                    .map(|name| find(folder, name).map(|c| c.data_source.clone()))
                    .collect();
                found.unwrap_or(own)
            } else {
                own
            };
            access::AccessCube {
                name: cube.name.clone(),
                view: cube.kind == "view",
                folder: folder.clone(),
                data_sources,
            }
        })
        .collect()
}

fn user_not_found() -> ApiError {
    ApiError::not_found("User not found")
}
