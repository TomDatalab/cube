//! An index of the model directory by owner, for the Playground's tree.
//!
//! The index reads the YAML files, applies the ownership policy
//! (`cubemodel::ownership`) and keeps each cube, file and rejection with the
//! owner its folder gives it. It is cached until a file changes or the policy
//! is replaced.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use cubemodel::ownership::{current_policy, Owner, Rejection};
use cubemodel::{ModelError, ModelLoader};

#[derive(Debug, Clone)]
pub struct IndexedCube {
    pub name: String,
    pub title: Option<String>,
    /// `"cube"` or `"view"`.
    pub kind: &'static str,
    pub data_source: String,
    pub file: String,
    pub owner: Owner,
    pub measures: usize,
    pub dimensions: usize,
    /// `(target cube, relationship)`.
    pub joins: Vec<(String, Option<String>)>,
    /// A view's source cubes: every segment of every `join_path`, deduped.
    pub includes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct IndexedFile {
    pub file_name: String,
    pub owner: Owner,
}

#[derive(Debug, Clone, Default)]
pub struct ModelIndex {
    pub cubes: Vec<IndexedCube>,
    pub files: Vec<IndexedFile>,
    pub rejected: Vec<(Owner, Rejection)>,
}

/// The YAML files under `dir`, by path relative to `root` (the rule of
/// `/playground/files`).
fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, out);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("yml") | Some("yaml")
        ) {
            let name = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            out.push((name, std::fs::read_to_string(&path).unwrap_or_default()));
        }
    }
}

impl ModelIndex {
    pub fn load(model_path: &Path) -> Result<ModelIndex, ModelError> {
        let policy = current_policy();
        let owner_of = |file: &str| match &policy {
            Some(p) => p.owner_of_file(file),
            None => Owner::Unowned(String::new()),
        };

        let mut sources = Vec::new();
        collect(model_path, model_path, &mut sources);
        sources.sort_by(|a, b| a.0.cmp(&b.0));

        // Not validated: a file that does not validate must not hide the
        // index. The policy is applied all the same.
        let model = ModelLoader::new()
            .without_validation()
            .load_sources(&sources)?;

        let cubes = model
            .cube_list()
            .into_iter()
            .map(|c| IndexedCube {
                name: c.name.clone(),
                title: c.title.clone(),
                kind: if c.is_view { "view" } else { "cube" },
                data_source: c.data_source.clone().unwrap_or_else(|| "default".into()),
                file: c.file_name.clone(),
                owner: owner_of(&c.file_name),
                measures: c.measures.len(),
                dimensions: c.dimensions.len(),
                joins: c
                    .joins
                    .iter()
                    .map(|j| (j.name.clone(), j.relationship.clone()))
                    .collect(),
                includes: {
                    let mut names: Vec<String> = Vec::new();
                    for part in c
                        .cubes
                        .iter()
                        .filter_map(|i| i.join_path.as_deref())
                        .flat_map(|p| p.split('.'))
                    {
                        if !part.is_empty() && !names.iter().any(|n| n == part) {
                            names.push(part.to_string());
                        }
                    }
                    names
                },
            })
            .collect();
        let files = sources
            .iter()
            .map(|(name, _)| IndexedFile {
                file_name: name.clone(),
                owner: owner_of(name),
            })
            .collect();
        let rejected = model
            .rejected
            .iter()
            .map(|r| (owner_of(&r.file), r.clone()))
            .collect();

        Ok(ModelIndex {
            cubes,
            files,
            rejected,
        })
    }
}

/// Newest mtime, a hash of the file names (a delete or a move changes no
/// file's mtime) and the policy generation.
type CacheKey = (Option<SystemTime>, u64, u64);

fn files_hash(model_path: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut names = Vec::new();
    let mut sources = Vec::new();
    collect_names(model_path, model_path, &mut sources);
    names.extend(sources);
    names.sort();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    names.hash(&mut hasher);
    hasher.finish()
}

fn collect_names(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_names(root, &path, out);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("yml") | Some("yaml")
        ) {
            out.push(
                path.strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
}

/// The index, rebuilt when the newest file changes or the policy does.
#[derive(Default)]
pub struct ModelIndexCache {
    cached: Mutex<Option<(CacheKey, Arc<ModelIndex>)>>,
}

impl std::fmt::Debug for ModelIndexCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ModelIndexCache")
    }
}

impl ModelIndexCache {
    pub fn get(&self, model_path: &Path) -> Result<Arc<ModelIndex>, ModelError> {
        let key = (
            crate::meta_adapter::newest_mtime(model_path),
            files_hash(model_path),
            current_policy().map(|p| p.generation).unwrap_or(0),
        );
        let mut cached = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((k, index)) = cached.as_ref() {
            if *k == key {
                return Ok(index.clone());
            }
        }
        let index = Arc::new(ModelIndex::load(model_path)?);
        *cached = Some((key, index.clone()));
        Ok(index)
    }
}
