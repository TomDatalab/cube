import { useEffect, useState } from 'react';
import { Code, X } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Field } from '@/components/kit';

import { CodeEditor } from '../NumberedCode';
import { Dialect, PORTABLE, TYPES } from './dialect';
import type { ModelDefinition, ViewColumn } from './modelGraph';
import {
  nodeIdOf, renameView, setView, setViewColumnType, setViewSchema,
} from './objects';
import { Check, DraftText, Palette, Section } from './PanelParts';
import { IssuesSection } from './TableProperties';
import type { Issue } from './validate';

type Columns = { name: string; type: string }[];

export type ViewPropertiesProps = {
  def: ModelDefinition;
  id: string;
  dialect: Dialect;
  columnsOf?: (t: ModelDefinition['tables'][number]) => Columns;
  issues: Issue[];
  onChange(next: ModelDefinition): void;
  focusKey?: number;
  onSqlPreview?(id: string): void;
  // Kept for the editor's contract: a view keeps its id when renamed, so the
  // selection stays on it without being told.
  onRenamed?(from: string, to: string): void;
};

// The view's name, kept as typed until Enter or leaving the field.
function ViewName({ def, id, onChange }: Pick<ViewPropertiesProps, 'def' | 'id' | 'onChange'>) {
  const name = def.views?.find((v) => v.id === id)?.name ?? '';
  const [value, setValue] = useState(name);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    setValue(name);
    setError(null);
  }, [id, name]);
  const commit = () => {
    const result = renameView(def, id, value.trim());
    if (result.error) {
      setError(result.error);
      return;
    }
    setError(null);
    if (result.def !== def) {
      onChange(result.def);
    }
  };
  return (
    <Field label="Name" htmlFor="model-view-name">
      <Input
        id="model-view-name"
        aria-label="View name"
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

// A text field whose commit may be refused: the refusal shows under it and
// the model keeps its value.
function CheckedText({
  id, label, value, check, list,
}: {
  id?: string;
  label: string;
  value: string;
  check(next: string): string | null;
  list?: string;
}) {
  const [error, setError] = useState<string | null>(null);
  useEffect(() => setError(null), [value]);
  return (
    <>
      <DraftText
        id={id}
        label={label}
        multiline={false}
        list={list}
        value={value}
        onCommit={(next) => setError(check(next.trim()))}
      />
      {error ? <p role="alert" className="tw:m-0 tw:text-xs tw:text-destructive">{error}</p> : null}
    </>
  );
}

// The query as typed, written to the model when the editor is left: one
// step of undo.
function QueryEditor({ value, onCommit }: { value: string; onCommit(next: string): void }) {
  const [text, setText] = useState(value);
  useEffect(() => setText(value), [value]);
  return (
    <div
      onBlur={() => {
        if (text !== value) {
          onCommit(text);
        }
      }}
    >
      <CodeEditor value={text} rows={8} onChange={setText} />
    </div>
  );
}

// The first `column<n>` the view has not taken.
function freeColumn(columns: ViewColumn[]): string {
  let n = columns.length + 1;
  while (columns.some((c) => c.name === `column${n}`)) {
    n += 1;
  }
  return `column${n}`;
}

// Redgate's view properties: General, SQL query, Columns, Dependencies,
// Additional SQL scripts and Issues.
export function ViewProperties({
  def, id, dialect, issues, onChange, onSqlPreview, focusKey,
}: ViewPropertiesProps) {
  useEffect(() => {
    if (focusKey) {
      (document.getElementById('model-view-name') as HTMLInputElement | null)?.focus();
    }
  }, [focusKey, id]);
  const view = def.views?.find((v) => v.id === id);
  if (!view) {
    return null;
  }
  const typeList = dialect ? TYPES[dialect] : PORTABLE;
  const patch = (next: Parameters<typeof setView>[2]) => onChange(setView(def, id, next));
  const editColumn = (k: number, change: Partial<ViewColumn>) => patch({
    columns: view.columns.map((c, j) => (j === k ? { ...c, ...change } : c)),
  });
  const toggle = (key: 'dependsOnTables' | 'dependsOnViews', value: string, on: boolean) => patch({
    [key]: on ? [...view[key], value] : view[key].filter((x) => x !== value),
  });
  return (
    <>
      <Section
        title="General"
        action={dialect && onSqlPreview ? (
          <Button variant="link" size="sm" className="tw:self-end tw:p-0" onClick={() => onSqlPreview(nodeIdOf('view', id))}>
            <Code />
            SQL preview
          </Button>
        ) : null}
      >
        <ViewName def={def} id={id} onChange={onChange} />
        <Field label="Schema" htmlFor="model-view-schema">
          <CheckedText
            key={`${id}:schema`}
            id="model-view-schema"
            label="View schema"
            value={view.schema}
            check={(schema) => {
              const result = setViewSchema(def, id, schema);
              if (!result.error) {
                onChange(result.def);
              }
              return result.error ?? null;
            }}
          />
        </Field>
        <Field label="Comment" htmlFor="model-view-comment">
          <DraftText
            key={`${id}:comment`}
            id="model-view-comment"
            label="View comment"
            value={view.comment ?? ''}
            onCommit={(comment) => patch({ comment: comment || undefined })}
          />
        </Field>
      </Section>

      <Section title="SQL query">
        <QueryEditor key={id} value={view.sql} onCommit={(sql) => patch({ sql })} />
      </Section>

      <Section title="Columns">
        <datalist id={`model-view-types-${dialect ?? 'portable'}`}>
          {typeList.map((t) => <option key={t} value={t}>{t}</option>)}
        </datalist>
        <ul className="tw:m-0 tw:flex tw:list-none tw:flex-col tw:gap-1 tw:p-0">
          {view.columns.map((c, k) => (
            // A column's place is its identity while it is renamed.
            // eslint-disable-next-line react/no-array-index-key
            <li key={`${id}:${k}`} className="tw:grid tw:grid-cols-[1fr_1fr_auto] tw:items-center tw:gap-1">
              <DraftText
                label={`View column ${k + 1} name`}
                multiline={false}
                value={c.name}
                onCommit={(name) => editColumn(k, { name })}
              />
              <CheckedText
                label={`View column ${k + 1} type`}
                list={`model-view-types-${dialect ?? 'portable'}`}
                value={c.type}
                check={(type) => {
                  const result = setViewColumnType(def, id, k, type, dialect);
                  if (!result.error) {
                    onChange(result.def);
                  }
                  return result.error ?? null;
                }}
              />
              <button
                type="button"
                aria-label={`Remove view column ${k + 1}`}
                className="tw:flex tw:size-6 tw:cursor-pointer tw:items-center tw:justify-center tw:rounded tw:border-0 tw:bg-transparent tw:p-0 tw:text-muted-foreground tw:hover:bg-accent tw:hover:text-foreground"
                onClick={() => patch({ columns: view.columns.filter((_, j) => j !== k) })}
              >
                <X className="tw:size-3.5" />
              </button>
            </li>
          ))}
        </ul>
        <Button
          variant="outline"
          size="sm"
          onClick={() => patch({ columns: [...view.columns, { name: freeColumn(view.columns), type: 'text' }] })}
        >
          + Add
        </Button>
      </Section>

      <Section title="Dependencies">
        {def.tables.length + (def.views?.length ?? 0) <= 1 ? (
          <span className="tw:text-xs tw:text-muted-foreground">Nothing to depend on yet</span>
        ) : null}
        {def.tables.map((t) => (
          <Check
            key={`t:${t.alias}`}
            label={`Depends on ${t.alias}`}
            checked={view.dependsOnTables.includes(t.alias)}
            onChange={(on) => toggle('dependsOnTables', t.alias, on)}
          >
            {t.alias}
          </Check>
        ))}
        {(def.views ?? []).filter((v) => v.id !== id).map((v) => (
          <Check
            key={`v:${v.id}`}
            label={`Depends on ${v.name}`}
            checked={view.dependsOnViews.includes(v.id)}
            onChange={(on) => toggle('dependsOnViews', v.id, on)}
          >
            {v.name}
          </Check>
        ))}
      </Section>

      <Section title="Additional SQL scripts" defaultOpen={false}>
        <Field label="Before the CREATE VIEW" htmlFor="model-view-before">
          <DraftText key={`${id}:b`} id="model-view-before" label="Script before the view" value={view.scriptBefore ?? ''} onCommit={(t) => patch({ scriptBefore: t || undefined })} />
        </Field>
        <Field label="After the CREATE VIEW" htmlFor="model-view-after">
          <DraftText key={`${id}:a`} id="model-view-after" label="Script after the view" value={view.scriptAfter ?? ''} onCommit={(t) => patch({ scriptAfter: t || undefined })} />
        </Field>
      </Section>

      <Section title="Format">
        <Field label="Color">
          <Palette
            slot="model-view-palette"
            none
            value={view.color}
            onPick={(color) => patch({ color })}
            transparency={view.colorTransparency}
            onTransparency={(colorTransparency) => patch({ colorTransparency })}
          />
        </Field>
      </Section>

      <IssuesSection issues={issues} heading="Issues" />
    </>
  );
}
