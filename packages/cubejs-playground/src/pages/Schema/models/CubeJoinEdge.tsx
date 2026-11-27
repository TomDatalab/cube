import { memo } from 'react';
import type { Edge, EdgeProps } from '@xyflow/react';

import type { CubeJoin, ModelJoin, Point } from './modelGraph';
import { REL_LABEL } from './modelGraph';
import { ReferenceLine } from './ReferenceEdge';
import { endMarks } from './referencePath';

export type CubeJoinEdgeData = {
  join: CubeJoin;
  onMovePoint: (id: string, pointIndex: number, to: Point) => void;
};
export type CubeJoinFlowEdge = Edge<CubeJoinEdgeData, 'cubejoin'>;

// A cube join is drawn as a table reference: the same crow's feet and square
// corners. Cube joins are inner joins; the label is the relationship.
export const CubeJoinEdge = memo((props: EdgeProps<CubeJoinFlowEdge>) => {
  const { data } = props;
  if (!data) {
    return null;
  }
  const { join } = data;
  const asReference = {
    from: { alias: join.from, column: '' },
    to: { alias: join.to, column: '' },
    relationship: join.relationship,
    join: 'inner',
  } as unknown as ModelJoin;
  return (
    <ReferenceLine
      id={props.id}
      source={props.source}
      target={props.target}
      sourceY={props.sourceY}
      targetY={props.targetY}
      selected={props.selected}
      points={join.points}
      swapped={false}
      ends={endMarks(asReference)}
      label={REL_LABEL[join.relationship]}
      onMovePoint={(pointIndex, to) => data.onMovePoint(join.id, pointIndex, to)}
    />
  );
});
CubeJoinEdge.displayName = 'CubeJoinEdge';
