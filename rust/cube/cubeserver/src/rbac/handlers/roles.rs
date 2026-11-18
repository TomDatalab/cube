//! Roles an organization defines: `/v1/organizations/{id}/roles` and
//! `/v1/roles/{id}`.
//!
//! A role is a name and the workspace permissions it grants, from
//! [`PERMISSIONS`]. In SpiceDB each granted permission is
//! `role#can_<permission>@user:*`, and a rule with the role gives its members
//! `member & role->can_<permission>` in its workspace: editing a role changes
//! the access of every rule that uses it at once. The organization's admins
//! manage its roles; anyone who views the organization lists them.

use std::collections::BTreeSet;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::organizations::visible_organization;
use super::{body, caller, resource_id, store_error};
use crate::app::AppState;
use crate::error::ApiError;
use crate::rbac::restrictions::Restrictions;
use crate::rbac::store::{Role, StoreError};
use crate::rbac::{authz_error, ObjectRef, Rbac, Relationship, RelationshipFilter, Update};
use crate::services::RequestContext;

/// The workspace permissions a role can grant, in display order.
pub const PERMISSIONS: [&str; 6] = [
    "view",
    "query",
    "read_objects",
    "write_objects",
    "execute",
    "manage",
];

/// The permissions of the built-in roles of a rule, as a role would grant
/// them.
pub(crate) fn builtin_permissions(role: &str) -> &'static [&'static str] {
    match role {
        "owner" => &PERMISSIONS,
        "editor" => &["view", "query", "read_objects", "write_objects"],
        _ => &["view", "query", "read_objects"],
    }
}

/// The longest name a role takes.
const MAX_NAME: usize = 100;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewRole {
    name: String,
    permissions: Vec<String>,
    restrictions: Option<Restrictions>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RolePatch {
    name: Option<String>,
    permissions: Option<Vec<String>>,
    restrictions: Option<Restrictions>,
}

pub(crate) fn role_ref(id: Uuid) -> ObjectRef {
    ObjectRef::new("role", id.to_string())
}

fn can_relation(permission: &str) -> &'static str {
    match permission {
        "view" => "can_view",
        "query" => "can_query",
        "read_objects" => "can_read_objects",
        "write_objects" => "can_write_objects",
        "execute" => "can_execute",
        _ => "can_manage",
    }
}

/// `role#can_<permission>@user:*` for each permission.
fn wildcard_grants(role: Uuid, permissions: &[String]) -> Vec<Relationship> {
    permissions
        .iter()
        .map(|p| Relationship {
            resource: role_ref(role),
            relation: can_relation(p),
            subject: ObjectRef::new("user", "*"),
        })
        .collect()
}

/// The permissions, each known, with what they imply added (`manage`
/// implies all; `write_objects` implies `read_objects`; any implies
/// `view`), in [`PERMISSIONS`] order. 400 for an unknown or empty list.
pub(crate) fn normalize(permissions: &[String]) -> Result<Vec<String>, ApiError> {
    if let Some(unknown) = permissions
        .iter()
        .find(|p| !PERMISSIONS.contains(&p.as_str()))
    {
        return Err(ApiError::bad_request(format!(
            "Unknown permission {unknown}: use {}",
            PERMISSIONS.join(", ")
        )));
    }
    if permissions.is_empty() {
        return Err(ApiError::bad_request(
            "A role grants at least one permission",
        ));
    }
    let has = |p: &str| permissions.iter().any(|x| x == p);
    let manage = has("manage");
    let implied = |p: &str| match p {
        "view" => true,
        "read_objects" => manage || has("read_objects") || has("write_objects"),
        other => manage || has(other),
    };
    Ok(PERMISSIONS
        .iter()
        .filter(|p| implied(p))
        .map(|p| p.to_string())
        .collect())
}

fn role_name(name: &str) -> Result<String, ApiError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("The name cannot be empty"));
    }
    if name.chars().count() > MAX_NAME {
        return Err(ApiError::bad_request(format!(
            "The name takes at most {MAX_NAME} characters"
        )));
    }
    Ok(name.to_string())
}

const DUPLICATE: &str = "A role with this name already exists in the organization";

fn not_found() -> ApiError {
    ApiError::not_found("role not found")
}

fn view(role: &Role, rules: i64) -> Value {
    json!({
        "id": role.id,
        "organizationId": role.organization_id,
        "name": role.name,
        "permissions": role.permissions,
        "restrictions": role.restrictions,
        "rules": rules,
        "createdAt": role.created_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "createdBy": role.created_by,
    })
}

async fn rules_count(rbac: &Rbac, role: Uuid) -> Result<i64, ApiError> {
    Ok(rbac
        .store
        .rules_of_role(role)
        .await
        .map_err(|e| store_error(e, "Conflict"))?
        .len() as i64)
}

/// The role in the path, after `permission` is checked on its organization
/// (404 when the caller cannot view it, 403 when they lack `permission`).
async fn visible_role(
    rbac: &Rbac,
    id: &str,
    permission: &str,
    user: &str,
) -> Result<Role, ApiError> {
    let id = resource_id(id, "role")?;
    let role = rbac
        .store
        .role(id)
        .await
        .map_err(|e| store_error(e, "Conflict"))?
        .ok_or_else(not_found)?;
    visible_organization(rbac, &role.organization_id.to_string(), permission, user)
        .await
        .map_err(|e| {
            if e.status == StatusCode::NOT_FOUND {
                not_found()
            } else {
                e
            }
        })?;
    Ok(role)
}

/// → `{roles: [...]}`: `view` on the organization.
pub async fn list(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(org): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let org = visible_organization(rbac, &org, "view", user).await?;
    let roles: Vec<Value> = rbac
        .store
        .roles_of_organization(org.id)
        .await
        .map_err(|e| store_error(e, "Conflict"))?
        .iter()
        .map(|r| view(&r.role, r.rules))
        .collect();
    Ok(Json(json!({ "roles": roles })))
}

/// `{name, permissions, restrictions?}` → 201: `admin` on the organization.
pub async fn create(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(org): Path<String>,
    request: Result<Json<NewRole>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let org = visible_organization(rbac, &org, "admin", user).await?;
    let request = body(request)?;
    let name = role_name(&request.name)?;
    let permissions = normalize(&request.permissions)?;
    let restrictions = request.restrictions.unwrap_or_default().validated()?;

    let tx = rbac
        .store
        .transaction()
        .await
        .map_err(|e| store_error(e, DUPLICATE))?;
    let role = tx
        .create_role(org.id, &name, &permissions, &restrictions, user)
        .await
        .map_err(|e| store_error(e, DUPLICATE))?;
    rbac.authz
        .touch(&wildcard_grants(role.id, &role.permissions))
        .await
        // Dropping `tx` rolls the row back.
        .map_err(authz_error)?;
    tx.commit().await.map_err(|e| store_error(e, DUPLICATE))?;
    Ok((StatusCode::CREATED, Json(view(&role, 0))))
}

/// `view` on the organization.
pub async fn get(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let role = visible_role(rbac, &id, "view", user).await?;
    let rules = rules_count(rbac, role.id).await?;
    Ok(Json(view(&role, rules)))
}

/// `{name?, permissions?, restrictions?}`: `admin` on the organization. New permissions
/// reach every rule with the role at once.
pub async fn update(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
    request: Result<Json<RolePatch>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let role = visible_role(rbac, &id, "admin", user).await?;
    let patch = body(request)?;
    let name = match &patch.name {
        Some(n) => role_name(n)?,
        None => role.name.clone(),
    };
    let permissions = match &patch.permissions {
        Some(p) => normalize(p)?,
        None => role.permissions.clone(),
    };
    let restrictions = match patch.restrictions {
        Some(r) => r.validated()?,
        None => role.restrictions.clone(),
    };

    let mut updates: Vec<Update> = wildcard_grants(role.id, &role.permissions)
        .into_iter()
        .zip(&role.permissions)
        .filter(|(_, p)| !permissions.contains(p))
        .map(|(r, _)| Update::Delete(r))
        .collect();
    let added: Vec<String> = permissions
        .iter()
        .filter(|p| !role.permissions.contains(p))
        .cloned()
        .collect();
    updates.extend(
        wildcard_grants(role.id, &added)
            .into_iter()
            .map(Update::Touch),
    );

    let tx = rbac
        .store
        .transaction()
        .await
        .map_err(|e| store_error(e, DUPLICATE))?;
    let updated = tx
        .update_role(role.id, &name, &permissions, &restrictions)
        .await
        .map_err(|e| store_error(e, DUPLICATE))?
        .ok_or_else(not_found)?;
    rbac.authz
        .write(&updates, &[])
        .await
        // Dropping `tx` rolls the row back.
        .map_err(authz_error)?;
    tx.commit().await.map_err(|e| store_error(e, DUPLICATE))?;
    let rules = rules_count(rbac, role.id).await?;
    Ok(Json(view(&updated, rules)))
}

/// `admin` on the organization; 409 while a rule uses the role.
pub async fn delete(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let role = visible_role(rbac, &id, "admin", user).await?;
    let in_use = || {
        ApiError::new(
            StatusCode::CONFLICT,
            "The role is used by rules: give them another role or delete them first",
        )
    };
    let tx = rbac
        .store
        .transaction()
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    match tx.delete_role(role.id).await {
        Ok(_) => {}
        Err(StoreError::Conflict(_)) => return Err(in_use()),
        Err(e) => return Err(store_error(e, "Conflict")),
    }
    forget(rbac, role.id).await?;
    tx.commit().await.map_err(|e| store_error(e, "Conflict"))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Removes every relationship of the role.
pub(crate) async fn forget(rbac: &Rbac, role: Uuid) -> Result<(), ApiError> {
    rbac.authz
        .delete_matching(&RelationshipFilter {
            kind: "role",
            id: Some(role.to_string()),
            relation: None,
            subject: None,
        })
        .await
        .map_err(authz_error)
}

/// Whether every data source of a cube is among `connections`. A cube the
/// model cannot place (no data source) belongs to no one.
fn owned_by(of_cube: &BTreeSet<String>, connections: &BTreeSet<String>) -> bool {
    !of_cube.is_empty() && of_cube.is_subset(connections)
}

/// The members of one cube's meta kinds, as `[{name, title}]`.
fn meta_members(cube: &Value, kind: &str) -> Vec<Value> {
    cube[kind]
        .as_array()
        .map(|members| {
            members
                .iter()
                .map(|m| json!({ "name": m["name"], "title": m["title"] }))
                .collect()
        })
        .unwrap_or_default()
}

/// The meta's cubes (no views) whose data sources are all `connections`, in
/// the shape of the route's answer. A cube the model cannot place is left out.
fn listable_cubes(
    meta: &Value,
    connections: &BTreeSet<String>,
    is_view: impl Fn(&str) -> bool,
    data_sources_of_cube: impl Fn(&str) -> BTreeSet<String>,
) -> Vec<Value> {
    meta["cubes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|cube| {
            let Some(name) = cube["name"].as_str() else {
                return false;
            };
            !is_view(name) && owned_by(&data_sources_of_cube(name), connections)
        })
        .map(|cube| {
            json!({
                "name": cube["name"],
                "title": cube["title"],
                "measures": meta_members(cube, "measures"),
                "dimensions": meta_members(cube, "dimensions"),
                "segments": meta_members(cube, "segments"),
            })
        })
        .collect()
}

/// → `{cubes: [{name, title, measures, dimensions, segments}]}`: the model's
/// cubes (no views) that read only from the organization's connections, for
/// picking the members a role filters or blocks. `admin` on the organization.
pub async fn model_members(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(org): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let org = visible_organization(rbac, &org, "admin", user).await?;
    let connections: BTreeSet<String> = rbac
        .store
        .connections_of_organization(org.id)
        .await
        .map_err(|e| store_error(e, "Conflict"))?
        .into_iter()
        .map(|c| c.name)
        .collect();
    let sources = state.query.data_sources().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "The data model is not available",
        )
    })?;
    let meta = state.meta.meta(&ctx, false).await?;
    let cubes = listable_cubes(
        &meta,
        &connections,
        |cube| sources.graph().is_view(cube),
        |cube| sources.data_sources_of_cube(cube),
    );
    Ok(Json(json!({ "cubes": cubes })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cube_the_model_cannot_place_is_not_listed() {
        let meta = json!({ "cubes": [
            { "name": "mine", "title": "Mine", "measures": [{"name": "mine.count", "title": "Count"}] },
            { "name": "theirs", "title": "Theirs" },
            { "name": "unplaced", "title": "Unplaced" },
            { "name": "v", "title": "V" },
        ]});
        let set = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>();
        let listed = listable_cubes(
            &meta,
            &set(&["a"]),
            |cube| cube == "v",
            |cube| match cube {
                "mine" | "v" => set(&["a"]),
                "theirs" => set(&["b"]),
                _ => set(&[]),
            },
        );
        let names: Vec<&str> = listed.iter().map(|c| c["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["mine"]);
        assert_eq!(listed[0]["measures"][0]["title"], "Count");
        assert!(!owned_by(&set(&[]), &set(&[])));
    }

    fn n(list: &[&str]) -> Vec<String> {
        normalize(&list.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap()
    }

    #[test]
    fn permissions_take_what_they_imply_in_order() {
        assert_eq!(n(&["query"]), ["view", "query"]);
        assert_eq!(
            n(&["write_objects"]),
            ["view", "read_objects", "write_objects"]
        );
        assert_eq!(n(&["manage"]), PERMISSIONS);
        assert_eq!(n(&["execute", "view", "execute"]), ["view", "execute"]);
    }

    #[test]
    fn unknown_or_no_permissions_are_refused() {
        assert!(normalize(&["admin".to_string()]).is_err());
        assert!(normalize(&[]).is_err());
    }
}
