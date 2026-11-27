import { PointerEvent as ReactPointerEvent, memo, useState } from 'react';
import {
  BaseEdge, EdgeLabelRenderer, useInternalNode, useReactFlow,
} from '@xyflow/react';
import type { Edge, EdgeProps } from '@xyflow/react';

import type { ModelJoin, Point } from './modelGraph';
import {
  EndMark, endMarks, endSides, markPath, orthogonalPath,
} from './referencePath';

export type ReferenceEdgeData = {
  index: number;
  join: ModelJoin;
  label: string;
  onMovePoint: (index: number, pointIndex: number, to: Point) => void;
};
export type ReferenceFlowEdge = Edge<ReferenceEdgeData, 'reference'>;

export type ReferenceLineProps = Pick<EdgeProps, 'id' | 'source' | 'target' | 'sourceY' | 'targetY' | 'selected'> & {
  points: Point[];
  swapped: boolean;
  ends: { from: EndMark; to: EndMark };
  label: string;
  onMovePoint: (pointIndex: number, to: Point) => void;
};

// A reference as Redgate draws it: square corners through its reference
// points, a crow's foot or bar at each end, the label in the middle of the
// longest segment. Selected, its points can be dragged. Shared by the table
// references and the cube joins.
export const ReferenceLine = memo(({
  id, source, target, sourceY, targetY, selected, points: saved, swapped, ends, label, onMovePoint,
}: ReferenceLineProps) => {
  const from = useInternalNode(source);
  const to = useInternalNode(target);
  const { screenToFlowPosition } = useReactFlow();
  const [dragging, setDragging] = useState<{ i: number; at: Point } | null>(null);
  if (!from || !to) {
    return null;
  }
  const box = (n: typeof from) => ({
    x: n.internals.positionAbsolute.x,
    width: n.measured.width ?? n.width ?? 0,
  });
  const a = box(from);
  const b = box(to);
  const [sourceSide, targetSide] = endSides(a, b, swapped);
  const sourcePoint = { x: sourceSide === 'right' ? a.x + a.width : a.x, y: sourceY };
  const targetPoint = { x: targetSide === 'right' ? b.x + b.width : b.x, y: targetY };
  const points = saved.map((p, i) => (dragging?.i === i ? dragging.at : p));
  const path = orthogonalPath(sourcePoint, sourceSide, targetPoint, targetSide, points);
  const stroke = selected ? 'var(--ring)' : 'var(--foreground)';
  const width = selected ? 2 : 1;

  const startDrag = (i: number) => (e: ReactPointerEvent) => {
    e.stopPropagation();
    const move = (ev: PointerEvent) => {
      const at = screenToFlowPosition({ x: ev.clientX, y: ev.clientY });
      setDragging({ i, at: { x: Math.round(at.x), y: Math.round(at.y) } });
    };
    const up = (ev: PointerEvent) => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      const at = screenToFlowPosition({ x: ev.clientX, y: ev.clientY });
      setDragging(null);
      onMovePoint(i, { x: Math.round(at.x), y: Math.round(at.y) });
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  };

  return (
    <>
      <BaseEdge id={id} path={path.d} style={{ stroke, strokeWidth: width }} interactionWidth={12} />
      <path
        d={`${markPath(sourcePoint, sourceSide, ends.from)} ${markPath(targetPoint, targetSide, ends.to)}`}
        fill="none"
        stroke={stroke}
        strokeWidth={width}
        className="react-flow__edge-path"
      />
      <EdgeLabelRenderer>
        <div
          data-slot="model-reference-label"
          className="nodrag nopan tw:pointer-events-none tw:absolute tw:rounded-sm tw:bg-card tw:px-1 tw:text-[10px] tw:text-card-foreground"
          style={{ transform: `translate(-50%, -50%) translate(${path.mid.x}px, ${path.mid.y}px)` }}
        >
          {label}
        </div>
        {selected ? points.map((p, i) => (
          <div
            // Points have no identity but their order.
            // eslint-disable-next-line react/no-array-index-key
            key={i}
            role="button"
            aria-label={`Reference point ${i + 1}`}
            data-slot="model-reference-point"
            className="nodrag nopan tw:pointer-events-auto tw:absolute tw:size-2 tw:cursor-move tw:border tw:border-ring tw:bg-card"
            style={{ transform: `translate(-50%, -50%) translate(${p.x}px, ${p.y}px)` }}
            onPointerDown={startDrag(i)}
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
      ends={endMarks(data.join)}
      label={data.label}
      onMovePoint={(pointIndex, to) => data.onMovePoint(data.index, pointIndex, to)}
    />
  );
});
ReferenceEdge.displayName = 'ReferenceEdge';
