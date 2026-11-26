import { CircleAlert, Copy, Download } from 'lucide-react';

import { Alert, AlertDescription } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from '@/components/ui/dialog';
import { Spinner, notify } from '@/components/kit';

import { CodeView } from '../NumberedCode';

export type SqlOptions = { drop: boolean; onlyNew: boolean };

export type SqlDialogProps = {
  open: boolean;
  // The file name the download takes, without `.sql`.
  name: string;
  // The tables the script is limited to (a table's "SQL preview").
  only?: string[];
  script: string | null;
  error: string | null;
  loading: boolean;
  options: SqlOptions;
  onOptions(next: SqlOptions): void;
  onClose(): void;
};

// Redgate's "Generate SQL script": the model's DDL in its connection's
// dialect, with its options, to copy or download. It runs nothing.
export function SqlDialog({
  open, name, only, script, error, loading, options, onOptions, onClose,
}: SqlDialogProps) {
  const download = () => {
    if (!script) {
      return;
    }
    const url = URL.createObjectURL(new Blob([script], { type: 'application/sql' }));
    const a = document.createElement('a');
    a.href = url;
    a.download = `${name || 'model'}.sql`;
    a.click();
    URL.revokeObjectURL(url);
  };
  const copy = async () => {
    if (script) {
      await navigator.clipboard?.writeText(script);
      notify.success('SQL copied');
    }
  };
  return (
    <Dialog open={open} onOpenChange={(next) => !next && onClose()}>
      <DialogContent className="tw:sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle>{only?.length ? `SQL preview: ${only.map((o) => o.replace(/^view:/, '')).join(', ')}` : 'Generate SQL script'}</DialogTitle>
          <DialogDescription>The model as SQL in its connection&apos;s dialect. Nothing runs.</DialogDescription>
        </DialogHeader>
        <div className="tw:flex tw:flex-wrap tw:items-center tw:gap-4 tw:text-sm">
          <span className="tw:flex tw:items-center tw:gap-1.5">
            <input
              type="checkbox"
              aria-label="Include DROP statements"
              checked={options.drop}
              onChange={(e) => onOptions({ ...options, drop: e.target.checked })}
            />
            Include DROP statements
          </span>
          {only?.length ? null : (
            <span className="tw:flex tw:items-center tw:gap-1.5">
              <input
                type="checkbox"
                aria-label="Only new tables"
                checked={options.onlyNew}
                onChange={(e) => onOptions({ ...options, onlyNew: e.target.checked })}
              />
              Only new tables
            </span>
          )}
          {loading ? <Spinner /> : null}
        </div>
        {error ? (
          <Alert variant="destructive">
            <CircleAlert />
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        ) : null}
        <div data-slot="model-sql-script" className="tw:max-h-[60vh] tw:overflow-auto tw:rounded-md tw:border">
          <CodeView code={script ?? ''} />
        </div>
        <DialogFooter>
          <Button variant="outline" disabled={!script} onClick={copy}>
            <Copy />
            Copy
          </Button>
          <Button disabled={!script} onClick={download}>
            <Download />
            Download
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
