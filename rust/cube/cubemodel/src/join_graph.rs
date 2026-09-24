//! The subset of `JoinGraph` the meta response needs: connected components.

use std::collections::HashMap;

use crate::model::DataModel;

/// Mirrors `JoinGraph.connectedComponents()`.
///
/// Only cubes that are the *source* of at least one join edge get a component
/// id — exactly like Node, where `this.nodes` is keyed by `join.from`. Cubes
/// without outgoing joins (and all views) therefore have no
/// `connectedComponent` in the meta response.
pub fn connected_components(model: &DataModel) -> HashMap<String, u32> {
    // Edges, in cubeList order, keyed `from-to` — the order `this.edges` has.
    let mut edges: Vec<(String, String)> = Vec::new();
    for cube in model.cube_list() {
        for join in &cube.joins {
            if join.name.is_empty() || !model.contains(&join.name) {
                continue;
            }
            let key = (cube.name.clone(), join.name.clone());
            if !edges.contains(&key) {
                edges.push(key);
            }
        }
    }

    // `nodes`: grouped by `from`, first-appearance order.
    let mut node_order: Vec<String> = Vec::new();
    for (from, _) in &edges {
        if !node_order.contains(from) {
            node_order.push(from.clone());
        }
    }

    // `undirectedNodes`: symmetric adjacency over both edge directions.
    let mut undirected: HashMap<String, Vec<String>> = HashMap::new();
    for (from, to) in &edges {
        undirected.entry(to.clone()).or_default().push(from.clone());
        undirected.entry(from.clone()).or_default().push(to.clone());
    }

    let mut components: HashMap<String, u32> = HashMap::new();
    for (component_id, node) in (1_u32..).zip(node_order.iter()) {
        visit(node, component_id, &undirected, &mut components);
    }
    components
}

fn visit(
    node: &str,
    component_id: u32,
    undirected: &HashMap<String, Vec<String>>,
    components: &mut HashMap<String, u32>,
) {
    if components.contains_key(node) {
        return;
    }
    components.insert(node.to_string(), component_id);
    if let Some(neighbours) = undirected.get(node) {
        for neighbour in neighbours.clone() {
            visit(&neighbour, component_id, undirected, components);
        }
    }
}
