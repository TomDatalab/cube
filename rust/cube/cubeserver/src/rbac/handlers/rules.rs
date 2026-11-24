//! Rules: `/v1/rules` and `/v1/workspaces/{id}/rules`.
//!
//! A rule is a named role in one workspace: a description, a type (any
//! label, e.g. `finance`) and what it grants, either a built-in role
//! (`role`: `owner`, `editor` or `viewer`) or one its organization defines
//! (`roleId`, see [`super::roles`]). In SpiceDB a built-in role is
//! `workspace#<role>_rule@rule`; an organization's is `workspace#rule@rule`
//! plus `rule#role@role`. Its members (`rule#member@user`) hold those
//! permissions in the workspace and see its organization through
//! `organization#rule@rule`. Managing a rule, or its members, takes `manage`
//! on its workspace; moving it to another workspace takes `manage` on both.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::organizations::{organization_ref, user_ref};
use super::roles::{builtin_permissions, role_ref};
use super::workspaces::{visible_workspace, workspace_ref, ROLES};
use super::{body, caller, existing_user, resource_id, store_error, SearchParams};
use crate::app::AppState;
use crate::error::ApiError;
use crate::rbac::identity::validate_user_id;
use crate::rbac::store::{Rule, RuleListing, RuleRole, Workspace};
use crate::rbac::{authz_error, ObjectRef, Rbac, Relationship, RelationshipFilter, Update};
use crate::services::RequestContext;

/// The longest description a rule takes.
const MAX_DESCRIPTION: usize = 500;
/// The longest type a rule takes.
const MAX_TYPE: usize = 100;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NewRule {
    description: String,
    r#type: String,
    role: Option<String>,
    role_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RulePatch {
    description: Option<String>,
    r#type: Option<String>,
    role: Option<String>,
    role_id: Option<String>,
    workspace_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleFilter {
    organization_id: Option<String>,
    workspace_id: Option<String>,
}

pub(crate) fn rule_ref(id: Uuid) -> ObjectRef {
    ObjectRef::new("rule", id.to_string())
}

/// The workspace relation that gives a rule a built-in role.
fn builtin_relation(role: &str) -> &'static str {
    match role {
        "owner" => "owner_rule",
        "editor" => "editor_rule",
        _ => "viewer_rule",
    }
}

/// Every relationship that places `rule` in `ws` with `role`.
fn placement(rule: Uuid, ws: &Workspace, role: RuleRole<'_>) -> Vec<Relationship> {
    let mut rels = vec![Relationship {
        resource: organization_ref(ws.organization_id),
        relation: "rule",
        subject: rule_ref(rule),
    }];
    match role {
        RuleRole::Builtin(r) => rels.push(Relationship {
            resource: workspace_ref(ws.id),
            relation: builtin_relation(r),
            subject: rule_ref(rule),
        }),
        RuleRole::Custom(role) => {
            rels.push(Relationship {
                resource: workspace_ref(ws.id),
                relation: "rule",
                subject: rule_ref(rule),
            });
            rels.push(Relationship {
                resource: rule_ref(rule),
                relation: "role",
                subject: role_ref(role),
            });
        }
    }
    rels
}

fn membership(rule: Uuid, user: &str) -> Relationship {
    Relationship {
        resource: rule_ref(rule),
        relation: "member",
        subject: user_ref(user),
    }
}

fn not_found() -> ApiError {
    ApiError::not_found("rule not found")
}

/// A text field: trimmed, not empty, at most `max` characters.
fn text_field(text: &str, field: &str, max: usize) -> Result<String, ApiError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(ApiError::bad_request(format!(
            "The {field} cannot be empty"
        )));
    }
    if text.chars().count() > max {
        return Err(ApiError::bad_request(format!(
            "The {field} takes at most {max} characters"
        )));
    }
    Ok(text.to_string())
}

fn description(text: &str) -> Result<String, ApiError> {
    text_field(text, "description", MAX_DESCRIPTION)
}

/// A type: any label.
fn rule_type(text: &str) -> Result<String, ApiError> {
    text_field(text, "type", MAX_TYPE)
}

/// A built-in role: one of the workspace roles.
fn builtin_role(text: &str) -> Result<&'static str, ApiError> {
    ROLES
        .iter()
        .copied()
        .find(|r| *r == text)
        .ok_or_else(|| ApiError::bad_request("The role must be owner, editor or viewer"))
}

/// The role a request names (`role` or `roleId`, not both), `None` when it
/// names none. A `roleId` must be a role of `organization`.
async fn requested_role(
    rbac: &Rbac,
    role: Option<&str>,
    role_id: Option<&str>,
    organization: Uuid,
) -> Result<Option<RuleRole<'static>>, ApiError> {
    match (role, role_id) {
        (Some(_), Some(_)) => Err(ApiError::bad_request("Give role or roleId, not both")),
        (Some(r), None) => Ok(Some(RuleRole::Builtin(builtin_role(r)?))),
        (None, Some(id)) => {
            let id: Uuid = id
                .parse()
                .map_err(|_| ApiError::bad_request("roleId must be a UUID"))?;
            let role = rbac
                .store
                .role(id)
                .await
                .map_err(|e| store_error(e, "Conflict"))?
                .ok_or_else(|| ApiError::bad_request("Unknown role"))?;
            if role.organization_id != organization {
                return Err(ApiError::bad_request(
                    "The role belongs to another organization",
                ));
            }
            Ok(Some(RuleRole::Custom(id)))
        }
        (None, None) => Ok(None),
    }
}

fn optional_uuid(id: Option<&str>, name: &str) -> Result<Option<Uuid>, ApiError> {
    id.map(|id| {
        id.parse::<Uuid>()
            .map_err(|_| ApiError::bad_request(format!("{name} must be a UUID")))
    })
    .transpose()
}

fn view(listing: &RuleListing) -> Value {
    let rule = &listing.rule;
    let (role_name, permissions): (String, Vec<String>) = match (&rule.role, &listing.custom_role) {
        (Some(r), _) => (
            r.clone(),
            builtin_permissions(r)
                .iter()
                .map(|p| p.to_string())
                .collect(),
        ),
        (None, Some((name, perms))) => (name.clone(), perms.clone()),
        (None, None) => (String::new(), vec![]),
    };
    json!({
        "id": rule.id,
        "description": rule.description,
        "type": rule.r#type,
        "role": rule.role,
        "roleId": rule.role_id,
        "roleName": role_name,
        "permissions": permissions,
        "workspaceId": rule.workspace_id,
        "workspaceName": listing.workspace_name,
        "organizationId": listing.organization_id,
        "organizationName": listing.organization_name,
        "createdAt": rule.created_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "createdBy": rule.created_by,
    })
}

/// `rule` with the names of its workspace, organization and custom role.
async fn listing(rbac: &Rbac, rule: Rule, ws: &Workspace) -> Result<RuleListing, ApiError> {
    let org = rbac
        .store
        .organization(ws.organization_id)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    let custom_role = match rule.role_id {
        Some(id) => rbac
            .store
            .role(id)
            .await
            .map_err(|e| store_error(e, "Conflict"))?
            .map(|r| (r.name, r.permissions)),
        None => None,
    };
    Ok(RuleListing {
        rule,
        workspace_name: ws.name.clone(),
        organization_id: ws.organization_id,
        organization_name: org.map(|o| o.name).unwrap_or_default(),
        custom_role,
    })
}

/// The rule in the path and its workspace, after `permission` is checked on
/// the workspace (404 when the caller cannot view it, 403 when they lack
/// `permission`).
async fn visible_rule(
    rbac: &Rbac,
    id: &str,
    permission: &str,
    user: &str,
) -> Result<(Rule, Workspace), ApiError> {
    let id = resource_id(id, "rule")?;
    let rule = rbac
        .store
        .rule(id)
        .await
        .map_err(|e| store_error(e, "Conflict"))?
        .ok_or_else(not_found)?;
    let ws = visible_workspace(rbac, &rule.workspace_id.to_string(), permission, user)
        .await
        .map_err(|e| {
            if e.status == StatusCode::NOT_FOUND {
                not_found()
            } else {
                e
            }
        })?;
    Ok((rule, ws))
}

/// `{description, type, role | roleId}` → 201: `manage` on the workspace.
pub async fn create(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(ws): Path<String>,
    request: Result<Json<NewRule>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let ws = visible_workspace(rbac, &ws, "manage", user).await?;
    let request = body(request)?;
    let text = description(&request.description)?;
    let r#type = rule_type(&request.r#type)?;
    let role = requested_role(
        rbac,
        request.role.as_deref(),
        request.role_id.as_deref(),
        ws.organization_id,
    )
    .await?
    .ok_or_else(|| ApiError::bad_request("Give a role or a roleId"))?;

    let tx = rbac
        .store
        .transaction()
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    let rule = tx
        .create_rule(ws.id, &text, &r#type, role, super::actor(&ctx)?)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    rbac.authz
        .touch(&placement(rule.id, &ws, role))
        .await
        // Dropping `tx` rolls the row back.
        .map_err(authz_error)?;
    tx.commit().await.map_err(|e| store_error(e, "Conflict"))?;
    let listing = listing(rbac, rule, &ws).await?;
    Ok((StatusCode::CREATED, Json(view(&listing))))
}

/// `?q&organizationId&workspaceId&limit&offset` → `{rules: [...], total}`:
/// the rules of every workspace the caller can view. Platform admins see all
/// of them.
pub(crate) async fn list_all(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Query(params): Query<SearchParams>,
    Query(filter): Query<RuleFilter>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let search = params.search()?;
    let organization = optional_uuid(filter.organization_id.as_deref(), "organizationId")?;
    let workspace = optional_uuid(filter.workspace_id.as_deref(), "workspaceId")?;
    let visible = if ctx.platform_admin {
        None
    } else {
        let ids: Vec<Uuid> = rbac
            .authz
            .lookup_resources("workspace", "view", user)
            .await
            .map_err(authz_error)?
            .iter()
            .filter_map(|id| id.parse().ok())
            .collect();
        Some(ids)
    };
    let page = rbac
        .store
        .search_rules(visible.as_deref(), organization, workspace, &search)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    let rules: Vec<Value> = page.items.iter().map(view).collect();
    Ok(Json(json!({ "rules": rules, "total": page.total })))
}

/// → `{rules: [...]}`: those of the workspace, `view`.
pub async fn of_workspace(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(ws): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let ws = visible_workspace(rbac, &ws, "view", user).await?;
    let rules = rbac
        .store
        .rules_of_workspace(ws.id)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    let mut out = Vec::with_capacity(rules.len());
    for rule in rules {
        out.push(view(&listing(rbac, rule, &ws).await?));
    }
    Ok(Json(json!({ "rules": out })))
}

/// `view` on the workspace.
pub async fn get(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let (rule, ws) = visible_rule(rbac, &id, "view", user).await?;
    Ok(Json(view(&listing(rbac, rule, &ws).await?)))
}

/// `{description?, type?, role? | roleId?, workspaceId?}`: `manage` on the
/// workspace, and on the new one when `workspaceId` moves the rule. Its
/// members come along. A rule with an organization's role moves to another
/// organization only with a role of that one.
pub async fn update(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
    request: Result<Json<RulePatch>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let (rule, from) = visible_rule(rbac, &id, "manage", user).await?;
    let patch = body(request)?;
    let text = match &patch.description {
        Some(d) => description(d)?,
        None => rule.description.clone(),
    };
    let r#type = match &patch.r#type {
        Some(t) => rule_type(t)?,
        None => rule.r#type.clone(),
    };
    let to = match &patch.workspace_id {
        Some(ws) if *ws != rule.workspace_id.to_string() => {
            visible_workspace(rbac, ws, "manage", user).await?
        }
        _ => from.clone(),
    };
    let current = rule.role_ref();
    let role = match requested_role(
        rbac,
        patch.role.as_deref(),
        patch.role_id.as_deref(),
        to.organization_id,
    )
    .await?
    {
        Some(role) => role,
        None if matches!(current, RuleRole::Custom(_))
            && to.organization_id != from.organization_id =>
        {
            return Err(ApiError::bad_request(
                "The rule's role belongs to its organization: give a role of the new one",
            ));
        }
        None => match current {
            RuleRole::Builtin(r) => RuleRole::Builtin(builtin_role(r)?),
            RuleRole::Custom(id) => RuleRole::Custom(id),
        },
    };

    // Only what changes, as one atomic write: SpiceDB refuses the same
    // relationship twice in a request, and the rule never grants in two
    // places.
    let old = placement(rule.id, &from, current);
    let new = placement(rule.id, &to, role);
    let mut updates: Vec<Update> = old
        .iter()
        .filter(|r| !new.contains(r))
        .cloned()
        .map(Update::Delete)
        .collect();
    updates.extend(
        new.iter()
            .filter(|r| !old.contains(r))
            .cloned()
            .map(Update::Touch),
    );

    let tx = rbac
        .store
        .transaction()
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    let updated = tx
        .update_rule(rule.id, to.id, &text, &r#type, role)
        .await
        .map_err(|e| store_error(e, "Conflict"))?
        .ok_or_else(not_found)?;
    rbac.authz
        .write(&updates, &[])
        .await
        // Dropping `tx` rolls the row back.
        .map_err(authz_error)?;
    tx.commit().await.map_err(|e| store_error(e, "Conflict"))?;
    Ok(Json(view(&listing(rbac, updated, &to).await?)))
}

/// `manage` on the workspace. The row goes only after SpiceDB forgot the
/// rule; its members lose what it gave them.
pub async fn delete(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let (rule, ws) = visible_rule(rbac, &id, "manage", user).await?;
    let tx = rbac
        .store
        .transaction()
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    tx.delete_rule(rule.id)
        .await
        .map_err(|e| store_error(e, "Conflict"))?;
    forget(rbac, &rule, &ws).await?;
    tx.commit().await.map_err(|e| store_error(e, "Conflict"))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Removes every relationship of `rule`: its placement first (so a failure
/// midway takes access away rather than leaving it), then its members and
/// role.
pub(crate) async fn forget(rbac: &Rbac, rule: &Rule, ws: &Workspace) -> Result<(), ApiError> {
    rbac.authz
        .delete(&placement(rule.id, ws, rule.role_ref()))
        .await
        .map_err(authz_error)?;
    rbac.authz
        .delete_matching(&RelationshipFilter {
            kind: "rule",
            id: Some(rule.id.to_string()),
            relation: None,
            subject: None,
        })
        .await
        .map_err(authz_error)
}

/// The ids of the rule's members, sorted.
async fn read_members(rbac: &Rbac, rule: Uuid) -> Result<Vec<String>, ApiError> {
    let mut members: Vec<String> = rbac
        .authz
        .read(&RelationshipFilter {
            kind: "rule",
            id: Some(rule.to_string()),
            relation: Some("member"),
            subject: None,
        })
        .await
        .map_err(authz_error)?
        .into_iter()
        .filter(|r| r.subject.kind == "user")
        .map(|r| r.subject.id)
        .collect();
    members.sort();
    Ok(members)
}

/// → `{members: [{user}]}`: `view` on the workspace.
pub async fn members(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let (rule, _) = visible_rule(rbac, &id, "view", user).await?;
    let members: Vec<Value> = read_members(rbac, rule.id)
        .await?
        .into_iter()
        .map(|user| json!({ "user": user }))
        .collect();
    Ok(Json(json!({ "members": members })))
}

/// `manage` on the workspace: the user joins the rule.
pub async fn add_member(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path((id, member)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let (rule, _) = visible_rule(rbac, &id, "manage", user).await?;
    let member = existing_user(rbac, &member).await?;
    rbac.authz
        .touch(&[membership(rule.id, &member.id)])
        .await
        .map_err(authz_error)?;
    Ok(Json(json!({ "user": member.id })))
}

/// `manage` on the workspace: the user leaves the rule.
pub async fn remove_member(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path((id, member)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let (rbac, user) = caller(&state, &ctx)?;
    let (rule, _) = visible_rule(rbac, &id, "manage", user).await?;
    validate_user_id(&member)?;
    rbac.authz
        .delete(&[membership(rule.id, &member)])
        .await
        .map_err(authz_error)?;
    Ok(StatusCode::NO_CONTENT)
}
