export type Relationship = 'one_to_one' | 'one_to_many' | 'many_to_one';
export type JoinKind = 'left' | 'inner';

export const COLUMN_TYPES = [
  'text',
  'integer',
  'bigint',
  'numeric',
  'boolean',
  'date',
  'timestamp',
] as const;
export type ColumnType = (typeof COLUMN_TYPES)[number];

export const TABLE_COLORS = [
  'slate',
  'blue',
  'green',
  'amber',
  'red',
  'violet',
  'pink',
  'teal',
] as const;
export type TableColor = (typeof TABLE_COLORS)[number];

export type DraftColumn = {
  name: string;
  type: ColumnType;
  nullable: boolean;
  primaryKey: boolean;
};
export type DraftTable = { columns: DraftColumn[] };

export type Point = { x: number; y: number };
// The same table shown again elsewhere on the canvas.
export type Shortcut = { id: string; x: number; y: number };

export type ModelTable = {
  alias: string;
  schema: string;
  table: string;
  x: number;
  y: number;
  color?: TableColor;
  draft?: DraftTable;
  comment?: string;
  // A size fixed on the canvas; none follows the content.
  width?: number;
  height?: number;
  shortcuts?: Shortcut[];
};

export type End = { alias: string; column: string };

export type ModelJoin = {
  from: End;
  to: End;
  relationship: Relationship;
  join: JoinKind;
  // Bends of the line, in canvas coordinates.
  points?: Point[];
  // The line leaves its tables on the other side.
  swapped?: boolean;
  // Not drawn on the canvas; still a link.
  hidden?: boolean;
};

export type ModelDefinition = {
  formatVersion?: 2;
  dataSource: string;
  description?: string;
  tables: ModelTable[];
  primary?: string | null;
  primaryKey?: string | null;
  joins: ModelJoin[];
};

export function emptyDefinition(): ModelDefinition {
  return { dataSource: '', tables: [], joins: [] };
}

export function addTable(
  def: ModelDefinition,
  ds: string,
  schema: string,
  table: string,
  x: number,
  y: number
): { def: ModelDefinition; error?: string } {
  if (def.tables.length > 0 && def.dataSource !== ds) {
    return {
      def,
      error: `Tables of one connection only: this model reads ${def.dataSource}`,
    };
  }
  const taken = new Set(def.tables.map((t) => t.alias));
  let alias = table;
  for (let n = 2; taken.has(alias); n++) {
    alias = `${table}_${n}`;
  }
  const first = def.tables.length === 0;
  return {
    def: {
      ...def,
      dataSource: ds,
      primary: first ? alias : def.primary,
      tables: [...def.tables, { alias, schema, table, x, y }],
    },
  };
}

export function removeTable(def: ModelDefinition, alias: string): ModelDefinition {
  const tables = def.tables.filter((t) => t.alias !== alias);
  const joins = def.joins.filter((j) => j.from.alias !== alias && j.to.alias !== alias);
  if (def.primary !== alias) {
    return { ...def, tables, joins };
  }
  return {
    ...def,
    dataSource: tables.length === 0 ? '' : def.dataSource,
    tables,
    joins,
    primary: tables[0]?.alias ?? null,
    primaryKey: null,
  };
}

export function setPrimary(def: ModelDefinition, alias: string): ModelDefinition {
  if (def.primary === alias || !def.tables.some((t) => t.alias === alias)) {
    return def;
  }
  return { ...def, primary: alias, primaryKey: null };
}

export function canConnect(def: ModelDefinition, from: End, to: End): string | null {
  for (const end of [from, to]) {
    if (!def.tables.some((t) => t.alias === end.alias)) {
      return `Unknown table: ${end.alias}`;
    }
  }
  if (from.alias === to.alias) {
    return 'A table cannot link to itself';
  }
  const samePair = (j: ModelJoin) => {
    const forward = j.from.alias === from.alias && j.to.alias === to.alias;
    const backward = j.from.alias === to.alias && j.to.alias === from.alias;

    return forward || backward;
  };
  if (def.joins.some(samePair)) {
    return 'These tables are already linked';
  }
  const parent = new Map<string, string>();
  const find = (a: string): string => {
    let r = a;
    while (parent.has(r) && parent.get(r) !== r) {
      r = parent.get(r) as string;
    }
    parent.set(a, r);
    return r;
  };
  for (const j of def.joins) {
    parent.set(find(j.from.alias), find(j.to.alias));
  }
  if (find(from.alias) === find(to.alias)) {
    return 'This link would create a cycle';
  }
  return null;
}

export function connect(
  def: ModelDefinition,
  from: End,
  to: End,
  relationship: Relationship,
  join: JoinKind
): ModelDefinition {
  return {
    ...def,
    joins: [...def.joins, { from: { ...from }, to: { ...to }, relationship, join }],
  };
}

export type JoinPatch = Partial<Pick<ModelJoin, 'relationship' | 'join' | 'points' | 'swapped' | 'hidden'>>;

// Copies `value` into `key`, or drops the key for an empty or false value.
function patched<T extends object>(target: T, patch: Partial<T>): T {
  const out: any = { ...target };
  for (const [key, value] of Object.entries(patch)) {
    if (value === undefined) {
      continue;
    }
    const empty = value === false || value === null || value === '' || (Array.isArray(value) && value.length === 0);
    if (empty) {
      delete out[key];
    } else {
      out[key] = value;
    }
  }
  return out;
}

export function setJoin(def: ModelDefinition, index: number, patch: JoinPatch): ModelDefinition {
  return {
    ...def,
    joins: def.joins.map((j, i) => (i === index ? patched(j, patch) : j)),
  };
}

export function updateJoin(
  def: ModelDefinition,
  index: number,
  patch: Partial<Pick<ModelJoin, 'relationship' | 'join'>>
): ModelDefinition {
  return setJoin(def, index, patch);
}

export function showAllJoins(def: ModelDefinition): ModelDefinition {
  return { ...def, joins: def.joins.map((j) => patched(j, { hidden: false })) };
}

export function removeJoin(def: ModelDefinition, index: number): ModelDefinition {
  return { ...def, joins: def.joins.filter((_, i) => i !== index) };
}

const REL_LABEL: Record<Relationship, string> = {
  one_to_one: '1:1',
  one_to_many: '1:N',
  many_to_one: 'N:1',
};

export function joinLabel(j: ModelJoin): string {
  return `${REL_LABEL[j.relationship]} · ${j.join.toUpperCase()}`;
}

export function unlinked(def: ModelDefinition): string[] {
  if (def.tables.length === 0) {
    return [];
  }
  const start = def.primary ?? def.tables[0].alias;
  const seen = new Set([start]);
  const queue = [start];
  while (queue.length) {
    const cur = queue.shift() as string;
    for (const j of def.joins) {
      let other: string | null = null;
      if (j.from.alias === cur) {
        other = j.to.alias;
      } else if (j.to.alias === cur) {
        other = j.from.alias;
      }
      if (other && !seen.has(other)) {
        seen.add(other);
        queue.push(other);
      }
    }
  }
  return def.tables.map((t) => t.alias).filter((a) => !seen.has(a));
}

export const IDENTIFIER = /^[A-Za-z_][A-Za-z0-9_]*$/;
const MAX_IDENTIFIER = 63;

function badIdentifier(name: string): boolean {
  return !IDENTIFIER.test(name) || name.length > MAX_IDENTIFIER;
}

export function draftProblem(
  schema: string,
  table: string,
  columns: DraftColumn[],
  def: ModelDefinition,
  editingAlias?: string
): string | null {
  if (badIdentifier(schema)) {
    return `Invalid schema name: ${schema}`;
  }
  if (badIdentifier(table)) {
    return `Invalid table name: ${table}`;
  }
  if (columns.length === 0) {
    return 'Add at least one column';
  }
  const seen = new Set<string>();
  for (const c of columns) {
    if (badIdentifier(c.name)) {
      return `Invalid column name: ${c.name}`;
    }
    if (!(COLUMN_TYPES as readonly string[]).includes(c.type)) {
      return `Invalid type for ${c.name}: ${c.type}`;
    }
    if (seen.has(c.name.toLowerCase())) {
      return `Duplicate column: ${c.name}`;
    }
    seen.add(c.name.toLowerCase());
  }
  const clash = def.tables.some(
    (t) => t.alias !== editingAlias && t.schema === schema && t.table === table
  );
  if (clash) {
    return `${schema}.${table} is already in this model`;
  }
  return null;
}

export function addDraftTable(
  def: ModelDefinition,
  ds: string,
  schema: string,
  table: string,
  columns: DraftColumn[],
  x: number,
  y: number
): { def: ModelDefinition; error?: string } {
  if (def.tables.length > 0 && def.dataSource !== ds) {
    return {
      def,
      error: `Tables of one connection only: this model reads ${def.dataSource}`,
    };
  }
  const problem = draftProblem(schema, table, columns, def);
  if (problem) {
    return { def, error: problem };
  }
  const taken = new Set(def.tables.map((t) => t.alias));
  let alias = table;
  for (let n = 2; taken.has(alias); n++) {
    alias = `${table}_${n}`;
  }
  const first = def.tables.length === 0;
  return {
    def: {
      ...def,
      dataSource: ds,
      primary: first ? alias : def.primary,
      tables: [...def.tables, { alias, schema, table, x, y, draft: { columns } }],
    },
  };
}

export function updateDraftTable(
  def: ModelDefinition,
  alias: string,
  schema: string,
  table: string,
  columns: DraftColumn[]
): { def: ModelDefinition; error?: string } {
  if (!def.tables.some((t) => t.alias === alias && t.draft)) {
    return { def, error: `Not a draft table: ${alias}` };
  }
  const problem = draftProblem(schema, table, columns, def, alias);
  if (problem) {
    return { def, error: problem };
  }
  const names = new Set(columns.map((c) => c.name));
  const gone = (e: End) => e.alias === alias && !names.has(e.column);
  const keepsPk = def.primary !== alias || !def.primaryKey || names.has(def.primaryKey);
  return {
    def: {
      ...def,
      primaryKey: keepsPk ? def.primaryKey : null,
      tables: def.tables.map((t) => {
        if (t.alias !== alias) {
          return t;
        }

        return { ...t, schema, table, draft: { columns } };
      }),
      joins: def.joins.filter((j) => !gone(j.from) && !gone(j.to)),
    },
  };
}

export function setColor(
  def: ModelDefinition,
  alias: string,
  color: TableColor | undefined
): ModelDefinition {
  return {
    ...def,
    tables: def.tables.map((t) => {
      if (t.alias !== alias) {
        return t;
      }
      if (color === undefined) {
        const { color: _drop, ...rest } = t;

        return rest;
      }
      return { ...t, color };
    }),
  };
}

export function drafts(def: ModelDefinition): ModelTable[] {
  return def.tables.filter((t) => t.draft !== undefined);
}

const LAYER_WIDTH = 320;
const GAP = 40;
const UNKNOWN_COLUMNS = 6;

function tableHeight(t: ModelTable): number {
  return 44 + 24 * (t.draft ? t.draft.columns.length : UNKNOWN_COLUMNS);
}

export function layout(def: ModelDefinition): ModelDefinition {
  if (def.tables.length === 0) {
    return def;
  }
  const start = def.primary ?? def.tables[0].alias;
  const depth = new Map<string, number>([[start, 0]]);
  const queue = [start];
  while (queue.length) {
    const cur = queue.shift() as string;
    for (const j of def.joins) {
      let other: string | null = null;
      if (j.from.alias === cur) {
        other = j.to.alias;
      } else if (j.to.alias === cur) {
        other = j.from.alias;
      }
      if (other && !depth.has(other)) {
        depth.set(other, (depth.get(cur) as number) + 1);
        queue.push(other);
      }
    }
  }
  let last = 0;
  for (const d of depth.values()) {
    last = Math.max(last, d);
  }
  const loose = def.tables.some((t) => !depth.has(t.alias));
  const looseLayer = loose ? last + 1 : last;
  const nextY = new Map<number, number>();
  return {
    ...def,
    tables: def.tables.map((t) => {
      const layer = depth.get(t.alias) ?? looseLayer;
      const y = nextY.get(layer) ?? 0;
      nextY.set(layer, y + tableHeight(t) + GAP);

      return { ...t, x: layer * LAYER_WIDTH, y };
    }),
  };
}

export function withFormat(def: ModelDefinition): ModelDefinition {
  return { ...def, formatVersion: 2 };
}

export function renameTable(
  def: ModelDefinition,
  alias: string,
  next: string
): { def: ModelDefinition; error?: string } {
  if (next === alias) {
    return { def };
  }
  if (badIdentifier(next)) {
    return { def, error: `Invalid name: ${next}` };
  }
  if (def.tables.some((t) => t.alias === next)) {
    return { def, error: `${next} is already in this model` };
  }
  const end = (e: End): End => (e.alias === alias ? { ...e, alias: next } : e);
  return {
    def: {
      ...def,
      primary: def.primary === alias ? next : def.primary,
      tables: def.tables.map((t) => (t.alias === alias ? { ...t, alias: next } : t)),
      joins: def.joins.map((j) => ({ ...j, from: end(j.from), to: end(j.to) })),
    },
  };
}

export function setTableComment(def: ModelDefinition, alias: string, comment: string): ModelDefinition {
  return {
    ...def,
    tables: def.tables.map((t) => (t.alias === alias ? patched(t, { comment }) : t)),
  };
}

export type Place = { x?: number; y?: number; width?: number | null; height?: number | null };

// Moves and sizes tables, keyed by alias, and moves shortcuts, keyed by
// `alias~id`; a null size goes back to following the content.
export function placeTables(def: ModelDefinition, places: Map<string, Place>): ModelDefinition {
  const round = (n: number | undefined) => (n === undefined ? undefined : Math.round(n));
  return {
    ...def,
    tables: def.tables.map((t) => {
      let out: ModelTable = t;
      const own = places.get(t.alias);
      if (own) {
        out = patched(out, {
          x: round(own.x),
          y: round(own.y),
          width: own.width === null ? null : round(own.width),
          height: own.height === null ? null : round(own.height),
        } as any);
      }
      if (t.shortcuts?.some((sc) => places.has(`${t.alias}~${sc.id}`))) {
        out = {
          ...out,
          shortcuts: t.shortcuts.map((sc) => {
            const p = places.get(`${t.alias}~${sc.id}`);
            return p ? { ...sc, x: round(p.x) ?? sc.x, y: round(p.y) ?? sc.y } : sc;
          }),
        };
      }
      return out;
    }),
  };
}
