import { PointerEvent as ReactPointerEvent, memo, useState } from 'react';
import {
  BaseEdge, EdgeLabelRenderer, useInternalNode, useReactFlow,
} from '@xyflow/react';
import type { Edge, EdgeProps } from '@xyflow/react';

import type { ModelJoin, Point } from './modelGraph';
import {
  EndMark, Side, endMarks, endSides, markPath, moveCorner, moveSegment, orthogonalPath, removeCorner, routeOf,
  sideAt,
} from './referencePath';

export type ReferenceEdgeData = {
  index: number;
  join: ModelJoin;
  label: string;
  onSetPoints: (index: number, points: Point[]) => void;
  onSetSide: (index: number, end: 'from' | 'to', side: Side) => void;
};
export type ReferenceFlowEdge = Edge<ReferenceEdgeData, 'reference'>;

export type ReferenceLineProps = Pick<EdgeProps, 'id' | 'source' | 'target' | 'sourceY' | 'targetY' | 'selected'> & {
  points: Point[];
  swapped: boolean;
  // No marks: a plain line (a cube's line to its table).
  ends: { from: EndMark; to: EndMark } | null;
  label: string;
  dashed?: boolean;
  // Each end's side when it was dragged there.
  sides: { from?: Side; to?: Side };
  onSetPoints: (points: Point[]) => void;
  onSetSide: (end: 'from' | 'to', side: Side) => void;
};

// A reference as Redgate draws it: square corners through its reference
// points, a crow's foot or bar at each end, the label in the middle of the
// longest segment. Selected, every corner can be dragged (double-click one
// to remove it), every segment dragged across itself, and each end dragged
// to the other side of its table. Shared by the table references, the cube
// joins and the lines from a cube to its tables.
export const ReferenceLine = memo(({
  id, source, target, sourceY, targetY, selected, points: saved, swapped, sides, ends, label, dashed, onSetPoints,
  onSetSide,
}: ReferenceLineProps) => {
  const from = useInternalNode(source);
  const to = useInternalNode(target);
  const { screenToFlowPosition } = useReactFlow();
  // The points while a drag is under way.
  const [draft, setDraft] = useState<Point[] | null>(null);
  // An end's side while it is dragged.
  const [endDraft, setEndDraft] = useState<{ end: 'from' | 'to'; side: Side } | null>(null);
  if (!from || !to) {
    return null;
  }
  const box = (n: typeof from) => ({
    x: n.internals.positionAbsolute.x,
    width: n.measured.width ?? n.width ?? 0,
  });
  const a = box(from);
  const b = box(to);
  const fixed = endDraft ? { ...sides, [endDraft.end]: endDraft.side } : sides;
  const [sourceSide, targetSide] = endSides(a, b, swapped, fixed);
  const sourcePoint = { x: sourceSide === 'right' ? a.x + a.width : a.x, y: sourceY };
  const targetPoint = { x: targetSide === 'right' ? b.x + b.width : b.x, y: targetY };
  const points = draft ?? saved;
  const path = orthogonalPath(sourcePoint, sourceSide, targetPoint, targetSide, points);
  const route = routeOf(sourcePoint, sourceSide, targetPoint, targetSide, points);
  const stroke = selected ? 'var(--ring)' : 'var(--foreground)';
  const width = selected ? 2 : 1;

  // Drag from the route as it is now; `edit` gives the points for a pointer
  // position. Nothing is saved unless the pointer moved.
  const startDrag = (edit: (at: Point) => Point[]) => (e: ReactPointerEvent) => {
    if (e.button !== 0) {
      return;
    }
    e.stopPropagation();
    e.preventDefault();
    const x0 = e.clientX;
    const y0 = e.clientY;
    let moved = false;
    let last: Point[] | null = null;
    const at = (ev: PointerEvent) => screenToFlowPosition({ x: ev.clientX, y: ev.clientY });
    const move = (ev: PointerEvent) => {
      if (!moved && Math.abs(ev.clientX - x0) + Math.abs(ev.clientY - y0) < 3) {
        return;
      }
      moved = true;
      last = edit(at(ev));
      setDraft(last);
    };
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      setDraft(null);
      if (moved && last) {
        onSetPoints(last);
      }
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  };

  // Drag an end: it goes to the side of its table nearer the pointer.
  const startEndDrag = (end: 'from' | 'to') => (e: ReactPointerEvent) => {
    if (e.button !== 0) {
      return;
    }
    e.stopPropagation();
    e.preventDefault();
    const own = end === 'from' ? a : b;
    const was = end === 'from' ? sourceSide : targetSide;
    let side = was;
    const move = (ev: PointerEvent) => {
      side = sideAt(own, screenToFlowPosition({ x: ev.clientX, y: ev.clientY }).x);
      setEndDraft({ end, side });
    };
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      setEndDraft(null);
      if (side !== was) {
        onSetSide(end, side);
      }
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  };

  const full = [route.start, ...route.corners, route.end];
  const segments = full.slice(1).map((q, i) => [full[i], q] as const)
    .map(([p, q], i) => ({ i, p, q }))
    // A handle only where there is room to grab the segment.
    .filter(({ p, q }) => Math.abs(q.x - p.x) + Math.abs(q.y - p.y) >= 16);

  return (
    <>
      <BaseEdge
        id={id}
        path={path.d}
        style={{ stroke, strokeWidth: width, strokeDasharray: dashed ? '6 4' : undefined }}
        interactionWidth={12}
      />
      {ends ? (
        <path
          d={`${markPath(sourcePoint, sourceSide, ends.from)} ${markPath(targetPoint, targetSide, ends.to)}`}
          fill="none"
          stroke={stroke}
          strokeWidth={width}
          className="react-flow__edge-path"
        />
      ) : null}
      <EdgeLabelRenderer>
        {label ? (
          <div
            data-slot="model-reference-label"
            className="nodrag nopan tw:pointer-events-none tw:absolute tw:rounded-sm tw:bg-card tw:px-1 tw:text-[10px] tw:text-card-foreground"
            style={{ transform: `translate(-50%, -50%) translate(${path.mid.x}px, ${path.mid.y}px)` }}
          >
            {label}
          </div>
        ) : null}
        {selected ? segments.map(({ i, p, q }) => {
          const across = p.y === q.y;
          return (
            // A pointer handle; the keyboard reshapes a line with Add point and
            // Reduce point.
            // eslint-disable-next-line jsx-a11y/interactive-supports-focus
            <div
              key={`s${i}`}
              role="button"
              aria-label={`Reference segment ${i + 1}`}
              title="Drag to move this segment"
              data-slot="model-reference-segment"
              className={`nodrag nopan tw:pointer-events-auto tw:absolute tw:rounded-full tw:border tw:border-ring tw:bg-card ${
                across ? 'tw:h-1.5 tw:w-3 tw:cursor-ns-resize' : 'tw:h-3 tw:w-1.5 tw:cursor-ew-resize'
              }`}
              style={{ transform: `translate(-50%, -50%) translate(${(p.x + q.x) / 2}px, ${(p.y + q.y) / 2}px)` }}
              onPointerDown={startDrag((at) => moveSegment(route.start, route.corners, route.end, i, at))}
            />
          );
        }) : null}
        {selected ? ([['from', sourcePoint], ['to', targetPoint]] as const).map(([end, p]) => (
          // A pointer handle, like the segment ones.
          // eslint-disable-next-line jsx-a11y/interactive-supports-focus
          <div
            key={end}
            role="button"
            aria-label={end === 'from' ? 'Reference start' : 'Reference end'}
            title="Drag to the other side of the table"
            data-slot="model-reference-end"
            data-end={end}
            className="nodrag nopan tw:pointer-events-auto tw:absolute tw:size-2.5 tw:cursor-ew-resize tw:rounded-full tw:border tw:border-ring tw:bg-ring"
            style={{ transform: `translate(-50%, -50%) translate(${p.x}px, ${p.y}px)` }}
            onPointerDown={startEndDrag(end)}
          />
        )) : null}
        {selected ? route.corners.map((p, k) => (
          // A pointer handle, like the segment ones.
          // eslint-disable-next-line jsx-a11y/interactive-supports-focus
          <div
            // Corners have no identity but their order.
            // eslint-disable-next-line react/no-array-index-key
            key={`c${k}`}
            role="button"
            aria-label={`Reference point ${k + 1}`}
            title="Drag to move this corner; double-click to remove it"
            data-slot="model-reference-point"
            className="nodrag nopan tw:pointer-events-auto tw:absolute tw:size-2 tw:cursor-move tw:border tw:border-ring tw:bg-card"
            style={{ transform: `translate(-50%, -50%) translate(${p.x}px, ${p.y}px)` }}
            onPointerDown={startDrag((at) => moveCorner(route.start, route.corners, route.end, k, at))}
            onDoubleClick={(e) => {
              e.stopPropagation();
              onSetPoints(removeCorner(route.start, route.corners, route.end, k));
            }}
          />
        )) : null}
      </EdgeLabelRenderer>
    </>
  );
});
ReferenceLine.displayName = 'ReferenceLine';

export const ReferenceEdge = memo((props: EdgeProps<ReferenceFlowEdge>) => {
  const { data, source, target, ...rest } = props;
  if (!data) {
    return null;
  }
  return (
    <ReferenceLine
      id={rest.id}
      source={source}
      target={target}
      sourceY={rest.sourceY}
      targetY={rest.targetY}
      selected={rest.selected}
      points={data.join.points ?? []}
      swapped={Boolean(data.join.swapped)}
      sides={{ from: data.join.fromSide, to: data.join.toSide }}
      ends={endMarks(data.join)}
      label={data.label}
      onSetPoints={(points) => data.onSetPoints(data.index, points)}
      onSetSide={(end, side) => data.onSetSide(data.index, end, side)}
    />
  );
});
ReferenceEdge.displayName = 'ReferenceEdge';
