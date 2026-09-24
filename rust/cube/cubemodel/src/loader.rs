//! Reads a model directory, renders Jinja, parses YAML and produces a
//! validated, view-resolved [`DataModel`].

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use minijinja::Environment;
use serde_json::{Map, Value};

use crate::error::{ErrorReporter, ModelError};
use crate::jinja::{build_environment, has_jinja_syntax, render, TemplateContext};
use crate::model::{CubeDef, DataModel};
use crate::validate;
use crate::views;
use crate::yaml;

const YAML_EXTENSIONS: &[&str] = &["yml", "yaml", "jinja", "j2"];

/// Model files that would need a JavaScript engine. This backend is pure Rust,
/// so they are rejected loudly rather than skipped.
const UNSUPPORTED_EXTENSIONS: &[&str] = &["js", "ts", "mjs", "cjs", "jsx", "tsx", "py"];

/// Loads YAML/Jinja data models from disk or from a string.
pub struct ModelLoader {
    env: Environment<'static>,
    context: TemplateContext,
    validate: bool,
}

impl Default for ModelLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelLoader {
    pub fn new() -> Self {
        Self {
            env: build_environment(false),
            context: TemplateContext::default(),
            validate: true,
        }
    }

    /// Uses a custom Jinja context (`COMPILE_CONTEXT` and extra variables).
    pub fn with_context(context: TemplateContext) -> Self {
        Self {
            env: build_environment(false),
            context,
            validate: true,
        }
    }

    /// Skips validation; view resolution still runs. Useful for inspecting a
    /// model that is known to be invalid.
    pub fn without_validation(mut self) -> Self {
        self.validate = false;
        self
    }

    /// Loads every `*.yml` / `*.yaml` / `*.jinja` file under `path`, recursively.
    pub fn load_dir(path: impl AsRef<Path>) -> Result<DataModel, ModelError> {
        Self::new().load_dir_with(path)
    }

    /// Loads a single YAML document. `file_name` is used in error messages.
    pub fn load_str(source: &str, file_name: &str) -> Result<DataModel, ModelError> {
        Self::new().load_str_with(source, file_name)
    }

    pub fn load_dir_with(&self, path: impl AsRef<Path>) -> Result<DataModel, ModelError> {
        let root = path.as_ref();
        let mut files = Vec::new();
        let mut unsupported = Vec::new();
        collect_files(root, &mut files, &mut unsupported)?;
        if let Some(file) = unsupported.into_iter().min() {
            let file_name = file
                .strip_prefix(root)
                .unwrap_or(&file)
                .to_string_lossy()
                .to_string();
            let message = if file.extension().and_then(|e| e.to_str()) == Some("py") {
                format!("Python data models and template functions are not supported: {file_name}. Convert it to YAML.")
            } else {
                format!(
                    "JavaScript data models are not supported: {file_name}. Convert it to YAML."
                )
            };
            return Err(ModelError::UnsupportedModelFile { file_name, message });
        }
        files.sort();

        let mut sources = Vec::new();
        for file in files {
            let content = fs::read_to_string(&file)?;
            let name = file
                .strip_prefix(root)
                .unwrap_or(&file)
                .to_string_lossy()
                .to_string();
            sources.push((name, content));
        }

        self.load_sources(&sources)
    }

    pub fn load_str_with(&self, source: &str, file_name: &str) -> Result<DataModel, ModelError> {
        self.load_sources(&[(file_name.to_string(), source.to_string())])
    }

    /// The whole pipeline: render -> parse -> normalise -> extends -> validate
    /// -> view resolution.
    pub fn load_sources(&self, sources: &[(String, String)]) -> Result<DataModel, ModelError> {
        let mut reporter = ErrorReporter::new();
        let mut model = DataModel::default();
        let mut seen_names: HashSet<String> = HashSet::new();

        for (file_name, content) in sources {
            reporter.in_file(file_name.clone());
            let rendered = if has_jinja_syntax(content) {
                match render(&self.env, content, &self.context) {
                    Ok(r) => r,
                    Err(e) => {
                        return Err(ModelError::Template {
                            file_name: file_name.clone(),
                            message: describe_template_error(&e),
                        })
                    }
                }
            } else {
                content.clone()
            };

            let parsed = match yaml::parse_yaml(&rendered) {
                Ok(Some(v)) => v,
                Ok(None) => continue,
                Err(e) => {
                    reporter.error(format!("Syntax error: {e}"));
                    continue;
                }
            };

            let Value::Object(top) = parsed else {
                reporter.error("Model file must contain a mapping at the top level");
                continue;
            };

            for (key, value) in top {
                match key.as_str() {
                    "cubes" | "views" => {
                        let is_view = key == "views";
                        let entries = match value {
                            Value::Array(items) => items,
                            Value::Null => continue,
                            _ => {
                                reporter.error(format!("{key} must be defined as array"));
                                continue;
                            }
                        };
                        let label = if is_view { "view" } else { "cube" };
                        yaml::check_duplicate_names(&entries, &mut reporter, |name| {
                            format!("Found duplicate {label} name '{name}'.")
                        });

                        for entry in entries {
                            if let Some(cube) = self.build_cube(
                                entry,
                                is_view,
                                file_name,
                                &mut seen_names,
                                &mut reporter,
                            ) {
                                if is_view {
                                    model.views.insert(cube.name.clone(), cube);
                                } else {
                                    model.cubes.insert(cube.name.clone(), cube);
                                }
                            }
                        }
                    }
                    // View groups are parsed by the gateway layer, not modelled here.
                    "view_groups" | "viewGroups" => {}
                    other => {
                        reporter.error(format!(
                            "Unexpected YAML key: {other}. Only 'cubes', 'views', and 'view_groups' are allowed here."
                        ));
                    }
                }
            }
        }
        reporter.exit_file();

        resolve_extends(&mut model, &mut reporter);

        if self.validate {
            validate::validate_model(&model, &mut reporter);
        }

        views::resolve_views(&mut model, &mut reporter);

        reporter.result(model)
    }

    fn build_cube(
        &self,
        entry: Value,
        is_view: bool,
        file_name: &str,
        seen_names: &mut HashSet<String>,
        reporter: &mut ErrorReporter,
    ) -> Option<CubeDef> {
        let Value::Object(_) = entry else {
            reporter.error("Cube definition must be a mapping");
            return None;
        };

        let mut entry = entry;
        yaml::camelize_cube(&mut entry);

        let Value::Object(mut obj) = entry else {
            unreachable!("camelize_cube keeps the value an object")
        };

        let name = match obj.get("name").and_then(Value::as_str) {
            Some(n) => n.to_string(),
            None => {
                reporter.error("name isn't defined for cube");
                return None;
            }
        };

        if !seen_names.insert(name.clone()) {
            let label = if is_view { "view" } else { "cube" };
            reporter.error(format!("Found duplicate {label} name '{name}'."));
        }

        reporter.push_context(format!("{name} cube"));

        for (key, member_type) in [
            ("measures", "measure"),
            ("dimensions", "dimension"),
            ("segments", "segment"),
            ("preAggregations", "preAggregation"),
            ("hierarchies", "hierarchies"),
        ] {
            let raw = obj.shift_remove(key);
            if raw.is_some() {
                let converted = yaml::yaml_array_to_obj(raw, member_type, &name, reporter);
                obj.insert(key.to_string(), converted);
            }
        }

        if let Some(joins) = obj.get("joins") {
            if !joins.is_array() && !joins.is_null() {
                reporter.error("joins must be defined as array");
                obj.insert("joins".to_string(), Value::Array(vec![]));
            }
        }

        normalize_nulls(&mut obj);

        let cube: Result<CubeDef, _> = serde_json::from_value(Value::Object(obj));
        let result = match cube {
            Ok(mut cube) => {
                cube.name = name;
                cube.file_name = file_name.to_string();
                cube.is_view = is_view;
                camel_case_types(&mut cube);
                default_pre_aggregation_types(&mut cube);
                Some(cube)
            }
            Err(e) => {
                reporter.error(format!("Failed to parse cube definition: {e}"));
                None
            }
        };

        reporter.pop_context();
        result
    }
}

/// `CubeSymbols.camelCaseTypes`: snake_case `type` / `relationship` values are
/// normalised (`count_distinct` -> `countDistinct`, `many_to_one` -> `manyToOne`).
fn camel_case_types(cube: &mut CubeDef) {
    fn fix(value: &mut Option<String>) {
        if let Some(v) = value {
            if v.contains('_') {
                *v = crate::naming::camelize_lower(v);
            }
        }
    }
    for measure in cube.measures.values_mut() {
        fix(&mut measure.member_type);
    }
    for dimension in cube.dimensions.values_mut() {
        fix(&mut dimension.member_type);
    }
    for pre_agg in cube.pre_aggregations.values_mut() {
        fix(&mut pre_agg.pre_agg_type);
    }
    for join in cube.joins.iter_mut() {
        fix(&mut join.relationship);
    }
}

/// `CubeSymbols.transformPreAggregations`: `rollup` is the default type, but an
/// entirely empty pre-aggregation keeps failing validation.
fn default_pre_aggregation_types(cube: &mut CubeDef) {
    for pre_agg in cube.pre_aggregations.values_mut() {
        let is_empty = pre_agg.pre_agg_type.is_none()
            && pre_agg.measures.is_empty()
            && pre_agg.dimensions.is_empty()
            && pre_agg.segments.is_empty()
            && pre_agg.time_dimension.is_none()
            && pre_agg.time_dimension_reference.is_none()
            && pre_agg.sql.is_none()
            && pre_agg.rollups.is_empty()
            && pre_agg.extra.is_empty();
        if !is_empty && pre_agg.pre_agg_type.is_none() {
            pre_agg.pre_agg_type = Some("rollup".to_string());
        }
    }
}

/// `null` values for collection-shaped keys are treated as "absent" so a
/// `includes:` with nothing under it behaves like the JS compiler's `undefined`.
fn normalize_nulls(obj: &mut Map<String, Value>) {
    let null_keys: Vec<String> = obj
        .iter()
        .filter(|(_, v)| v.is_null())
        .map(|(k, _)| k.clone())
        .collect();
    for key in null_keys {
        if key != "meta" {
            obj.shift_remove(&key);
        }
    }
}

/// Renders a minijinja error with the hint that Python template functions are
/// not available in this backend.
fn describe_template_error(error: &minijinja::Error) -> String {
    let mut message = error.to_string();
    if let Some(source) = std::error::Error::source(error) {
        message = format!("{message}: {source}");
    }
    if matches!(
        error.kind(),
        minijinja::ErrorKind::UnknownFunction | minijinja::ErrorKind::UnknownMethod
    ) {
        message.push_str(
            ". Python template functions and filters (cube.py) are not supported by the Rust backend; only `env_var` and `COMPILE_CONTEXT` are available",
        );
    }
    message
}

fn collect_files(
    dir: &Path,
    out: &mut Vec<PathBuf>,
    unsupported: &mut Vec<PathBuf>,
) -> std::io::Result<()> {
    if !dir.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Model directory not found: {}", dir.display()),
        ));
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out, unsupported)?;
        } else {
            let extension = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                .unwrap_or_default();
            if YAML_EXTENSIONS.contains(&extension.as_str()) {
                out.push(path);
            } else if UNSUPPORTED_EXTENSIONS.contains(&extension.as_str()) {
                unsupported.push(path);
            }
        }
    }
    Ok(())
}

/// Applies `extends` using the same merge rules as `CubeSymbols.createCube`.
fn resolve_extends(model: &mut DataModel, reporter: &mut ErrorReporter) {
    let order = match topological_order(model, reporter) {
        Some(order) => order,
        None => return,
    };

    for name in order {
        let Some(parent_name) = model.get(&name).and_then(|c| c.extends.clone()) else {
            continue;
        };
        let Some(parent) = model.get(&parent_name).cloned() else {
            reporter.with_context(format!("{name} cube"), |r| {
                r.error(format!("Can't resolve {parent_name}"));
            });
            continue;
        };
        if let Some(child) = model.get_mut(&name) {
            merge_parent(child, &parent);
        }
    }
}

fn topological_order(model: &DataModel, reporter: &mut ErrorReporter) -> Option<Vec<String>> {
    let names = model.names();
    let mut order: Vec<String> = Vec::new();
    let mut state: std::collections::HashMap<String, u8> = std::collections::HashMap::new();

    fn visit(
        name: &str,
        model: &DataModel,
        state: &mut std::collections::HashMap<String, u8>,
        order: &mut Vec<String>,
        reporter: &mut ErrorReporter,
    ) -> bool {
        match state.get(name) {
            Some(2) => return true,
            Some(1) => {
                reporter.error(format!("Circular extends detected for cube '{name}'"));
                return false;
            }
            _ => {}
        }
        state.insert(name.to_string(), 1);
        if let Some(cube) = model.get(name) {
            if let Some(parent) = &cube.extends {
                if model.contains(parent) && !visit(parent, model, state, order, reporter) {
                    return false;
                }
            }
        }
        state.insert(name.to_string(), 2);
        order.push(name.to_string());
        true
    }

    for name in &names {
        if !visit(name, model, &mut state, &mut order, reporter) {
            return None;
        }
    }
    Some(order)
}

/// `allDefinitions` / `rawCubes` / `rawFolders` semantics: parent first, child
/// overrides; arrays concatenate parent-then-child.
fn merge_parent(child: &mut CubeDef, parent: &CubeDef) {
    macro_rules! merge_members {
        ($field:ident) => {{
            let mut merged = parent.$field.clone();
            for (k, v) in child.$field.iter() {
                merged.insert(k.clone(), v.clone());
            }
            child.$field = merged;
        }};
    }
    merge_members!(measures);
    merge_members!(dimensions);
    merge_members!(segments);
    merge_members!(hierarchies);

    // Pre-aggregations: `{...local, ...parent, ...local}` — local order first,
    // parent entries appended, local definitions win.
    let mut pre_aggs = child.pre_aggregations.clone();
    for (k, v) in parent.pre_aggregations.iter() {
        pre_aggs.insert(k.clone(), v.clone());
    }
    for (k, v) in child.pre_aggregations.iter() {
        pre_aggs.insert(k.clone(), v.clone());
    }
    child.pre_aggregations = pre_aggs;

    let mut joins = parent.joins.clone();
    joins.extend(child.joins.clone());
    child.joins = joins;

    let mut folders = parent.folders.clone();
    folders.extend(child.folders.clone());
    child.folders = folders;

    let mut cubes = parent.cubes.clone();
    cubes.extend(child.cubes.clone());
    child.cubes = cubes;

    let mut access_policy = parent.access_policy.clone();
    access_policy.extend(child.access_policy.clone());
    child.access_policy = access_policy;

    macro_rules! inherit {
        ($($field:ident),*) => {
            $(if child.$field.is_none() { child.$field = parent.$field.clone(); })*
        };
    }
    inherit!(
        sql,
        sql_table,
        sql_alias,
        data_source,
        title,
        description,
        public,
        shown,
        visible,
        rewrite_queries,
        calendar,
        refresh_key,
        meta
    );

    // `sql` and `sqlTable` are mutually exclusive: an override in the child
    // hides the other one inherited from the parent.
    let child_has_sql_table = child.sql_table.is_some();
    let child_has_sql = child.sql.is_some();
    if child_has_sql_table && child_has_sql {
        if parent.sql.is_some() && parent.sql == child.sql {
            child.sql = None;
        } else if parent.sql_table.is_some() && parent.sql_table == child.sql_table {
            child.sql_table = None;
        }
    }

    if child.view_groups.is_empty() {
        child.view_groups = parent.view_groups.clone();
    }
}
