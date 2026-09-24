//! Validation reproducing the messages `CubeValidator` emits for the cases that
//! are actually hit by YAML models.
//!
//! Joi renders every problem as `(<path> = <value>) <reason>` (see
//! `formatErrorMessage`); this module produces the same per-problem strings but
//! reports each one separately instead of folding them into Joi's
//! `Possible reasons (one of):` aggregate — see the crate docs.

use serde_json::Value;

use crate::error::ErrorReporter;
use crate::model::{CubeDef, DataModel, Includes};

const MEASURE_TYPES: &[&str] = &[
    "number",
    "string",
    "boolean",
    "time",
    "sum",
    "avg",
    "min",
    "max",
    "countDistinct",
    "countDistinctApprox",
];

const MEASURE_TYPES_WITH_COUNT: &[&str] = &[
    "count",
    "number",
    "string",
    "boolean",
    "time",
    "sum",
    "avg",
    "min",
    "max",
    "countDistinct",
    "countDistinctApprox",
];

const DIMENSION_TYPES: &[&str] = &["string", "number", "boolean", "time", "geo"];

const RELATIONSHIPS: &[&str] = &[
    "belongsTo",
    "belongs_to",
    "many_to_one",
    "manyToOne",
    "hasMany",
    "has_many",
    "one_to_many",
    "oneToMany",
    "hasOne",
    "has_one",
    "one_to_one",
    "oneToOne",
];

const PRE_AGGREGATION_TYPES: &[&str] = &[
    "autoRollup",
    "originalSql",
    "rollup",
    "rollupJoin",
    "rollupLambda",
];

const PREDEFINED_GRANULARITIES: &[&str] = &[
    "second", "minute", "hour", "day", "week", "month", "quarter", "year",
];

const PARTITION_GRANULARITIES: &[&str] = &["hour", "day", "week", "month", "quarter", "year"];

fn render_value(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

fn required(reporter: &mut ErrorReporter, path: &str) {
    reporter.error(format!("({path}) is required"));
}

fn must_be_one_of(reporter: &mut ErrorReporter, path: &str, value: &str, allowed: &[&str]) {
    reporter.error(format!(
        "({path} = {value}) must be one of [{}]",
        allowed.join(", ")
    ));
}

fn not_allowed(reporter: &mut ErrorReporter, path: &str, value: &Value) {
    reporter.error(format!("({path} = {}) is not allowed", render_value(value)));
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Validates every cube and view in the model.
pub fn validate_model(model: &DataModel, reporter: &mut ErrorReporter) {
    for cube in model.cube_list() {
        reporter.in_file(cube.file_name.clone());
        reporter.push_context(format!("{} cube", cube.name));
        validate_cube(cube, model, reporter);
        reporter.pop_context();
    }
    reporter.exit_file();
}

fn validate_cube(cube: &CubeDef, model: &DataModel, reporter: &mut ErrorReporter) {
    if !is_identifier(&cube.name) {
        reporter.error(format!(
            "(name = {}) with value \"{}\" fails to match the identifier pattern",
            cube.name, cube.name
        ));
    }

    for (key, value) in cube.extra.iter() {
        not_allowed(reporter, key, value);
    }

    if cube.is_view {
        validate_view_cubes(cube, reporter);
    } else {
        let has_sql = cube.sql.is_some();
        let has_sql_table = cube.sql_table.is_some();
        if has_sql == has_sql_table {
            reporter.error("You must use either sql or sqlTable within a model, but not both");
        }
    }

    check_duplicate_member_names(cube, reporter);

    validate_joins(cube, model, reporter);
    validate_measures(cube, reporter);
    validate_dimensions(cube, reporter);
    validate_segments(cube, reporter);
    validate_hierarchies(cube, reporter);
    validate_pre_aggregations(cube, reporter);
}

/// `CubeSymbols.transform`'s duplicate detection across all member namespaces.
fn check_duplicate_member_names(cube: &CubeDef, reporter: &mut ErrorReporter) {
    let mut counts: Vec<(&String, usize)> = Vec::new();
    let mut all: Vec<&String> = Vec::new();
    all.extend(cube.measures.keys());
    all.extend(cube.dimensions.keys());
    all.extend(cube.segments.keys());
    all.extend(cube.pre_aggregations.keys());
    all.extend(cube.hierarchies.keys());

    for name in all {
        match counts.iter_mut().find(|(n, _)| *n == name) {
            Some((_, count)) => *count += 1,
            None => counts.push((name, 1)),
        }
    }

    let duplicates: Vec<String> = counts
        .into_iter()
        .filter(|(_, c)| *c > 1)
        .map(|(n, _)| n.clone())
        .collect();

    if !duplicates.is_empty() {
        reporter.error(format!("{} defined more than once", duplicates.join(", ")));
    }
}

fn validate_joins(cube: &CubeDef, model: &DataModel, reporter: &mut ErrorReporter) {
    for (index, join) in cube.joins.iter().enumerate() {
        let base = format!("joins[{index}]");
        if join.name.is_empty() {
            required(reporter, &format!("{base}.name"));
        } else if !is_identifier(&join.name) {
            reporter.error(format!(
                "({base}.name = {}) with value \"{}\" fails to match the identifier pattern",
                join.name, join.name
            ));
        }
        if join.sql.is_none() {
            required(reporter, &format!("{base}.sql"));
        }
        match &join.relationship {
            None => required(reporter, &format!("{base}.relationship")),
            Some(rel) if !RELATIONSHIPS.contains(&rel.as_str()) => {
                must_be_one_of(
                    reporter,
                    &format!("{base}.relationship"),
                    rel,
                    RELATIONSHIPS,
                );
            }
            Some(_) => {}
        }
        for (key, value) in join.extra.iter() {
            not_allowed(reporter, &format!("{base}.{key}"), value);
        }
        if !join.name.is_empty() && !model.contains(&join.name) {
            reporter.error(format!("Cube {} doesn't exist", join.name));
        }
    }
}

fn validate_measures(cube: &CubeDef, reporter: &mut ErrorReporter) {
    for (name, measure) in cube.measures.iter() {
        let base = format!("measures.{name}");
        if !is_identifier(name) {
            reporter.error(format!(
                "({base}) with value \"{name}\" fails to match the identifier pattern"
            ));
        }
        match &measure.member_type {
            None => required(reporter, &format!("{base}.type")),
            Some(t) if t == "count" => {}
            Some(t) if MEASURE_TYPES.contains(&t.as_str()) => {
                if measure.sql.is_none() && !cube.is_view {
                    required(reporter, &format!("{base}.sql"));
                }
            }
            Some(t) => must_be_one_of(
                reporter,
                &format!("{base}.type"),
                t,
                MEASURE_TYPES_WITH_COUNT,
            ),
        }

        if measure.currency.is_some() {
            if let Some(t) = &measure.member_type {
                if matches!(t.as_str(), "string" | "boolean" | "time") {
                    reporter.error(format!(
                        "\"currency\" property is not allowed for measures of type \"{t}\""
                    ));
                }
            }
        }

        for (key, value) in measure.extra.iter() {
            not_allowed(reporter, &format!("{base}.{key}"), value);
        }
    }
}

fn validate_dimensions(cube: &CubeDef, reporter: &mut ErrorReporter) {
    for (name, dimension) in cube.dimensions.iter() {
        let base = format!("dimensions.{name}");
        if !is_identifier(name) {
            reporter.error(format!(
                "({base}) with value \"{name}\" fails to match the identifier pattern"
            ));
        }

        match &dimension.member_type {
            None => required(reporter, &format!("{base}.type")),
            Some(t) if t == "switch" => {
                if dimension.values.is_empty() {
                    reporter.error(format!("({base}.values) must contain at least 1 items"));
                }
            }
            Some(t) if DIMENSION_TYPES.contains(&t.as_str()) => {}
            Some(t) => must_be_one_of(reporter, &format!("{base}.type"), t, DIMENSION_TYPES),
        }

        let is_switch = dimension.member_type.as_deref() == Some("switch");
        let geo = dimension.latitude.is_some() && dimension.longitude.is_some();
        if dimension.sql.is_none()
            && !geo
            && dimension.case.is_none()
            && !is_switch
            && !cube.is_view
        {
            required(reporter, &format!("{base}.sql"));
        }

        if dimension.currency.is_some() && dimension.member_type.as_deref() != Some("number") {
            reporter
                .error("\"currency\" property can only be used with dimensions of type \"number\"");
        }

        if !dimension.granularities.is_empty() && dimension.member_type.as_deref() != Some("time") {
            reporter.error(format!("({base}.granularities) is not allowed"));
        }

        for (granularity_name, granularity) in dimension.granularities.iter() {
            let gbase = format!("{base}.granularities.{granularity_name}");
            if granularity.sql.is_some() {
                if !PREDEFINED_GRANULARITIES.contains(&granularity_name.to_lowercase().as_str()) {
                    reporter.error(format!(
                        "dimensions.{name}.granularities.{granularity_name}: a granularity defined with 'sql' must be named after one of the predefined granularities ({}). Define '{granularity_name}' with 'interval' instead",
                        PREDEFINED_GRANULARITIES.join(", ")
                    ));
                }
            } else if granularity.interval.is_none() {
                required(reporter, &format!("{gbase}.interval"));
            } else if let Some(interval) = &granularity.interval {
                if !is_valid_interval(interval) {
                    reporter.error(format!(
                        "({gbase}.interval = {interval}) does not match regexp: /^(-?\\d+) (minute|hour|day|week|month|quarter|year)s?$/"
                    ));
                }
            }
        }

        if let Some(order) = &dimension.order {
            if order != "asc" && order != "desc" {
                must_be_one_of(reporter, &format!("{base}.order"), order, &["asc", "desc"]);
            }
        }

        for (key, value) in dimension.extra.iter() {
            not_allowed(reporter, &format!("{base}.{key}"), value);
        }
    }
}

fn is_valid_interval(interval: &str) -> bool {
    let parts: Vec<&str> = interval.split_whitespace().collect();
    if parts.len() != 2 {
        return false;
    }
    if parts[0].parse::<i64>().is_err() {
        return false;
    }
    let unit = parts[1].trim_end_matches('s');
    PREDEFINED_GRANULARITIES.contains(&unit)
}

fn validate_segments(cube: &CubeDef, reporter: &mut ErrorReporter) {
    for (name, segment) in cube.segments.iter() {
        let base = format!("segments.{name}");
        if segment.sql.is_none() && !cube.is_view {
            required(reporter, &format!("{base}.sql"));
        }
        for (key, value) in segment.extra.iter() {
            not_allowed(reporter, &format!("{base}.{key}"), value);
        }
    }
}

fn validate_hierarchies(cube: &CubeDef, reporter: &mut ErrorReporter) {
    for (name, hierarchy) in cube.hierarchies.iter() {
        let base = format!("hierarchies.{name}");
        for (key, value) in hierarchy.extra.iter() {
            not_allowed(reporter, &format!("{base}.{key}"), value);
        }
    }
}

fn validate_pre_aggregations(cube: &CubeDef, reporter: &mut ErrorReporter) {
    for (name, pre_agg) in cube.pre_aggregations.iter() {
        let base = format!("preAggregations.{name}");
        let pre_agg_type = pre_agg.pre_agg_type.clone().unwrap_or_default();

        if pre_agg.pre_agg_type.is_none() {
            required(reporter, &format!("{base}.type"));
        } else if !PRE_AGGREGATION_TYPES.contains(&pre_agg_type.as_str()) {
            must_be_one_of(
                reporter,
                &format!("{base}.type"),
                &pre_agg_type,
                PRE_AGGREGATION_TYPES,
            );
        }

        let time_dimension = pre_agg
            .time_dimension
            .as_ref()
            .or(pre_agg.time_dimension_reference.as_ref());

        match pre_agg_type.as_str() {
            "rollup" => {
                if time_dimension.is_some() && pre_agg.granularity.is_none() {
                    required(reporter, &format!("{base}.granularity"));
                }
                if pre_agg.granularity.is_some() && time_dimension.is_none() {
                    required(reporter, &format!("{base}.timeDimension"));
                }
                if pre_agg.measures.is_empty()
                    && pre_agg.dimensions.is_empty()
                    && time_dimension.is_none()
                {
                    required(reporter, &format!("{base}.measures"));
                }
            }
            "rollupLambda" | "rollupJoin" => {
                if pre_agg.rollups.is_empty() && pre_agg.rollup_references.is_empty() {
                    required(reporter, &format!("{base}.rollups"));
                }
                if time_dimension.is_some() && pre_agg.granularity.is_none() {
                    required(reporter, &format!("{base}.granularity"));
                }
            }
            "originalSql" => {
                if pre_agg.partition_granularity.is_some() && time_dimension.is_none() {
                    required(reporter, &format!("{base}.timeDimension"));
                }
                if pre_agg.scheduled_refresh == Some(true) && pre_agg.refresh_key.is_none() {
                    // matches the `originalSql` scheduledRefresh restriction
                    reporter.error(format!("({base}.scheduledRefresh = true) must be [false]"));
                }
            }
            _ => {}
        }

        if let Some(max_pre_aggregations) = &pre_agg.max_pre_aggregations {
            if !max_pre_aggregations.is_number() {
                reporter.error(format!(
                    "({base}.maxPreAggregations = {}) must be a number",
                    render_value(max_pre_aggregations)
                ));
            }
        }

        if let Some(partition_granularity) = &pre_agg.partition_granularity {
            if !PARTITION_GRANULARITIES.contains(&partition_granularity.as_str()) {
                must_be_one_of(
                    reporter,
                    &format!("{base}.partitionGranularity"),
                    partition_granularity,
                    PARTITION_GRANULARITIES,
                );
            }
        }

        for (key, value) in pre_agg.extra.iter() {
            not_allowed(reporter, &format!("{base}.{key}"), value);
        }
    }
}

fn validate_view_cubes(view: &CubeDef, reporter: &mut ErrorReporter) {
    for (index, include) in view.cubes.iter().enumerate() {
        let base = format!("cubes[{index}]");
        if include.join_path.is_none() {
            required(reporter, &format!("{base}.joinPath"));
        }
        if matches!(include.includes, Includes::None) {
            required(reporter, &format!("{base}.includes"));
        }
        if include.split.unwrap_or(false) && include.prefix.unwrap_or(false) {
            reporter.error("Using split together with prefix is not supported");
        }
        for (key, value) in include.extra.iter() {
            not_allowed(reporter, &format!("{base}.{key}"), value);
        }
    }

    validate_unique_leaf_cubes(view, reporter);
}

/// cube name -> every join path that reaches it under one root.
type CubePaths = Vec<(String, Vec<String>)>;

/// `CubeValidator.validateUniqueLeafCubes`.
fn validate_unique_leaf_cubes(view: &CubeDef, reporter: &mut ErrorReporter) {
    if view.cubes.is_empty() {
        return;
    }

    // root cube -> cube name -> set of paths
    let mut roots: Vec<(String, CubePaths)> = Vec::new();

    for include in &view.cubes {
        let Some(full_path) = &include.join_path else {
            continue;
        };
        let split: Vec<&str> = full_path.split('.').collect();
        let root = split[0].to_string();

        let entry = match roots.iter_mut().find(|(r, _)| *r == root) {
            Some(e) => e,
            None => {
                roots.push((root.clone(), Vec::new()));
                roots.last_mut().unwrap()
            }
        };

        for i in 0..split.len() {
            let cube_name = split[i].to_string();
            let path = split[..=i].join(".");
            match entry.1.iter_mut().find(|(c, _)| *c == cube_name) {
                Some((_, paths)) => {
                    if !paths.contains(&path) {
                        paths.push(path);
                    }
                }
                None => entry.1.push((cube_name, vec![path])),
            }
        }
    }

    for (root, cubes) in roots {
        for (cube_name, paths) in cubes {
            if paths.len() > 1 {
                let path_list = paths
                    .iter()
                    .map(|p| format!("'{p}'"))
                    .collect::<Vec<_>>()
                    .join(", ");
                reporter.error(format!(
                    "Views can't define multiple join paths to the same cube. View '{}' has multiple paths to '{cube_name}' within root '{root}': {path_list}. Use extends to create a child cube and reference it instead",
                    view.name
                ));
            }
        }
    }
}
