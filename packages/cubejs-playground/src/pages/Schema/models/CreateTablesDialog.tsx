import { CircleAlert } from 'lucide-react';

import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import {
  AlertDialog, AlertDialogCancel, AlertDialogContent, AlertDialogDescription,
  AlertDialogFooter, AlertDialogHeader, AlertDialogTitle,
} from '@/components/ui/alert-dialog';
import { Button } from '@/components/ui/button';
import { Spinner } from '@/components/kit';

// The CREATE TABLE statements the server would run, asked before running
// them. An error of the run stays on it.
export function CreateTablesDialog({
  open, dataSource, statements, notes, error, created, running, onCreate, onClose,
}: {
  open: boolean;
  dataSource: string;
  statements: string[];
  // What the database leaves out (no foreign keys, no UNIQUE…).
  notes?: string[];
  error?: string | null;
  // Tables a run already created before it failed.
  created?: string[];
  running: boolean;
  onCreate(): void;
  onClose(): void;
}) {
  return (
    <AlertDialog open={open} onOpenChange={(next) => !next && !running && onClose()}>
      <AlertDialogContent className="tw:sm:max-w-2xl">
        <AlertDialogHeader>
          <AlertDialogTitle>Create the new tables?</AlertDialogTitle>
          <AlertDialogDescription>{`This changes the database of ${dataSource}`}</AlertDialogDescription>
        </AlertDialogHeader>
        <pre
          data-slot="model-create-statements"
          className="tw:m-0 tw:max-h-80 tw:overflow-auto tw:rounded-md tw:border tw:bg-muted tw:p-3 tw:text-xs tw:whitespace-pre-wrap"
        >
          {statements.map((s) => `${s};`).join('\n\n')}
        </pre>
        {notes?.length ? (
          <Alert data-slot="model-create-notes">
            <CircleAlert />
            <AlertTitle>Left out by this database</AlertTitle>
            <AlertDescription>
              <ul className="tw:m-0 tw:list-disc tw:pl-4">
                {notes.map((n) => <li key={n}>{n}</li>)}
              </ul>
            </AlertDescription>
          </Alert>
        ) : null}
        {created?.length ? (
          <p data-slot="model-create-created" className="tw:m-0 tw:text-sm">
            {`Already created: ${created.join(', ')}`}
          </p>
        ) : null}
        {error ? (
          <Alert variant="destructive" data-slot="model-create-error">
            <CircleAlert />
            <AlertTitle>Could not create every table</AlertTitle>
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        ) : null}
        <AlertDialogFooter>
          <AlertDialogCancel disabled={running}>Cancel</AlertDialogCancel>
          <Button type="button" disabled={running} onClick={onCreate}>
            {running ? <Spinner className="tw:text-current" /> : null}
            Create
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
