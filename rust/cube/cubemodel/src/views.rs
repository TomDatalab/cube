//! View resolution: the Rust port of `CubeSymbols.prepareIncludes` and of
//! `CubeEvaluator.prepareHierarchies` / `prepareFolders`.

use std::collections::HashSet;

use crate::error::ErrorReporter;
use crate::model::{
    CubeDef, DataModel, Dimension, EvaluatedFolder, EvaluatedFolderItem, EvaluatedHierarchy,
    Folder, FolderInclude, FolderIncludes, Hierarchy, IncludeMember, IncludedMember, Includes,
    Measure, Members, Segment, ViewCubeInclude,
};

/// `CubeSymbols.isCalculatedMeasureType`.
pub fn is_calculated_measure_type(member_type: &str) -> bool {
    matches!(member_type, "number" | "string" | "time" | "boolean")
}

/// `CubeSymbols.toMemberDataType`.
pub fn to_member_data_type(member_type: &str) -> String {
    if is_calculated_measure_type(member_type) {
        member_type.to_string()
    } else {
        "number".to_string()
    }
}

/// Resolves a member reference as written in YAML (`status`, `{CUBE}.status`,
/// `users.city`) into a `cube.member` path.
pub fn resolve_reference(cube_name: &str, reference: &str) -> String {
    let cleaned: String = reference
        .chars()
        .filter(|c| *c != '{' && *c != '}')
        .collect();
    let cleaned = cleaned.trim();

    if let Some(rest) = cleaned.strip_prefix("CUBE.") {
        return format!("{cube_name}.{rest}");
    }
    if cleaned.contains('.') {
        cleaned.to_string()
    } else {
        format!("{cube_name}.{cleaned}")
    }
}

#[derive(Debug, Clone)]
struct ResolvedRef {
    /// `<joinPath>.<memberName>`
    member: String,
    /// Name inside the view.
    name: String,
    overrides: Option<IncludeMember>,
}

/// Entry point: evaluates cube hierarchies then resolves every view.
pub fn resolve_views(model: &mut DataModel, reporter: &mut ErrorReporter) {
    for cube in model.cubes.values_mut() {
        evaluate_cube_hierarchies(cube);
    }

    let mut views = std::mem::take(&mut model.views);
    for view in views.values_mut() {
        reporter.in_file(view.file_name.clone());
        reporter.push_context(format!("{} cube", view.name));
        prepare_includes(view, &model.cubes, reporter);
        prepare_view_hierarchies(view, &model.cubes, reporter);
        prepare_folders(view, reporter);
        reporter.pop_context();
    }
    reporter.exit_file();
    model.views = views;
}

fn evaluate_cube_hierarchies(cube: &mut CubeDef) {
    cube.evaluated_hierarchies = cube
        .hierarchies
        .iter()
        .map(|(name, h)| EvaluatedHierarchy {
            name: name.clone(),
            title: h.title.clone(),
            public: h.public,
            levels: h
                .levels
                .iter()
                .map(|l| resolve_reference(&cube.name, l))
                .collect(),
            alias_member: None,
        })
        .collect();
}

fn member_names(cube: &CubeDef, member_type: &str) -> Vec<String> {
    match member_type {
        "measures" => cube.measures.keys().cloned().collect(),
        "dimensions" => cube.dimensions.keys().cloned().collect(),
        "segments" => cube.segments.keys().cloned().collect(),
        "hierarchies" => cube.hierarchies.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

fn has_member(cube: &CubeDef, member_type: &str, name: &str) -> bool {
    match member_type {
        "measures" => cube.measures.contains_key(name),
        "dimensions" => cube.dimensions.contains_key(name),
        "segments" => cube.segments.contains_key(name),
        "hierarchies" => cube.hierarchies.contains_key(name),
        _ => false,
    }
}

fn join_path_of(include: &ViewCubeInclude) -> String {
    include.join_path.clone().unwrap_or_default()
}

/// `CubeSymbols.prepareIncludes`.
fn prepare_includes(view: &mut CubeDef, cubes: &Members<CubeDef>, reporter: &mut ErrorReporter) {
    if view.cubes.is_empty() {
        return;
    }

    let mut all_members: Vec<String> = Vec::new();
    let mut resolved_members: HashSet<String> = HashSet::new();
    let mut auto_include_members: Vec<String> = Vec::new();
    let mut join_map: Vec<Vec<String>> = Vec::new();
    let mut view_all_members: Vec<ResolvedRef> = Vec::new();

    // `hierarchies` first (it feeds auto-included levels), then `dimensions`
    // before `measures` (drill member filtering depends on it), then `segments`.
    for member_type in ["hierarchies", "dimensions", "measures", "segments"] {
        let includes_source: Vec<ViewCubeInclude> = if member_type == "dimensions" {
            view.cubes
                .iter()
                .map(|it| {
                    let full_path = join_path_of(it);
                    let split: Vec<&str> = full_path.split('.').collect();
                    let cube_ref = split.last().copied().unwrap_or_default();
                    if split.len() > 1 {
                        join_map.push(split.iter().map(|s| s.to_string()).collect());
                    }

                    match &it.includes {
                        Includes::List(list) => {
                            let existing: Vec<String> =
                                list.iter().map(|i| i.name().to_string()).collect();
                            let extra: Vec<String> = auto_include_members
                                .iter()
                                .filter(|path| path.starts_with(&format!("{cube_ref}.")))
                                .filter_map(|path| path.split('.').nth(1).map(str::to_string))
                                .filter(|m| !existing.contains(m))
                                .collect();
                            let mut augmented = it.clone();
                            if !extra.is_empty() {
                                let mut new_list = list.clone();
                                for name in extra {
                                    new_list.push(crate::model::IncludeItem::Name(name));
                                }
                                augmented.includes = Includes::List(new_list);
                            }
                            augmented
                        }
                        _ => it.clone(),
                    }
                })
                .collect()
        } else {
            view.cubes.clone()
        };

        let cube_includes = members_from_cubes(
            view,
            &includes_source,
            member_type,
            cubes,
            &mut all_members,
            &mut resolved_members,
            reporter,
        );
        view_all_members.extend(cube_includes.iter().cloned());

        if member_type == "hierarchies" {
            for member in &cube_includes {
                let parts: Vec<&str> = member.member.split('.').collect();
                if parts.len() < 2 {
                    continue;
                }
                let cube_name = parts[parts.len() - 2];
                let hierarchy_name = parts[parts.len() - 1];
                if let Some(source) = cubes.get(cube_name) {
                    if let Some(hierarchy) = source.hierarchies.get(hierarchy_name) {
                        for level in &hierarchy.levels {
                            let resolved = resolve_reference(cube_name, level);
                            if !auto_include_members.contains(&resolved) {
                                auto_include_members.push(resolved);
                            }
                        }
                    }
                }
            }
        }

        apply_include_members(
            view,
            member_type,
            &cube_includes,
            &view_all_members,
            cubes,
            reporter,
        );

        let mut seen: HashSet<String> = view
            .included_members
            .iter()
            .map(|m| format!("{}|{}|{}", m.member_type, m.member_path, m.name))
            .collect();

        for member in &cube_includes {
            let parts: Vec<&str> = member.member.split('.').collect();
            let member_path = parts[parts.len().saturating_sub(2)..].join(".");
            let key = format!("{member_type}|{member_path}|{}", member.name);
            if seen.insert(key) {
                view.included_members.push(IncludedMember {
                    member_type: member_type.to_string(),
                    member_path,
                    name: member.name.clone(),
                });
            }
        }
    }

    view.join_map = join_map;

    for member in all_members {
        if !resolved_members.contains(&member) {
            reporter.error(format!(
                "Member '{member}' is included in '{}' but not defined in any cube",
                view.name
            ));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn members_from_cubes(
    view: &CubeDef,
    includes_source: &[ViewCubeInclude],
    member_type: &str,
    cubes: &Members<CubeDef>,
    all_members: &mut Vec<String>,
    resolved_members: &mut HashSet<String>,
    reporter: &mut ErrorReporter,
) -> Vec<ResolvedRef> {
    let mut result: Vec<ResolvedRef> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for cube_include in includes_source {
        let full_path = join_path_of(cube_include);
        let split: Vec<&str> = full_path.split('.').collect();
        let cube_reference = split.last().copied().unwrap_or_default();
        let cube_name = cube_include
            .alias
            .clone()
            .unwrap_or_else(|| cube_reference.to_string());
        let prefix = cube_include.prefix.unwrap_or(false);
        let full_member_name = |member_name: &str| {
            if prefix {
                format!("{cube_name}_{member_name}")
            } else {
                member_name.to_string()
            }
        };

        let source = cubes.get(cube_reference);

        let includes: Vec<ResolvedRef> = match &cube_include.includes {
            Includes::All(_) => source
                .map(|c| member_names(c, member_type))
                .unwrap_or_default()
                .into_iter()
                .map(|member_name| ResolvedRef {
                    member: format!("{full_path}.{member_name}"),
                    name: full_member_name(&member_name),
                    overrides: None,
                })
                .collect(),
            Includes::List(list) => list
                .iter()
                .filter_map(|include| {
                    let member = include.alias().unwrap_or_else(|| include.name());
                    if member.contains('.') {
                        reporter.error(format!(
                            "Paths aren't allowed in cube includes but '{member}' provided as include member"
                        ));
                    }
                    let name = full_member_name(member);
                    if !all_members.contains(&name) {
                        all_members.push(name.clone());
                    }

                    let included_member_name = include.name();
                    let resolved =
                        source.map(|c| has_member(c, member_type, included_member_name)).unwrap_or(false);
                    if !resolved {
                        return None;
                    }
                    resolved_members.insert(name.clone());

                    let overrides = include
                        .overrides()
                        .filter(|o| o.has_overrides())
                        .cloned();

                    Some(ResolvedRef {
                        member: format!("{full_path}.{included_member_name}"),
                        name,
                        overrides,
                    })
                })
                .collect(),
            Includes::None => Vec::new(),
        };

        let excludes: HashSet<String> = cube_include
            .excludes
            .iter()
            .filter_map(|exclude| {
                if exclude.contains('.') {
                    reporter.error(format!(
                        "Paths aren't allowed in cube excludes but '{exclude}' provided as exclude member"
                    ));
                }
                let resolved = source.map(|c| has_member(c, member_type, exclude)).unwrap_or(false);
                resolved.then(|| format!("{full_path}.{exclude}"))
            })
            .collect();

        if cube_include.split.unwrap_or(false) {
            // Split views are not produced by this crate; see the crate docs.
            continue;
        }

        for member in includes
            .into_iter()
            .filter(|m| !excludes.contains(&m.member))
        {
            let key = format!("{}|{}", member.member, member.name);
            if seen.insert(key) {
                result.push(member);
            }
        }
    }

    let _ = view;
    result
}

/// `CubeSymbols.generateIncludeMembers` + `applyIncludeMembers`.
fn apply_include_members(
    view: &mut CubeDef,
    member_type: &str,
    members: &[ResolvedRef],
    view_all_members: &[ResolvedRef],
    cubes: &Members<CubeDef>,
    reporter: &mut ErrorReporter,
) {
    for member_ref in members {
        let parts: Vec<&str> = member_ref.member.split('.').collect();
        if parts.len() < 2 {
            continue;
        }
        let source_cube_name = parts[parts.len() - 2];
        let source_member_name = parts[parts.len() - 1];
        let Some(source_cube) = cubes.get(source_cube_name) else {
            continue;
        };

        let name = member_ref.name.clone();
        let conflict = has_member(view, member_type, &name);
        if conflict {
            reporter.error(format!(
                "Included member '{name}' conflicts with existing member of '{}'. Please consider excluding this member or assigning it an alias.",
                view.name
            ));
            continue;
        }

        let ov = member_ref.overrides.as_ref();

        match member_type {
            "measures" => {
                let Some(resolved) = source_cube.measures.get(source_member_name) else {
                    continue;
                };
                let raw_type = resolved.member_type.clone().unwrap_or_default();
                let drill_members = if resolved.drill_members.is_empty() {
                    resolved.drill_member_references.clone()
                } else {
                    resolved.drill_members.clone()
                };
                let filtered_drill_members: Vec<String> = drill_members
                    .iter()
                    .filter_map(|m| {
                        let resolved_path = resolve_reference(source_cube_name, m);
                        view_all_members
                            .iter()
                            .find(|v| v.member.ends_with(&resolved_path))
                            .map(|v| format!("{}.{}", view.name, v.name))
                    })
                    .collect();

                let currency = ov
                    .and_then(|o| o.currency.clone())
                    .or_else(|| resolved.currency.clone());

                let measure = Measure {
                    sql: Some(member_ref.member.clone()),
                    member_type: Some(to_member_data_type(&raw_type)),
                    agg_type: resolved.member_type.clone(),
                    title: ov
                        .and_then(|o| o.title.clone())
                        .or_else(|| resolved.title.clone()),
                    description: ov
                        .and_then(|o| o.description.clone())
                        .or_else(|| resolved.description.clone()),
                    format: ov
                        .and_then(|o| o.format.clone())
                        .or_else(|| resolved.format.clone()),
                    meta: ov
                        .and_then(|o| o.meta.clone())
                        .or_else(|| resolved.meta.clone()),
                    currency,
                    multi_stage: resolved.multi_stage,
                    time_shift: resolved.time_shift.clone(),
                    order_by: resolved.order_by.clone(),
                    drill_members: filtered_drill_members,
                    mask: resolved.mask.clone(),
                    cumulative: resolved.cumulative,
                    rolling_window: resolved.rolling_window.clone(),
                    alias_member: Some(member_ref.member.clone()),
                    ..Default::default()
                };
                view.measures.insert(name, measure);
            }
            "dimensions" => {
                let Some(resolved) = source_cube.dimensions.get(source_member_name) else {
                    continue;
                };
                let currency = ov
                    .and_then(|o| o.currency.clone())
                    .or_else(|| resolved.currency.clone());

                let key_reference = resolved.key_reference.as_ref().and_then(|key| {
                    match view_all_members.iter().find(|v| &v.member == key) {
                        Some(found) => Some(format!("{}.{}", view.name, found.name)),
                        None => {
                            reporter.error(format!(
                                "Dimension '{}' has key '{key}' but the key dimension is not included in view '{}'",
                                member_ref.member, view.name
                            ));
                            None
                        }
                    }
                });

                let dimension = Dimension {
                    sql: Some(member_ref.member.clone()),
                    member_type: resolved.member_type.clone(),
                    title: ov
                        .and_then(|o| o.title.clone())
                        .or_else(|| resolved.title.clone()),
                    description: ov
                        .and_then(|o| o.description.clone())
                        .or_else(|| resolved.description.clone()),
                    format: ov
                        .and_then(|o| o.format.clone())
                        .or_else(|| resolved.format.clone()),
                    meta: ov
                        .and_then(|o| o.meta.clone())
                        .or_else(|| resolved.meta.clone()),
                    currency,
                    granularities: resolved.granularities.clone(),
                    multi_stage: resolved.multi_stage,
                    key_reference,
                    mask: resolved.mask.clone(),
                    links: resolved.links.clone(),
                    synthetic: resolved.synthetic,
                    suggest_filter_values: resolved.suggest_filter_values,
                    order: resolved.order.clone(),
                    values: resolved.values.clone(),
                    alias_member: Some(member_ref.member.clone()),
                    ..Default::default()
                };
                view.dimensions.insert(name, dimension);
            }
            "segments" => {
                let Some(resolved) = source_cube.segments.get(source_member_name) else {
                    continue;
                };
                let segment = Segment {
                    sql: Some(member_ref.member.clone()),
                    title: ov
                        .and_then(|o| o.title.clone())
                        .or_else(|| resolved.title.clone()),
                    description: ov
                        .and_then(|o| o.description.clone())
                        .or_else(|| resolved.description.clone()),
                    meta: ov
                        .and_then(|o| o.meta.clone())
                        .or_else(|| resolved.meta.clone()),
                    aliases: resolved.aliases.clone(),
                    ..Default::default()
                };
                view.segments.insert(name, segment);
            }
            "hierarchies" => {
                let Some(resolved) = source_cube.hierarchies.get(source_member_name) else {
                    continue;
                };
                let hierarchy = Hierarchy {
                    title: ov
                        .and_then(|o| o.title.clone())
                        .or_else(|| resolved.title.clone()),
                    public: resolved.public,
                    levels: resolved.levels.clone(),
                    ..Default::default()
                };
                view.hierarchies.insert(name, hierarchy);
            }
            _ => {}
        }
    }
}

/// `CubeEvaluator.prepareHierarchies` for views.
fn prepare_view_hierarchies(
    view: &mut CubeDef,
    cubes: &Members<CubeDef>,
    reporter: &mut ErrorReporter,
) {
    if view.included_members.is_empty() {
        return;
    }

    let included_member_paths: Vec<String> = {
        let mut seen = HashSet::new();
        view.included_members
            .iter()
            .filter(|m| seen.insert(m.member_path.clone()))
            .map(|m| m.member_path.clone())
            .collect()
    };
    let included_cube_names: Vec<String> = {
        let mut seen = HashSet::new();
        included_member_paths
            .iter()
            .filter_map(|p| p.split('.').next().map(str::to_string))
            .filter(|c| seen.insert(c.clone()))
            .collect()
    };

    let hierarchy_members: Vec<&IncludedMember> = view
        .included_members
        .iter()
        .filter(|m| m.member_type == "hierarchies")
        .collect();
    let included_hierarchy_names: Vec<String> = hierarchy_members
        .iter()
        .filter_map(|m| m.member_path.split('.').nth(1).map(str::to_string))
        .collect();
    let hierarchy_path_to_name: Vec<(String, String)> = hierarchy_members
        .iter()
        .map(|m| (m.member_path.clone(), m.name.clone()))
        .collect();

    let mut evaluated: Vec<EvaluatedHierarchy> = Vec::new();

    for cube_name in &included_cube_names {
        let Some(source) = cubes.get(cube_name) else {
            continue;
        };
        for hierarchy in &source.evaluated_hierarchies {
            if !included_hierarchy_names.contains(&hierarchy.name) {
                continue;
            }
            let levels: Vec<String> = hierarchy
                .levels
                .iter()
                .filter(|level| {
                    match view.included_members.iter().find(|m| &&m.member_path == level) {
                        Some(member) if member.member_type != "dimensions" => {
                            let member_name =
                                level.split('.').nth(1).unwrap_or(level.as_str());
                            reporter.error(format!(
                                "Only dimensions can be part of a hierarchy. Please remove the '{member_name}' member from the '{}' hierarchy.",
                                hierarchy.name
                            ));
                            false
                        }
                        Some(_) => included_member_paths.contains(level),
                        None => false,
                    }
                })
                .cloned()
                .collect();

            if levels.is_empty() {
                continue;
            }

            let alias_member = format!("{cube_name}.{}", hierarchy.name);
            let Some((_, name)) = hierarchy_path_to_name
                .iter()
                .find(|(path, _)| path == &alias_member)
            else {
                reporter.error(format!(
                    "Hierarchy '{}' not found in cube '{cube_name}'",
                    hierarchy.name
                ));
                continue;
            };

            evaluated.push(EvaluatedHierarchy {
                name: name.clone(),
                title: view
                    .hierarchies
                    .get(&hierarchy.name)
                    .and_then(|h| h.title.clone())
                    .or_else(|| hierarchy.title.clone()),
                public: hierarchy.public,
                levels,
                alias_member: Some(alias_member),
            });
        }
    }

    // Re-map levels onto the view's own member names.
    for hierarchy in evaluated.iter_mut() {
        hierarchy.levels = hierarchy
            .levels
            .iter()
            .filter_map(|level| {
                view.included_members
                    .iter()
                    .find(|m| &m.member_path == level)
                    .map(|m| format!("{}.{}", view.name, m.name))
            })
            .collect();
    }

    view.evaluated_hierarchies = evaluated;
}

/// `CubeEvaluator.prepareFolders`.
fn prepare_folders(view: &mut CubeDef, reporter: &mut ErrorReporter) {
    if view.folders.is_empty() {
        return;
    }

    let mut seen: HashSet<String> = HashSet::new();
    for folder in &view.folders {
        if !folder.name.is_empty() && !seen.insert(folder.name.clone()) {
            reporter.error(format!(
                "Folder names must be unique within a view. Found duplicate folder '{}' in view '{}'.",
                folder.name, view.name
            ));
        }
    }

    let folders = view.folders.clone();
    let evaluated: Vec<EvaluatedFolder> = folders
        .iter()
        .map(|folder| process_folder(view, folder, reporter))
        .collect();
    view.evaluated_folders = evaluated;
}

fn all_member_names(view: &CubeDef) -> Vec<String> {
    view.measures
        .keys()
        .chain(view.dimensions.keys())
        .chain(view.segments.keys())
        .cloned()
        .collect()
}

fn check_folder_member(
    view: &CubeDef,
    member_name: &str,
    folder_name: &str,
    reporter: &mut ErrorReporter,
) -> Option<IncludedMember> {
    if member_name.contains('.') {
        reporter.error(format!(
            "Paths aren't allowed in the 'folders' but '{member_name}' has been provided for {}",
            view.name
        ));
    }
    match view.included_members.iter().find(|m| m.name == member_name) {
        Some(m) => Some(m.clone()),
        None => {
            reporter.error(format!(
                "Member '{member_name}' included in folder '{folder_name}' not found"
            ));
            None
        }
    }
}

fn process_folder(
    view: &CubeDef,
    folder: &Folder,
    reporter: &mut ErrorReporter,
) -> EvaluatedFolder {
    let includes: Vec<EvaluatedFolderItem> = match &folder.includes {
        FolderIncludes::All(_) => all_member_names(view)
            .iter()
            .filter_map(|m| check_folder_member(view, m, &folder.name, reporter))
            .map(EvaluatedFolderItem::Member)
            .collect(),
        FolderIncludes::List(items) => items
            .iter()
            .flat_map(|item| match item {
                FolderInclude::JoinPath(join_path) => folder_members_from_join_path(
                    view,
                    join_path.join_path.as_deref().unwrap_or(""),
                    &folder.name,
                    reporter,
                )
                .into_iter()
                .map(EvaluatedFolderItem::Member)
                .collect::<Vec<_>>(),
                FolderInclude::Nested(nested) => {
                    vec![EvaluatedFolderItem::Folder(process_folder(
                        view, nested, reporter,
                    ))]
                }
                FolderInclude::Member(name) => {
                    check_folder_member(view, name, &folder.name, reporter)
                        .map(EvaluatedFolderItem::Member)
                        .into_iter()
                        .collect()
                }
            })
            .collect(),
        FolderIncludes::None => Vec::new(),
    };

    EvaluatedFolder {
        name: folder.name.clone(),
        includes,
    }
}

/// `CubeEvaluator.getFolderMembersFromJoinPath`.
fn folder_members_from_join_path(
    view: &CubeDef,
    full_path: &str,
    folder_name: &str,
    reporter: &mut ErrorReporter,
) -> Vec<IncludedMember> {
    let path_cube_name = full_path.split('.').next_back().unwrap_or_default();

    let matching = view
        .cubes
        .iter()
        .any(|c| c.join_path.as_deref() == Some(full_path));
    if !matching {
        reporter.error(format!(
            "Join path '{full_path}' included in folder '{folder_name}' not found in view '{}' cubes definition",
            view.name
        ));
        return Vec::new();
    }

    view.included_members
        .iter()
        .filter(|m| m.member_path.split('.').next() == Some(path_cube_name))
        .cloned()
        .collect()
}
