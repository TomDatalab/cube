import { memo, useEffect } from 'react';
import { Handle, Position, useUpdateNodeInternals } from '@xyflow/react';
import type { Node, NodeProps } from '@xyflow/react';
import {
  ChevronDown, ChevronRight, Forward, X,
} from 'lucide-react';

import { cn } from '@/lib/utils';

import type { TableColor } from './modelGraph';
import { ModelResizer } from './ModelResizer';
import { inkOn, paint } from './paint';
import { LINK_HANDLE_STYLE, NODE_HANDLE_STYLE } from './nodeHandle';

export type TableNodeData = {
  // Someone else edits the model: no resizer, no Remove.
  readOnly?: boolean;
  alias: string;
  schema: string;
  table: string;
  columns: { name: string; type: string }[];
  // Not in the connection's tables any more.
  missing: boolean;
  // Designed in the model, not in the database yet.
  draft: boolean;
  color?: TableColor;
  // The column rows' color; none is the card.
  bodyColor?: TableColor;
  // How see-through those colors are, in percent.
  colorTransparency?: number;
  bodyTransparency?: number;
  // The columns shown with a key: the primary key a cube gives the table
  // (cubeKeyOf), a draft's own primary key columns.
  keys: string[];
  // The columns that are an end of a link.
  linked: string[];
  // The columns that point at another table: shown FK.
  fks: string[];
  // The model's format: the type column, and N on the columns allowing nulls.
  showTypes?: boolean;
  showNullable?: boolean;
  nullable?: string[];
  // Removes the table, or only the shortcut when this node is one.
  onRemove: (nodeId: string) => void;
  // The shortcut's id when this node is a shortcut of the table.
  shortcut?: string;
  // Sized by hand: the box fills the node's fixed size.
  fixedSize: boolean;
  onResize: (nodeId: string, box: { x: number; y: number; width: number; height: number }) => void;
  // Shown as its header only; its links still end on it.
  collapsed: boolean;
  onToggle: (alias: string) => void;
  // The column of this table picked to start a link, if any.
  picked: string | null;
  onPick: (alias: string, column: string) => void;
};

export type TableFlowNode = Node<TableNodeData, 'table'>;

// React Flow's 6px dot is hard to grab.
const HANDLE_STYLE = LINK_HANDLE_STYLE;

// The handles of a collapsed table: kept, so its links stay drawn, but out
// of sight in the middle of its header.
// The rows' tracks: name and type take their text's width and share what a
// resize adds or takes away; the key marks keep theirs.
const COLUMNS = 'minmax(0, auto) minmax(0, auto) auto';
const COLUMNS_NO_TYPE = 'minmax(0, auto) auto';

const HIDDEN_HANDLE_STYLE = { ...HANDLE_STYLE, opacity: 0, pointerEvents: 'none' as const };

// The table and column of a row under the pointer, so a link dropped
// anywhere on a column's row, not only on its small handle, still links.
export function columnAt(element: Element | null): { alias: string; column: string } | null {
  const row = element?.closest('[data-column]');
  const alias = row?.closest('.react-flow__node')?.getAttribute('data-id');
  const column = row?.getAttribute('data-column');
  return alias && column ? { alias, column } : null;
}

type TableNodeProps = Pick<NodeProps<TableFlowNode>, 'data'> & {
  id?: string;
  selected?: boolean;
  // Set by React Flow while a resizer is dragged, and for a fixed size.
  width?: number;
};

// What a table row shows after its type, as Redgate writes it.
function marks(data: TableNodeData, column: string) {
  const shown = [
    data.keys.includes(column) ? { text: 'PK', label: 'primary key' } : null,
    data.fks.includes(column) ? { text: 'FK', label: 'foreign key' } : null,
    data.showNullable && data.nullable?.includes(column) ? { text: 'N', label: 'allows nulls' } : null,
  ].filter((m): m is { text: string; label: string } => m !== null);
  return (
    <span data-slot="model-column-marks" className="tw:shrink-0 tw:text-right tw:whitespace-nowrap">
      {shown.map((m, i) => (
        <span key={m.text}>
          {i > 0 ? ' ' : null}
          <span aria-label={m.label}>{m.text}</span>
        </span>
      ))}
    </span>
  );
}

const ICON_BUTTON = 'nodrag tw:flex tw:size-4 tw:shrink-0 tw:cursor-pointer tw:items-center tw:justify-center tw:rounded-sm tw:border-0 tw:bg-transparent tw:p-0 tw:text-foreground/60 tw:hover:bg-background/60 tw:hover:text-foreground';

// A table on the canvas, drawn as Redgate draws it: a thin box, its name
// centred on a header of its color, then one row per column with its name,
// its type and PK/FK. Each column has a handle on both sides named after it.
export const TableNode = memo(({
  id, data, selected, width,
}: TableNodeProps) => {
  // React Flow measures a node's handles once: a column added or renamed
  // (a new foreign key, say) needs a new measure for its links to draw.
  const updateNodeInternals = useUpdateNodeInternals();
  const shape = data.columns.map((c) => c.name).join('\u0000');
  useEffect(() => {
    updateNodeInternals(id ?? data.alias);
  }, [updateNodeInternals, id, data.alias, shape, data.collapsed]);
  return (
    <div
      data-slot="model-table-node"
      data-selected={selected || undefined}
      className={cn(
        'tw:flex tw:min-w-40 tw:flex-col tw:border tw:border-foreground/70 tw:bg-card tw:text-xs tw:text-card-foreground',
        // The content follows the frame while it is dragged, not on release.
        // Rows that do not fit are cut off below; the link dots still stick out
        // at the sides.
        (data.fixedSize || width !== undefined) && 'tw:h-full tw:w-full tw:overflow-x-visible tw:overflow-y-clip',
        selected && 'tw:ring-2 tw:ring-ring'
      )}
      style={data.bodyColor
        ? { background: paint(data.bodyColor, data.bodyTransparency), color: inkOn(data.bodyTransparency) }
        : undefined}
    >
      {data.readOnly ? null : (
        <ModelResizer
          visible={Boolean(selected)}
          minWidth={160}
          minHeight={28}
          onResizeEnd={(_event, box) => data.onResize(id ?? data.alias, box)}
        />
      )}
      <Handle type="target" position={Position.Top} id="node" style={NODE_HANDLE_STYLE} isConnectable={false} />
      <Handle type="source" position={Position.Top} id="node" style={NODE_HANDLE_STYLE} isConnectable={false} />
      <div
        data-slot="model-table-band"
        className={cn('tw:relative tw:flex tw:items-center tw:gap-1 tw:px-1.5 tw:py-1', !data.collapsed && 'tw:border-b tw:border-foreground/70')}
        style={data.color
          ? { background: paint(data.color, data.colorTransparency), color: inkOn(data.colorTransparency) }
          : { background: 'var(--muted)', color: 'var(--card-foreground)' }}
      >
        {/* Where a cube's line to this table ends: the middle of the header. */}
        <Handle type="target" position={Position.Left} id="band" style={HIDDEN_HANDLE_STYLE} isConnectable={false} />
        <button
          type="button"
          data-slot="model-table-toggle"
          aria-label={`${data.collapsed ? 'Expand' : 'Collapse'} ${data.alias}`}
          aria-expanded={!data.collapsed}
          title={data.collapsed ? 'Show the columns' : 'Hide the columns'}
          className={ICON_BUTTON}
          onClick={() => data.onToggle(data.alias)}
        >
          {data.collapsed ? <ChevronRight className="tw:size-3" /> : <ChevronDown className="tw:size-3" />}
        </button>
        {data.shortcut ? <Forward aria-label="shortcut" className="tw:size-3 tw:shrink-0" /> : null}
        <div className="tw:min-w-0 tw:flex-1 tw:text-center">
          <div data-slot="model-table-name" className="tw:truncate tw:font-bold">{data.alias}</div>
          {data.collapsed ? (
            <div className="tw:truncate tw:text-foreground/60">
              {`${data.columns.length} ${data.columns.length === 1 ? 'column' : 'columns'}`}
            </div>
          ) : null}
        </div>
        {data.draft ? <span className="tw:text-[10px] tw:text-foreground/70">new</span> : null}
        {data.readOnly ? null : (
          <button
            type="button"
            data-slot="model-table-remove"
            aria-label={`Remove ${data.alias}`}
            title={data.shortcut ? 'Remove this shortcut' : 'Remove from the model'}
            className={ICON_BUTTON}
            onClick={() => data.onRemove(id ?? data.alias)}
          >
            <X className="tw:size-3" />
          </button>
        )}
        {data.collapsed ? data.columns.map((c) => (
          <span key={c.name}>
            <Handle type="target" position={Position.Left} id={c.name} style={HIDDEN_HANDLE_STYLE} isConnectable={false} />
            <Handle type="source" position={Position.Right} id={c.name} style={HIDDEN_HANDLE_STYLE} isConnectable={false} />
          </span>
        )) : null}
      </div>
      {!data.collapsed && data.missing ? (
        <p className="tw:m-0 tw:px-2 tw:py-1 tw:text-destructive">Not in the connection</p>
      ) : null}
      {!data.collapsed && !data.missing ? (
        // One grid for all the rows: the types line up, and a resize shares
        // the width between the names and the types.
        <ul
          className="tw:m-0 tw:grid tw:min-h-0 tw:list-none tw:gap-x-3 tw:p-0 tw:py-0.5"
          style={{ gridTemplateColumns: data.showTypes === false ? COLUMNS_NO_TYPE : COLUMNS }}
        >
          {data.columns.map((c) => (
            <li
              key={c.name}
              data-column={c.name}
              data-picked={data.picked === c.name || undefined}
              className={cn(
                'tw:relative tw:col-span-full tw:grid tw:grid-cols-subgrid tw:items-center tw:px-1.5 tw:leading-4 tw:hover:bg-accent',
                data.picked === c.name && 'tw:bg-accent tw:ring-2 tw:ring-inset tw:ring-primary'
              )}
            >
              <Handle type="target" position={Position.Left} id={c.name} style={HANDLE_STYLE} />
              <button
                type="button"
                data-slot="model-column-name"
                title="Click, then click a column of another table to link them"
                className="nodrag tw:min-w-0 tw:cursor-pointer tw:truncate tw:border-0 tw:bg-transparent tw:p-0 tw:text-left tw:text-inherit tw:[font:inherit] tw:hover:underline"
                onClick={() => data.onPick(data.alias, c.name)}
              >
                {c.name}
              </button>
              {data.showTypes === false ? null : <span className="tw:min-w-0 tw:truncate">{c.type}</span>}
              <span className="tw:w-9 tw:text-right">{marks(data, c.name)}</span>
              <Handle type="source" position={Position.Right} id={c.name} style={HANDLE_STYLE} />
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
});
TableNode.displayName = 'TableNode';
