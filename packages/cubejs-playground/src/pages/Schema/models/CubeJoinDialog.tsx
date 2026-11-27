import { useEffect, useState } from 'react';
import { Plus, X } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { Field, FormDialog } from '@/components/kit';

import { REL_LABEL } from './modelGraph';
import type { Relationship } from './modelGraph';

const SELECT = 'tw:h-8 tw:w-full tw:rounded-md tw:border tw:bg-background tw:px-2 tw:text-sm tw:text-foreground';
const ICON = 'tw:flex tw:size-6 tw:shrink-0 tw:cursor-pointer tw:items-center tw:justify-center tw:rounded tw:border-0 tw:bg-transparent tw:p-0 tw:text-muted-foreground tw:hover:bg-accent tw:hover:text-foreground';
const RELATIONSHIPS: Relationship[] = ['one_to_one', 'one_to_many', 'many_to_one'];

type Pair = { from: string; to: string };
export type CubeJoinChoice = { relationship: Relationship; on: Pair[] };

// A new join between two cubes: its relationship and the pairs of
// dimensions it joins on, `from` of the cube the line starts at.
export function CubeJoinDialog({
  open, from, to, fromDimensions, toDimensions, onSave, onClose,
}: {
  open: boolean;
  from: string;
  to: string;
  fromDimensions: string[];
  toDimensions: string[];
  onSave: (choice: CubeJoinChoice) => void;
  onClose: () => void;
}) {
  const canPair = fromDimensions.length > 0 && toDimensions.length > 0;
  const firstPair = (): Pair => ({ from: fromDimensions[0], to: toDimensions[0] });
  const [relationship, setRelationship] = useState<Relationship>('many_to_one');
  const [pairs, setPairs] = useState<Pair[]>([]);

  useEffect(() => {
    if (open) {
      setRelationship('many_to_one');
      setPairs(canPair ? [firstPair()] : []);
    }
    // A new dialog starts again; the lists are read as it opens.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  return (
    <FormDialog
      open={open}
      title="New cube join"
      description={`${from} → ${to}`}
      onClose={onClose}
      footer={(
        <>
          <Button variant="outline" onClick={onClose}>Cancel</Button>
          <Button disabled={pairs.length === 0} onClick={() => onSave({ relationship, on: pairs })}>Create join</Button>
        </>
      )}
    >
      <div className="tw:flex tw:flex-col tw:gap-4">
        <Field label="Relationship" htmlFor="cube-join-relationship">
          <select
            id="cube-join-relationship"
            aria-label="Relationship"
            className={SELECT}
            value={relationship}
            onChange={(e) => setRelationship(e.target.value as Relationship)}
          >
            {RELATIONSHIPS.map((r) => <option key={r} value={r}>{`${REL_LABEL[r]} (${r})`}</option>)}
          </select>
        </Field>
        <div className="tw:grid tw:grid-cols-[1fr_1fr_auto] tw:items-center tw:gap-2 tw:text-sm">
          <span className="tw:font-semibold">{from}</span>
          <span className="tw:font-semibold">{to}</span>
          <span className="tw:w-6" />
          {pairs.map((p, k) => (
            // A pair's place is its identity.
            // eslint-disable-next-line react/no-array-index-key
            <div key={k} className="tw:col-span-3 tw:grid tw:grid-cols-subgrid tw:items-center">
              <select
                aria-label={`Pair ${k + 1} from dimension`}
                className={SELECT}
                value={p.from}
                onChange={(e) => setPairs(pairs.map((x, i) => (i === k ? { ...x, from: e.target.value } : x)))}
              >
                {fromDimensions.map((d) => <option key={d} value={d}>{d}</option>)}
              </select>
              <select
                aria-label={`Pair ${k + 1} to dimension`}
                className={SELECT}
                value={p.to}
                onChange={(e) => setPairs(pairs.map((x, i) => (i === k ? { ...x, to: e.target.value } : x)))}
              >
                {toDimensions.map((d) => <option key={d} value={d}>{d}</option>)}
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
          {canPair ? null : (
            <span data-slot="cube-join-no-dimensions" className="tw:col-span-3 tw:text-muted-foreground">
              Both cubes need a dimension to join on
            </span>
          )}
          <Button
            variant="link"
            size="sm"
            className="tw:col-span-3 tw:justify-self-end tw:p-0"
            disabled={!canPair}
            onClick={() => setPairs([...pairs, firstPair()])}
          >
            <Plus />
            Add pair
          </Button>
        </div>
      </div>
    </FormDialog>
  );
}
