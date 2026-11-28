import { BaseEdge, getSmoothStepPath } from '@xyflow/react';
import type { Edge, EdgeProps } from '@xyflow/react';

// A view depends on a table or another view: from the view's `node` handle
// to the other's. Dashed and quiet; not selectable.
export type DependencyFlowEdge = Edge<Record<string, never>, 'dependency'>;

export function DependencyEdge({
  id, sourceX, sourceY, targetX, targetY, sourcePosition, targetPosition, markerEnd,
}: EdgeProps<DependencyFlowEdge>) {
  const [path] = getSmoothStepPath({
    sourceX, sourceY, targetX, targetY, sourcePosition, targetPosition,
  });
  return (
    <BaseEdge
      id={id}
      path={path}
      markerEnd={markerEnd}
      style={{ stroke: 'var(--muted-foreground)', strokeDasharray: '6 4' }}
    />
  );
}
