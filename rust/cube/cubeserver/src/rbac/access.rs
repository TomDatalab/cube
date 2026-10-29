//! A user's access graph: the grants they hold, the permissions those derive
//! (the SpiceDB schema plus the server's own restrictions) and why.
//!
//! Pure: the caller loads the inputs, this only assembles them.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::Serialize;
use uuid::Uuid;

use super::authz::Relationship;
use super::store::{ConnectionOrigin, ConnectionRow, Organization, User, Workspace};

const ORG_OWNER: &[&str] = &["admin", "view", "manage_connections"];
const ORG_MEMBER: &[&str] = &["view"];
const WS_OWNER: &[&str] = &[
    "manage",
    "edit",
    "view",
    "query",
    "execute",
    "read objects",
    "write objects",
];
const WS_EDITOR: &[&str] = &["edit", "view", "query", "read objects", "write objects"];
const WS_VIEWER: &[&str] = &["view", "query", "read objects"];

const ORG_ORDER: &[&str] = &["admin", "view", "manage_connections"];
const WS_ORDER: &[&str] = WS_OWNER;

const NOTE_ASSIGNED: &str = "assigned by a platform admin: only platform admins edit it";
const NOTE_MISSING: &str = "data source missing";

pub struct AccessInput {
    pub user: User,
    pub platform_admin: bool,
    /// Relationships whose subject is the user.
    pub direct: Vec<Relationship>,
    pub organizations: Vec<Organization>,
    pub workspaces: Vec<Workspace>,
    pub connections: Vec<ConnectionRow>,
    /// (connection, workspace) pairs.
    pub releases: Vec<(String, Uuid)>,
    /// Data source names in `cube.yml`.
    pub configured: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GraphUser {
    pub id: String,
    pub name: Option<String>,
    pub disabled: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub organization_id: Option<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub label: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Reason {
    pub text: String,
    pub path: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Grant {
    pub resource: String,
    pub permission: String,
    pub reasons: Vec<Reason>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AccessGraph {
    pub user: GraphUser,
    pub platform_admin: bool,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub grants: Vec<Grant>,
}

/// One way of holding a set of permissions on a resource.
struct Source {
    text: String,
    path: Vec<String>,
    perms: &'static [&'static str],
}

/// Grants for `resource`, one per permission in `order`, each with a reason
/// per source that gives it (no repeated reasons).
fn grants_of(resource: &str, order: &[&str], sources: &[Source]) -> Vec<Grant> {
    order
        .iter()
        .filter_map(|perm| {
            let mut reasons: Vec<Reason> = Vec::new();
            for s in sources.iter().filter(|s| s.perms.contains(perm)) {
                let reason = Reason {
                    text: s.text.clone(),
                    path: s.path.clone(),
                };
                if !reasons.contains(&reason) {
                    reasons.push(reason);
                }
            }
            (!reasons.is_empty()).then(|| Grant {
                resource: resource.to_string(),
                permission: (*perm).to_string(),
                reasons,
            })
        })
        .collect()
}

/// Edges, one per (from, to): parallel ones are merged into a single edge
/// whose label joins theirs with " · ", direct if any of them is.
#[derive(Default)]
struct Edges {
    list: Vec<Edge>,
    index: HashMap<(String, String), usize>,
}

impl Edges {
    fn add(&mut self, from: &str, to: &str, label: &str, kind: &str) {
        let key = (from.to_string(), to.to_string());
        if let Some(&i) = self.index.get(&key) {
            let edge = &mut self.list[i];
            if !edge.label.split(" · ").any(|l| l == label) {
                edge.label = format!("{} · {label}", edge.label);
            }
            if kind == "direct" {
                edge.kind = kind.to_string();
            }
            return;
        }
        self.index.insert(key, self.list.len());
        self.list.push(Edge {
            from: from.to_string(),
            to: to.to_string(),
            label: label.to_string(),
            kind: kind.to_string(),
        });
    }
}

pub fn build(input: &AccessInput) -> AccessGraph {
    let user_id = format!("user:{}", input.user.id);
    let user_node = Node {
        id: user_id.clone(),
        kind: "user".into(),
        name: input
            .user
            .name
            .clone()
            .unwrap_or_else(|| input.user.id.clone()),
        organization_id: None,
        notes: vec![],
    };
    let mut graph = AccessGraph {
        user: GraphUser {
            id: input.user.id.clone(),
            name: input.user.name.clone(),
            disabled: input.user.disabled,
        },
        platform_admin: input.platform_admin,
        nodes: vec![user_node],
        edges: vec![],
        grants: vec![],
    };
    if input.platform_admin {
        return graph;
    }

    let orgs: HashMap<String, &Organization> = input
        .organizations
        .iter()
        .map(|o| (o.id.to_string(), o))
        .collect();
    let workspaces: HashMap<String, &Workspace> = input
        .workspaces
        .iter()
        .map(|w| (w.id.to_string(), w))
        .collect();
    let mut edges = Edges::default();

    // Organizations: direct roles.
    let mut org_sources: BTreeMap<String, Vec<Source>> = BTreeMap::new();
    let mut owned_orgs: Vec<&Organization> = Vec::new();
    for rel in input
        .direct
        .iter()
        .filter(|r| r.resource.kind == "organization")
    {
        let Some(org) = orgs.get(&rel.resource.id) else {
            continue;
        };
        let node = format!("organization:{}", org.id);
        let perms = match rel.relation {
            "owner" => {
                if !owned_orgs.iter().any(|o| o.id == org.id) {
                    owned_orgs.push(org);
                }
                ORG_OWNER
            }
            "member" => ORG_MEMBER,
            _ => continue,
        };
        edges.add(&user_id, &node, rel.relation, "direct");
        org_sources
            .entry(org.id.to_string())
            .or_default()
            .push(Source {
                text: rel.relation.to_string(),
                path: vec![user_id.clone(), node],
                perms,
            });
    }

    // Workspaces: direct roles, then ownership of the organization.
    let mut ws_sources: BTreeMap<String, Vec<Source>> = BTreeMap::new();
    for rel in input
        .direct
        .iter()
        .filter(|r| r.resource.kind == "workspace")
    {
        let Some(ws) = workspaces.get(&rel.resource.id) else {
            continue;
        };
        let perms = match rel.relation {
            "owner" => WS_OWNER,
            "editor" => WS_EDITOR,
            "viewer" => WS_VIEWER,
            _ => continue,
        };
        let node = format!("workspace:{}", ws.id);
        edges.add(&user_id, &node, rel.relation, "direct");
        ws_sources
            .entry(ws.id.to_string())
            .or_default()
            .push(Source {
                text: rel.relation.to_string(),
                path: vec![user_id.clone(), node],
                perms,
            });
    }
    for org in &owned_orgs {
        let org_node = format!("organization:{}", org.id);
        for ws in input
            .workspaces
            .iter()
            .filter(|w| w.organization_id == org.id)
        {
            let node = format!("workspace:{}", ws.id);
            edges.add(&org_node, &node, "admin ⇒ manage", "derived");
            ws_sources
                .entry(ws.id.to_string())
                .or_default()
                .push(Source {
                    text: format!("owner of {}", org.name),
                    path: vec![user_id.clone(), org_node.clone(), node],
                    perms: WS_OWNER,
                });
        }
    }

    // Connections: managed by the owners of a created one, queried through
    // the workspaces they are released to.
    let mut conn_sources: BTreeMap<String, Vec<Source>> = BTreeMap::new();
    let mut conn_in_graph: BTreeSet<String> = BTreeSet::new();
    let rows: HashMap<&str, &ConnectionRow> = input
        .connections
        .iter()
        .map(|c| (c.name.as_str(), c))
        .collect();
    for org in &owned_orgs {
        let org_node = format!("organization:{}", org.id);
        for c in input
            .connections
            .iter()
            .filter(|c| c.organization_id == org.id)
        {
            conn_in_graph.insert(c.name.clone());
            if c.origin == ConnectionOrigin::Created {
                let node = format!("connection:{}", c.name);
                edges.add(&org_node, &node, "manage", "derived");
                conn_sources
                    .entry(c.name.clone())
                    .or_default()
                    .push(Source {
                        text: format!("owner of {}", org.name),
                        path: vec![user_id.clone(), org_node.clone(), node],
                        perms: &["manage"],
                    });
            }
        }
    }
    for (conn, ws_id) in &input.releases {
        let ws_key = ws_id.to_string();
        let (Some(row), Some(ws), Some(via)) = (
            rows.get(conn.as_str()),
            workspaces.get(&ws_key),
            ws_sources.get(&ws_key),
        ) else {
            continue;
        };
        // The server keeps a release only inside the workspace's organization.
        if row.organization_id != ws.organization_id {
            continue;
        }
        let query_paths: Vec<&Source> = via.iter().filter(|s| s.perms.contains(&"query")).collect();
        if query_paths.is_empty() {
            continue;
        }
        conn_in_graph.insert(row.name.clone());
        if !input.configured.contains(&row.name) {
            continue;
        }
        let ws_node = format!("workspace:{}", ws.id);
        let node = format!("connection:{}", row.name);
        edges.add(&ws_node, &node, "released", "derived");
        for s in query_paths {
            let mut path = s.path.clone();
            path.push(node.clone());
            conn_sources
                .entry(row.name.clone())
                .or_default()
                .push(Source {
                    text: format!("released to {} (as {})", ws.name, s.text),
                    path,
                    perms: &["query"],
                });
        }
    }
    for name in conn_sources.keys() {
        conn_in_graph.insert(name.clone());
    }

    // Nodes and grants, in the documented order.
    let mut org_list: Vec<&Organization> = org_sources
        .keys()
        .filter_map(|k| orgs.get(k).copied())
        .collect();
    org_list.sort_by(|a, b| (&a.name, a.id).cmp(&(&b.name, b.id)));
    for org in org_list {
        let id = format!("organization:{}", org.id);
        graph
            .grants
            .extend(grants_of(&id, ORG_ORDER, &org_sources[&org.id.to_string()]));
        graph.nodes.push(Node {
            id,
            kind: "organization".into(),
            name: org.name.clone(),
            organization_id: None,
            notes: vec![],
        });
    }

    let org_name = |id: Uuid| {
        orgs.get(&id.to_string())
            .map(|o| o.name.clone())
            .unwrap_or_default()
    };
    let mut ws_list: Vec<&Workspace> = ws_sources
        .keys()
        .filter_map(|k| workspaces.get(k).copied())
        .collect();
    ws_list.sort_by(|a, b| {
        (org_name(a.organization_id), &a.name, a.id).cmp(&(
            org_name(b.organization_id),
            &b.name,
            b.id,
        ))
    });
    for ws in ws_list {
        let id = format!("workspace:{}", ws.id);
        graph
            .grants
            .extend(grants_of(&id, WS_ORDER, &ws_sources[&ws.id.to_string()]));
        graph.nodes.push(Node {
            id,
            kind: "workspace".into(),
            name: ws.name.clone(),
            organization_id: Some(ws.organization_id.to_string()),
            notes: vec![],
        });
    }

    for name in &conn_in_graph {
        let Some(row) = rows.get(name.as_str()) else {
            continue;
        };
        let id = format!("connection:{name}");
        let mut notes = Vec::new();
        if row.origin == ConnectionOrigin::Assigned {
            notes.push(NOTE_ASSIGNED.to_string());
        }
        if !input.configured.contains(name) {
            notes.push(NOTE_MISSING.to_string());
        }
        if let Some(sources) = conn_sources.get(name) {
            graph
                .grants
                .extend(grants_of(&id, &["manage", "query"], sources));
        }
        graph.nodes.push(Node {
            id,
            kind: "connection".into(),
            name: name.clone(),
            organization_id: Some(row.organization_id.to_string()),
            notes,
        });
    }
    graph.edges = edges.list;
    graph
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rbac::authz::ObjectRef;
    use chrono::Utc;

    const O: Uuid = Uuid::from_u128(1);
    const W: Uuid = Uuid::from_u128(2);

    fn user(disabled: bool) -> User {
        User {
            id: "ana".into(),
            name: Some("Ana".into()),
            email: None,
            password_hash: String::new(),
            disabled,
            created_at: Utc::now(),
            password_changed_at: Utc::now(),
        }
    }

    fn rel(kind: &'static str, id: Uuid, relation: &'static str) -> Relationship {
        Relationship {
            resource: ObjectRef::new(kind, id.to_string()),
            relation,
            subject: ObjectRef::new("user", "ana"),
        }
    }

    fn org() -> Organization {
        Organization {
            id: O,
            name: "Acme".into(),
            created_at: Utc::now(),
            created_by: "x".into(),
        }
    }

    fn ws() -> Workspace {
        Workspace {
            id: W,
            organization_id: O,
            name: "Operations".into(),
            created_at: Utc::now(),
            created_by: "x".into(),
        }
    }

    fn conn(name: &str, origin: ConnectionOrigin) -> ConnectionRow {
        ConnectionRow {
            name: name.into(),
            organization_id: O,
            origin,
        }
    }

    fn input(direct: Vec<Relationship>) -> AccessInput {
        AccessInput {
            user: user(false),
            platform_admin: false,
            direct,
            organizations: vec![org()],
            workspaces: vec![ws()],
            connections: vec![],
            releases: vec![],
            configured: BTreeSet::new(),
        }
    }

    fn grant<'a>(g: &'a AccessGraph, resource: &str, perm: &str) -> Option<&'a Grant> {
        g.grants
            .iter()
            .find(|x| x.resource == resource && x.permission == perm)
    }

    fn perms(g: &AccessGraph, resource: &str) -> Vec<String> {
        g.grants
            .iter()
            .filter(|x| x.resource == resource)
            .map(|x| x.permission.clone())
            .collect()
    }

    #[test]
    fn a_viewer_can_view_query_and_read_objects() {
        let g = build(&input(vec![rel("workspace", W, "viewer")]));
        let id = format!("workspace:{W}");
        assert_eq!(perms(&g, &id), ["view", "query", "read objects"]);
        let r = &grant(&g, &id, "query").unwrap().reasons;
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].text, "viewer");
        assert_eq!(r[0].path, ["user:ana".to_string(), id.clone()]);
        assert_eq!(g.nodes[0].id, "user:ana");
        assert_eq!(g.edges.len(), 1);
        assert_eq!(
            (g.edges[0].label.as_str(), g.edges[0].kind.as_str()),
            ("viewer", "direct")
        );
    }

    #[test]
    fn an_organization_owner_manages_every_workspace_of_it() {
        let g = build(&input(vec![rel("organization", O, "owner")]));
        let id = format!("workspace:{W}");
        assert_eq!(perms(&g, &id).len(), 7);
        let r = &grant(&g, &id, "manage").unwrap().reasons;
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].text, "owner of Acme");
        assert_eq!(
            r[0].path,
            [
                "user:ana".to_string(),
                format!("organization:{O}"),
                id.clone()
            ]
        );
        assert_eq!(
            perms(&g, &format!("organization:{O}")),
            ["admin", "view", "manage_connections"]
        );
        assert!(g
            .edges
            .iter()
            .any(|e| e.label == "admin ⇒ manage" && e.kind == "derived"));
    }

    #[test]
    fn two_paths_give_two_reasons() {
        let g = build(&input(vec![
            rel("workspace", W, "editor"),
            rel("organization", O, "owner"),
        ]));
        let r = &grant(&g, &format!("workspace:{W}"), "edit")
            .unwrap()
            .reasons;
        let texts: Vec<_> = r.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(texts, ["editor", "owner of Acme"]);
        let w = format!("workspace:{W}");
        assert_eq!(r[0].path, ["user:ana".to_string(), w.clone()]);
        assert_eq!(
            r[1].path,
            ["user:ana".to_string(), format!("organization:{O}"), w]
        );
    }

    fn with_release(origin: ConnectionOrigin, configured: bool) -> AccessGraph {
        let mut i = input(vec![
            rel("workspace", W, "viewer"),
            rel("organization", O, "owner"),
        ]);
        i.connections = vec![conn("db", origin)];
        i.releases = vec![("db".into(), W)];
        if configured {
            i.configured.insert("db".into());
        }
        build(&i)
    }

    #[test]
    fn an_assigned_connection_is_queried_but_not_managed() {
        let g = with_release(ConnectionOrigin::Assigned, true);
        assert!(grant(&g, "connection:db", "manage").is_none());
        let r = &grant(&g, "connection:db", "query").unwrap().reasons;
        // viewer and owner of the organization both query W: one reason each.
        let texts: Vec<_> = r.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "released to Operations (as viewer)",
                "released to Operations (as owner of Acme)"
            ]
        );
        assert_eq!(r[0].path.last().unwrap(), "connection:db");
        let node = g.nodes.iter().find(|n| n.id == "connection:db").unwrap();
        assert_eq!(node.notes, [NOTE_ASSIGNED]);
        let released = g.edges.iter().filter(|e| e.label == "released").count();
        assert_eq!(released, 1);
    }

    #[test]
    fn parallel_roles_share_one_edge() {
        let g = build(&input(vec![
            rel("organization", O, "owner"),
            rel("organization", O, "member"),
        ]));
        let to_org: Vec<_> = g
            .edges
            .iter()
            .filter(|e| e.from == "user:ana" && e.to == format!("organization:{O}"))
            .collect();
        assert_eq!(to_org.len(), 1);
        assert_eq!(to_org[0].label, "owner · member");
        assert_eq!(to_org[0].kind, "direct");
    }

    #[test]
    fn a_merged_edge_is_direct_if_any_part_is() {
        let mut e = Edges::default();
        e.add("a", "b", "x", "derived");
        e.add("a", "b", "y", "direct");
        e.add("a", "b", "x", "derived");
        assert_eq!(e.list.len(), 1);
        assert_eq!(
            (e.list[0].label.as_str(), e.list[0].kind.as_str()),
            ("x · y", "direct")
        );
    }

    #[test]
    fn a_created_connection_is_managed_by_the_owner() {
        let mut i = input(vec![rel("organization", O, "owner")]);
        i.connections = vec![conn("db", ConnectionOrigin::Created)];
        i.configured.insert("db".into());
        let g = build(&i);
        let r = &grant(&g, "connection:db", "manage").unwrap().reasons;
        assert_eq!(r[0].text, "owner of Acme");
        assert_eq!(
            r[0].path,
            [
                "user:ana".to_string(),
                format!("organization:{O}"),
                "connection:db".to_string()
            ]
        );
        assert!(g
            .edges
            .iter()
            .any(|e| e.label == "manage" && e.kind == "derived"));
    }

    #[test]
    fn a_release_to_a_missing_data_source_grants_nothing() {
        // Only a viewer of W: the node can come from the release alone.
        let mut i = input(vec![rel("workspace", W, "viewer")]);
        i.connections = vec![conn("db", ConnectionOrigin::Created)];
        i.releases = vec![("db".into(), W)];
        let g = build(&i);
        let node = g.nodes.iter().find(|n| n.id == "connection:db").unwrap();
        assert!(node.notes.contains(&NOTE_MISSING.to_string()));
        assert!(grant(&g, "connection:db", "query").is_none());
        assert!(g.edges.iter().all(|e| e.label != "released"));
    }

    #[test]
    fn a_release_across_organizations_grants_nothing() {
        let mut i = input(vec![rel("workspace", W, "viewer")]);
        i.connections = vec![ConnectionRow {
            name: "db".into(),
            organization_id: Uuid::from_u128(9),
            origin: ConnectionOrigin::Created,
        }];
        i.releases = vec![("db".into(), W)];
        i.configured.insert("db".into());
        let g = build(&i);
        assert!(grant(&g, "connection:db", "query").is_none());
        assert!(g.edges.iter().all(|e| e.label != "released"));
        assert!(g.nodes.iter().all(|n| n.id != "connection:db"));
    }

    #[test]
    fn a_connection_without_organization_is_left_out() {
        let mut i = input(vec![rel("workspace", W, "viewer")]);
        i.releases = vec![("ghost".into(), W)];
        i.configured.insert("ghost".into());
        let g = build(&i);
        assert!(g.nodes.iter().all(|n| n.id != "connection:ghost"));
        assert!(g.grants.iter().all(|x| x.resource != "connection:ghost"));
    }

    #[test]
    fn a_platform_admin_is_a_single_node() {
        let mut i = input(vec![rel("organization", O, "owner")]);
        i.platform_admin = true;
        let g = build(&i);
        assert!(g.platform_admin);
        assert_eq!(g.nodes.len(), 1);
        assert!(g.edges.is_empty() && g.grants.is_empty());
    }

    #[test]
    fn a_disabled_user_keeps_their_grants_flagged() {
        let mut i = input(vec![rel("workspace", W, "viewer")]);
        i.user = user(true);
        let g = build(&i);
        assert!(g.user.disabled);
        assert!(!g.grants.is_empty());
    }
}
