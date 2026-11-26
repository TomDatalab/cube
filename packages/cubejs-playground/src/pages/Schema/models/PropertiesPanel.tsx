import { ReactNode, useEffect, useState } from 'react';
import { Ban, Minus, Plus } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { Field, SimpleSelect, Spinner } from '@/components/kit';

import {
  JoinKind, ModelDefinition, ModelTable, Relationship, TABLE_COLORS, joinLabel, removeJoin, renameTable,
  setColor, setJoin, setPrimary, setTableComment,
} from './modelGraph';

type Columns = { name: string; type: string }[];

type ModelSelected = { kind: 'model' };
type TableSelected = { kind: 'table'; alias: string };
type ReferenceSelected = { kind: 'reference'; index: number };
export type Selected = ModelSelected | TableSelected | ReferenceSelected;

export type PropertiesPanelProps = {
  def: ModelDefinition;
  name: string;
  selected: Selected;
  columnsOf(t: ModelTable): Columns;
  onChange(next: ModelDefinition): void;
  onName(name: string): void;
  onEditColumns(alias: string): void;
  // A table took a new name: what is selected follows it.
  onRenamed(from: string, to: string): void;
  // The reference was deleted: nothing is selected any more.
  onDeselect(): void;
  onDeleteModel(): void;
  deleting: boolean;
  busy: boolean;
};

const TITLE: Record<Selected['kind'], string> = {
  model: 'Model properties',
  table: 'Table properties',
  reference: 'Reference properties',
};

const RELATIONSHIPS: { value: Relationship; label: string }[] = [
  { value: 'many_to_one', label: 'N:1 (many to one)' },
  { value: 'one_to_many', label: '1:N (one to many)' },
  { value: 'one_to_one', label: '1:1 (one to one)' },
];
const JOINS: { value: JoinKind; label: string }[] = [
  { value: 'left', label: 'LEFT' },
  { value: 'inner', label: 'INNER' },
];

const NATIVE_SELECT = 'tw:h-8 tw:w-full tw:rounded-md tw:border tw:bg-background tw:px-2 tw:text-sm tw:text-foreground';

// A section of the pane that opens and closes with + and −, as Redgate's.
function Section({ title, children }: { title: string; children: ReactNode }) {
  const [open, setOpen] = useState(true);
  return (
    <section className="tw:border-b">
      <button
        type="button"
        aria-expanded={open}
        className="tw:flex tw:w-full tw:cursor-pointer tw:items-center tw:justify-between tw:border-0 tw:bg-muted/50 tw:px-3 tw:py-2 tw:text-left tw:text-sm tw:font-semibold tw:text-foreground"
        onClick={() => setOpen((was) => !was)}
      >
        {title}
        {open ? <Minus className="tw:size-4" /> : <Plus className="tw:size-4" />}
      </button>
      {open ? <div className="tw:flex tw:flex-col tw:gap-3 tw:px-3 tw:py-3">{children}</div> : null}
    </section>
  );
}

function Value({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="tw:flex tw:flex-col tw:gap-0.5 tw:text-sm">
      <span className="tw:text-xs tw:text-muted-foreground">{label}</span>
      <span className="tw:font-medium">{children}</span>
    </div>
  );
}

// The table's name, kept as typed until Enter or leaving the field.
function TableName({
  def, alias, onChange, onRenamed,
}: {
  def: ModelDefinition;
  alias: string;
  onChange(d: ModelDefinition): void;
  onRenamed(from: string, to: string): void;
}) {
  const [value, setValue] = useState(alias);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    setValue(alias);
    setError(null);
  }, [alias]);
  const commit = () => {
    const result = renameTable(def, alias, value.trim());
    if (result.error) {
      setError(result.error);
      return;
    }
    setError(null);
    if (result.def !== def) {
      onChange(result.def);
      onRenamed(alias, value.trim());
    }
  };
  return (
    <Field label="Name" htmlFor="model-table-name">
      <Input
        id="model-table-name"
        aria-label="Selected table name"
        value={value}
        aria-invalid={error ? true : undefined}
        onChange={(e) => setValue(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            commit();
          }
        }}
      />
      {error ? <p role="alert" className="tw:m-0 tw:text-xs tw:text-destructive">{error}</p> : null}
    </Field>
  );
}

function ModelProperties({
  def, name, columnsOf, onChange, onName, onDeleteModel, deleting, busy,
}: PropertiesPanelProps) {
  const principal = def.tables.find((t) => t.alias === def.primary);
  const principalColumns = principal ? columnsOf(principal) : [];
  return (
    <Section title="General">
      <Field label="Name" htmlFor="model-name">
        <Input id="model-name" aria-label="Model name" value={name} onChange={(e) => onName(e.target.value)} />
      </Field>
      <Field label="Description" htmlFor="model-description">
        <Textarea
          id="model-description"
          aria-label="Model description"
          rows={2}
          value={def.description ?? ''}
          onChange={(e) => {
            const { description: _drop, ...rest } = def;
            onChange(e.target.value ? { ...rest, description: e.target.value } : rest);
          }}
        />
      </Field>
      <Value label="Connection">
        <span data-slot="model-connection">{def.dataSource || 'Drop a table to choose it'}</span>
      </Value>
      <Field label="Principal" htmlFor="model-principal">
        <SimpleSelect
          id="model-principal"
          value={def.primary ?? undefined}
          placeholder="No table yet"
          disabled={def.tables.length === 0}
          options={def.tables.map((t) => ({ value: t.alias, label: t.alias }))}
          onChange={(v) => v && onChange(setPrimary(def, v))}
        />
      </Field>
      <Field label="Primary key" htmlFor="model-primary-key">
        <SimpleSelect
          id="model-primary-key"
          value={def.primaryKey ?? undefined}
          placeholder="Choose a column"
          disabled={principalColumns.length === 0}
          options={principalColumns.map((c) => ({ value: c.name, label: c.name }))}
          onChange={(v) => onChange({ ...def, primaryKey: v ?? null })}
        />
      </Field>
      <Value label="Size">{`${def.tables.length} ${def.tables.length === 1 ? 'table' : 'tables'}`}</Value>
      <Button variant="destructive" size="sm" disabled={busy} onClick={onDeleteModel}>
        {deleting ? <Spinner className="tw:text-current" /> : null}
        Delete model
      </Button>
    </Section>
  );
}

function TableProperties({
  def, alias, columnsOf, onChange, onEditColumns, onRenamed,
}: PropertiesPanelProps & { alias: string }) {
  const table = def.tables.find((t) => t.alias === alias);
  if (!table) {
    return null;
  }
  return (
    <>
      <Section title="General">
        <TableName def={def} alias={alias} onChange={onChange} onRenamed={onRenamed} />
        <Value label="Source">{`${table.schema}.${table.table}${table.draft ? ' (new)' : ''}`}</Value>
        <Field label="Comment" htmlFor="model-table-comment">
          <Textarea
            id="model-table-comment"
            aria-label="Table comment"
            rows={2}
            value={table.comment ?? ''}
            onChange={(e) => onChange(setTableComment(def, alias, e.target.value))}
          />
        </Field>
        <div className="tw:flex tw:flex-col tw:gap-1">
          <span className="tw:text-xs tw:text-muted-foreground">Color</span>
          <div className="tw:flex tw:flex-wrap tw:items-center tw:gap-1" data-slot="model-color-palette">
            {TABLE_COLORS.map((color) => (
              <button
                key={color}
                type="button"
                aria-label={color}
                aria-pressed={table.color === color}
                title={color}
                className="tw:size-6 tw:cursor-pointer tw:rounded-full tw:border-2 tw:border-transparent tw:p-0 tw:aria-pressed:border-foreground"
                style={{ background: `var(--model-${color})` }}
                onClick={() => onChange(setColor(def, alias, color))}
              />
            ))}
            <button
              type="button"
              aria-label="No color"
              title="No color"
              className="tw:flex tw:size-6 tw:cursor-pointer tw:items-center tw:justify-center tw:rounded-full tw:border tw:bg-transparent tw:p-0 tw:text-muted-foreground tw:hover:bg-accent"
              onClick={() => onChange(setColor(def, alias, undefined))}
            >
              <Ban className="tw:size-3.5" />
            </button>
          </div>
        </div>
      </Section>
      <Section title="Columns">
        <div className="tw:grid tw:grid-cols-[1fr_auto] tw:gap-x-3 tw:text-xs tw:text-muted-foreground">
          <span>Name</span>
          <span>Type</span>
        </div>
        <ul className="tw:m-0 tw:flex tw:list-none tw:flex-col tw:gap-1 tw:p-0">
          {columnsOf(table).map((c) => (
            <li key={c.name} data-slot="model-property-column" className="tw:grid tw:grid-cols-[1fr_auto] tw:gap-x-3 tw:text-sm">
              <span className="tw:truncate">{c.name}</span>
              <span className="tw:text-muted-foreground">{c.type}</span>
            </li>
          ))}
        </ul>
        {table.draft ? (
          <Button variant="outline" size="sm" onClick={() => onEditColumns(alias)}>Edit columns</Button>
        ) : (
          <p className="tw:m-0 tw:text-xs tw:text-muted-foreground">The columns come from the database.</p>
        )}
      </Section>
    </>
  );
}

function ReferenceProperties({
  def, index, onChange, onDeselect,
}: PropertiesPanelProps & { index: number }) {
  const join = def.joins[index];
  if (!join) {
    return null;
  }
  return (
    <Section title="General">
      <Value label="From">{`${join.from.alias}.${join.from.column}`}</Value>
      <Value label="To">{`${join.to.alias}.${join.to.column}`}</Value>
      <Field label="Relationship" htmlFor="model-reference-relationship">
        <select
          id="model-reference-relationship"
          aria-label="Relationship"
          className={NATIVE_SELECT}
          value={join.relationship}
          onChange={(e) => onChange(setJoin(def, index, { relationship: e.target.value as Relationship }))}
        >
          {RELATIONSHIPS.map((r) => <option key={r.value} value={r.value}>{r.label}</option>)}
        </select>
      </Field>
      <Field label="Join" htmlFor="model-reference-join">
        <select
          id="model-reference-join"
          aria-label="Join"
          className={NATIVE_SELECT}
          value={join.join}
          onChange={(e) => onChange(setJoin(def, index, { join: e.target.value as JoinKind }))}
        >
          {JOINS.map((j) => <option key={j.value} value={j.value}>{j.label}</option>)}
        </select>
      </Field>
      <Value label="Label">{joinLabel(join)}</Value>
      <label className="tw:flex tw:items-center tw:gap-2 tw:text-sm" htmlFor="model-reference-hidden">
        <input
          id="model-reference-hidden"
          type="checkbox"
          checked={Boolean(join.hidden)}
          onChange={(e) => onChange(setJoin(def, index, { hidden: e.target.checked }))}
        />
        Hidden on the canvas
      </label>
      <Button
        variant="destructive"
        size="sm"
        onClick={() => {
          onChange(removeJoin(def, index));
          onDeselect();
        }}
      >
        Delete reference
      </Button>
    </Section>
  );
}

// The right pane: the properties of what is selected, or of the model.
export function PropertiesPanel(props: PropertiesPanelProps) {
  const { selected } = props;
  return (
    <aside
      data-slot="model-properties"
      className="tw:flex tw:w-80 tw:shrink-0 tw:flex-col tw:overflow-y-auto tw:border-l tw:bg-card"
    >
      <h2 className="tw:m-0 tw:border-b tw:px-3 tw:py-2.5 tw:text-sm tw:font-semibold tw:tracking-wide tw:uppercase">
        {TITLE[selected.kind]}
      </h2>
      {selected.kind === 'model' ? <ModelProperties {...props} /> : null}
      {selected.kind === 'table' ? <TableProperties {...props} alias={selected.alias} /> : null}
      {selected.kind === 'reference' ? <ReferenceProperties {...props} index={selected.index} /> : null}
    </aside>
  );
}
