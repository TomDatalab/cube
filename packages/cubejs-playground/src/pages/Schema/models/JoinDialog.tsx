import { useEffect, useState } from 'react';

import { Button } from '@/components/ui/button';
import { Label } from '@/components/ui/label';
import { RadioGroup, RadioGroupItem } from '@/components/ui/radio-group';
import { FormDialog } from '@/components/kit';

import type { End, JoinKind, Relationship } from './modelGraph';

// The hint names both sides: `a` is where the link starts, `b` where it ends.
const RELATIONSHIPS: { value: Relationship; label: string; hint: (a: string, b: string) => string }[] = [
  { value: 'one_to_one', label: '1:1', hint: (a, b) => `one ${a} row to one ${b} row` },
  { value: 'one_to_many', label: '1:N', hint: (a, b) => `one ${a} row to many ${b} rows` },
  { value: 'many_to_one', label: 'N:1', hint: (a, b) => `many ${a} rows to one ${b} row` },
];

const JOINS: { value: JoinKind; label: string; hint: string }[] = [
  { value: 'left', label: 'LEFT', hint: 'LEFT keeps every row of the principal table' },
  { value: 'inner', label: 'INNER', hint: 'INNER keeps only rows with a match on both sides' },
];

export type JoinChoice = { relationship: Relationship; join: JoinKind };

// The relationship and the JOIN of a link: a new one (`onDelete` absent) or
// one clicked on the canvas.
export function JoinDialog({
  open, from, to, initial, onSave, onDelete, onClose,
}: {
  open: boolean;
  from?: End;
  to?: End;
  initial: JoinChoice;
  onSave: (choice: JoinChoice) => void;
  onDelete?: () => void;
  onClose: () => void;
}) {
  const [relationship, setRelationship] = useState(initial.relationship);
  const [join, setJoin] = useState(initial.join);

  useEffect(() => {
    if (open) {
      setRelationship(initial.relationship);
      setJoin(initial.join);
    }
  }, [open, initial.relationship, initial.join]);

  const editing = Boolean(onDelete);
  return (
    <FormDialog
      open={open}
      title={editing ? 'Link' : 'New link'}
      description={from && to ? `${from.alias}.${from.column} → ${to.alias}.${to.column}` : undefined}
      onClose={onClose}
      footer={(
        <>
          {onDelete ? (
            <Button variant="destructive" className="tw:sm:mr-auto" onClick={onDelete}>Delete</Button>
          ) : null}
          <Button variant="outline" onClick={onClose}>Cancel</Button>
          <Button onClick={() => onSave({ relationship, join })}>{editing ? 'Save link' : 'Link'}</Button>
        </>
      )}
    >
      <div className="tw:flex tw:flex-col tw:gap-5">
        <fieldset className="tw:m-0 tw:flex tw:flex-col tw:gap-2 tw:border-0 tw:p-0">
          <legend className="tw:mb-2 tw:text-sm tw:font-medium">Relationship</legend>
          <RadioGroup value={relationship} onValueChange={(v) => setRelationship(v as Relationship)}>
            {RELATIONSHIPS.map((r) => (
              <div key={r.value} className="tw:flex tw:items-center tw:gap-2">
                <RadioGroupItem id={`join-rel-${r.value}`} value={r.value} />
                <Label htmlFor={`join-rel-${r.value}`}>
                  {r.label}
                  <span className="tw:font-normal tw:text-muted-foreground">{r.hint(from?.alias ?? 'source', to?.alias ?? 'target')}</span>
                </Label>
              </div>
            ))}
          </RadioGroup>
        </fieldset>
        <fieldset className="tw:m-0 tw:flex tw:flex-col tw:gap-2 tw:border-0 tw:p-0">
          <legend className="tw:mb-2 tw:text-sm tw:font-medium">Join</legend>
          <RadioGroup value={join} onValueChange={(v) => setJoin(v as JoinKind)}>
            {JOINS.map((j) => (
              <div key={j.value} className="tw:flex tw:items-center tw:gap-2">
                <RadioGroupItem id={`join-kind-${j.value}`} value={j.value} />
                <Label htmlFor={`join-kind-${j.value}`}>{j.label}</Label>
              </div>
            ))}
          </RadioGroup>
          <ul className="tw:m-0 tw:list-none tw:p-0 tw:text-sm tw:text-muted-foreground">
            {JOINS.map((j) => <li key={j.value}>{j.hint}</li>)}
          </ul>
        </fieldset>
      </div>
    </FormDialog>
  );
}
