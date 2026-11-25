//! A user's access graph: the grants they hold, the permissions those derive
//! (the SpiceDB schema plus the server's own restrictions) and why.
//!
//! Pure: the caller loads the inputs, this only assembles them.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::Serialize;
use uuid::Uuid;

use super::authz::Relationship;
use super::restrictions::{Restrictions, RowOperator};
use super::store::{
    ConnectionOrigin, ConnectionRow, Organization, Role, Rule, RuleRole, User, Workspace,
};

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

/// `(filters: orders.region = region; blocks: orders.margin)`: what a role
/// hides, a filter reading `member = attribute`, `member != [v1, v2]` for
/// fixed values. `None` when the role has no restrictions.
fn restriction_note(r: &Restrictions) -> Option<String> {
    let mut parts = Vec::new();
    if !r.row_filters.is_empty() {
        let filters: Vec<String> = r
            .row_filters
            .iter()
            .map(|f| {
                let op = match f.operator {
                    RowOperator::Equals => "=",
                    RowOperator::NotEquals => "!=",
                };
                let source = match (&f.attribute, &f.values) {
                    (Some(attribute), _) => attribute.clone(),
                    (None, values) => format!(
                        "[{}]",
                        values
                            .iter()
                            .flatten()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                };
                format!("{} {op} {source}", f.member)
            })
            .collect();
        parts.push(format!("filters: {}", filters.join(", ")));
    }
    if !r.blocked_members.is_empty() {
        parts.push(format!("blocks: {}", r.blocked_members.join(", ")));
    }
    (!parts.is_empty()).then(|| format!("({})", parts.join("; ")))
}

/// The graph's names for the permissions of an organization's role
/// (`manage` grants everything a workspace owner has).
fn role_perms(permissions: &[String]) -> Vec<&'static str> {
    if permissions.iter().any(|p| p == "manage") {
        return WS_OWNER.to_vec();
    }
    permissions
        .iter()
        .filter_map(|p| match p.as_str() {
            "view" => Some("view"),
            "query" => Some("query"),
            "read_objects" => Some("read objects"),
            "write_objects" => Some("write objects"),
            "execute" => Some("execute"),
            _ => None,
        })
        .collect()
}

const ORG_ORDER: &[&str] = &["admin", "view", "manage_connections"];
const WS_ORDER: &[&str] = WS_OWNER;

const NOTE_ASSIGNED: &str = "assigned by a platform admin: only platform admins edit it";
const NOTE_MISSING: &str = "data source missing";
const NOTE_UNRELEASED: &str = "no access: not released to a workspace of the user";
const NOTE_PLATFORM: &str = "platform admins only";
const NOTE_VIEW: &str = "view";
const NOTE_VIEW_DENIED: &str = "no access: spans a data source the user cannot query";
const NOTE_VIEW_SPLIT: &str =
    "no access: no workspace of the user is released every data source it spans";

/// The Platform: the data sources and cubes in no organization.
const PLATFORM: &str = "_platform";

pub struct AccessInput {
    pub user: User,
    pub platform_admin: bool,
    /// Relationships whose subject is the user.
    pub direct: Vec<Relationship>,
    pub organizations: Vec<Organization>,
    pub workspaces: Vec<Workspace>,
    /// The rules the user is a member of.
    pub rules: Vec<Rule>,
    /// The organization roles those rules grant.
    pub roles: Vec<Role>,
    pub connections: Vec<ConnectionRow>,
    /// (connection, workspace) pairs.
    pub releases: Vec<(String, Uuid)>,
    /// Data source names in `cube.yml`.
    pub configured: BTreeSet<String>,
    /// Configured data sources that are in no organization, sorted.
    pub platform_sources: Vec<String>,
    pub cubes: Vec<AccessCube>,
}

pub struct AccessCube {
    pub name: String,
    pub view: bool,
    /// `_platform` or an organization slug; unowned files are left out by the caller.
    pub folder: String,
    /// Every data source the cube reads: its own; for a view, those of the cubes it includes (in its folder's scope).
    pub data_sources: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GraphUser {
    /// The UUID.
    pub id: String,
    pub username: String,
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
    /// A role's permissions.
    pub details: Vec<String>,
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
    perms: Vec<&'static str>,
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
            .unwrap_or_else(|| input.user.username.clone()),
        organization_id: None,
        notes: vec![],
        details: vec![],
    };
    let mut graph = AccessGraph {
        user: GraphUser {
            id: input.user.id.clone(),
            username: input.user.username.clone(),
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
                perms: perms.to_vec(),
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
                perms: perms.to_vec(),
            });
    }
    // Workspaces: the role of each rule the user is a member of, through the
    // rule's node; the role is a node of its own, shared by its rules.
    let mut rule_nodes: Vec<(&Rule, &Workspace)> = Vec::new();
    let mut role_nodes: BTreeMap<String, Node> = BTreeMap::new();
    for rule in &input.rules {
        let Some(ws) = workspaces.get(&rule.workspace_id.to_string()) else {
            continue;
        };
        let (role_id, role, perms, note, role_org): (
            String,
            String,
            Vec<&'static str>,
            Option<String>,
            Option<String>,
        ) = match rule.role_ref() {
            RuleRole::Builtin(r) => (
                format!("role:builtin:{r}"),
                r.to_string(),
                match r {
                    "owner" => WS_OWNER,
                    "editor" => WS_EDITOR,
                    "viewer" => WS_VIEWER,
                    _ => continue,
                }
                .to_vec(),
                None,
                None,
            ),
            RuleRole::Custom(id) => {
                let Some(role) = input.roles.iter().find(|r| r.id == id) else {
                    continue;
                };
                (
                    format!("role:{id}"),
                    role.name.clone(),
                    role_perms(&role.permissions),
                    restriction_note(&role.restrictions),
                    Some(role.organization_id.to_string()),
                )
            }
        };
        let node = format!("workspace:{}", ws.id);
        let rule_node = format!("rule:{}", rule.id);
        rule_nodes.push((rule, ws));
        role_nodes.entry(role_id.clone()).or_insert_with(|| Node {
            id: role_id.clone(),
            kind: "role".into(),
            name: role.clone(),
            organization_id: role_org,
            notes: note.clone().into_iter().collect(),
            details: perms.iter().map(|p| p.to_string()).collect(),
        });
        edges.add(&user_id, &rule_node, "member", "direct");
        edges.add(&rule_node, &role_id, "role", "derived");
        edges.add(&rule_node, &node, &role, "derived");
        ws_sources
            .entry(ws.id.to_string())
            .or_default()
            .push(Source {
                text: format!(
                    "{role} through the rule {}{}",
                    rule.description,
                    note.map(|n| format!(" {n}")).unwrap_or_default()
                ),
                path: vec![user_id.clone(), rule_node.clone(), node.clone()],
                perms: perms.to_vec(),
            });
        // `organization#rule`: the rule's members see its organization.
        if let Some(org) = orgs.get(&ws.organization_id.to_string()) {
            let org_node = format!("organization:{}", org.id);
            org_sources
                .entry(org.id.to_string())
                .or_default()
                .push(Source {
                    text: format!("member of the rule {}", rule.description),
                    path: vec![user_id.clone(), rule_node, node, org_node],
                    perms: ORG_MEMBER.to_vec(),
                });
        }
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
                    perms: WS_OWNER.to_vec(),
                });
        }
    }

    // Connections: managed by the owners of a created one, queried through
    // the workspaces they are released to.
    let mut conn_sources: BTreeMap<String, Vec<Source>> = BTreeMap::new();
    let mut conn_in_graph: BTreeSet<String> = BTreeSet::new();
    // Released to a workspace where the user holds `query`.
    let mut released_to_user: BTreeSet<String> = BTreeSet::new();
    // Per configured connection, its query paths, each with the workspace it
    // goes through: a request queries in one workspace, so a cube or view is
    // queried only through a workspace released every one of its sources.
    let mut conn_query: BTreeMap<String, Vec<(String, Source)>> = BTreeMap::new();
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
                        perms: vec!["manage"],
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
        released_to_user.insert(row.name.clone());
        if !input.configured.contains(&row.name) {
            continue;
        }
        let ws_node = format!("workspace:{}", ws.id);
        let node = format!("connection:{}", row.name);
        edges.add(&ws_node, &node, "released", "derived");
        for s in query_paths {
            let mut path = s.path.clone();
            path.push(node.clone());
            let text = format!("released to {} (as {})", ws.name, s.text);
            conn_query.entry(row.name.clone()).or_default().push((
                ws_key.clone(),
                Source {
                    text: text.clone(),
                    path: path.clone(),
                    perms: vec!["query"],
                },
            ));
            conn_sources
                .entry(row.name.clone())
                .or_default()
                .push(Source {
                    text,
                    path,
                    perms: vec!["query"],
                });
        }
    }
    for name in conn_sources.keys() {
        conn_in_graph.insert(name.clone());
    }
    // Every connection of an organization in the graph, with or without access.
    for c in input
        .connections
        .iter()
        .filter(|c| org_sources.contains_key(&c.organization_id.to_string()))
    {
        conn_in_graph.insert(c.name.clone());
    }

    // Nodes and grants, in column order: user, organization, rule, role,
    // workspace, connection, cube.
    let mut org_list: Vec<&Organization> = org_sources
        .keys()
        .filter_map(|k| orgs.get(k).copied())
        .collect();
    org_list.sort_by(|a, b| (&a.name, a.id).cmp(&(&b.name, b.id)));
    let platform_node = format!("organization:{PLATFORM}");
    let platform =
        !input.platform_sources.is_empty() || input.cubes.iter().any(|c| c.folder == PLATFORM);
    if platform {
        edges.add(&user_id, &platform_node, "platform", "derived");
        graph.nodes.push(Node {
            id: platform_node.clone(),
            kind: "organization".into(),
            name: "Platform".into(),
            organization_id: None,
            notes: vec![],
            details: vec![],
        });
    }
    for org in &org_list {
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
            details: vec![],
        });
    }

    let org_name = |id: Uuid| {
        orgs.get(&id.to_string())
            .map(|o| o.name.clone())
            .unwrap_or_default()
    };
    rule_nodes.sort_by(|(ra, wa), (rb, wb)| {
        (
            org_name(wa.organization_id),
            &wa.name,
            &ra.description,
            ra.id,
        )
            .cmp(&(
                org_name(wb.organization_id),
                &wb.name,
                &rb.description,
                rb.id,
            ))
    });
    rule_nodes.dedup_by_key(|(r, _)| r.id);
    for (rule, ws) in &rule_nodes {
        graph.nodes.push(Node {
            id: format!("rule:{}", rule.id),
            kind: "rule".into(),
            name: rule.description.clone(),
            organization_id: Some(ws.organization_id.to_string()),
            notes: vec![],
            details: vec![],
        });
    }
    // Built-in roles first (owner, editor, viewer), then by name.
    let builtin_rank = |id: &str| match id {
        "role:builtin:owner" => 0,
        "role:builtin:editor" => 1,
        "role:builtin:viewer" => 2,
        _ => 3,
    };
    let mut roles: Vec<Node> = role_nodes.into_values().collect();
    roles.sort_by(|a, b| {
        (builtin_rank(&a.id), &a.name, &a.id).cmp(&(builtin_rank(&b.id), &b.name, &b.id))
    });
    graph.nodes.extend(roles);

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
    for ws in &ws_list {
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
            details: vec![],
        });
    }

    // Connections: the Platform's first (platform admins query them), then
    // the organizations'.
    let mut conn_nodes: BTreeSet<&str> = BTreeSet::new();
    for name in &input.platform_sources {
        let id = format!("connection:{name}");
        edges.add(&platform_node, &id, "of", "derived");
        conn_nodes.insert(name);
        graph.nodes.push(Node {
            id,
            kind: "connection".into(),
            name: name.clone(),
            organization_id: Some(PLATFORM.into()),
            notes: vec![NOTE_PLATFORM.into()],
            details: vec![],
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
        match conn_sources.get(name) {
            Some(sources) => graph
                .grants
                .extend(grants_of(&id, &["manage", "query"], sources)),
            None => {
                if !released_to_user.contains(name) {
                    notes.push(NOTE_UNRELEASED.to_string());
                }
                let org_key = row.organization_id.to_string();
                if org_sources.contains_key(&org_key) {
                    edges.add(&format!("organization:{org_key}"), &id, "of", "derived");
                }
            }
        }
        conn_nodes.insert(name);
        graph.nodes.push(Node {
            id,
            kind: "connection".into(),
            name: name.clone(),
            organization_id: Some(row.organization_id.to_string()),
            notes,
            details: vec![],
        });
    }

    // Cubes: those of the Platform and of the user's organizations, hanging
    // off the connections of their data sources; queried only through a
    // workspace released every one of them (enforcement checks a request's
    // data sources against its one workspace).
    let mut folders: Vec<(&str, String)> = Vec::new();
    if platform {
        folders.push((PLATFORM, PLATFORM.to_string()));
    }
    let mut cube_orgs: Vec<&Organization> = org_list.clone();
    for ws in &ws_list {
        if let Some(org) = orgs.get(&ws.organization_id.to_string()) {
            if !cube_orgs.iter().any(|o| o.id == org.id) {
                cube_orgs.push(org);
            }
        }
    }
    cube_orgs.sort_by(|a, b| (&a.name, a.id).cmp(&(&b.name, b.id)));
    folders.extend(
        cube_orgs
            .iter()
            .map(|o| (o.slug.as_str(), o.id.to_string())),
    );
    for (folder, org_id) in &folders {
        let mut cubes: Vec<&AccessCube> = input
            .cubes
            .iter()
            .filter(|c| c.folder == *folder)
            .filter(|c| {
                !c.data_sources.is_empty()
                    && c.data_sources
                        .iter()
                        .all(|d| conn_nodes.contains(d.as_str()))
            })
            .collect();
        cubes.sort_by(|a, b| a.name.cmp(&b.name));
        for cube in cubes {
            let id = format!("cube:{folder}:{}", cube.name);
            // The workspaces through which every data source is queried.
            let mut common: Option<BTreeSet<&str>> = None;
            let mut each_queried = true;
            for d in &cube.data_sources {
                let conn = format!("connection:{d}");
                edges.add(&conn, &id, "serves", "derived");
                let ws: BTreeSet<&str> = conn_query
                    .get(d)
                    .map(|v| v.iter().map(|(w, _)| w.as_str()).collect())
                    .unwrap_or_default();
                each_queried &= !ws.is_empty();
                common = Some(match common {
                    None => ws,
                    Some(c) => c.intersection(&ws).copied().collect(),
                });
            }
            let common = common.unwrap_or_default();
            // One reason per path through a common workspace, per data source.
            let mut reasons: Vec<Source> = Vec::new();
            for d in &cube.data_sources {
                for (w, s) in conn_query.get(d).into_iter().flatten() {
                    if !common.contains(w.as_str()) {
                        continue;
                    }
                    let mut path = s.path.clone();
                    path.push(id.clone());
                    reasons.push(Source {
                        text: s.text.clone(),
                        path,
                        perms: vec!["query"],
                    });
                }
            }
            let mut notes = Vec::new();
            if cube.view {
                notes.push(NOTE_VIEW.to_string());
            }
            // An organization's cube never reads a Platform source: the
            // ownership policy rejects it, so only the folder says Platform.
            if !common.is_empty() {
                graph.grants.extend(grants_of(&id, &["query"], &reasons));
            } else if *folder == PLATFORM {
                notes.push(NOTE_PLATFORM.to_string());
            } else if cube.view {
                notes.push(if each_queried {
                    NOTE_VIEW_SPLIT.to_string()
                } else {
                    NOTE_VIEW_DENIED.to_string()
                });
            } else if cube
                .data_sources
                .iter()
                .any(|d| released_to_user.contains(d) && !input.configured.contains(d))
            {
                notes.push(NOTE_MISSING.to_string());
            } else {
                notes.push(NOTE_UNRELEASED.to_string());
            }
            graph.nodes.push(Node {
                id,
                kind: "cube".into(),
                name: cube.name.clone(),
                organization_id: Some(org_id.clone()),
                notes,
                details: vec![],
            });
        }
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
            username: "ana".into(),
            name: Some("Ana".into()),
            email: None,
            password_hash: String::new(),
            disabled,
            created_at: Utc::now(),
            password_changed_at: Utc::now(),
            username_changed_at: None,
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
            slug: "acme".into(),
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
            rules: vec![],
            roles: vec![],
            connections: vec![],
            releases: vec![],
            configured: BTreeSet::new(),
            platform_sources: vec![],
            cubes: vec![],
        }
    }

    const P: Uuid = Uuid::from_u128(10);

    fn other_org() -> Organization {
        Organization {
            id: P,
            name: "Pinnacle".into(),
            slug: "pinnacle".into(),
            created_at: Utc::now(),
            created_by: "x".into(),
        }
    }

    fn cube(name: &str, folder: &str, view: bool, sources: &[&str]) -> AccessCube {
        AccessCube {
            name: name.into(),
            view,
            folder: folder.into(),
            data_sources: sources.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn rule(id: u128, workspace_id: Uuid, role: Option<&str>, role_id: Option<Uuid>) -> Rule {
        Rule {
            id: Uuid::from_u128(id),
            workspace_id,
            description: format!("Rule {id}"),
            r#type: "t".into(),
            role: role.map(Into::into),
            role_id,
            created_at: Utc::now(),
            created_by: "x".into(),
        }
    }

    fn node<'a>(g: &'a AccessGraph, id: &str) -> Option<&'a Node> {
        g.nodes.iter().find(|n| n.id == id)
    }

    fn has_edge(g: &AccessGraph, from: &str, to: &str, label: &str, kind: &str) -> bool {
        g.edges
            .iter()
            .any(|e| e.from == from && e.to == to && e.label == label && e.kind == kind)
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
    fn a_rule_member_holds_the_rule_type_in_its_workspace() {
        let mut i = input(vec![]);
        i.rules = vec![Rule {
            id: Uuid::from_u128(3),
            workspace_id: W,
            description: "Analysts".into(),
            r#type: "finance".into(),
            role: Some("editor".into()),
            role_id: None,
            created_at: Utc::now(),
            created_by: "x".into(),
        }];
        let g = build(&i);
        let id = format!("workspace:{W}");
        assert_eq!(
            perms(&g, &id),
            ["edit", "view", "query", "read objects", "write objects"]
        );
        let r = &grant(&g, &id, "edit").unwrap().reasons;
        assert_eq!(r[0].text, "editor through the rule Analysts");
        let rule = format!("rule:{}", Uuid::from_u128(3));
        assert!(has_edge(&g, "user:ana", &rule, "member", "direct"));
        assert!(has_edge(&g, &rule, &id, "editor", "derived"));
        assert!(g
            .edges
            .iter()
            .all(|e| !(e.from == "user:ana" && e.to == id)));
        assert_eq!(r[0].path, ["user:ana".to_string(), rule, id.clone()]);
        assert_eq!(perms(&g, &format!("organization:{O}")), ["view"]);
    }

    #[test]
    fn a_rule_with_an_organization_role_grants_its_permissions() {
        let role = Uuid::from_u128(4);
        let mut i = input(vec![]);
        i.rules = vec![Rule {
            id: Uuid::from_u128(3),
            workspace_id: W,
            description: "Analysts".into(),
            r#type: "finance".into(),
            role: None,
            role_id: Some(role),
            created_at: Utc::now(),
            created_by: "x".into(),
        }];
        i.roles = vec![Role {
            id: role,
            organization_id: O,
            builtin: None,
            name: "Analyst".into(),
            permissions: vec!["view".into(), "query".into()],
            restrictions: Default::default(),
            created_at: Utc::now(),
            created_by: "x".into(),
        }];
        let g = build(&i);
        let id = format!("workspace:{W}");
        assert_eq!(perms(&g, &id), ["view", "query"]);
        let r = &grant(&g, &id, "query").unwrap().reasons;
        assert_eq!(r[0].text, "Analyst through the rule Analysts");
    }

    #[test]
    fn a_restricted_rule_explains_its_restrictions() {
        use crate::rbac::restrictions::RowFilter;
        let role = Uuid::from_u128(4);
        let mut i = input(vec![]);
        i.rules = vec![Rule {
            id: Uuid::from_u128(3),
            workspace_id: W,
            description: "Analysts".into(),
            r#type: "finance".into(),
            role: None,
            role_id: Some(role),
            created_at: Utc::now(),
            created_by: "x".into(),
        }];
        i.roles = vec![Role {
            id: role,
            organization_id: O,
            builtin: None,
            name: "Analyst".into(),
            permissions: vec!["view".into(), "query".into()],
            restrictions: Restrictions {
                row_filters: vec![
                    RowFilter {
                        member: "orders.region".into(),
                        operator: RowOperator::Equals,
                        attribute: Some("region".into()),
                        values: None,
                    },
                    RowFilter {
                        member: "orders.status".into(),
                        operator: RowOperator::NotEquals,
                        attribute: None,
                        values: Some(vec!["draft".into(), "void".into()]),
                    },
                ],
                blocked_members: vec!["orders.margin".into()],
            },
            created_at: Utc::now(),
            created_by: "x".into(),
        }];
        let g = build(&i);
        let r = &grant(&g, &format!("workspace:{W}"), "query")
            .unwrap()
            .reasons;
        assert_eq!(
            r[0].text,
            "Analyst through the rule Analysts (filters: orders.region = region, orders.status != [draft, void]; blocks: orders.margin)"
        );
        assert_eq!(restriction_note(&Restrictions::default()), None);
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
        i.platform_sources = vec!["default".into()];
        i.cubes = vec![cube("u", "_platform", false, &["default"])];
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

    #[test]
    fn every_connection_of_a_users_organization_is_shown() {
        let mut i = input(vec![rel("organization", O, "member")]);
        i.connections = vec![conn("c", ConnectionOrigin::Assigned)];
        i.configured.insert("c".into());
        let g = build(&i);
        let n = node(&g, "connection:c").unwrap();
        assert!(n.notes.contains(&NOTE_UNRELEASED.to_string()));
        assert_eq!(
            NOTE_UNRELEASED,
            "no access: not released to a workspace of the user"
        );
        assert!(perms(&g, "connection:c").is_empty());
        assert!(has_edge(
            &g,
            &format!("organization:{O}"),
            "connection:c",
            "of",
            "derived"
        ));
    }

    #[test]
    fn the_platform_is_shown_for_admins_only() {
        let mut i = input(vec![rel("organization", O, "member")]);
        i.platform_sources = vec!["default".into()];
        i.cubes = vec![cube("u", "_platform", false, &["default"])];
        let g = build(&i);
        let p = node(&g, "organization:_platform").unwrap();
        assert_eq!(p.name, "Platform");
        assert!(has_edge(
            &g,
            "user:ana",
            "organization:_platform",
            "platform",
            "derived"
        ));
        let c = node(&g, "connection:default").unwrap();
        assert_eq!(c.notes, ["platform admins only"]);
        assert!(node(&g, "cube:_platform:u").is_some());
        for id in [
            "organization:_platform",
            "connection:default",
            "cube:_platform:u",
        ] {
            assert!(perms(&g, id).is_empty(), "{id} has a grant");
        }
        // Placed first in the organization column.
        assert_eq!(g.nodes[1].id, "organization:_platform");
    }

    #[test]
    fn a_rule_and_its_role_are_nodes() {
        let x = Uuid::from_u128(4);
        let mut i = input(vec![]);
        i.rules = vec![rule(3, W, None, Some(x))];
        i.roles = vec![Role {
            id: x,
            organization_id: O,
            builtin: None,
            name: "Analyst".into(),
            permissions: vec!["view".into(), "query".into()],
            restrictions: Restrictions {
                row_filters: vec![],
                blocked_members: vec!["orders.margin".into()],
            },
            created_at: Utc::now(),
            created_by: "x".into(),
        }];
        let g = build(&i);
        let r = format!("rule:{}", Uuid::from_u128(3));
        let role = format!("role:{x}");
        let w = format!("workspace:{W}");
        assert_eq!(node(&g, &r).unwrap().kind, "rule");
        let rn = node(&g, &role).unwrap();
        assert_eq!(rn.kind, "role");
        assert_eq!(rn.notes, ["(blocks: orders.margin)"]);
        assert_eq!(rn.details, ["view", "query"]);
        assert!(has_edge(&g, "user:ana", &r, "member", "direct"));
        assert!(has_edge(&g, &r, &role, "role", "derived"));
        assert!(has_edge(&g, &r, &w, "Analyst", "derived"));
        assert_eq!(perms(&g, &w), ["view", "query"]);
        let reasons = &grant(&g, &w, "query").unwrap().reasons;
        assert_eq!(reasons[0].path, ["user:ana".to_string(), r, w.clone()]);
        assert!(perms(&g, &role).is_empty());
        // Column order: user, organization, rule, role, workspace.
        let kinds: Vec<_> = g.nodes.iter().map(|n| n.kind.as_str()).collect();
        assert_eq!(kinds, ["user", "organization", "rule", "role", "workspace"]);
    }

    #[test]
    fn a_builtin_rule_role_is_one_shared_node() {
        let w2 = Uuid::from_u128(5);
        let mut i = input(vec![]);
        i.workspaces.push(Workspace {
            id: w2,
            organization_id: O,
            name: "Sales".into(),
            created_at: Utc::now(),
            created_by: "x".into(),
        });
        i.rules = vec![
            rule(3, W, Some("viewer"), None),
            rule(6, w2, Some("viewer"), None),
        ];
        let g = build(&i);
        let roles: Vec<_> = g.nodes.iter().filter(|n| n.kind == "role").collect();
        assert_eq!(roles.len(), 1);
        assert_eq!(roles[0].id, "role:builtin:viewer");
        assert_eq!(roles[0].details, ["view", "query", "read objects"]);
        assert_eq!(g.nodes.iter().filter(|n| n.kind == "rule").count(), 2);
    }

    #[test]
    fn cubes_hang_off_their_connections() {
        let mut i = input(vec![rel("workspace", W, "viewer")]);
        i.organizations.push(other_org());
        i.connections = vec![
            conn("c", ConnectionOrigin::Assigned),
            ConnectionRow {
                name: "c2".into(),
                organization_id: P,
                origin: ConnectionOrigin::Assigned,
            },
        ];
        i.releases = vec![("c".into(), W)];
        i.configured.extend(["c".to_string(), "c2".to_string()]);
        i.cubes = vec![
            cube("a", "acme", false, &["c"]),
            cube("b", "pinnacle", false, &["c2"]),
        ];
        let g = build(&i);
        let a = node(&g, "cube:acme:a").unwrap();
        assert_eq!(a.kind, "cube");
        assert!(a.notes.is_empty());
        assert!(has_edge(
            &g,
            "connection:c",
            "cube:acme:a",
            "serves",
            "derived"
        ));
        let r = &grant(&g, "cube:acme:a", "query").unwrap().reasons;
        assert_eq!(
            r[0].path,
            [
                "user:ana".to_string(),
                format!("workspace:{W}"),
                "connection:c".to_string(),
                "cube:acme:a".to_string()
            ]
        );
        assert!(node(&g, "cube:pinnacle:b").is_none());
        assert_eq!(g.nodes.last().unwrap().kind, "cube");
    }

    #[test]
    fn same_name_in_two_organizations_is_two_cubes() {
        let mut i = input(vec![
            rel("organization", O, "member"),
            rel("organization", P, "member"),
        ]);
        i.organizations.push(other_org());
        i.connections = vec![
            conn("c", ConnectionOrigin::Assigned),
            ConnectionRow {
                name: "c2".into(),
                organization_id: P,
                origin: ConnectionOrigin::Assigned,
            },
        ];
        i.configured.extend(["c".to_string(), "c2".to_string()]);
        i.cubes = vec![
            cube("x", "pinnacle", false, &["c2"]),
            cube("x", "acme", false, &["c"]),
        ];
        let g = build(&i);
        let cubes: Vec<_> = g
            .nodes
            .iter()
            .filter(|n| n.kind == "cube")
            .map(|n| n.id.as_str())
            .collect();
        // Sorted by organization (Acme before Pinnacle).
        assert_eq!(cubes, ["cube:acme:x", "cube:pinnacle:x"]);
    }

    #[test]
    fn a_view_spanning_an_unreleased_connection_has_no_query() {
        let mut i = input(vec![
            rel("organization", O, "member"),
            rel("workspace", W, "viewer"),
        ]);
        i.connections = vec![
            conn("c", ConnectionOrigin::Assigned),
            conn("d", ConnectionOrigin::Assigned),
        ];
        i.releases = vec![("c".into(), W)];
        i.configured.extend(["c".to_string(), "d".to_string()]);
        i.cubes = vec![cube("v", "acme", true, &["c", "d"])];
        let g = build(&i);
        let v = node(&g, "cube:acme:v").unwrap();
        assert_eq!(
            v.notes,
            [
                "view",
                "no access: spans a data source the user cannot query"
            ]
        );
        assert!(perms(&g, "cube:acme:v").is_empty());
        assert!(has_edge(
            &g,
            "connection:c",
            "cube:acme:v",
            "serves",
            "derived"
        ));
        assert!(has_edge(
            &g,
            "connection:d",
            "cube:acme:v",
            "serves",
            "derived"
        ));
        assert!(grant(&g, "connection:c", "query").is_some());
    }

    fn workspace(id: Uuid, name: &str) -> Workspace {
        Workspace {
            id,
            organization_id: O,
            name: name.into(),
            created_at: Utc::now(),
            created_by: "x".into(),
        }
    }

    /// A viewer of W and of W2 in Acme, with `c` and `d` configured and
    /// released as given, and a view `v` over both.
    fn view_over(releases: &[(&str, Uuid)]) -> AccessGraph {
        let w2 = Uuid::from_u128(5);
        let mut i = input(vec![
            rel("workspace", W, "viewer"),
            rel("workspace", w2, "viewer"),
        ]);
        i.workspaces.push(workspace(w2, "Sales"));
        i.connections = vec![
            conn("c", ConnectionOrigin::Assigned),
            conn("d", ConnectionOrigin::Assigned),
        ];
        i.releases = releases.iter().map(|(c, w)| (c.to_string(), *w)).collect();
        i.configured.extend(["c".to_string(), "d".to_string()]);
        i.cubes = vec![cube("v", "acme", true, &["c", "d"])];
        build(&i)
    }

    #[test]
    fn a_view_over_connections_released_to_different_workspaces_has_no_query() {
        let w2 = Uuid::from_u128(5);
        let g = view_over(&[("c", W), ("d", w2)]);
        // Each connection alone is queried, through its own workspace.
        assert!(grant(&g, "connection:c", "query").is_some());
        assert!(grant(&g, "connection:d", "query").is_some());
        // No one workspace is released both: the server refuses the view in each.
        assert!(perms(&g, "cube:acme:v").is_empty());
        let v = node(&g, "cube:acme:v").unwrap();
        assert_eq!(
            v.notes,
            [
                "view",
                "no access: no workspace of the user is released every data source it spans"
            ]
        );
    }

    #[test]
    fn a_view_is_queried_only_through_the_workspaces_released_all_its_sources() {
        let w2 = Uuid::from_u128(5);
        // c in W and W2, d in W2 only: the view is queried through W2 alone.
        let g = view_over(&[("c", W), ("c", w2), ("d", w2)]);
        let r = &grant(&g, "cube:acme:v", "query").unwrap().reasons;
        let w2_node = format!("workspace:{w2}");
        let paths: Vec<_> = r.iter().map(|x| x.path.clone()).collect();
        assert_eq!(
            paths,
            [
                vec![
                    "user:ana".to_string(),
                    w2_node.clone(),
                    "connection:c".to_string(),
                    "cube:acme:v".to_string()
                ],
                vec![
                    "user:ana".to_string(),
                    w2_node,
                    "connection:d".to_string(),
                    "cube:acme:v".to_string()
                ],
            ]
        );
        assert!(r.iter().all(|x| x.text == "released to Sales (as viewer)"));
        assert_eq!(node(&g, "cube:acme:v").unwrap().notes, ["view"]);
    }

    #[test]
    fn a_custom_role_used_in_two_workspaces_is_one_node() {
        let x = Uuid::from_u128(4);
        let w2 = Uuid::from_u128(5);
        let mut i = input(vec![]);
        i.workspaces.push(workspace(w2, "Sales"));
        i.rules = vec![rule(3, W, None, Some(x)), rule(6, w2, None, Some(x))];
        i.roles = vec![Role {
            id: x,
            organization_id: O,
            builtin: None,
            name: "Analyst".into(),
            permissions: vec!["view".into(), "query".into()],
            restrictions: Default::default(),
            created_at: Utc::now(),
            created_by: "x".into(),
        }];
        let g = build(&i);
        let role = format!("role:{x}");
        let roles: Vec<_> = g.nodes.iter().filter(|n| n.kind == "role").collect();
        assert_eq!(roles.len(), 1);
        assert_eq!(roles[0].id, role);
        let rules: Vec<_> = g
            .nodes
            .iter()
            .filter(|n| n.kind == "rule")
            .map(|n| n.id.clone())
            .collect();
        assert_eq!(
            rules,
            [
                format!("rule:{}", Uuid::from_u128(3)),
                format!("rule:{}", Uuid::from_u128(6))
            ]
        );
        for r in &rules {
            assert!(has_edge(&g, r, &role, "role", "derived"));
        }
        assert!(g
            .edges
            .iter()
            .all(|e| !(e.from == role && e.to.starts_with("workspace:"))));
        assert!(g.edges.iter().all(|e| e.from != role));
    }

    #[test]
    fn a_user_without_organizations_sees_the_platform_only() {
        let mut i = input(vec![]);
        i.organizations = vec![];
        i.workspaces = vec![];
        i.platform_sources = vec!["default".into()];
        i.configured.insert("default".into());
        i.cubes = vec![cube("u", "_platform", false, &["default"])];
        let g = build(&i);
        let ids: Vec<_> = g.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "user:ana",
                "organization:_platform",
                "connection:default",
                "cube:_platform:u"
            ]
        );
        assert_eq!(
            node(&g, "connection:default").unwrap().notes,
            ["platform admins only"]
        );
        assert!(g.grants.is_empty());
    }

    #[test]
    fn a_cube_over_a_released_missing_data_source_says_so() {
        let mut i = input(vec![rel("workspace", W, "viewer")]);
        i.connections = vec![conn("db", ConnectionOrigin::Assigned)];
        i.releases = vec![("db".into(), W)];
        i.cubes = vec![cube("a", "acme", false, &["db"])];
        let g = build(&i);
        assert!(perms(&g, "cube:acme:a").is_empty());
        assert_eq!(node(&g, "cube:acme:a").unwrap().notes, [NOTE_MISSING]);
    }
}
