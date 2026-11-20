//! Folder ownership policy: the first folder of a model file decides which
//! organization owns the cubes in it. Misplaced cubes are rejected so one of
//! them never breaks the rest of the model.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::{Arc, RwLock};

use serde::Serialize;

use crate::model::DataModel;

pub const PLATFORM_FOLDER: &str = "_platform";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct OwnershipPolicy {
    /// slug -> data sources of that organization.
    pub organizations: BTreeMap<String, BTreeSet<String>>,
    /// Data sources without an organization (`default` included when it applies).
    pub platform: BTreeSet<String>,
    /// Changes with every new policy.
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Owner {
    Organization(String),
    Platform,
    /// First path segment, or "" for a file at the root.
    Unowned(String),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Rejection {
    pub file: String,
    pub cube: String,
    pub message: String,
}

impl OwnershipPolicy {
    pub fn owner_of_file(&self, file_name: &str) -> Owner {
        let Some((seg, _)) = file_name.split_once('/') else {
            return Owner::Unowned(String::new());
        };
        if seg == PLATFORM_FOLDER {
            Owner::Platform
        } else if self.organizations.contains_key(seg) {
            Owner::Organization(seg.to_string())
        } else {
            Owner::Unowned(seg.to_string())
        }
    }

    pub fn owner_of_data_source(&self, ds: &str) -> Option<Owner> {
        for (slug, sources) in &self.organizations {
            if sources.contains(ds) {
                return Some(Owner::Organization(slug.clone()));
            }
        }
        self.platform.contains(ds).then_some(Owner::Platform)
    }
}

static POLICY: RwLock<Option<Arc<OwnershipPolicy>>> = RwLock::new(None);

/// Sets (or clears) the process-wide policy.
pub fn set_policy(policy: Option<Arc<OwnershipPolicy>>) {
    *POLICY.write().unwrap_or_else(|e| e.into_inner()) = policy;
}

pub fn current_policy() -> Option<Arc<OwnershipPolicy>> {
    POLICY.read().unwrap_or_else(|e| e.into_inner()).clone()
}

fn owner_label(owner: &Owner) -> String {
    match owner {
        Owner::Organization(slug) => slug.clone(),
        Owner::Platform => PLATFORM_FOLDER.to_string(),
        Owner::Unowned(seg) => seg.clone(),
    }
}

/// Removes the cubes and views that do not respect `policy` from `model` and
/// returns why, sorted by `(file, cube)`.
pub fn apply(model: &mut DataModel, policy: &OwnershipPolicy) -> Vec<Rejection> {
    let mut rejections: Vec<Rejection> = Vec::new();
    let mut rejected: HashSet<String> = HashSet::new();

    // Owner of every cube, computed before anything is removed.
    let owners: BTreeMap<String, Owner> = model
        .cube_list()
        .into_iter()
        .map(|c| (c.name.clone(), policy.owner_of_file(&c.file_name)))
        .collect();

    // 1. By the file's owner and the data source.
    for cube in model.cube_list() {
        let ds = cube.data_source.as_deref().unwrap_or("default");
        let message = match &owners[&cube.name] {
            Owner::Unowned(seg) => Some(format!(
                "the folder {seg}/ does not belong to any organization"
            )),
            // A view has no connection of its own: only its folder and the
            // cubes it includes matter.
            Owner::Organization(_) | Owner::Platform if cube.is_view => None,
            Owner::Organization(slug) => {
                let ok = policy
                    .organizations
                    .get(slug)
                    .is_some_and(|s| s.contains(ds));
                (!ok).then(|| format!("data source {ds} is not a connection of {slug}"))
            }
            Owner::Platform => matches!(
                policy.owner_of_data_source(ds),
                Some(Owner::Organization(_))
            )
            .then(|| {
                format!("data source {ds} belongs to an organization; move this file to its folder")
            }),
        };
        if let Some(message) = message {
            rejected.insert(cube.name.clone());
            rejections.push(Rejection {
                file: cube.file_name.clone(),
                cube: cube.name.clone(),
                message,
            });
        }
    }

    // 2. Cross references, until nothing changes.
    loop {
        let mut round: Vec<Rejection> = Vec::new();
        for cube in model.cube_list() {
            if rejected.contains(&cube.name) {
                continue;
            }
            let mine = &owners[&cube.name];
            let mut refs: Vec<(&str, String)> = Vec::new();
            for j in &cube.joins {
                refs.push(("joins", j.name.clone()));
            }
            if let Some(e) = &cube.extends {
                refs.push(("extends", e.clone()));
            }
            for inc in &cube.cubes {
                if let Some(path) = &inc.join_path {
                    for seg in path.split('.') {
                        refs.push(("includes", seg.to_string()));
                    }
                }
            }
            for (kind, other) in refs {
                if other == cube.name {
                    continue;
                }
                let Some(other_owner) = owners.get(&other) else {
                    continue;
                };
                let message = if other_owner != mine {
                    Some(format!(
                        "{kind} {other} belongs to {}",
                        owner_label(other_owner)
                    ))
                } else if rejected.contains(&other) {
                    Some(format!("depends on rejected cube {other}"))
                } else {
                    None
                };
                if let Some(message) = message {
                    round.push(Rejection {
                        file: cube.file_name.clone(),
                        cube: cube.name.clone(),
                        message,
                    });
                    break;
                }
            }
        }
        if round.is_empty() {
            break;
        }
        for r in round {
            rejected.insert(r.cube.clone());
            rejections.push(r);
        }
    }

    for name in &rejected {
        model.cubes.shift_remove(name);
        model.views.shift_remove(name);
    }
    rejections.sort_by(|a, b| (&a.file, &a.cube).cmp(&(&b.file, &b.cube)));
    rejections
}
