import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Field, Spinner } from '@/components/kit';

import type { Dialect } from './dialect';
import type { Issue } from './validate';
import {
  Check, DraftText, Section, Value,
} from './PanelParts';
import { RULES } from './validate';
import { setAllRules, setSetting } from './physical';
import { TableProperties } from './TableProperties';
import { ReferenceProperties } from './ReferenceProperties';
import { NoteProperties } from './NoteProperties';
import { AreaProperties } from './AreaProperties';
import { ViewProperties } from './ViewProperties';
import { CubeProperties } from './CubeProperties';
import { CubeJoinProperties } from './CubeJoinProperties';

import {
  ModelDefinition, ModelTable,
} from './modelGraph';

type Columns = { name: string; type: string }[];

type ModelSelected = { kind: 'model' };
type TableSelected = { kind: 'table'; alias: string };
type ReferenceSelected = { kind: 'reference'; index: number };
type ObjectSelected = { kind: 'note' | 'area' | 'view'; id: string };
// A cube by its id, a cube join by its id.
type CubeSelected = { kind: 'cube' | 'cubeJoin'; id: string };
export type Selected = ModelSelected | TableSelected | ReferenceSelected | ObjectSelected | CubeSelected;

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
  // A table alias, or a node id such as view:v1.
  onSqlPreview?(id: string): void;
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
  note: 'Text note properties',
  area: 'Subject area properties',
  view: 'View properties',
  cube: 'Cube properties',
  cubeJoin: 'Cube join properties',
};

// `1 table, 2 views, 1 note`: zero counts are left out, except tables.
function sizeOf(def: ModelDefinition): string {
  const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;
  return [
    count(def.tables.length, 'table', 'tables'),
    ...[
      [def.views?.length ?? 0, 'view', 'views'],
      [def.notes?.length ?? 0, 'note', 'notes'],
      [def.areas?.length ?? 0, 'area', 'areas'],
    ].filter(([n]) => n).map(([n, one, many]) => count(n as number, one as string, many as string)),
  ].join(', ');
}

function ModelProperties({
  def, name, onChange, onName, onDeleteModel, deleting, busy,
}: PropertiesPanelProps) {
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
      <Value label="Size">{sizeOf(def)}</Value>
      <Button variant="destructive" size="sm" disabled={busy} onClick={onDeleteModel}>
        {deleting ? <Spinner className="tw:text-current" /> : null}
        Delete model
      </Button>
    </Section>
  );
}

const SCRIPTS: { key: 'createBefore' | 'createAfter' | 'dropBefore' | 'dropAfter'; label: string }[] = [
  { key: 'createBefore', label: 'Beginning of the create script' },
  { key: 'createAfter', label: 'End of the create script' },
  { key: 'dropBefore', label: 'Beginning of the drop script' },
  { key: 'dropAfter', label: 'End of the drop script' },
];

// Redgate's other model sections: scripts, validation rules, format and
// SQL generation, each closed until opened.
function ModelSettings({ def, onChange }: Pick<PropertiesPanelProps, 'def' | 'onChange'>) {
  const s = def.settings ?? {};
  const groups = [...new Set(RULES.map((r) => r.group))];
  return (
    <>
      <Section title="Additional SQL scripts" defaultOpen={false}>
        {SCRIPTS.map(({ key, label }) => (
          <Field key={key} label={label} htmlFor={`model-script-${key}`}>
            <DraftText
              id={`model-script-${key}`}
              label={label}
              value={s.scripts?.[key] ?? ''}
              onCommit={(text) => onChange(setSetting(def, 'scripts', key, text, ''))}
            />
          </Field>
        ))}
      </Section>
      <Section title="Model validation settings" defaultOpen={false}>
        <div className="tw:flex tw:items-center tw:gap-2 tw:text-sm">
          <Button variant="link" size="sm" className="tw:p-0" onClick={() => onChange(setAllRules(def, RULES.map((r) => r.id), true))}>
            Select all
          </Button>
          <span className="tw:text-muted-foreground">|</span>
          <Button variant="link" size="sm" className="tw:p-0" onClick={() => onChange(setAllRules(def, RULES.map((r) => r.id), false))}>
            Deselect all
          </Button>
        </div>
        {groups.map((group) => (
          <div key={group} className="tw:flex tw:flex-col tw:gap-1">
            <span className="tw:text-xs tw:font-semibold">{group}</span>
            {RULES.filter((r) => r.group === group).map((rule) => (
              <Check
                key={rule.id}
                label={`${rule.label} (${rule.severity}s)`}
                checked={s.validation?.[rule.id] !== false}
                onChange={(on) => onChange(setSetting(def, 'validation', rule.id, on, true))}
              >
                {`${rule.label} (${rule.severity}s)`}
              </Check>
            ))}
          </div>
        ))}
      </Section>
      <Section title="Format" defaultOpen={false}>
        <Check
          label="Show column types"
          checked={s.format?.showTypes ?? true}
          onChange={(on) => onChange(setSetting(def, 'format', 'showTypes', on, true))}
        >
          Show column types
        </Check>
        <Check
          label="Show N (nullable)"
          checked={s.format?.showNullable ?? false}
          onChange={(on) => onChange(setSetting(def, 'format', 'showNullable', on, false))}
        >
          Show N (nullable)
        </Check>
      </Section>
      <Section title="SQL generation settings" defaultOpen={false}>
        <Check
          label="Generate DROP statements"
          checked={s.sqlGeneration?.drop ?? false}
          onChange={(on) => onChange(setSetting(def, 'sqlGeneration', 'drop', on, false))}
        >
          Generate DROP statements
        </Check>
        <Check
          label="Generate comments"
          checked={s.sqlGeneration?.comments ?? true}
          onChange={(on) => onChange(setSetting(def, 'sqlGeneration', 'comments', on, true))}
        >
          Generate comments
        </Check>
      </Section>
    </>
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
      {selected.kind === 'model' ? (
        <>
          <ModelProperties {...props} />
          <ModelSettings def={props.def} onChange={props.onChange} />
        </>
      ) : null}
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
      {selected.kind === 'note' ? (
        <NoteProperties def={props.def} id={selected.id} onChange={props.onChange} focusKey={props.focusKey} />
      ) : null}
      {selected.kind === 'area' ? (
        <AreaProperties def={props.def} id={selected.id} onChange={props.onChange} focusKey={props.focusKey} />
      ) : null}
      {selected.kind === 'view' ? (
        <ViewProperties
          def={props.def}
          id={selected.id}
          dialect={props.dialect}
          columnsOf={props.columnsOf}
          issues={props.issues.filter((i) => i.target.kind === 'view' && i.target.id === selected.id)}
          onChange={props.onChange}
          focusKey={props.focusKey}
          onSqlPreview={props.onSqlPreview}
        />
      ) : null}
      {selected.kind === 'cube' ? (
        <CubeProperties
          def={props.def}
          id={selected.id}
          columnsOf={props.columnsOf}
          issues={props.issues.filter((i) => i.target.kind === 'cube' && i.target.id === selected.id)}
          onChange={props.onChange}
          focusKey={props.focusKey}
        />
      ) : null}
      {selected.kind === 'cubeJoin' ? (
        <CubeJoinProperties
          def={props.def}
          id={selected.id}
          columnsOf={props.columnsOf}
          issues={props.issues.filter((i) => i.target.kind === 'cubeJoin' && i.target.id === selected.id)}
          onChange={props.onChange}
        />
      ) : null}
    </aside>
  );
}
