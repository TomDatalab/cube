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
//! SpiceDB, the RBAC database or the model failing to answer is a 503.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

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

/// What a file tree looked like: every entry's path, modification time and
/// size. Any edit, addition, removal or rename changes it.
type Stamp = Vec<(PathBuf, Option<SystemTime>, u64)>;

fn stamp(root: &Path) -> std::io::Result<Stamp> {
    fn walk(dir: &Path, out: &mut Stamp) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            out.push((entry.path(), metadata.modified().ok(), metadata.len()));
            if metadata.is_dir() {
                walk(&entry.path(), out)?;
            }
        }
        Ok(())
    }

    let metadata = std::fs::metadata(root)?;
    let mut out = vec![(root.to_path_buf(), metadata.modified().ok(), 0)];
    walk(root, &mut out)?;
    out.sort();
    Ok(out)
}

/// Which data source each cube and member of the model reads from, rebuilt
/// when a file of the model directory changes, so the checks follow the
/// model the planner reloads.
pub struct ModelIndex {
    schema_path: PathBuf,
    context: cubemodel::TemplateContext,
    cached: Mutex<Option<(Stamp, Arc<DataSources>)>>,
}

impl std::fmt::Debug for ModelIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelIndex")
            .field("schema_path", &self.schema_path)
            .finish_non_exhaustive()
    }
}

impl ModelIndex {
    pub fn new(schema_path: PathBuf, context: cubemodel::TemplateContext) -> Self {
        Self {
            schema_path,
            context,
            cached: Mutex::new(None),
        }
    }

    /// The current index; 503 when the model cannot be read, so nothing is
    /// allowed against a model nobody can check.
    fn current(&self) -> Result<Arc<DataSources>, ApiError> {
        let failed = |e: String| {
            tracing::error!(
                "Cannot index the data model at {} for the RBAC checks: {e}",
                self.schema_path.display()
            );
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "The data model cannot be read",
            )
        };
        let stamp = stamp(&self.schema_path).map_err(|e| failed(e.to_string()))?;
        let mut cached = self.cached.lock().map_err(|_| failed("poisoned".into()))?;
        if let Some((known, sources)) = cached.as_ref() {
            if *known == stamp {
                return Ok(sources.clone());
            }
        }
        let sources =
            Arc::new(DataSources::load(&self.schema_path, &self.context).map_err(failed)?);
        *cached = Some((stamp, sources.clone()));
        Ok(sources)
    }

    /// Every data source `members` read from (`default` by name). 403 naming
    /// the cube of a member the model cannot place.
    pub fn data_sources_of(&self, members: &[String]) -> Result<BTreeSet<String>, ApiError> {
        let sources = self.current()?;
        if let Some(unknown) = members
            .iter()
            .find(|member| sources.data_source_of(member).is_none())
        {
            return Err(denied(unknown));
        }
        Ok(sources.data_sources_of(members))
    }

    /// The data sources of a cube, or of every member of a view. Empty when
    /// the model does not know it or cannot be read.
    pub fn data_sources_of_cube(&self, cube: &str) -> BTreeSet<String> {
        self.current()
            .map(|sources| sources.data_sources_of_cube(cube))
            .unwrap_or_default()
    }
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

impl Access {
    fn allows(&self, data_source: &str) -> bool {
        match self {
            Access::All => true,
            Access::Only(allowed) => allowed.contains(data_source),
        }
    }
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
/// connections the user can query through any workspace (or manages) that
/// are still tracked connections.
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
    Ok(Access::Only(rows.into_iter().map(|row| row.name).collect()))
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
fn check_queries(
    model: &ModelIndex,
    access: &Access,
    queries: &[NormalizedQuery],
) -> Result<(), ApiError> {
    if matches!(access, Access::All) {
        return Ok(());
    }
    let sources = model.current()?;
    for query in queries {
        for member in members_of(query)? {
            match sources.data_source_of(&member) {
                Some(data_source) if access.allows(data_source) => {}
                // Not released, or not placeable on a data source at all.
                _ => return Err(denied(&member)),
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
    model: Arc<ModelIndex>,
}

impl RbacQueryService {
    pub fn new(inner: QueryServiceRef, rbac: Arc<Rbac>, model: Arc<ModelIndex>) -> Self {
        Self { inner, rbac, model }
    }

    async fn authorize(
        &self,
        ctx: &RequestContext,
        request: &NormalizedRequest,
    ) -> Result<(), ApiError> {
        let access = query_access(&self.rbac, ctx).await?;
        check_queries(&self.model, &access, &request.queries)
    }
}

#[async_trait]
impl QueryService for RbacQueryService {
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
    model: Arc<ModelIndex>,
}

impl RbacMetaService {
    pub fn new(inner: MetaServiceRef, rbac: Arc<Rbac>, model: Arc<ModelIndex>) -> Self {
        Self { inner, rbac, model }
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
        let sources = self.model.current()?;
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

    #[test]
    fn a_model_change_is_picked_up() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("model.yml");
        let cube = |source: &str| {
            format!(
                "cubes:\n  - name: c\n    data_source: {source}\n    sql_table: t\n    measures:\n      - name: count\n        type: count\n"
            )
        };
        std::fs::write(&file, cube("one")).unwrap();
        let index = ModelIndex::new(dir.path().to_path_buf(), Default::default());
        assert_eq!(
            index.data_sources_of(&["c.count".into()]).unwrap(),
            BTreeSet::from(["one".to_string()])
        );
        std::fs::write(&file, cube("second")).unwrap();
        assert_eq!(
            index.data_sources_of_cube("c"),
            BTreeSet::from(["second".to_string()])
        );
        assert_eq!(
            index
                .data_sources_of(&["nowhere.count".into()])
                .unwrap_err()
                .status,
            StatusCode::FORBIDDEN
        );
    }
}
