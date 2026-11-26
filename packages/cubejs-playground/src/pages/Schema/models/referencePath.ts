import type { ModelJoin, Point } from './modelGraph';

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
export function endSides(
  source: { x: number; width: number },
  target: { x: number; width: number },
  swapped: boolean
): [Side, Side] {
  const rightward = source.x + source.width / 2 <= target.x + target.width / 2;
  const [s, t]: [Side, Side] = rightward ? ['right', 'left'] : ['left', 'right'];
  return swapped ? [t, s] : [s, t];
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
