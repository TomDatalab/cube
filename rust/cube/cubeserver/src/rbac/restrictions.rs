//! Data restrictions of an organization role: columns it blocks and row
//! filters driven by user attributes.

use std::collections::{BTreeMap, BTreeSet};

use cubequery::types::{FilterOperator, NormalizedFilter, NormalizedFilterLeaf, NormalizedQuery};
use serde::{Deserialize, Serialize};

use super::dependencies::{Dep, MemberGraph};
use super::enforce::members_of;
use crate::error::ApiError;

const MAX_ROW_FILTERS: usize = 50;
const MAX_BLOCKED_MEMBERS: usize = 500;
const MAX_ATTRIBUTES: usize = 50;
const MAX_VALUES: usize = 100;
const MAX_VALUE_LEN: usize = 200;
const MAX_NAME_LEN: usize = 64;

/// What a role hides from its holders. Empty means no restriction.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Restrictions {
    #[serde(default)]
    pub row_filters: Vec<RowFilter>,
    #[serde(default)]
    pub blocked_members: Vec<String>,
}

/// Keeps only the rows where `member` matches the values from exactly one
/// source: the user's `attribute`, or the fixed `values`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RowFilter {
    pub member: String,
    pub operator: RowOperator,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribute: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RowOperator {
    Equals,
    NotEquals,
}

/// A user's attributes: name to values.
pub type Attributes = BTreeMap<String, Vec<String>>;

impl Restrictions {
    /// Validates (400 on failure) and normalizes: `blocked_members` come out
    /// without repeats, in order.
    pub fn validated(mut self) -> Result<Restrictions, ApiError> {
        if self.row_filters.len() > MAX_ROW_FILTERS {
            return Err(ApiError::bad_request(format!(
                "at most {MAX_ROW_FILTERS} row filters"
            )));
        }
        if self.blocked_members.len() > MAX_BLOCKED_MEMBERS {
            return Err(ApiError::bad_request(format!(
                "at most {MAX_BLOCKED_MEMBERS} blocked members"
            )));
        }
        for filter in &self.row_filters {
            validate_member(&filter.member)?;
            match (&filter.attribute, &filter.values) {
                (Some(attribute), None) => validate_attribute_name(attribute)?,
                (None, Some(values)) => {
                    if values.is_empty() {
                        return Err(ApiError::bad_request(
                            "a row filter needs at least one value",
                        ));
                    }
                    validate_values(values)?;
                }
                _ => {
                    return Err(ApiError::bad_request(
                        "a row filter takes exactly one of attribute and values",
                    ))
                }
            }
        }
        for member in &self.blocked_members {
            validate_member(member)?;
        }
        self.blocked_members.sort();
        self.blocked_members.dedup();
        Ok(self)
    }

    pub fn is_empty(&self) -> bool {
        self.row_filters.is_empty() && self.blocked_members.is_empty()
    }
}

/// `cube.member`, each part made of `[A-Za-z0-9_]`.
fn validate_member(member: &str) -> Result<(), ApiError> {
    let ok = member.split_once('.').is_some_and(|(cube, name)| {
        let part =
            |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        part(cube) && part(name)
    });
    if ok {
        Ok(())
    } else {
        Err(ApiError::bad_request(format!(
            "invalid member `{member}`: expected cube.member"
        )))
    }
}

fn validate_values(values: &[String]) -> Result<(), ApiError> {
    if values.len() > MAX_VALUES {
        return Err(ApiError::bad_request(format!(
            "at most {MAX_VALUES} values"
        )));
    }
    if values.iter().any(|v| v.chars().count() > MAX_VALUE_LEN) {
        return Err(ApiError::bad_request(format!(
            "values have at most {MAX_VALUE_LEN} characters"
        )));
    }
    Ok(())
}

/// `a-z`, `0-9` and `_`, 1 to 64 characters.
pub fn validate_attribute_name(name: &str) -> Result<(), ApiError> {
    let ok = !name.is_empty()
        && name.len() <= MAX_NAME_LEN
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(ApiError::bad_request(format!(
            "invalid attribute name `{name}`: use a-z, 0-9 and _, up to {MAX_NAME_LEN} characters"
        )))
    }
}

/// Checks names, counts and value sizes. An empty value list is allowed.
pub fn validated_attributes(attributes: Attributes) -> Result<Attributes, ApiError> {
    if attributes.len() > MAX_ATTRIBUTES {
        return Err(ApiError::bad_request(format!(
            "at most {MAX_ATTRIBUTES} attributes"
        )));
    }
    for (name, values) in &attributes {
        validate_attribute_name(name)?;
        validate_values(values)?;
    }
    Ok(attributes)
}

/// What holds for a user in a workspace (or, for the meta without a
/// workspace, in all of them): every role's restrictions together.
#[derive(Debug, Clone, Default)]
pub struct Effective {
    pub row_filters: Vec<RowFilter>,
    pub blocked: BTreeSet<String>,
    pub attributes: Attributes,
}

/// The cube part of `cube.member`.
fn cube_of(member: &str) -> &str {
    member.split('.').next().unwrap_or(member)
}

/// `cube.member` of a name that may carry a granularity, resolved to the
/// cube member a view member points at.
fn resolved<'a>(graph: &'a MemberGraph, name: &'a str) -> &'a str {
    let two = match name.match_indices('.').nth(1) {
        Some((idx, _)) => &name[..idx],
        None => name,
    };
    graph.source_member(two).unwrap_or(two)
}

impl Effective {
    /// Rule B: filters of every role apply together (AND), blocked members
    /// add up.
    pub fn combine<'a>(
        roles: impl IntoIterator<Item = &'a Restrictions>,
        attributes: Attributes,
    ) -> Effective {
        let mut effective = Effective {
            attributes,
            ..Effective::default()
        };
        for role in roles {
            effective
                .row_filters
                .extend(role.row_filters.iter().cloned());
            effective
                .blocked
                .extend(role.blocked_members.iter().cloned());
        }
        effective
    }

    /// No filter and no blocked member.
    pub fn is_empty(&self) -> bool {
        self.row_filters.is_empty() && self.blocked.is_empty()
    }

    /// The first blocked member in the closure of `name` (the member itself
    /// first), where `Dep::Cube(c)` hits any blocked member of `c`. Cubes
    /// only joined through by a view do not count.
    pub fn blocked_dependency(&self, graph: &MemberGraph, name: &str) -> Option<String> {
        if self.blocked.is_empty() {
            return None;
        }
        let blocked: Vec<(&str, &str)> = self
            .blocked
            .iter()
            .map(|b| (b.as_str(), graph.source_member(b).unwrap_or(b)))
            .collect();
        let deps = graph.closure(name);
        let own = resolved(graph, name);
        if deps.contains(&Dep::Member(own.to_string())) {
            if let Some((b, _)) = blocked.iter().find(|(_, source)| *source == own) {
                return Some(b.to_string());
            }
        }
        deps.iter().find_map(|dep| {
            let hit = match dep {
                Dep::Member(member) => blocked.iter().find(|(_, source)| source == member),
                Dep::Cube(cube) => blocked.iter().find(|(_, source)| cube_of(source) == cube),
                Dep::Joined(_) => None,
            };
            hit.map(|(b, _)| b.to_string())
        })
    }
}

/// Applies `effective` to one query: 403 when it names a blocked member, or
/// one built on a blocked member; otherwise the row filters of every cube it
/// reads are ANDed to its filters (the user's are never rewritten). A filter
/// that cannot be applied safely is a 403, never skipped.
pub fn restrict_query(
    mut query: NormalizedQuery,
    effective: &Effective,
    graph: &MemberGraph,
) -> Result<NormalizedQuery, ApiError> {
    if effective.is_empty() {
        return Ok(query);
    }
    let names = members_of(&query)?;

    for blocked in &effective.blocked {
        if graph.source_member(blocked).is_none() {
            tracing::warn!("Blocked member {blocked} is not in the data model; ignored");
        }
    }
    for name in &names {
        if let Some(b) = effective.blocked_dependency(graph, name) {
            let message = if resolved(graph, &b) == resolved(graph, name) {
                format!("Permission denied: {b} is not available to you")
            } else {
                format!("Permission denied: {name} depends on {b}, which is not available to you")
            };
            return Err(ApiError::forbidden(message));
        }
    }
    if effective.row_filters.is_empty() {
        return Ok(query);
    }

    let read_cubes: BTreeSet<String> = names
        .iter()
        .flat_map(|name| graph.closure(name))
        .map(|dep| dep.cube().to_string())
        .collect();
    // The planner may join through cubes the query does not name.
    let read_cubes = graph.cubes_between(&read_cubes);
    let opaque = graph.opaque_reads(&names);
    let direct = names.iter().any(|name| !graph.is_view(cube_of(name)));
    let views: BTreeSet<&str> = names
        .iter()
        .map(|name| cube_of(name))
        .filter(|first| graph.is_view(first))
        .collect();

    let mut leaves = Vec::new();
    for filter in &effective.row_filters {
        // A view member stands for the cube member it shows: the filter is
        // that cube member's from here on.
        let source = graph.source_member(&filter.member);
        let cube = cube_of(source.unwrap_or(&filter.member));
        if !read_cubes.contains(cube) {
            continue;
        }
        let Some(member) = source else {
            return Err(ApiError::forbidden(format!(
                "Permission denied: a restriction of yours filters on {}, which the model no longer has",
                filter.member
            )));
        };
        // On a measure it would be a HAVING, on a segment a segment: neither
        // keeps rows out.
        if !graph.is_dimension(member) {
            return Err(ApiError::forbidden(format!(
                "Permission denied: your row filter on {} is not on a dimension",
                filter.member
            )));
        }
        // A read the injected filter does not reach. The planner propagates
        // the outer dimension filters into a sub_query dimension that asks
        // for it, but not those whose member reaches a sub_query dimension,
        // at any depth.
        if let Some(read) = opaque
            .iter()
            .find(|read| read.cube == cube && (!read.propagates || graph.reaches_sub_query(member)))
        {
            return Err(ApiError::forbidden(format!(
                "Permission denied: your row filter on {cube} cannot be applied inside {}",
                read.inside
            )));
        }
        let values = match (&filter.values, &filter.attribute) {
            (Some(values), _) if !values.is_empty() => values.clone(),
            (None, Some(attribute)) => match effective.attributes.get(attribute) {
                Some(values) if !values.is_empty() => values.clone(),
                _ => {
                    return Err(ApiError::forbidden(format!(
                        "Permission denied: you have no {attribute} attribute, which {cube} needs"
                    )))
                }
            },
            // Validation never stores this; still, no values, no query.
            _ => {
                return Err(ApiError::forbidden(format!(
                    "Permission denied: a restriction of yours on {member} has no values"
                )))
            }
        };
        let operator = match filter.operator {
            RowOperator::Equals => FilterOperator::Equals,
            RowOperator::NotEquals => FilterOperator::NotEquals,
        };
        let leaf = |member: &str| {
            NormalizedFilter::Leaf(NormalizedFilterLeaf {
                member: member.to_string(),
                operator,
                values: Some(values.iter().cloned().map(Some).collect()),
            })
        };

        if direct {
            leaves.push(leaf(member));
        }
        let mut through_view = false;
        for view in views
            .iter()
            .filter(|view| graph.view_includes_cube(view, cube))
        {
            through_view = true;
            match graph.view_member_for(view, member) {
                Some(view_member) => leaves.push(leaf(view_member)),
                None => {
                    return Err(ApiError::forbidden(format!(
                        "Permission denied: view {view} does not expose {member}, which filters your rows"
                    )))
                }
            }
        }
        // Read only through a dependency: the cube member itself; the
        // planner joins it in or fails, both safe.
        if !direct && !through_view {
            leaves.push(leaf(member));
        }
    }
    query.filters.extend(leaves);
    Ok(query)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cubequery::types::{FilterOperator, NormalizedFilter, NormalizedFilterLeaf};
    use serde_json::{json, Value};
    use std::collections::BTreeSet;

    fn parse(v: Value) -> Result<Restrictions, serde_json::Error> {
        serde_json::from_value(v)
    }

    fn filter(member: &str, attribute: Option<&str>, values: Option<Vec<&str>>) -> RowFilter {
        RowFilter {
            member: member.into(),
            operator: RowOperator::Equals,
            attribute: attribute.map(Into::into),
            values: values.map(|v| v.into_iter().map(Into::into).collect()),
        }
    }

    fn with_filter(f: RowFilter) -> Restrictions {
        Restrictions {
            row_filters: vec![f],
            blocked_members: vec![],
        }
    }

    #[test]
    fn restrictions_accept_the_spec_example() {
        let r = parse(json!({
            "rowFilters": [
                { "member": "orders.region", "operator": "equals", "attribute": "region" },
                { "member": "orders.status", "operator": "notEquals", "values": ["internal"] }
            ],
            "blockedMembers": ["orders.margin", "customers.cpf"]
        }))
        .unwrap();
        assert_eq!(r.row_filters[1].operator, RowOperator::NotEquals);
        let r = r.validated().unwrap();
        assert_eq!(r.row_filters.len(), 2);
        assert!(!r.is_empty());
        assert!(Restrictions::default().is_empty());
    }

    #[test]
    fn a_filter_takes_exactly_one_value_source() {
        assert!(with_filter(filter("a.b", Some("x"), Some(vec!["1"])))
            .validated()
            .is_err());
        assert!(with_filter(filter("a.b", None, None)).validated().is_err());
        assert!(with_filter(filter("a.b", None, Some(vec![])))
            .validated()
            .is_err());
        assert!(with_filter(filter("a.b", Some("x"), None))
            .validated()
            .is_ok());
        assert!(with_filter(filter("a.b", None, Some(vec!["1"])))
            .validated()
            .is_ok());
    }

    #[test]
    fn members_must_be_cube_dot_member() {
        for bad in ["orders", "orders.", "a.b.c", "or ders.x"] {
            let blocked = Restrictions {
                row_filters: vec![],
                blocked_members: vec![bad.into()],
            };
            assert!(blocked.validated().is_err(), "{bad}");
            assert!(
                with_filter(filter(bad, Some("x"), None))
                    .validated()
                    .is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn unknown_operators_and_fields_are_refused() {
        assert!(parse(json!({"rowFilters": [
            {"member": "a.b", "operator": "gt", "attribute": "x"}
        ]}))
        .is_err());
        assert!(parse(json!({"rowFilters": [
            {"member": "a.b", "operator": "equals", "attribute": "x", "extra": 1}
        ]}))
        .is_err());
        assert!(parse(json!({"other": []})).is_err());
    }

    #[test]
    fn blocked_members_are_deduplicated() {
        let r = Restrictions {
            row_filters: vec![],
            blocked_members: vec!["b.y".into(), "a.x".into(), "b.y".into()],
        }
        .validated()
        .unwrap();
        assert_eq!(r.blocked_members, vec!["a.x", "b.y"]);
    }

    #[test]
    fn limits_are_enforced() {
        let many = Restrictions {
            row_filters: (0..51).map(|_| filter("a.b", Some("x"), None)).collect(),
            blocked_members: vec![],
        };
        assert!(many.validated().is_err());
        let ok = Restrictions {
            row_filters: (0..50).map(|_| filter("a.b", Some("x"), None)).collect(),
            blocked_members: (0..500).map(|i| format!("a.m{i}")).collect(),
        };
        assert!(ok.validated().is_ok());
        let blocked = Restrictions {
            row_filters: vec![],
            blocked_members: (0..501).map(|i| format!("a.m{i}")).collect(),
        };
        assert!(blocked.validated().is_err());
    }

    #[test]
    fn attribute_names_follow_the_identifier_rule() {
        assert!(validate_attribute_name("Region").is_err());
        assert!(validate_attribute_name("a-b").is_err());
        assert!(validate_attribute_name("").is_err());
        assert!(validate_attribute_name(&"a".repeat(65)).is_err());
        assert!(validate_attribute_name("cost_center").is_ok());
        assert!(validate_attribute_name(&"a".repeat(64)).is_ok());
    }

    #[test]
    fn attribute_values_are_bounded() {
        let one = |values: Vec<String>| Attributes::from([("region".to_string(), values)]);
        assert!(validated_attributes(one((0..101).map(|i| i.to_string()).collect())).is_err());
        assert!(validated_attributes(one((0..100).map(|i| i.to_string()).collect())).is_ok());
        assert!(validated_attributes(one(vec!["x".repeat(201)])).is_err());
        assert!(validated_attributes(one(vec![])).is_ok());
        let many: Attributes = (0..51).map(|i| (format!("a{i}"), vec![])).collect();
        assert!(validated_attributes(many).is_err());
        assert!(validated_attributes(Attributes::from([("Bad".to_string(), vec![])])).is_err());
    }

    // ---- applying restrictions to queries ----

    const MODEL: &str = r#"
cubes:
  - name: a_orders
    sql_table: orders
    joins:
      - name: b_orders
        sql: "{CUBE}.id = {b_orders}.id"
        relationship: many_to_one
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
      - name: status
        sql: status
        type: string
      - name: region
        sql: region
        type: string
      - name: margin
        sql: margin
        type: number
      - name: margin_pct
        sql: "{margin} * 100"
        type: number
      - name: created_at
        sql: created_at
        type: time
    measures:
      - name: count
        type: count
    segments:
      - name: open
        sql: "{CUBE}.status = 'open'"

  - name: b_orders
    sql_table: orders
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
      - name: status
        sql: status
        type: string
    measures:
      - name: count
        type: count

views:
  - name: a_view
    cubes:
      - join_path: a_orders
        includes: [count, status]
"#;

    fn graph_of(model: &str) -> MemberGraph {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("model.yml"), model).unwrap();
        let model = cubemodel::ModelLoader::with_context(cubemodel::TemplateContext::default())
            .load_dir_with(dir.path())
            .unwrap();
        MemberGraph::build(&model)
    }

    fn model_graph() -> MemberGraph {
        graph_of(MODEL)
    }

    fn query(v: Value) -> NormalizedQuery {
        let (_, mut queries) =
            cubequery::get_normalized_queries(&v, false, None, &cubequery::QueryConfig::default())
                .unwrap();
        queries.remove(0)
    }

    fn blocking(members: &[&str]) -> Effective {
        Effective::combine(
            [&Restrictions {
                row_filters: vec![],
                blocked_members: members.iter().map(|m| m.to_string()).collect(),
            }],
            Attributes::new(),
        )
    }

    fn filtering(filters: Vec<RowFilter>, attributes: &[(&str, &[&str])]) -> Effective {
        Effective::combine(
            [&Restrictions {
                row_filters: filters,
                blocked_members: vec![],
            }],
            attributes
                .iter()
                .map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect()))
                .collect(),
        )
    }

    fn leaf(member: &str, operator: FilterOperator, values: &[&str]) -> NormalizedFilter {
        NormalizedFilter::Leaf(NormalizedFilterLeaf {
            member: member.into(),
            operator,
            values: Some(values.iter().map(|v| Some(v.to_string())).collect()),
        })
    }

    fn by_region() -> RowFilter {
        filter("a_orders.region", Some("region"), None)
    }

    fn refused(result: Result<NormalizedQuery, ApiError>) -> String {
        let err = result.unwrap_err();
        assert_eq!(err.status, axum::http::StatusCode::FORBIDDEN, "{err:?}");
        err.body.error
    }

    #[test]
    fn no_restrictions_leave_the_query_untouched() {
        let graph = model_graph();
        let q = query(json!({
            "measures": ["a_orders.count"],
            "dimensions": ["a_orders.margin"],
            "filters": [{ "member": "a_orders.status", "operator": "equals", "values": ["x"] }]
        }));
        let effective = Effective::default();
        assert!(effective.is_empty());
        assert_eq!(restrict_query(q.clone(), &effective, &graph).unwrap(), q);
    }

    #[test]
    fn a_blocked_member_is_refused_wherever_the_query_names_it() {
        let graph = model_graph();
        let effective = blocking(&["a_orders.margin"]);
        for q in [
            json!({ "measures": ["a_orders.count"], "dimensions": ["a_orders.margin"] }),
            json!({ "measures": ["a_orders.count"], "filters": [{ "or": [
                { "member": "a_orders.status", "operator": "set" },
                { "member": "a_orders.margin", "operator": "gt", "values": ["1"] }
            ] }] }),
            json!({ "measures": ["a_orders.count"], "order": [["a_orders.margin", "asc"]] }),
            json!({ "measures": ["a_orders.count"], "segments": ["a_orders.margin"] }),
            json!({ "measures": ["a_orders.count"],
                "timeDimensions": [{ "dimension": "a_orders.margin", "granularity": "day" }] }),
        ] {
            let message = refused(restrict_query(query(q.clone()), &effective, &graph));
            assert_eq!(
                message, "Permission denied: a_orders.margin is not available to you",
                "{q}"
            );
        }
        // Other members of the same cube still run.
        let q = query(json!({ "measures": ["a_orders.count"], "dimensions": ["a_orders.status"] }));
        assert_eq!(restrict_query(q.clone(), &effective, &graph).unwrap(), q);
    }

    #[test]
    fn a_member_built_on_a_blocked_one_is_refused() {
        let graph = model_graph();
        let effective = blocking(&["a_orders.margin"]);
        let q = query(json!({ "dimensions": ["a_orders.margin_pct"] }));
        assert_eq!(
            refused(restrict_query(q, &effective, &graph)),
            "Permission denied: a_orders.margin_pct depends on a_orders.margin, which is not available to you"
        );
        // Through a view, the blocked member is the view member's own.
        let effective = blocking(&["a_orders.status"]);
        let q = query(json!({ "dimensions": ["a_view.status"] }));
        assert_eq!(
            refused(restrict_query(q, &effective, &graph)),
            "Permission denied: a_orders.status is not available to you"
        );
    }

    #[test]
    fn a_row_filter_uses_the_users_attribute() {
        let graph = model_graph();
        let effective = filtering(vec![by_region()], &[("region", &["Sul", "Sudeste"])]);
        let q = query(json!({ "measures": ["a_orders.count"] }));
        let restricted = restrict_query(q, &effective, &graph).unwrap();
        assert_eq!(
            restricted.filters,
            vec![leaf(
                "a_orders.region",
                FilterOperator::Equals,
                &["Sul", "Sudeste"]
            )]
        );
    }

    #[test]
    fn fixed_values_and_not_equals() {
        let graph = model_graph();
        let mut fixed = filter("a_orders.status", None, Some(vec!["internal", "test"]));
        fixed.operator = RowOperator::NotEquals;
        let effective = filtering(vec![fixed], &[]);
        let q = query(json!({ "measures": ["a_orders.count"] }));
        let restricted = restrict_query(q, &effective, &graph).unwrap();
        assert_eq!(
            restricted.filters,
            vec![leaf(
                "a_orders.status",
                FilterOperator::NotEquals,
                &["internal", "test"]
            )]
        );
    }

    #[test]
    fn a_missing_or_empty_attribute_is_refused() {
        let graph = model_graph();
        let q = query(json!({ "measures": ["a_orders.count"] }));
        for attributes in [&[][..], &[("region", &[][..])][..]] {
            let effective = filtering(vec![by_region()], attributes);
            assert_eq!(
                refused(restrict_query(q.clone(), &effective, &graph)),
                "Permission denied: you have no region attribute, which a_orders needs"
            );
        }
    }

    #[test]
    fn filters_only_apply_to_cubes_the_query_reads() {
        let graph = model_graph();
        let effective = filtering(vec![filter("b_orders.status", None, Some(vec!["x"]))], &[]);
        let q = query(json!({ "measures": ["a_orders.count"] }));
        assert_eq!(restrict_query(q.clone(), &effective, &graph).unwrap(), q);
        // A query of b_orders gets it.
        let q = query(json!({ "measures": ["b_orders.count"] }));
        assert_eq!(
            restrict_query(q, &effective, &graph).unwrap().filters,
            vec![leaf("b_orders.status", FilterOperator::Equals, &["x"])]
        );
    }

    #[test]
    fn injected_filters_are_anded_outside_the_users_or() {
        let graph = model_graph();
        let effective = filtering(vec![by_region()], &[("region", &["Sul"])]);
        let q = query(json!({
            "measures": ["a_orders.count"],
            "filters": [{ "or": [
                { "member": "a_orders.status", "operator": "equals", "values": ["a"] },
                { "member": "a_orders.status", "operator": "equals", "values": ["b"] }
            ] }]
        }));
        let users = q.filters.clone();
        assert!(matches!(users[..], [NormalizedFilter::Or(_)]));
        let restricted = restrict_query(q, &effective, &graph).unwrap();
        assert_eq!(
            restricted.filters,
            vec![
                users[0].clone(),
                leaf("a_orders.region", FilterOperator::Equals, &["Sul"])
            ]
        );
    }

    #[test]
    fn a_view_without_the_filtered_member_is_refused() {
        let graph = model_graph();
        let effective = filtering(vec![by_region()], &[("region", &["Sul"])]);
        let q = query(json!({ "measures": ["a_view.count"] }));
        assert_eq!(
            refused(restrict_query(q, &effective, &graph)),
            "Permission denied: view a_view does not expose a_orders.region, which filters your rows"
        );
    }

    #[test]
    fn a_view_exposing_the_member_is_filtered_through_it() {
        let graph = model_graph();
        let effective = filtering(
            vec![filter("a_orders.status", None, Some(vec!["open"]))],
            &[],
        );
        let q = query(json!({ "measures": ["a_view.count"] }));
        assert_eq!(
            restrict_query(q, &effective, &graph).unwrap().filters,
            vec![leaf("a_view.status", FilterOperator::Equals, &["open"])]
        );
    }

    #[test]
    fn filters_from_several_roles_all_apply() {
        let graph = model_graph();
        let first = Restrictions {
            row_filters: vec![by_region()],
            blocked_members: vec!["a_orders.margin".into()],
        };
        let second = Restrictions {
            row_filters: vec![filter("a_orders.status", None, Some(vec!["open"]))],
            blocked_members: vec!["a_orders.id".into(), "a_orders.margin".into()],
        };
        let effective = Effective::combine(
            [&first, &second],
            Attributes::from([("region".to_string(), vec!["Sul".to_string()])]),
        );
        assert_eq!(effective.row_filters.len(), 2);
        assert_eq!(
            effective.blocked,
            BTreeSet::from(["a_orders.id".to_string(), "a_orders.margin".to_string()])
        );
        let q = query(json!({ "measures": ["a_orders.count"] }));
        assert_eq!(
            restrict_query(q, &effective, &graph).unwrap().filters,
            vec![
                leaf("a_orders.region", FilterOperator::Equals, &["Sul"]),
                leaf("a_orders.status", FilterOperator::Equals, &["open"])
            ]
        );
        for blocked in ["a_orders.id", "a_orders.margin"] {
            let q = query(json!({ "dimensions": [blocked] }));
            assert!(restrict_query(q, &effective, &graph).is_err(), "{blocked}");
        }
    }

    #[test]
    fn a_filter_on_a_member_the_model_lost_is_refused() {
        let graph = model_graph();
        let effective = filtering(vec![filter("a_orders.gone", None, Some(vec!["x"]))], &[]);
        let q = query(json!({ "measures": ["a_orders.count"] }));
        assert_eq!(
            refused(restrict_query(q, &effective, &graph)),
            "Permission denied: a restriction of yours filters on a_orders.gone, which the model no longer has"
        );
        // A blocked member the model lost blocks nothing.
        let q = query(json!({ "measures": ["a_orders.count"] }));
        assert!(restrict_query(q, &blocking(&["a_orders.gone"]), &graph).is_ok());
    }

    #[test]
    fn a_filter_named_through_a_view_filters_its_source_cube() {
        let graph = model_graph();
        let effective = filtering(vec![filter("a_view.status", None, Some(vec!["open"]))], &[]);
        let q = query(json!({ "measures": ["a_orders.count"] }));
        assert_eq!(
            restrict_query(q, &effective, &graph).unwrap().filters,
            vec![leaf("a_orders.status", FilterOperator::Equals, &["open"])]
        );
        let q = query(json!({ "measures": ["a_view.count"] }));
        assert_eq!(
            restrict_query(q, &effective, &graph).unwrap().filters,
            vec![leaf("a_view.status", FilterOperator::Equals, &["open"])]
        );
    }

    const OPAQUE_MODEL: &str = r#"
cubes:
  - name: orders
    sql_table: orders
    joins:
      - name: buyers
        sql: "{CUBE}.buyer_id = {buyers}.id"
        relationship: many_to_one
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
      - name: region
        sql: region
        type: string
      - name: big
        sql: "{count} > 10"
        type: boolean
        sub_query: true
        propagate_filters_to_sub_query: true
      - name: big_label
        sql: "CASE WHEN {big} THEN 'big' END"
        type: string
    measures:
      - name: count
        type: count

  - name: buyers
    sql_table: buyers
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
      - name: name
        sql: name
        type: string
      - name: order_count
        sql: "{orders.count}"
        type: number
        sub_query: true
      - name: propagated_count
        sql: "{orders.count}"
        type: number
        sub_query: true
        propagate_filters_to_sub_query: true
    measures:
      - name: count
        type: count

  - name: wrapped
    sql: "SELECT * FROM {orders.sql()}"
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
    measures:
      - name: count
        type: count
"#;

    #[test]
    fn a_row_filter_that_cannot_reach_inside_a_read_is_refused() {
        let graph = graph_of(OPAQUE_MODEL);
        let effective = filtering(
            vec![filter("orders.region", Some("region"), None)],
            &[("region", &["Sul"])],
        );
        for (q, inside) in [
            (
                json!({ "dimensions": ["buyers.order_count"] }),
                "buyers.order_count",
            ),
            // Naming the cube directly does not filter the subquery.
            (
                json!({ "measures": ["orders.count"], "dimensions": ["buyers.order_count"] }),
                "buyers.order_count",
            ),
            (json!({ "measures": ["wrapped.count"] }), "wrapped"),
        ] {
            assert_eq!(
                refused(restrict_query(query(q.clone()), &effective, &graph)),
                format!("Permission denied: your row filter on orders cannot be applied inside {inside}"),
                "{q}"
            );
        }
        // Filters propagated into the subquery carry the injected one.
        let q = query(json!({ "dimensions": ["buyers.propagated_count"] }));
        assert_eq!(
            restrict_query(q, &effective, &graph).unwrap().filters,
            vec![leaf("orders.region", FilterOperator::Equals, &["Sul"])]
        );
        // The subquery's own cube is joined back by its primary key.
        let effective = filtering(vec![filter("buyers.name", None, Some(vec!["x"]))], &[]);
        let q = query(json!({ "dimensions": ["buyers.order_count"] }));
        assert_eq!(
            restrict_query(q, &effective, &graph).unwrap().filters,
            vec![leaf("buyers.name", FilterOperator::Equals, &["x"])]
        );
    }

    #[test]
    fn a_row_filter_on_a_sub_query_dimension_is_not_propagated() {
        // The planner leaves filters on sub_query dimensions out of the
        // filters it propagates.
        let graph = graph_of(OPAQUE_MODEL);
        // Nor those on a member built on one, at any depth.
        for member in ["orders.big", "orders.big_label"] {
            let effective = filtering(vec![filter(member, None, Some(vec!["true"]))], &[]);
            let q = query(json!({ "dimensions": ["buyers.propagated_count"] }));
            assert_eq!(
                refused(restrict_query(q, &effective, &graph)),
                "Permission denied: your row filter on orders cannot be applied inside buyers.propagated_count",
                "{member}"
            );
        }
    }

    #[test]
    fn a_row_filter_must_be_on_a_dimension() {
        let graph = model_graph();
        let q = query(json!({ "measures": ["a_orders.count"] }));
        for member in ["a_orders.count", "a_orders.open"] {
            let effective = filtering(vec![filter(member, None, Some(vec!["1"]))], &[]);
            assert_eq!(
                refused(restrict_query(q.clone(), &effective, &graph)),
                format!("Permission denied: your row filter on {member} is not on a dimension")
            );
        }
        // Named through a view, it is the source member that counts.
        let effective = filtering(vec![filter("a_view.count", None, Some(vec!["1"]))], &[]);
        assert_eq!(
            refused(restrict_query(q, &effective, &graph)),
            "Permission denied: your row filter on a_view.count is not on a dimension"
        );
    }

    #[test]
    fn cubes_joined_through_are_filtered() {
        let graph = graph_of(
            r#"
cubes:
  - name: a
    sql_table: a
    joins:
      - name: b
        sql: "{CUBE}.b_id = {b}.id"
        relationship: many_to_one
    measures:
      - name: count
        type: count
  - name: b
    sql_table: b
    joins:
      - name: c
        sql: "{CUBE}.c_id = {c}.id"
        relationship: many_to_one
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
      - name: region
        sql: region
        type: string
  - name: c
    sql_table: c
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
      - name: name
        sql: name
        type: string
"#,
        );
        let effective = filtering(vec![filter("b.region", None, Some(vec!["Sul"]))], &[]);
        let q = query(json!({ "measures": ["a.count"], "dimensions": ["c.name"] }));
        assert_eq!(
            restrict_query(q, &effective, &graph).unwrap().filters,
            vec![leaf("b.region", FilterOperator::Equals, &["Sul"])]
        );
        // A query of one cube joins nothing.
        let q = query(json!({ "measures": ["a.count"] }));
        assert!(restrict_query(q, &effective, &graph)
            .unwrap()
            .filters
            .is_empty());
    }

    /// D→A, D→C and A→X→Y→C: undirected, A and C are two hops apart
    /// through D; the planner, which follows joins from→to only, joins
    /// A→X→Y→C.
    const DIRECTED_MODEL: &str = r#"
cubes:
  - name: d
    sql_table: d
    joins:
      - name: a
        sql: "{CUBE}.a_id = {a}.id"
        relationship: many_to_one
      - name: c
        sql: "{CUBE}.c_id = {c}.id"
        relationship: many_to_one
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
  - name: a
    sql_table: a
    joins:
      - name: x
        sql: "{CUBE}.x_id = {x}.id"
        relationship: many_to_one
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
    measures:
      - name: count
        type: count
  - name: x
    sql_table: x
    joins:
      - name: y
        sql: "{CUBE}.y_id = {y}.id"
        relationship: many_to_one
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
      - name: region
        sql: region
        type: string
  - name: y
    sql_table: y
    joins:
      - name: c
        sql: "{CUBE}.c_id = {c}.id"
        relationship: many_to_one
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
      - name: region
        sql: region
        type: string
  - name: c
    sql_table: c
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
      - name: name
        sql: name
        type: string
"#;

    #[test]
    fn cubes_a_directed_join_path_goes_through_are_filtered() {
        let graph = graph_of(DIRECTED_MODEL);
        let effective = filtering(
            vec![
                filter("x.region", None, Some(vec!["Sul"])),
                filter("y.region", None, Some(vec!["Norte"])),
            ],
            &[],
        );
        let q = query(json!({ "measures": ["a.count"], "dimensions": ["c.name"] }));
        assert_eq!(
            restrict_query(q, &effective, &graph).unwrap().filters,
            vec![
                leaf("x.region", FilterOperator::Equals, &["Sul"]),
                leaf("y.region", FilterOperator::Equals, &["Norte"]),
            ]
        );
    }
}
