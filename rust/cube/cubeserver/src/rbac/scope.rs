//! Model scopes: the unit a model runtime compiles.
//!
//! With RBAC on, an organization's scope is `_platform/**` plus its own
//! folder; the Platform scope is `_platform/**` plus every file no
//! organization owns. Without RBAC there is one `All` scope.

use std::collections::BTreeSet;

use cubemodel::ownership::PLATFORM_FOLDER;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ModelScope {
    All,
    Platform,
    Organization(String),
}

impl ModelScope {
    pub fn key(&self) -> &str {
        match self {
            ModelScope::All => "_all",
            ModelScope::Platform => PLATFORM_FOLDER,
            ModelScope::Organization(slug) => slug,
        }
    }
}

/// First path segment, or `None` for a file at the root.
fn owner_folder(path: &str) -> Option<&str> {
    path.split_once('/').map(|(folder, _)| folder)
}

/// Whether `scope` compiles the file at `path` (relative to the model root).
pub fn in_scope(path: &str, scope: &ModelScope, org_slugs: &BTreeSet<String>) -> bool {
    let owner = owner_folder(path);
    match scope {
        ModelScope::All => true,
        ModelScope::Platform => {
            owner == Some(PLATFORM_FOLDER) || owner.map_or(true, |o| !org_slugs.contains(o))
        }
        ModelScope::Organization(slug) => owner == Some(PLATFORM_FOLDER) || owner == Some(slug),
    }
}

/// The `(path, content)` subset that `scope` compiles.
pub fn scope_sources(
    sources: &[(String, String)],
    scope: &ModelScope,
    org_slugs: &BTreeSet<String>,
) -> Vec<(String, String)> {
    sources
        .iter()
        .filter(|(path, _)| in_scope(path, scope, org_slugs))
        .cloned()
        .collect()
}

/// Scopes whose compiled model changes when `paths` change. A `_platform/`
/// or unowned root path reaches Platform and every organization.
pub fn scopes_touched(paths: &[&str], org_slugs: &BTreeSet<String>) -> BTreeSet<ModelScope> {
    let mut touched = BTreeSet::new();
    for path in paths {
        match owner_folder(path) {
            Some(PLATFORM_FOLDER) | None => {
                touched.insert(ModelScope::Platform);
                touched.extend(org_slugs.iter().cloned().map(ModelScope::Organization));
            }
            Some(owner) if org_slugs.contains(owner) => {
                touched.insert(ModelScope::Organization(owner.to_string()));
            }
            Some(_) => {
                touched.insert(ModelScope::Platform);
            }
        }
    }
    touched
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slugs(list: &[&str]) -> BTreeSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn paths(v: Vec<(String, String)>) -> Vec<String> {
        v.into_iter().map(|(p, _)| p).collect()
    }

    #[test]
    fn scope_sources_keeps_platform_and_own_folder() {
        let orgs = slugs(&["digital", "on"]);
        let src: Vec<(String, String)> =
            ["digital/a.yml", "on/a.yml", "_platform/u.yml", "stray/x.yml", "root.yml"]
                .iter()
                .map(|p| (p.to_string(), "c".to_string()))
                .collect();
        let org = ModelScope::Organization("digital".into());
        assert_eq!(paths(scope_sources(&src, &org, &orgs)), ["digital/a.yml", "_platform/u.yml"]);
        assert_eq!(
            paths(scope_sources(&src, &ModelScope::Platform, &orgs)),
            ["_platform/u.yml", "stray/x.yml", "root.yml"]
        );
        assert_eq!(scope_sources(&src, &ModelScope::All, &orgs).len(), 5);
    }

    #[test]
    fn scopes_touched_platform_reaches_every_org() {
        let orgs = slugs(&["digital", "on"]);
        for p in ["_platform/u.yml", "root.yml"] {
            let t = scopes_touched(&[p], &orgs);
            assert_eq!(t.len(), 3, "{p}");
            assert!(t.contains(&ModelScope::Platform));
            assert!(t.contains(&ModelScope::Organization("on".into())));
        }
    }

    #[test]
    fn scopes_touched_org_file_only_its_org() {
        let t = scopes_touched(&["digital/a.yml"], &slugs(&["digital", "on"]));
        assert_eq!(t, BTreeSet::from([ModelScope::Organization("digital".into())]));
    }

    #[test]
    fn keys() {
        assert_eq!(ModelScope::All.key(), "_all");
        assert_eq!(ModelScope::Platform.key(), "_platform");
        assert_eq!(ModelScope::Organization("on".into()).key(), "on");
    }
}
