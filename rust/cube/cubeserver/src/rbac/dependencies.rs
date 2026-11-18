//! What each member of the model reads: the cube members it is built on,
//! directly or through other members, views and cube SQL.
//!
//! Used to refuse queries that touch a blocked member through a member built
//! on it, and to decide which cubes a query reads for row filters. When a
//! reference cannot be resolved the graph depends on more (the whole cube),
//! never on less. It also knows the reads an injected row filter cannot
//! reach ([`OpaqueRead`]) and the join graph, for the cubes a query is
//! joined through.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use serde_json::Value;

/// Something a member reads.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Dep {
    /// `cube.member` of a cube, never of a view.
    Member(String),
    /// The whole cube.
    Cube(String),
    /// A cube a view member is read through (an intermediate cube of its
    /// `join_path`): it counts as read for row filters, not for blocking.
    Joined(String),
}

/// A cube read where a row filter injected into the outer query does not
/// reach: inside a `sub_query` dimension, a `{X.sql()}` (`{X.__sql_fn}`)
/// subselect or a bare `{X}` in a cube's own `sql`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct OpaqueRead {
    /// The cube read.
    pub cube: String,
    /// The member (or, for a cube's own `sql`, the cube) the read is inside.
    pub inside: String,
    /// Inside a `sub_query` dimension with `propagate_filters_to_sub_query`:
    /// the outer dimension filters are planned into it too.
    pub propagates: bool,
}

/// Where a reference is written: what an embedded cube reads differs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    /// A member's SQL: a bare `{X}` is the joined cube's alias.
    Member,
    /// A cube's own `sql`: a bare `{X}` is not joined, so it is read raw.
    CubeSql,
}

/// What a reference stands for, past what it adds to the dependencies.
enum Resolved {
    /// A cube or view name (`{CUBE}`, `{TABLE}`, `{customers}`): the
    /// caller decides, as a `.column` may follow the closing brace.
    Whole { cube: String, own: bool },
    /// `{X.sql()}` / `{X.__sql_fn}`: the cube's SQL as a subselect.
    Table(String),
    /// A member, parameters, or something placed on the whole cube.
    Done,
}

/// What one member reads: all of it, and the part read opaquely.
#[derive(Default)]
struct Refs {
    deps: BTreeSet<Dep>,
    opaque: BTreeSet<Dep>,
}

impl Dep {
    /// The cube this dependency belongs to.
    pub fn cube(&self) -> &str {
        match self {
            Dep::Member(member) => member.split('.').next().unwrap_or(member),
            Dep::Cube(cube) | Dep::Joined(cube) => cube,
        }
    }
}

/// Member dependencies of one data model.
#[derive(Debug, Default)]
pub struct MemberGraph {
    /// Cube (not view) name → its dimensions, measures and segments as
    /// `cube.member`, in declaration order.
    cube_members: BTreeMap<String, Vec<String>>,
    /// Time dimensions (`cube.member`), the only members that may carry a
    /// granularity.
    time_dimensions: HashSet<String>,
    /// Every dimension (`cube.member`), time dimensions included.
    dimensions: HashSet<String>,
    /// `sub_query` dimensions (`cube.member`).
    sub_queries: HashSet<String>,
    /// `cube.member` → what it reads where an outer row filter does not
    /// reach (a subset of `direct`).
    opaque: HashMap<String, BTreeSet<Dep>>,
    /// `sub_query` dimension with `propagate_filters_to_sub_query` → what
    /// its subquery reads.
    propagating: HashMap<String, BTreeSet<Dep>>,
    /// Cube → what its own `sql` reads opaquely.
    cube_opaque: HashMap<String, BTreeSet<Dep>>,
    /// Cube → the cubes it joins or is joined from (both ways).
    joins: BTreeMap<String, BTreeSet<String>>,
    /// Cube → the cubes it joins (from→to, the way the planner follows).
    joins_to: BTreeMap<String, BTreeSet<String>>,
    /// Cube → the cubes that join it (to→from).
    joined_from: BTreeMap<String, BTreeSet<String>>,
    /// View names.
    views: BTreeSet<String>,
    /// `cube.member` → what it references directly (itself excluded).
    direct: HashMap<String, BTreeSet<Dep>>,
    /// `view.member` → `cube.member` it was included from.
    view_sources: HashMap<String, String>,
    /// `view.member` → the other cubes of its `join_path`.
    view_joined: HashMap<String, BTreeSet<String>>,
    /// View → (`cube.member` → first `view.member` exposing it).
    view_members: HashMap<String, HashMap<String, String>>,
    /// View → what a read of the whole view reads: `Cube` for each cube it
    /// includes, `Joined` for the other cubes of its join paths.
    view_cubes: HashMap<String, BTreeSet<Dep>>,
}

/// Member fields whose value is a bare member path (no braces), in the
/// typed (camelCase) and the verbatim YAML (snake_case) spelling.
const BARE_REFERENCE_FIELDS: [&str; 12] = [
    "aliasMember",
    "alias_member",
    "keyReference",
    "key_reference",
    "reduceBy",
    "reduce_by",
    "groupBy",
    "group_by",
    "addGroupBy",
    "add_group_by",
    "timeDimension",
    "time_dimension",
];

/// The cubes of a `join_path`: every part but the last is read through.
fn join_path_cubes(join_path: &str) -> (Vec<&str>, &str) {
    let parts: Vec<&str> = join_path
        .split('.')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    match parts.split_last() {
        Some((last, through)) => (through.to_vec(), last),
        None => (Vec::new(), ""),
    }
}

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
            for (name, dimension) in &cube.dimensions {
                let full = format!("{}.{name}", cube.name);
                if dimension.member_type.as_deref() == Some("time") {
                    graph.time_dimensions.insert(full.clone());
                }
                if dimension.sub_query == Some(true) {
                    graph.sub_queries.insert(full.clone());
                }
                graph.dimensions.insert(full);
            }
            for join in &cube.joins {
                graph
                    .joins_to
                    .entry(cube.name.clone())
                    .or_default()
                    .insert(join.name.clone());
                graph
                    .joined_from
                    .entry(join.name.clone())
                    .or_default()
                    .insert(cube.name.clone());
                graph
                    .joins
                    .entry(cube.name.clone())
                    .or_default()
                    .insert(join.name.clone());
                graph
                    .joins
                    .entry(join.name.clone())
                    .or_default()
                    .insert(cube.name.clone());
            }
            graph.cube_members.insert(cube.name.clone(), members);
        }

        for view in model.cube_list().into_iter().filter(|cube| cube.is_view) {
            // Source cube → the cubes it is read through, over every
            // include that reaches it (the member's own include is not
            // recorded, so this may name more, never fewer).
            let mut through: HashMap<&str, BTreeSet<String>> = HashMap::new();
            let mut whole = BTreeSet::new();
            for include in &view.cubes {
                let (via, source) = join_path_cubes(include.join_path.as_deref().unwrap_or(""));
                if source.is_empty() {
                    continue;
                }
                whole.insert(Dep::Cube(source.to_string()));
                let entry = through.entry(source).or_default();
                for cube in via.into_iter().filter(|cube| *cube != source) {
                    entry.insert(cube.to_string());
                }
            }

            let index = graph.view_members.entry(view.name.clone()).or_default();
            for included in &view.included_members {
                let view_member = format!("{}.{}", view.name, included.name);
                let source_cube = included.member_path.split('.').next().unwrap_or_default();
                whole.insert(Dep::Cube(source_cube.to_string()));
                index
                    .entry(included.member_path.clone())
                    .or_insert_with(|| view_member.clone());
                let joined = through.get(source_cube).cloned().unwrap_or_default();
                graph.view_joined.insert(view_member.clone(), joined);
                graph
                    .view_sources
                    .insert(view_member, included.member_path.clone());
            }

            for via in through.values().flatten() {
                if !whole.contains(&Dep::Cube(via.clone())) {
                    whole.insert(Dep::Joined(via.clone()));
                }
            }
            graph.view_cubes.insert(view.name.clone(), whole);
        }

        for cube in model.cube_list().into_iter().filter(|cube| !cube.is_view) {
            let mut shared = Refs::default();
            for sql in cube.sql.iter().chain(cube.sql_table.iter()) {
                graph.references_in_str(&cube.name, sql, Place::CubeSql, &mut shared);
            }
            if !shared.opaque.is_empty() {
                graph
                    .cube_opaque
                    .insert(cube.name.clone(), std::mem::take(&mut shared.opaque));
            }
            let dimensions = &cube.dimensions;

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
                let mut refs = Refs::default();
                match value {
                    Ok(value) => graph.references_in_value(&cube.name, &value, &mut refs),
                    // Cannot see what it reads: it may read anything of its cube.
                    Err(_) => {
                        refs.deps.insert(Dep::Cube(cube.name.clone()));
                    }
                }
                refs.deps.remove(&Dep::Member(full.clone()));
                refs.opaque.remove(&Dep::Member(full.clone()));
                // A `sub_query` dimension is planned as a subquery of what
                // its SQL reads, joined back by the primary key of its cube.
                if let Some(dimension) = dimensions.get(name.as_str()) {
                    if dimension.sub_query == Some(true) {
                        let inside = refs.deps.clone();
                        if dimension.propagate_filters_to_sub_query == Some(true) {
                            graph.propagating.insert(full.clone(), inside);
                        } else {
                            refs.opaque.extend(inside);
                        }
                    }
                }
                if !refs.opaque.is_empty() {
                    graph.opaque.insert(full.clone(), refs.opaque);
                }
                let mut deps = shared.deps.clone();
                deps.extend(refs.deps);
                deps.remove(&Dep::Member(full.clone()));
                graph.direct.insert(full, deps);
            }
        }

        graph
    }

    /// Transitive closure of what `name` reads: `cube.member`,
    /// `view.member`, `cube.member.granularity` or just `cube`.
    /// Includes the member itself (resolved to the cube's). An unknown name
    /// gives `{Cube(first part)}`; for a view, the cubes it includes.
    pub fn closure(&self, name: &str) -> BTreeSet<Dep> {
        let mut parts = name.split('.');
        let first = parts.next().unwrap_or_default();
        let mut seeds = BTreeSet::new();
        match parts.next() {
            Some(second) => {
                let member = format!("{first}.{second}");
                if !self.emit_member(&member, &mut seeds) {
                    self.emit_whole(first, &mut seeds);
                }
            }
            None => {
                self.emit_whole(first, &mut seeds);
                // A bare view also reads what each of its members reads.
                for source in self
                    .view_members
                    .get(first)
                    .into_iter()
                    .flat_map(|m| m.keys())
                {
                    if self.direct.contains_key(source) {
                        seeds.insert(Dep::Member(source.clone()));
                    }
                }
            }
        }

        self.expand(seeds)
    }

    /// `seeds` and everything their members read, transitively.
    fn expand(&self, seeds: BTreeSet<Dep>) -> BTreeSet<Dep> {
        let mut visited = BTreeSet::new();
        let mut queue = VecDeque::new();
        for dep in seeds {
            if visited.insert(dep.clone()) {
                queue.push_back(dep);
            }
        }
        while let Some(dep) = queue.pop_front() {
            let Dep::Member(member) = dep else { continue };
            for next in self.direct.get(&member).into_iter().flatten() {
                if visited.insert(next.clone()) {
                    queue.push_back(next.clone());
                }
            }
        }
        visited
    }

    /// The cubes `names` read where a row filter injected into the outer
    /// query does not reach, with the member each read is inside. A read
    /// inside an opaque read is opaque too. The cube of a `sub_query`
    /// dimension is not listed for it: its subquery is joined back by that
    /// cube's primary key, so the outer filter holds.
    pub fn opaque_reads(&self, names: &[String]) -> BTreeSet<OpaqueRead> {
        // (what is read, inside what, propagates, the cube exempt)
        type Pending = (Dep, String, bool, Option<String>);
        let mut pending: Vec<Pending> = Vec::new();
        let edges = |dep: &Dep, inside: Option<(&str, bool)>, pending: &mut Vec<Pending>| {
            if let Dep::Member(member) = dep {
                let (at, carried) = inside.unwrap_or((member, true));
                let own = member.split('.').next().unwrap_or(member).to_string();
                for next in self.opaque.get(member).into_iter().flatten() {
                    let exempt = self.sub_queries.contains(member).then(|| own.clone());
                    pending.push((next.clone(), at.to_string(), false, exempt));
                }
                for next in self.propagating.get(member).into_iter().flatten() {
                    pending.push((next.clone(), at.to_string(), carried, Some(own.clone())));
                }
            }
            let cube = dep.cube();
            for next in self.cube_opaque.get(cube).into_iter().flatten() {
                let at = inside.map_or(cube, |(at, _)| at);
                pending.push((next.clone(), at.to_string(), false, None));
            }
        };

        let outer: BTreeSet<Dep> = names.iter().flat_map(|name| self.closure(name)).collect();
        for dep in &outer {
            edges(dep, None, &mut pending);
        }
        let mut reads = BTreeSet::new();
        let mut seen = HashSet::new();
        while let Some((dep, inside, propagates, exempt)) = pending.pop() {
            if !seen.insert((dep.clone(), inside.clone(), propagates, exempt.clone())) {
                continue;
            }
            for reached in self.expand(BTreeSet::from([dep])) {
                let cube = reached.cube().to_string();
                if exempt.as_deref() != Some(cube.as_str()) {
                    reads.insert(OpaqueRead {
                        cube,
                        inside: inside.clone(),
                        propagates,
                    });
                }
                edges(&reached, Some((&inside, propagates)), &mut pending);
            }
        }
        // Read opaquely and with propagated filters: the opaque read wins.
        let opaque: BTreeSet<String> = reads
            .iter()
            .filter(|read| !read.propagates)
            .map(|read| read.cube.clone())
            .collect();
        reads.retain(|read| !read.propagates || !opaque.contains(&read.cube));
        reads
    }

    /// Whether `member` is a `sub_query` dimension of a cube.
    pub fn is_sub_query(&self, member: &str) -> bool {
        self.sub_queries.contains(member)
    }

    /// Whether `member` reads a `sub_query` dimension, itself included, at
    /// any depth: the planner does not propagate a filter on it.
    pub fn reaches_sub_query(&self, member: &str) -> bool {
        self.closure(member)
            .iter()
            .any(|dep| matches!(dep, Dep::Member(m) if self.is_sub_query(m)))
    }

    /// Whether `member` (`cube.member`) is a dimension of a cube.
    pub fn is_dimension(&self, member: &str) -> bool {
        self.dimensions.contains(member)
    }

    /// `cubes` and every cube on a shortest join path between two of them.
    /// The planner joins along directed (from→to) shortest paths, rooted at
    /// one of the cubes; which root and which of equal-length paths it
    /// picks is not ours to know, so every directed shortest path between
    /// every ordered pair counts, and every undirected one too.
    pub fn cubes_between(&self, cubes: &BTreeSet<String>) -> BTreeSet<String> {
        let mut between = cubes.clone();
        let mut paths = |forward: &BTreeMap<String, BTreeSet<String>>,
                         backward: &BTreeMap<String, BTreeSet<String>>| {
            let from: Vec<(&str, HashMap<&str, usize>)> = cubes
                .iter()
                .map(|cube| (cube.as_str(), Self::join_distances(forward, cube)))
                .collect();
            let to: HashMap<&str, HashMap<&str, usize>> = cubes
                .iter()
                .map(|cube| (cube.as_str(), Self::join_distances(backward, cube)))
                .collect();
            for (a, from_a) in &from {
                for (b, to_b) in &to {
                    if a == b {
                        continue;
                    }
                    let Some(&length) = from_a.get(b) else {
                        continue;
                    };
                    for (cube, hops) in from_a {
                        if to_b.get(cube).is_some_and(|rest| hops + rest == length) {
                            between.insert(cube.to_string());
                        }
                    }
                }
            }
        };
        paths(&self.joins_to, &self.joined_from);
        paths(&self.joins, &self.joins);
        between
    }

    /// Hops from `from` to every cube it reaches along `edges`.
    fn join_distances<'a>(
        edges: &'a BTreeMap<String, BTreeSet<String>>,
        from: &'a str,
    ) -> HashMap<&'a str, usize> {
        let mut distance = HashMap::from([(from, 0)]);
        let mut queue = VecDeque::from([from]);
        while let Some(cube) = queue.pop_front() {
            let next = distance[cube] + 1;
            for other in edges.get(cube).into_iter().flatten() {
                if !distance.contains_key(other.as_str()) {
                    distance.insert(other, next);
                    queue.push_back(other);
                }
            }
        }
        distance
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

    fn is_cube_or_view(&self, name: &str) -> bool {
        self.cube_members.contains_key(name) || self.views.contains(name)
    }

    /// `Member` of the cube member `name` stands for (a view member's
    /// source, plus `Joined` for its join path). False when unknown.
    fn emit_member(&self, name: &str, deps: &mut BTreeSet<Dep>) -> bool {
        let Some(source) = self.source_member(name) else {
            return false;
        };
        if self.direct.contains_key(source) {
            deps.insert(Dep::Member(source.to_string()));
        } else {
            // A view member whose source is not a cube member (a hierarchy).
            let cube = source.split('.').next().unwrap_or(source);
            deps.insert(Dep::Cube(cube.to_string()));
        }
        for cube in self.view_joined.get(name).into_iter().flatten() {
            deps.insert(Dep::Joined(cube.clone()));
        }
        true
    }

    /// The whole of `name`: `Cube(name)`, or for a view the cubes it
    /// includes (`Dep::Cube` never names a view).
    fn emit_whole(&self, name: &str, deps: &mut BTreeSet<Dep>) {
        match self.view_cubes.get(name) {
            Some(cubes) => deps.extend(cubes.iter().cloned()),
            None if self.is_view(name) => {}
            None => {
                deps.insert(Dep::Cube(name.to_string()));
            }
        }
    }

    /// Every reference in any string of `value`, plus bare member paths in
    /// the fields that hold them.
    fn references_in_value(&self, cube: &str, value: &Value, refs: &mut Refs) {
        match value {
            Value::String(text) => self.references_in_str(cube, text, Place::Member, refs),
            Value::Array(items) => {
                for item in items {
                    self.references_in_value(cube, item, refs);
                }
            }
            Value::Object(fields) => {
                for (key, field) in fields {
                    if BARE_REFERENCE_FIELDS.contains(&key.as_str()) {
                        self.bare_references(cube, field, refs);
                    }
                    self.references_in_value(cube, field, refs);
                }
            }
            _ => {}
        }
    }

    fn bare_references(&self, cube: &str, value: &Value, refs: &mut Refs) {
        match value {
            Value::String(path) => {
                let resolved = self.resolve(cube, path, &mut refs.deps);
                self.place(resolved, None, Place::Member, refs);
            }
            Value::Array(items) => {
                for item in items {
                    self.bare_references(cube, item, refs);
                }
            }
            _ => {}
        }
    }

    /// Adds what a [`Resolved`] reference reads. `column` is the identifier
    /// after `}.`, if any: after a cube it names that cube's member, as in
    /// `{CUBE}.margin`.
    fn place(&self, resolved: Resolved, column: Option<&str>, place: Place, refs: &mut Refs) {
        match resolved {
            Resolved::Done => {}
            Resolved::Table(cube) => {
                let mut whole = BTreeSet::new();
                self.emit_whole(&cube, &mut whole);
                refs.deps.extend(whole.iter().cloned());
                refs.opaque.extend(whole);
            }
            Resolved::Whole { cube, own } => {
                let mut read = BTreeSet::new();
                let member = column
                    .is_some_and(|column| self.emit_member(&format!("{cube}.{column}"), &mut read));
                if !member && !own {
                    self.emit_whole(&cube, &mut read);
                }
                if place == Place::CubeSql && !own {
                    // Not joined in: the cube is read without the outer query.
                    self.emit_whole(&cube, &mut refs.opaque);
                }
                refs.deps.extend(read);
            }
        }
    }

    /// Each `{` starts a reference, which runs to the next `{` or `}`: in
    /// `{FILTER_PARAMS.a.b.filter((x) => `${x}`)}` both the outer text and
    /// `x` are resolved. A `{` with no `}` after it may hide anything of
    /// the cube.
    fn references_in_str(&self, cube: &str, text: &str, place: Place, refs: &mut Refs) {
        for (start, _) in text.match_indices('{') {
            let rest = &text[start + 1..];
            if !rest.contains('}') {
                refs.deps.insert(Dep::Cube(cube.to_string()));
                continue;
            }
            let end = rest.find(['{', '}']).unwrap_or(rest.len());
            let resolved = self.resolve(cube, &rest[..end], &mut refs.deps);
            // `}.identifier` right after a closing brace.
            let column = rest[end..]
                .strip_prefix('}')
                .and_then(|after| after.strip_prefix('.'))
                .map(|after| {
                    let len = after
                        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                        .unwrap_or(after.len());
                    &after[..len]
                })
                .filter(|column| !column.is_empty());
            self.place(resolved, column, place, refs);
        }
    }

    /// Resolves one reference written inside `cube`, as the planner's
    /// `SymbolPath::resolve_parts` walks it. A path that cannot be placed
    /// reads the whole cube it was walking. A path that ends on a cube is
    /// left to the caller.
    fn resolve(&self, cube: &str, reference: &str, deps: &mut BTreeSet<Dep>) -> Resolved {
        let reference = reference.trim();
        let head = reference
            .split(['.', '('])
            .next()
            .unwrap_or_default()
            .trim();
        match head {
            "FILTER_PARAMS" | "FILTER_GROUP" => {
                if !self.filter_params(reference, deps) {
                    deps.insert(Dep::Cube(cube.to_string()));
                }
                return Resolved::Done;
            }
            // Values of the request, not data of the model.
            "SECURITY_CONTEXT" | "COMPILE_CONTEXT" => return Resolved::Done,
            _ => {}
        }

        let parts: Vec<&str> = reference.split('.').map(str::trim).collect();
        let mut current = cube.to_string();
        // The cubes the path walks onto: the planner joins them as one
        // `Vector` hint (`{b.c.name}` joins b, not only c).
        let mut walked: Vec<String> = Vec::new();
        // Still on the cube the reference is written in, by CUBE or TABLE.
        let mut own = true;
        let mut index = 0;
        while index < parts.len() {
            let part = parts[index];
            let rest = parts.len() - index - 1;
            let member = format!("{current}.{part}");
            // As `try_resolve_as_member`: a member takes no further part,
            // except a time dimension, which takes one granularity. When it
            // cannot take the rest, the part is tried as a cube name.
            let takes_rest = self.source_member(&member).is_some_and(|source| {
                rest == 0 || (rest == 1 && self.time_dimensions.contains(source))
            });
            if takes_rest {
                self.emit_member(&member, deps);
                joined_through(&walked, cube, &current, deps);
                return Resolved::Done;
            }
            if part == "CUBE" || part == "TABLE" {
                index += 1;
                continue;
            }
            if self.is_cube_or_view(part) {
                current = part.to_string();
                if !self.is_view(part) {
                    walked.push(current.clone());
                }
                own = false;
                index += 1;
                continue;
            }
            // The cube's SQL as a subselect, as the planner's `__sql_fn`.
            if rest == 0 && (part == "__sql_fn" || part.starts_with("sql(")) {
                joined_through(&walked, cube, &current, deps);
                return Resolved::Table(current);
            }
            break;
        }
        joined_through(&walked, cube, &current, deps);
        if index == parts.len() {
            // Ran out on a cube name (`{CUBE}`, `{customers}`,
            // `{customers.CUBE}`).
            return Resolved::Whole { cube: current, own };
        }
        // Hit something it cannot place (`{CUBE.col}`).
        self.emit_whole(&current, deps);
        Resolved::Done
    }

    /// `Member(a.b)` for every `FILTER_PARAMS.a.b` in `reference`. False
    /// when one names no member, or there is none.
    fn filter_params(&self, reference: &str, deps: &mut BTreeSet<Dep>) -> bool {
        let identifier = |text: &str| -> String {
            text.trim_start()
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect()
        };
        let mut found = false;
        for (start, marker) in reference.match_indices("FILTER_PARAMS.") {
            let after = &reference[start + marker.len()..];
            let cube = identifier(after);
            let member = after
                .trim_start()
                .get(cube.len()..)
                .and_then(|rest| rest.strip_prefix('.'))
                .map(identifier)
                .unwrap_or_default();
            if cube.is_empty() || member.is_empty() {
                return false;
            }
            if !self.emit_member(&format!("{cube}.{member}"), deps) {
                return false;
            }
            found = true;
        }
        found
    }
}

/// `Joined` for each cube a dotted path walked through, other than the
/// cube it is written in and the one it ends on.
fn joined_through(walked: &[String], from: &str, to: &str, deps: &mut BTreeSet<Dep>) {
    for cube in walked {
        if cube != from && cube != to {
            deps.insert(Dep::Joined(cube.clone()));
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
      - name: created_at
        sql: created_at
        type: time
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
    fn unknown_names_of_a_view_depend_on_every_cube_it_includes() {
        let graph = graph(&format!("{}{}", with_customers(ORDERS), ORDERS_VIEW));
        for name in ["orders_view", "orders_view.nope"] {
            let deps = graph.closure(name);
            assert!(deps.contains(&cube("orders")), "{name}: {deps:?}");
            assert!(deps.contains(&cube("customers")), "{name}: {deps:?}");
            assert!(
                deps.iter().all(|dep| dep.cube() != "orders_view"),
                "{name}: {deps:?}"
            );
        }
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

    const ORDERS: &str = r#"
  - name: orders
    sql_table: orders
    joins:
      - name: customers
        sql: "{CUBE}.customer_id = {customers}.id"
        relationship: many_to_one
    dimensions:
      - name: status
        sql: status
        type: string
"#;

    const ORDERS_VIEW: &str = r#"
views:
  - name: orders_view
    cubes:
      - join_path: orders
        includes: [status]
      - join_path: orders.customers
        includes: [segment]
"#;

    #[test]
    fn view_members_depend_on_their_join_path() {
        let graph = graph(&format!("{}{}", with_customers(ORDERS), ORDERS_VIEW));
        assert_eq!(
            graph.closure("orders_view.segment"),
            BTreeSet::from([
                member("customers.segment"),
                Dep::Joined("orders".to_string())
            ])
        );
        assert_eq!(
            graph.closure("orders_view.status"),
            BTreeSet::from([member("orders.status")])
        );
        assert_eq!(Dep::Joined("orders".to_string()).cube(), "orders");
    }

    #[test]
    fn paths_resolve_like_the_planner() {
        let graph = graph(&with_customers(
            r#"
  - name: orders
    sql_table: orders
    dimensions:
      - name: status
        sql: status
        type: string
      - name: created_at
        sql: created_at
        type: time
      - name: via_cube
        sql: "{CUBE.customers.cpf}"
        type: string
      - name: via_name
        sql: "{orders.customers.cpf}"
        type: string
      - name: via_table
        sql: "{TABLE.status}"
        type: string
      - name: granularity
        sql: "{created_at.day}"
        type: time
      - name: column
        sql: "{CUBE}.col"
        type: string
      - name: unplaced
        sql: "{CUBE.col}"
        type: string
"#,
        ));
        let has = |name: &str, dep: Dep| {
            let deps = graph.closure(name);
            assert!(deps.contains(&dep), "{name}: {deps:?}");
        };
        has("orders.via_cube", member("customers.cpf"));
        has("orders.via_name", member("customers.cpf"));
        has("orders.via_table", member("orders.status"));
        has("orders.granularity", member("orders.created_at"));
        has("orders.unplaced", cube("orders"));
        assert_eq!(
            graph.closure("orders.column"),
            BTreeSet::from([member("orders.column")])
        );
    }

    #[test]
    fn a_dotted_reference_joins_the_cubes_on_its_path() {
        // The planner turns `{b.c.name}` into the join hint [b, c], so it
        // joins b; `{CUBE.b.c}` (a cube name) does the same.
        let graph = graph(
            r#"
cubes:
  - name: a
    sql_table: a
    dimensions:
      - name: label
        sql: "{b.c.name}"
        type: string
      - name: tag
        sql: "{CUBE.b.c}.name"
        type: string
  - name: b
    sql_table: b
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
  - name: c
    sql_table: c
    dimensions:
      - name: name
        sql: name
        type: string
"#,
        );
        assert_eq!(
            graph.closure("a.label"),
            BTreeSet::from([
                member("a.label"),
                member("c.name"),
                Dep::Joined("b".to_string())
            ])
        );
        let tag = graph.closure("a.tag");
        assert!(tag.contains(&Dep::Joined("b".to_string())), "{tag:?}");
    }

    #[test]
    fn filter_group_params_are_followed() {
        let graph = graph(&with_customers(
            r#"
  - name: orders
    sql_table: orders
    dimensions:
      - name: grouped
        sql: "{FILTER_GROUP(FILTER_PARAMS.customers.cpf.filter('x'), FILTER_PARAMS.customers.segment.filter('y'))}"
        type: string
      - name: opaque
        sql: "{FILTER_GROUP(something)}"
        type: string
"#,
        ));
        let grouped = graph.closure("orders.grouped");
        assert!(grouped.contains(&member("customers.cpf")), "{grouped:?}");
        assert!(
            grouped.contains(&member("customers.segment")),
            "{grouped:?}"
        );
        assert!(graph.closure("orders.opaque").contains(&cube("orders")));
    }

    #[test]
    fn nested_braces_resolve_the_outer_reference() {
        let graph = graph(&with_customers(
            r#"
  - name: orders
    sql_table: orders
    dimensions:
      - name: ranged
        sql: "{FILTER_PARAMS.customers.created_at.filter((a, b) => `${a} AND ${b}`)}"
        type: string
"#,
        ));
        let deps = graph.closure("orders.ranged");
        assert!(deps.contains(&member("customers.created_at")), "{deps:?}");
    }

    #[test]
    fn time_shift_time_dimensions_are_followed() {
        let graph = graph(&with_customers(
            r#"
  - name: orders
    sql_table: orders
    dimensions:
      - name: created_at
        sql: created_at
        type: time
    measures:
      - name: count
        type: count
      - name: count_prior_year
        multi_stage: true
        sql: "{count}"
        type: number
        time_shift:
          - time_dimension: created_at
            interval: 1 year
            type: prior
"#,
        ));
        let deps = graph.closure("orders.count_prior_year");
        assert!(deps.contains(&member("orders.created_at")), "{deps:?}");
    }

    #[test]
    fn a_member_that_cannot_take_the_rest_falls_back_to_the_cube_name() {
        let graph = graph(&with_customers(
            r#"
  - name: orders
    sql_table: orders
    dimensions:
      - name: customers
        sql: customer_name
        type: string
      - name: created_at
        sql: created_at
        type: time
      - name: via_dimension
        sql: "{customers.cpf}"
        type: string
      - name: day
        sql: "{created_at.day}"
        type: time
      - name: too_long
        sql: "{created_at.day.x}"
        type: time
    measures:
      - name: customers_count
        type: count
"#,
        ));
        assert_eq!(
            graph.closure("orders.via_dimension"),
            BTreeSet::from([member("orders.via_dimension"), member("customers.cpf")])
        );
        assert_eq!(
            graph.closure("orders.day"),
            BTreeSet::from([member("orders.day"), member("orders.created_at")])
        );
        // `created_at` is not a cube: nothing places the path, so the cube.
        assert_eq!(
            graph.closure("orders.too_long"),
            BTreeSet::from([member("orders.too_long"), cube("orders")])
        );
    }

    #[test]
    fn a_measure_named_like_a_cube_falls_back_to_the_cube() {
        let graph = graph(&with_customers(
            r#"
  - name: orders
    sql_table: orders
    measures:
      - name: customers
        type: count
      - name: via_measure
        sql: "{customers.segment}"
        type: number
"#,
        ));
        assert_eq!(
            graph.closure("orders.via_measure"),
            BTreeSet::from([member("orders.via_measure"), member("customers.segment")])
        );
    }

    #[test]
    fn a_time_dimension_named_like_a_cube_falls_through_with_two_extra_parts() {
        let graph = graph(&with_customers(
            r#"
  - name: orders
    sql_table: orders
    dimensions:
      - name: customers
        sql: customer_at
        type: time
      - name: day
        sql: "{customers.day}"
        type: time
      - name: through
        sql: "{customers.created_at.day}"
        type: time
"#,
        ));
        assert_eq!(
            graph.closure("orders.day"),
            BTreeSet::from([member("orders.day"), member("orders.customers")])
        );
        assert_eq!(
            graph.closure("orders.through"),
            BTreeSet::from([member("orders.through"), member("customers.created_at")])
        );
    }

    #[test]
    fn a_column_after_a_cube_reference_is_its_member() {
        let graph = graph(&with_customers(
            r#"
  - name: orders
    sql_table: orders
    dimensions:
      - name: margin
        sql: margin
        type: number
      - name: double_margin
        sql: "{CUBE}.margin * 2"
        type: number
      - name: by_table
        sql: "{TABLE}.margin"
        type: number
      - name: by_name
        sql: "{customers}.segment"
        type: string
      - name: raw
        sql: "{CUBE}.not_a_member"
        type: number
"#,
        ));
        assert_eq!(
            graph.closure("orders.double_margin"),
            BTreeSet::from([member("orders.double_margin"), member("orders.margin")])
        );
        assert_eq!(
            graph.closure("orders.by_table"),
            BTreeSet::from([member("orders.by_table"), member("orders.margin")])
        );
        assert_eq!(
            graph.closure("orders.by_name"),
            BTreeSet::from([member("orders.by_name"), member("customers.segment")])
        );
        assert_eq!(
            graph.closure("orders.raw"),
            BTreeSet::from([member("orders.raw")])
        );
    }

    fn read(cube: &str, inside: &str, propagates: bool) -> OpaqueRead {
        OpaqueRead {
            cube: cube.to_string(),
            inside: inside.to_string(),
            propagates,
        }
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| name.to_string()).collect()
    }

    const SUB_QUERY_MODEL: &str = r#"
  - name: orders
    sql_table: orders
    joins:
      - name: customers
        sql: "{CUBE}.customer_id = {customers}.id"
        relationship: many_to_one
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
      - name: region
        sql: region
        type: string
      - name: amount
        sql: amount
        type: number
    measures:
      - name: count
        type: count
      - name: total
        sql: "{amount}"
        type: sum
"#;

    #[test]
    fn a_sub_query_dimension_reads_its_cubes_opaquely() {
        let graph = graph(&format!(
            "{}{}",
            with_customers(SUB_QUERY_MODEL),
            r#"
  - name: buyers
    sql_table: customers
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
      - name: order_count
        sql: "{orders.count}"
        type: number
        sub_query: true
      - name: propagated_total
        sql: "{orders.total} + {CUBE}.id"
        type: number
        sub_query: true
        propagate_filters_to_sub_query: true
      - name: plain
        sql: "{orders.region}"
        type: string
"#
        ));
        // The member itself still depends on what it reads.
        assert!(graph
            .closure("buyers.order_count")
            .contains(&member("orders.count")));
        assert_eq!(
            graph.opaque_reads(&names(&["buyers.order_count"])),
            BTreeSet::from([read("orders", "buyers.order_count", false)])
        );
        // With propagated filters, the read is marked as such; the cube of
        // the dimension itself is joined back by its primary key.
        assert_eq!(
            graph.opaque_reads(&names(&["buyers.propagated_total"])),
            BTreeSet::from([read("orders", "buyers.propagated_total", true)])
        );
        assert!(graph.is_sub_query("buyers.order_count"));
        assert!(!graph.is_sub_query("buyers.plain"));
        // A plain reference is not opaque.
        assert!(graph.opaque_reads(&names(&["buyers.plain"])).is_empty());
        // Nor is a member built on a sub_query one: it reads it opaquely.
        assert!(graph.opaque_reads(&names(&["orders.count"])).is_empty());
    }

    #[test]
    fn cube_sql_and_sql_calls_read_opaquely() {
        let graph = graph(&with_customers(
            r#"
  - name: orders
    sql: "SELECT * FROM t JOIN {customers.sql()} c ON c.id = t.customer_id"
    dimensions:
      - name: status
        sql: status
        type: string
  - name: wrapped
    sql: "SELECT * FROM {customers}"
    dimensions:
      - name: status
        sql: status
        type: string
  - name: events
    sql_table: events
    dimensions:
      - name: kind
        sql: kind
        type: string
      - name: biggest
        sql: "(SELECT max(x) FROM {CUBE.__sql_fn})"
        type: number
      - name: other_table
        sql: "(SELECT count(*) FROM {customers.__sql_fn})"
        type: number
      - name: customer_name
        sql: "{customers}.name"
        type: string
"#,
        ));
        assert_eq!(
            graph.opaque_reads(&names(&["orders.status"])),
            BTreeSet::from([read("customers", "orders", false)])
        );
        assert_eq!(
            graph.opaque_reads(&names(&["wrapped.status"])),
            BTreeSet::from([read("customers", "wrapped", false)])
        );
        assert_eq!(
            graph.opaque_reads(&names(&["events.biggest"])),
            BTreeSet::from([read("events", "events.biggest", false)])
        );
        assert_eq!(
            graph.opaque_reads(&names(&["events.other_table"])),
            BTreeSet::from([read("customers", "events.other_table", false)])
        );
        // A bare cube in member SQL is the joined cube's alias: not opaque.
        assert!(graph
            .opaque_reads(&names(&["events.customer_name"]))
            .is_empty());
        assert!(graph.opaque_reads(&names(&["events.kind"])).is_empty());
    }

    #[test]
    fn an_opaque_read_through_a_member_is_found() {
        let graph = graph(&format!(
            "{}{}",
            with_customers(SUB_QUERY_MODEL),
            r#"
  - name: buyers
    sql_table: customers
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
      - name: order_count
        sql: "{orders.count}"
        type: number
        sub_query: true
      - name: doubled
        sql: "{order_count} * 2"
        type: number
views:
  - name: buyers_view
    cubes:
      - join_path: buyers
        includes: [doubled]
"#
        ));
        for name in ["buyers.doubled", "buyers_view.doubled"] {
            assert_eq!(
                graph.opaque_reads(&names(&[name])),
                BTreeSet::from([read("orders", "buyers.order_count", false)]),
                "{name}"
            );
        }
    }

    #[test]
    fn dimensions_are_known() {
        let graph = graph(&with_customers(SUB_QUERY_MODEL));
        assert!(graph.is_dimension("orders.region"));
        assert!(graph.is_dimension("customers.created_at"));
        assert!(!graph.is_dimension("orders.count"));
        assert!(!graph.is_dimension("orders.nope"));
    }

    #[test]
    fn cubes_between_follow_the_shortest_join_paths() {
        let graph = graph(
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
    measures:
      - name: count
        type: count
  - name: c
    sql_table: c
    measures:
      - name: count
        type: count
  - name: d
    sql_table: d
    joins:
      - name: a
        sql: "{CUBE}.a_id = {a}.id"
        relationship: many_to_one
    measures:
      - name: count
        type: count
  - name: alone
    sql_table: alone
    measures:
      - name: count
        type: count
"#,
        );
        let set = |list: &[&str]| -> BTreeSet<String> {
            list.iter().map(|cube| cube.to_string()).collect()
        };
        assert_eq!(
            graph.cubes_between(&set(&["a", "c"])),
            set(&["a", "b", "c"])
        );
        // Joins are followed both ways.
        assert_eq!(
            graph.cubes_between(&set(&["c", "d"])),
            set(&["a", "b", "c", "d"])
        );
        assert_eq!(graph.cubes_between(&set(&["a"])), set(&["a"]));
        // No path, nothing in between.
        assert_eq!(
            graph.cubes_between(&set(&["a", "alone"])),
            set(&["a", "alone"])
        );
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
    fn cubes_between_cover_the_directed_join_paths() {
        let graph = graph(DIRECTED_MODEL);
        let set = |list: &[&str]| -> BTreeSet<String> {
            list.iter().map(|cube| cube.to_string()).collect()
        };
        assert_eq!(
            graph.cubes_between(&set(&["a", "c"])),
            set(&["a", "c", "d", "x", "y"])
        );
        // D→A→X, and X-A-D both ways.
        assert_eq!(
            graph.cubes_between(&set(&["x", "d"])),
            set(&["a", "d", "x"])
        );
    }
}
