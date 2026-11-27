import { memo } from 'react';
import type { CSSProperties } from 'react';
import type { Node, NodeProps } from '@xyflow/react';

import { cn } from '@/lib/utils';

import type { NoteFormat, TableColor } from './modelGraph';
import { ModelResizer } from './ModelResizer';
import { inkOn, paint } from './paint';

export type NoteNodeData = {
  // Someone else edits the model: no resizer.
  readOnly?: boolean;
  id: string;
  text: string;
  color?: TableColor;
  format?: NoteFormat;
  onResize: (nodeId: string, box: { x: number; y: number; width: number; height: number }) => void;
};

export type NoteFlowNode = Node<NoteNodeData, 'note'>;

type NoteNodeProps = Pick<NodeProps<NoteFlowNode>, 'data'> & { id?: string; selected?: boolean };

const FAMILY = {
  sans: 'inherit',
  serif: 'Georgia, "Times New Roman", serif',
  mono: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
};
const JUSTIFY = { top: 'flex-start', middle: 'center', bottom: 'flex-end' };

// The box and text styles of a note's format. On a palette color the text is
// dark unless it has a color of its own.
export function noteStyle(color: TableColor | undefined, f: NoteFormat = {}): CSSProperties {
  const decoration = [f.underline && 'underline', f.strike && 'line-through'].filter(Boolean).join(' ');
  let text = 'var(--foreground)';
  if (f.textColor) {
    text = paint(f.textColor, f.textTransparency);
  } else if (color) {
    text = inkOn(f.colorTransparency);
  }
  return {
    background: color ? paint(color, f.colorTransparency) : 'var(--model-note)',
    color: text,
    border: f.borderStyle === 'none'
      ? 'none'
      : `1px ${f.borderStyle ?? 'solid'} ${f.borderColor ? paint(f.borderColor, f.borderTransparency) : 'color-mix(in oklch, var(--foreground) 30%, transparent)'}`,
    fontSize: f.fontSize ?? 12,
    fontFamily: FAMILY[f.fontFamily ?? 'sans'],
    fontWeight: f.bold ? 700 : undefined,
    fontStyle: f.italic ? 'italic' : undefined,
    textDecoration: decoration || undefined,
    textAlign: f.align ?? 'left',
    justifyContent: JUSTIFY[f.valign ?? 'top'],
    whiteSpace: 'pre-wrap',
    overflowWrap: 'anywhere',
  };
}

// A text note on the canvas: its text with its line breaks, formatted as
// set in its properties, on pale yellow or a palette color.
export const NoteNode = memo(({ id, data, selected }: NoteNodeProps) => (
  <div
    data-slot="model-note-node"
    data-selected={selected || undefined}
    className={cn(
      'tw:flex tw:h-full tw:w-full tw:flex-col tw:overflow-hidden tw:p-2 tw:leading-snug',
      selected && 'tw:ring-2 tw:ring-ring'
    )}
    style={noteStyle(data.color, data.format)}
  >
    {data.readOnly ? null : (
      <ModelResizer
        visible={Boolean(selected)}
        minWidth={60}
        minHeight={30}
        onResizeEnd={(_event, box) => data.onResize(id ?? `note:${data.id}`, box)}
      />
    )}
    <div data-slot="model-note-text">{data.text}</div>
  </div>
));
NoteNode.displayName = 'NoteNode';
