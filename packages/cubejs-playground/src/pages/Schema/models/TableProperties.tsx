import {
  DragEvent, KeyboardEvent, useEffect, useRef, useState,
} from 'react';
import {
  ChevronDown, ChevronRight, CircleAlert, Code, GripVertical, Plus, TriangleAlert, X,
} from 'lucide-react';

import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Field } from '@/components/kit';
import { cn } from '@/lib/utils';

import { Dialect, dialectSpec, typesOf } from './dialect';
import type { Issue } from './validate';
import {
  DraftColumn, ModelDefinition, ModelTable, renameTable, setColor, setTableTransparency, setTableComment,
} from './modelGraph';
import {
  addColumn, moveColumn, removeColumn, renameColumn, setChecks, setColumn, setIndexes, setPrimaryKeyName,
  setTableScripts, setUniqueKeys,
} from './physical';
import {
  Check, DraftText, Palette, Section, Value,
} from './PanelParts';

type Columns = { name: string; type: string }[];

export type TablePropertiesProps = {
  def: ModelDefinition;
  alias: string;
  dialect: Dialect;
  columnsOf(t: ModelTable): Columns;
  issues: Issue[];
  onChange(next: ModelDefinition): void;
  onRenamed(from: string, to: string): void;
  // Bumped to focus the first column's name (a double click on the table).
  focusKey?: number;
  onSqlPreview?(alias: string): void;
};

const ICON = 'tw:flex tw:size-6 tw:shrink-0 tw:cursor-pointer tw:items-center tw:justify-center tw:rounded tw:border-0 tw:bg-transparent tw:p-0 tw:text-muted-foreground tw:hover:bg-accent tw:hover:text-foreground';

// The table's name, kept as typed until Enter or leaving the field.
function TableName({
  def, alias, onChange, onRenamed,
}: Pick<TablePropertiesProps, 'def' | 'alias' | 'onChange' | 'onRenamed'>) {
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

// `list` with its `k`th item patched.
function editAt<T>(list: T[], k: number, patch: Partial<T>): T[] {
  return list.map((x, j) => (j === k ? { ...x, ...patch } : x));
}

// The first `<base><n>` no name of `taken` has.
function freeName(base: string, taken: string[]): string {
  let n = 1;
  while (taken.includes(`${base}${n}`)) {
    n += 1;
  }
  return `${base}${n}`;
}

// A column's row of the grid, and its details under it when open: Redgate's
// "…" (comment, default value, check, auto increment, MySQL's switches).
function ColumnRow({
  column, index, count, dialect, alias, open, onToggle, onPatch, onRename, onRemove, onMove,
}: {
  column: DraftColumn;
  index: number;
  count: number;
  dialect: Dialect;
  alias: string;
  open: boolean;
  onToggle(): void;
  onPatch(patch: Partial<DraftColumn>): void;
  onRename(name: string): void;
  onRemove(): void;
  onMove(from: number, to: number): void;
}) {
  const n = index + 1;
  const onHandleKey = (e: KeyboardEvent) => {
    if (e.altKey && e.key === 'ArrowDown' && index < count - 1) {
      e.preventDefault();
      onMove(index, index + 1);
    } else if (e.altKey && e.key === 'ArrowUp' && index > 0) {
      e.preventDefault();
      onMove(index, index - 1);
    }
  };
  return (
    <li
      data-slot="model-column-row"
      className="tw:flex tw:flex-col tw:gap-1"
      onDragOver={(e: DragEvent) => e.preventDefault()}
      onDrop={(e: DragEvent) => {
        const from = Number(e.dataTransfer.getData('text/x-model-column'));
        if (!Number.isNaN(from) && from !== index) {
          onMove(from, index);
        }
      }}
    >
      <div className="tw:grid tw:grid-cols-[auto_1fr_1fr_auto_auto_auto_auto] tw:items-center tw:gap-1">
        <span
          role="button"
          tabIndex={0}
          draggable
          aria-label={`Move ${column.name}`}
          title="Drag, or Alt+↑/↓, to move"
          className={cn(ICON, 'tw:cursor-grab')}
          onKeyDown={onHandleKey}
          onDragStart={(e) => e.dataTransfer.setData('text/x-model-column', String(index))}
        >
          <GripVertical className="tw:size-3.5" />
        </span>
        <DraftText
          multiline={false}
          label={`Column ${n} name`}
          value={column.name}
          onCommit={(name) => onRename(name.trim())}
        />
        <DraftText
          multiline={false}
          label={`Column ${n} type`}
          value={column.type}
          list={`model-types-${dialect ?? 'portable'}`}
          onCommit={(type) => onPatch({ type: type.trim() })}
        />
        <input
          type="checkbox"
          aria-label={`Column ${n} allows nulls`}
          title="N: allows nulls"
          checked={column.nullable}
          onChange={(e) => onPatch({ nullable: e.target.checked })}
        />
        <input
          type="checkbox"
          aria-label={`Column ${n} primary key`}
          title="PK: in the primary key"
          checked={column.primaryKey}
          onChange={(e) => onPatch({ primaryKey: e.target.checked, ...(e.target.checked ? { nullable: false } : {}) })}
        />
        <button
          type="button"
          aria-label={`Column ${n} details`}
          aria-expanded={open}
          className={ICON}
          onClick={onToggle}
        >
          {open ? <ChevronDown className="tw:size-3.5" /> : <ChevronRight className="tw:size-3.5" />}
        </button>
        <button type="button" aria-label={`Remove column ${n}`} title={`Remove ${alias}.${column.name}`} className={ICON} onClick={onRemove}>
          <X className="tw:size-3.5" />
        </button>
      </div>
      {open ? (
        <div className="tw:ml-7 tw:flex tw:flex-col tw:gap-1.5 tw:rounded tw:border tw:p-2" data-slot="model-column-details">
          <DraftText multiline={false} label={`Column ${n} comment`} placeholder="Comment" value={column.comment ?? ''} onCommit={(comment) => onPatch({ comment })} />
          <DraftText multiline={false} label={`Column ${n} default value`} placeholder="Default value" value={column.default ?? ''} onCommit={(v) => onPatch({ default: v })} />
          <DraftText multiline={false} label={`Column ${n} check expression`} placeholder="Check expression" value={column.check ?? ''} onCommit={(check) => onPatch({ check })} />
          <Check label={`Column ${n} auto increment`} checked={Boolean(column.autoIncrement)} onChange={(v) => onPatch({ autoIncrement: v })}>Auto increment</Check>
          {dialectSpec(dialect)?.features.unsigned ? (
            <>
              <Check label={`Column ${n} unsigned`} checked={Boolean(column.unsigned)} onChange={(v) => onPatch({ unsigned: v })}>Unsigned</Check>
              <Check label={`Column ${n} zerofill`} checked={Boolean(column.zerofill)} onChange={(v) => onPatch({ zerofill: v })}>Zerofill</Check>
            </>
          ) : null}
        </div>
      ) : null}
    </li>
  );
}

export function IssuesSection({ issues, heading = 'Table issues' }: { issues: Issue[]; heading?: string }) {
  const errors = issues.filter((i) => i.severity === 'error');
  const warnings = issues.filter((i) => i.severity === 'warning');
  return (
    <Section title={heading} slot="model-table-issues">
      {[{ title: 'Errors', list: errors, Icon: CircleAlert, tone: 'tw:text-destructive' },
        { title: 'Warnings', list: warnings, Icon: TriangleAlert, tone: 'tw:text-amber-500' }].map(({
        title, list, Icon, tone,
      }) => (
        <div key={title} className="tw:flex tw:flex-col tw:gap-1 tw:text-sm">
          <span className="tw:flex tw:items-center tw:gap-1.5 tw:font-medium">
            <Icon className={cn('tw:size-4', tone)} />
            {`${title} (${list.length})`}
          </span>
          <ul className="tw:m-0 tw:list-none tw:p-0 tw:pl-6 tw:text-xs tw:text-muted-foreground">
            {list.map((i) => <li key={`${i.rule}:${i.message}`}>{i.message}</li>)}
          </ul>
        </div>
      ))}
    </Section>
  );
}

// Redgate's table properties: General, Columns, Primary key, Alternate
// keys, Indexes, Checks, Additional SQL scripts, Format and Table issues.
// A table of the database shows its columns read-only.
export function TableProperties({
  def, alias, dialect, columnsOf, issues, onChange, onRenamed, focusKey, onSqlPreview,
}: TablePropertiesProps) {
  const [details, setDetails] = useState<number | null>(null);
  const columnList = useRef<HTMLUListElement>(null);
  useEffect(() => {
    if (focusKey) {
      columnList.current?.querySelector<HTMLInputElement>('input')?.focus();
    }
  }, [focusKey]);
  const table = def.tables.find((t) => t.alias === alias);
  if (!table) {
    return null;
  }
  const { draft } = table;
  const names = draft ? draft.columns.map((c) => c.name) : columnsOf(table).map((c) => c.name);
  const typeList = typesOf(dialect);
  const keys = draft?.uniqueKeys ?? [];
  const indexes = draft?.indexes ?? [];
  const checks = draft?.checks ?? [];
  const taken = [...keys.map((k) => k.name), ...indexes.map((i) => i.name), ...checks.map((c) => c.name)];
  return (
    <>
      <Section
        title="General"
        action={onSqlPreview ? (
          <Button variant="link" size="sm" className="tw:self-end tw:p-0" onClick={() => onSqlPreview(alias)}>
            <Code />
            SQL preview
          </Button>
        ) : null}
      >
        <TableName def={def} alias={alias} onChange={onChange} onRenamed={onRenamed} />
        <Value label="Source">{`${table.schema}.${table.table}${draft ? ' (new)' : ''}`}</Value>
        <Field label="Comment" htmlFor="model-table-comment">
          <DraftText
            key={alias}
            id="model-table-comment"
            label="Table comment"
            value={table.comment ?? ''}
            onCommit={(text) => onChange(setTableComment(def, alias, text))}
          />
        </Field>
      </Section>

      <Section title="Columns">
        {draft ? (
          <>
            <datalist id={`model-types-${dialect ?? 'portable'}`}>
              {typeList.map((t) => <option key={t} value={t}>{t}</option>)}
            </datalist>
            <div className="tw:grid tw:grid-cols-[auto_1fr_1fr_auto_auto_auto_auto] tw:gap-1 tw:pl-1 tw:text-xs tw:text-muted-foreground">
              <span className="tw:w-6" />
              <span>Name</span>
              <span>Type</span>
              <span>N</span>
              <span>PK</span>
              <span className="tw:w-6" />
              <span className="tw:w-6" />
            </div>
            <ul ref={columnList} className="tw:m-0 tw:flex tw:list-none tw:flex-col tw:gap-1 tw:p-0">
              {draft.columns.map((c, i) => (
                <ColumnRow
                  // A column's place is its identity while it is renamed.
                  // eslint-disable-next-line react/no-array-index-key
                  key={i}
                  column={c}
                  index={i}
                  count={draft.columns.length}
                  dialect={dialect}
                  alias={alias}
                  open={details === i}
                  onToggle={() => setDetails(details === i ? null : i)}
                  onPatch={(patch) => onChange(setColumn(def, alias, i, patch))}
                  onRename={(name) => name && onChange(renameColumn(def, alias, c.name, name))}
                  onRemove={() => onChange(removeColumn(def, alias, c.name))}
                  onMove={(from, to) => onChange(moveColumn(def, alias, from, to))}
                />
              ))}
            </ul>
            <Button variant="outline" size="sm" aria-label="Add column" onClick={() => onChange(addColumn(def, alias, dialect))}>
              <Plus />
              Add column
            </Button>
          </>
        ) : (
          <>
            <ul className="tw:m-0 tw:flex tw:list-none tw:flex-col tw:gap-1 tw:p-0">
              {columnsOf(table).map((c) => (
                <li key={c.name} data-slot="model-property-column" className="tw:grid tw:grid-cols-[1fr_auto] tw:gap-x-3 tw:text-sm">
                  <span className="tw:truncate">{c.name}</span>
                  <span className="tw:text-muted-foreground">{c.type}</span>
                </li>
              ))}
            </ul>
            <p className="tw:m-0 tw:text-xs tw:text-muted-foreground">The structure comes from the database.</p>
          </>
        )}
      </Section>

      {draft ? (
        <>
          <Section title="Primary key">
            <DraftText
              multiline={false}
              label="Primary key name"
              placeholder="Name"
              value={draft.primaryKeyName ?? ''}
              onCommit={(name) => onChange(setPrimaryKeyName(def, alias, name.trim()))}
            />
            <Value label="Columns">
              {draft.columns.filter((c) => c.primaryKey).map((c) => c.name).join(', ') || 'None'}
            </Value>
          </Section>

          <Section title="Alternate (unique) keys">
            {keys.map((key, k) => (
              // eslint-disable-next-line react/no-array-index-key
              <div key={k} className="tw:flex tw:flex-col tw:gap-1 tw:rounded tw:border tw:p-2">
                <div className="tw:flex tw:items-center tw:gap-1">
                  <DraftText
                    multiline={false}
                    label={`Alternate key ${k + 1} name`}
                    value={key.name}
                    onCommit={(name) => onChange(setUniqueKeys(def, alias, editAt(keys, k, { name: name.trim() })))}
                  />
                  <button type="button" aria-label={`Remove alternate key ${k + 1}`} className={ICON} onClick={() => onChange(setUniqueKeys(def, alias, keys.filter((_, j) => j !== k)))}>
                    <X className="tw:size-3.5" />
                  </button>
                </div>
                <div className="tw:flex tw:flex-wrap tw:gap-2">
                  {names.map((name) => (
                    <Check
                      key={name}
                      label={`Alternate key ${k + 1} column ${name}`}
                      checked={key.columns.includes(name)}
                      onChange={(on) => onChange(setUniqueKeys(def, alias, editAt(keys, k, {
                        columns: on
                          ? names.filter((c) => c === name || key.columns.includes(c))
                          : key.columns.filter((c) => c !== name),
                      })))}
                    >
                      {name}
                    </Check>
                  ))}
                </div>
              </div>
            ))}
            <Button
              variant="outline"
              size="sm"
              aria-label="Add alternate key"
              onClick={() => onChange(setUniqueKeys(def, alias, [...keys, { name: freeName(`${table.table}_ak_`, taken), columns: [] }]))}
            >
              <Plus />
              Add key
            </Button>
          </Section>

          <Section title="Indexes">
            {indexes.map((index, k) => {
              const patchIndex = (next: typeof index) => onChange(setIndexes(def, alias, editAt(indexes, k, next)));
              return (
                // eslint-disable-next-line react/no-array-index-key
                <div key={k} className="tw:flex tw:flex-col tw:gap-1 tw:rounded tw:border tw:p-2">
                  <div className="tw:flex tw:items-center tw:gap-1">
                    <DraftText
                      multiline={false}
                      label={`Index ${k + 1} name`}
                      value={index.name}
                      onCommit={(name) => patchIndex({ ...index, name: name.trim() })}
                    />
                    <Check label={`Index ${k + 1} unique`} checked={index.unique} onChange={(unique) => patchIndex({ ...index, unique })}>Unique</Check>
                    <button type="button" aria-label={`Remove index ${k + 1}`} className={ICON} onClick={() => onChange(setIndexes(def, alias, indexes.filter((_, j) => j !== k)))}>
                      <X className="tw:size-3.5" />
                    </button>
                  </div>
                  {names.map((name) => {
                    const at = index.columns.find((c) => c.name === name);
                    return (
                      <div key={name} className="tw:flex tw:items-center tw:gap-2">
                        <Check
                          label={`Index ${k + 1} column ${name}`}
                          checked={Boolean(at)}
                          onChange={(on) => patchIndex({
                            ...index,
                            columns: on ? [...index.columns, { name }] : index.columns.filter((c) => c.name !== name),
                          })}
                        >
                          {name}
                        </Check>
                        {at ? (
                          <button
                            type="button"
                            aria-label={`Index ${k + 1} ${name} descending`}
                            aria-pressed={Boolean(at.desc)}
                            className="tw:cursor-pointer tw:rounded tw:border tw:bg-transparent tw:px-1 tw:text-[10px] tw:text-foreground"
                            onClick={() => patchIndex({
                              ...index,
                              columns: index.columns.map((c) => (
                                c.name === name ? { name, ...(c.desc ? {} : { desc: true }) } : c
                              )),
                            })}
                          >
                            {at.desc ? 'DESC' : 'ASC'}
                          </button>
                        ) : null}
                      </div>
                    );
                  })}
                </div>
              );
            })}
            <Button
              variant="outline"
              size="sm"
              aria-label="Add index"
              onClick={() => onChange(setIndexes(def, alias, [...indexes, { name: freeName(`${table.table}_idx_`, taken), columns: [], unique: false }]))}
            >
              <Plus />
              Add index
            </Button>
          </Section>

          <Section title="Checks">
            {checks.map((check, k) => (
              // eslint-disable-next-line react/no-array-index-key
              <div key={k} className="tw:flex tw:items-center tw:gap-1">
                <DraftText
                  multiline={false}
                  label={`Check ${k + 1} name`}
                  value={check.name}
                  onCommit={(name) => onChange(setChecks(def, alias, editAt(checks, k, { name: name.trim() })))}
                />
                <DraftText
                  multiline={false}
                  label={`Check ${k + 1} expression`}
                  placeholder="Expression"
                  value={check.expression}
                  onCommit={(expression) => onChange(setChecks(def, alias, editAt(checks, k, { expression })))}
                />
                <button type="button" aria-label={`Remove check ${k + 1}`} className={ICON} onClick={() => onChange(setChecks(def, alias, checks.filter((_, j) => j !== k)))}>
                  <X className="tw:size-3.5" />
                </button>
              </div>
            ))}
            <Button
              variant="outline"
              size="sm"
              aria-label="Add check"
              onClick={() => onChange(setChecks(def, alias, [...checks, { name: freeName(`${table.table}_ck_`, taken), expression: '' }]))}
            >
              <Plus />
              Add check
            </Button>
          </Section>

          <Section title="Additional SQL scripts" defaultOpen={false}>
            <Field label="Before the CREATE TABLE" htmlFor="model-table-before">
              <DraftText id="model-table-before" label="Script before the table" value={draft.scriptBefore ?? ''} onCommit={(before) => onChange(setTableScripts(def, alias, { before }))} />
            </Field>
            <Field label="After the CREATE TABLE" htmlFor="model-table-after">
              <DraftText id="model-table-after" label="Script after the table" value={draft.scriptAfter ?? ''} onCommit={(after) => onChange(setTableScripts(def, alias, { after }))} />
            </Field>
          </Section>
        </>
      ) : null}

      <Section title="Format">
        <Field label="Header color">
          <Palette
            slot="model-color-palette"
            none
            value={table.color}
            onPick={(c) => onChange(setColor(def, alias, c))}
            transparency={table.colorTransparency}
            onTransparency={(t) => onChange(setTableTransparency(def, alias, 'colorTransparency', t))}
          />
        </Field>
        <Field label="Body color">
          <Palette
            slot="model-body-palette"
            none
            value={table.bodyColor}
            onPick={(c) => onChange(setColor(def, alias, c, 'bodyColor'))}
            transparency={table.bodyTransparency}
            onTransparency={(t) => onChange(setTableTransparency(def, alias, 'bodyTransparency', t))}
          />
        </Field>
      </Section>

      <IssuesSection issues={issues} />
    </>
  );
}
