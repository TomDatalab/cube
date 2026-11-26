import { nodeIdOf } from './objects';
import type { ModelDefinition, ModelTable } from './modelGraph';

// A thing of the model the search found, and where it is on the canvas.
export type Hit = {
  kind: 'table' | 'column' | 'reference' | 'note' | 'area' | 'view';
  label: string;
  nodeId: string;
  edgeIndex?: number;
};

// Tables by alias or schema.table, columns by name, references by their
// two ends; case-insensitive.
export function searchModel(
  def: ModelDefinition,
  columnsOf: (t: ModelTable) => { name: string; type: string }[],
  query: string,
  limit = 50
): Hit[] {
  const needle = query.trim().toLowerCase();
  if (!needle) {
    return [];
  }
  const has = (text: string) => text.toLowerCase().includes(needle);
  const hits: Hit[] = [];
  for (const t of def.tables) {
    if (has(t.alias) || has(`${t.schema}.${t.table}`)) {
      hits.push({ kind: 'table', label: t.alias, nodeId: t.alias });
    }
  }
  for (const t of def.tables) {
    for (const c of columnsOf(t)) {
      if (has(c.name)) {
        hits.push({ kind: 'column', label: `${t.alias}.${c.name}`, nodeId: t.alias });
      }
    }
  }
  def.joins.forEach((j, edgeIndex) => {
    const label = `${j.from.alias}.${j.from.column} → ${j.to.alias}.${j.to.column}`;
    if (has(label)) {
      hits.push({
        kind: 'reference', label, nodeId: j.from.alias, edgeIndex,
      });
    }
  });
  for (const n of def.notes ?? []) {
    if (has(n.text)) {
      hits.push({ kind: 'note', label: n.text.slice(0, 40).replace(/\r?\n/g, ' '), nodeId: nodeIdOf('note', n.id) });
    }
  }
  for (const a of def.areas ?? []) {
    if (has(a.name)) {
      hits.push({ kind: 'area', label: a.name, nodeId: nodeIdOf('area', a.id) });
    }
  }
  for (const v of def.views ?? []) {
    if (has(v.name)) {
      hits.push({ kind: 'view', label: v.name, nodeId: nodeIdOf('view', v.id) });
    }
    for (const c of v.columns) {
      if (has(c.name)) {
        hits.push({ kind: 'view', label: `${v.name}.${c.name}`, nodeId: nodeIdOf('view', v.id) });
      }
    }
  }
  return hits.slice(0, limit);
}
