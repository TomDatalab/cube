import type { ModelDefinition } from './modelGraph';

// The dialect a connection designs tables in, by its id (`postgres`,
// `clickhouse`, `snowflake`…), as the server names it; null for a connection
// with none, which keeps the 7 portable types.
export type Dialect = string | null;

// What a type takes in parentheses: nothing, a size (maybe required), a
// precision with an optional scale, another type of the dialect
// (`Nullable(String)`), or anything (`Enum8('a' = 1)`). The server's list.
type Param = 'none' | 'size' | 'size!' | 'precision' | 'type' | 'free';

const PORTABLE_NAMES = ['text', 'integer', 'bigint', 'numeric', 'boolean', 'date', 'timestamp'] as const;
type Portable = (typeof PORTABLE_NAMES)[number];

// What a dialect has, as the server sends it with the model
// (`dialects::Dialect::to_json`).
export type DialectSpec = {
  id: string;
  label: string;
  types: [string, Param][];
  portable: Record<Portable, string>;
  text: string;
  canCreate: boolean;
  canSql: boolean;
  reason?: string | null;
  features: {
    primaryKey: boolean;
    unique: boolean;
    check: boolean;
    foreignKey: boolean;
    index: boolean;
    comments: boolean;
    autoIncrement: boolean;
    unsigned: boolean;
    notNull: boolean;
    default: boolean;
    views: boolean;
  };
};

// The 7 portable types as a dialect writes them, in PORTABLE_NAMES order.
export function portableAs(...written: string[]): Record<Portable, string> {
  return Object.fromEntries(PORTABLE_NAMES.map((name, i) => [name, written[i] ?? name])) as Record<Portable, string>;
}

const ALL_FEATURES: DialectSpec['features'] = {
  primaryKey: true,
  unique: true,
  check: true,
  foreignKey: true,
  index: true,
  comments: true,
  autoIncrement: true,
  unsigned: false,
  notNull: true,
  default: true,
  views: true,
};

// Postgres and MySQL are known before the server says so (and in tests).
const BUILT_IN: DialectSpec[] = [
  {
    id: 'postgres',
    label: 'PostgreSQL',
    types: [
      ['smallint', 'none'], ['integer', 'none'], ['bigint', 'none'], ['serial', 'none'], ['bigserial', 'none'],
      ['numeric', 'precision'], ['decimal', 'precision'], ['real', 'none'], ['double precision', 'none'],
      ['boolean', 'none'], ['char', 'size'], ['varchar', 'size'], ['text', 'none'], ['date', 'none'],
      ['time', 'none'], ['timestamp', 'none'], ['timestamptz', 'none'], ['interval', 'none'], ['uuid', 'none'],
      ['json', 'none'], ['jsonb', 'none'], ['bytea', 'none'],
    ],
    portable: portableAs('text', 'integer', 'bigint', 'numeric(18,4)', 'boolean', 'date', 'timestamp'),
    text: 'text',
    canCreate: true,
    canSql: true,
    features: ALL_FEATURES,
  },
  {
    id: 'mysql',
    label: 'MySQL',
    types: [
      ['tinyint', 'size'], ['smallint', 'size'], ['mediumint', 'size'], ['int', 'size'], ['bigint', 'size'],
      ['decimal', 'precision'], ['float', 'none'], ['double', 'none'], ['boolean', 'none'], ['bit', 'size'],
      ['char', 'size'], ['varchar', 'size!'], ['tinytext', 'none'], ['text', 'none'], ['mediumtext', 'none'],
      ['longtext', 'none'], ['date', 'none'], ['time', 'none'], ['datetime', 'none'], ['timestamp', 'none'],
      ['year', 'none'], ['json', 'none'], ['blob', 'none'], ['binary', 'size'], ['varbinary', 'size!'],
    ],
    portable: portableAs('text', 'int', 'bigint', 'decimal(18,4)', 'boolean', 'date', 'datetime'),
    text: 'varchar(255)',
    canCreate: true,
    canSql: true,
    features: { ...ALL_FEATURES, unsigned: true },
  },
];

const registry = new Map<string, DialectSpec>(BUILT_IN.map((d) => [d.id, d]));

// The dialects the server sent with a model.
export function registerDialects(specs: Record<string, DialectSpec> | undefined): void {
  for (const spec of Object.values(specs ?? {})) {
    registry.set(spec.id, spec);
  }
}

export function dialectSpec(dialect: Dialect): DialectSpec | undefined {
  return dialect ? registry.get(dialect) : undefined;
}

// The types of models made before dialects: valid everywhere.
export const PORTABLE: string[] = [...PORTABLE_NAMES];

// The base type names of a dialect, for the type pickers.
export function typesOf(dialect: Dialect): string[] {
  const spec = dialectSpec(dialect);
  return spec ? spec.types.map(([name]) => name) : PORTABLE;
}

function splitType(type: string): { base: string; params: string | null } | null {
  const t = type.trim();
  const open = t.indexOf('(');
  if (open < 0) {
    return { base: t, params: null };
  }
  if (!t.endsWith(')')) {
    return null;
  }
  return { base: t.slice(0, open).trim(), params: t.slice(open + 1, -1) };
}

function validParams(param: Param, params: string): boolean {
  if ((param === 'size' || param === 'size!') && params.trim().toLowerCase() === 'max') {
    return true;
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

function validIn(spec: DialectSpec, type: string, depth: number): boolean {
  const parsed = depth > 4 ? null : splitType(type);
  if (!parsed) {
    return false;
  }
  const { base, params } = parsed;
  const entry = spec.types.find(([name]) => name.toLowerCase() === base.toLowerCase());
  if (!entry) {
    return false;
  }
  const param = entry[1];
  if (params === null) {
    return param !== 'size!' && param !== 'type';
  }
  if (param === 'free') {
    return params.trim() !== '';
  }
  if (param === 'type') {
    return validIn(spec, params, depth + 1);
  }
  return param !== 'none' && validParams(param, params);
}

export function validType(dialect: Dialect, type: string): boolean {
  if (PORTABLE.includes(type.trim().toLowerCase())) {
    return true;
  }
  const spec = dialectSpec(dialect);
  return spec ? validIn(spec, type, 0) : false;
}

// A type some known dialect has: what a table may be designed with before
// its connection says which.
export function validInAnyDialect(type: string): boolean {
  return [...registry.keys()].some((id) => validType(id, type));
}

// A portable type as the dialect writes it, as the server creates it.
export function toDialect(dialect: Dialect, type: string): string {
  const spec = dialectSpec(dialect);
  if (!spec) {
    return type;
  }
  return (PORTABLE_NAMES as readonly string[]).includes(type) ? spec.portable[type as Portable] : type;
}

// The draft columns' portable types written in the dialect; the same
// definition when nothing changes.
export function convertDrafts(def: ModelDefinition, dialect: Dialect): ModelDefinition {
  if (!dialectSpec(dialect)) {
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
