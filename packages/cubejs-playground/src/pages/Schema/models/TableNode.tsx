import { memo } from 'react';
import { Handle, Position } from '@xyflow/react';
import type { Node, NodeProps } from '@xyflow/react';
import { X } from 'lucide-react';

import { Badge } from '@/components/ui/badge';

export type TableNodeData = {
  alias: string;
  schema: string;
  table: string;
  columns: { name: string; type: string }[];
  principal: boolean;
  // Not in the connection's tables any more.
  missing: boolean;
  onRemove: (alias: string) => void;
};

export type TableFlowNode = Node<TableNodeData, 'table'>;

// A table on the canvas: its alias, the principal badge and its columns,
// each with a handle on both sides named after the column.
export const TableNode = memo(({ data }: Pick<NodeProps<TableFlowNode>, 'data'>) => (
  <div
    data-slot="model-table-node"
    className="tw:min-w-52 tw:rounded-md tw:border tw:bg-card tw:text-xs tw:text-card-foreground tw:shadow-sm"
  >
    <div className="tw:flex tw:items-center tw:gap-2 tw:border-b tw:px-3 tw:py-2">
      <div className="tw:min-w-0 tw:flex-1">
        <div className="tw:truncate tw:text-sm tw:font-medium">{data.alias}</div>
        {data.alias !== data.table ? (
          <div className="tw:truncate tw:text-muted-foreground">{`${data.schema}.${data.table}`}</div>
        ) : null}
      </div>
      {data.principal ? <Badge>principal</Badge> : null}
      <button
        type="button"
        data-slot="model-table-remove"
        aria-label={`Remove ${data.alias}`}
        title="Remove from the model"
        className="nodrag tw:flex tw:size-5 tw:cursor-pointer tw:items-center tw:justify-center tw:rounded tw:border-0 tw:bg-transparent tw:p-0 tw:text-muted-foreground tw:hover:bg-accent tw:hover:text-foreground"
        onClick={() => data.onRemove(data.alias)}
      >
        <X className="tw:size-3.5" />
      </button>
    </div>
    {data.missing ? (
      <p className="tw:m-0 tw:px-3 tw:py-2 tw:text-destructive">Not in the connection</p>
    ) : (
      <ul className="tw:m-0 tw:list-none tw:p-0 tw:py-1">
        {data.columns.map((c) => (
          <li key={c.name} className="tw:relative tw:flex tw:items-center tw:gap-3 tw:px-3 tw:py-1">
            <Handle type="target" position={Position.Left} id={c.name} />
            <span className="tw:flex-1 tw:truncate">{c.name}</span>
            <span className="tw:text-muted-foreground">{c.type}</span>
            <Handle type="source" position={Position.Right} id={c.name} />
          </li>
        ))}
      </ul>
    )}
  </div>
));
TableNode.displayName = 'TableNode';
