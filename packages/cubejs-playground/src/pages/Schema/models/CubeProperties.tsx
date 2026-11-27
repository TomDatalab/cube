import { useEffect, useState } from 'react';
import { X } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Field } from '@/components/kit';

import {
  addMember, cubeReads, generateMembers, removeMember, renameCube, setCube, setCubeSource,
  setMember,
} from './cubes';
import type { MemberGroup } from './cubes';
import {
  DIMENSION_TYPES, MEASURE_FORMATS, MEASURE_TYPES, SOURCE_KINDS,
} from './modelGraph';
import type { CubeSource, ModelDefinition, ModelTable } from './modelGraph';
import {
  Check, DraftText, Palette, Section,
} from './PanelParts';
import { IssuesSection } from './TableProperties';
import type { Issue } from './validate';

type Columns = { name: string; type: string }[];

const SELECT = 'tw:h-7 tw:w-full tw:min-w-0 tw:rounded-md tw:border tw:bg-background tw:px-1 tw:text-xs tw:text-foreground';
const ICON = 'tw:flex tw:size-6 tw:shrink-0 tw:cursor-pointer tw:items-center tw:justify-center tw:rounded tw:border-0 tw:bg-transparent tw:p-0 tw:text-muted-foreground tw:hover:bg-accent tw:hover:text-foreground';

export type CubePropertiesProps = {
  def: ModelDefinition;
  id: string;
  columnsOf(t: ModelTable): Columns;
  issues: Issue[];
  onChange(next: ModelDefinition): void;
  focusKey?: number;
};

// A closed list as a select; a value outside the list is still shown.
function Pick({
  label, value, options, onPick, blank,
}: {
  label: string;
  value: string;
  options: readonly string[];
  onPick(next: string): void;
  // The text of the empty choice, when there is one.
  blank?: string;
}) {
  const shown = value && !options.includes(value) ? [...options, value] : options;
  return (
    <select aria-label={label} className={SELECT} value={value} onChange={(e) => onPick(e.target.value)}>
      {blank !== undefined ? <option value="">{blank}</option> : null}
      {shown.map((o) => <option key={o} value={o}>{o}</option>)}
    </select>
  );
}

// The cube's name, kept as typed until Enter or leaving the field.
function CubeName({ def, id, onChange }: Pick<CubePropertiesProps, 'def' | 'id' | 'onChange'>) {
  const name = def.cubes?.find((c) => c.id === id)?.name ?? '';
  const [value, setValue] = useState(name);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    setValue(name);
    setError(null);
  }, [id, name]);
  const commit = () => {
    const result = renameCube(def, id, value.trim());
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
    <Field label="Name" htmlFor="model-cube-name">
      <Input
        id="model-cube-name"
        aria-label="Cube name"
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

const GROUP_LABEL: Record<MemberGroup, { title: string; one: string }> = {
  dimensions: { title: 'Dimensions', one: 'Dimension' },
  measures: { title: 'Measures', one: 'Measure' },
  segments: { title: 'Segments', one: 'Segment' },
};

// Redgate's cube properties: General, Source, the members, Format, Issues.
export function CubeProperties({
  def, id, columnsOf, issues, onChange, focusKey,
}: CubePropertiesProps) {
  const [confirming, setConfirming] = useState(false);
  useEffect(() => {
    if (focusKey) {
      (document.getElementById('model-cube-name') as HTMLInputElement | null)?.focus();
    }
  }, [focusKey, id]);
  useEffect(() => setConfirming(false), [id]);
  const cube = def.cubes?.find((c) => c.id === id);
  if (!cube) {
    return null;
  }
  const patch = (next: Parameters<typeof setCube>[2]) => onChange(setCube(def, id, next));
  const columnsOfAlias = (alias: string) => {
    const t = def.tables.find((x) => x.alias === alias);
    return t ? columnsOf(t).map((c) => c.name) : [];
  };
  // `alias.column` of every table a joined cube reads, plain columns otherwise.
  const columns = cube.source.kind === 'sql'
    ? []
    : cubeReads(def, cube).flatMap((alias) => columnsOfAlias(alias)
      .map((c) => (cube.source.kind === 'joined' ? `${alias}.${c}` : c)));
  const { source } = cube;
  const hasMembers = cube.dimensions.length + cube.measures.length + cube.segments.length > 0;

  const changeKind = (kind: CubeSource['kind']) => {
    if (kind === 'sql') {
      onChange(setCubeSource(def, id, { kind, sql: source.sql ?? '', tables: source.tables ?? [] }));
    } else {
      onChange(setCubeSource(def, id, {
        kind, table: source.table ?? def.tables[0]?.alias, primaryKey: source.primaryKey,
      }));
    }
  };
  const generate = () => {
    onChange(generateMembers(def, id, (schema, table) => {
      const t = def.tables.find((x) => x.schema === schema && x.table === table);
      return t ? columnsOf(t) : undefined;
    }));
    setConfirming(false);
  };

  const rows = (group: MemberGroup) => {
    const { one } = GROUP_LABEL[group];
    const list = cube[group] as {
      name: string; title?: string; column?: string; sql?: string; primaryKey?: boolean; type?: string; format?: string;
    }[];
    const edit = (k: number, change: Parameters<typeof setMember>[4]) => onChange(setMember(def, id, group, k, change));
    return (
      <Section title={GROUP_LABEL[group].title}>
        <ul className="tw:m-0 tw:flex tw:list-none tw:flex-col tw:gap-2 tw:p-0">
          {list.map((m, k) => {
            const label = `${one} ${k + 1}`;
            return (
              // A member's place is its identity while it is renamed.
              // eslint-disable-next-line react/no-array-index-key
              <li key={`${id}:${group}:${k}`} data-slot="model-cube-member" className="tw:flex tw:flex-col tw:gap-1 tw:rounded-md tw:border tw:p-1.5">
                <div className="tw:flex tw:items-center tw:gap-1">
                  <DraftText label={`${label} name`} multiline={false} value={m.name} onCommit={(name) => edit(k, { name })} />
                  <button
                    type="button"
                    aria-label={`Remove ${one.toLowerCase()} ${k + 1}`}
                    className={ICON}
                    onClick={() => onChange(removeMember(def, id, group, k))}
                  >
                    <X className="tw:size-3.5" />
                  </button>
                </div>
                {group === 'dimensions' ? (
                  <Pick label={`${label} type`} value={m.type ?? 'string'} options={DIMENSION_TYPES} onPick={(type) => edit(k, { type: type as never })} />
                ) : null}
                {group === 'measures' ? (
                  <div className="tw:grid tw:grid-cols-2 tw:gap-1">
                    <Pick label={`${label} type`} value={m.type ?? 'count'} options={MEASURE_TYPES} onPick={(type) => edit(k, { type: type as never })} />
                    <Pick
                      label={`${label} format`}
                      value={m.format ?? ''}
                      options={MEASURE_FORMATS}
                      blank="no format"
                      onPick={(format) => edit(k, { format: (format || undefined) as never })}
                    />
                  </div>
                ) : null}
                {group !== 'segments' ? (
                  <Pick
                    label={`${label} column`}
                    value={m.column ?? ''}
                    options={columns}
                    blank="(SQL)"
                    onPick={(column) => edit(k, column ? { column, sql: undefined } : { column: undefined })}
                  />
                ) : null}
                {group === 'segments' || !m.column ? (
                  <DraftText
                    label={`${label} SQL`}
                    multiline={false}
                    placeholder="SQL"
                    value={m.sql ?? ''}
                    onCommit={(sql) => edit(k, sql ? { sql, column: undefined } : { sql: undefined })}
                  />
                ) : null}
                <DraftText
                  label={`${label} title`}
                  multiline={false}
                  placeholder="Title"
                  value={m.title ?? ''}
                  onCommit={(title) => edit(k, { title: title || undefined })}
                />
                {group === 'dimensions' ? (
                  <Check
                    label={`${label} primary key`}
                    checked={Boolean(m.primaryKey)}
                    onChange={(on) => edit(k, { primaryKey: on || undefined })}
                  >
                    Primary key
                  </Check>
                ) : null}
              </li>
            );
          })}
        </ul>
        <Button
          variant="outline"
          size="sm"
          onClick={() => onChange(addMember(def, id, group).def)}
        >
          {`+ Add ${one.toLowerCase()}`}
        </Button>
      </Section>
    );
  };

  return (
    <>
      <Section title="General">
        <CubeName def={def} id={id} onChange={onChange} />
        <Field label="Title" htmlFor="model-cube-title">
          <DraftText
            key={`${id}:title`}
            id="model-cube-title"
            label="Cube title"
            multiline={false}
            value={cube.title ?? ''}
            onCommit={(title) => patch({ title: title || undefined })}
          />
        </Field>
        <Field label="Description" htmlFor="model-cube-description">
          <DraftText
            key={`${id}:description`}
            id="model-cube-description"
            label="Cube description"
            value={cube.description ?? ''}
            onCommit={(description) => patch({ description: description || undefined })}
          />
        </Field>
      </Section>

      <Section title="Source">
        <Field label="Kind" htmlFor="model-cube-kind">
          <Pick
            label="Source kind"
            value={source.kind}
            options={SOURCE_KINDS}
            onPick={(k) => changeKind(k as CubeSource['kind'])}
          />
        </Field>
        {source.kind === 'sql' ? (
          <>
            <Field label="SQL" htmlFor="model-cube-sql">
              <DraftText
                key={`${id}:sql`}
                id="model-cube-sql"
                label="Cube SQL"
                value={source.sql ?? ''}
                onCommit={(sql) => onChange(setCubeSource(def, id, { ...source, sql }))}
              />
            </Field>
            <div className="tw:flex tw:flex-col tw:gap-1">
              <span className="tw:text-xs tw:text-muted-foreground">Reads from</span>
              {def.tables.map((t) => (
                <Check
                  key={t.alias}
                  label={`Reads from ${t.alias}`}
                  checked={(source.tables ?? []).includes(t.alias)}
                  onChange={(on) => onChange(setCubeSource(def, id, {
                    ...source,
                    tables: on
                      ? [...(source.tables ?? []), t.alias]
                      : (source.tables ?? []).filter((a) => a !== t.alias),
                  }))}
                >
                  {t.alias}
                </Check>
              ))}
            </div>
          </>
        ) : (
          <>
            <Field label={source.kind === 'joined' ? 'Primary table' : 'Table'} htmlFor="model-cube-table">
              <Pick
                label="Cube table"
                value={source.table ?? ''}
                options={def.tables.map((t) => t.alias)}
                blank="Choose a table"
                onPick={(table) => onChange(setCubeSource(def, id, {
                  ...source, table: table || undefined, primaryKey: undefined,
                }))}
              />
            </Field>
            <Field label="Primary key" htmlFor="model-cube-primary-key">
              <Pick
                label="Cube primary key"
                value={source.primaryKey ?? ''}
                options={source.table ? columnsOfAlias(source.table) : []}
                blank="No primary key"
                onPick={(primaryKey) => onChange(setCubeSource(def, id, {
                  ...source, primaryKey: primaryKey || undefined,
                }))}
              />
            </Field>
          </>
        )}
        <div className="tw:flex tw:flex-col tw:gap-1">
          <Button
            variant="outline"
            size="sm"
            disabled={source.kind === 'sql'}
            title={source.kind === 'sql' ? 'A SQL cube has no table to read members from' : undefined}
            onClick={() => (hasMembers ? setConfirming(true) : generate())}
          >
            Generate members
          </Button>
          {confirming ? (
            <div role="group" aria-label="Replace members" className="tw:flex tw:flex-col tw:gap-1 tw:text-xs">
              <span>Replace the existing members?</span>
              <div className="tw:flex tw:gap-1">
                <Button size="sm" onClick={generate}>Replace</Button>
                <Button variant="outline" size="sm" onClick={() => setConfirming(false)}>Cancel</Button>
              </div>
            </div>
          ) : null}
        </div>
      </Section>

      {rows('dimensions')}
      {rows('measures')}
      {rows('segments')}

      <Section title="Format">
        <Palette slot="model-cube-color" value={cube.color ?? 'violet'} onPick={(color) => patch({ color })} />
      </Section>

      <IssuesSection issues={issues} heading="Issues" />
    </>
  );
}
