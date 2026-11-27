import { cubeKeyOf } from './cubes';
import type { Dialect } from './dialect';
import type {
  CubeNode, DraftColumn, DraftTable, End, ModelDefinition, ModelJoin, ModelTable, TableCheck, TableIndex, UniqueKey,
} from './modelGraph';

type Columns = { name: string; type: string }[];

// The cubes of `def` that read `alias` as their table, through `edit`.
function withCubesOn(def: ModelDefinition, alias: string, edit: (c: CubeNode) => CubeNode): ModelDefinition {
  if (!def.cubes) {
    return def;
  }
  return {
    ...def,
    cubes: def.cubes.map((c) => (c.source.kind !== 'sql' && c.source.table === alias ? edit(c) : c)),
  };
}

// The draft of `alias` through `edit`; other tables and non-drafts as they are.
function withDraft(def: ModelDefinition, alias: string, edit: (d: DraftTable) => DraftTable): ModelDefinition {
  return {
    ...def,
    tables: def.tables.map((t) => (t.alias === alias && t.draft ? { ...t, draft: edit(t.draft) } : t)),
  };
}

// Drops the keys of `obj` whose value is empty: '', undefined, or false for
// the optional switches.
const OPTIONAL_FLAGS = ['autoIncrement', 'unsigned', 'zerofill', 'desc', 'hidden', 'swapped'];
function tidy<T extends object>(obj: T): T {
  const out = { ...obj } as Record<string, unknown>;
  for (const [key, value] of Object.entries(out)) {
    const emptyList = Array.isArray(value) && value.length === 0;
    if (value === undefined || value === '' || emptyList || (value === false && OPTIONAL_FLAGS.includes(key))) {
      delete out[key];
    }
  }
  return out as T;
}

function firstFree(base: string, taken: (name: string) => boolean, from = 1): string {
  let n = from;
  while (taken(`${base}${n}`)) {
    n += 1;
  }
  return `${base}${n}`;
}

export function setColumn(
  def: ModelDefinition,
  alias: string,
  index: number,
  patch: Partial<DraftColumn>
): ModelDefinition {
  return withDraft(def, alias, (d) => ({
    ...d,
    columns: d.columns.map((c, i) => (i === index ? tidy({ ...c, ...patch }) : c)),
  }));
}

export function addColumn(def: ModelDefinition, alias: string, dialect: Dialect): ModelDefinition {
  return withDraft(def, alias, (d) => ({
    ...d,
    columns: [...d.columns, {
      name: firstFree('column_', (n) => d.columns.some((c) => c.name === n)),
      type: dialect === 'mysql' ? 'varchar(255)' : 'text',
      nullable: true,
      primaryKey: false,
    }],
  }));
}

export function moveColumn(def: ModelDefinition, alias: string, from: number, to: number): ModelDefinition {
  return withDraft(def, alias, (d) => {
    if (to < 0 || to >= d.columns.length || from === to) {
      return d;
    }
    const columns = [...d.columns];
    const [moved] = columns.splice(from, 1);
    columns.splice(to, 0, moved);
    return { ...d, columns };
  });
}

// Keys and indexes over the columns `keep` says stay, renamed by `rename`;
// the ones left without columns go.
function remapDraft(d: DraftTable, rename: (c: string) => string | null): DraftTable {
  const uniqueKeys = (d.uniqueKeys ?? []).map((k) => ({
    ...k, columns: k.columns.map(rename).filter((c): c is string => c !== null),
  })).filter((k) => k.columns.length > 0);
  const indexes = (d.indexes ?? []).map((ix) => ({
    ...ix,
    columns: ix.columns.flatMap((c) => {
      const name = rename(c.name);
      return name === null ? [] : [{ ...c, name }];
    }),
  })).filter((ix) => ix.columns.length > 0);
  return tidy({ ...d, uniqueKeys, indexes });
}

export function renameColumn(def: ModelDefinition, alias: string, from: string, to: string): ModelDefinition {
  if (from === to) {
    return def;
  }
  const rename = (c: string) => (c === from ? to : c);
  const next = withDraft(def, alias, (d) => remapDraft({
    ...d,
    columns: d.columns.map((c) => (c.name === from ? { ...c, name: to } : c)),
  }, rename));
  const end = (e: End): End => (e.alias === alias && e.column === from ? { ...e, column: to } : e);
  // The cube key and the members of the column follow it: a `joined` cube
  // names `alias.column` of any table it reaches, a `table` cube on `alias`
  // the plain column.
  const cubeColumn = (c: CubeNode): CubeNode => {
    const own = c.source.kind !== 'sql' && c.source.table === alias;
    let old: string | null = null;
    let renamed = '';
    if (c.source.kind === 'joined') {
      old = `${alias}.${from}`;
      renamed = `${alias}.${to}`;
    } else if (own) {
      old = from;
      renamed = to;
    }
    const member = <M extends { column?: string }>(m: M): M => (old !== null && m.column === old
      ? { ...m, column: renamed }
      : m);
    return {
      ...c,
      source: own && c.source.primaryKey === from ? { ...c.source, primaryKey: to } : c.source,
      dimensions: c.dimensions.map(member),
      measures: c.measures.map(member),
    };
  };
  return {
    ...next,
    ...(next.cubes ? { cubes: next.cubes.map(cubeColumn) } : {}),
    joins: next.joins.map((j) => ({
      ...j,
      from: end(j.from),
      to: end(j.to),
      ...(j.pairs ? {
        pairs: j.pairs.map((p) => ({
          from: j.from.alias === alias ? rename(p.from) : p.from,
          to: j.to.alias === alias ? rename(p.to) : p.to,
        })),
      } : {}),
    })),
  };
}

export function removeColumn(def: ModelDefinition, alias: string, name: string): ModelDefinition {
  const next = withDraft(def, alias, (d) => remapDraft({
    ...d,
    columns: d.columns.filter((c) => c.name !== name),
  }, (c) => (c === name ? null : c)));
  const fromUses = (j: ModelJoin) => j.from.alias === alias
    && (j.from.column === name || Boolean(j.pairs?.some((p) => p.from === name)));
  const toUses = (j: ModelJoin) => j.to.alias === alias
    && (j.to.column === name || Boolean(j.pairs?.some((p) => p.to === name)));
  const uses = (j: ModelJoin) => fromUses(j) || toUses(j);
  // A member of the column stays, for the validation to flag; the key goes.
  const dropKey = (c: CubeNode) => {
    if (c.source.primaryKey !== name) {
      return c;
    }
    const { primaryKey: _drop, ...source } = c.source;
    return { ...c, source };
  };
  return {
    ...withCubesOn(next, alias, dropKey),
    joins: next.joins.filter((j) => !uses(j)),
  };
}

export function setPrimaryKeyName(def: ModelDefinition, alias: string, name: string): ModelDefinition {
  return withDraft(def, alias, (d) => tidy({ ...d, primaryKeyName: name }));
}

export function setUniqueKeys(def: ModelDefinition, alias: string, keys: UniqueKey[]): ModelDefinition {
  return withDraft(def, alias, (d) => tidy({ ...d, uniqueKeys: keys }));
}

export function setIndexes(def: ModelDefinition, alias: string, indexes: TableIndex[]): ModelDefinition {
  return withDraft(def, alias, (d) => tidy({ ...d, indexes }));
}

export function setChecks(def: ModelDefinition, alias: string, checks: TableCheck[]): ModelDefinition {
  return withDraft(def, alias, (d) => tidy({ ...d, checks }));
}

export function setTableScripts(
  def: ModelDefinition,
  alias: string,
  scripts: { before?: string; after?: string }
): ModelDefinition {
  return withDraft(def, alias, (d) => tidy({
    ...d,
    ...(scripts.before !== undefined ? { scriptBefore: scripts.before } : {}),
    ...(scripts.after !== undefined ? { scriptAfter: scripts.after } : {}),
  }));
}

// The table the foreign key points at (the "one" side) and the table that
// holds it.
export function referenceEnds(j: ModelJoin): { parent: End; child: End } {
  return j.relationship === 'one_to_many' ? { parent: j.from, child: j.to } : { parent: j.to, child: j.from };
}

export type ReferencePatch = Partial<Pick<ModelJoin,
  'name' | 'comment' | 'cardinality' | 'mandatory' | 'onUpdate' | 'onDelete' | 'fromRole' | 'toRole' | 'pairs'>>;

// The reference's physical fields, with the cube join kept in step: the
// child's cardinality sets the relationship, mandatory sets INNER.
export function setReference(def: ModelDefinition, id: string, patch: ReferencePatch): ModelDefinition {
  return {
    ...def,
    joins: def.joins.map((j) => {
      if (j.id !== id) {
        return j;
      }
      let next: ModelJoin = tidy({ ...j, ...patch });
      if (patch.cardinality) {
        const childIsFrom = referenceEnds(j).child === j.from;
        if (patch.cardinality.endsWith('1')) {
          // A one_to_one reads `from` as the child: a child on the `to`
          // side moves over, so the same table stays the parent.
          if (!childIsFrom) {
            next = mirrored(next);
          }
          next.relationship = 'one_to_one';
        } else {
          next.relationship = childIsFrom ? 'many_to_one' : 'one_to_many';
        }
      }
      if (patch.mandatory !== undefined) {
        next.join = patch.mandatory ? 'inner' : 'left';
      }
      return next;
    }),
  };
}

// The reference read from the other end: `from` and `to`, their roles and
// their pairs swapped.
function mirrored(j: ModelJoin): ModelJoin {
  return tidy({
    ...j,
    from: j.to,
    to: j.from,
    fromRole: j.toRole,
    toRole: j.fromRole,
    pairs: j.pairs?.map((p) => ({ from: p.to, to: p.from })),
  });
}

// Redgate's "Swap reference": the parent becomes the child.
export function swapReference(def: ModelDefinition, id: string): ModelDefinition {
  return {
    ...def,
    joins: def.joins.map((j) => {
      if (j.id !== id) {
        return j;
      }
      if (j.relationship === 'many_to_one') {
        return { ...j, relationship: 'one_to_many' };
      }
      if (j.relationship === 'one_to_many') {
        return { ...j, relationship: 'many_to_one' };
      }
      return mirrored(j);
    }),
  };
}

const nextId = (def: ModelDefinition) => firstFree('r', (n) => def.joins.some((j) => j.id === n));

// Every reference with an id, the first free `r<n>`.
export function ensureIds(def: ModelDefinition): ModelDefinition {
  if (def.joins.every((j) => j.id)) {
    return def;
  }
  const taken = new Set(def.joins.map((j) => j.id).filter(Boolean));
  return {
    ...def,
    joins: def.joins.map((j) => {
      if (j.id) {
        return j;
      }
      const id = firstFree('r', (n) => taken.has(n));
      taken.add(id);
      return { ...j, id };
    }),
  };
}

// The column a reference to `t` points at: its key.
function keyOf(def: ModelDefinition, t: ModelTable, columns: Columns): string | undefined {
  const draftKey = t.draft?.columns.find((c) => c.primaryKey)?.name;
  if (draftKey) {
    return draftKey;
  }
  const cubeKey = cubeKeyOf(def, t.alias);
  if (cubeKey) {
    return cubeKey;
  }
  return columns.find((c) => c.name === 'id')?.name ?? columns[0]?.name;
}

// A key's type as a column of the dialect can hold it: the database's own
// names (information_schema) written as the dialect does, and a serial
// becoming the integer it is.
const RAW_TYPES: Record<string, string> = {
  'character varying': 'varchar(255)',
  varchar: 'varchar(255)',
  character: 'char(1)',
  'timestamp without time zone': 'timestamp',
  'timestamp with time zone': 'timestamptz',
  'time without time zone': 'time',
  serial: 'integer',
  bigserial: 'bigint',
  int4: 'integer',
  int8: 'bigint',
};

export function keyType(raw: string, dialect: Dialect): string {
  const t = raw.trim().toLowerCase();
  if (dialect === 'mysql') {
    if (t === 'integer') {
      return 'int';
    }
    if (t === 'timestamp without time zone') {
      return 'datetime';
    }
  }
  return RAW_TYPES[t] ?? t;
}

// `customers` → `customer`, `categories` → `category`.
function singular(name: string): string {
  if (name.endsWith('ies')) {
    return `${name.slice(0, -3)}y`;
  }
  return name.endsWith('s') ? name.slice(0, -1) : name;
}

// Redgate's new reference, drawn from the parent to the child: mandatory,
// 0..*; a new child table gets the foreign key column, typed as the
// parent's key, an existing one uses `<parent>_<key>` when it has it.
export function addReference(
  def: ModelDefinition,
  parentAlias: string,
  childAlias: string,
  columnsOf: (t: ModelTable) => Columns,
  dialect: Dialect = null
): { def: ModelDefinition; id: string } {
  const parent = def.tables.find((t) => t.alias === parentAlias);
  const child = def.tables.find((t) => t.alias === childAlias);
  if (!parent || !child) {
    return { def, id: '' };
  }
  const parentColumns = columnsOf(parent);
  const key = keyOf(def, parent, parentColumns);
  if (!key) {
    return { def, id: '' };
  }
  const keyColumn = parent.draft?.columns.find((c) => c.name === key);
  const type = keyType(parentColumns.find((c) => c.name === key)?.type ?? 'integer', dialect);
  let next = def;
  let column: string;
  const childColumns = columnsOf(child);
  const childNames = childColumns.map((c) => c.name);
  if (child.draft) {
    const base = `${parentAlias}_${key}`;
    column = childNames.includes(base)
      ? firstFree(`${base}_`, (n) => childNames.includes(n))
      : base;
    next = withDraft(def, childAlias, (d) => ({
      ...d,
      columns: [...d.columns, tidy({
        name: column, type, nullable: false, primaryKey: false, unsigned: keyColumn?.unsigned,
      })],
    }));
  } else {
    const own = keyOf(def, child, childColumns);
    column = [`${parentAlias}_${key}`, `${singular(parentAlias)}_${key}`].find((c) => childNames.includes(c))
      ?? childNames.find((c) => c !== own)
      ?? childNames[0]
      ?? key;
  }
  const id = nextId(next);
  return {
    id,
    def: {
      ...next,
      joins: [...next.joins, {
        id,
        from: { alias: childAlias, column },
        to: { alias: parentAlias, column: key },
        relationship: 'many_to_one',
        join: 'inner',
        cardinality: '0..*',
        mandatory: true,
      }],
    },
  };
}

type SettingGroup = 'validation' | 'format' | 'sqlGeneration' | 'scripts' | 'print';

// One of the model's settings; a value equal to its default (or empty)
// leaves the key out, and empty groups go with it.
export function setSetting(
  def: ModelDefinition,
  group: SettingGroup,
  key: string,
  value: string | boolean | number,
  fallback: string | boolean | number
): ModelDefinition {
  const settings = { ...(def.settings ?? {}) } as Record<string, Record<string, unknown> | undefined>;
  const values = { ...(settings[group] ?? {}) };
  if (value === fallback || value === '') {
    delete values[key];
  } else {
    values[key] = value;
  }
  if (Object.keys(values).length) {
    settings[group] = values;
  } else {
    delete settings[group];
  }
  const { settings: _drop, ...rest } = def;
  return Object.keys(settings).length ? { ...rest, settings: settings as ModelDefinition['settings'] } : rest;
}

// Every validation rule on (Redgate's "Select all") or off.
export function setAllRules(def: ModelDefinition, ids: string[], on: boolean): ModelDefinition {
  const { validation: _drop, ...others } = def.settings ?? {};
  const settings = on ? others : { ...others, validation: Object.fromEntries(ids.map((id) => [id, false])) };
  const { settings: _old, ...rest } = def;
  return Object.keys(settings).length ? { ...rest, settings } : rest;
}
