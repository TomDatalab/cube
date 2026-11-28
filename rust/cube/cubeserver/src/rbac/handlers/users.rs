//! User management, for platform admins: `/v1/users`.
//!
//! Responses carry `{id, username, name, email, disabled, createdAt}`
//! (`id` the UUID), never the password hash. A user in the path is named by
//! UUID or by username.

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
use super::{body, named_user, rbac_of, store_error, user_not_found, SearchParams};
use crate::app::AppState;
use crate::error::ApiError;
use crate::handlers::data_sources::data_source_types;
use crate::rbac::access::{self, AccessInput};
use crate::rbac::identity::{hash_password, user_token_claims, validate_new_username};
use crate::rbac::restrictions::{validated_attributes, Attributes};
use crate::rbac::store::{NewUser, StoreError, User};
use crate::rbac::{authz_error, unauthenticated, Rbac, RelationshipFilter};
use crate::services::RequestContext;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateUser {
    username: Option<String>,
    /// The username, as the field was called before users had a UUID.
    id: Option<String>,
    name: Option<String>,
    email: Option<String>,
    password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateUser {
    username: Option<String>,
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
        "username": user.username,
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

/// `{username, name?, email?, password}` → 201 with the user. `id` is
/// accepted in place of `username` (its old name).
pub async fn create(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    request: Result<Json<CreateUser>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let rbac = require_admin(&state, &ctx)?;
    let request = body(request)?;
    let username = match (request.username, request.id) {
        (Some(u), Some(id)) if u != id => {
            return Err(ApiError::bad_request("Give username or id, not both"))
        }
        (Some(u), _) | (None, Some(u)) => u,
        (None, None) => return Err(ApiError::bad_request("The username is required")),
    };
    validate_new_username(&username)?;

    let password = request.password;
    let password_hash = rbac.run_hashing(move || hash_password(&password)).await??;
    let user = rbac
        .store
        .create_user(&NewUser {
            username,
            name: request.name,
            email: request.email,
            password_hash,
        })
        .await
        .map_err(|e| store_error(e, "A user with this username already exists"))?;
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

/// `{username?, name?, email?, disabled?}`: disabling locks the user out at
/// once. A rename keeps the user's id, and so their access and their tokens;
/// a username taken by another user is 409.
pub async fn update(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
    request: Result<Json<UpdateUser>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let rbac = require_admin(&state, &ctx)?;
    let request = body(request)?;
    let user = named_user(rbac, &id).await?;
    if request.username.is_none()
        && request.name.is_none()
        && request.email.is_none()
        && request.disabled.is_none()
    {
        return Err(ApiError::bad_request("Nothing to update"));
    }
    if let Some(username) = &request.username {
        validate_new_username(username)?;
        if *username != user.username {
            refuse_configured_rename(rbac, &user.username)?;
        }
    }
    // Disabling the only admin would lock user management out for good. The
    // path may name the caller by username: compare ids.
    if request.disabled == Some(true) && ctx.user.as_deref() == Some(user.id.as_str()) {
        return Err(ApiError::bad_request("You cannot disable your own account"));
    }
    let user = rbac
        .store
        .update_user(
            &user.id,
            request.username,
            request.name,
            request.email,
            request.disabled,
        )
        .await
        .map_err(|e| match e {
            StoreError::Conflict(c) if c.contains("username") => ApiError::new(
                StatusCode::CONFLICT,
                "A user with this username already exists",
            ),
            e => store_error(e, "This email is already in use"),
        })?
        .ok_or_else(user_not_found)?;
    Ok(Json(view(&user)))
}

/// `CUBE_RBAC_ADMINS` and `CUBE_SYNC_USER` name users by username: renamed,
/// the next start would create a new admin under the old name, and the sync
/// routes would refuse the renamed user (and serve whoever takes the old
/// name). 409 for those users.
fn refuse_configured_rename(rbac: &Rbac, username: &str) -> Result<(), ApiError> {
    let variable = if rbac.config.admins.iter().any(|a| a == username) {
        "CUBE_RBAC_ADMINS"
    } else if rbac.config.sync_user == username {
        "CUBE_SYNC_USER"
    } else {
        return Ok(());
    };
    Err(ApiError::new(
        StatusCode::CONFLICT,
        format!(
            "{username} is listed in {variable}: remove it from {variable} and restart, \
             then rename the user, then add the new username to {variable}"
        ),
    ))
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
    let user = named_user(rbac, &id).await?;

    let password = request.password;
    let hash = rbac.run_hashing(move || hash_password(&password)).await??;
    rbac.store
        .set_password_hash(&user.id, &hash)
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
    let user = named_user(rbac, &id).await?;
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

/// → `{attributes: {name: [values]}}`. Platform admins only.
pub(crate) async fn attributes(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let rbac = require_admin(&state, &ctx)?;
    let user = named_user(rbac, &id).await?;
    let attributes = rbac
        .store
        .user_attributes(&user.id)
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
    let user = named_user(rbac, &id).await?;
    let attributes = validated_attributes(body(request)?.attributes)?;
    rbac.store
        .set_user_attributes(&user.id, &attributes)
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
    let user = named_user(rbac, &id).await?;
    Ok(Json(access::build(
        &access_input(&state, rbac, user).await?,
    )))
}

/// → what the user reaches, by kind (`rbac::access::summarize`), with the
/// objects of the workspaces where they read objects. Platform admins only.
pub(crate) async fn permissions(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let rbac = require_admin(&state, &ctx)?;
    let user = named_user(rbac, &id).await?;
    let summary = access::summarize(&access::build(&access_input(&state, rbac, user).await?));
    let readable: Vec<(Uuid, bool)> = summary
        .workspaces
        .iter()
        .filter(|w| w.permissions.iter().any(|p| p == "read objects"))
        .filter_map(|w| {
            let write = w.permissions.iter().any(|p| p == "write objects");
            w.id.parse().ok().map(|id| (id, write))
        })
        .collect();
    let listed = try_join_all(
        readable
            .iter()
            .map(|(ws, _)| rbac.store.objects_of_workspace(*ws, None)),
    )
    .await
    .map_err(|e| store_error(e, "Conflict"))?;
    let objects: Vec<Value> = readable
        .iter()
        .zip(listed)
        .flat_map(|((ws, write), objects)| {
            objects.into_iter().map(move |o| {
                let permissions: &[&str] = if *write {
                    &["read", "write"]
                } else {
                    &["read"]
                };
                json!({
                    "id": o.id,
                    "type": o.r#type,
                    "name": o.name,
                    "workspaceId": ws,
                    "permissions": permissions,
                })
            })
        })
        .collect();
    let mut body = serde_json::to_value(&summary).map_err(|_| ApiError::internal())?;
    body["objects"] = Value::Array(objects);
    Ok(Json(body))
}

/// What `access::build` reads about a user: their relationships, rules,
/// groups, roles, workspaces, organizations, connections and the model's cubes.
async fn access_input(state: &AppState, rbac: &Rbac, user: User) -> Result<AccessInput, ApiError> {
    let id = user.id.clone();
    let platform_admin = rbac.is_platform_admin(&id).await?;
    let mut input = AccessInput {
        user,
        platform_admin,
        direct: vec![],
        organizations: vec![],
        workspaces: vec![],
        rules: vec![],
        indirect_rules: BTreeSet::new(),
        groups: vec![],
        rule_groups: vec![],
        roles: vec![],
        connections: vec![],
        releases: vec![],
        configured: BTreeSet::new(),
        platform_sources: vec![],
        cubes: vec![],
    };
    if platform_admin {
        return Ok(input);
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
    // The groups the user is in, and the rules those are placed in.
    let group_ids: Vec<Uuid> = rbac
        .authz
        .read(&RelationshipFilter {
            kind: "group",
            id: None,
            relation: Some("member"),
            subject: Some(user_ref(&id)),
        })
        .await
        .map_err(authz_error)?
        .iter()
        .filter_map(|r| r.resource.id.parse().ok())
        .collect();
    let groups = rbac
        .store
        .groups_by_ids(&group_ids)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    let placed = try_join_all(
        groups
            .iter()
            .map(|g| super::groups::rule_ids(rbac, g.id)),
    )
    .await?;
    let rule_groups: Vec<(Uuid, Uuid)> = groups
        .iter()
        .zip(placed)
        .flat_map(|(g, rules)| rules.into_iter().map(move |r| (r, g.id)))
        .collect();
    let indirect_rules: BTreeSet<Uuid> = rule_groups
        .iter()
        .map(|(r, _)| *r)
        .filter(|r| !rule_ids.contains(r))
        .collect();
    let mut rule_ids = rule_ids;
    rule_ids.extend(indirect_rules.iter().copied());
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
    input.indirect_rules = indirect_rules;
    input.groups = groups;
    input.rule_groups = rule_groups;
    input.roles = roles;
    input.connections = connections;
    input.releases = releases;
    input.configured = data_source_types(state).await?.into_keys().collect();

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
    input.cubes = access_cubes(&*crate::handlers::tree::index(state).await?);
    Ok(input)
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
