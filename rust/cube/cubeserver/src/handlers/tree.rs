//! `GET /playground/tree*` — the model by organization, for the Playground's
//! tree. Mounted with the other Playground helpers, so with RBAC on only
//! platform admins reach it.

use std::path::{Component, Path as FsPath, PathBuf};

use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use axum::Json;
use cubemodel::ownership::{current_policy, Owner, PLATFORM_FOLDER};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::error::ApiError;
use crate::model_index::ModelIndex;
use crate::rbac::model_snapshot::{is_editable, source_path};
use crate::rbac::model_store::ModelFile;
use crate::rbac::store::Search;

/// The one node of the tree when RBAC is off.
const MODEL_NODE: &str = "_model";
const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;

#[derive(Debug, Deserialize)]
pub struct TreeQuery {
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

pub(crate) async fn index(state: &AppState) -> Result<std::sync::Arc<ModelIndex>, ApiError> {
    let cache = state.model_index.clone();
    let path = PathBuf::from(&state.config.schema_path);
    tokio::task::spawn_blocking(move || cache.get(&path))
        .await
        .map_err(|e| {
            tracing::error!("Cannot index the data model: {e}");
            ApiError::internal()
        })
}

/// A store failure is ours, not the caller's: 503, like the rest of RBAC.
fn store_error(e: crate::rbac::store::StoreError) -> ApiError {
    tracing::error!("The RBAC database cannot answer the tree: {e}");
    crate::rbac::unavailable()
}

/// Whether something owned by `o` shows under the node of `node`. What no
/// organization owns shows under `_platform`, so the admin sees why it is
/// refused.
pub(crate) fn belongs(node: &Owner, o: &Owner) -> bool {
    match node {
        Owner::Platform => matches!(o, Owner::Platform | Owner::Unowned(_)),
        _ => node == o,
    }
}

fn clamp_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(0, MAX_LIMIT)
}

/// The key of the scope a node's model is compiled in (`_all` for the one
/// node without RBAC).
pub(crate) fn scope_key(node: &Owner) -> &str {
    match node {
        Owner::Organization(slug) => slug,
        Owner::Platform => PLATFORM_FOLDER,
        Owner::Unowned(_) => crate::rbac::scope::ModelScope::All.key(),
    }
}

fn counts(index: &ModelIndex, owner: &Owner, connections: usize) -> (Value, usize) {
    let errors = index.rejected.iter().filter(|(o, _)| belongs(owner, o)).count()
        + index.errors_in(scope_key(owner)).count();
    let cubes = index.cubes.iter().filter(|c| belongs(owner, &c.owner)).count();
    let files = index.files.iter().filter(|f| belongs(owner, &f.owner)).count();
    (
        json!({ "cubes": cubes, "files": files, "connections": connections }),
        errors,
    )
}

/// `GET /playground/tree?q=&limit=&offset=`
pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<TreeQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let index = index(&state).await?;

    let Some(rbac) = &state.rbac else {
        let all = Owner::Unowned(String::new());
        let (counts, errors) = counts(&index, &all, state.config.data_sources.len());
        return Ok(Json(json!({
            "organizations": [{ "slug": MODEL_NODE, "name": "Model", "counts": counts, "errors": errors }],
            "total": 1,
        })));
    };

    let q = query.q.filter(|q| !q.is_empty());
    let limit = clamp_limit(query.limit);
    let offset = query.offset.unwrap_or(0).max(0);
    let search = Search {
        q: q.clone(),
        limit: Some(limit),
        offset,
    };
    let page = rbac
        .store
        .search_organizations(None, &search)
        .await
        .map_err(store_error)?;

    let policy = current_policy();
    let connections = |slug: &str| {
        policy
            .as_ref()
            .and_then(|p| p.organizations.get(slug))
            .map_or(0, |s| s.len())
    };

    let mut organizations: Vec<Value> = page
        .items
        .iter()
        .map(|org| {
            let (counts, errors) = counts(
                &index,
                &Owner::Organization(org.slug.clone()),
                connections(&org.slug),
            );
            json!({ "id": org.id, "slug": org.slug, "name": org.name,
                    "counts": counts, "errors": errors })
        })
        .collect();

    // `_platform` closes the list, on the last page.
    let last_page = offset + page.items.len() as i64 >= page.total;
    let matches = q.as_deref().is_none_or(|q| {
        let q = q.to_lowercase();
        "platform".contains(&q) || PLATFORM_FOLDER.contains(&q)
    });
    if last_page && matches {
        let (counts, errors) = counts(
            &index,
            &Owner::Platform,
            policy.as_ref().map_or(0, |p| p.platform.len()),
        );
        organizations.push(json!({ "slug": PLATFORM_FOLDER, "name": "Platform",
                                   "counts": counts, "errors": errors }));
    }

    Ok(Json(json!({ "organizations": organizations, "total": page.total })))
}

/// Who a node of the tree is: its owner, and the data sources it holds.
async fn resolve(state: &AppState, slug: &str) -> Result<(Option<Owner>, Vec<String>), ApiError> {
    let unknown = || ApiError::not_found(format!("Unknown organization {slug}"));
    let policy = current_policy();
    let Some(rbac) = &state.rbac else {
        if slug != MODEL_NODE {
            return Err(unknown());
        }
        let mut names: Vec<String> = state.config.data_sources.iter().map(|(n, _)| n.clone()).collect();
        if names.is_empty() {
            names.push("default".into());
        }
        return Ok((None, names));
    };
    if slug == PLATFORM_FOLDER {
        let sources = policy.map_or_else(Vec::new, |p| p.platform.iter().cloned().collect());
        return Ok((Some(Owner::Platform), sources));
    }
    rbac.store
        .organization_by_slug(slug)
        .await
        .map_err(store_error)?
        .ok_or_else(unknown)?;
    let sources = policy
        .and_then(|p| p.organizations.get(slug).cloned())
        .map_or_else(Vec::new, |s| s.into_iter().collect());
    Ok((Some(Owner::Organization(slug.to_string())), sources))
}

/// `GET /playground/tree/:slug`
pub async fn detail(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(Json(org_detail(&state, &slug).await?))
}

/// The detail of one node of the tree (an organization's slug, or
/// `_platform`). With RBAC on, each file also carries `id`, `path`,
/// `version` and `editable`.
pub(crate) async fn org_detail(state: &AppState, slug: &str) -> Result<Value, ApiError> {
    org_detail_for(state, slug, false).await
}

/// [`org_detail`]; with `owner_only` the model's errors are redacted for an
/// owner who is not a platform admin (see `model_redact`).
pub(crate) async fn org_detail_for(
    state: &AppState,
    slug: &str,
    owner_only: bool,
) -> Result<Value, ApiError> {
    let (owner, sources) = resolve(state, slug).await?;
    let index = index(state).await?;
    // `None`: no policy, everything is in the one node.
    let mine = |o: &Owner| owner.as_ref().is_none_or(|owner| belongs(owner, o));
    let all = Owner::Unowned(String::new());
    let key = scope_key(owner.as_ref().unwrap_or(&all));

    let cubes: Vec<Value> = index
        .cubes
        .iter()
        .filter(|c| mine(&c.owner))
        .map(|c| {
            let members = |list: &[crate::model_index::IndexedMember]| -> Vec<Value> {
                list.iter()
                    .map(|m| json!({ "name": m.name, "type": m.r#type, "title": m.title,
                                     "primaryKey": m.primary_key, "public": m.public }))
                    .collect()
            };
            json!({ "name": c.name, "title": c.title, "type": c.kind,
                    "measures": members(&c.measure_list),
                    "dimensions": members(&c.dimension_list),
                    "segments": members(&c.segment_list),
                    "dataSource": c.data_source, "file": c.file,
                    "members": { "measures": c.measures, "dimensions": c.dimensions },
                    "joins": c.joins.iter()
                        .map(|(name, rel)| json!({ "name": name, "relationship": rel }))
                        .collect::<Vec<_>>(),
                    "includes": c.includes })
        })
        .collect();
    let rows = match &state.rbac {
        Some(rbac) => {
            let rows = rbac.store.model_rows().await.map_err(store_error)?;
            let slugs = rbac.store.organization_slugs().await.map_err(store_error)?;
            Some((rows, slugs))
        }
        None => None,
    };
    let by_source: std::collections::HashMap<String, &ModelFile> = rows
        .as_ref()
        .map(|(rows, slugs)| {
            rows.files
                .iter()
                .filter_map(|f| source_path(f, slugs).map(|p| (p, f)))
                .collect()
        })
        .unwrap_or_default();
    let files: Vec<Value> = index
        .files
        .iter()
        .filter(|f| mine(&f.owner))
        .map(|f| match by_source.get(&f.file_name) {
            Some(row) => json!({
                "fileName": f.file_name,
                "id": row.id,
                "path": row.path,
                "version": row.version,
                "editable": is_editable(&row.path, row.managed_by.as_deref()),
            }),
            None => json!({ "fileName": f.file_name }),
        })
        .collect();
    let mut errors: Vec<Value> = index
        .rejected
        .iter()
        .filter(|(o, _)| mine(o))
        .map(|(_, r)| r)
        .chain(index.errors_in(key))
        .map(|r| json!({ "file": r.file, "cube": r.cube, "message": r.message }))
        .collect();
    if owner_only {
        let problems: Vec<crate::rbac::model_validate::Problem> = index
            .rejected
            .iter()
            .filter(|(o, _)| mine(o))
            .map(|(_, r)| r)
            .chain(index.errors_in(key))
            .map(|r| crate::rbac::model_validate::Problem {
                file: r.file.clone(),
                cube: r.cube.clone(),
                message: r.message.clone(),
            })
            .collect();
        let defs = rows
            .as_ref()
            .map(|(rows, slugs)| {
                // The node's own scope: another organization may define the
                // same names.
                let scope = match &owner {
                    Some(Owner::Organization(slug)) => {
                        crate::rbac::scope::ModelScope::Organization(slug.clone())
                    }
                    _ => crate::rbac::scope::ModelScope::Platform,
                };
                let org_slugs = slugs.values().cloned().collect();
                crate::rbac::model_validate::definitions_in(
                    &crate::rbac::model_snapshot::sources(rows, slugs),
                    &scope,
                    &org_slugs,
                )
            })
            .unwrap_or_default();
        errors = crate::rbac::model_redact::redact(&problems, &[], slug, &defs)
            .into_iter()
            .map(|p| json!({ "file": p.file, "cube": p.cube, "message": p.message }))
            .collect();
    }

    // Only here, on expanding, are the databases asked for their tables.
    let results = futures::future::join_all(
        sources
            .iter()
            .map(|s| super::playground::data_source_schema(s)),
    )
    .await;
    let mut tables = serde_json::Map::new();
    for (name, result) in sources.iter().zip(results) {
        match result {
            Ok(structure) => {
                tables.insert(name.clone(), json!(structure));
            }
            Err(e) => errors.push(json!({ "file": "", "cube": "", "message": e.body.error })),
        }
    }

    Ok(json!({ "cubes": cubes, "tables": tables, "files": files, "errors": errors }))
}

/// `GET /playground/tree/:slug/files/*path` — `path` is a `fileName` from the
/// listing: relative to the model directory, inside the node's folder.
pub async fn file(
    State(state): State<AppState>,
    Path((slug, path)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let (owner, _) = resolve(&state, &slug).await?;

    let rel = FsPath::new(&path);
    // The files the index lists: the loader's model files.
    let clean = !path.is_empty()
        && rel.components().all(|c| matches!(c, Component::Normal(_)))
        && cubemodel::is_model_file(rel);
    if !clean {
        return Err(ApiError::bad_request("Invalid file path"));
    }

    let root = FsPath::new(&state.config.schema_path);
    let folder = match &owner {
        None => root.to_path_buf(),
        Some(Owner::Platform) => root.join(PLATFORM_FOLDER),
        Some(Owner::Organization(slug)) => root.join(slug),
        Some(Owner::Unowned(_)) => unreachable!("a node is never unowned"),
    };
    let not_found = || ApiError::not_found(format!("No file {path}"));
    // Resolved, so a symlink cannot lead out of the folder.
    let folder = folder.canonicalize().map_err(|_| not_found())?;
    let abs = root.join(rel).canonicalize().map_err(|_| not_found())?;
    if !abs.starts_with(&folder) || !abs.is_file() {
        return Err(not_found());
    }
    let content = std::fs::read_to_string(&abs).map_err(|_| not_found())?;

    Ok(Json(json!({
        "fileName": path,
        "content": content,
        "absPath": abs.to_string_lossy(),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limit_defaults_to_50_and_stops_at_200() {
        assert_eq!(clamp_limit(None), 50);
        assert_eq!(clamp_limit(Some(100_000)), 200);
        assert_eq!(clamp_limit(Some(-3)), 0);
        assert_eq!(clamp_limit(Some(7)), 7);
    }
}
