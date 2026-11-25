import { FormEvent, useEffect, useState } from 'react';
import { CircleAlert } from 'lucide-react';

import { Alert, AlertDescription } from '@/components/ui/alert';
import { Input } from '@/components/ui/input';
import { Field, FormDialog } from '@/components/kit';

import { RemoteSelect } from './RemoteSelect';

type Choice = {
  field: 'owner' | 'organization';
  label: string;
  message: string;
  fetchOptions: (q: string) => Promise<{ value: string; label: string }[]>;
  initial?: string;
  // The label of `initial`, so the select does not show a bare id.
  initialLabel?: string;
};

export type NameValues = { name: string; owner?: string; organization?: string };

type Props = {
  title: string | null;
  initialName?: string;
  // Asks for one more value from a list as well: the first owner of a new
  // organization, or the organization of a new workspace.
  choice?: Choice;
  onSubmit: (values: NameValues) => Promise<unknown>;
  onClose: () => void;
};

// A dialog asking for a name; open while `title` is set. A failure keeps it
// open with the server's message.
export function NameDialog({
  title, initialName = '', choice, onSubmit, onClose,
}: Props) {
  const [name, setName] = useState('');
  const [picked, setPicked] = useState<string | undefined>();
  const [missing, setMissing] = useState<{ name?: string; choice?: string }>({});
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (title != null) {
      setName(initialName);
      setPicked(choice?.initial);
      setMissing({});
      setError(null);
    }
  }, [title, initialName, choice?.initial]);

  async function submit(event?: FormEvent) {
    event?.preventDefault();
    const lacking = {
      name: name.trim() ? undefined : 'Enter a name',
      choice: choice && !picked ? choice.message : undefined,
    };
    setMissing(lacking);
    if (lacking.name || lacking.choice) {
      return;
    }
    setSaving(true);
    setError(null);
    try {
      await onSubmit(choice ? { name, [choice.field]: picked } : { name });
      onClose();
    } catch (e: any) {
      setError(e.message);
    } finally {
      setSaving(false);
    }
  }

  return (
    <FormDialog
      open={title != null}
      title={title}
      onClose={onClose}
      onOk={() => submit()}
      saving={saving}
    >
      <form className="tw:flex tw:flex-col tw:gap-4" onSubmit={submit} noValidate>
        <Field label="Name" htmlFor="name-dialog-name" error={missing.name}>
          <Input
            id="name-dialog-name"
            autoComplete="off"
            autoFocus
            value={name}
            aria-invalid={Boolean(missing.name)}
            onChange={(e) => {
              setName(e.target.value);
              setMissing((m) => ({ ...m, name: undefined }));
            }}
          />
        </Field>
        {choice ? (
          <Field label={choice.label} htmlFor="name-dialog-choice" error={missing.choice}>
            <RemoteSelect
              id="name-dialog-choice"
              fetchOptions={choice.fetchOptions}
              valueLabel={choice.initialLabel}
              value={picked}
              invalid={Boolean(missing.choice)}
              onChange={(v) => {
                setPicked(v);
                setMissing((m) => ({ ...m, choice: undefined }));
              }}
            />
          </Field>
        ) : null}
        {error ? (
          <Alert variant="destructive">
            <CircleAlert />
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        ) : null}
        {/* Enter in the name submits. */}
        <button type="submit" hidden aria-hidden tabIndex={-1} />
      </form>
    </FormDialog>
  );
}
