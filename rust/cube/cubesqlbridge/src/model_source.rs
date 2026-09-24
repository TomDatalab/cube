//! Where the data model is read from.
//!
//! The same source feeds two different representations of one model:
//! [`cubemodel::DataModel`], which is plain data and crosses threads freely and
//! is what the `/v1/meta` view is built from, and [`cubeplanner::Model`], which
//! is built on `Rc` and stays on the thread that created it.

use std::path::{Path, PathBuf};

use cubesql::CubeError;

/// A data model, either on disk or inline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelSource {
    /// A model directory, as `CUBEJS_SCHEMA_PATH` points at.
    Dir(PathBuf),
    /// One YAML document, which is what most tests want.
    Yaml(String),
}

impl ModelSource {
    pub fn dir(path: impl AsRef<Path>) -> Self {
        Self::Dir(path.as_ref().to_path_buf())
    }

    pub fn yaml(source: impl Into<String>) -> Self {
        Self::Yaml(source.into())
    }

    /// The loader's view of the model: cubes, views and their meta config.
    pub fn load_data_model(&self) -> Result<cubemodel::DataModel, CubeError> {
        match self {
            Self::Dir(path) => cubemodel::ModelLoader::load_dir(path),
            Self::Yaml(source) => cubemodel::ModelLoader::load_str(source, "model.yml"),
        }
        .map_err(|e| CubeError::user(format!("Failed to compile the data model: {e}")))
    }

    /// The planner's view of the model. Not `Send`, so it never leaves the
    /// thread that loads it - see [`crate::planner::PlannerPool`].
    pub fn load_planner_model(&self) -> Result<cubeplanner::Model, CubeError> {
        match self {
            Self::Dir(path) => cubeplanner::Model::from_dir(path),
            Self::Yaml(source) => cubeplanner::Model::from_yaml_str(source),
        }
        .map_err(|e| CubeError::user(format!("Failed to compile the data model: {}", e.message())))
    }
}
