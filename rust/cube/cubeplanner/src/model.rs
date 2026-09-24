//! Loading a Cube data model from YAML.

use crate::error::PlannerError;
use cubenativeutils::CubeError;
use cubesqlplanner::cube_bridge::cube_definition::CubeDefinition;
use cubesqlplanner::cube_bridge::dimension_definition::DimensionDefinition;
use cubesqlplanner::cube_bridge::measure_definition::MeasureDefinition;
use cubesqlplanner::cube_bridge::member_sql::MemberSql;
use cubesqlplanner::cube_bridge::segment_definition::SegmentDefinition;
use cubesqlplanner::rust_model::MockSchema;
use serde_yaml::{Mapping, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// A parsed, JavaScript-free Cube data model: cubes, views, their members,
/// joins, granularities and pre-aggregations.
///
/// A `Model` is request-independent — every member `sql` is parsed once, here,
/// and only bound to a security context and a dialect when [`crate::plan`]
/// runs. It is therefore cheap to keep one `Model` for the process and plan
/// many queries against it.
#[derive(Clone)]
pub struct Model {
    schema: MockSchema,
}

impl Model {
    /// Parses one YAML document holding `cubes:` and/or `views:`.
    pub fn from_yaml_str(yaml: &str) -> Result<Self, PlannerError> {
        Self::from_yaml_documents(std::iter::once(("<input>", yaml)))
    }

    /// Loads every `.yml` / `.yaml` file under `dir`, recursively, and merges
    /// them into one model — the layout `model/cubes/*.yml` uses.
    ///
    /// A JavaScript or TypeScript data-model file is reported rather than
    /// silently skipped: this planner evaluates no JavaScript, so a model
    /// relying on one would plan differently than the caller expects.
    pub fn from_dir(dir: impl AsRef<Path>) -> Result<Self, PlannerError> {
        let dir = dir.as_ref();
        let mut files = Vec::new();
        collect_model_files(dir, &mut files)?;
        files.sort();

        if files.is_empty() {
            return Err(PlannerError::model(format!(
                "No YAML data model files found under `{}`",
                dir.display()
            )));
        }

        Self::from_dir_with_context(dir, &cubemodel::TemplateContext::default())
    }

    /// As [`Model::from_dir`], with the `COMPILE_CONTEXT` the templates see.
    ///
    /// A multi-tenant deployment compiles one model per tenant, so the context
    /// has to reach the planner too — otherwise `/v1/meta` and `/v1/sql` would
    /// answer for different models.
    pub fn from_dir_with_context(
        dir: impl AsRef<Path>,
        context: &cubemodel::TemplateContext,
    ) -> Result<Self, PlannerError> {
        let dir = dir.as_ref();
        let mut files = Vec::new();
        collect_model_files(dir, &mut files)?;
        files.sort();

        if files.is_empty() {
            return Err(PlannerError::model(format!(
                "No YAML data model files found under `{}`",
                dir.display()
            )));
        }

        let mut documents = Vec::with_capacity(files.len());
        for path in &files {
            let content = std::fs::read_to_string(path).map_err(|e| {
                PlannerError::model(format!("Failed to read `{}`: {}", path.display(), e))
            })?;
            documents.push((path.display().to_string(), content));
        }

        Self::from_yaml_documents_with_context(
            documents
                .iter()
                .map(|(name, content)| (name.as_str(), content.as_str())),
            context,
        )
    }

    /// Merges several YAML documents into one model. Each is named, so a parse
    /// error points at the file it came from.
    pub fn from_yaml_documents<'a>(
        documents: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<Self, PlannerError> {
        Self::from_yaml_documents_with_context(documents, &cubemodel::TemplateContext::default())
    }

    /// As [`Model::from_yaml_documents`], with the `COMPILE_CONTEXT` the
    /// templates see.
    pub fn from_yaml_documents_with_context<'a>(
        documents: impl IntoIterator<Item = (&'a str, &'a str)>,
        context: &cubemodel::TemplateContext,
    ) -> Result<Self, PlannerError> {
        let mut cubes = Vec::new();
        let mut views = Vec::new();
        // The same environment `cubemodel` renders with, so a model that
        // compiles for `/v1/meta` also plans.
        let env = cubemodel::jinja::build_environment(false);

        for (name, content) in documents {
            let rendered = if cubemodel::jinja::has_jinja_syntax(content) {
                cubemodel::jinja::render(&env, content, context)
                    .map_err(|e| PlannerError::model(format!("Failed to render `{name}`: {e}")))?
            } else {
                content.to_string()
            };

            let value: Value = serde_yaml::from_str(&rendered).map_err(|e| {
                PlannerError::model(format!("Failed to parse YAML in `{name}`: {e}"))
            })?;
            let mapping = match value {
                Value::Null => continue,
                Value::Mapping(mapping) => mapping,
                _ => {
                    return Err(PlannerError::model(format!(
                        "`{name}` must be a YAML mapping with `cubes:` and/or `views:`"
                    )))
                }
            };
            take_sequence(&mapping, "cubes", name, &mut cubes)?;
            take_sequence(&mapping, "views", name, &mut views)?;
        }

        let mut merged = Mapping::new();
        merged.insert(Value::from("cubes"), Value::Sequence(cubes));
        merged.insert(Value::from("views"), Value::Sequence(views));

        // Strict, eager checking happens before the model is built: the
        // builder drops unknown keys, parses member `sql` lazily and panics on
        // an unparseable pre-aggregation reference list, so every one of those
        // has to be caught here, where the cube and member names are known.
        crate::model_check::check_model(&merged)?;

        let yaml = serde_yaml::to_string(&Value::Mapping(merged))
            .map_err(|e| PlannerError::model(format!("Failed to merge data model: {e}")))?;

        let schema = MockSchema::from_yaml(&yaml).map_err(|e| {
            PlannerError::model(format!("Failed to build data model: {}", e.message))
        })?;

        let model = Self { schema };
        model.validate()?;
        Ok(model)
    }

    /// Parses every member `sql:` in the model up front, so a construct this
    /// planner cannot express is reported here — naming the cube and the
    /// member — instead of surfacing halfway through planning a query.
    ///
    /// Covers cube `sql:` / `sql_table:`, every measure and dimension `sql:`
    /// and `mask:`, and every segment `sql:`.
    fn validate(&self) -> Result<(), PlannerError> {
        for cube_name in self.cube_names() {
            let Some(cube) = self.schema.get_cube(&cube_name) else {
                continue;
            };

            check(&cube_name, "sql", cube.definition.sql())?;
            check(&cube_name, "sql_table", cube.definition.sql_table())?;

            for (name, measure) in sorted(&cube.measures) {
                check(&cube_name, &format!("measure `{name}` sql"), measure.sql())?;
                check(
                    &cube_name,
                    &format!("measure `{name}` mask"),
                    measure.mask_sql(),
                )?;
            }
            for (name, dimension) in sorted(&cube.dimensions) {
                check(
                    &cube_name,
                    &format!("dimension `{name}` sql"),
                    dimension.sql(),
                )?;
                check(
                    &cube_name,
                    &format!("dimension `{name}` mask"),
                    dimension.mask_sql(),
                )?;
            }
            for (name, segment) in sorted(&cube.segments) {
                check(
                    &cube_name,
                    &format!("segment `{name}` sql"),
                    segment.sql().map(Some),
                )?;
            }
        }
        Ok(())
    }

    /// Names of every cube and view in the model.
    pub fn cube_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.schema.cube_names().into_iter().cloned().collect();
        names.sort();
        names
    }

    /// The model's join graph. `build_join` on it is Cube's `buildJoin`:
    /// shortest join path from every hint, fewest joins wins, with the
    /// multiplication factor per cube.
    pub fn join_graph(&self) -> Result<crate::join_graph::JoinGraph, PlannerError> {
        Ok(self.schema.create_join_graph()?)
    }

    pub(crate) fn schema(&self) -> &MockSchema {
        &self.schema
    }
}

impl std::fmt::Debug for Model {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Model")
            .field("cubes", &self.cube_names())
            .finish()
    }
}

/// Reports a member-sql parse failure with the cube and member it came from.
fn check(
    cube: &str,
    what: &str,
    result: Result<Option<Rc<dyn MemberSql>>, CubeError>,
) -> Result<(), PlannerError> {
    match result {
        Ok(_) => Ok(()),
        Err(err) => Err(PlannerError::unsupported(format!(
            "Cube `{cube}`, {what}: {}",
            err.message
        ))),
    }
}

/// Members of one kind, in a stable order, so errors do not depend on hashing.
fn sorted<T>(members: &HashMap<String, Rc<T>>) -> Vec<(String, Rc<T>)> {
    let mut items: Vec<(String, Rc<T>)> = members
        .iter()
        .map(|(name, def)| (name.clone(), def.clone()))
        .collect();
    items.sort_by(|a, b| a.0.cmp(&b.0));
    items
}

fn take_sequence(
    mapping: &Mapping,
    key: &str,
    source: &str,
    out: &mut Vec<Value>,
) -> Result<(), PlannerError> {
    match mapping.get(Value::from(key)) {
        None | Some(Value::Null) => Ok(()),
        Some(Value::Sequence(items)) => {
            out.extend(items.iter().cloned());
            Ok(())
        }
        Some(_) => Err(PlannerError::model(format!(
            "`{key}:` in `{source}` must be a list"
        ))),
    }
}

fn collect_model_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), PlannerError> {
    let entries = std::fs::read_dir(dir).map_err(|e| {
        PlannerError::model(format!(
            "Failed to read directory `{}`: {}",
            dir.display(),
            e
        ))
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| {
            PlannerError::model(format!("Failed to read `{}`: {}", dir.display(), e))
        })?;
        let path = entry.path();
        if path.is_dir() {
            collect_model_files(&path, out)?;
            continue;
        }
        match path.extension().and_then(|e| e.to_str()) {
            Some("yml") | Some("yaml") => out.push(path),
            Some("js") | Some("ts") => {
                return Err(PlannerError::unsupported(format!(
                    "JavaScript data models are not supported by the Rust planner: `{}`. \
                     Rewrite the cube in YAML, or keep this model on the JavaScript planner",
                    path.display()
                )))
            }
            _ => {}
        }
    }
    Ok(())
}
