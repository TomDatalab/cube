import { memo } from 'react';
import { NodeResizer } from '@xyflow/react';
import type { Node, NodeProps } from '@xyflow/react';

import { cn } from '@/lib/utils';

import type { TableColor } from './modelGraph';

export type NoteNodeData = {
  id: string;
  text: string;
  color?: TableColor;
  onResize: (nodeId: string, box: { x: number; y: number; width: number; height: number }) => void;
};

export type NoteFlowNode = Node<NoteNodeData, 'note'>;

type NoteNodeProps = Pick<NodeProps<NoteFlowNode>, 'data'> & { id?: string; selected?: boolean };

// A text note on the canvas: its text with its line breaks, on pale yellow
// or, when picked, a palette color at 18%.
export const NoteNode = memo(({ id, data, selected }: NoteNodeProps) => (
  <div
    data-slot="model-note-node"
    data-selected={selected || undefined}
    className={cn(
      'tw:h-full tw:w-full tw:overflow-hidden tw:border tw:border-foreground/30 tw:p-2 tw:text-xs tw:text-foreground',
      selected && 'tw:ring-2 tw:ring-ring'
    )}
    style={{
      background: data.color
        ? `color-mix(in oklch, var(--model-${data.color}) 18%, var(--card))`
        : 'var(--model-note)',
      whiteSpace: 'pre-wrap',
      overflowWrap: 'anywhere',
    }}
  >
    <NodeResizer
      isVisible={Boolean(selected)}
      minWidth={60}
      minHeight={30}
      color="var(--ring)"
      onResizeEnd={(_event, box) => data.onResize(id ?? `note:${data.id}`, box)}
    />
    {data.text}
  </div>
));
NoteNode.displayName = 'NoteNode';
