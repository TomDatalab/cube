import type { Dialect } from './dialect';
import type {
  DraftColumn, DraftTable, End, ModelDefinition, ModelJoin, ModelTable, TableCheck, TableIndex, UniqueKey,
} from './modelGraph';

type Columns = { name: string; type: string }[];

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
  return {
    ...next,
    primaryKey: def.primary === alias && def.primaryKey === from ? to : def.primaryKey,
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
  return {
    ...next,
    primaryKey: def.primary === alias && def.primaryKey === name ? null : def.primaryKey,
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
      const next: ModelJoin = tidy({ ...j, ...patch });
      if (patch.cardinality) {
        const childIsFrom = referenceEnds(j).child === j.from;
        if (patch.cardinality.endsWith('1')) {
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
      return tidy({
        ...j,
        from: j.to,
        to: j.from,
        fromRole: j.toRole,
        toRole: j.fromRole,
        pairs: j.pairs?.map((p) => ({ from: p.to, to: p.from })),
      });
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
  if (def.primary === t.alias && def.primaryKey) {
    return def.primaryKey;
  }
  return columns.find((c) => c.name === 'id')?.name ?? columns[0]?.name;
}

// Redgate's new reference, drawn from the parent to the child: mandatory,
// 0..*; a new child table gets the foreign key column.
export function addReference(
  def: ModelDefinition,
  parentAlias: string,
  childAlias: string,
  columnsOf: (t: ModelTable) => Columns
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
  const keyType = parentColumns.find((c) => c.name === key)?.type ?? 'integer';
  let next = def;
  let column: string;
  const childNames = columnsOf(child).map((c) => c.name);
  if (child.draft) {
    const base = `${parentAlias}_${key}`;
    column = childNames.includes(base)
      ? firstFree(`${base}_`, (n) => childNames.includes(n))
      : base;
    next = withDraft(def, childAlias, (d) => ({
      ...d,
      columns: [...d.columns, {
        name: column, type: keyType, nullable: false, primaryKey: false,
      }],
    }));
  } else {
    column = [key, `${parentAlias}_${key}`].find((c) => childNames.includes(c)) ?? childNames[0] ?? key;
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

type SettingGroup = 'validation' | 'format' | 'sqlGeneration' | 'scripts';

// One of the model's settings; a value equal to its default (or empty)
// leaves the key out, and empty groups go with it.
export function setSetting(
  def: ModelDefinition,
  group: SettingGroup,
  key: string,
  value: string | boolean,
  fallback: string | boolean
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
