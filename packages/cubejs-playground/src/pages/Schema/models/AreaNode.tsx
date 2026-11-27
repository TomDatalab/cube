import { memo } from 'react';
import { NodeResizer } from '@xyflow/react';
import type { Node, NodeProps } from '@xyflow/react';

import { cn } from '@/lib/utils';

import type { TableColor } from './modelGraph';

export type AreaNodeData = {
  // Someone else edits the model: no resizer.
  readOnly?: boolean;
  id: string;
  name: string;
  fill?: TableColor;
  border?: TableColor;
  onResize: (nodeId: string, box: { x: number; y: number; width: number; height: number }) => void;
};

export type AreaFlowNode = Node<AreaNodeData, 'area'>;

type AreaNodeProps = Pick<NodeProps<AreaFlowNode>, 'data'> & { id?: string; selected?: boolean };

// A subject area: a dashed rectangle behind the tables, its name at the
// bottom-right corner.
export const AreaNode = memo(({ id, data, selected }: AreaNodeProps) => (
  <div
    data-slot="model-area-node"
    data-selected={selected || undefined}
    className={cn(
      'tw:relative tw:h-full tw:w-full tw:text-xs tw:text-foreground',
      selected && 'tw:ring-2 tw:ring-ring'
    )}
    style={{
      background: `color-mix(in oklch, var(--model-${data.fill ?? 'slate'}) 12%, var(--card))`,
      border: `1.5px dashed var(--model-${data.border ?? 'slate'})`,
    }}
  >
    {data.readOnly ? null : (
      <NodeResizer
        isVisible={Boolean(selected)}
        minWidth={80}
        minHeight={60}
        color="var(--ring)"
        onResizeEnd={(_event, box) => data.onResize(id ?? `area:${data.id}`, box)}
      />
    )}
    <span
      data-slot="model-area-name"
      className="tw:absolute tw:right-2 tw:bottom-1 tw:max-w-full tw:truncate tw:font-bold"
    >
      {data.name}
    </span>
  </div>
));
AreaNode.displayName = 'AreaNode';
