import { memo } from 'react';
import { Handle, NodeResizer, Position } from '@xyflow/react';
import type { Node, NodeProps } from '@xyflow/react';
import { Box } from 'lucide-react';

import { cn } from '@/lib/utils';

import { cubeReads } from './cubes';
import type { CubeSourceEdgeData, CubeSourceFlowEdge } from './CubeJoinEdge';
import type { CubeNode as CubeModel, ModelDefinition } from './modelGraph';
import { LINK_HANDLE_STYLE, NODE_HANDLE_STYLE } from './nodeHandle';
import { paint } from './paint';

export type CubeNodeData = {
  // Someone else edits the model: no resizer.
  readOnly?: boolean;
  cube: CubeModel;
  // The names shown: the cube's own, or the generated ones in auto mode.
  dimensions: string[];
  measures: string[];
  issues: number;
  // Sized by hand: the box fills the node's fixed size.
  fixedSize?: boolean;
  onResize: (nodeId: string, box: { x: number; y: number; width: number; height: number }) => void;
};

export type CubeFlowNode = Node<CubeNodeData, 'cube'>;

// Same as a table's links.
const HANDLE_STYLE = LINK_HANDLE_STYLE;

type CubeNodeProps = Pick<NodeProps<CubeFlowNode>, 'data'> & {
  id?: string;
  selected?: boolean;
  // Set by React Flow while a resizer is dragged, and for a fixed size.
  width?: number;
};

function Section({ title, names, auto }: { title: string; names: string[]; auto: boolean }) {
  return (
    <div data-slot={`model-cube-${title.toLowerCase()}`}>
      <div className="tw:px-1.5 tw:py-0.5 tw:text-[10px] tw:font-semibold tw:uppercase tw:text-foreground/60">
        {title}
      </div>
      <ul className="tw:m-0 tw:min-h-0 tw:list-none tw:p-0 tw:pb-0.5">
        {names.map((n) => (
          <li
            key={n}
            data-member={n}
            data-auto={auto || undefined}
            title={auto ? 'generated at build' : undefined}
            className={cn('tw:truncate tw:px-1.5 tw:leading-4', auto && 'tw:italic')}
          >
            {n}
          </li>
        ))}
      </ul>
    </div>
  );
}

// A cube on the canvas: the table box, a header of its color with the Box
// icon and the name, then its dimensions and its measures. An auto cube
// (no members of its own) shows the generated names in italic.
export const CubeNode = memo(({
  id, data, selected, width,
}: CubeNodeProps) => {
  const { cube } = data;
  const auto = cube.dimensions.length === 0 && cube.measures.length === 0;
  return (
    <div
      data-slot="model-cube-node"
      data-selected={selected || undefined}
      className={cn(
        'tw:flex tw:min-w-40 tw:flex-col tw:border tw:border-foreground/70 tw:bg-card tw:text-xs tw:text-card-foreground',
        // The content follows the frame while it is dragged, not on release.
        // Rows that do not fit are cut off below; the link dots still stick out
        // at the sides.
        (data.fixedSize || width !== undefined) && 'tw:h-full tw:w-full tw:overflow-x-visible tw:overflow-y-clip',
        selected && 'tw:ring-2 tw:ring-ring'
      )}
    >
      {data.readOnly ? null : (
        <NodeResizer
          isVisible={Boolean(selected)}
          minWidth={160}
          minHeight={28}
          color="var(--ring)"
          onResizeEnd={(_event, box) => data.onResize(id ?? `cube:${cube.id}`, box)}
        />
      )}
      <Handle type="target" position={Position.Top} id="node" style={NODE_HANDLE_STYLE} isConnectable={false} />
      <Handle type="source" position={Position.Top} id="node" style={NODE_HANDLE_STYLE} isConnectable={false} />
      <div
        data-slot="model-cube-band"
        className="tw:relative tw:flex tw:items-center tw:gap-1 tw:border-b tw:border-foreground/70 tw:px-1.5 tw:py-1"
        style={{ background: paint(cube.color ?? 'violet', cube.colorTransparency) }}
      >
        <Box aria-hidden className="tw:size-3 tw:shrink-0" />
        <div data-slot="model-cube-name" className="tw:min-w-0 tw:flex-1 tw:truncate tw:text-center tw:font-bold">
          {cube.name}
        </div>
        {data.issues > 0 ? (
          <span
            data-slot="model-cube-issues"
            aria-label={`${data.issues} ${data.issues === 1 ? 'issue' : 'issues'}`}
            className="tw:text-[10px] tw:text-destructive"
          >
            {data.issues}
          </span>
        ) : null}
        <Handle type="target" position={Position.Left} id="join" style={HANDLE_STYLE} />
        <Handle type="source" position={Position.Right} id="join" style={HANDLE_STYLE} />
      </div>
      <Section title="Dimensions" names={data.dimensions} auto={auto} />
      <Section title="Measures" names={data.measures} auto={auto} />
    </div>
  );
});
CubeNode.displayName = 'CubeNode';

// The edge id of a cube's line to a table, kept in `selectedEdge`.
export const CUBE_SOURCE = 'cubesrc:';
export const cubeSourceEdgeId = (cubeId: string, alias: string) => `${CUBE_SOURCE}${cubeId}:${alias}`;

type SourceLineHandlers = Pick<CubeSourceEdgeData, 'onSetPoints' | 'onSetSide'>;
const NO_HANDLERS: SourceLineHandlers = { onSetPoints: () => undefined, onSetSide: () => undefined };

// The dashed lines from a cube to each table it reads: one per alias, from
// the cube's header to the table's, reshaped like references.
export function cubeSourceEdges(
  def: ModelDefinition,
  selectedEdge: string | null = null,
  handlers: SourceLineHandlers = NO_HANDLERS
): CubeSourceFlowEdge[] {
  return (def.cubes ?? []).flatMap((c) => cubeReads(def, c)
    .filter((alias) => def.tables.some((t) => t.alias === alias))
    .map((alias) => ({
      id: cubeSourceEdgeId(c.id, alias),
      source: `cube:${c.id}`,
      sourceHandle: 'join',
      target: alias,
      targetHandle: 'band',
      type: 'cubesource' as const,
      selected: selectedEdge === cubeSourceEdgeId(c.id, alias),
      data: {
        cubeId: c.id, table: alias, line: c.sourceLines?.find((l) => l.table === alias), ...handlers,
      },
    })));
}
