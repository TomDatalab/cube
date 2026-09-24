//! Strict, eager checking of a YAML data model.
//!
//! The planner's YAML loader is permissive: an unknown key is dropped, and a
//! member's `sql:` is only parsed when the planner first walks it — so a typo
//! plans a *different* query instead of failing, and an unsupported `sql:`
//! surfaces halfway through a request (or, inside a pre-aggregation reference
//! list, aborts the process).
//!
//! This pass runs over the merged YAML before the model is built, so:
//!
//! - every key is checked against the ones the loader honours, and an unknown
//!   one is reported with the cube and the member it sits on;
//! - every `sql:` the model carries — joins, `case` branches, measure
//!   `filters:` / `drill_filters:` / `order_by:`, granularity `sql:`,
//!   pre-aggregation reference lists — is parsed here, where the cube and
//!   member names are still in hand.

use crate::error::PlannerError;
use crate::member_sql;
use serde_yaml::{Mapping, Value};

/// Keys the loader honours, per construct. A key listed as ignored is
/// documented Cube YAML that does not reach the generated SQL (titles,
/// descriptions, visibility), so accepting it is safe; anything else is a typo
/// until proven otherwise.
struct Keys {
    what: &'static str,
    honoured: &'static [&'static str],
    ignored: &'static [&'static str],
}

/// Presentation-only keys every member may carry.
const MEMBER_IGNORED: &[&str] = &["description", "format", "meta", "public", "shown", "title"];

const CUBE: Keys = Keys {
    what: "cube",
    honoured: &[
        "name",
        "sql",
        "sql_table",
        "calendar",
        "joins",
        "dimensions",
        "measures",
        "segments",
        "pre_aggregations",
    ],
    ignored: &[
        "description",
        "meta",
        "public",
        "shown",
        "title",
        "data_source",
        "refresh_key",
        "rewrite_queries",
        "access_policy",
        "folders",
        "hierarchies",
    ],
};

const VIEW: Keys = Keys {
    what: "view",
    honoured: &[
        "name",
        "cubes",
        "default_filters",
        "dimensions",
        "measures",
        "segments",
    ],
    ignored: &[
        "description",
        "meta",
        "public",
        "shown",
        "title",
        "access_policy",
        "folders",
        "hierarchies",
    ],
};

const VIEW_CUBE: Keys = Keys {
    what: "view cube",
    honoured: &["join_path", "includes", "prefix"],
    ignored: &["alias", "excludes", "split"],
};

const DEFAULT_FILTER: Keys = Keys {
    what: "default filter",
    honoured: &["member", "operator", "values", "unless"],
    ignored: &[],
};

const JOIN: Keys = Keys {
    what: "join",
    honoured: &["name", "sql", "relationship"],
    ignored: &["description", "meta"],
};

const DIMENSION: Keys = Keys {
    what: "dimension",
    honoured: &[
        "name",
        "type",
        "sql",
        "case",
        "multi_stage",
        "add_group_by",
        "sub_query",
        "propagate_filters_to_sub_query",
        "values",
        "primary_key",
        "latitude",
        "longitude",
        "time_shift",
        "granularities",
        "filter",
        "mask",
    ],
    ignored: MEMBER_IGNORED,
};

const MEASURE: Keys = Keys {
    what: "measure",
    honoured: &[
        "name",
        "type",
        "sql",
        "case",
        "multi_stage",
        "reduce_by",
        "add_group_by",
        "group_by",
        "time_shift",
        "rolling_window",
        "filter",
        "grain",
        "filters",
        "drill_filters",
        "order_by",
        "mask",
    ],
    ignored: MEMBER_IGNORED,
};

const SEGMENT: Keys = Keys {
    what: "segment",
    honoured: &["name", "type", "sql"],
    ignored: MEMBER_IGNORED,
};

const GRANULARITY: Keys = Keys {
    what: "granularity",
    honoured: &["name", "interval", "origin", "offset", "sql"],
    ignored: &["title", "description"],
};

const PRE_AGGREGATION: Keys = Keys {
    what: "pre-aggregation",
    honoured: &[
        "name",
        "type",
        "granularity",
        "sql_alias",
        "external",
        "allow_non_strict_date_range_match",
        "measures",
        "dimensions",
        "time_dimension",
        "time_dimensions",
        "segments",
        "partition_granularity",
        "refresh_key",
        "scheduled_refresh",
        "incremental",
        "build_range_start",
        "build_range_end",
        "use_original_sql_pre_aggregations",
        "union_with_source_data",
        "indexes",
        "rollups",
    ],
    ignored: &["description", "meta", "public"],
};

/// Checks one YAML document that has already been merged into
/// `{cubes: [...], views: [...]}`.
pub(crate) fn check_model(root: &Mapping) -> Result<(), PlannerError> {
    for cube in sequence(root, "cubes") {
        check_cube(cube)?;
    }
    for view in sequence(root, "views") {
        check_view(view)?;
    }
    Ok(())
}

fn check_cube(cube: &Value) -> Result<(), PlannerError> {
    let cube = as_mapping(cube, "A `cubes:` entry")?;
    let name = name_of(cube, "cube")?;
    let at = Where::cube(&name);

    check_keys(cube, &CUBE, &at)?;
    check_sql(cube, "sql", &at)?;
    check_sql(cube, "sql_table", &at)?;

    for join in sequence(cube, "joins") {
        let join = as_mapping(join, &format!("A `joins:` entry of cube `{name}`"))?;
        let join_name = name_of(join, "join")?;
        let at = at.member("join", &join_name);
        check_keys(join, &JOIN, &at)?;
        // A join without `sql:` silently joins on nothing, so it is rejected
        // here rather than left to the join graph.
        require(join, "sql", &at)?;
        require(join, "relationship", &at)?;
        check_sql(join, "sql", &at)?;
    }

    check_members(cube, &at)?;

    for pre_aggregation in sequence(cube, "pre_aggregations") {
        let pre_aggregation = as_mapping(
            pre_aggregation,
            &format!("A `pre_aggregations:` entry of cube `{name}`"),
        )?;
        let pre_aggregation_name = name_of(pre_aggregation, "pre-aggregation")?;
        let at = at.member("pre-aggregation", &pre_aggregation_name);
        check_keys(pre_aggregation, &PRE_AGGREGATION, &at)?;
        for key in ["measures", "dimensions", "segments", "rollups"] {
            check_reference_list(pre_aggregation, key, &at)?;
        }
        check_reference(pre_aggregation, "time_dimension", &at)?;
    }

    Ok(())
}

fn check_view(view: &Value) -> Result<(), PlannerError> {
    let view = as_mapping(view, "A `views:` entry")?;
    let name = name_of(view, "view")?;
    let at = Where::view(&name);

    check_keys(view, &VIEW, &at)?;

    for view_cube in sequence(view, "cubes") {
        let view_cube = as_mapping(view_cube, &format!("A `cubes:` entry of view `{name}`"))?;
        check_keys(view_cube, &VIEW_CUBE, &at)?;
        require(view_cube, "join_path", &at)?;
    }

    for filter in sequence(view, "default_filters") {
        let filter = as_mapping(
            filter,
            &format!("A `default_filters:` entry of view `{name}`"),
        )?;
        check_keys(filter, &DEFAULT_FILTER, &at)?;
        require(filter, "member", &at)?;
        require(filter, "operator", &at)?;
    }

    check_members(view, &at)
}

/// The `dimensions:` / `measures:` / `segments:` a cube or a view declares.
fn check_members(owner: &Mapping, at: &Where) -> Result<(), PlannerError> {
    for dimension in sequence(owner, "dimensions") {
        let dimension = as_mapping(dimension, &format!("A `dimensions:` entry of {at}"))?;
        let dimension_name = name_of(dimension, "dimension")?;
        let at = at.member("dimension", &dimension_name);
        check_keys(dimension, &DIMENSION, &at)?;
        require(dimension, "type", &at)?;
        check_sql(dimension, "sql", &at)?;
        check_sql(dimension, "latitude", &at)?;
        check_sql(dimension, "longitude", &at)?;
        check_mask(dimension, &at)?;
        check_case(dimension, &at)?;
        for granularity in sequence(dimension, "granularities") {
            let granularity =
                as_mapping(granularity, &format!("A `granularities:` entry of {at}"))?;
            let granularity_name = name_of(granularity, "granularity")?;
            let at = at.nested("granularity", &granularity_name);
            check_keys(granularity, &GRANULARITY, &at)?;
            check_granularity_interval(granularity, &granularity_name, &at)?;
            check_sql(granularity, "sql", &at)?;
        }
        for time_shift in sequence(dimension, "time_shift") {
            let time_shift = as_mapping(time_shift, &format!("A `time_shift:` entry of {at}"))?;
            check_sql(time_shift, "sql", &at)?;
        }
    }

    for measure in sequence(owner, "measures") {
        let measure = as_mapping(measure, &format!("A `measures:` entry of {at}"))?;
        let measure_name = name_of(measure, "measure")?;
        let at = at.member("measure", &measure_name);
        check_keys(measure, &MEASURE, &at)?;
        require(measure, "type", &at)?;
        check_sql(measure, "sql", &at)?;
        check_mask(measure, &at)?;
        check_case(measure, &at)?;
        for key in ["filters", "drill_filters"] {
            for (index, filter) in sequence(measure, key).iter().enumerate() {
                let filter = as_mapping(filter, &format!("A `{key}:` entry of {at}"))?;
                let at = at.nested(&format!("{key}[{index}]"), "");
                require(filter, "sql", &at)?;
                check_sql(filter, "sql", &at)?;
            }
        }
        for (index, order_by) in sequence(measure, "order_by").iter().enumerate() {
            let order_by = as_mapping(order_by, &format!("An `order_by:` entry of {at}"))?;
            let at = at.nested(&format!("order_by[{index}]"), "");
            require(order_by, "sql", &at)?;
            require(order_by, "dir", &at)?;
            check_sql(order_by, "sql", &at)?;
        }
    }

    for segment in sequence(owner, "segments") {
        let segment = as_mapping(segment, &format!("A `segments:` entry of {at}"))?;
        let segment_name = name_of(segment, "segment")?;
        let at = at.member("segment", &segment_name);
        check_keys(segment, &SEGMENT, &at)?;
        require(segment, "sql", &at)?;
        check_sql(segment, "sql", &at)?;
    }

    Ok(())
}

/// Both `case:` shapes: the labelled one (`when[].sql` + `label`) and the
/// switch one (`switch` + `when[].value`/`sql`).
fn check_case(member: &Mapping, at: &Where) -> Result<(), PlannerError> {
    let Some(case) = member.get(Value::from("case")) else {
        return Ok(());
    };
    let case = as_mapping(case, &format!("The `case:` of {at}"))?;
    let is_switch = case.contains_key(Value::from("switch"));

    if is_switch {
        check_sql(case, "switch", at)?;
    }

    let when = case
        .get(Value::from("when"))
        .ok_or_else(|| PlannerError::model(format!("{at}: a `case:` needs a `when:` list")))?;
    let Value::Sequence(branches) = when else {
        return Err(PlannerError::model(format!(
            "{at}: a `case:`'s `when:` must be a list"
        )));
    };

    for (index, branch) in branches.iter().enumerate() {
        let branch = as_mapping(branch, &format!("A `case.when[{index}]` of {at}"))?;
        let at = at.nested(&format!("case.when[{index}]"), "");
        require(branch, "sql", &at)?;
        check_sql(branch, "sql", &at)?;
        if is_switch {
            require(branch, "value", &at)?;
        } else {
            require(branch, "label", &at)?;
        }
    }

    match case.get(Value::from("else")) {
        None => Err(PlannerError::model(format!(
            "{at}: a `case:` needs an `else:` branch"
        ))),
        Some(else_branch) => {
            let else_branch = as_mapping(else_branch, &format!("The `case.else` of {at}"))?;
            let at = at.nested("case.else", "");
            if is_switch {
                require(else_branch, "sql", &at)?;
                check_sql(else_branch, "sql", &at)
            } else {
                require(else_branch, "label", &at)
            }
        }
    }
}

/// A `mask:` may be a literal or `{sql: …}`; only the second is compiled.
fn check_mask(member: &Mapping, at: &Where) -> Result<(), PlannerError> {
    match member.get(Value::from("mask")) {
        Some(Value::Mapping(mask)) => {
            let at = at.nested("mask", "");
            require(mask, "sql", &at)?;
            check_sql(mask, "sql", &at)
        }
        _ => Ok(()),
    }
}

/// A granularity with no `interval:` is only valid for a predefined name, which
/// the loader synthesizes `1 <name>` for — and panics on otherwise.
fn check_granularity_interval(
    granularity: &Mapping,
    name: &str,
    at: &Where,
) -> Result<(), PlannerError> {
    const PREDEFINED: &[&str] = &[
        "second", "minute", "hour", "day", "week", "month", "quarter", "year",
    ];
    if granularity.contains_key(Value::from("interval")) || PREDEFINED.contains(&name) {
        return Ok(());
    }
    Err(PlannerError::model(format!(
        "{at}: a custom granularity needs an explicit `interval:`"
    )))
}

fn check_sql(owner: &Mapping, key: &str, at: &Where) -> Result<(), PlannerError> {
    let Some(value) = owner.get(Value::from(key)) else {
        return Ok(());
    };
    let Some(sql) = scalar_string(value) else {
        return Err(PlannerError::model(format!(
            "{at}: `{key}:` must be a string"
        )));
    };
    member_sql::parse(&sql)
        .map(|_| ())
        .map_err(|e| PlannerError::unsupported(format!("{at}, `{key}`: {}", e.message())))
}

fn check_reference_list(owner: &Mapping, key: &str, at: &Where) -> Result<(), PlannerError> {
    let Some(value) = owner.get(Value::from(key)) else {
        return Ok(());
    };
    let Value::Sequence(items) = value else {
        return Err(PlannerError::model(format!(
            "{at}: `{key}:` must be a list of member references"
        )));
    };
    let references = items
        .iter()
        .map(|item| {
            scalar_string(item).ok_or_else(|| {
                PlannerError::model(format!(
                    "{at}: every `{key}:` entry must be a member reference"
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if references.is_empty() {
        return Ok(());
    }
    member_sql::parse_reference_list(&references)
        .map(|_| ())
        .map_err(|e| PlannerError::unsupported(format!("{at}, `{key}`: {}", e.message())))
}

fn check_reference(owner: &Mapping, key: &str, at: &Where) -> Result<(), PlannerError> {
    let Some(value) = owner.get(Value::from(key)) else {
        return Ok(());
    };
    let Some(reference) = scalar_string(value) else {
        return Err(PlannerError::model(format!(
            "{at}: `{key}:` must be a member reference"
        )));
    };
    member_sql::parse_reference_list(std::slice::from_ref(&reference))
        .map(|_| ())
        .map_err(|e| PlannerError::unsupported(format!("{at}, `{key}`: {}", e.message())))
}

fn check_keys(owner: &Mapping, keys: &Keys, at: &Where) -> Result<(), PlannerError> {
    for key in owner.keys() {
        let Some(key) = key.as_str() else {
            return Err(PlannerError::model(format!(
                "{at}: every key of a {} must be a name",
                keys.what
            )));
        };
        if keys.honoured.contains(&key) || keys.ignored.contains(&key) {
            continue;
        }
        return Err(PlannerError::model(format!(
            "{at}: unknown {} key `{key}`. Known keys: {}",
            keys.what,
            keys.honoured.join(", ")
        )));
    }
    Ok(())
}

fn require(owner: &Mapping, key: &str, at: &Where) -> Result<(), PlannerError> {
    if owner.contains_key(Value::from(key)) {
        Ok(())
    } else {
        Err(PlannerError::model(format!("{at}: `{key}:` is required")))
    }
}

fn sequence<'a>(owner: &'a Mapping, key: &str) -> &'a [Value] {
    match owner.get(Value::from(key)) {
        Some(Value::Sequence(items)) => items,
        _ => &[],
    }
}

fn as_mapping<'a>(value: &'a Value, what: &str) -> Result<&'a Mapping, PlannerError> {
    value
        .as_mapping()
        .ok_or_else(|| PlannerError::model(format!("{what} must be a mapping")))
}

fn name_of(owner: &Mapping, what: &str) -> Result<String, PlannerError> {
    owner
        .get(Value::from("name"))
        .and_then(scalar_string)
        .ok_or_else(|| PlannerError::model(format!("Every {what} needs a `name:`")))
}

/// YAML scalars a `sql:` may be written as. A number or a boolean is a valid
/// `sql:` in Cube (`sql: 1`), so they are accepted as their text.
fn scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Where in the model an error was found, rendered as the prefix of every
/// message: ``Cube `orders`, measure `revenue` ``.
struct Where {
    prefix: String,
}

impl Where {
    fn cube(name: &str) -> Self {
        Self {
            prefix: format!("Cube `{name}`"),
        }
    }

    fn view(name: &str) -> Self {
        Self {
            prefix: format!("View `{name}`"),
        }
    }

    fn member(&self, what: &str, name: &str) -> Self {
        Self {
            prefix: format!("{}, {what} `{name}`", self.prefix),
        }
    }

    fn nested(&self, what: &str, name: &str) -> Self {
        Self {
            prefix: if name.is_empty() {
                format!("{}, {what}", self.prefix)
            } else {
                format!("{}, {what} `{name}`", self.prefix)
            },
        }
    }
}

impl std::fmt::Display for Where {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.prefix)
    }
}
