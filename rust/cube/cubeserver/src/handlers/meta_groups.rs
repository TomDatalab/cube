//! `GET /v1/meta/groups` — the cubes and views `/v1/meta` shows the caller,
//! grouped by the organization whose folder holds them and, inside it, by
//! data source. The Playground's query builder and its Data Source page list
//! them this way.
//!
//! It answers whoever may read `/v1/meta`, with the same scope and the same
//! workspace: it only groups what that meta shows, so it names no cube the
//! caller could not see. The model's errors are counted for platform admins
//! (and everyone without RBAC) only, as the Data Model page does.

use std::collections::{BTreeMap, HashMap};

use axum::extract::State;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use cubemodel::ownership::{Owner, PLATFORM_FOLDER};
use serde_json::{json, Value};

use crate::app::AppState;
use crate::error::ApiError;
use crate::handlers::auth::{assert_api_scope, ForContext};
use crate::handlers::tree::{belongs, index};
use crate::rbac::scope::ModelScope;
use crate::services::RequestContext;

/// The one group without RBAC: every cube is in it.
const MODEL_GROUP: &str = "_model";
/// A cube the meta shows that the index does not know (the index lags a
/// reload): listed rather than put under an owner it may not have.
const OTHER_GROUP: &str = "_other";

/// Where a group sorts: Platform first, then the organizations by name, then
/// what has no known owner.
fn rank(slug: &str) -> u8 {
    match slug {
        PLATFORM_FOLDER => 0,
        OTHER_GROUP => 2,
        _ => 1,
    }
}

pub async fn groups(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
) -> Result<impl IntoResponse, ApiError> {
    assert_api_scope(&state, &ctx, "meta").await?;

    let meta = state
        .meta_for(&ctx)
        .await?
        .meta(&ctx, false)
        .await
        .map_err(|e| e.for_context(&state, &ctx))?;
    let visible: Vec<&str> = meta["cubes"]
        .as_array()
        .map(|cubes| cubes.iter().filter_map(|c| c["name"].as_str()).collect())
        .unwrap_or_default();

    // A name is unique within a scope, not across them: each cube is looked
    // up among the owners of the request's scope only.
    let scope = state.scope_for(&ctx).await?;
    let in_scope = |owner: &Owner| match &scope {
        ModelScope::All => true,
        ModelScope::Platform => matches!(owner, Owner::Platform | Owner::Unowned(_)),
        ModelScope::Organization(slug) => match owner {
            Owner::Platform => true,
            Owner::Organization(o) => o == slug,
            Owner::Unowned(_) => false,
        },
    };
    let index = index(&state).await?;
    let indexed: HashMap<&str, _> = index
        .cubes
        .iter()
        .filter(|c| in_scope(&c.owner))
        .map(|c| (c.name.as_str(), c))
        .collect();

    // slug -> data source -> cube names.
    let mut grouped: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    for name in visible {
        let (slug, data_source) = match (indexed.get(name), &state.rbac) {
            (Some(cube), None) => (MODEL_GROUP.to_string(), cube.data_source.clone()),
            (Some(cube), Some(_)) => match &cube.owner {
                Owner::Organization(slug) => (slug.clone(), cube.data_source.clone()),
                Owner::Platform => (PLATFORM_FOLDER.to_string(), cube.data_source.clone()),
                Owner::Unowned(_) => (OTHER_GROUP.to_string(), cube.data_source.clone()),
            },
            (None, None) => (MODEL_GROUP.to_string(), "default".to_string()),
            (None, Some(_)) => (OTHER_GROUP.to_string(), "default".to_string()),
        };
        grouped
            .entry(slug)
            .or_default()
            .entry(data_source)
            .or_default()
            .push(name.to_string());
    }

    // Errors are the admin's: an organization with nothing to show but
    // rejected cubes still gets its group, so the admin sees why.
    let show_errors = state.rbac.is_none() || ctx.platform_admin;
    let owner_of = |slug: &str| match slug {
        PLATFORM_FOLDER => Some(Owner::Platform),
        MODEL_GROUP => Some(Owner::Unowned(String::new())),
        OTHER_GROUP => None,
        slug => Some(Owner::Organization(slug.to_string())),
    };
    let errors_of = |slug: &str| -> usize {
        let Some(owner) = owner_of(slug) else {
            return 0;
        };
        let rejected = if slug == MODEL_GROUP {
            index.rejected.len()
        } else {
            index
                .rejected
                .iter()
                .filter(|(o, _)| in_scope(o) && belongs(&owner, o))
                .count()
        };
        rejected + index.errors_in(scope.key()).count()
    };
    if show_errors && state.rbac.is_some() {
        for (owner, _) in index.rejected.iter().filter(|(o, _)| in_scope(o)) {
            let slug = match owner {
                Owner::Organization(slug) => slug.clone(),
                Owner::Platform | Owner::Unowned(_) => PLATFORM_FOLDER.to_string(),
            };
            grouped.entry(slug).or_default();
        }
    }

    let mut groups: Vec<(u8, String, Value)> = Vec::with_capacity(grouped.len());
    for (slug, sources) in grouped {
        let (id, name) = match (slug.as_str(), &state.rbac) {
            (MODEL_GROUP, _) => (Value::Null, "Model".to_string()),
            (PLATFORM_FOLDER, _) => (Value::Null, "Platform".to_string()),
            (OTHER_GROUP, _) => (Value::Null, "Other".to_string()),
            (slug, Some(rbac)) => match rbac.store.organization_by_slug(slug).await {
                Ok(Some(org)) => (json!(org.id), org.name),
                Ok(None) => (Value::Null, slug.to_string()),
                Err(e) => {
                    tracing::error!("The RBAC database cannot name organization {slug}: {e}");
                    return Err(crate::rbac::unavailable());
                }
            },
            (slug, None) => (Value::Null, slug.to_string()),
        };
        let data_sources: Vec<Value> = sources
            .into_iter()
            .map(|(ds, mut cubes)| {
                cubes.sort();
                json!({ "name": ds, "cubes": cubes })
            })
            .collect();
        let errors = if show_errors { json!(errors_of(&slug)) } else { Value::Null };
        groups.push((
            rank(&slug),
            name.to_lowercase(),
            json!({ "slug": slug, "id": id, "name": name, "errors": errors,
                    "dataSources": data_sources }),
        ));
    }
    groups.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));

    Ok(Json(json!({
        "groups": groups.into_iter().map(|(_, _, g)| g).collect::<Vec<_>>(),
    })))
}
