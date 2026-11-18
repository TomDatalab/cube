//! What each member of the model reads: the cube members it is built on,
//! directly or through other members, views and cube SQL.
//!
//! Used to refuse queries that touch a blocked member through a member built
//! on it, and to decide which cubes a query reads for row filters. When a
//! reference cannot be resolved the graph depends on more (the whole cube),
//! never on less.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use serde_json::Value;

/// Something a member reads.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Dep {
    /// `cube.member` of a cube, never of a view.
    Member(String),
    /// The whole cube.
    Cube(String),
}

impl Dep {
    /// The cube this dependency belongs to.
    pub fn cube(&self) -> &str {
        match self {
            Dep::Member(member) => member.split('.').next().unwrap_or(member),
            Dep::Cube(cube) => cube,
        }
    }
}

/// Member dependencies of one data model.
#[derive(Debug, Default)]
pub struct MemberGraph {
    /// Cube (not view) name → its dimensions, measures and segments as
    /// `cube.member`, in declaration order.
    cube_members: BTreeMap<String, Vec<String>>,
    /// View names.
    views: BTreeSet<String>,
    /// `cube.member` → what it references directly (itself excluded).
    direct: HashMap<String, BTreeSet<Dep>>,
    /// `view.member` → `cube.member` it was included from.
    view_sources: HashMap<String, String>,
    /// View → (`cube.member` → first `view.member` exposing it).
    view_members: HashMap<String, HashMap<String, String>>,
}

/// Member fields whose value is a bare member path (no braces).
const BARE_REFERENCE_FIELDS: [&str; 5] = [
    "aliasMember",
    "keyReference",
    "reduceBy",
    "groupBy",
    "addGroupBy",
];

impl MemberGraph {
    pub fn build(model: &cubemodel::DataModel) -> MemberGraph {
        let mut graph = MemberGraph::default();

        for cube in model.cube_list() {
            if cube.is_view {
                graph.views.insert(cube.name.clone());
                continue;
            }
            let members: Vec<String> = cube
                .dimensions
                .keys()
                .chain(cube.measures.keys())
                .chain(cube.segments.keys())
                .map(|name| format!("{}.{name}", cube.name))
                .collect();
            // Known before any reference is resolved, whatever the order.
            for member in &members {
                graph.direct.insert(member.clone(), BTreeSet::new());
            }
            graph.cube_members.insert(cube.name.clone(), members);
        }

        for view in model.cube_list().into_iter().filter(|cube| cube.is_view) {
            let index = graph.view_members.entry(view.name.clone()).or_default();
            for included in &view.included_members {
                let view_member = format!("{}.{}", view.name, included.name);
                index
                    .entry(included.member_path.clone())
                    .or_insert_with(|| view_member.clone());
                graph
                    .view_sources
                    .insert(view_member, included.member_path.clone());
            }
        }

        for cube in model.cube_list().into_iter().filter(|cube| !cube.is_view) {
            let mut shared = BTreeSet::new();
            for sql in cube.sql.iter().chain(cube.sql_table.iter()) {
                graph.references_in_str(&cube.name, sql, &mut shared);
            }

            let members = cube
                .dimensions
                .iter()
                .map(|(name, member)| (name, serde_json::to_value(member)))
                .chain(
                    cube.measures
                        .iter()
                        .map(|(name, member)| (name, serde_json::to_value(member))),
                )
                .chain(
                    cube.segments
                        .iter()
                        .map(|(name, member)| (name, serde_json::to_value(member))),
                );
            for (name, value) in members {
                let full = format!("{}.{name}", cube.name);
                let mut deps = shared.clone();
                match value {
                    Ok(value) => graph.references_in_value(&cube.name, &value, &mut deps),
                    // Cannot see what it reads: it may read anything of its cube.
                    Err(_) => {
                        deps.insert(Dep::Cube(cube.name.clone()));
                    }
                }
                deps.remove(&Dep::Member(full.clone()));
                graph.direct.insert(full, deps);
            }
        }

        graph
    }

    /// Transitive closure of what `name` reads: `cube.member`,
    /// `view.member`, `cube.member.granularity` or just `cube`.
    /// Includes the member itself (resolved to the cube's). An unknown name
    /// gives `{Cube(first part)}`.
    pub fn closure(&self, name: &str) -> BTreeSet<Dep> {
        let mut parts = name.split('.');
        let first = parts.next().unwrap_or_default();
        let Some(second) = parts.next() else {
            if self.is_view(first) {
                // A bare view reads every member it includes.
                let mut deps = BTreeSet::new();
                for source in self
                    .view_members
                    .get(first)
                    .into_iter()
                    .flat_map(|m| m.keys())
                {
                    deps.extend(self.closure(source));
                }
                deps.insert(Dep::Cube(first.to_string()));
                return deps;
            }
            return BTreeSet::from([Dep::Cube(first.to_string())]);
        };

        let start = match self.source_member(&format!("{first}.{second}")) {
            Some(member) => member.to_string(),
            None => return BTreeSet::from([Dep::Cube(first.to_string())]),
        };
        if !self.direct.contains_key(&start) {
            // A view member whose source is not a cube member (a hierarchy).
            let cube = start.split('.').next().unwrap_or(first);
            return BTreeSet::from([Dep::Cube(cube.to_string())]);
        }

        let mut visited = BTreeSet::from([Dep::Member(start.clone())]);
        let mut queue = VecDeque::from([start]);
        while let Some(member) = queue.pop_front() {
            for dep in self.direct.get(&member).into_iter().flatten() {
                if visited.insert(dep.clone()) {
                    if let Dep::Member(next) = dep {
                        queue.push_back(next.clone());
                    }
                }
            }
        }
        visited
    }

    /// View member → cube member; cube member → itself; `None` if unknown.
    pub fn source_member(&self, name: &str) -> Option<&str> {
        if let Some(source) = self.view_sources.get(name) {
            return Some(source);
        }
        self.direct.get_key_value(name).map(|(key, _)| key.as_str())
    }

    /// The member of `view` that points at `cube_member`, if the view
    /// exposes it.
    pub fn view_member_for(&self, view: &str, cube_member: &str) -> Option<&str> {
        self.view_members
            .get(view)?
            .get(cube_member)
            .map(String::as_str)
    }

    /// Whether some member of `view` has a source member on `cube`.
    pub fn view_includes_cube(&self, view: &str, cube: &str) -> bool {
        self.view_members.get(view).is_some_and(|members| {
            members
                .keys()
                .any(|source| source.split('.').next() == Some(cube))
        })
    }

    pub fn is_view(&self, name: &str) -> bool {
        self.views.contains(name)
    }

    /// The members (dimensions, measures, segments) of a cube, for
    /// `Dep::Cube` and for the meta.
    pub fn members_of_cube(&self, cube: &str) -> Vec<&str> {
        self.cube_members
            .get(cube)
            .map(|members| members.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }

    fn is_cube(&self, name: &str) -> bool {
        self.cube_members.contains_key(name)
    }

    fn is_cube_member(&self, name: &str) -> bool {
        self.direct.contains_key(name)
    }

    /// Every reference in any string of `value`, plus bare member paths in
    /// the fields that hold them.
    fn references_in_value(&self, cube: &str, value: &Value, deps: &mut BTreeSet<Dep>) {
        match value {
            Value::String(text) => self.references_in_str(cube, text, deps),
            Value::Array(items) => {
                for item in items {
                    self.references_in_value(cube, item, deps);
                }
            }
            Value::Object(fields) => {
                for (key, field) in fields {
                    if BARE_REFERENCE_FIELDS.contains(&key.as_str()) {
                        self.bare_references(cube, field, deps);
                    }
                    self.references_in_value(cube, field, deps);
                }
            }
            _ => {}
        }
    }

    fn bare_references(&self, cube: &str, value: &Value, deps: &mut BTreeSet<Dep>) {
        match value {
            Value::String(path) => self.resolve(cube, path, deps),
            Value::Array(items) => {
                for item in items {
                    self.bare_references(cube, item, deps);
                }
            }
            _ => {}
        }
    }

    /// Each text between `{` and the next `}` is a reference; a `{` with
    /// no `}` after it may hide anything of the cube.
    fn references_in_str(&self, cube: &str, text: &str, deps: &mut BTreeSet<Dep>) {
        for (start, _) in text.match_indices('{') {
            let rest = &text[start + 1..];
            match rest.find('}') {
                Some(end) => {
                    let inner = &rest[..end];
                    // `{a{b}`: the innermost opening brace starts the reference.
                    let inner = inner.rsplit('{').next().unwrap_or(inner);
                    self.resolve(cube, inner, deps);
                }
                None => {
                    deps.insert(Dep::Cube(cube.to_string()));
                }
            }
        }
    }

    /// Resolves one reference `r` written inside `cube`.
    fn resolve(&self, cube: &str, reference: &str, deps: &mut BTreeSet<Dep>) {
        let reference = reference.trim();
        let parts: Vec<&str> = reference.split('.').map(str::trim).collect();
        match parts.as_slice() {
            ["CUBE"] => {}
            ["FILTER_PARAMS", a, b, ..] => self.member_if_known(&format!("{a}.{b}"), deps),
            ["FILTER_PARAMS", ..] | ["SECURITY_CONTEXT", ..] | ["COMPILE_CONTEXT", ..] => {}
            ["CUBE", x, ..] => self.member_if_known(&format!("{cube}.{x}"), deps),
            [x] => {
                let member = format!("{cube}.{x}");
                if self.is_cube_member(&member) {
                    deps.insert(Dep::Member(member));
                } else if self.is_cube(x) {
                    deps.insert(Dep::Cube(x.to_string()));
                } else {
                    deps.insert(Dep::Cube(cube.to_string()));
                }
            }
            [a, b, ..] => {
                let member = format!("{a}.{b}");
                if let Some(source) = self.source_member(&member) {
                    deps.insert(Dep::Member(source.to_string()));
                } else if self.is_cube(a) {
                    deps.insert(Dep::Cube(a.to_string()));
                }
            }
            [] => {}
        }
    }

    /// `Member(member)` when it is a cube member (or a view member, resolved
    /// to its source); nothing otherwise.
    fn member_if_known(&self, member: &str, deps: &mut BTreeSet<Dep>) {
        if let Some(source) = self.source_member(member) {
            deps.insert(Dep::Member(source.to_string()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(model: &str) -> MemberGraph {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("model.yml"), model).unwrap();
        let model = cubemodel::ModelLoader::with_context(cubemodel::TemplateContext::default())
            .load_dir_with(dir.path())
            .unwrap();
        MemberGraph::build(&model)
    }

    fn member(name: &str) -> Dep {
        Dep::Member(name.to_string())
    }

    fn cube(name: &str) -> Dep {
        Dep::Cube(name.to_string())
    }

    const CUSTOMERS: &str = r#"
  - name: customers
    sql_table: customers
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
      - name: segment
        sql: segment
        type: string
      - name: cpf
        sql: cpf
        type: string
"#;

    fn with_customers(cubes: &str) -> String {
        format!("cubes:{CUSTOMERS}{cubes}")
    }

    #[test]
    fn a_member_depends_on_itself() {
        let graph = graph(&with_customers(
            r#"
  - name: orders
    sql_table: orders
    dimensions:
      - name: status
        sql: status
        type: string
"#,
        ));
        assert_eq!(
            graph.closure("orders.status"),
            BTreeSet::from([member("orders.status")])
        );
        assert_eq!(
            graph.closure("orders.status.day"),
            BTreeSet::from([member("orders.status")])
        );
    }

    #[test]
    fn sql_references_are_followed() {
        let graph = graph(&with_customers(
            r#"
  - name: orders
    sql_table: orders
    measures:
      - name: margin
        sql: margin
        type: sum
      - name: margin_pct
        sql: "{margin} / {CUBE}.revenue"
        type: number
"#,
        ));
        assert_eq!(
            graph.closure("orders.margin_pct"),
            BTreeSet::from([member("orders.margin_pct"), member("orders.margin")])
        );
    }

    #[test]
    fn closure_is_transitive() {
        let graph = graph(&with_customers(
            r#"
  - name: orders
    sql_table: orders
    measures:
      - name: margin
        sql: margin
        type: sum
      - name: margin_ratio
        sql: "{margin} / 100"
        type: number
      - name: margin_pct
        sql: "{margin_ratio} * 100"
        type: number
"#,
        ));
        assert_eq!(
            graph.closure("orders.margin_pct"),
            BTreeSet::from([
                member("orders.margin_pct"),
                member("orders.margin_ratio"),
                member("orders.margin"),
            ])
        );
    }

    #[test]
    fn references_to_other_cubes() {
        let graph = graph(&with_customers(
            r#"
  - name: orders
    sql_table: orders
    dimensions:
      - name: customer_segment
        sql: "{customers.segment}"
        type: string
      - name: customer_name
        sql: "{customers}.name"
        type: string
"#,
        ));
        assert_eq!(
            graph.closure("orders.customer_segment"),
            BTreeSet::from([
                member("orders.customer_segment"),
                member("customers.segment")
            ])
        );
        assert_eq!(
            graph.closure("orders.customer_name"),
            BTreeSet::from([member("orders.customer_name"), cube("customers")])
        );
    }

    #[test]
    fn measure_filters_are_followed() {
        let graph = graph(&with_customers(
            r#"
  - name: orders
    sql_table: orders
    measures:
      - name: identified
        type: count
        filters:
          - sql: "{CUBE}.status = 'x' AND {customers.cpf} IS NOT NULL"
"#,
        ));
        assert!(graph
            .closure("orders.identified")
            .contains(&member("customers.cpf")));
    }

    #[test]
    fn cube_sql_applies_to_every_member() {
        let graph = graph(&with_customers(
            r#"
  - name: orders
    sql: "SELECT * FROM t JOIN {customers.sql()} c"
    dimensions:
      - name: status
        sql: status
        type: string
    measures:
      - name: count
        type: count
    segments:
      - name: open
        sql: "{CUBE}.status = 'open'"
"#,
        ));
        let members = graph.members_of_cube("orders");
        assert_eq!(
            members,
            vec!["orders.status", "orders.count", "orders.open"]
        );
        for name in members {
            assert!(graph.closure(name).contains(&cube("customers")), "{name}");
        }
    }

    #[test]
    fn view_members_resolve_to_their_cube_member() {
        let graph = graph(&format!(
            "{}{}",
            with_customers(
                r#"
  - name: orders
    sql_table: orders
    dimensions:
      - name: status
        sql: status
        type: string
"#,
            ),
            r#"
views:
  - name: orders_view
    cubes:
      - join_path: orders
        includes: [status]
"#
        ));
        assert!(graph.is_view("orders_view"));
        assert!(!graph.is_view("orders"));
        assert_eq!(
            graph.closure("orders_view.status"),
            BTreeSet::from([member("orders.status")])
        );
        assert_eq!(
            graph.source_member("orders_view.status"),
            Some("orders.status")
        );
        assert_eq!(graph.source_member("orders.status"), Some("orders.status"));
        assert_eq!(graph.source_member("orders.nope"), None);
        assert_eq!(
            graph.view_member_for("orders_view", "orders.status"),
            Some("orders_view.status")
        );
        assert_eq!(graph.view_member_for("orders_view", "customers.cpf"), None);
        assert!(graph.view_includes_cube("orders_view", "orders"));
        assert!(!graph.view_includes_cube("orders_view", "customers"));
    }

    #[test]
    fn unknown_names_depend_on_their_whole_cube() {
        let graph = graph(&with_customers(
            r#"
  - name: orders
    sql_table: orders
    dimensions:
      - name: status
        sql: status
        type: string
"#,
        ));
        assert_eq!(
            graph.closure("orders.nope"),
            BTreeSet::from([cube("orders")])
        );
        assert_eq!(graph.closure("orders"), BTreeSet::from([cube("orders")]));
        assert_eq!(
            graph.closure("nothing.x"),
            BTreeSet::from([cube("nothing")])
        );
    }

    #[test]
    fn cycles_terminate() {
        let graph = graph(&with_customers(
            r#"
  - name: orders
    sql_table: orders
    dimensions:
      - name: a
        sql: "{b}"
        type: string
      - name: b
        sql: "{a}"
        type: string
"#,
        ));
        let both = BTreeSet::from([member("orders.a"), member("orders.b")]);
        assert_eq!(graph.closure("orders.a"), both);
        assert_eq!(graph.closure("orders.b"), both);
    }

    #[test]
    fn unresolvable_references_depend_on_the_whole_cube() {
        let graph = graph(&with_customers(
            r#"
  - name: orders
    sql_table: orders
    dimensions:
      - name: open_brace
        sql: "{customers.segment"
        type: string
      - name: odd
        sql: "{whatever()}"
        type: string
      - name: params
        sql: "{FILTER_PARAMS.customers.cpf.filter('x')} AND {SECURITY_CONTEXT.region}"
        type: string
"#,
        ));
        assert!(graph.closure("orders.open_brace").contains(&cube("orders")));
        assert!(graph.closure("orders.odd").contains(&cube("orders")));
        assert_eq!(
            graph.closure("orders.params"),
            BTreeSet::from([member("orders.params"), member("customers.cpf")])
        );
    }
}
