import { ReactNode, useState } from 'react';
import {
  Box, ChevronDown, ChevronRight, Database, Spline, Square, StickyNote, Table2, TableProperties,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';

import { cn } from '@/lib/utils';
import { EmptyState, SearchInput } from '@/components/kit';

import type { OrgDetail } from '../tree';
import { useResizable } from '../useResizable';
import { ResizeHandle } from '../ResizeHandle';
import type { ModelDefinition, ModelTable } from './modelGraph';

// What a table of the database tables section carries while dragged to the
// canvas.
export const TABLE_MIME = 'application/x-cube-table';

const PANE_WIDTH = { initial: 256, min: 180, max: 520 };
type Columns = { name: string; type: string }[];

// A section of the pane, as Redgate's: a header that opens and closes it.
function Section({
  title, open, onToggle, disabled, children,
}: {
  title: string;
  open: boolean;
  onToggle(): void;
  disabled?: boolean;
  children?: ReactNode;
}) {
  const Chevron = open ? ChevronDown : ChevronRight;
  return (
    <section className={cn('tw:flex tw:min-h-0 tw:flex-col tw:border-b', open && 'tw:flex-1')}>
      <button
        type="button"
        aria-label={title}
        aria-expanded={open}
        disabled={disabled}
        title={disabled ? 'Coming soon' : undefined}
        className="tw:flex tw:w-full tw:cursor-pointer tw:items-center tw:justify-between tw:border-0 tw:bg-transparent tw:px-3 tw:py-2 tw:text-left tw:text-xs tw:font-semibold tw:tracking-wide tw:text-foreground tw:uppercase tw:hover:bg-accent tw:disabled:cursor-default tw:disabled:opacity-50"
        onClick={onToggle}
      >
        {title}
        <Chevron className="tw:size-4" />
      </button>
      {open ? <div className="tw:min-h-0 tw:flex-1 tw:overflow-y-auto tw:px-2 tw:pb-2">{children}</div> : null}
    </section>
  );
}

// One line of the tree: an icon, a name, and what a click on it picks.
function Item({
  id, icon: Icon, label, selected, onPick,
}: { id: string; icon: LucideIcon; label: string; selected: boolean; onPick(): void }) {
  return (
    <button
      type="button"
      data-slot="model-structure-item"
      data-id={id}
      aria-current={selected || undefined}
      className={cn(
        'tw:flex tw:min-w-0 tw:flex-1 tw:cursor-pointer tw:items-center tw:gap-1.5 tw:rounded-sm tw:border-0 tw:bg-transparent tw:px-1 tw:py-0.5 tw:text-left tw:text-xs tw:text-inherit tw:hover:bg-accent',
        selected && 'tw:bg-primary tw:text-primary-foreground tw:hover:bg-primary'
      )}
      onClick={onPick}
    >
      <Icon aria-hidden className="tw:size-3.5 tw:shrink-0" />
      <span className="tw:truncate">{label}</span>
    </button>
  );
}

// A group of the tree: Tables, References, Text notes...
function Group({
  icon: Icon, label, open, onToggle, children,
}: { icon: LucideIcon; label: string; open: boolean; onToggle(): void; children?: ReactNode }) {
  const Chevron = open ? ChevronDown : ChevronRight;
  return (
    <li>
      <button
        type="button"
        aria-expanded={open}
        className="tw:flex tw:w-full tw:cursor-pointer tw:items-center tw:gap-1 tw:border-0 tw:bg-transparent tw:px-0 tw:py-0.5 tw:text-left tw:text-xs tw:font-medium tw:text-inherit"
        onClick={onToggle}
      >
        <Chevron className="tw:size-3" />
        <Icon aria-hidden className="tw:size-3.5" />
        {label}
      </button>
      {open ? <ul className="tw:m-0 tw:list-none tw:p-0 tw:pl-4">{children}</ul> : null}
    </li>
  );
}

export type StructureTreeProps = {
  def: ModelDefinition;
  columnsOf(t: ModelTable): Columns;
  selection: string[];
  selectedEdge: number | null;
  onPickNode(id: string): void;
  onPickEdge(index: number): void;
};

// Redgate's model structure: Model › Tables (› columns), References, Text
// notes, Views, Subject areas, and the cubes.
function StructureTree({
  def, columnsOf, selection, selectedEdge, onPickNode, onPickEdge,
}: StructureTreeProps) {
  const [open, setOpen] = useState<Record<string, boolean>>({ tables: true, references: true });
  const [columnsOpen, setColumnsOpen] = useState<string[]>([]);
  const toggle = (key: string) => setOpen((was) => ({ ...was, [key]: !was[key] }));
  return (
    <div data-slot="model-structure" className="tw:text-xs">
      <div className="tw:px-1 tw:py-1 tw:font-semibold">Model</div>
      <ul className="tw:m-0 tw:list-none tw:p-0 tw:pl-2">
        <Group icon={Table2} label="Tables" open={Boolean(open.tables)} onToggle={() => toggle('tables')}>
          {def.tables.map((t) => {
            const shown = columnsOpen.includes(t.alias);
            return (
              <li key={t.alias}>
                <div className="tw:flex tw:items-center">
                  <button
                    type="button"
                    aria-label={`${shown ? 'Hide' : 'Show'} columns of ${t.alias}`}
                    aria-expanded={shown}
                    className="tw:flex tw:size-4 tw:shrink-0 tw:cursor-pointer tw:items-center tw:justify-center tw:border-0 tw:bg-transparent tw:p-0 tw:text-muted-foreground"
                    onClick={() => setColumnsOpen((was) => (
                      shown ? was.filter((a) => a !== t.alias) : [...was, t.alias]
                    ))}
                  >
                    {shown ? <ChevronDown className="tw:size-3" /> : <ChevronRight className="tw:size-3" />}
                  </button>
                  <Item
                    id={t.alias}
                    icon={Table2}
                    label={t.alias}
                    selected={selection.includes(t.alias)}
                    onPick={() => onPickNode(t.alias)}
                  />
                </div>
                {shown ? (
                  <ul className="tw:m-0 tw:list-none tw:p-0 tw:pl-8 tw:text-muted-foreground">
                    {columnsOf(t).map((c) => (
                      <li key={c.name} className="tw:flex tw:gap-2 tw:py-0.5">
                        <span className="tw:min-w-0 tw:flex-1 tw:truncate">{c.name}</span>
                        <span className="tw:shrink-0">{c.type}</span>
                      </li>
                    ))}
                  </ul>
                ) : null}
              </li>
            );
          })}
        </Group>
        <Group icon={Spline} label="References" open={Boolean(open.references)} onToggle={() => toggle('references')}>
          {def.joins.map((j, index) => (
            // A reference has no name yet: its place is its identity.
            // eslint-disable-next-line react/no-array-index-key
            <li key={index}>
              <Item
                id={`join-${index}`}
                icon={Spline}
                label={`${j.from.alias} → ${j.to.alias}`}
                selected={selectedEdge === index}
                onPick={() => onPickEdge(index)}
              />
            </li>
          ))}
        </Group>
        <Group icon={StickyNote} label="Text notes" open={Boolean(open.notes)} onToggle={() => toggle('notes')} />
        <Group icon={TableProperties} label="Views" open={Boolean(open.views)} onToggle={() => toggle('views')} />
        <Group icon={Square} label="Subject areas" open={Boolean(open.areas)} onToggle={() => toggle('areas')} />
        <Group icon={Box} label="Cubes" open={Boolean(open.cubes)} onToggle={() => toggle('cubes')} />
      </ul>
    </div>
  );
}

// A table of the connection: dragged to the canvas, its columns on demand.
function TableItem({
  ds, schema, table, columns, disabled, dataSource,
}: {
  ds: string;
  schema: string;
  table: string;
  columns: Columns;
  disabled: boolean;
  dataSource: string;
}) {
  const [open, setOpen] = useState(false);
  const Chevron = open ? ChevronDown : ChevronRight;
  return (
    <li>
      <div className="tw:flex tw:items-center">
        <button
          type="button"
          data-slot="model-table-expand"
          aria-label={`${open ? 'Hide' : 'Show'} the columns of ${table}`}
          aria-expanded={open}
          disabled={columns.length === 0}
          className="tw:flex tw:size-5 tw:shrink-0 tw:cursor-pointer tw:items-center tw:justify-center tw:rounded tw:border-0 tw:bg-transparent tw:p-0 tw:text-muted-foreground tw:hover:bg-accent tw:disabled:invisible"
          onClick={() => setOpen((was) => !was)}
        >
          <Chevron className="tw:size-3.5" />
        </button>
        <div
          data-slot="model-table-item"
          draggable={!disabled}
          aria-disabled={disabled || undefined}
          title={disabled
            ? `Tables of one connection only: this model reads ${dataSource}`
            : `${ds}.${schema}.${table}`}
          className={disabled
            ? 'tw:min-w-0 tw:flex-1 tw:cursor-not-allowed tw:truncate tw:rounded tw:px-1 tw:py-0.5 tw:text-muted-foreground tw:opacity-50'
            : 'tw:min-w-0 tw:flex-1 tw:cursor-grab tw:truncate tw:rounded tw:px-1 tw:py-0.5 tw:hover:bg-accent'}
          onDragStart={(e) => {
            if (disabled) {
              e.preventDefault();
              return;
            }
            e.dataTransfer.setData(TABLE_MIME, JSON.stringify({ ds, schema, table }));
            e.dataTransfer.effectAllowed = 'move';
          }}
        >
          {table}
        </div>
      </div>
      {open ? (
        <ul data-slot="model-table-columns" className="tw:m-0 tw:list-none tw:p-0 tw:pl-6 tw:text-xs">
          {columns.map((c) => (
            <li key={c.name} className="tw:flex tw:gap-2 tw:py-0.5">
              <span className="tw:min-w-0 tw:flex-1 tw:truncate">{c.name}</span>
              <span className="tw:shrink-0 tw:text-muted-foreground">{c.type}</span>
            </li>
          ))}
        </ul>
      ) : null}
    </li>
  );
}

// The connections' tables, ds › schema › table, matching the search.
function DatabaseTables({
  tables, dataSource, locked,
}: {
  tables: OrgDetail['tables'];
  dataSource: string;
  // The model has tables: only its connection's can join it.
  locked: boolean;
}) {
  const [search, setSearch] = useState('');
  const needle = search.trim().toLowerCase();
  const tree = Object.entries(tables || {}).map(([ds, schemas]) => ({
    ds,
    schemas: Object.entries(schemas).map(([schema, list]) => ({
      schema,
      tables: Object.entries(list).filter(([t]) => (
        !needle || [ds, schema, t].some((part) => part.toLowerCase().includes(needle))
      )).map(([t, columns]) => ({ name: t, columns: columns || [] })),
    })).filter((s) => s.tables.length > 0),
  })).filter((d) => d.schemas.length > 0);

  return (
    <div data-slot="model-table-panel" className="tw:flex tw:flex-col tw:gap-2 tw:pt-1">
      <SearchInput value={search} onChange={setSearch} placeholder="Search tables" />
      <p className="tw:m-0 tw:text-xs tw:text-muted-foreground">Drag a table onto the canvas</p>
      {tree.length === 0 ? <EmptyState>No table matches</EmptyState> : null}
      <ul className="tw:m-0 tw:list-none tw:p-0 tw:text-sm">
        {tree.map(({ ds, schemas }) => {
          const disabled = locked && ds !== dataSource;
          return (
            <li key={ds} className="tw:mb-2">
              <div className="tw:flex tw:items-center tw:gap-1 tw:font-medium">
                <Database aria-hidden className="tw:size-3.5" />
                {ds}
              </div>
              <ul className="tw:m-0 tw:list-none tw:p-0 tw:pl-3">
                {schemas.map(({ schema, tables: names }) => (
                  <li key={schema}>
                    <div className="tw:text-muted-foreground">{schema}</div>
                    <ul className="tw:m-0 tw:list-none tw:p-0 tw:pl-3">
                      {names.map(({ name: table, columns }) => (
                        <TableItem
                          key={table}
                          ds={ds}
                          schema={schema}
                          table={table}
                          columns={columns}
                          disabled={disabled}
                          dataSource={dataSource}
                        />
                      ))}
                    </ul>
                  </li>
                ))}
              </ul>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

export type StructurePanelProps = StructureTreeProps & {
  tables: OrgDetail['tables'];
  dataSource: string;
  locked: boolean;
};

// The left pane, as Redgate's: Model structure, then our Database tables,
// then Model explorer and Issues (later phases).
export function StructurePanel({
  tables, dataSource, locked, ...tree
}: StructurePanelProps) {
  const [width, setWidth] = useResizable('cubePlayground.modelLeftWidth', PANE_WIDTH.initial, PANE_WIDTH.min, PANE_WIDTH.max);
  const [open, setOpen] = useState({ structure: true, tables: true });
  return (
    <aside
      data-slot="model-left-pane"
      style={{ width }}
      className="tw:relative tw:flex tw:shrink-0 tw:flex-col tw:border-r tw:bg-card"
    >
      <ResizeHandle width={width} min={PANE_WIDTH.min} max={PANE_WIDTH.max} label="navigation pane" onResize={setWidth} />
      <Section
        title="Model structure"
        open={open.structure}
        onToggle={() => setOpen((was) => ({ ...was, structure: !was.structure }))}
      >
        <StructureTree {...tree} />
      </Section>
      <Section
        title="Database tables"
        open={open.tables}
        onToggle={() => setOpen((was) => ({ ...was, tables: !was.tables }))}
      >
        <DatabaseTables tables={tables} dataSource={dataSource} locked={locked} />
      </Section>
      <Section title="Model explorer" open={false} disabled onToggle={() => {}} />
      <Section title="Issues" open={false} disabled onToggle={() => {}} />
    </aside>
  );
}
