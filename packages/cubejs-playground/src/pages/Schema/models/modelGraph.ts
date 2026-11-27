import { validType } from './dialect';

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
  // A portable type, or one of the connection's dialect (see dialect.ts).
  type: string;
  nullable: boolean;
  primaryKey: boolean;
  comment?: string;
  // SQL text, as Redgate takes it.
  default?: string;
  check?: string;
  autoIncrement?: boolean;
  // MySQL only.
  unsigned?: boolean;
  zerofill?: boolean;
};
export type UniqueKey = { name: string; columns: string[] };
export type IndexColumn = { name: string; desc?: boolean };
export type TableIndex = { name: string; columns: IndexColumn[]; unique: boolean };
export type TableCheck = { name: string; expression: string };
export type DraftTable = {
  columns: DraftColumn[];
  primaryKeyName?: string;
  uniqueKeys?: UniqueKey[];
  indexes?: TableIndex[];
  checks?: TableCheck[];
  scriptBefore?: string;
  scriptAfter?: string;
};

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
  // What the reference is as a foreign key, as Redgate shows it.
  id?: string;
  name?: string;
  comment?: string;
  // Of the child side.
  cardinality?: Cardinality;
  mandatory?: boolean;
  onUpdate?: RefAction;
  onDelete?: RefAction;
  fromRole?: string;
  toRole?: string;
  // More column pairs after from.column = to.column.
  pairs?: { from: string; to: string }[];
};

export const CARDINALITIES = ['0..*', '0..1', '1..*', '1..1'] as const;
export type Cardinality = (typeof CARDINALITIES)[number];
export const REF_ACTIONS = ['none', 'restrict', 'cascade', 'set null', 'set default'] as const;
export type RefAction = (typeof REF_ACTIONS)[number];

export type ModelSettings = {
  validation?: Record<string, boolean>;
  format?: { showTypes?: boolean; showNullable?: boolean };
  sqlGeneration?: { drop?: boolean; comments?: boolean };
  scripts?: { createBefore?: string; createAfter?: string; dropBefore?: string; dropAfter?: string };
};

export type Note = {
  id: string;
  x: number;
  y: number;
  width?: number;
  height?: number;
  z?: number;
  text: string;
  color?: TableColor;
};

export type Area = {
  id: string;
  name: string;
  comment?: string;
  x: number;
  y: number;
  width: number;
  height: number;
  z?: number;
  fill?: TableColor;
  border?: TableColor;
};

export type ViewColumn = { name: string; type: string };

export type ModelView = {
  id: string;
  name: string;
  schema: string;
  comment?: string;
  sql: string;
  columns: ViewColumn[];
  dependsOnTables: string[];
  dependsOnViews: string[];
  scriptBefore?: string;
  scriptAfter?: string;
  x: number;
  y: number;
  width?: number;
  height?: number;
  z?: number;
};

export const SOURCE_KINDS = ['table', 'joined', 'sql'] as const;
export type CubeSourceKind = (typeof SOURCE_KINDS)[number];
export const DIMENSION_TYPES = ['string', 'number', 'time', 'boolean', 'geo'] as const;
export type DimensionType = (typeof DIMENSION_TYPES)[number];
export const MEASURE_TYPES = [
  'count',
  'count_distinct',
  'count_distinct_approx',
  'sum',
  'avg',
  'min',
  'max',
  'number',
] as const;
export type MeasureType = (typeof MEASURE_TYPES)[number];
export const MEASURE_FORMATS = ['percent', 'currency'] as const;
export type MeasureFormat = (typeof MEASURE_FORMATS)[number];

// Where a cube reads from.
export type CubeSource = {
  kind: CubeSourceKind;
  // The table alias of a `table` cube, the primary table of a `joined` one.
  table?: string;
  // A column of `table`, used in auto mode.
  primaryKey?: string;
  // The query of a `sql` cube, and the aliases it reads.
  sql?: string;
  tables?: string[];
};

export type CubeDimension = {
  name: string;
  type: DimensionType;
  // `alias.column` in a `joined` cube, a plain column otherwise.
  column?: string;
  sql?: string;
  primaryKey?: boolean;
  title?: string;
};

export type CubeMeasure = {
  name: string;
  type: MeasureType;
  column?: string;
  sql?: string;
  title?: string;
  format?: MeasureFormat;
};

export type CubeSegment = { name: string; sql: string; title?: string };

// A cube drawn on the canvas, with its own members; none at all is auto mode.
export type CubeNode = {
  id: string;
  name: string;
  title?: string;
  description?: string;
  color?: TableColor;
  source: CubeSource;
  dimensions: CubeDimension[];
  measures: CubeMeasure[];
  segments: CubeSegment[];
  x: number;
  y: number;
  width?: number;
  height?: number;
  z?: number;
};

// A reference between two cubes (by id); `on` pairs dimensions of `from` and `to`.
export type CubeJoin = {
  id: string;
  from: string;
  to: string;
  relationship: Relationship;
  on: { from: string; to: string }[];
  points: Point[];
  hidden?: boolean;
};

export type ModelDefinition = {
  formatVersion?: 2;
  dataSource: string;
  description?: string;
  tables: ModelTable[];
  joins: ModelJoin[];
  settings?: ModelSettings;
  notes?: Note[];
  areas?: Area[];
  views?: ModelView[];
  cubes?: CubeNode[];
  cubeJoins?: CubeJoin[];
};

// The cubes of a model, with `f` applied to each one's source.
function mapCubeSources(def: ModelDefinition, f: (c: CubeNode) => CubeNode): Pick<ModelDefinition, 'cubes'> {
  return def.cubes ? { cubes: def.cubes.map(f) } : {};
}

// The cube's source without `keys`.
function withoutSource(c: CubeNode, keys: (keyof CubeSource)[]): CubeNode {
  const source = { ...c.source };
  for (const key of keys) {
    delete source[key];
  }
  return { ...c, source };
}

// The views of a model, with `f` applied to each one's table dependencies.
function mapViewTables(def: ModelDefinition, f: (aliases: string[]) => string[]): Pick<ModelDefinition, 'views'> {
  return def.views ? { views: def.views.map((v) => ({ ...v, dependsOnTables: f(v.dependsOnTables) })) } : {};
}

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
  return {
    def: {
      ...def,
      dataSource: ds,
      tables: [...def.tables, { alias, schema, table, x, y }],
    },
  };
}

export function removeTable(def: ModelDefinition, alias: string): ModelDefinition {
  const tables = def.tables.filter((t) => t.alias !== alias);
  const joins = def.joins.filter((j) => j.from.alias !== alias && j.to.alias !== alias);
  const views = mapViewTables(def, (list) => list.filter((a) => a !== alias));
  // A cube that read the table keeps its members; validation flags them.
  const cubes = mapCubeSources(def, (c) => {
    let out = c;
    if (out.source.table === alias) {
      out = withoutSource(out, ['table', 'primaryKey']);
    }
    if (out.source.tables?.includes(alias)) {
      const rest = out.source.tables.filter((a) => a !== alias);
      out = rest.length ? { ...out, source: { ...out.source, tables: rest } } : withoutSource(out, ['tables']);
    }
    return out;
  });
  return {
    ...def,
    ...views,
    ...cubes,
    dataSource: tables.length === 0 ? '' : def.dataSource,
    tables,
    joins,
  };
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
function patched<T extends object>(target: T, patch: Record<string, unknown>): T {
  const out = { ...target } as Record<string, unknown>;
  for (const [key, value] of Object.entries(patch)) {
    const empty = value === false || value === null || value === '' || (Array.isArray(value) && value.length === 0);
    if (empty) {
      delete out[key];
    } else if (value !== undefined) {
      out[key] = value;
    }
  }
  return out as T;
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
    if (!validType('postgres', c.type) && !validType('mysql', c.type)) {
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
  const viewClash = (def.views ?? []).some((v) => v.schema === schema && v.name === table);
  if (clash || viewClash) {
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
  return {
    def: {
      ...def,
      dataSource: ds,
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
  const lostKey = (c: CubeNode) => c.source.table === alias && c.source.primaryKey !== undefined
    && !names.has(c.source.primaryKey);
  return {
    def: {
      ...def,
      ...mapCubeSources(def, (c) => (lostKey(c) ? withoutSource(c, ['primaryKey']) : c)),
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
  const start = def.cubes?.find((c) => c.source.table)?.source.table ?? def.tables[0].alias;
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
  const prefix = `${alias}.`;
  const column = <M extends { column?: string }>(m: M): M => (m.column?.startsWith(prefix)
    ? { ...m, column: `${next}.${m.column.slice(prefix.length)}` }
    : m);
  const cubes = mapCubeSources(def, (c) => {
    const source = {
      ...c.source,
      ...(c.source.table === alias ? { table: next } : {}),
      ...(c.source.tables ? { tables: c.source.tables.map((a) => (a === alias ? next : a)) } : {}),
    };
    if (c.source.kind !== 'joined') {
      return { ...c, source };
    }
    return {
      ...c, source, dimensions: c.dimensions.map(column), measures: c.measures.map(column),
    };
  });
  return {
    def: {
      ...def,
      ...cubes,
      tables: def.tables.map((t) => (t.alias === alias ? { ...t, alias: next } : t)),
      joins: def.joins.map((j) => ({ ...j, from: end(j.from), to: end(j.to) })),
      ...mapViewTables(def, (list) => list.map((a) => (a === alias ? next : a))),
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
        });
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

export function removeShortcut(def: ModelDefinition, alias: string, id: string): ModelDefinition {
  return {
    ...def,
    tables: def.tables.map((t) => {
      if (t.alias !== alias || !t.shortcuts) {
        return t;
      }
      const shortcuts = t.shortcuts.filter((s) => s.id !== id);
      if (shortcuts.length) {
        return { ...t, shortcuts };
      }
      const { shortcuts: _drop, ...rest } = t;
      return rest;
    }),
  };
}
