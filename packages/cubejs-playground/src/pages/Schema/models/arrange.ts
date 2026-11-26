// The geometry of Redgate's Size, Align and Distribution menus, on boxes in
// canvas coordinates. Each returns the new place of every box it moves.

export type Box = { id: string; x: number; y: number; width: number; height: number };
type Edge = 'top' | 'middle' | 'bottom' | 'left' | 'center' | 'right';
export type AlignMode = Edge;
export type DistributeMode = 'v-justify' | 'h-justify' | Edge;
export type Places = Map<string, { x: number; y: number }>;

type Axis = {
  pos: 'x' | 'y';
  size: 'width' | 'height';
};
const H: Axis = { pos: 'x', size: 'width' };
const V: Axis = { pos: 'y', size: 'height' };

// Where on its axis a mode anchors a box: its start, middle or end.
const ANCHOR: Record<string, [Axis, 0 | 0.5 | 1]> = {
  left: [H, 0],
  center: [H, 0.5],
  right: [H, 1],
  top: [V, 0],
  middle: [V, 0.5],
  bottom: [V, 1],
};

const place = (box: Box, axis: Axis, value: number) => ({
  x: axis === H ? value : box.x,
  y: axis === V ? value : box.y,
});

export function align(boxes: Box[], mode: AlignMode): Places {
  const out: Places = new Map();
  if (boxes.length < 2) {
    return out;
  }
  const [axis, k] = ANCHOR[mode];
  const start = Math.min(...boxes.map((b) => b[axis.pos]));
  const end = Math.max(...boxes.map((b) => b[axis.pos] + b[axis.size]));
  const target = start + (end - start) * k;
  for (const b of boxes) {
    out.set(b.id, place(b, axis, target - b[axis.size] * k));
  }
  return out;
}

export function distribute(boxes: Box[], mode: DistributeMode): Places {
  const out: Places = new Map();
  if (boxes.length < 3) {
    return out;
  }
  const justify = mode === 'h-justify' || mode === 'v-justify';
  const [axis, k] = justify ? [mode === 'h-justify' ? H : V, 0] as const : ANCHOR[mode];
  const sorted = [...boxes].sort((p, q) => p[axis.pos] - q[axis.pos]);
  const first = sorted[0];
  const last = sorted[sorted.length - 1];
  if (justify) {
    const span = last[axis.pos] + last[axis.size] - first[axis.pos];
    const used = sorted.reduce((sum, b) => sum + b[axis.size], 0);
    const gap = (span - used) / (sorted.length - 1);
    let at = first[axis.pos];
    for (const b of sorted) {
      out.set(b.id, place(b, axis, at));
      at += b[axis.size] + gap;
    }
    return out;
  }
  const anchor = (b: Box) => b[axis.pos] + b[axis.size] * k;
  const step = (anchor(last) - anchor(first)) / (sorted.length - 1);
  sorted.forEach((b, i) => {
    out.set(b.id, place(b, axis, anchor(first) + step * i - b[axis.size] * k));
  });
  return out;
}

// The other boxes take the first box's width or height.
export function sameSize(
  boxes: Box[],
  kind: 'width' | 'height'
): Map<string, { width?: number; height?: number }> {
  const out = new Map<string, { width?: number; height?: number }>();
  const [first, ...rest] = boxes;
  for (const b of rest) {
    out.set(b.id, { [kind]: first[kind] });
  }
  return out;
}
