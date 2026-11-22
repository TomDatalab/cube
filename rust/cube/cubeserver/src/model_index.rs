//! An index of the model directory by owner, for the Playground's tree.
//!
//! The index reads the YAML files, applies the ownership policy
//! (`cubemodel::ownership`) and keeps each cube, file and rejection with the
//! owner its folder gives it. It is cached until a file changes or the policy
//! is replaced.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use cubemodel::ownership::{current_policy, Owner, OwnershipPolicy, Rejection};
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
    /// Why the model did not compile (then `cubes` and `rejected` are
    /// empty). They concern the whole model, so every node shows them.
    pub errors: Vec<Rejection>,
}

/// The model files under `model_path` (the loader's own list:
/// `cubemodel::model_files`), by path relative to it. A directory that cannot
/// be read is an empty model.
fn model_file_names(model_path: &Path) -> Vec<(String, std::path::PathBuf)> {
    cubemodel::model_files(model_path)
        .unwrap_or_default()
        .into_iter()
        .map(|path| {
            let name = path
                .strip_prefix(model_path)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            (name, path)
        })
        .collect()
}

/// What a failed compile says, one entry per problem, with the file when it
/// is known ("" otherwise) and no cube.
fn compile_errors(error: &ModelError) -> Vec<Rejection> {
    let entry = |file: &str, message: String| Rejection {
        file: file.to_string(),
        cube: String::new(),
        message,
    };
    match error {
        ModelError::Compile(items) if !items.is_empty() => items
            .iter()
            .map(|i| entry(i.file_name.as_deref().unwrap_or(""), i.full_message()))
            .collect(),
        ModelError::Template { file_name, message }
        | ModelError::UnsupportedModelFile { file_name, message } => {
            vec![entry(file_name, message.clone())]
        }
        other => vec![entry("", other.to_string())],
    }
}

impl ModelIndex {
    /// The index with the policy installed now.
    pub fn load(model_path: &Path) -> ModelIndex {
        Self::load_with(model_path, current_policy().as_deref())
    }

    /// The index under `policy`: owners and rejections both come from it.
    ///
    /// A model that does not compile still lists its files by folder; the
    /// compile errors go to [`ModelIndex::errors`] and no cube is listed.
    pub fn load_with(model_path: &Path, policy: Option<&OwnershipPolicy>) -> ModelIndex {
        let owner_of = |file: &str| match policy {
            Some(p) => p.owner_of_file(file),
            None => Owner::Unowned(String::new()),
        };

        let sources: Vec<(String, String)> = model_file_names(model_path)
            .into_iter()
            .map(|(name, path)| (name, std::fs::read_to_string(&path).unwrap_or_default()))
            .collect();
        let files = sources
            .iter()
            .map(|(name, _)| IndexedFile {
                file_name: name.clone(),
                owner: owner_of(name),
            })
            .collect();

        // Not validated: a file that does not validate must not hide the
        // index. The policy is applied all the same. Templates render with
        // the default context (no tenant's `COMPILE_CONTEXT`).
        let model = match ModelLoader::new()
            .without_validation()
            .load_sources_with_policy(&sources, policy)
        {
            Ok(model) => model,
            Err(e) => {
                return ModelIndex {
                    files,
                    errors: compile_errors(&e),
                    ..ModelIndex::default()
                }
            }
        };

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
        let rejected = model
            .rejected
            .iter()
            .map(|r| (owner_of(&r.file), r.clone()))
            .collect();

        ModelIndex {
            cubes,
            files,
            rejected,
            errors: Vec::new(),
        }
    }
}

/// What a model directory looks like for change detection: its newest mtime
/// and a hash of its model file names (a delete or a move changes no file's
/// mtime). The tree index and the model reload loops share it, so they agree
/// on when the model changed.
pub type ModelStamp = (Option<SystemTime>, u64);

pub fn model_stamp(model_path: &Path) -> ModelStamp {
    use std::hash::{Hash, Hasher};
    let names: Vec<String> = model_file_names(model_path)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    names.hash(&mut hasher);
    (crate::meta_adapter::newest_mtime(model_path), hasher.finish())
}

/// The model's stamp and the policy generation (0: none).
type CacheKey = (ModelStamp, u64);

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
    pub fn get(&self, model_path: &Path) -> Arc<ModelIndex> {
        // One snapshot of the policy for the key and the index.
        let policy = current_policy();
        let key = (
            model_stamp(model_path),
            policy.as_ref().map_or(0, |p| p.generation),
        );
        let mut cached = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((k, index)) = cached.as_ref() {
            if *k == key {
                return index.clone();
            }
        }
        let index = Arc::new(ModelIndex::load_with(model_path, policy.as_deref()));
        *cached = Some((key, index.clone()));
        index
    }
}
