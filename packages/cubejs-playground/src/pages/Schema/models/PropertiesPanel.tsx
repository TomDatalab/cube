import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Field, SimpleSelect, Spinner } from '@/components/kit';

import type { Dialect } from './dialect';
import type { Issue } from './validate';
import { DraftText, Section, Value } from './PanelParts';
import { TableProperties } from './TableProperties';

import {
  JoinKind, ModelDefinition, ModelTable, Relationship, joinLabel, removeJoin, setJoin, setPrimary,
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
  dialect: Dialect;
  issues: Issue[];
  // Bumped to focus the selected table's first column.
  focusKey?: number;
  onSqlPreview?(alias: string): void;
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
        <DraftText
          id="model-description"
          label="Model description"
          value={def.description ?? ''}
          onCommit={(text) => {
            const { description: _drop, ...rest } = def;
            onChange(text ? { ...rest, description: text } : rest);
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
      {selected.kind === 'table' ? (
        <TableProperties
          def={props.def}
          alias={selected.alias}
          dialect={props.dialect}
          columnsOf={props.columnsOf}
          issues={props.issues.filter((i) => i.target.kind === 'table' && i.target.alias === selected.alias)}
          onChange={props.onChange}
          onRenamed={props.onRenamed}
          focusKey={props.focusKey}
          onSqlPreview={props.onSqlPreview}
        />
      ) : null}
      {selected.kind === 'reference' ? <ReferenceProperties {...props} index={selected.index} /> : null}
    </aside>
  );
}
