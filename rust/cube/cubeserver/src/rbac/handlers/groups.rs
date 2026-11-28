//! Groups: `/v1/groups`, `/v1/organizations/{id}/groups` and
//! `/v1/groups/{id}`.
//!
//! A group gathers users of an organization (`group#member@user`) and is
//! placed in rules of that organization (`rule#group@group`): there its
//! members are members of the rule (`rule#members = member + group->member`),
//! with what the rule grants, for as long as they stay in the group. The
//! organization's admins manage its groups; anyone who views the
//! organization reads them. Placing a group in a rule also takes `manage` on
//! the rule's workspace. A group placed in rules is not deleted.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use futures::future::try_join_all;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::organizations::{user_ref, visible_organization};
use super::rules::{rule_listing, rule_ref, rule_view, visible_rule};
use super::{body, caller, existing_user, resource_id, store_error, SearchParams};
use crate::app::AppState;
use crate::error::ApiError;
use crate::rbac::store::{Group, GroupListing};
use crate::rbac::{authz_error, ObjectRef, Rbac, Relationship, RelationshipFilter};
use crate::services::RequestContext;

/// The longest name a group takes.
const MAX_NAME: usize = 100;

const DUPLICATE: &str = "A group with this name already exists in the organization";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewGroup {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupFilter {
    organization_id: Option<String>,
}

pub(crate) fn group_ref(id: Uuid) -> ObjectRef {
    ObjectRef::new("group", id.to_string())
}

fn membership(group: Uuid, user: &str) -> Relationship {
    Relationship {
        resource: group_ref(group),
        relation: "member",
        subject: user_ref(user),
    }
}

/// `rule#group@group`: the group's members are members of the rule.
fn placement(rule: Uuid, group: Uuid) -> Relationship {
    Relationship {
        resource: rule_ref(rule),
        relation: "group",
        subject: group_ref(group),
    }
}

fn not_found() -> ApiError {
    ApiError::not_found("group not found")
}

fn group_name(name: &str) -> Result<String, ApiError> {
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

/// The ids of the group's members, sorted.
async fn member_ids(rbac: &Rbac, group: Uuid) -> Result<Vec<String>, ApiError> {
    let mut ids: Vec<String> = rbac
        .authz
        .read(&RelationshipFilter {
            kind: "group",
            id: Some(group.to_string()),
            relation: Some("member"),
            subject: None,
        })
        .await
        .map_err(authz_error)?
        .into_iter()
        .filter(|r| r.subject.kind == "user")
        .map(|r| r.subject.id)
        .collect();
    ids.sort();
    Ok(ids)
}

/// The ids of the rules the group is placed in.
pub(crate) async fn rule_ids(rbac: &Rbac, group: Uuid) -> Result<Vec<Uuid>, ApiError> {
    Ok(rbac
        .authz
        .read(&RelationshipFilter {
            kind: "rule",
            id: None,
            relation: Some("group"),
            subject: Some(group_ref(group)),
        })
        .await
        .map_err(authz_error)?
        .iter()
        .filter_map(|r| r.resource.id.parse().ok())
        .collect())
}

/// The ids of the groups placed in the rule.
pub(crate) async fn groups_of_rule(rbac: &Rbac, rule: Uuid) -> Result<Vec<Uuid>, ApiError> {
    Ok(rbac
        .authz
        .read(&RelationshipFilter {
            kind: "rule",
            id: Some(rule.to_string()),
            relation: Some("group"),
            subject: None,
        })
        .await
        .map_err(authz_error)?
        .iter()
        .filter(|r| r.subject.kind == "group")
        .filter_map(|r| r.subject.id.parse().ok())
        .collect())
}

fn view(group: &Group, organization_name: &str, members: usize, rules: usize) -> Value {
    json!({
        "id": group.id,
        "organizationId": group.organization_id,
        "organizationName": organization_name,
        "name": group.name,
        "members": members,
        "rules": rules,
        "createdAt": group.created_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "createdBy": group.created_by,
    })
}

/// [`view`] with the group's member and rule counts.
async fn counted(rbac: &Rbac, group: &Group, organization_name: &str) -> Result<Value, ApiError> {
    let (members, rules) = futures::try_join!(
        member_ids(rbac, group.id),
        rule_ids(rbac, group.id)
    )?;
    Ok(view(group, organization_name, members.len(), rules.len()))
}

async fn listed(rbac: &Rbac, items: &[GroupListing]) -> Result<Vec<Value>, ApiError> {
    try_join_all(
        items
            .iter()
            .map(|l| counted(rbac, &l.group, &l.organization_name)),
    )
    .await
}

/// The group in the path and its organization's name, after `permission` is
/// checked on the organization (404 when the caller cannot view it, 403 when
/// they lack `permission`).
async fn visible_group(
    rbac: &Rbac,
    id: &str,
    permission: &str,
    user: &str,
) -> Result<(Group, String), ApiError> {
    let id = resource_id(id, "group")?;
    let group = rbac
        .store
        .group(id)
        .await
        .map_err(|e| store_error(e, "Conflict"))?
        .ok_or_else(not_found)?;
    let org = visible_organization(rbac, &group.organization_id.to_string(), permission, user)
        .await
        .map_err(|e| {
            if e.status == StatusCode::NOT_FOUND {
                not_found()
            } else {
                e
            }
        })?;
    Ok((group, org.name))
}

/// `?q&organizationId&limit&offset` → `{groups: [...], total}`: the groups of
/// every organization the caller views. Platform admins see all of them.
pub(crate) async fn list_all(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Query(params): Query<SearchParams>,
    Query(filter): Query<GroupFilter>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let search = params.search()?;
    let organization = filter
        .organization_id
        .as_deref()
        .map(|id| {
            id.parse::<Uuid>()
                .map_err(|_| ApiError::bad_request("organizationId must be a UUID"))
        })
        .transpose()?;
    let visible = if ctx.platform_admin {
        None
    } else {
        let ids: Vec<Uuid> = rbac
            .authz
            .lookup_resources("organization", "view", user)
            .await
            .map_err(authz_error)?
            .iter()
            .filter_map(|id| id.parse().ok())
            .collect();
        Some(ids)
    };
    let page = rbac
        .store
        .search_groups(visible.as_deref(), organization, &search)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    let groups = listed(rbac, &page.items).await?;
    Ok(Json(json!({ "groups": groups, "total": page.total })))
}

/// → `{groups: [...]}`: `view` on the organization.
pub async fn of_organization(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(org): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let org = visible_organization(rbac, &org, "view", user).await?;
    let page = rbac
        .store
        .search_groups(None, Some(org.id), &Default::default())
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    let groups = listed(rbac, &page.items).await?;
    Ok(Json(json!({ "groups": groups })))
}

/// `{name}` → 201: `admin` on the organization. The group starts empty.
pub async fn create(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(org): Path<String>,
    request: Result<Json<NewGroup>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let org = visible_organization(rbac, &org, "admin", user).await?;
    let request = body(request)?;
    let name = group_name(&request.name)?;
    let tx = rbac
        .store
        .transaction()
        .await
        .map_err(|e| store_error(e, DUPLICATE))?;
    let group = tx
        .create_group(org.id, &name, super::actor(&ctx)?)
        .await
        .map_err(|e| store_error(e, DUPLICATE))?;
    tx.commit().await.map_err(|e| store_error(e, DUPLICATE))?;
    Ok((StatusCode::CREATED, Json(view(&group, &org.name, 0, 0))))
}

/// `view` on the organization.
pub async fn get(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let (group, org_name) = visible_group(rbac, &id, "view", user).await?;
    Ok(Json(counted(rbac, &group, &org_name).await?))
}

/// `{name}`: `admin` on the organization.
pub async fn update(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
    request: Result<Json<NewGroup>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let (group, org_name) = visible_group(rbac, &id, "admin", user).await?;
    let name = group_name(&body(request)?.name)?;
    let tx = rbac
        .store
        .transaction()
        .await
        .map_err(|e| store_error(e, DUPLICATE))?;
    let renamed = tx
        .rename_group(group.id, &name)
        .await
        .map_err(|e| store_error(e, DUPLICATE))?
        .ok_or_else(not_found)?;
    tx.commit().await.map_err(|e| store_error(e, DUPLICATE))?;
    Ok(Json(counted(rbac, &renamed, &org_name).await?))
}

/// `admin` on the organization; 409 while the group is placed in rules. Its
/// members go with it.
pub async fn delete(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let (group, _) = visible_group(rbac, &id, "admin", user).await?;
    if !rule_ids(rbac, group.id).await?.is_empty() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "The group is in rules: take it out of them first",
        ));
    }
    let tx = rbac
        .store
        .transaction()
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    tx.delete_group(group.id)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    forget(rbac, group.id).await?;
    tx.commit().await.map_err(|e| store_error(e, "Conflict"))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Removes every relationship of the group: its placements in rules first
/// (so a failure midway takes access away rather than leaving it), then its
/// members.
pub(crate) async fn forget(rbac: &Rbac, group: Uuid) -> Result<(), ApiError> {
    let placements: Vec<Relationship> = rule_ids(rbac, group)
        .await?
        .into_iter()
        .map(|rule| placement(rule, group))
        .collect();
    rbac.authz
        .delete(&placements)
        .await
        .map_err(authz_error)?;
    rbac.authz
        .delete_matching(&RelationshipFilter {
            kind: "group",
            id: Some(group.to_string()),
            relation: None,
            subject: None,
        })
        .await
        .map_err(authz_error)
}

/// → `{members: [{user, username}]}`, `user` the UUID: `view` on the
/// organization.
pub async fn members(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let (group, _) = visible_group(rbac, &id, "view", user).await?;
    let ids = member_ids(rbac, group.id).await?;
    let names = super::workspaces::usernames(rbac, &ids).await?;
    let members: Vec<Value> = ids
        .into_iter()
        .map(|user| json!({ "username": names.get(&user), "user": user }))
        .collect();
    Ok(Json(json!({ "members": members })))
}

/// `admin` on the organization: the user joins the group, and with it its
/// rules.
pub async fn add_member(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path((id, member)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let (group, _) = visible_group(rbac, &id, "admin", user).await?;
    let member = existing_user(rbac, &member).await?;
    rbac.authz
        .touch(&[membership(group.id, &member.id)])
        .await
        .map_err(authz_error)?;
    Ok(Json(
        json!({ "user": member.id, "username": member.username }),
    ))
}

/// `admin` on the organization: the user leaves the group.
pub async fn remove_member(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path((id, member)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let (group, _) = visible_group(rbac, &id, "admin", user).await?;
    let member =
        super::member_subject(rbac, &member, "group", group.id.to_string(), &["member"]).await?;
    rbac.authz
        .delete(&[membership(group.id, &member)])
        .await
        .map_err(authz_error)?;
    Ok(StatusCode::NO_CONTENT)
}

/// → `{rules: [...]}`, as `/v1/rules` lists them: `view` on the
/// organization.
pub async fn rules(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let (group, _) = visible_group(rbac, &id, "view", user).await?;
    let ids = rule_ids(rbac, group.id).await?;
    let rules = rbac
        .store
        .rules_by_ids(&ids)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    let mut out = Vec::with_capacity(rules.len());
    for rule in rules {
        let Some(ws) = rbac
            .store
            .workspace(rule.workspace_id)
            .await
            .map_err(|e| store_error(e, "Conflict"))?
        else {
            continue;
        };
        out.push(rule_view(&rule_listing(rbac, rule, &ws).await?));
    }
    Ok(Json(json!({ "rules": out })))
}

/// The group and the rule in the path, for placing one in the other: `admin`
/// on the group's organization, `manage` on the rule's workspace, and both
/// in the same organization.
async fn group_and_rule(
    rbac: &Rbac,
    id: &str,
    rule: &str,
    user: &str,
) -> Result<(Group, Uuid), ApiError> {
    let (group, _) = visible_group(rbac, id, "admin", user).await?;
    let (rule, ws) = visible_rule(rbac, rule, "manage", user).await?;
    if ws.organization_id != group.organization_id {
        return Err(ApiError::bad_request(
            "The rule belongs to another organization",
        ));
    }
    Ok((group, rule.id))
}

/// The group's members become members of the rule.
pub async fn add_rule(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path((id, rule)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let (group, rule) = group_and_rule(rbac, &id, &rule, user).await?;
    rbac.authz
        .touch(&[placement(rule, group.id)])
        .await
        .map_err(authz_error)?;
    Ok(StatusCode::NO_CONTENT)
}

/// The group leaves the rule; members who are also in it directly stay.
pub async fn remove_rule(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path((id, rule)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let (group, rule) = group_and_rule(rbac, &id, &rule, user).await?;
    rbac.authz
        .delete(&[placement(rule, group.id)])
        .await
        .map_err(authz_error)?;
    Ok(StatusCode::NO_CONTENT)
}
