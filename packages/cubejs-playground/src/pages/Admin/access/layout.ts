import { AccessEdge, AccessGraph, AccessNode, AccessNodeKind, Reason } from './types';

export const COLLAPSE_AT = 8;
export const NODE_W = 176;
export const NODE_H = 58;

const GAP = 22;
const PAD = 20;
const COLUMN_X: Record<AccessNodeKind, number> = {
  user: 20,
  organization: 260,
  workspace: 510,
  connection: 764,
};

export type LaidOutNode = AccessNode & {
  x: number;
  y: number;
  perms: string;
  hiddenCount?: number;
};

/**
 * `expanded` holds organization node ids (`organization:<uuid>`); an
 * organization with more than COLLAPSE_AT workspaces is collapsed unless its
 * id is in the set.
 */
export function layout(
  graph: AccessGraph,
  expanded: Set<string>
): { nodes: LaidOutNode[]; edges: AccessEdge[]; width: number; height: number } {
  const perms = new Map<string, string[]>();
  graph.grants.forEach((g) => {
    perms.set(g.resource, [...(perms.get(g.resource) || []), g.permission]);
  });

  // A workspace's organizationId is the bare UUID of the organization node.
  const workspaceCount = new Map<string, number>();
  graph.nodes.forEach((n) => {
    if (n.kind === 'workspace' && n.organizationId) {
      const key = `organization:${n.organizationId}`;
      workspaceCount.set(key, (workspaceCount.get(key) || 0) + 1);
    }
  });
  const collapsed = new Set<string>();
  workspaceCount.forEach((count, orgId) => {
    if (count > COLLAPSE_AT && !expanded.has(orgId)) {
      collapsed.add(orgId);
    }
  });

  const hidden = new Set<string>();
  graph.nodes.forEach((n) => {
    const orgId = n.kind === 'workspace' && n.organizationId ? `organization:${n.organizationId}` : null;
    if (orgId && collapsed.has(orgId)) {
      hidden.add(n.id);
    }
  });

  const rowOf: Record<AccessNodeKind, number> = {
    user: 0,
    organization: 0,
    workspace: 0,
    connection: 0,
  };
  let bottom = 0;
  const nodes: LaidOutNode[] = [];
  graph.nodes.forEach((n) => {
    if (hidden.has(n.id)) {
      return;
    }
    const y = PAD + rowOf[n.kind]++ * (NODE_H + GAP);
    bottom = Math.max(bottom, y + NODE_H);
    const laid: LaidOutNode = {
      ...n,
      x: COLUMN_X[n.kind],
      y,
      perms: (perms.get(n.id) || []).join(' · '),
    };
    if (collapsed.has(n.id)) {
      laid.hiddenCount = workspaceCount.get(n.id);
    }
    nodes.push(laid);
  });

  return {
    nodes,
    edges: graph.edges.filter((e) => !hidden.has(e.from) && !hidden.has(e.to)),
    width: COLUMN_X.connection + NODE_W + PAD,
    height: bottom + PAD,
  };
}

export function highlight(reason: Reason): { nodes: Set<string>; edges: Set<string> } {
  const edges = new Set<string>();
  for (let i = 0; i + 1 < reason.path.length; i++) {
    edges.add(`${reason.path[i]}>${reason.path[i + 1]}`);
  }
  return { nodes: new Set(reason.path), edges };
}
