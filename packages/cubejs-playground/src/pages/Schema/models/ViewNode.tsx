import { memo } from 'react';
import { Handle, NodeResizer, Position } from '@xyflow/react';
import type { Node, NodeProps } from '@xyflow/react';

import { cn } from '@/lib/utils';

import type { ViewColumn } from './modelGraph';

export type ViewNodeData = {
  id: string;
  name: string;
  columns: ViewColumn[];
  showTypes: boolean;
  // Sized by hand: the box fills the node's fixed size.
  fixedSize: boolean;
  onResize: (nodeId: string, box: { x: number; y: number; width: number; height: number }) => void;
};

export type ViewFlowNode = Node<ViewNodeData, 'view'>;

// The node-level handle a dependency line ends on: invisible, top centre.
// Inline style: React Flow's stylesheet is unlayered (see TableNode).
export const NODE_HANDLE_STYLE = {
  opacity: 0,
  pointerEvents: 'none' as const,
  top: 0,
  left: '50%',
};

type ViewNodeProps = Pick<NodeProps<ViewFlowNode>, 'data'> & { id?: string; selected?: boolean };

// A view on the canvas: a rounded box, its name centred in bold, then one
// row per column with its name and type, like a table.
export const ViewNode = memo(({ id, data, selected }: ViewNodeProps) => (
  <div
    data-slot="model-view-node"
    data-selected={selected || undefined}
    className={cn(
      'tw:flex tw:min-w-40 tw:flex-col tw:rounded-md tw:border tw:text-xs tw:text-foreground',
      data.fixedSize && 'tw:h-full tw:w-full tw:overflow-hidden',
      selected && 'tw:ring-2 tw:ring-ring'
    )}
    style={{ background: 'var(--model-view)', borderColor: 'var(--model-view-border)' }}
  >
    <NodeResizer
      isVisible={Boolean(selected)}
      minWidth={160}
      minHeight={28}
      color="var(--ring)"
      onResizeEnd={(_event, box) => data.onResize(id ?? `view:${data.id}`, box)}
    />
    <Handle type="target" position={Position.Top} id="node" style={NODE_HANDLE_STYLE} isConnectable={false} />
    <Handle type="source" position={Position.Top} id="node" style={NODE_HANDLE_STYLE} isConnectable={false} />
    <div
      data-slot="model-view-band"
      className="tw:px-1.5 tw:py-1 tw:text-center"
      style={{ borderBottom: data.columns.length ? '1px solid var(--model-view-border)' : undefined }}
    >
      <div data-slot="model-view-name" className="tw:truncate tw:font-bold">{data.name}</div>
    </div>
    <ul className="tw:m-0 tw:min-h-0 tw:list-none tw:p-0 tw:py-0.5">
      {data.columns.map((c) => (
        <li key={c.name} data-column={c.name} className="tw:flex tw:items-center tw:gap-3 tw:px-1.5 tw:leading-4">
          <span className="tw:min-w-0 tw:flex-1 tw:truncate">{c.name}</span>
          {data.showTypes ? <span className="tw:w-20 tw:shrink-0 tw:truncate">{c.type}</span> : null}
        </li>
      ))}
    </ul>
  </div>
));
ViewNode.displayName = 'ViewNode';
