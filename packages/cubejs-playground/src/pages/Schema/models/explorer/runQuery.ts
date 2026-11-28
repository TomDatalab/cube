import type { SqlJsStatic } from 'sql.js';
import wasmUrl from 'sql.js/dist/sql-wasm.wasm?url';

import type { Cell, ExplorerTable } from './explorerTables';

export type QueryResult = { columns: string[]; rows: Cell[][] };

let library: Promise<SqlJsStatic> | null = null;

// sql.js and its wasm load on the first query only, so opening the editor
// costs nothing. Tests run its Node build, which finds the wasm itself.
function sqlJs(): Promise<SqlJsStatic> {
  if (!library) {
    library = import('sql.js')
      .then(({ default: initSqlJs }) => (import.meta.env.MODE === 'test'
        ? initSqlJs()
        : initSqlJs({ locateFile: () => wasmUrl })))
      .catch((e) => {
        // A failed download may succeed on the next Run.
        library = null;
        throw e;
      });
  }
  return library;
}

const quote = (name: string) => `"${name.replace(/"/g, '""')}"`;

const cell = (v: unknown): Cell => {
  if (v === null || v === undefined || typeof v === 'number' || typeof v === 'string') {
    return (v ?? null) as Cell;
  }
  return String(v);
};

// Runs `sql` over the model's tables in a fresh in-memory database and
// answers the last statement's result; rejects with SQLite's message.
export async function runQuery(tables: ExplorerTable[], sql: string): Promise<QueryResult> {
  const SQL = await sqlJs();
  const db = new SQL.Database();
  try {
    for (const t of tables) {
      db.run(`CREATE TABLE ${quote(t.name)} (${t.columns.map(quote).join(', ')})`);
      if (t.rows.length) {
        const insert = db.prepare(
          `INSERT INTO ${quote(t.name)} VALUES (${t.columns.map(() => '?').join(', ')})`
        );
        try {
          for (const row of t.rows) {
            insert.run(row);
          }
        } finally {
          insert.free();
        }
      }
    }
    let result: QueryResult = { columns: [], rows: [] };
    for (const stmt of db.iterateStatements(sql)) {
      try {
        const rows: Cell[][] = [];
        while (stmt.step()) {
          rows.push(stmt.get().map(cell));
        }
        result = { columns: stmt.getColumnNames(), rows };
      } finally {
        stmt.free();
      }
    }
    return result;
  } catch (e) {
    throw new Error(e instanceof Error ? e.message : String(e));
  } finally {
    db.close();
  }
}
