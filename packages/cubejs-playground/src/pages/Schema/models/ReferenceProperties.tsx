import { CircleAlert, Plus, TriangleAlert, X } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { Field } from '@/components/kit';
import { cn } from '@/lib/utils';

import type { Issue } from './validate';
import {
  CARDINALITIES, Cardinality, ModelDefinition, ModelJoin, ModelTable, REF_ACTIONS, RefAction, joinLabel,
  removeJoin, setJoin,
} from './modelGraph';
import { referenceEnds, setReference, swapReference } from './physical';
import {
  Check, DraftText, Section, Value,
} from './PanelParts';

type Columns = { name: string; type: string }[];

export type ReferencePropertiesProps = {
  def: ModelDefinition;
  index: number;
  columnsOf(t: ModelTable): Columns;
  issues: Issue[];
  onChange(next: ModelDefinition): void;
  onDeselect(): void;
};

const SELECT = 'tw:h-8 tw:w-full tw:rounded-md tw:border tw:bg-background tw:px-2 tw:text-sm tw:text-foreground';
const ICON = 'tw:flex tw:size-6 tw:shrink-0 tw:cursor-pointer tw:items-center tw:justify-center tw:rounded tw:border-0 tw:bg-transparent tw:p-0 tw:text-muted-foreground tw:hover:bg-accent tw:hover:text-foreground';

// The cardinality a reference shows when none was chosen: what its cube
// join reads.
function cardinalityOf(j: ModelJoin): Cardinality {
  return j.cardinality ?? (j.relationship === 'one_to_one' ? '0..1' : '0..*');
}

// The column pairs as (parent, child), the first one from the ends.
function pairsOf(j: ModelJoin): { parent: string; child: string }[] {
  const parentIsFrom = referenceEnds(j).parent === j.from;
  return [{ from: j.from.column, to: j.to.column }, ...(j.pairs ?? [])]
    .map((p) => (parentIsFrom ? { parent: p.from, child: p.to } : { parent: p.to, child: p.from }));
}

// The reference with its pairs given as (parent, child).
function withPairs(j: ModelJoin, pairs: { parent: string; child: string }[]): ModelJoin {
  const parentIsFrom = referenceEnds(j).parent === j.from;
  const asEnds = pairs.map((p) => (parentIsFrom ? { from: p.parent, to: p.child } : { from: p.child, to: p.parent }));
  const [first, ...rest] = asEnds;
  const { pairs: _drop, ...without } = j;
  return {
    ...without,
    from: { ...j.from, column: first.from },
    to: { ...j.to, column: first.to },
    ...(rest.length ? { pairs: rest } : {}),
  };
}

// Redgate's reference properties: name, comment, cardinality, the primary
// and foreign tables with roles and column pairs, mandatory and the
// constraint actions; then what the cube build joins with, and its issues.
export function ReferenceProperties({
  def, index, columnsOf, issues, onChange, onDeselect,
}: ReferencePropertiesProps) {
  const j = def.joins[index];
  if (!j) {
    return null;
  }
  const id = j.id ?? '';
  const { parent, child } = referenceEnds(j);
  const parentIsFrom = parent === j.from;
  const table = (alias: string) => def.tables.find((t) => t.alias === alias);
  const parentColumns = table(parent.alias) ? columnsOf(table(parent.alias)!) : [];
  const childColumns = table(child.alias) ? columnsOf(table(child.alias)!) : [];
  const pairs = pairsOf(j);
  const replace = (next: ModelJoin) => onChange({ ...def, joins: def.joins.map((x, i) => (i === index ? next : x)) });
  const setPairs = (next: { parent: string; child: string }[]) => replace(withPairs(j, next));
  const roleKey = (side: 'parent' | 'child'): 'fromRole' | 'toRole' => {
    const isFrom = side === 'parent' ? parentIsFrom : !parentIsFrom;
    return isFrom ? 'fromRole' : 'toRole';
  };
  const errors = issues.filter((i) => i.severity === 'error');
  const warnings = issues.filter((i) => i.severity === 'warning');
  return (
    <>
      <Section title="General">
        <Field label="Name" htmlFor="model-reference-name">
          <DraftText
            id="model-reference-name"
            multiline={false}
            label="Reference name"
            value={j.name ?? ''}
            onCommit={(name) => onChange(setReference(def, id, { name: name.trim() }))}
          />
        </Field>
        <Field label="Comment" htmlFor="model-reference-comment">
          <DraftText
            id="model-reference-comment"
            label="Reference comment"
            value={j.comment ?? ''}
            onCommit={(comment) => onChange(setReference(def, id, { comment }))}
          />
        </Field>
        <Field label="Cardinality" htmlFor="model-reference-cardinality">
          <select
            id="model-reference-cardinality"
            aria-label="Cardinality"
            className={SELECT}
            value={cardinalityOf(j)}
            onChange={(e) => onChange(setReference(def, id, { cardinality: e.target.value as Cardinality }))}
          >
            {CARDINALITIES.map((c) => <option key={c} value={c}>{c}</option>)}
          </select>
        </Field>
        <Button variant="outline" size="sm" className="tw:self-start" onClick={() => onChange(swapReference(def, id))}>
          Swap reference
        </Button>

        <div className="tw:grid tw:grid-cols-2 tw:gap-2 tw:rounded tw:border tw:p-2 tw:text-sm">
          <span className="tw:font-semibold">Primary</span>
          <span className="tw:font-semibold">Foreign</span>
          <span data-slot="model-reference-primary">{parent.alias}</span>
          <span data-slot="model-reference-foreign">{child.alias}</span>
          <DraftText
            multiline={false}
            label="Primary role"
            placeholder="Role"
            value={j[roleKey('parent')] ?? ''}
            onCommit={(role) => onChange(setReference(def, id, { [roleKey('parent')]: role }))}
          />
          <DraftText
            multiline={false}
            label="Foreign role"
            placeholder="Role"
            value={j[roleKey('child')] ?? ''}
            onCommit={(role) => onChange(setReference(def, id, { [roleKey('child')]: role }))}
          />
          {pairs.map((p, k) => (
            // A pair's place is its identity.
            // eslint-disable-next-line react/no-array-index-key
            <div key={k} className="tw:col-span-2 tw:grid tw:grid-cols-[1fr_1fr_auto] tw:items-center tw:gap-2">
              <select
                aria-label={`Pair ${k + 1} primary column`}
                className={SELECT}
                value={p.parent}
                onChange={(e) => setPairs(pairs.map((x, i) => (i === k ? { ...x, parent: e.target.value } : x)))}
              >
                {parentColumns.map((c) => <option key={c.name} value={c.name}>{c.name}</option>)}
              </select>
              <select
                aria-label={`Pair ${k + 1} foreign column`}
                className={SELECT}
                value={p.child}
                onChange={(e) => setPairs(pairs.map((x, i) => (i === k ? { ...x, child: e.target.value } : x)))}
              >
                {childColumns.map((c) => <option key={c.name} value={c.name}>{c.name}</option>)}
              </select>
              {k > 0 ? (
                <button
                  type="button"
                  aria-label={`Remove pair ${k + 1}`}
                  className={ICON}
                  onClick={() => setPairs(pairs.filter((_, i) => i !== k))}
                >
                  <X className="tw:size-3.5" />
                </button>
              ) : <span className="tw:w-6" />}
            </div>
          ))}
          <Button
            variant="link"
            size="sm"
            className="tw:col-span-2 tw:justify-self-end tw:p-0"
            disabled={parentColumns.length === 0 || childColumns.length === 0}
            onClick={() => setPairs([...pairs, { parent: parentColumns[0].name, child: childColumns[0].name }])}
          >
            <Plus />
            Add pair
          </Button>
        </div>

        <Check
          label="Mandatory"
          checked={j.mandatory ?? j.join === 'inner'}
          onChange={(mandatory) => onChange(setReference(def, id, { mandatory }))}
        >
          Mandatory
        </Check>
        {(['onUpdate', 'onDelete'] as const).map((key) => {
          const label = key === 'onUpdate' ? 'Update constraint action' : 'Delete constraint action';
          return (
            <Field key={key} label={label} htmlFor={`model-reference-${key}`}>
              <select
                id={`model-reference-${key}`}
                aria-label={label}
                className={SELECT}
                value={j[key] ?? 'none'}
                onChange={(e) => onChange(setReference(def, id, { [key]: e.target.value as RefAction }))}
              >
                {REF_ACTIONS.map((a) => <option key={a} value={a}>{a}</option>)}
              </select>
            </Field>
          );
        })}
        <Check
          label="Hidden on the canvas"
          checked={Boolean(j.hidden)}
          onChange={(hidden) => onChange(setJoin(def, index, { hidden }))}
        >
          Hidden on the canvas
        </Check>
        <Value label="Cube join">{joinLabel(j)}</Value>
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
      <Section title="Reference issues" slot="model-reference-issues">
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
    </>
  );
}
