import { describe, expect, it } from 'vitest';

import {
  applyPositions,
  bounds,
  clampScale,
  COLLAPSE_AT,
  edgeGeometry,
  expandFor,
  fitView,
  highlight,
  layout,
  movedBeyond,
  MAX_SCALE,
  MIN_SCALE,
  NAME_CHARS,
  NODE_W,
  NOTE_CHARS,
  nodeRows,
  PERM_CHARS,
  toWorld,
  zoomAt,
} from '../pages/Admin/access/layout';
import { AccessGraph, AccessNode } from '../pages/Admin/access/types';

const O = 'o1';
const OID = `organization:${O}`;
const OWNER_PERMS = ['manage', 'edit', 'view', 'query', 'execute', 'read objects', 'write objects'];

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
      ...ws.map((w) => ({ from: OID, to: w.id, label: 'admin ⇒ manage', kind: 'derived' as const })),
      { from: 'workspace:w0', to: 'connection:c', label: 'released', kind: 'derived' },
    ],
    grants: [
      { resource: OID, permission: 'manage', reasons: [] },
      { resource: OID, permission: 'view', reasons: [] },
      ...ws.flatMap((w) => OWNER_PERMS.map((permission) => ({ resource: w.id, permission, reasons: [] }))),
    ],
  };
}

function overlap(a: { y: number; h: number }, b: { y: number; h: number }) {
  return a.y < b.y + b.h && b.y < a.y + a.h;
}

describe('access layout', () => {
  it('lays nodes out in four columns', () => {
    const out = layout(graphWith(2), new Set());
    const x = (id: string) => out.nodes.find((n) => n.id === id)!.x;
    expect([x('user:ana'), x(OID), x('workspace:w0'), x('connection:c')]).toEqual([20, 400, 780, 1160]);
    expect(out.nodes.find((n) => n.id === OID)!.permLines).toEqual(['manage · view']);
  });

  it('wraps permissions to fit the node and stacks nodes without overlap', () => {
    const g = graphWith(3);
    g.nodes[2] = { ...g.nodes[2], name: 'A workspace with a really long name indeed' };
    g.nodes[g.nodes.length - 1].notes = ['assigned by a platform admin: only platform admins edit it', 'data source missing'];
    const out = layout(g, new Set());
    const w0 = out.nodes.find((n) => n.id === 'workspace:w0')!;
    expect(w0.permLines.length).toBeGreaterThan(1);
    expect(w0.permLines.join(' · ')).toBe(OWNER_PERMS.join(' · '));
    expect(w0.label.length).toBeLessThanOrEqual(NAME_CHARS);
    expect(w0.label.endsWith('…')).toBe(true);
    out.nodes.forEach((n) => {
      n.permLines.forEach((l) => expect(l.length).toBeLessThanOrEqual(PERM_CHARS));
      n.noteLines.forEach((l) => expect(l.length).toBeLessThanOrEqual(NOTE_CHARS));
      // Rows go down without overlapping and stay inside the box.
      const r = nodeRows(n);
      const ys = [r.kindY, r.nameY, ...r.permY, ...r.noteY, ...(r.pillY == null ? [] : [r.pillY + 14])];
      ys.forEach((y, i) => i && expect(y - ys[i - 1]).toBeGreaterThanOrEqual(12));
      expect(ys[ys.length - 1]).toBeLessThan(n.h);
    });
    const conn = out.nodes.find((n) => n.id === 'connection:c')!;
    expect(conn.noteLines.length).toBeGreaterThan(1);
    ['user', 'organization', 'workspace', 'connection'].forEach((kind) => {
      const column = out.nodes.filter((n) => n.kind === kind);
      column.forEach((a, i) => column.slice(i + 1).forEach((b) => expect(overlap(a, b)).toBe(false)));
    });
  });

  it('shows a platform admin as full access', () => {
    const g = graphWith(0);
    g.platformAdmin = true;
    g.nodes = [node('user:ana', 'user')];
    g.edges = [];
    g.grants = [];
    expect(layout(g, new Set()).nodes[0].permLines).toEqual(['platform admin: full access']);
  });

  it('collapses an organization with more than COLLAPSE_AT workspaces', () => {
    const g = graphWith(COLLAPSE_AT + 1);
    const collapsed = layout(g, new Set());
    const org = collapsed.nodes.find((n) => n.id === OID)!;
    expect(collapsed.nodes.some((n) => n.kind === 'workspace')).toBe(false);
    expect(org.hiddenCount).toBe(9);
    expect(org.foldable).toBe(true);
    expect(nodeRows(org).pillY).not.toBeNull();
    expect(collapsed.nodes.some((n) => n.id === 'connection:c')).toBe(true);
    expect(collapsed.edges.every((e) => !e.to.startsWith('workspace:') && !e.from.startsWith('workspace:'))).toBe(true);
    const open = layout(g, new Set([OID]));
    expect(open.nodes.filter((n) => n.kind === 'workspace')).toHaveLength(9);
    expect(open.nodes.find((n) => n.id === OID)!.hiddenCount).toBeUndefined();
    expect(open.nodes.find((n) => n.id === OID)!.foldable).toBe(true);
    expect(layout(graphWith(2), new Set()).nodes.find((n) => n.id === OID)!.foldable).toBe(false);
  });

  it('keeps a connection attached to a collapsed organization', () => {
    const g = graphWith(COLLAPSE_AT + 1);
    g.edges.push({ from: 'workspace:w1', to: 'connection:c', label: 'released', kind: 'derived' });
    g.edges.push({ from: OID, to: 'connection:c', label: 'manage', kind: 'derived' });
    const out = layout(g, new Set());
    const toConn = out.edges.filter((e) => e.to === 'connection:c');
    expect(toConn).toHaveLength(1);
    expect(toConn[0]).toMatchObject({ from: OID, label: 'released · manage' });
  });

  it('labels a fan-out once', () => {
    const out = layout(graphWith(3), new Set());
    const fan = out.edges.filter((e) => e.from === OID && e.label === 'admin ⇒ manage');
    expect(fan.map((e) => e.showLabel)).toEqual([true, false, false]);
    expect(out.edges.find((e) => e.from === 'user:ana')!.showLabel).toBe(true);
  });

  it('places labels before their target without overlapping', () => {
    const g = graphWith(3);
    g.edges.push({ from: 'user:ana', to: 'workspace:w0', label: 'editor', kind: 'direct' });
    const out = layout(g, new Set());
    const geo = edgeGeometry(out.nodes, out.edges);
    const labels = geo.flatMap((e) => (e.label ? [e.label] : []));
    expect(labels.length).toBe(4);
    labels.forEach((a, i) => labels.slice(i + 1).forEach((b) => {
      const apart = a.x + a.w / 2 <= b.x - b.w / 2 || b.x + b.w / 2 <= a.x - a.w / 2
        || Math.abs(a.y - b.y) >= 18;
      expect(apart).toBe(true);
    }));
    // Each label sits in the gutter just left of its target column.
    geo.forEach((e) => {
      if (e.label) {
        const to = out.nodes.find((n) => n.id === e.edge.to)!;
        expect(e.label.x + e.label.w / 2).toBeLessThanOrEqual(to.x);
        expect(e.label.x - e.label.w / 2).toBeGreaterThanOrEqual(to.x - 160);
      }
    });
  });

  it('opens the organizations a reason passes through', () => {
    const g = graphWith(COLLAPSE_AT + 1);
    const drawn = layout(g, new Set());
    const next = expandFor(g, drawn.nodes, ['user:ana', OID, 'workspace:w3'], new Set(['organization:x']));
    expect([...next].sort()).toEqual([OID, 'organization:x']);
    const same = new Set<string>();
    expect(expandFor(graphWith(2), layout(graphWith(2), same).nodes, ['user:ana', OID], same)).toBe(same);
  });

  it('highlight follows the reason path', () => {
    const h = highlight({ text: 't', path: ['u', 'o', 'w'] });
    expect([...h.edges]).toEqual(['u>o', 'o>w']);
    expect([...h.nodes]).toEqual(['u', 'o', 'w']);
  });
});

describe('dragging nodes', () => {
  it('tells a click from a drag', () => {
    expect(movedBeyond(2, 3)).toBe(false);
    expect(movedBeyond(3, 3)).toBe(true);
    expect(movedBeyond(-5, 0)).toBe(true);
  });

  it('moves only the dragged nodes, whatever the layout', () => {
    const out = layout(graphWith(2), new Set());
    const moved = applyPositions(out.nodes, { [OID]: { x: 5, y: 900 }, 'workspace:gone': { x: 1, y: 1 } });
    expect(moved.find((n) => n.id === OID)).toMatchObject({ x: 5, y: 900 });
    expect(moved.find((n) => n.id === 'user:ana')).toEqual(out.nodes.find((n) => n.id === 'user:ana'));
    expect(moved).toHaveLength(out.nodes.length);
  });

  it('bounds enclose dragged nodes', () => {
    const out = layout(graphWith(2), new Set());
    const moved = applyPositions(out.nodes, { [OID]: { x: -300, y: 2000 } });
    const org = moved.find((n) => n.id === OID)!;
    const b = bounds(moved, edgeGeometry(moved, out.edges));
    expect(b.minX).toBeLessThanOrEqual(-300);
    expect(b.maxY).toBeGreaterThanOrEqual(2000 + org.h);
    expect(b.maxX).toBeGreaterThanOrEqual(1160 + NODE_W);
  });
});

describe('zoom and pan', () => {
  it('clamps the scale', () => {
    expect(clampScale(0.01)).toBe(MIN_SCALE);
    expect(clampScale(10)).toBe(MAX_SCALE);
    expect(clampScale(1.5)).toBe(1.5);
  });

  it('keeps the point under the cursor fixed', () => {
    const view = { x: 30, y: -12, k: 0.8 };
    const before = toWorld(view, 200, 150);
    const next = zoomAt(view, 1.7, 200, 150);
    expect(next.k).toBeCloseTo(1.36);
    const after = toWorld(next, 200, 150);
    expect(after.x).toBeCloseTo(before.x);
    expect(after.y).toBeCloseTo(before.y);
    // At the limit the scale stops and the point still stays put.
    const capped = zoomAt({ x: 0, y: 0, k: 2.9 }, 2, 50, 60);
    expect(capped.k).toBe(MAX_SCALE);
    expect(toWorld(capped, 50, 60).x).toBeCloseTo(50 / 2.9);
  });

  it('fits the bounds into the viewport', () => {
    const b = {
      minX: 0, minY: 0, maxX: 2000, maxY: 500,
    };
    const v = fitView(b, 1000, 600, 20);
    expect(v.k).toBeCloseTo(960 / 2000);
    // Both corners land inside the viewport.
    expect(v.x + b.minX * v.k).toBeGreaterThanOrEqual(20 - 1e-9);
    expect(v.x + b.maxX * v.k).toBeLessThanOrEqual(980 + 1e-9);
    expect(v.y + b.maxY * v.k).toBeLessThanOrEqual(600);
    // A small graph is not blown up.
    expect(fitView({
      minX: 10, minY: 10, maxX: 110, maxY: 60,
    }, 1000, 600).k).toBe(1);
  });
});
