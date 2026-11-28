import { memo } from 'react';
import type { Edge, EdgeProps } from '@xyflow/react';

import type { CubeJoin, CubeSourceLine, ModelJoin, Point } from './modelGraph';
import { REL_LABEL } from './modelGraph';
import { ReferenceLine } from './ReferenceEdge';
import { endMarks } from './referencePath';

export type CubeJoinEdgeData = {
  join: CubeJoin;
  onSetPoints: (id: string, points: Point[]) => void;
  onSetSide: (id: string, end: 'from' | 'to', side: 'left' | 'right') => void;
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
      sides={{ from: join.fromSide, to: join.toSide }}
      ends={endMarks(asReference)}
      label={REL_LABEL[join.relationship]}
      onSetPoints={(points) => data.onSetPoints(join.id, points)}
      onSetSide={(end, side) => data.onSetSide(join.id, end, side)}
    />
  );
});
CubeJoinEdge.displayName = 'CubeJoinEdge';

export type CubeSourceEdgeData = {
  cubeId: string;
  table: string;
  line?: CubeSourceLine;
  onSetPoints: (cubeId: string, table: string, points: Point[]) => void;
  onSetSide: (cubeId: string, table: string, end: 'from' | 'to', side: 'left' | 'right') => void;
};
export type CubeSourceFlowEdge = Edge<CubeSourceEdgeData, 'cubesource'>;

// A cube's line to a table it reads: dashed and without marks, reshaped like
// a reference (corners, segments, and each end to the other side).
export const CubeSourceEdge = memo((props: EdgeProps<CubeSourceFlowEdge>) => {
  const { data } = props;
  if (!data) {
    return null;
  }
  const { cubeId, table, line } = data;
  return (
    <ReferenceLine
      id={props.id}
      source={props.source}
      target={props.target}
      sourceY={props.sourceY}
      targetY={props.targetY}
      selected={props.selected}
      points={line?.points ?? []}
      swapped={false}
      sides={{ from: line?.fromSide, to: line?.toSide }}
      ends={null}
      label=""
      dashed
      onSetPoints={(points) => data.onSetPoints(cubeId, table, points)}
      onSetSide={(end, side) => data.onSetSide(cubeId, table, end, side)}
    />
  );
});
CubeSourceEdge.displayName = 'CubeSourceEdge';
