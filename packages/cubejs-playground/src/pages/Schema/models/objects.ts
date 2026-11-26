import { Box } from './arrange';
import { removeShortcut } from './clipboard';
import {
  Area, IDENTIFIER, ModelDefinition, ModelView, Note, Place, placeTables, removeTable,
} from './modelGraph';

// Everything on the canvas: tables (and their shortcuts) plus the objects
// of Redgate's model that are not tables.
export type ObjectKind = 'table' | 'note' | 'area' | 'view';
type Loose = Exclude<ObjectKind, 'table'>;

const LOOSE: Loose[] = ['note', 'area', 'view'];
const MAX_IDENTIFIER = 63;
const PREFIX: Record<Loose, string> = { note: 'n', area: 'a', view: 'v' };

export function nodeIdOf(kind: Loose, id: string): string {
  return `${kind}:${id}`;
}

export function objectOf(nodeId: string): { kind: ObjectKind; id: string; shortcut?: string } {
  for (const kind of LOOSE) {
    if (nodeId.startsWith(`${kind}:`)) {
      return { kind, id: nodeId.slice(kind.length + 1) };
    }
  }
  const at = nodeId.indexOf('~');
  return at < 0
    ? { kind: 'table', id: nodeId }
    : { kind: 'table', id: nodeId.slice(0, at), shortcut: nodeId.slice(at + 1) };
}

// `<prefix><k>` with the smallest free k >= 1.
function freeId(prefix: string, ids: string[]): string {
  const taken = new Set(ids);
  let n = 1;
  while (taken.has(`${prefix}${n}`)) {
    n += 1;
  }
  return `${prefix}${n}`;
}

function freeName(base: string, names: string[]): string {
  const taken = new Set(names);
  let n = 1;
  while (taken.has(`${base}${n}`)) {
    n += 1;
  }
  return `${base}${n}`;
}

const round = (n: number) => Math.round(n);

export function addNote(def: ModelDefinition, x: number, y: number): { def: ModelDefinition; id: string } {
  const notes = def.notes ?? [];
  const id = freeId(PREFIX.note, notes.map((n) => n.id));
  return {
    id,
    def: {
      ...def,
      notes: [...notes, {
        id, x: round(x), y: round(y), width: 200, height: 100, text: '',
      }],
    },
  };
}

export function addArea(def: ModelDefinition, x: number, y: number): { def: ModelDefinition; id: string } {
  const areas = def.areas ?? [];
  const id = freeId(PREFIX.area, areas.map((a) => a.id));
  const name = freeName('Area_', areas.map((a) => a.name));
  return {
    id,
    def: {
      ...def,
      areas: [...areas, {
        id, name, x: round(x), y: round(y), width: 400, height: 300,
      }],
    },
  };
}

export function addView(
  def: ModelDefinition,
  x: number,
  y: number,
  schema: string
): { def: ModelDefinition; id: string } {
  const views = def.views ?? [];
  const id = freeId(PREFIX.view, views.map((v) => v.id));
  const name = freeName('view_', views.map((v) => v.name));
  return {
    id,
    def: {
      ...def,
      views: [...views, {
        id, name, schema, sql: '', columns: [], dependsOnTables: [], dependsOnViews: [], x: round(x), y: round(y),
      }],
    },
  };
}

// Merges a patch; an `undefined` value removes its key.
function merged<T extends object>(target: T, patch: Record<string, unknown>): T {
  const out = { ...target } as Record<string, unknown>;
  for (const [key, value] of Object.entries(patch)) {
    if (value === undefined) {
      delete out[key];
    } else {
      out[key] = value;
    }
  }
  return out as T;
}

export function setNote(def: ModelDefinition, id: string, patch: Partial<Omit<Note, 'id'>>): ModelDefinition {
  return { ...def, notes: (def.notes ?? []).map((n) => (n.id === id ? merged(n, patch) : n)) };
}

export function setArea(def: ModelDefinition, id: string, patch: Partial<Omit<Area, 'id'>>): ModelDefinition {
  return { ...def, areas: (def.areas ?? []).map((a) => (a.id === id ? merged(a, patch) : a)) };
}

export function setView(def: ModelDefinition, id: string, patch: Partial<Omit<ModelView, 'id'>>): ModelDefinition {
  return { ...def, views: (def.views ?? []).map((v) => (v.id === id ? merged(v, patch) : v)) };
}

export function renameView(def: ModelDefinition, id: string, next: string): { def: ModelDefinition; error?: string } {
  const views = def.views ?? [];
  if (views.find((v) => v.id === id)?.name === next) {
    return { def };
  }
  if (!IDENTIFIER.test(next) || next.length > MAX_IDENTIFIER) {
    return { def, error: `Invalid name: ${next}` };
  }
  if (views.some((v) => v.name === next)) {
    return { def, error: `${next} is already in this model` };
  }
  return { def: setView(def, id, { name: next }) };
}

export function removeObjects(def: ModelDefinition, nodeIds: string[]): ModelDefinition {
  let next = def;
  const gone: Record<Loose, Set<string>> = { note: new Set(), area: new Set(), view: new Set() };
  for (const nodeId of nodeIds) {
    const { kind, id, shortcut } = objectOf(nodeId);
    if (kind === 'table') {
      next = shortcut ? removeShortcut(next, id, shortcut) : removeTable(next, id);
    } else {
      gone[kind].add(id);
    }
  }
  if (next.notes && gone.note.size) {
    next = { ...next, notes: next.notes.filter((n) => !gone.note.has(n.id)) };
  }
  if (next.areas && gone.area.size) {
    next = { ...next, areas: next.areas.filter((a) => !gone.area.has(a.id)) };
  }
  if (next.views && gone.view.size) {
    next = {
      ...next,
      views: next.views
        .filter((v) => !gone.view.has(v.id))
        .map((v) => ({ ...v, dependsOnViews: v.dependsOnViews.filter((d) => !gone.view.has(d)) })),
    };
  }
  return next;
}

export function placeObjects(def: ModelDefinition, places: Map<string, Place>): ModelDefinition {
  const own = (kind: Loose, id: string) => places.get(nodeIdOf(kind, id));
  const apply = <T extends { id: string; x: number; y: number }>(
    kind: Loose,
    o: T,
    sized: boolean
  ): T => {
    const p = own(kind, o.id);
    if (!p) {
      return o;
    }
    const patch: Record<string, unknown> = {};
    if (p.x !== undefined) patch.x = round(p.x);
    if (p.y !== undefined) patch.y = round(p.y);
    for (const [key, value] of [['width', p.width], ['height', p.height]] as const) {
      if (value === null) {
        patch[key] = sized ? undefined : (o as Record<string, unknown>)[key];
      } else if (value !== undefined) {
        patch[key] = round(value);
      }
    }
    return merged(o, patch);
  };
  const tables = placeTables(def, places);
  return {
    ...tables,
    ...(def.notes ? { notes: def.notes.map((n) => apply('note', n, true)) } : {}),
    ...(def.areas ? { areas: def.areas.map((a) => apply('area', a, false)) } : {}),
    ...(def.views ? { views: def.views.map((v) => apply('view', v, true)) } : {}),
  };
}

// Moves the picked notes or areas past every other of their kind, in front
// (`front`) or behind, keeping their order among themselves.
function reorder(def: ModelDefinition, nodeIds: string[], front: boolean): ModelDefinition {
  const picks = nodeIds.map(objectOf);
  const restack = <T extends { id: string; z?: number }>(kind: Loose, list: T[]): T[] => {
    const picked = new Set(picks.filter((o) => o.kind === kind).map((o) => o.id));
    const others = list.filter((o) => !picked.has(o.id)).map((o) => o.z ?? 0);
    const moving = list.filter((o) => picked.has(o.id)).sort((a, b) => (a.z ?? 0) - (b.z ?? 0));
    const base = front ? Math.max(0, ...others) + 1 : Math.min(0, ...others) - moving.length;
    const zs = new Map(moving.map((o, i) => [o.id, base + i]));
    return list.map((o) => (zs.has(o.id) ? { ...o, z: zs.get(o.id) } : o));
  };
  return {
    ...def,
    ...(def.notes ? { notes: restack('note', def.notes) } : {}),
    ...(def.areas ? { areas: restack('area', def.areas) } : {}),
  };
}

export const bringToFront = (def: ModelDefinition, nodeIds: string[]) => reorder(def, nodeIds, true);
export const sendToBack = (def: ModelDefinition, nodeIds: string[]) => reorder(def, nodeIds, false);

// The boxes entirely within the area, edges included; not the area itself.
export function insideArea(area: Box, boxes: Box[]): string[] {
  return boxes
    .filter((b) => b.id !== area.id
      && b.x >= area.x && b.y >= area.y
      && b.x + b.width <= area.x + area.width
      && b.y + b.height <= area.y + area.height)
    .map((b) => b.id);
}

export function nodeIdsOf(def: ModelDefinition): string[] {
  return [
    ...def.tables.flatMap((t) => [t.alias, ...(t.shortcuts ?? []).map((s) => `${t.alias}~${s.id}`)]),
    ...(def.notes ?? []).map((n) => nodeIdOf('note', n.id)),
    ...(def.areas ?? []).map((a) => nodeIdOf('area', a.id)),
    ...(def.views ?? []).map((v) => nodeIdOf('view', v.id)),
  ];
}
