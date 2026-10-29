//! Per-workspace data access with RBAC on.
//!
//! Every data path (REST `/v1/load`, `/v1/sql`, `/v1/dry-run`,
//! `/v1/subscribe`, GraphQL and the WebSocket transport) reaches the model
//! through [`AppState::query_for`](crate::app::AppState::query_for) and
//! [`AppState::meta_for`](crate::app::AppState::meta_for), which wrap their
//! services in [`RbacQueryService`] and [`RbacMetaService`]:
//!
//! * platform admins pass, with or without a workspace;
//! * anyone else names a workspace (`X-Cube-Workspace`, 400 without it),
//!   needs `workspace#query`, and every data source the query reads from
//!   must be released to that workspace; otherwise 403 naming the cube;
//! * a member the model cannot place on a data source is refused, never
//!   allowed (fail-closed), and so are member expressions and subquery
//!   joins, which carry SQL of their own;
//! * `/v1/meta` keeps only the cubes and views whose data sources are all
//!   allowed.
//!
//! Members are placed on data sources with the snapshot the request's
//! planner runs with ([`QueryService::data_sources`]), built with the same
//! template context and replaced with it on a model reload: what is checked
//! is what runs. No snapshot, or SpiceDB or the RBAC database failing to
//! answer, is a 503.

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use cubequery::types::{NormalizedFilter, NormalizedQuery, QueryMember};
use serde_json::Value;
use uuid::Uuid;

use super::handlers::connections::released_connections;
use super::handlers::workspaces::workspace_ref;
use super::{authz_error, unauthenticated, unavailable, Rbac};
use crate::app::AppState;
use crate::error::ApiError;
use crate::planner_adapter::DataSources;
use crate::services::{
    MetaService, MetaServiceRef, NormalizedRequest, QueryService, QueryServiceRef, RequestContext,
};

/// The answer without `X-Cube-Workspace` on a data request.
pub const WORKSPACE_REQUIRED: &str = "X-Cube-Workspace is required";

/// 503: no data-source snapshot to check against, so nothing is allowed.
fn no_snapshot() -> ApiError {
    tracing::error!("No data-source snapshot for the RBAC checks; refusing");
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "The data model is not available",
    )
}

/// 403 naming the cube of `member`.
fn denied(member: &str) -> ApiError {
    let cube = member.split('.').next().unwrap_or(member);
    ApiError::forbidden(format!(
        "Permission denied: cube {cube} is not available in this workspace"
    ))
}

/// The data sources a request may read from.
enum Access {
    /// A platform admin: everything.
    All,
    Only(BTreeSet<String>),
}

/// The caller's user, or 401.
fn user_of(ctx: &RequestContext) -> Result<&str, ApiError> {
    ctx.user.as_deref().ok_or_else(unauthenticated)
}

/// The workspace of a request after `permission` on it is checked: 400
/// without one, 403 when it is not a workspace the user holds `permission` on.
async fn workspace_with(
    rbac: &Rbac,
    ctx: &RequestContext,
    permission: &str,
) -> Result<String, ApiError> {
    let user = user_of(ctx)?;
    let workspace = ctx
        .workspace
        .as_deref()
        .ok_or_else(|| ApiError::bad_request(WORKSPACE_REQUIRED))?;
    let Ok(id) = workspace.parse::<Uuid>() else {
        return Err(ApiError::forbidden(format!(
            "Permission denied: {permission} on workspace {workspace}"
        )));
    };
    rbac.require(&workspace_ref(id), permission, user).await?;
    Ok(workspace.to_string())
}

/// Data access of a query: `workspace#query` and the workspace's releases.
async fn query_access(rbac: &Rbac, ctx: &RequestContext) -> Result<Access, ApiError> {
    user_of(ctx)?;
    if ctx.platform_admin {
        return Ok(Access::All);
    }
    let workspace = workspace_with(rbac, ctx, "query").await?;
    Ok(Access::Only(released_connections(rbac, &workspace).await?))
}

/// What `/v1/meta` may show: with a workspace, as a query; without one, the
/// connections the user can query through any workspace (or manages) whose
/// row belongs to an organization the user can view.
async fn meta_access(rbac: &Rbac, ctx: &RequestContext) -> Result<Access, ApiError> {
    let user = user_of(ctx)?;
    if ctx.platform_admin {
        return Ok(Access::All);
    }
    if ctx.workspace.is_some() {
        return query_access(rbac, ctx).await;
    }
    let names = rbac
        .authz
        .lookup_resources("connection", "query", user)
        .await
        .map_err(authz_error)?;
    let rows = rbac.store.connections_by_names(&names).await.map_err(|e| {
        tracing::error!("RBAC store failed: {e}");
        unavailable()
    })?;
    // As `released_connections` does per workspace: a connection counts only
    // when its organization is one the user belongs to, so a stale
    // relationship from another organization grants nothing.
    let organizations: BTreeSet<String> = rbac
        .authz
        .lookup_resources("organization", "view", user)
        .await
        .map_err(authz_error)?
        .into_iter()
        .collect();
    Ok(Access::Only(
        rows.into_iter()
            .filter(|row| organizations.contains(&row.organization_id.to_string()))
            .map(|row| row.name)
            .collect(),
    ))
}

/// Every member and cube name a normalized query reads, typed so nothing is
/// skipped: measures, dimensions, segments, time dimensions, filters (nested
/// `and`/`or` included), order and join hints. Member expressions and
/// subquery joins carry SQL the model does not describe, so they are refused.
fn members_of(query: &NormalizedQuery) -> Result<Vec<String>, ApiError> {
    fn named(out: &mut Vec<String>, members: &[QueryMember]) -> Result<(), ApiError> {
        for member in members {
            match member {
                QueryMember::Name(name) => out.push(name.clone()),
                QueryMember::Expression(_) => {
                    return Err(ApiError::forbidden(
                        "Permission denied: member expressions are reserved to platform admins",
                    ))
                }
            }
        }
        Ok(())
    }

    fn filters(out: &mut Vec<String>, list: &[NormalizedFilter]) {
        for filter in list {
            match filter {
                NormalizedFilter::Leaf(leaf) => out.push(leaf.member.clone()),
                NormalizedFilter::And(nested) | NormalizedFilter::Or(nested) => {
                    filters(out, nested)
                }
            }
        }
    }

    let mut out = Vec::new();
    named(&mut out, &query.measures)?;
    named(&mut out, &query.dimensions)?;
    named(&mut out, &query.segments)?;
    out.extend(query.time_dimensions.iter().map(|td| td.dimension.clone()));
    filters(&mut out, &query.filters);
    out.extend(query.order.iter().flatten().map(|item| item.id.clone()));
    out.extend(query.join_hints.iter().flatten().flatten().cloned());
    out.extend(
        query
            .masked_members
            .iter()
            .flatten()
            .map(|masked| masked.member.clone()),
    );
    if query.subquery_joins.as_ref().is_some_and(|j| !j.is_empty()) {
        return Err(ApiError::forbidden(
            "Permission denied: subquery joins are reserved to platform admins",
        ));
    }
    Ok(out)
}

/// 403 unless every member of every query reads from an allowed data source.
/// A view member also needs every data source the view spans, as `/v1/meta`
/// hides such a view.
fn check_queries(
    sources: &DataSources,
    access: &Access,
    queries: &[NormalizedQuery],
) -> Result<(), ApiError> {
    let Access::Only(allowed) = access else {
        return Ok(());
    };
    for query in queries {
        for member in members_of(query)? {
            let cube = member.split('.').next().unwrap_or(&member);
            let of_cube = sources.data_sources_of_cube(cube);
            let allowed_here = match sources.data_source_of(&member) {
                Some(data_source) => {
                    allowed.contains(data_source)
                        && !of_cube.is_empty()
                        && of_cube.is_subset(allowed)
                }
                // Not placeable on a data source at all.
                None => false,
            };
            if !allowed_here {
                return Err(denied(&member));
            }
        }
    }
    Ok(())
}

/// [`QueryService`] that checks the workspace's access before the inner one
/// sees the request.
#[derive(Debug)]
pub struct RbacQueryService {
    inner: QueryServiceRef,
    rbac: Arc<Rbac>,
}

impl RbacQueryService {
    pub fn new(inner: QueryServiceRef, rbac: Arc<Rbac>) -> Self {
        Self { inner, rbac }
    }

    async fn authorize(
        &self,
        ctx: &RequestContext,
        request: &NormalizedRequest,
    ) -> Result<(), ApiError> {
        let access = query_access(&self.rbac, ctx).await?;
        if matches!(access, Access::All) {
            return Ok(());
        }
        // The planner's own snapshot: checked and run with the same one.
        let sources = self.inner.data_sources().ok_or_else(no_snapshot)?;
        check_queries(&sources, &access, &request.queries)
    }
}

#[async_trait]
impl QueryService for RbacQueryService {
    fn data_sources(&self) -> Option<Arc<DataSources>> {
        self.inner.data_sources()
    }

    async fn load(
        &self,
        ctx: &RequestContext,
        request: NormalizedRequest,
    ) -> Result<Value, ApiError> {
        self.authorize(ctx, &request).await?;
        self.inner.load(ctx, request).await
    }

    async fn sql(
        &self,
        ctx: &RequestContext,
        request: NormalizedRequest,
    ) -> Result<Value, ApiError> {
        self.authorize(ctx, &request).await?;
        self.inner.sql(ctx, request).await
    }

    async fn dry_run(
        &self,
        ctx: &RequestContext,
        request: NormalizedRequest,
    ) -> Result<Value, ApiError> {
        self.authorize(ctx, &request).await?;
        self.inner.dry_run(ctx, request).await
    }

    /// Cancelling reads nothing; the request id is known only to whoever
    /// started the query.
    async fn cancel_query(&self, ctx: &RequestContext, request_id: &str) -> Result<bool, ApiError> {
        user_of(ctx)?;
        self.inner.cancel_query(ctx, request_id).await
    }
}

/// [`MetaService`] that keeps only the cubes and views the request may read.
#[derive(Debug)]
pub struct RbacMetaService {
    inner: MetaServiceRef,
    rbac: Arc<Rbac>,
    /// The snapshot of the planner serving the same runtime as `inner`.
    sources: Option<Arc<DataSources>>,
}

impl RbacMetaService {
    pub fn new(inner: MetaServiceRef, rbac: Arc<Rbac>, sources: Option<Arc<DataSources>>) -> Self {
        Self {
            inner,
            rbac,
            sources,
        }
    }

    /// Drops from `meta.cubes` every cube or view with a data source outside
    /// `access`; one the model cannot place is dropped too.
    fn filter(&self, access: &Access, mut meta: Value) -> Result<Value, ApiError> {
        let Access::Only(allowed) = access else {
            return Ok(meta);
        };
        let Some(cubes) = meta.get_mut("cubes").and_then(Value::as_array_mut) else {
            return Ok(meta);
        };
        if cubes.is_empty() {
            return Ok(meta);
        }
        let sources = self.sources.as_ref().ok_or_else(no_snapshot)?;
        {
            cubes.retain(|cube| {
                let Some(name) = cube.get("name").and_then(Value::as_str) else {
                    return false;
                };
                let of_cube = sources.data_sources_of_cube(name);
                !of_cube.is_empty() && of_cube.is_subset(allowed)
            });
        }
        Ok(meta)
    }
}

#[async_trait]
impl MetaService for RbacMetaService {
    async fn meta(&self, ctx: &RequestContext, only_views: bool) -> Result<Value, ApiError> {
        let access = meta_access(&self.rbac, ctx).await?;
        let meta = self.inner.meta(ctx, only_views).await?;
        self.filter(&access, meta)
    }

    async fn meta_extended(
        &self,
        ctx: &RequestContext,
        only_views: bool,
    ) -> Result<Value, ApiError> {
        let access = meta_access(&self.rbac, ctx).await?;
        let meta = self.inner.meta_extended(ctx, only_views).await?;
        self.filter(&access, meta)
    }
}

/// `/v1/execute` with RBAC on: platform admins run anything; anyone else
/// needs a workspace, `workspace#execute`, and a named connection released
/// to it. `connection` is `None` for an ad hoc connection (settings), which
/// is for platform admins only.
pub(crate) async fn authorize_execute(
    state: &AppState,
    ctx: &RequestContext,
    connection: Option<&str>,
) -> Result<(), ApiError> {
    let Some(rbac) = state.rbac.as_deref() else {
        return Ok(());
    };
    user_of(ctx)?;
    if ctx.platform_admin {
        return Ok(());
    }
    let workspace = workspace_with(rbac, ctx, "execute").await?;
    let Some(name) = connection else {
        return Err(ApiError::forbidden(
            "Permission denied: only platform admins run statements on ad hoc connections",
        ));
    };
    if released_connections(rbac, &workspace).await?.contains(name) {
        Ok(())
    } else {
        Err(ApiError::forbidden(format!(
            "Permission denied: connection {name} is not released to this workspace"
        )))
    }
}

/// Route layer: with RBAC on, only platform admins pass (routes that run
/// SQL or build data without the cube checks).
pub async fn platform_admin_only(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if state.rbac.is_some() {
        let admin = request
            .extensions()
            .get::<RequestContext>()
            .is_some_and(|ctx| ctx.user.is_some() && ctx.platform_admin);
        if !admin {
            return ApiError::forbidden("Permission denied: platform admins only").into_response();
        }
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn normalized(query: Value) -> NormalizedQuery {
        let (_, mut queries) = cubequery::get_normalized_queries(
            &query,
            false,
            None,
            &cubequery::QueryConfig::default(),
        )
        .unwrap();
        queries.remove(0)
    }

    #[test]
    fn members_cover_every_place_a_member_can_hide() {
        let query = normalized(json!({
            "measures": ["m.count"],
            "dimensions": ["d.status"],
            "segments": ["s.only"],
            "timeDimensions": [{ "dimension": "t.at", "granularity": "day" }],
            "filters": [{ "or": [{ "and": [{ "member": "f.deep", "operator": "set" }] }] }],
            "order": [["o.key", "asc"]],
        }));
        let mut members = members_of(&query).unwrap();
        members.sort();
        members.dedup();
        for expected in ["m.count", "d.status", "s.only", "t.at", "f.deep", "o.key"] {
            assert!(
                members.iter().any(|m| m == expected),
                "{expected} in {members:?}"
            );
        }
    }
}
