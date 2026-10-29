import { describe, expect, it } from 'vitest';

import { COLLAPSE_AT, NODE_H, NODE_W, highlight, layout } from '../pages/Admin/access/layout';
import { AccessGraph, AccessNode } from '../pages/Admin/access/types';

const O = 'o1';
const OID = `organization:${O}`;

function node(id: string, kind: AccessNode['kind'], organizationId: string | null = null): AccessNode {
  return { id, kind, name: id, organizationId, notes: [] };
}

function graphWith(workspaces: number): AccessGraph {
  const ws = Array.from({ length: workspaces }, (_, i) => node(`workspace:w${i}`, 'workspace', O));
  return {
    user: { id: 'ana', name: null, disabled: false },
    platformAdmin: false,
    nodes: [node('user:ana', 'user'), node(OID, 'organization'), ...ws, node('connection:c', 'connection', O)],
    edges: [
      { from: 'user:ana', to: OID, label: 'owner', kind: 'direct' },
      ...ws.map((w) => ({ from: OID, to: w.id, label: 'owns', kind: 'derived' as const })),
      { from: 'workspace:w0', to: 'connection:c', label: 'uses', kind: 'direct' },
    ],
    grants: [
      { resource: OID, permission: 'manage', reasons: [] },
      { resource: OID, permission: 'view', reasons: [] },
    ],
  };
}

describe('access layout', () => {
  it('lays nodes out in four columns', () => {
    const out = layout(graphWith(2), new Set());
    const x = (id: string) => out.nodes.find((n) => n.id === id)!.x;
    expect([x('user:ana'), x(OID), x('workspace:w0'), x('connection:c')]).toEqual([20, 260, 510, 764]);
    expect(out.nodes.find((n) => n.id === OID)!.perms).toBe('manage · view');
    expect(out.nodes.every((n) => n.x + NODE_W <= out.width && n.y + NODE_H <= out.height)).toBe(true);
  });

  it('collapses an organization with more than COLLAPSE_AT workspaces', () => {
    const g = graphWith(COLLAPSE_AT + 1);
    const collapsed = layout(g, new Set());
    expect(collapsed.nodes.some((n) => n.kind === 'workspace')).toBe(false);
    expect(collapsed.nodes.find((n) => n.id === OID)!.hiddenCount).toBe(9);
    expect(collapsed.nodes.some((n) => n.id === 'connection:c')).toBe(true);
    expect(collapsed.edges.every((e) => !e.to.startsWith('workspace:') && !e.from.startsWith('workspace:'))).toBe(true);
    const open = layout(g, new Set([OID]));
    expect(open.nodes.filter((n) => n.kind === 'workspace')).toHaveLength(9);
    expect(open.nodes.find((n) => n.id === OID)!.hiddenCount).toBeUndefined();
  });

  it('highlight follows the reason path', () => {
    const h = highlight({ text: 't', path: ['u', 'o', 'w'] });
    expect([...h.edges]).toEqual(['u>o', 'o>w']);
    expect([...h.nodes]).toEqual(['u', 'o', 'w']);
  });
});
