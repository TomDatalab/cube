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
const COLUMNS: AccessNodeKind[] = ['user', 'organization', 'rule', 'role', 'workspace', 'connection', 'cube'];
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
  /** A parent with more than COLLAPSE_AT children (an organization's workspaces, a connection's cubes): it has a toggle. */
  foldable: boolean;
  /** Children hidden while it is collapsed. */
  hiddenCount?: number;
  /** A connection or cube without access: drawn faded. */
  dim: boolean;
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
 * Parent → children where the parent folds: an organization's workspaces
 * (a workspace's organizationId is the bare UUID of the organization node)
 * and a connection's cubes (its `connection → cube` edges). `foldables` keeps
 * the parents with more than COLLAPSE_AT children.
 */
function childrenOf(graph: AccessGraph): Map<string, string[]> {
  const children = new Map<string, string[]>();
  const add = (parent: string, child: string) => children.set(parent, [...(children.get(parent) || []), child]);
  const kindOf = new Map(graph.nodes.map((n) => [n.id, n.kind]));
  graph.nodes.forEach((n) => {
    if (n.kind === 'workspace' && n.organizationId) {
      add(`organization:${n.organizationId}`, n.id);
    }
  });
  graph.edges.forEach((e) => {
    if (kindOf.get(e.from) === 'connection' && kindOf.get(e.to) === 'cube') {
      add(e.from, e.to);
    }
  });
  return new Map([...children].map(([k, c]) => [k, [...new Set(c)]]));
}

function foldables(children: Map<string, string[]>): Map<string, string[]> {
  return new Map([...children].filter(([, c]) => c.length > COLLAPSE_AT));
}

/** Child → every parent it hangs from, foldable or not. */
function parentsOf(folds: Map<string, string[]>): Map<string, string[]> {
  const parents = new Map<string, string[]>();
  folds.forEach((cs, p) => cs.forEach((c) => parents.set(c, [...(parents.get(c) || []), p])));
  return parents;
}

/**
 * `expanded` holds the ids of the parents to open (`organization:<uuid>`,
 * `connection:<id>`); a parent with more than COLLAPSE_AT children is
 * collapsed unless its id is in the set. A child with several parents is
 * hidden only if all of them are collapsed.
 */
export function layout(
  graph: AccessGraph,
  expanded: Set<string>
): { nodes: LaidOutNode[]; edges: LaidOutEdge[] } {
  const perms = new Map<string, string[]>();
  graph.grants.forEach((g) => {
    perms.set(g.resource, [...(perms.get(g.resource) || []), g.permission]);
  });

  const children = childrenOf(graph);
  const folds = foldables(children);
  const parents = parentsOf(children);
  const collapsed = new Set([...folds.keys()].filter((k) => !expanded.has(k)));

  // Hidden child → the collapsed parent it is folded into.
  const hidden = new Map<string, string>();
  parents.forEach((ps, child) => {
    if (ps.every((p) => collapsed.has(p))) {
      hidden.set(child, ps[0]);
    }
  });

  const nextY: Record<AccessNodeKind, number> = {
    user: PAD,
    organization: PAD,
    rule: PAD,
    role: PAD,
    workspace: PAD,
    connection: PAD,
    cube: PAD,
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
    if (n.kind === 'role') {
      permList = n.details || [];
    }
    const rows = {
      permLines: wrap(permList, ' · ', PERM_CHARS),
      noteLines: n.notes.flatMap((note) => wrap(note.split(' '), ' ', NOTE_CHARS)),
      foldable: folds.has(n.id),
    };
    const { h } = nodeRows(rows);
    const laid: LaidOutNode = {
      ...n,
      ...rows,
      x: columnX.get(n.kind) ?? PAD,
      y: nextY[n.kind],
      h,
      label: shorten(n.name, NAME_CHARS),
      dim: (n.kind === 'connection' || n.kind === 'cube') && !perms.has(n.id),
    };
    nextY[n.kind] += h + GAP;
    if (collapsed.has(n.id)) {
      // A child still drawn under another, open parent is not hidden here.
      laid.hiddenCount = folds.get(n.id)!.filter((c) => hidden.has(c)).length;
    }
    nodes.push(laid);
  });

  // Edges into a hidden child go; edges out of one start at its parent, merged as the server merges parallel edges.
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

/** The parents (organizations, connections) to open so that every node of `path` is drawn. */
export function expandFor(
  graph: AccessGraph,
  drawn: LaidOutNode[],
  path: string[],
  expanded: Set<string>
): Set<string> {
  const shown = new Set(drawn.map((n) => n.id));
  const folds = foldables(childrenOf(graph));
  const parents = parentsOf(childrenOf(graph));
  const open = path.flatMap((id, i) => {
    const ps = (shown.has(id) ? [] : parents.get(id) || []).filter((p) => folds.has(p));
    // Prefer the parent the path itself goes through.
    const via = ps.find((p) => p === path[i - 1]) ?? ps[0];
    return via && !expanded.has(via) ? [via] : [];
  });
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
/** `points`: where the edge goes, start to end, level across every column it skips. */
export type EdgeGeometry = { edge: LaidOutEdge; d: string; points: Point[]; label: EdgeLabel | null };

/** How far a line keeps from a box it passes. */
const CLEARANCE = 6;

/**
 * The waypoints of an edge from `from` to `to`: straight to the target
 * when nothing stands between them; otherwise, for each column of boxes in
 * between, an entry and an exit at the height of the gap (between two boxes,
 * above the first or below the last) nearest to where the edge would pass.
 */
function route(from: LaidOutNode, to: LaidOutNode, obstacles: LaidOutNode[]): Point[] {
  const start = { x: from.x + NODE_W, y: from.y + from.h / 2 };
  const end = { x: to.x, y: to.y + to.h / 2 };
  if (end.x <= start.x) {
    return [start, end];
  }
  // The boxes between the two, grouped into columns (overlapping x ranges).
  const between = obstacles
    .filter((n) => n.id !== from.id && n.id !== to.id && n.x < end.x && n.x + NODE_W > start.x)
    .sort((a, b) => a.x - b.x);
  const columns: { x1: number; x2: number; boxes: LaidOutNode[] }[] = [];
  between.forEach((n) => {
    const last = columns[columns.length - 1];
    if (last && n.x < last.x2) {
      last.x2 = Math.max(last.x2, n.x + NODE_W);
      last.boxes.push(n);
    } else {
      columns.push({ x1: n.x, x2: n.x + NODE_W, boxes: [n] });
    }
  });
  const points = [start];
  columns.forEach((col) => {
    const t = ((col.x1 + col.x2) / 2 - start.x) / (end.x - start.x);
    const want = start.y + (end.y - start.y) * Math.min(1, Math.max(0, t));
    const spans = col.boxes
      .map((b) => [b.y - CLEARANCE, b.y + b.h + CLEARANCE] as const)
      .sort((a, b) => a[0] - b[0]);
    // Free heights: above everything, each gap, below everything.
    const lanes = [spans[0][0] - GAP / 2];
    let reach = spans[0][1];
    spans.slice(1).forEach(([top, bottom]) => {
      if (top > reach) {
        lanes.push((reach + top) / 2);
      }
      reach = Math.max(reach, bottom);
    });
    lanes.push(reach + GAP / 2);
    const y = lanes.reduce((best, lane) => (Math.abs(lane - want) < Math.abs(best - want) ? lane : best));
    points.push({ x: col.x1 - CLEARANCE, y }, { x: col.x2 + CLEARANCE, y });
  });
  points.push(end);
  return points;
}

/** An S-curve leaving `a` and reaching `b` level. */
function curve(a: Point, b: Point): string {
  const mx = (a.x + b.x) / 2;
  return ` C${mx},${a.y} ${mx},${b.y} ${b.x},${b.y}`;
}

function cubic(t: number, a: number, b: number, c: number, d: number) {
  const u = 1 - t;
  return u * u * u * a + 3 * u * u * t * b + 3 * u * t * t * c + t * t * t * d;
}

/**
 * The path of each edge between its nodes (wherever they are), and its label:
 * on the curve just before the target, pushed down off any label it would
 * cover. Edges go around the boxes of `obstacles` (the nodes by default);
 * while a node is dragged the caller passes the nodes as they were when the
 * drag began, so edges that only pass the dragged box keep their path.
 */
export function edgeGeometry(
  nodes: LaidOutNode[],
  edges: LaidOutEdge[],
  obstacles: LaidOutNode[] = nodes
): EdgeGeometry[] {
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const out: EdgeGeometry[] = [];
  edges.forEach((edge) => {
    const from = byId.get(edge.from);
    const to = byId.get(edge.to);
    if (!from || !to) {
      return;
    }
    const points = route(from, to, obstacles);
    // Curves between columns, level lines across the columns it skips.
    let d = `M${points[0].x},${points[0].y}`;
    for (let i = 1; i < points.length; i++) {
      d += i % 2 === 0 ? ` L${points[i].x},${points[i].y}` : curve(points[i - 1], points[i]);
    }
    // The label goes on the last curve, just before the target.
    const { x: x1, y: y1 } = points[points.length - 2];
    const { x: x2, y: y2 } = points[points.length - 1];
    const dx = (x2 - x1) * 0.5;
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
    out.push({ edge, d, points, label });
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
/** How far each dragged node was moved from where the layout puts it. */
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

/**
 * Dragged nodes moved by their offset from where the layout puts them, the
 * others where it puts them. An offset, not a place: when expanding or
 * collapsing moves a node along its column, a node dragged before moves with
 * it rather than staying behind on top of the nodes that moved.
 */
export function applyPositions(nodes: LaidOutNode[], positions: Positions): LaidOutNode[] {
  return nodes.map((n) => {
    const offset = positions[n.id];
    return offset ? { ...n, x: n.x + offset.x, y: n.y + offset.y } : n;
  });
}

/** The nodes in drawing order: the one being dragged last, on top. */
export function drawOrder(nodes: LaidOutNode[], dragged: string | undefined): LaidOutNode[] {
  const top = dragged ? nodes.find((n) => n.id === dragged) : undefined;
  return top ? [...nodes.filter((n) => n !== top), top] : nodes;
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
