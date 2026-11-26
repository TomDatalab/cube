import {
  Area, End, ModelDefinition, ModelJoin, ModelTable, ModelView, Note, removeShortcut,
} from './modelGraph';

import {
  nodeIdOf, objectOf, removeObjects, viewNameTaken,
} from './objects';

export { removeShortcut };

// What Copy or Cut keeps: the objects and the links between the tables.
export type Clip = {
  tables: ModelTable[];
  joins: ModelJoin[];
  notes: Note[];
  areas: Area[];
  views: ModelView[];
};

const SEPARATOR = '~';

// A shortcut's node on the canvas: its table's alias and its own id.
export function shortcutNodeId(alias: string, id: string): string {
  return `${alias}${SEPARATOR}${id}`;
}

export function parseNodeId(nodeId: string): { alias: string; shortcut?: string } {
  const at = nodeId.indexOf(SEPARATOR);
  return at < 0 ? { alias: nodeId } : { alias: nodeId.slice(0, at), shortcut: nodeId.slice(at + 1) };
}

export function copy(def: ModelDefinition, nodeIds: string[]): Clip {
  const picked = nodeIds.map(objectOf);
  const ids = (kind: string) => new Set(picked.filter((o) => o.kind === kind).map((o) => o.id));
  const aliases = ids('table');
  const notes = ids('note');
  const areas = ids('area');
  const views = ids('view');
  return {
    tables: def.tables.filter((t) => aliases.has(t.alias)),
    joins: def.joins.filter((j) => aliases.has(j.from.alias) && aliases.has(j.to.alias)),
    notes: (def.notes ?? []).filter((n) => notes.has(n.id)),
    areas: (def.areas ?? []).filter((a) => areas.has(a.id)),
    views: (def.views ?? []).filter((v) => views.has(v.id)),
  };
}

// The name itself when it is free, else the first free `<name>_<n>`.
function free(base: string, taken: (name: string) => boolean): string {
  if (!taken(base)) {
    return base;
  }
  let n = 1;
  while (taken(`${base}_${n}`)) {
    n += 1;
  }
  return `${base}_${n}`;
}

export function paste(
  def: ModelDefinition,
  clip: Clip,
  offset = 20
): { def: ModelDefinition; nodeIds: string[] } {
  const tables = [...def.tables];
  const renamed = new Map<string, string>();
  for (const t of clip.tables) {
    const alias = free(t.alias, (a) => tables.some((x) => x.alias === a));
    renamed.set(t.alias, alias);
    const { shortcuts: _drop, ...rest } = t;
    let { table } = t;
    if (t.draft) {
      table = free(t.table, (name) => tables.some((x) => x.schema === t.schema && x.table === name));
    }
    tables.push({
      ...rest, alias, table, x: t.x + offset, y: t.y + offset,
    });
  }
  const end = (e: End): End => ({ ...e, alias: renamed.get(e.alias) as string });
  // A pasted reference is a new one: its own id, and no name to repeat.
  const taken = new Set(def.joins.map((j) => j.id).filter(Boolean));
  const joins = clip.joins.map((j) => {
    const { name: _name, id: _id, ...rest } = j;
    let n = 1;
    while (taken.has(`r${n}`)) {
      n += 1;
    }
    taken.add(`r${n}`);
    return {
      ...rest, id: `r${n}`, from: end(j.from), to: end(j.to),
    };
  });
  const moved = <T extends { x: number; y: number }>(o: T): T => ({
    ...o, x: o.x + offset, y: o.y + offset,
  });
  const smallestFree = (prefix: string, used: Set<string>) => {
    let n = 1;
    while (used.has(`${prefix}${n}`)) {
      n += 1;
    }
    used.add(`${prefix}${n}`);
    return `${prefix}${n}`;
  };
  const fresh = <T extends { id: string }>(prefix: string, own: T[] | undefined, pasted: T[]) => {
    const used = new Set((own ?? []).map((o) => o.id));
    return pasted.map((o) => ({ ...o, id: smallestFree(prefix, used) }));
  };
  const notes = fresh('n', def.notes, clip.notes).map(moved);
  const areas = fresh('a', def.areas, clip.areas).map(moved);
  const views: ModelView[] = [...(def.views ?? [])];
  const viewIds = new Map<string, string>();
  const usedViews = new Set(views.map((v) => v.id));
  const pastedViews = clip.views.map((v) => {
    const id = smallestFree('v', usedViews);
    viewIds.set(v.id, id);
    return { v, id };
  });
  for (const { v, id } of pastedViews) {
    const name = free(v.name, (n) => viewNameTaken({ ...def, tables, views }, v.schema, n));
    views.push(moved({
      ...v,
      id,
      name,
      dependsOnTables: v.dependsOnTables.map((a) => renamed.get(a) ?? a),
      dependsOnViews: v.dependsOnViews.filter((d) => viewIds.has(d)).map((d) => viewIds.get(d) as string),
    }));
  }
  return {
    def: {
      ...def,
      tables,
      joins: [...def.joins, ...joins],
      ...(def.notes || notes.length ? { notes: [...(def.notes ?? []), ...notes] } : {}),
      ...(def.areas || areas.length ? { areas: [...(def.areas ?? []), ...areas] } : {}),
      ...(def.views || views.length ? { views } : {}),
    },
    nodeIds: [
      ...renamed.values(),
      ...notes.map((n) => nodeIdOf('note', n.id)),
      ...areas.map((a) => nodeIdOf('area', a.id)),
      ...pastedViews.map((p) => nodeIdOf('view', p.id)),
    ],
  };
}

export function cut(def: ModelDefinition, nodeIds: string[]): { def: ModelDefinition; clip: Clip } {
  return { def: removeObjects(def, nodeIds), clip: copy(def, nodeIds) };
}

export function addShortcut(
  def: ModelDefinition,
  alias: string,
  x: number,
  y: number
): { def: ModelDefinition; id: string } {
  const taken = new Set(def.tables.flatMap((t) => t.shortcuts ?? []).map((s) => s.id));
  let n = 1;
  while (taken.has(`s${n}`)) {
    n += 1;
  }
  const id = `s${n}`;
  return {
    id,
    def: {
      ...def,
      tables: def.tables.map((t) => (t.alias === alias
        ? { ...t, shortcuts: [...(t.shortcuts ?? []), { id, x: Math.round(x), y: Math.round(y) }] }
        : t)),
    },
  };
}
