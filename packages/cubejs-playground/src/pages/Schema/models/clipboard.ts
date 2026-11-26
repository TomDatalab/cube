import {
  End, ModelDefinition, ModelJoin, ModelTable, removeTable,
} from './modelGraph';

// What Copy or Cut keeps: the tables and the links between them.
export type Clip = { tables: ModelTable[]; joins: ModelJoin[] };

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
  const aliases = new Set(nodeIds.map((id) => parseNodeId(id).alias));
  return {
    tables: def.tables.filter((t) => aliases.has(t.alias)),
    joins: def.joins.filter((j) => aliases.has(j.from.alias) && aliases.has(j.to.alias)),
  };
}

function free(base: string, taken: (name: string) => boolean): string {
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
): { def: ModelDefinition; aliases: string[] } {
  const tables = [...def.tables];
  const renamed = new Map<string, string>();
  for (const t of clip.tables) {
    const alias = free(t.alias, (a) => tables.some((x) => x.alias === a));
    renamed.set(t.alias, alias);
    const { shortcuts: _drop, ...rest } = t;
    let { table } = t;
    if (t.draft) {
      const clash = (name: string) => tables.some((x) => x.schema === t.schema && x.table === name);
      if (clash(table)) {
        table = free(t.table, clash);
      }
    }
    tables.push({
      ...rest, alias, table, x: t.x + offset, y: t.y + offset,
    });
  }
  const end = (e: End): End => ({ ...e, alias: renamed.get(e.alias) as string });
  const joins = clip.joins.map((j) => ({ ...j, from: end(j.from), to: end(j.to) }));
  return {
    def: { ...def, tables, joins: [...def.joins, ...joins] },
    aliases: [...renamed.values()],
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

export function cut(def: ModelDefinition, nodeIds: string[]): { def: ModelDefinition; clip: Clip } {
  const clip = copy(def, nodeIds);
  let next = def;
  for (const nodeId of nodeIds) {
    const { alias, shortcut } = parseNodeId(nodeId);
    next = shortcut ? removeShortcut(next, alias, shortcut) : removeTable(next, alias);
  }
  return { def: next, clip };
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
