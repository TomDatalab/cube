import type { ModelDefinition } from './modelGraph';

// The databases the model designs tables for; null for any other
// connection, which keeps the 7 portable types.
export type Dialect = 'postgres' | 'mysql' | null;

// What a type takes in parentheses: nothing, a size (maybe required), or a
// precision with an optional scale. The same list as the server's.
type Param = 'none' | 'size' | 'size!' | 'precision';

const POSTGRES: [string, Param][] = [
  ['smallint', 'none'], ['integer', 'none'], ['bigint', 'none'], ['serial', 'none'], ['bigserial', 'none'],
  ['numeric', 'precision'], ['decimal', 'precision'], ['real', 'none'], ['double precision', 'none'],
  ['boolean', 'none'], ['char', 'size'], ['varchar', 'size'], ['text', 'none'], ['date', 'none'],
  ['time', 'none'], ['timestamp', 'none'], ['timestamptz', 'none'], ['interval', 'none'], ['uuid', 'none'],
  ['json', 'none'], ['jsonb', 'none'], ['bytea', 'none'],
];

const MYSQL: [string, Param][] = [
  ['tinyint', 'size'], ['smallint', 'size'], ['mediumint', 'size'], ['int', 'size'], ['bigint', 'size'],
  ['decimal', 'precision'], ['float', 'none'], ['double', 'none'], ['boolean', 'none'], ['bit', 'size'],
  ['char', 'size'], ['varchar', 'size!'], ['tinytext', 'none'], ['text', 'none'], ['mediumtext', 'none'],
  ['longtext', 'none'], ['date', 'none'], ['time', 'none'], ['datetime', 'none'], ['timestamp', 'none'],
  ['year', 'none'], ['json', 'none'], ['blob', 'none'], ['binary', 'size'], ['varbinary', 'size!'],
];

const LISTS = { postgres: POSTGRES, mysql: MYSQL };

// The base type names of each dialect, for the type pickers.
export const TYPES: Record<'postgres' | 'mysql', string[]> = {
  postgres: POSTGRES.map(([name]) => name),
  mysql: MYSQL.map(([name]) => name),
};

// The types of models made before dialects: valid everywhere.
export const PORTABLE = ['text', 'integer', 'bigint', 'numeric', 'boolean', 'date', 'timestamp'];

export function validType(dialect: Dialect, type: string): boolean {
  const t = type.trim().toLowerCase();
  if (PORTABLE.includes(t)) {
    return true;
  }
  if (!dialect) {
    return false;
  }
  const open = t.indexOf('(');
  let base = t;
  let params: string | null = null;
  if (open >= 0) {
    if (!t.endsWith(')')) {
      return false;
    }
    base = t.slice(0, open).trim();
    params = t.slice(open + 1, -1);
  }
  const entry = LISTS[dialect].find(([name]) => name === base);
  if (!entry) {
    return false;
  }
  const param = entry[1];
  if (params === null) {
    return param !== 'size!';
  }
  const parts = params.split(',').map((p) => p.trim());
  if (parts.some((p) => !/^\d+$/.test(p))) {
    return false;
  }
  const [a, b] = parts.map(Number);
  if (param === 'size' || param === 'size!') {
    return parts.length === 1 && a >= 1;
  }
  if (param === 'precision') {
    return parts.length === 1 ? a >= 1 : parts.length === 2 && a >= 1 && b <= a;
  }
  return false;
}

const MAP: Record<'postgres' | 'mysql', Record<string, string>> = {
  postgres: { numeric: 'numeric(18,4)' },
  mysql: { integer: 'int', numeric: 'decimal(18,4)', timestamp: 'datetime' },
};

// A portable type as the dialect writes it, as the server creates it.
export function toDialect(dialect: Dialect, type: string): string {
  return dialect ? MAP[dialect][type] ?? type : type;
}

// The draft columns' portable types written in the dialect; the same
// definition when nothing changes.
export function convertDrafts(def: ModelDefinition, dialect: Dialect): ModelDefinition {
  if (!dialect) {
    return def;
  }
  let changed = false;
  const tables = def.tables.map((t) => {
    if (!t.draft?.columns.some((c) => toDialect(dialect, c.type) !== c.type)) {
      return t;
    }
    changed = true;
    return {
      ...t,
      draft: {
        ...t.draft,
        columns: t.draft.columns.map((c) => ({ ...c, type: toDialect(dialect, c.type) })),
      },
    };
  });
  return changed ? { ...def, tables } : def;
}
