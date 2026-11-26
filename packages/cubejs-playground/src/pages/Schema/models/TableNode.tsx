import { memo } from 'react';
import { Handle, Position } from '@xyflow/react';
import type { Node, NodeProps } from '@xyflow/react';
import {
  Calendar, ChevronDown, ChevronRight, Hash, KeyRound, Link2, Table2, ToggleLeft, Type, X,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';

import { Badge } from '@/components/ui/badge';
import { cn } from '@/lib/utils';

import type { TableColor } from './modelGraph';

export type TableNodeData = {
  alias: string;
  schema: string;
  table: string;
  columns: { name: string; type: string }[];
  principal: boolean;
  // Not in the connection's tables any more.
  missing: boolean;
  // Designed in the model, not in the database yet.
  draft: boolean;
  color?: TableColor;
  // The columns shown with a key: the model's primary key on the principal,
  // a draft's own primary key columns.
  keys: string[];
  // The columns that are an end of a link.
  linked: string[];
  onRemove: (alias: string) => void;
  // Shown as its header only; its links still end on it.
  collapsed: boolean;
  onToggle: (alias: string) => void;
  // The column of this table picked to start a link, if any.
  picked: string | null;
  onPick: (alias: string, column: string) => void;
};

export type TableFlowNode = Node<TableNodeData, 'table'>;

// React Flow's 6px dot is hard to grab; its stylesheet is unlayered, so it
// would win over Tailwind classes, hence the inline style.
const HANDLE_STYLE = {
  width: 12,
  height: 12,
  background: 'var(--primary)',
  border: '2px solid var(--card)',
};

// The handles of a collapsed table: kept, so its links stay drawn, but out
// of sight in the middle of its header.
const HIDDEN_HANDLE_STYLE = { ...HANDLE_STYLE, opacity: 0, pointerEvents: 'none' as const };

const NUMBER = ['int', 'numeric', 'decimal', 'double', 'float', 'real', 'number', 'money', 'serial'];
const TEXT = ['text', 'char', 'string', 'uuid', 'json', 'enum'];

// The icon of a column's type, portable or as the database names it.
export function typeIcon(type: string): LucideIcon | null {
  const t = type.toLowerCase();
  if (t.includes('bool')) {
    return ToggleLeft;
  }
  if (t.includes('date') || t.includes('time')) {
    return Calendar;
  }
  if (NUMBER.some((n) => t.includes(n))) {
    return Hash;
  }
  if (TEXT.some((n) => t.includes(n))) {
    return Type;
  }
  return null;
}

// The table and column of a row under the pointer, so a link dropped
// anywhere on a column's row, not only on its small handle, still links.
export function columnAt(element: Element | null): { alias: string; column: string } | null {
  const row = element?.closest('[data-column]');
  const alias = row?.closest('.react-flow__node')?.getAttribute('data-id');
  const column = row?.getAttribute('data-column');
  return alias && column ? { alias, column } : null;
}

type TableNodeProps = Pick<NodeProps<TableFlowNode>, 'data'> & { selected?: boolean };

// A table on the canvas: a band of its color, its alias, the principal and
// new badges and its columns, each with a handle on both sides named after
// the column.
export const TableNode = memo(({ data, selected }: TableNodeProps) => (
  <div
    data-slot="model-table-node"
    data-selected={selected || undefined}
    className={cn(
      'tw:min-w-52 tw:overflow-hidden tw:rounded-md tw:border tw:bg-card tw:text-xs tw:text-card-foreground tw:shadow-md',
      selected && 'tw:ring-2 tw:ring-ring'
    )}
  >
    <div
      data-slot="model-table-band"
      className="tw:h-1.5"
      style={{ background: data.color ? `var(--model-${data.color})` : 'var(--muted)' }}
    />
    <div className={cn('tw:relative tw:flex tw:items-center tw:gap-2 tw:px-3 tw:py-2', !data.collapsed && 'tw:border-b')}>
      <button
        type="button"
        data-slot="model-table-toggle"
        aria-label={`${data.collapsed ? 'Expand' : 'Collapse'} ${data.alias}`}
        aria-expanded={!data.collapsed}
        title={data.collapsed ? 'Show the columns' : 'Hide the columns'}
        className="nodrag tw:flex tw:size-5 tw:shrink-0 tw:cursor-pointer tw:items-center tw:justify-center tw:rounded tw:border-0 tw:bg-transparent tw:p-0 tw:text-muted-foreground tw:hover:bg-accent tw:hover:text-foreground"
        onClick={() => data.onToggle(data.alias)}
      >
        {data.collapsed ? <ChevronRight className="tw:size-3.5" /> : <ChevronDown className="tw:size-3.5" />}
      </button>
      <Table2 aria-hidden className="tw:size-4 tw:shrink-0 tw:text-muted-foreground" />
      <div className="tw:min-w-0 tw:flex-1">
        <div className="tw:truncate tw:text-sm tw:font-medium">{data.alias}</div>
        {data.collapsed ? (
          <div className="tw:truncate tw:text-muted-foreground">
            {`${data.columns.length} ${data.columns.length === 1 ? 'column' : 'columns'}`}
          </div>
        ) : null}
        {!data.collapsed && data.alias !== data.table ? (
          <div className="tw:truncate tw:text-muted-foreground">{`${data.schema}.${data.table}`}</div>
        ) : null}
      </div>
      {data.principal ? <Badge>principal</Badge> : null}
      {data.draft ? <Badge variant="secondary">new</Badge> : null}
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
      {data.collapsed ? data.columns.map((c) => (
        <span key={c.name}>
          <Handle type="target" position={Position.Left} id={c.name} style={HIDDEN_HANDLE_STYLE} isConnectable={false} />
          <Handle type="source" position={Position.Right} id={c.name} style={HIDDEN_HANDLE_STYLE} isConnectable={false} />
        </span>
      )) : null}
    </div>
    {!data.collapsed && data.missing ? (
      <p className="tw:m-0 tw:px-3 tw:py-2 tw:text-destructive">Not in the connection</p>
    ) : null}
    {!data.collapsed && !data.missing ? (
      <ul className="tw:m-0 tw:list-none tw:p-0 tw:py-1">
        {data.columns.map((c) => {
          const Icon = typeIcon(c.type);
          return (
            <li
              key={c.name}
              data-column={c.name}
              data-picked={data.picked === c.name || undefined}
              className={cn(
                'tw:relative tw:flex tw:items-center tw:gap-2 tw:px-3 tw:py-1 tw:hover:bg-accent',
                data.picked === c.name && 'tw:bg-accent tw:ring-2 tw:ring-inset tw:ring-primary'
              )}
            >
              <Handle type="target" position={Position.Left} id={c.name} style={HANDLE_STYLE} />
              <span className="tw:flex tw:size-3.5 tw:shrink-0 tw:items-center tw:text-muted-foreground">
                {Icon ? <Icon aria-hidden className="tw:size-3.5" /> : null}
              </span>
              <button
                type="button"
                data-slot="model-column-name"
                title="Click, then click a column of another table to link them"
                className="nodrag tw:min-w-0 tw:flex-1 tw:cursor-pointer tw:truncate tw:border-0 tw:bg-transparent tw:p-0 tw:text-left tw:text-inherit tw:[font:inherit] tw:hover:underline"
                onClick={() => data.onPick(data.alias, c.name)}
              >
                {c.name}
              </button>
              {data.keys.includes(c.name) ? (
                <KeyRound aria-label="primary key" className="tw:size-3.5 tw:shrink-0 tw:text-primary" />
              ) : null}
              {data.linked.includes(c.name) ? (
                <Link2 aria-label="linked" className="tw:size-3.5 tw:shrink-0 tw:text-muted-foreground" />
              ) : null}
              <span className="tw:text-muted-foreground">{c.type}</span>
              <Handle type="source" position={Position.Right} id={c.name} style={HANDLE_STYLE} />
            </li>
          );
        })}
      </ul>
    ) : null}
  </div>
));
TableNode.displayName = 'TableNode';
