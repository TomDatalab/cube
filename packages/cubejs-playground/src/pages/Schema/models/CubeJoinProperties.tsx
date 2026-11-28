import { ArrowLeftRight, X } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { Field } from '@/components/kit';

import {
  dimensionNames, moveCubeJoinEnd, setCubeJoin, swapCubeJoin,
} from './cubes';
import { REL_LABEL } from './modelGraph';
import type { ModelDefinition, ModelTable, Relationship } from './modelGraph';
import { Section } from './PanelParts';
import { IssuesSection } from './TableProperties';
import type { Issue } from './validate';

type Columns = { name: string; type: string }[];

const SELECT = 'tw:h-7 tw:w-full tw:min-w-0 tw:rounded-md tw:border tw:bg-background tw:px-1 tw:text-xs tw:text-foreground';
const ICON = 'tw:flex tw:size-6 tw:shrink-0 tw:cursor-pointer tw:items-center tw:justify-center tw:rounded tw:border-0 tw:bg-transparent tw:p-0 tw:text-muted-foreground tw:hover:bg-accent tw:hover:text-foreground';
const RELATIONSHIPS: Relationship[] = ['one_to_one', 'one_to_many', 'many_to_one'];

export type CubeJoinPropertiesProps = {
  def: ModelDefinition;
  id: string;
  columnsOf(t: ModelTable): Columns;
  issues: Issue[];
  onChange(next: ModelDefinition): void;
};

// Redgate's reference properties for a join of two cubes: the cubes it
// joins (either end can move to another cube, or the two swap), its
// relationship and the dimensions it pairs, then its issues.
export function CubeJoinProperties({
  def, id, columnsOf, issues, onChange,
}: CubeJoinPropertiesProps) {
  const join = def.cubeJoins?.find((j) => j.id === id);
  if (!join) {
    return null;
  }
  const from = def.cubes?.find((c) => c.id === join.from);
  const to = def.cubes?.find((c) => c.id === join.to);
  const lookup = (schema: string, table: string) => {
    const t = def.tables.find((x) => x.schema === schema && x.table === table);
    return t ? columnsOf(t) : undefined;
  };
  const fromNames = from ? dimensionNames(def, from, lookup) : [];
  const toNames = to ? dimensionNames(def, to, lookup) : [];
  const cubes = def.cubes ?? [];
  // An end may go to any cube but the other end's, and not to one another
  // join already joins to the other end.
  const free = (end: 'from' | 'to', cubeId: string) => {
    const other = end === 'from' ? join.to : join.from;
    const pair = [cubeId, other];
    return cubeId !== other
      && !(def.cubeJoins ?? []).some((j) => j.id !== id && pair.includes(j.from) && pair.includes(j.to));
  };
  const moveEnd = (end: 'from' | 'to', cubeId: string) => {
    const c = cubes.find((x) => x.id === cubeId);
    onChange(moveCubeJoinEnd(def, id, end, cubeId, c ? dimensionNames(def, c, lookup) : []));
  };
  const cubePicker = (end: 'from' | 'to') => (
    <select
      id={`model-cube-join-${end}`}
      aria-label={end === 'from' ? 'From cube' : 'To cube'}
      className={SELECT}
      value={join[end]}
      onChange={(e) => moveEnd(end, e.target.value)}
    >
      {cubes.map((c) => (
        <option key={c.id} value={c.id} disabled={c.id !== join[end] && !free(end, c.id)}>
          {c.name}
          {c.id !== join[end] && c.id !== join[end === 'from' ? 'to' : 'from'] && !free(end, c.id) ? ' (already joined)' : ''}
        </option>
      ))}
    </select>
  );
  const patchPairs = (on: typeof join.on) => onChange(setCubeJoin(def, id, { on }));
  const options = (names: string[], value: string) => (names.includes(value) || !value ? names : [...names, value]);
  return (
    <>
      <Section title="General">
        <Field label="From" htmlFor="model-cube-join-from">{cubePicker('from')}</Field>
        <Field label="To" htmlFor="model-cube-join-to">{cubePicker('to')}</Field>
        <Button variant="outline" size="sm" onClick={() => onChange(swapCubeJoin(def, id))}>
          <ArrowLeftRight />
          Swap direction
        </Button>
        <Field label="Relationship" htmlFor="model-cube-join-relationship">
          <select
            id="model-cube-join-relationship"
            aria-label="Relationship"
            className={SELECT}
            value={join.relationship}
            onChange={(e) => onChange(setCubeJoin(def, id, { relationship: e.target.value as Relationship }))}
          >
            {RELATIONSHIPS.map((r) => <option key={r} value={r}>{`${REL_LABEL[r]} (${r})`}</option>)}
          </select>
        </Field>
      </Section>

      <Section title="Pairs">
        <ul className="tw:m-0 tw:flex tw:list-none tw:flex-col tw:gap-1 tw:p-0">
          {join.on.map((p, k) => (
            // A pair's place is its identity.
            // eslint-disable-next-line react/no-array-index-key
            <li key={`${id}:${k}`} className="tw:grid tw:grid-cols-[1fr_1fr_auto] tw:items-center tw:gap-1">
              <select
                aria-label={`Pair ${k + 1} from dimension`}
                className={SELECT}
                value={p.from}
                onChange={(e) => patchPairs(join.on.map((x, i) => (i === k ? { ...x, from: e.target.value } : x)))}
              >
                {options(fromNames, p.from).map((d) => <option key={d} value={d}>{d}</option>)}
              </select>
              <select
                aria-label={`Pair ${k + 1} to dimension`}
                className={SELECT}
                value={p.to}
                onChange={(e) => patchPairs(join.on.map((x, i) => (i === k ? { ...x, to: e.target.value } : x)))}
              >
                {options(toNames, p.to).map((d) => <option key={d} value={d}>{d}</option>)}
              </select>
              <button
                type="button"
                aria-label={`Remove pair ${k + 1}`}
                // A join needs a pair: the last one stays.
                disabled={join.on.length === 1}
                title={join.on.length === 1 ? 'A cube join needs a pair' : undefined}
                className={`${ICON} tw:disabled:cursor-not-allowed tw:disabled:opacity-40`}
                onClick={() => patchPairs(join.on.filter((_, i) => i !== k))}
              >
                <X className="tw:size-3.5" />
              </button>
            </li>
          ))}
        </ul>
        <Button
          variant="outline"
          size="sm"
          disabled={fromNames.length === 0 || toNames.length === 0}
          onClick={() => patchPairs([...join.on, { from: fromNames[0], to: toNames[0] }])}
        >
          + Add pair
        </Button>
      </Section>

      <IssuesSection issues={issues} heading="Issues" />
    </>
  );
}
