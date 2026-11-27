import type { ModelJoin, Point } from './modelGraph';
import { referenceEnds } from './physical';

// How a reference ends at one of its tables, in crow's-foot notation.
export type EndMark = { many: boolean; optional: boolean };
export type Side = 'left' | 'right';
export type ReferencePath = {
  // The SVG path.
  d: string;
  // The middle of the longest segment: where the label goes.
  mid: Point;
  segments: [Point, Point][];
  // How many reference points come before each segment along the line.
  slots: number[];
};

// The line leaves and enters its tables straight for this long.
export const STUB = 20;

export function endMarks(j: ModelJoin): { from: EndMark; to: EndMark } {
  if (j.cardinality) {
    const child = { many: j.cardinality.endsWith('*'), optional: j.cardinality.startsWith('0') };
    const parent = { many: false, optional: !(j.mandatory ?? j.join === 'inner') };
    return referenceEnds(j).child === j.from ? { from: child, to: parent } : { from: parent, to: child };
  }
  const oneOptional = j.join === 'left';
  if (j.relationship === 'many_to_one') {
    return { from: { many: true, optional: true }, to: { many: false, optional: oneOptional } };
  }
  if (j.relationship === 'one_to_many') {
    return { from: { many: false, optional: oneOptional }, to: { many: true, optional: true } };
  }
  return { from: { many: false, optional: oneOptional }, to: { many: false, optional: true } };
}

const out = (side: Side) => (side === 'right' ? 1 : -1);

// The sides a reference leaves its two tables from: towards the other
// table, or the other way when it is swapped.
// An end dragged to a side stays there.
export function endSides(
  source: { x: number; width: number },
  target: { x: number; width: number },
  swapped: boolean,
  fixed: { from?: Side; to?: Side } = {}
): [Side, Side] {
  const rightward = source.x + source.width / 2 <= target.x + target.width / 2;
  const [s, t]: [Side, Side] = rightward ? ['right', 'left'] : ['left', 'right'];
  const [a, b] = swapped ? [t, s] : [s, t];
  return [fixed.from ?? a, fixed.to ?? b];
}

// The side of a box nearer to a pointer at `x`.
export function sideAt(box: { x: number; width: number }, x: number): Side {
  return x < box.x + box.width / 2 ? 'left' : 'right';
}

const flip = (side?: Side): Side | undefined => (side && (side === 'left' ? 'right' : 'left'));

// What Swap does to a line's dragged ends: each goes to its other side.
export function swappedSides(fixed: { fromSide?: Side; toSide?: Side }): { fromSide?: Side; toSide?: Side } {
  return { fromSide: flip(fixed.fromSide), toSide: flip(fixed.toSide) };
}
const same = (p: Point, q: Point) => p.x === q.x && p.y === q.y;

export function orthogonalPath(
  source: Point,
  sourceSide: Side,
  target: Point,
  targetSide: Side,
  points: Point[] = []
): ReferencePath {
  const s1 = { x: source.x + out(sourceSide) * STUB, y: source.y };
  const t1 = { x: target.x + out(targetSide) * STUB, y: target.y };
  // The corners of the line, each with the count of reference points
  // already passed.
  const corners: { p: Point; slot: number }[] = [{ p: source, slot: 0 }, { p: s1, slot: 0 }];
  const stops = [...points, t1];
  stops.forEach((q, i) => {
    const { p } = corners[corners.length - 1];
    if (p.x !== q.x && p.y !== q.y) {
      corners.push({ p: { x: q.x, y: p.y }, slot: i });
    }
    corners.push({ p: q, slot: i + 1 });
  });
  corners.push({ p: target, slot: points.length });

  const segments: [Point, Point][] = [];
  const slots: number[] = [];
  for (let i = 1; i < corners.length; i++) {
    const { p } = corners[i - 1];
    const { p: q } = corners[i];
    if (!same(p, q)) {
      segments.push([p, q]);
      slots.push(corners[i - 1].slot);
    }
  }
  const length = ([p, q]: [Point, Point]) => Math.abs(q.x - p.x) + Math.abs(q.y - p.y);
  let longest = 0;
  segments.forEach((seg, i) => {
    if (length(seg) > length(segments[longest])) {
      longest = i;
    }
  });
  const [a, b] = segments[longest] ?? [source, target];
  const d = [`M ${source.x} ${source.y}`, ...segments.map(([, q]) => `L ${q.x} ${q.y}`)].join(' ');
  return {
    d, mid: { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 }, segments, slots,
  };
}

// A new reference point in the middle of the longest segment.
export function addPoint(points: Point[], path: ReferencePath): Point[] {
  const length = ([p, q]: [Point, Point]) => Math.abs(q.x - p.x) + Math.abs(q.y - p.y);
  let longest = 0;
  path.segments.forEach((seg, i) => {
    if (length(seg) > length(path.segments[longest])) {
      longest = i;
    }
  });
  const at = path.slots[longest] ?? points.length;
  const next = [...points];
  next.splice(at, 0, { x: Math.round(path.mid.x), y: Math.round(path.mid.y) });
  return next;
}

// Where an edited line turns. `start` and `end` are the ends of its stubs,
// which stay put; `corners` are every turn in between, the ones the line
// made by itself included, so each can be dragged.
export type Route = { start: Point; corners: Point[]; end: Point };

export function routeOf(
  source: Point,
  sourceSide: Side,
  target: Point,
  targetSide: Side,
  points: Point[] = []
): Route {
  const path = orthogonalPath(source, sourceSide, target, targetSide, points);
  const vertices = [source, ...path.segments.map(([, q]) => q)];
  if (vertices.length < 4) {
    return { start: vertices[1] ?? source, corners: [], end: vertices[1] ?? target };
  }
  return { start: vertices[1], corners: vertices.slice(2, -2), end: vertices[vertices.length - 2] };
}

const aligned = (p: Point, q: Point) => p.x === q.x || p.y === q.y;

// The reference points for a run of turns: repeated points and turns in the
// middle of a straight run are dropped.
function simplify(full: Point[]): Point[] {
  const kept: Point[] = [];
  full.forEach((p) => {
    if (!kept.length || !same(kept[kept.length - 1], p)) {
      kept.push(p);
    }
  });
  for (let i = kept.length - 2; i >= 1; i--) {
    const [a, b, c] = [kept[i - 1], kept[i], kept[i + 1]];
    if ((a.x === b.x && b.x === c.x) || (a.y === b.y && b.y === c.y)) {
      kept.splice(i, 1);
    }
  }
  return kept.slice(1, -1);
}

const round = (p: Point) => ({ x: Math.round(p.x), y: Math.round(p.y) });

// Drag corner `k` to `to`. The segments on either side keep their direction:
// a new turn is added where they no longer meet square.
export function moveCorner(start: Point, corners: Point[], end: Point, k: number, to: Point): Point[] {
  const full = [start, ...corners, end];
  const i = k + 1;
  const c = full[i];
  const moved = round(to);
  const prev = full[i - 1];
  const next = full[i + 1];
  const before: Point[] = [];
  const after: Point[] = [];
  if (!aligned(prev, moved)) {
    // Arrive the way the segment did: across into a horizontal one.
    before.push(prev.y === c.y ? { x: prev.x, y: moved.y } : { x: moved.x, y: prev.y });
  }
  if (!aligned(moved, next)) {
    // Leave the way the segment did: down first for a vertical one.
    after.push(c.x === next.x ? { x: moved.x, y: next.y } : { x: next.x, y: moved.y });
  }
  return simplify([...full.slice(0, i), ...before, moved, ...after, ...full.slice(i + 1)]);
}

// Drag segment `i` (from turn i to turn i + 1, the stub ends counted) across
// itself to `to`: a horizontal one up or down, a vertical one sideways. An
// end that is a stub stays and gets a new turn.
export function moveSegment(start: Point, corners: Point[], end: Point, i: number, to: Point): Point[] {
  const full = [start, ...corners, end];
  const a = full[i];
  const b = full[i + 1];
  if (!a || !b) {
    return corners;
  }
  const at = round(to);
  const shift = (p: Point) => (a.y === b.y ? { x: p.x, y: at.y } : { x: at.x, y: p.y });
  const next = [...full];
  if (i + 1 === full.length - 1) {
    next.splice(i + 1, 0, shift(b));
  } else {
    next[i + 1] = shift(b);
  }
  if (i === 0) {
    next.splice(1, 0, shift(a));
  } else {
    next[i] = shift(a);
  }
  return simplify(next);
}

export function removeCorner(start: Point, corners: Point[], end: Point, k: number): Point[] {
  return simplify([start, ...corners.filter((_, i) => i !== k), end]);
}

export function reducePoint(points: Point[]): Point[] {
  return points.slice(0, -1);
}

const bar = (x: number, y: number) => `M ${x} ${y - 6} L ${x} ${y + 6}`;
const circle = (cx: number, y: number) => `M ${cx - 4} ${y} a 4 4 0 1 0 8 0 a 4 4 0 1 0 -8 0`;

// The end symbol where a reference meets a table at `at`, the line going
// out to `side`: a crow's foot for many, a bar for one, then a bar for
// mandatory or a circle for optional.
export function markPath(at: Point, side: Side, mark: EndMark): string {
  const k = out(side);
  const { x, y } = at;
  const parts: string[] = [];
  let next: number;
  if (mark.many) {
    const tip = x + k * 12;
    parts.push(`M ${tip} ${y} L ${x} ${y - 6}`, `M ${tip} ${y} L ${x} ${y}`, `M ${tip} ${y} L ${x} ${y + 6}`);
    next = 16;
  } else {
    parts.push(bar(x + k * 4, y));
    next = 10;
  }
  parts.push(mark.optional ? circle(x + k * (next + 4), y) : bar(x + k * next, y));
  return parts.join(' ');
}
