import { useEffect, useState } from 'react';
import { Plus, X } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/checkbox';
import { Input } from '@/components/ui/input';
import { Field, FormDialog, SimpleSelect } from '@/components/kit';

import { COLUMN_TYPES, ColumnType, DraftColumn } from './modelGraph';

export type TableDraft = {
  ds: string;
  schema: string;
  table: string;
  columns: DraftColumn[];
};

// The schema select's entry for a schema typed by hand.
const OTHER = '__other__';

const firstColumn = (): DraftColumn => ({
  name: 'id', type: 'integer', nullable: false, primaryKey: true,
});
const nextColumn = (): DraftColumn => ({
  name: '', type: 'text', nullable: true, primaryKey: false,
});

// A new table of the model, or one not in the database yet: its name, its
// schema (one of the connection's or typed) and its columns. `onSave`
// answers the reason it is refused, or null once it is taken.
export function TableDialog({
  open, initial, dataSource, connections, schemasOf, onSave, onClose,
}: {
  open: boolean;
  // The table edited; absent for a new one.
  initial?: { schema: string; table: string; columns: DraftColumn[] };
  // The model's connection; empty while the model has no table.
  dataSource: string;
  connections: string[];
  schemasOf(ds: string): string[];
  onSave(value: TableDraft): string | null;
  onClose(): void;
}) {
  const [ds, setDs] = useState(dataSource);
  const [schema, setSchema] = useState('');
  const [typed, setTyped] = useState(false);
  const [table, setTable] = useState('');
  const [columns, setColumns] = useState<DraftColumn[]>([firstColumn()]);
  const [error, setError] = useState<string | null>(null);

  const connection = dataSource || ds;
  const schemas = connection ? schemasOf(connection) : [];

  useEffect(() => {
    if (!open) {
      return;
    }
    const startDs = dataSource || connections[0] || '';
    const known = startDs ? schemasOf(startDs) : [];
    const startSchema = initial?.schema
      ?? (known.includes('public') ? 'public' : known[0] ?? 'public');
    setDs(startDs);
    setSchema(startSchema);
    setTyped(!known.includes(startSchema));
    setTable(initial?.table ?? '');
    setColumns(initial ? initial.columns.map((c) => ({ ...c })) : [firstColumn()]);
    setError(null);
    // Only when it opens: the rest is what the user typed.
  }, [open]);

  const patch = (index: number, change: Partial<DraftColumn>) => {
    setColumns((cs) => cs.map((c, i) => (i === index ? { ...c, ...change } : c)));
  };

  const save = () => {
    const problem = onSave({
      ds: connection,
      schema: schema.trim(),
      table: table.trim(),
      columns: columns.map((c) => ({ ...c, name: c.name.trim() })),
    });
    setError(problem);
  };

  return (
    <FormDialog
      open={open}
      title={initial ? `Edit table ${initial.table}` : 'New table'}
      description="Designed here; Create in database makes it in the connection."
      onClose={onClose}
      onOk={save}
      okText={initial ? 'Save table' : 'Add table'}
      className="tw:sm:max-w-2xl"
    >
      <div className="tw:flex tw:flex-col tw:gap-4">
        <div className="tw:flex tw:flex-wrap tw:items-start tw:gap-3">
          {!dataSource && connections.length > 0 ? (
            <Field label="Connection" htmlFor="table-connection" className="tw:w-44">
              <SimpleSelect
                id="table-connection"
                value={ds}
                options={connections.map((c) => ({ value: c, label: c }))}
                onChange={(v) => {
                  if (v) {
                    // A new connection has its own schemas: start over, as
                    // when the dialog opens.
                    const known = schemasOf(v);
                    const next = known.includes('public') ? 'public' : known[0] ?? 'public';
                    setDs(v);
                    setSchema(next);
                    setTyped(!known.includes(next));
                  }
                }}
              />
            </Field>
          ) : null}
          <Field label="Name" htmlFor="table-name" className="tw:w-48">
            <Input
              id="table-name"
              aria-label="Table name"
              value={table}
              autoFocus
              onChange={(e) => setTable(e.target.value)}
            />
          </Field>
          <Field label="Schema" htmlFor="table-schema" className="tw:w-48">
            {schemas.length ? (
              <SimpleSelect
                id="table-schema"
                value={typed ? OTHER : schema}
                options={[
                  ...schemas.map((s) => ({ value: s, label: s })),
                  { value: OTHER, label: 'Other…' },
                ]}
                onChange={(v) => {
                  if (v === OTHER) {
                    setTyped(true);
                  } else if (v) {
                    setTyped(false);
                    setSchema(v);
                  }
                }}
              />
            ) : null}
            {typed || !schemas.length ? (
              <Input
                id={schemas.length ? undefined : 'table-schema'}
                aria-label="Schema name"
                value={schema}
                onChange={(e) => setSchema(e.target.value)}
              />
            ) : null}
          </Field>
        </div>

        <div className="tw:flex tw:flex-col tw:gap-2">
          <div className="tw:grid tw:grid-cols-[1fr_9rem_auto_auto_auto] tw:items-center tw:gap-x-3 tw:gap-y-2 tw:text-sm">
            <span className="tw:font-medium">Column</span>
            <span className="tw:font-medium">Type</span>
            <span className="tw:font-medium">PK</span>
            <span className="tw:font-medium">Nullable</span>
            <span />
            {columns.map((c, i) => {
              const n = i + 1;
              return [
                <Input
                  key={`name-${n}`}
                  aria-label={`Column ${n} name`}
                  value={c.name}
                  onChange={(e) => patch(i, { name: e.target.value })}
                />,
                <SimpleSelect
                  key={`type-${n}`}
                  id={`table-column-${n}-type`}
                  value={c.type}
                  options={COLUMN_TYPES.map((t) => ({ value: t, label: t }))}
                  onChange={(v) => v && patch(i, { type: v as ColumnType })}
                />,
                <Checkbox
                  key={`pk-${n}`}
                  aria-label={`Column ${n} primary key`}
                  checked={c.primaryKey}
                  onCheckedChange={(v) => patch(
                    i,
                    v === true ? { primaryKey: true, nullable: false } : { primaryKey: false }
                  )}
                />,
                <Checkbox
                  key={`null-${n}`}
                  aria-label={`Column ${n} nullable`}
                  checked={c.nullable}
                  disabled={c.primaryKey}
                  onCheckedChange={(v) => patch(i, { nullable: v === true })}
                />,
                <Button
                  key={`remove-${n}`}
                  type="button"
                  variant="ghost"
                  size="icon-sm"
                  aria-label={`Remove column ${n}`}
                  disabled={columns.length === 1}
                  onClick={() => setColumns((cs) => cs.filter((_, j) => j !== i))}
                >
                  <X />
                </Button>,
              ];
            })}
          </div>
          <div>
            <Button type="button" variant="outline" size="sm" onClick={() => setColumns((cs) => [...cs, nextColumn()])}>
              <Plus />
              Add column
            </Button>
          </div>
        </div>

        {error ? <p className="tw:m-0 tw:text-sm tw:text-destructive" role="alert">{error}</p> : null}
      </div>
    </FormDialog>
  );
}
