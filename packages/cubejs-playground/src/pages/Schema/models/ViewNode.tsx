import { memo } from 'react';
import { Handle, Position } from '@xyflow/react';
import type { Node, NodeProps } from '@xyflow/react';

import { cn } from '@/lib/utils';

import type { TableColor, ViewColumn } from './modelGraph';
import { ModelResizer } from './ModelResizer';
import { inkOn, paint } from './paint';
import { NODE_HANDLE_STYLE } from './nodeHandle';

export type ViewNodeData = {
  // Someone else edits the model: no resizer.
  readOnly?: boolean;
  id: string;
  name: string;
  columns: ViewColumn[];
  showTypes: boolean;
  // The whole view's color; none is the view blue.
  color?: TableColor;
  colorTransparency?: number;
  // Sized by hand: the box fills the node's fixed size.
  fixedSize: boolean;
  onResize: (nodeId: string, box: { x: number; y: number; width: number; height: number }) => void;
};

export type ViewFlowNode = Node<ViewNodeData, 'view'>;

type ViewNodeProps = Pick<NodeProps<ViewFlowNode>, 'data'> & {
  id?: string;
  selected?: boolean;
  // Set by React Flow while a resizer is dragged, and for a fixed size.
  width?: number;
};

// A view on the canvas as Redgate draws it: a table's box with round
// corners, its name centred in bold on a header, then one row per column
// with its name and type. One color fills the whole view.
export const ViewNode = memo(({
  id, data, selected, width,
}: ViewNodeProps) => {
  const line = data.color ? 'color-mix(in oklch, var(--foreground) 70%, transparent)' : 'var(--model-view-border)';
  return (
    <div
      data-slot="model-view-node"
      data-selected={selected || undefined}
      data-color={data.color}
      className={cn(
        'tw:flex tw:min-w-40 tw:flex-col tw:rounded-md tw:border tw:text-xs tw:text-foreground',
        // The content follows the frame while it is dragged, not on release.
        (data.fixedSize || width !== undefined) && 'tw:h-full tw:w-full tw:overflow-hidden',
        selected && 'tw:ring-2 tw:ring-ring'
      )}
      style={data.color
        ? {
          background: paint(data.color, data.colorTransparency),
          borderColor: line,
          color: inkOn(data.colorTransparency),
        }
        : { background: 'var(--model-view)', borderColor: line }}
    >
      {data.readOnly ? null : (
        <ModelResizer
          visible={Boolean(selected)}
          minWidth={160}
          minHeight={28}
          onResizeEnd={(_event, box) => data.onResize(id ?? `view:${data.id}`, box)}
        />
      )}
      <Handle type="target" position={Position.Top} id="node" style={NODE_HANDLE_STYLE} isConnectable={false} />
      <Handle type="source" position={Position.Top} id="node" style={NODE_HANDLE_STYLE} isConnectable={false} />
      <div
        data-slot="model-view-band"
        className="tw:px-1.5 tw:py-1 tw:text-center"
        style={{ borderBottom: data.columns.length ? `1px solid ${line}` : undefined }}
      >
        <div data-slot="model-view-name" className="tw:truncate tw:font-bold">{data.name}</div>
      </div>
      {/* One grid for all the rows, as in a table. */}
      <ul
        className="tw:m-0 tw:grid tw:min-h-0 tw:list-none tw:gap-x-3 tw:p-0 tw:py-0.5"
        style={{ gridTemplateColumns: data.showTypes ? 'minmax(0, auto) minmax(0, auto)' : 'minmax(0, auto)' }}
      >
        {data.columns.map((c, i) => (
          <li
            // Names may repeat until validation has them fixed.
            // eslint-disable-next-line react/no-array-index-key
            key={i}
            data-column={c.name}
            className="tw:col-span-full tw:grid tw:grid-cols-subgrid tw:items-center tw:px-1.5 tw:leading-4"
          >
            <span className="tw:min-w-0 tw:truncate">{c.name}</span>
            {data.showTypes ? <span className="tw:min-w-0 tw:truncate">{c.type}</span> : null}
          </li>
        ))}
      </ul>
    </div>
  );
});
ViewNode.displayName = 'ViewNode';
