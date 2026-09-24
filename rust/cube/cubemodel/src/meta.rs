//! `CubeToMetaTransformer` + the `/v1/meta` REST body from `gateway.ts`.

use std::collections::HashMap;

use serde_json::{json, Map, Value};

use crate::formats::{
    resolve_format_description, transform_dimension_format, transform_measure_format,
};
use crate::join_graph::connected_components;
use crate::model::{
    CubeDef, DataModel, Dimension, EvaluatedFolder, EvaluatedFolderItem, Measure, Segment,
};
use crate::naming::member_title;
use crate::views::{resolve_reference, to_member_data_type};

/// One transformed cube, matching `TransformedCube` in Node.
#[derive(Debug, Clone)]
pub struct CubeMetaConfig {
    pub name: String,
    pub is_view: bool,
    /// The `config` object exactly as the REST API serialises it.
    pub config: Value,
}

/// The whole `metaConfig` payload.
#[derive(Debug, Clone, Default)]
pub struct MetaConfig {
    pub cubes: Vec<CubeMetaConfig>,
}

fn insert_opt(map: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        map.insert(key.to_string(), value);
    }
}

/// `CubeToMetaTransformer.isVisible`.
fn is_visible(
    public: Option<bool>,
    visible: Option<bool>,
    shown: Option<bool>,
    default_value: bool,
) -> bool {
    public.or(visible).or(shown).unwrap_or(default_value)
}

fn title(cube_title: &str, name: &str, explicit: Option<&str>, short: bool) -> String {
    let suffix = explicit
        .map(str::to_string)
        .unwrap_or_else(|| member_title(name));
    if short {
        suffix
    } else {
        format!("{cube_title} {suffix}")
    }
}

/// Builds the meta config for a whole model.
pub fn meta_config(model: &DataModel) -> MetaConfig {
    let components = connected_components(model);
    MetaConfig {
        cubes: model
            .cube_list()
            .into_iter()
            .map(|cube| transform(cube, model, &components))
            .collect(),
    }
}

fn transform(
    cube: &CubeDef,
    model: &DataModel,
    components: &HashMap<String, u32>,
) -> CubeMetaConfig {
    let cube_name = cube.name.clone();
    let cube_title = cube
        .title
        .clone()
        .unwrap_or_else(|| member_title(&cube_name));
    let cube_visible = is_visible(cube.public, cube.visible, cube.shown, true);

    let mut flat_folders: Vec<Value> = Vec::new();
    let nested_folders: Vec<Value> = cube
        .evaluated_folders
        .iter()
        .map(|folder| process_folder(&cube_name, folder, &mut flat_folders))
        .collect();

    let mut config = Map::new();
    config.insert("name".to_string(), Value::String(cube_name.clone()));
    config.insert(
        "type".to_string(),
        Value::String(if cube.is_view { "view" } else { "cube" }.to_string()),
    );
    config.insert("title".to_string(), Value::String(cube_title.clone()));
    config.insert("isVisible".to_string(), Value::Bool(cube_visible));
    config.insert("public".to_string(), Value::Bool(cube_visible));
    insert_opt(
        &mut config,
        "description",
        cube.description.clone().map(Value::String),
    );
    if cube.is_view && !cube.view_groups.is_empty() {
        config.insert(
            "viewGroups".to_string(),
            Value::Array(
                cube.view_groups
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        );
    }
    insert_opt(
        &mut config,
        "connectedComponent",
        components.get(&cube_name).map(|c| json!(c)),
    );
    insert_opt(&mut config, "meta", cube.meta.clone());

    config.insert(
        "measures".to_string(),
        Value::Array(
            cube.measures
                .iter()
                .map(|(name, measure)| {
                    measure_config(
                        &cube_name,
                        &cube_title,
                        name,
                        measure,
                        cube,
                        model,
                        cube_visible,
                    )
                })
                .collect(),
        ),
    );
    config.insert(
        "dimensions".to_string(),
        Value::Array(
            cube.dimensions
                .iter()
                .map(|(name, dimension)| {
                    dimension_config(&cube_name, &cube_title, name, dimension, cube_visible)
                })
                .collect(),
        ),
    );
    config.insert(
        "segments".to_string(),
        Value::Array(
            cube.segments
                .iter()
                .map(|(name, segment)| {
                    segment_config(&cube_name, &cube_title, name, segment, cube_visible)
                })
                .collect(),
        ),
    );
    config.insert(
        "hierarchies".to_string(),
        Value::Array(
            cube.evaluated_hierarchies
                .iter()
                .map(|hierarchy| {
                    let mut obj = Map::new();
                    obj.insert(
                        "name".to_string(),
                        Value::String(format!("{cube_name}.{}", hierarchy.name)),
                    );
                    insert_opt(
                        &mut obj,
                        "title",
                        hierarchy.title.clone().map(Value::String),
                    );
                    obj.insert(
                        "public".to_string(),
                        Value::Bool(hierarchy.public.unwrap_or(true)),
                    );
                    insert_opt(
                        &mut obj,
                        "aliasMember",
                        hierarchy.alias_member.clone().map(Value::String),
                    );
                    obj.insert(
                        "levels".to_string(),
                        Value::Array(
                            hierarchy
                                .levels
                                .iter()
                                .cloned()
                                .map(Value::String)
                                .collect(),
                        ),
                    );
                    Value::Object(obj)
                })
                .collect(),
        ),
    );
    config.insert("folders".to_string(), Value::Array(flat_folders));
    config.insert("nestedFolders".to_string(), Value::Array(nested_folders));

    CubeMetaConfig {
        name: cube_name,
        is_view: cube.is_view,
        config: Value::Object(config),
    }
}

/// `processFolder` from `CubeToMetaTransformer.transform`, with the default
/// (empty) `CUBEJS_NESTED_FOLDERS_DELIMITER`, i.e. nested folders are flattened
/// into their root folder.
fn process_folder(
    cube_name: &str,
    folder: &EvaluatedFolder,
    flat_folders: &mut Vec<Value>,
) -> Value {
    fn walk(
        cube_name: &str,
        folder: &EvaluatedFolder,
        depth: usize,
        merged_members: &mut Vec<String>,
        flat_folders: &mut Vec<Value>,
    ) -> Value {
        let mut flat_members: Vec<String> = Vec::new();
        let nested: Vec<Value> = folder
            .includes
            .iter()
            .map(|item| match item {
                EvaluatedFolderItem::Folder(nested) => walk(
                    cube_name,
                    nested,
                    depth + 1,
                    &mut flat_members,
                    flat_folders,
                ),
                EvaluatedFolderItem::Member(member) => {
                    let member_name = format!("{cube_name}.{}", member.name);
                    flat_members.push(member_name.clone());
                    Value::String(member_name)
                }
            })
            .collect();

        if depth > 0 {
            merged_members.extend(flat_members);
        } else {
            let mut unique: Vec<String> = Vec::new();
            for m in flat_members {
                if !unique.contains(&m) {
                    unique.push(m);
                }
            }
            flat_folders.push(json!({ "name": folder.name, "members": unique }));
        }

        json!({ "name": folder.name, "members": nested })
    }

    let mut merged = Vec::new();
    walk(cube_name, folder, 0, &mut merged, flat_folders)
}

#[allow(clippy::too_many_arguments)]
fn measure_config(
    cube_name: &str,
    cube_title: &str,
    name: &str,
    measure: &Measure,
    cube: &CubeDef,
    model: &DataModel,
    cube_visible: bool,
) -> Value {
    let raw_type = measure
        .member_type
        .clone()
        .unwrap_or_else(|| "number".to_string());
    let member_type = to_member_data_type(&raw_type);
    let cumulative = measure.cumulative.unwrap_or(false) || measure.rolling_window.is_some();

    let drill_members_raw = if measure.drill_members.is_empty() {
        &measure.drill_member_references
    } else {
        &measure.drill_members
    };
    let drill_members: Vec<String> = drill_members_raw
        .iter()
        .map(|m| resolve_reference(cube_name, m))
        .collect();

    let mut drill_measures: Vec<String> = Vec::new();
    let mut drill_dimensions: Vec<String> = Vec::new();
    for member in &drill_members {
        let mut parts = member.rsplitn(2, '.');
        let member_name = parts.next().unwrap_or_default();
        let owner = parts.next().unwrap_or_default();
        let owner_cube = if owner == cube_name {
            Some(cube)
        } else {
            model.get(owner)
        };
        if let Some(owner_cube) = owner_cube {
            if owner_cube.measures.contains_key(member_name) {
                drill_measures.push(member.clone());
            } else if owner_cube.dimensions.contains_key(member_name) {
                drill_dimensions.push(member.clone());
            }
        }
    }

    let format = transform_measure_format(measure.format.as_ref());
    let currency = measure.currency.as_ref().map(|c| c.to_uppercase());
    let format_description =
        resolve_format_description(format.as_ref(), &member_type, true, currency.as_deref());

    let visible = cube_visible && is_visible(measure.public, measure.visible, measure.shown, true);

    let mut obj = Map::new();
    obj.insert(
        "name".to_string(),
        Value::String(format!("{cube_name}.{name}")),
    );
    obj.insert(
        "title".to_string(),
        Value::String(title(cube_title, name, measure.title.as_deref(), false)),
    );
    insert_opt(
        &mut obj,
        "description",
        measure.description.clone().map(Value::String),
    );
    obj.insert(
        "shortTitle".to_string(),
        Value::String(title(cube_title, name, measure.title.as_deref(), true)),
    );
    insert_opt(&mut obj, "format", format);
    insert_opt(&mut obj, "formatDescription", format_description);
    insert_opt(&mut obj, "currency", currency.map(Value::String));
    obj.insert("cumulativeTotal".to_string(), Value::Bool(cumulative));
    obj.insert("cumulative".to_string(), Value::Bool(cumulative));
    obj.insert("type".to_string(), Value::String(member_type));
    obj.insert(
        "aggType".to_string(),
        Value::String(
            measure
                .agg_type
                .clone()
                .or_else(|| measure.member_type.clone())
                .unwrap_or_default(),
        ),
    );
    obj.insert(
        "drillMembers".to_string(),
        Value::Array(drill_members.into_iter().map(Value::String).collect()),
    );
    obj.insert(
        "drillMembersGrouped".to_string(),
        json!({ "measures": drill_measures, "dimensions": drill_dimensions }),
    );
    insert_opt(
        &mut obj,
        "aliasMember",
        measure.alias_member.clone().map(Value::String),
    );
    insert_opt(&mut obj, "meta", measure.meta.clone());
    obj.insert("isVisible".to_string(), Value::Bool(visible));
    obj.insert("public".to_string(), Value::Bool(visible));
    Value::Object(obj)
}

fn dimension_config(
    cube_name: &str,
    cube_title: &str,
    name: &str,
    dimension: &Dimension,
    cube_visible: bool,
) -> Value {
    let raw_type = dimension
        .member_type
        .clone()
        .unwrap_or_else(|| "string".to_string());
    // `switch` dimensions are exposed as strings.
    let member_type = if raw_type == "switch" {
        "string".to_string()
    } else {
        raw_type
    };

    let format = transform_dimension_format(dimension.format.as_ref(), Some(&member_type));
    let currency = dimension.currency.as_ref().map(|c| c.to_uppercase());
    let format_description =
        resolve_format_description(format.as_ref(), &member_type, false, currency.as_deref());

    let primary_key = dimension.primary_key.unwrap_or(false);
    let visible = cube_visible
        && is_visible(
            dimension.public,
            dimension.visible,
            dimension.shown,
            !primary_key,
        );

    let mut obj = Map::new();
    obj.insert(
        "name".to_string(),
        Value::String(format!("{cube_name}.{name}")),
    );
    obj.insert(
        "title".to_string(),
        Value::String(title(cube_title, name, dimension.title.as_deref(), false)),
    );
    obj.insert("type".to_string(), Value::String(member_type.clone()));
    insert_opt(
        &mut obj,
        "description",
        dimension.description.clone().map(Value::String),
    );
    obj.insert(
        "shortTitle".to_string(),
        Value::String(title(cube_title, name, dimension.title.as_deref(), true)),
    );
    obj.insert(
        "suggestFilterValues".to_string(),
        Value::Bool(dimension.suggest_filter_values.unwrap_or(true)),
    );
    insert_opt(&mut obj, "format", format);
    insert_opt(&mut obj, "formatDescription", format_description);
    insert_opt(&mut obj, "currency", currency.map(Value::String));
    insert_opt(&mut obj, "meta", dimension.meta.clone());
    obj.insert("isVisible".to_string(), Value::Bool(visible));
    obj.insert("public".to_string(), Value::Bool(visible));
    obj.insert("primaryKey".to_string(), Value::Bool(primary_key));
    insert_opt(
        &mut obj,
        "aliasMember",
        dimension.alias_member.clone().map(Value::String),
    );
    if !dimension.granularities.is_empty() {
        obj.insert(
            "granularities".to_string(),
            Value::Array(
                dimension
                    .granularities
                    .iter()
                    .map(|(g_name, g)| {
                        let mut g_obj = Map::new();
                        g_obj.insert("name".to_string(), Value::String(g_name.clone()));
                        g_obj.insert(
                            "title".to_string(),
                            Value::String(title(cube_title, g_name, g.title.as_deref(), true)),
                        );
                        insert_opt(
                            &mut g_obj,
                            "interval",
                            g.interval.clone().map(Value::String),
                        );
                        insert_opt(&mut g_obj, "offset", g.offset.clone().map(Value::String));
                        insert_opt(&mut g_obj, "origin", g.origin.clone().map(Value::String));
                        Value::Object(g_obj)
                    })
                    .collect(),
            ),
        );
    }
    insert_opt(
        &mut obj,
        "order",
        dimension.order.clone().map(Value::String),
    );
    insert_opt(
        &mut obj,
        "key",
        dimension.key_reference.clone().map(Value::String),
    );
    insert_opt(&mut obj, "links", dimension.links.clone());
    if dimension.synthetic == Some(true) {
        obj.insert("synthetic".to_string(), Value::Bool(true));
    }
    Value::Object(obj)
}

fn segment_config(
    cube_name: &str,
    cube_title: &str,
    name: &str,
    segment: &Segment,
    cube_visible: bool,
) -> Value {
    let visible = cube_visible && is_visible(segment.public, segment.visible, segment.shown, true);

    let mut obj = Map::new();
    obj.insert(
        "name".to_string(),
        Value::String(format!("{cube_name}.{name}")),
    );
    obj.insert(
        "title".to_string(),
        Value::String(title(cube_title, name, segment.title.as_deref(), false)),
    );
    obj.insert(
        "shortTitle".to_string(),
        Value::String(title(cube_title, name, segment.title.as_deref(), true)),
    );
    insert_opt(
        &mut obj,
        "description",
        segment.description.clone().map(Value::String),
    );
    insert_opt(&mut obj, "meta", segment.meta.clone());
    obj.insert("isVisible".to_string(), Value::Bool(visible));
    obj.insert("public".to_string(), Value::Bool(visible));
    Value::Object(obj)
}

/// `gateway.meta()`: the `/v1/meta` body, `{ cubes: [...] }`.
///
/// Hidden members are dropped and cubes left with no visible members at all are
/// removed, exactly like `filterVisibleItemsInMeta` in production mode.
pub fn rest_meta_response(meta: &MetaConfig, only_views: bool) -> Value {
    rest_meta_response_with(meta, only_views, false)
}

/// As [`rest_meta_response`], but `include_hidden` keeps invisible members —
/// the dev-mode / playground-secret branch of `filterVisibleItemsInMeta`.
pub fn rest_meta_response_with(meta: &MetaConfig, only_views: bool, include_hidden: bool) -> Value {
    let cubes: Vec<Value> = meta
        .cubes
        .iter()
        .filter(|c| !only_views || c.is_view)
        .filter_map(|c| {
            let Value::Object(config) = &c.config else {
                return None;
            };
            let mut config = config.clone();
            for key in ["measures", "dimensions", "segments"] {
                if let Some(Value::Array(items)) = config.get(key) {
                    let filtered: Vec<Value> = items
                        .iter()
                        .filter(|item| {
                            include_hidden
                                || item
                                    .get("isVisible")
                                    .and_then(Value::as_bool)
                                    .unwrap_or(false)
                        })
                        .cloned()
                        .collect();
                    config.insert(key.to_string(), Value::Array(filtered));
                }
            }
            let has_members = ["measures", "dimensions", "segments"].iter().any(|key| {
                config
                    .get(*key)
                    .and_then(Value::as_array)
                    .map(|a| !a.is_empty())
                    .unwrap_or(false)
            });
            has_members.then_some(Value::Object(config))
        })
        .collect();

    json!({ "cubes": cubes })
}
