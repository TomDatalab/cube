import { FormEvent, useEffect, useRef, useState } from 'react';
import { CircleAlert, MinusCircle, Plus } from 'lucide-react';

import { Alert, AlertDescription } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import {
  FormDialog, notify, Spinner, TagsInput,
} from '@/components/kit';

import { useIsMounted } from '../../hooks';
import {
  AdminApi, ATTRIBUTE_NAME, attributesFromPairs, User,
} from './api';

type Props = {
  api: AdminApi;
  user: User | null;
  onClose: () => void;
};

type Pair = { name?: string; values?: string[] };
// A row of the form; `key` stays with it while rows above are removed.
type Row = Pair & { key: number };
type RowErrors = { name?: string; values?: string };

// The same checks, and messages, as each row's fields had.
function rowErrors({ name, values }: Pair): RowErrors {
  const errors: RowErrors = {};
  if (!name) {
    errors.name = 'Name the attribute';
  } else if (!ATTRIBUTE_NAME.test(name)) {
    errors.name = 'a-z, 0-9 and _, up to 64';
  }
  if (!values?.length) {
    errors.values = 'Add at least one value';
  }
  return errors;
}

// A user's attributes: name -> values. A role's row filter can read one by
// name, so each user sees only the rows their own values allow.
export function AttributesModal({ api, user, onClose }: Props) {
  const isMounted = useIsMounted();
  const [rows, setRows] = useState<Row[]>([]);
  const [rowErrorsByKey, setRowErrors] = useState<Record<number, RowErrors>>({});
  const nextKey = useRef(0);
  const [loading, setLoading] = useState(false);
  // Saving replaces the whole set, so it waits for a successful load.
  const [loaded, setLoaded] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const toRows = (pairs: Pair[]): Row[] => pairs.map((p) => ({ ...p, key: nextKey.current++ }));

  useEffect(() => {
    if (!user) {
      return;
    }
    // The modal stays mounted: an answer for another user, or for an earlier
    // attempt, must neither fill the form nor allow saving.
    let cancelled = false;
    const current = () => !cancelled && isMounted();
    setError(null);
    setLoaded(false);
    setLoading(true);
    setRows([]);
    setRowErrors({});
    api.userAttributes(user.id).then((attributes) => {
      if (current()) {
        setRows(toRows(Object.entries(attributes).map(([name, values]) => ({ name, values }))));
        setLoaded(true);
      }
    }).catch((e: any) => {
      if (current()) {
        setError(e.message);
      }
    }).finally(() => {
      if (current()) {
        setLoading(false);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [api, user, attempt]);

  function change(key: number, patch: Pair) {
    setRows((rs) => rs.map((r) => (r.key === key ? { ...r, ...patch } : r)));
    // A changed field's error goes until the next save.
    setRowErrors((errs) => {
      const { [key]: rowErr, ...rest } = errs;
      if (!rowErr) {
        return errs;
      }
      const left = { ...rowErr };
      Object.keys(patch).forEach((field) => delete left[field as keyof RowErrors]);
      return { ...rest, [key]: left };
    });
  }

  async function save(event?: FormEvent) {
    event?.preventDefault();
    if (!loaded) {
      return;
    }
    const found: Record<number, RowErrors> = {};
    rows.forEach((r) => {
      const errs = rowErrors(r);
      if (errs.name || errs.values) {
        found[r.key] = errs;
      }
    });
    setRowErrors(found);
    if (Object.keys(found).length) {
      return;
    }
    let attributes;
    try {
      attributes = attributesFromPairs(rows.map(({ name, values }) => ({ name, values })));
    } catch (e: any) {
      setError(e.message);
      return;
    }
    setSaving(true);
    setError(null);
    try {
      await api.setUserAttributes(user!.id, attributes);
      notify.success(`Attributes of ${user!.username} saved`);
      onClose();
    } catch (e: any) {
      setError(e.message);
    } finally {
      setSaving(false);
    }
  }

  return (
    <FormDialog
      open={user != null}
      title={user ? `Attributes of ${user.username}` : 'Attributes'}
      onClose={onClose}
      onOk={() => save()}
      okText="Save"
      saving={saving}
      okDisabled={!loaded}
      className="tw:sm:max-w-xl"
    >
      <p className="tw:text-sm tw:text-muted-foreground">
        Role row filters can match a member against one of these by name.
      </p>
      {loading ? <Spinner className="tw:mx-auto tw:my-2" /> : null}
      {!loaded && error ? (
        <Alert variant="destructive">
          <CircleAlert />
          <AlertDescription className="tw:flex tw:items-center tw:justify-between tw:gap-2">
            <span>{error}</span>
            <Button size="sm" variant="outline" onClick={() => setAttempt(attempt + 1)}>Retry</Button>
          </AlertDescription>
        </Alert>
      ) : null}
      <div style={{ display: loaded ? undefined : 'none' }}>
        <form className="tw:flex tw:flex-col tw:gap-3" onSubmit={save} noValidate>
          {rows.map((row) => {
            const errs = rowErrorsByKey[row.key] || {};
            return (
              <div key={row.key} className="tw:flex tw:items-start tw:gap-2">
                <div className="tw:flex tw:w-[160px] tw:shrink-0 tw:flex-col tw:gap-1">
                  <Input
                    placeholder="Name, e.g. region"
                    maxLength={64}
                    autoComplete="off"
                    value={row.name ?? ''}
                    aria-invalid={Boolean(errs.name)}
                    onChange={(e) => change(row.key, { name: e.target.value })}
                  />
                  {errs.name ? <p className="tw:text-sm tw:text-destructive">{errs.name}</p> : null}
                </div>
                <div className="tw:flex tw:w-[240px] tw:min-w-0 tw:flex-col tw:gap-1">
                  <TagsInput
                    placeholder="Values"
                    value={row.values ?? []}
                    onChange={(values) => change(row.key, { values })}
                  />
                  {errs.values ? <p className="tw:text-sm tw:text-destructive">{errs.values}</p> : null}
                </div>
                <Button
                  type="button"
                  size="icon"
                  variant="ghost"
                  aria-label="Remove attribute"
                  onClick={() => {
                    setRows((rs) => rs.filter((r) => r.key !== row.key));
                    setRowErrors(({ [row.key]: _, ...rest }) => rest);
                  }}
                >
                  <MinusCircle />
                </Button>
              </div>
            );
          })}
          <Button
            type="button"
            variant="outline"
            className="tw:border-dashed"
            onClick={() => setRows((rs) => [...rs, ...toRows([{ values: [] }])])}
          >
            <Plus />
            Add attribute
          </Button>
          {loaded && error ? (
            <Alert variant="destructive" className="tw:mt-1">
              <CircleAlert />
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          ) : null}
          {/* Enter in a name saves. */}
          <button type="submit" hidden aria-hidden tabIndex={-1} />
        </form>
      </div>
    </FormDialog>
  );
}
