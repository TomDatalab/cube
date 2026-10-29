import { AccessEdge, AccessGraph, AccessNode, AccessNodeKind, Reason } from './types';

export const COLLAPSE_AT = 8;
export const NODE_W = 220;

// Characters per line, so text stays inside NODE_W (16 px left, 12 px right):
// permissions in 11.5 px monospace (~6.9 px a character), names in 14 px
// bold and notes in 10.5 px italic. The node also clips its text.
export const PERM_CHARS = 27;
export const NAME_CHARS = 20;
export const NOTE_CHARS = 30;

const GAP = 22;
const PAD = 20;
const COLUMNS: AccessNodeKind[] = ['user', 'organization', 'workspace', 'connection'];
// NODE_W plus a 160 px gutter for the edge labels.
const COLUMN_STEP = 380;

const PLATFORM_ADMIN = 'platform admin: full access';

export type LaidOutNode = AccessNode & {
  x: number;
  y: number;
  /** Height of the box, from its rows. */
  h: number;
  /** The name, shortened to NAME_CHARS. */
  label: string;
  permLines: string[];
  noteLines: string[];
  /** An organization with more than COLLAPSE_AT workspaces: it has a toggle. */
  foldable: boolean;
  /** Workspaces hidden while it is collapsed. */
  hiddenCount?: number;
};

export type LaidOutEdge = AccessEdge & {
  /** False for every edge but the first of a fan-out with one label. */
  showLabel: boolean;
};

function shorten(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}

/** Joins `tokens` with `sep` into lines of at most `max` characters. */
function wrap(tokens: string[], sep: string, max: number): string[] {
  const lines: string[] = [];
  tokens.forEach((token) => {
    const t = shorten(token, max);
    const last = lines[lines.length - 1];
    if (last != null && last.length + sep.length + t.length <= max) {
      lines[lines.length - 1] = `${last}${sep}${t}`;
    } else {
      lines.push(t);
    }
  });
  return lines;
}

/**
 * Baselines (and the pill's top) of a node's rows, relative to its top: the
 * kind, the name, the permission lines, the note lines and the toggle.
 */
export function nodeRows(n: Pick<LaidOutNode, 'permLines' | 'noteLines' | 'foldable'>) {
  let cursor = 44;
  const permY = n.permLines.map(() => {
    cursor += 15;
    return cursor - 3;
  });
  const noteY = n.noteLines.map(() => {
    cursor += 14;
    return cursor - 3;
  });
  let pillY: number | null = null;
  if (n.foldable) {
    pillY = cursor + 4;
    cursor += 26;
  }
  return {
    kindY: 18, nameY: 38, permY, noteY, pillY, h: cursor + 8,
  };
}

/**
 * `expanded` holds organization node ids (`organization:<uuid>`); an
 * organization with more than COLLAPSE_AT workspaces is collapsed unless its
 * id is in the set.
 */
export function layout(
  graph: AccessGraph,
  expanded: Set<string>
): { nodes: LaidOutNode[]; edges: LaidOutEdge[] } {
  const perms = new Map<string, string[]>();
  graph.grants.forEach((g) => {
    perms.set(g.resource, [...(perms.get(g.resource) || []), g.permission]);
  });

  // A workspace's organizationId is the bare UUID of the organization node.
  const orgOf = (n: AccessNode) => (n.kind === 'workspace' && n.organizationId ? `organization:${n.organizationId}` : null);
  const workspaceCount = new Map<string, number>();
  graph.nodes.forEach((n) => {
    const org = orgOf(n);
    if (org) {
      workspaceCount.set(org, (workspaceCount.get(org) || 0) + 1);
    }
  });
  const foldable = new Set([...workspaceCount].filter(([, c]) => c > COLLAPSE_AT).map(([k]) => k));
  const collapsed = new Set([...foldable].filter((k) => !expanded.has(k)));

  // Hidden workspace → its organization node.
  const hidden = new Map<string, string>();
  graph.nodes.forEach((n) => {
    const org = orgOf(n);
    if (org && collapsed.has(org)) {
      hidden.set(n.id, org);
    }
  });

  const nextY: Record<AccessNodeKind, number> = {
    user: PAD,
    organization: PAD,
    workspace: PAD,
    connection: PAD,
  };
  // Columns left to right; one with no node (all workspaces hidden, say)
  // takes no room.
  const shownKinds = new Set(graph.nodes.filter((n) => !hidden.has(n.id)).map((n) => n.kind));
  const columnX = new Map(COLUMNS.filter((k) => shownKinds.has(k)).map((k, i) => [k, PAD + i * COLUMN_STEP]));
  const nodes: LaidOutNode[] = [];
  graph.nodes.forEach((n) => {
    if (hidden.has(n.id)) {
      return;
    }
    let permList = perms.get(n.id) || [];
    if (graph.platformAdmin && n.kind === 'user') {
      permList = [PLATFORM_ADMIN];
    }
    const rows = {
      permLines: wrap(permList, ' · ', PERM_CHARS),
      noteLines: n.notes.flatMap((note) => wrap(note.split(' '), ' ', NOTE_CHARS)),
      foldable: foldable.has(n.id),
    };
    const { h } = nodeRows(rows);
    const laid: LaidOutNode = {
      ...n,
      ...rows,
      x: columnX.get(n.kind) ?? PAD,
      y: nextY[n.kind],
      h,
      label: shorten(n.name, NAME_CHARS),
    };
    nextY[n.kind] += h + GAP;
    if (collapsed.has(n.id)) {
      laid.hiddenCount = workspaceCount.get(n.id);
    }
    nodes.push(laid);
  });

  // Edges into a hidden workspace go; edges out of one start at its
  // organization, merged as the server merges parallel edges.
  const edges: LaidOutEdge[] = [];
  const byEnds = new Map<string, LaidOutEdge>();
  graph.edges.forEach((e) => {
    if (hidden.has(e.to)) {
      return;
    }
    const from = hidden.get(e.from) ?? e.from;
    const key = `${from}>${e.to}`;
    const seen = byEnds.get(key);
    if (seen) {
      if (!seen.label.split(' · ').includes(e.label)) {
        seen.label = `${seen.label} · ${e.label}`;
      }
      if (e.kind === 'direct') {
        seen.kind = 'direct';
      }
      return;
    }
    const edge: LaidOutEdge = { ...e, from, showLabel: true };
    byEnds.set(key, edge);
    edges.push(edge);
  });
  const labelled = new Set<string>();
  edges.forEach((e) => {
    const key = `${e.from}>${e.label}`;
    e.showLabel = !labelled.has(key);
    labelled.add(key);
  });

  return { nodes, edges };
}

/** The organizations to open so that every node of `path` is drawn. */
export function expandFor(
  graph: AccessGraph,
  drawn: LaidOutNode[],
  path: string[],
  expanded: Set<string>
): Set<string> {
  const shown = new Set(drawn.map((n) => n.id));
  const open = path
    .filter((id) => !shown.has(id))
    .map((id) => graph.nodes.find((n) => n.id === id))
    .filter((n): n is AccessNode => n?.kind === 'workspace' && n.organizationId != null)
    .map((n) => `organization:${n.organizationId}`)
    .filter((id) => !expanded.has(id));
  return open.length ? new Set([...expanded, ...open]) : expanded;
}

export function highlight(reason: Reason): { nodes: Set<string>; edges: Set<string> } {
  const edges = new Set<string>();
  for (let i = 0; i + 1 < reason.path.length; i++) {
    edges.add(`${reason.path[i]}>${reason.path[i + 1]}`);
  }
  return { nodes: new Set(reason.path), edges };
}

// ---- edges ----

const LABEL_H = 18;
const LABEL_GAP = 8;

export type EdgeLabel = { x: number; y: number; w: number };
export type EdgeGeometry = { edge: LaidOutEdge; d: string; label: EdgeLabel | null };

function cubic(t: number, a: number, b: number, c: number, d: number) {
  const u = 1 - t;
  return u * u * u * a + 3 * u * u * t * b + 3 * u * t * t * c + t * t * t * d;
}

/**
 * The path of each edge between its nodes (wherever they are), and its label:
 * on the curve just before the target, pushed down off any label it would
 * cover.
 */
export function edgeGeometry(nodes: LaidOutNode[], edges: LaidOutEdge[]): EdgeGeometry[] {
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const out: EdgeGeometry[] = [];
  edges.forEach((edge) => {
    const from = byId.get(edge.from);
    const to = byId.get(edge.to);
    if (!from || !to) {
      return;
    }
    const x1 = from.x + NODE_W;
    const y1 = from.y + from.h / 2;
    const x2 = to.x;
    const y2 = to.y + to.h / 2;
    const dx = (x2 - x1) * 0.5;
    const d = `M${x1},${y1} C${x1 + dx},${y1} ${x2 - dx},${y2} ${x2},${y2}`;
    let label: EdgeLabel | null = null;
    if (edge.showLabel) {
      const w = edge.label.length * 6.4 + 12;
      let t = 0.5;
      const lx = x2 - LABEL_GAP - w / 2;
      if (lx - w / 2 >= x1 + LABEL_GAP) {
        // x(t) grows with t here: find where it reaches lx.
        let lo = 0;
        let hi = 1;
        for (let i = 0; i < 30; i++) {
          t = (lo + hi) / 2;
          if (cubic(t, x1, x1 + dx, x2 - dx, x2) < lx) {
            lo = t;
          } else {
            hi = t;
          }
        }
      }
      label = { x: cubic(t, x1, x1 + dx, x2 - dx, x2), y: cubic(t, y1, y1, y2, y2), w };
    }
    out.push({ edge, d, label });
  });

  const labels = out.flatMap((g) => (g.label ? [g.label] : [])).sort((a, b) => a.y - b.y);
  const placed: EdgeLabel[] = [];
  labels.forEach((l) => {
    for (;;) {
      const hit = placed.find((p) => Math.abs(p.x - l.x) < (p.w + l.w) / 2 + 4 && Math.abs(p.y - l.y) < LABEL_H + 2);
      if (!hit) {
        break;
      }
      l.y = hit.y + LABEL_H + 2;
    }
    placed.push(l);
  });
  return out;
}

// ---- dragging, zoom and pan ----

export type Point = { x: number; y: number };
export type Positions = Record<string, Point>;
/** Screen = world * k + (x, y). */
export type View = { x: number; y: number; k: number };
export type Bounds = { minX: number; minY: number; maxX: number; maxY: number };

export const DRAG_THRESHOLD = 4;
export const MIN_SCALE = 0.25;
export const MAX_SCALE = 3;

/** A pointer that moved this far is dragging, not clicking. */
export function movedBeyond(dx: number, dy: number): boolean {
  return Math.hypot(dx, dy) >= DRAG_THRESHOLD;
}

/** Nodes at their dragged position, if any; the others where layout put them. */
export function applyPositions(nodes: LaidOutNode[], positions: Positions): LaidOutNode[] {
  return nodes.map((n) => (positions[n.id] ? { ...n, ...positions[n.id] } : n));
}

export function bounds(nodes: LaidOutNode[], geometry: EdgeGeometry[]): Bounds {
  const b = {
    minX: Infinity, minY: Infinity, maxX: -Infinity, maxY: -Infinity,
  };
  const add = (x1: number, y1: number, x2: number, y2: number) => {
    b.minX = Math.min(b.minX, x1);
    b.minY = Math.min(b.minY, y1);
    b.maxX = Math.max(b.maxX, x2);
    b.maxY = Math.max(b.maxY, y2);
  };
  nodes.forEach((n) => add(n.x, n.y, n.x + NODE_W, n.y + n.h));
  geometry.forEach(({ label: l }) => l && add(l.x - l.w / 2, l.y - LABEL_H / 2, l.x + l.w / 2, l.y + LABEL_H / 2));
  return b.minX === Infinity ? {
    minX: 0, minY: 0, maxX: 0, maxY: 0,
  } : b;
}

export function clampScale(k: number): number {
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, k));
}

export function toWorld(view: View, sx: number, sy: number): Point {
  return { x: (sx - view.x) / view.k, y: (sy - view.y) / view.k };
}

/** Scales by `factor` around the screen point (sx, sy), which stays put. */
export function zoomAt(view: View, factor: number, sx: number, sy: number): View {
  const k = clampScale(view.k * factor);
  const p = toWorld(view, sx, sy);
  return { x: sx - p.x * k, y: sy - p.y * k, k };
}

/** The view that shows all of `b` in a `width` × `height` viewport, at most 1:1. */
export function fitView(b: Bounds, width: number, height: number, pad = 20): View {
  const bw = Math.max(1, b.maxX - b.minX);
  const bh = Math.max(1, b.maxY - b.minY);
  const k = clampScale(Math.min(1, (width - 2 * pad) / bw, (height - 2 * pad) / bh));
  return { x: (width - bw * k) / 2 - b.minX * k, y: (height - bh * k) / 2 - b.minY * k, k };
}
