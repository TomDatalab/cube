import { useEffect, useRef, useState } from 'react';
import { Crosshair, Download, Play } from 'lucide-react';

import { Button } from '@/components/ui/button';
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from '@/components/ui/dialog';
import {
  Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
} from '@/components/ui/table';
import { Textarea } from '@/components/ui/textarea';
import { Spinner } from '@/components/kit';

import { download } from '../export/download';
import { emptyDefinition } from '../modelGraph';
import { toCsv } from './csv';
import { ExplorerTable, explorerTables } from './explorerTables';
import { QueryResult, runQuery } from './runQuery';

export const DEFAULT_QUERY = 'SELECT * FROM tables';
// The tables and their columns, for the chips.
const SCHEMA = explorerTables(emptyDefinition(), () => []);

export type ExplorerDialogProps = {
  open: boolean;
  // The model's name: the CSV is `{name}-explorer.csv`.
  name: string;
  // The model as tables, read at each run so a query sees the latest edit.
  tables(): ExplorerTable[];
  // Selects and centers a canvas node.
  onFind(nodeId: string): void;
  onClose(): void;
};

// Redgate's Model explorer: SQL over the model itself, in the browser. It
// reads the model and changes nothing.
export function ExplorerDialog({
  open, name, tables, onFind, onClose,
}: ExplorerDialogProps) {
  const [query, setQuery] = useState(DEFAULT_QUERY);
  const [result, setResult] = useState<QueryResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [running, setRunning] = useState(false);
  const [selected, setSelected] = useState<number | null>(null);
  const editor = useRef<HTMLTextAreaElement>(null);
  // Only the latest run answers.
  const runs = useRef(0);
  const latest = useRef({ query, tables });
  latest.current = { query, tables };

  const run = async () => {
    const id = ++runs.current;
    setRunning(true);
    try {
      const answer = await runQuery(latest.current.tables(), latest.current.query);
      if (id === runs.current) {
        setResult(answer);
        setError(null);
        setSelected(null);
      }
    } catch (e) {
      if (id === runs.current) {
        setResult(null);
        setError(e instanceof Error ? e.message : String(e));
        setSelected(null);
      }
    } finally {
      if (id === runs.current) {
        setRunning(false);
      }
    }
  };

  useEffect(() => {
    if (open) {
      run();
    }
    // Runs once per opening, with the query the explorer kept.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const nodeColumn = result ? result.columns.indexOf('node') : -1;
  const nodeId = selected !== null && nodeColumn >= 0 ? result?.rows[selected]?.[nodeColumn] : null;

  const find = () => {
    if (nodeId !== null && nodeId !== undefined) {
      onClose();
      onFind(String(nodeId));
    }
  };
  const exportCsv = () => {
    if (result) {
      download(
        new Blob([toCsv(result.columns, result.rows)], { type: 'text/csv;charset=utf-8' }),
        `${name || 'model'}-explorer.csv`
      );
    }
  };
  const write = (tableName: string) => {
    setQuery(`SELECT * FROM ${tableName}`);
    editor.current?.focus();
  };

  return (
    <Dialog open={open} onOpenChange={(next) => !next && onClose()}>
      <DialogContent className="tw:sm:max-w-5xl">
        <DialogHeader>
          <DialogTitle>Model explorer</DialogTitle>
          <DialogDescription>
            SQL over the model itself (SQLite), in the browser. Ctrl + Enter runs. Nothing changes.
          </DialogDescription>
        </DialogHeader>
        <div data-slot="model-explorer-tables" className="tw:flex tw:flex-wrap tw:gap-1">
          {SCHEMA.map((t) => (
            <button
              key={t.name}
              type="button"
              title={t.columns.join(', ')}
              className="tw:cursor-pointer tw:rounded-sm tw:border tw:bg-muted tw:px-1.5 tw:py-0.5 tw:font-mono tw:text-xs tw:text-muted-foreground tw:hover:bg-accent tw:hover:text-accent-foreground"
              onClick={() => write(t.name)}
            >
              {t.name}
            </button>
          ))}
        </div>
        <Textarea
          ref={editor}
          aria-label="Query"
          spellCheck={false}
          value={query}
          className="tw:min-h-24 tw:font-mono tw:text-xs"
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
              e.preventDefault();
              run();
            }
          }}
        />
        <div className="tw:flex tw:items-center tw:gap-2">
          <Button size="sm" disabled={running} onClick={() => run()}>
            <Play />
            Run
          </Button>
          {running ? <Spinner /> : null}
          {result ? (
            <span className="tw:text-xs tw:text-muted-foreground">
              {result.rows.length === 1 ? '1 row' : `${result.rows.length} rows`}
            </span>
          ) : null}
        </div>
        {error ? <p role="alert" className="tw:m-0 tw:font-mono tw:text-xs tw:text-destructive">{error}</p> : null}
        <div data-slot="model-explorer-grid" className="tw:max-h-[45vh] tw:overflow-auto tw:rounded-md tw:border">
          {result ? (
            <Table className="tw:font-mono tw:text-xs">
              <TableHeader className="tw:sticky tw:top-0 tw:bg-card">
                <TableRow>
                  {result.columns.map((c, i) => (
                    // Columns may share a name (SELECT a.name, b.name).
                    // eslint-disable-next-line react/no-array-index-key
                    <TableHead key={`${c}-${i}`} className="tw:h-8">{c}</TableHead>
                  ))}
                </TableRow>
              </TableHeader>
              <TableBody>
                {result.rows.map((row, r) => (
                  <TableRow
                    // Rows have no identity but their place.
                    // eslint-disable-next-line react/no-array-index-key
                    key={r}
                    aria-selected={selected === r}
                    data-state={selected === r ? 'selected' : undefined}
                    className="tw:cursor-pointer"
                    onClick={() => setSelected(r)}
                  >
                    {row.map((v, i) => (
                      // eslint-disable-next-line react/no-array-index-key
                      <TableCell key={i} className="tw:py-1">
                        {v === null ? <span className="tw:italic tw:text-muted-foreground">NULL</span> : String(v)}
                      </TableCell>
                    ))}
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          ) : null}
        </div>
        <DialogFooter>
          <Button
            variant="outline"
            disabled={nodeId === null || nodeId === undefined}
            title={nodeColumn < 0 ? 'The result has no node column' : undefined}
            onClick={find}
          >
            <Crosshair />
            Find in diagram
          </Button>
          <Button variant="outline" disabled={!result} onClick={exportCsv}>
            <Download />
            Export to CSV
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
