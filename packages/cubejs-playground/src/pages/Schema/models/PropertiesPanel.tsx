import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Field, SimpleSelect, Spinner } from '@/components/kit';

import type { Dialect } from './dialect';
import type { Issue } from './validate';
import { DraftText, Section, Value } from './PanelParts';
import { TableProperties } from './TableProperties';
import { ReferenceProperties } from './ReferenceProperties';

import {
  ModelDefinition, ModelTable, setPrimary,
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
      {selected.kind === 'reference' ? (
        <ReferenceProperties
          def={props.def}
          index={selected.index}
          columnsOf={props.columnsOf}
          issues={props.issues.filter((i) => i.target.kind === 'reference' && i.target.id === props.def.joins[selected.index]?.id)}
          onChange={props.onChange}
          onDeselect={props.onDeselect}
        />
      ) : null}
    </aside>
  );
}
