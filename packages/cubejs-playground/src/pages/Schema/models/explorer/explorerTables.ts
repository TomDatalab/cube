import type { ModelDefinition, ModelTable } from '../modelGraph';
import { nodeIdOf } from '../objects';

export type Cell = string | number | null;
export type ExplorerTable = { name: string; columns: string[]; rows: Cell[][] };

const text = (v: string | undefined): Cell => (v === undefined || v === '' ? null : v);
const bool = (v: boolean | undefined): Cell => (v === undefined ? null : Number(v));
const list = (names: string[]) => names.join(', ');

// The model as the tables of the Model explorer. Every table has a `node`
// column, the canvas node "Find in diagram" selects: a table's alias for the
// tables and what they own, `kind:id` for the other objects.
export function explorerTables(
  def: ModelDefinition,
  columnsOf: (t: ModelTable) => { name: string; type: string }[]
): ExplorerTable[] {
  const tables: Cell[][] = [];
  const columns: Cell[][] = [];
  const primaryKeys: Cell[][] = [];
  const uniqueKeys: Cell[][] = [];
  const indexes: Cell[][] = [];
  const checks: Cell[][] = [];
  for (const t of def.tables) {
    tables.push([t.alias, t.alias, t.schema, t.table, text(t.comment), t.draft ? 1 : 0]);
    const { draft } = t;
    if (!draft) {
      // A table of the database: only what the connection tells.
      for (const c of columnsOf(t)) {
        columns.push([t.alias, t.alias, c.name, c.type, null, null, null, null]);
      }
    } else {
      for (const c of draft.columns) {
        columns.push([
          t.alias, t.alias, c.name, c.type, Number(c.nullable), Number(c.primaryKey), text(c.comment), text(c.default),
        ]);
      }
      const pk = draft.columns.filter((c) => c.primaryKey).map((c) => c.name);
      if (pk.length) {
        primaryKeys.push([t.alias, t.alias, text(draft.primaryKeyName), list(pk)]);
      }
      for (const u of draft.uniqueKeys ?? []) {
        uniqueKeys.push([t.alias, t.alias, u.name, list(u.columns)]);
      }
      for (const i of draft.indexes ?? []) {
        indexes.push([t.alias, t.alias, i.name, list(i.columns.map((c) => c.name)), Number(i.unique)]);
      }
      for (const k of draft.checks ?? []) {
        checks.push([t.alias, t.alias, k.name, k.expression]);
      }
    }
  }

  const foreignKeys: Cell[][] = def.joins.map((j) => [
    j.from.alias,
    text(j.name),
    j.from.alias,
    list([j.from.column, ...(j.pairs ?? []).map((p) => p.from)]),
    j.to.alias,
    list([j.to.column, ...(j.pairs ?? []).map((p) => p.to)]),
    j.cardinality ?? null,
    bool(j.mandatory),
  ]);

  const views = def.views ?? [];
  const cubes = def.cubes ?? [];
  return [
    { name: 'tables', columns: ['node', 'alias', 'schema', 'name', 'comment', 'is_draft'], rows: tables },
    {
      name: 'columns',
      columns: ['node', 'table_alias', 'name', 'type', 'nullable', 'primary_key', 'comment', 'default_value'],
      rows: columns,
    },
    { name: 'primary_keys', columns: ['node', 'table_alias', 'name', 'columns'], rows: primaryKeys },
    { name: 'unique_keys', columns: ['node', 'table_alias', 'name', 'columns'], rows: uniqueKeys },
    { name: 'indexes', columns: ['node', 'table_alias', 'name', 'columns', 'is_unique'], rows: indexes },
    { name: 'checks', columns: ['node', 'table_alias', 'name', 'expression'], rows: checks },
    {
      name: 'foreign_keys',
      columns: ['node', 'name', 'from_table', 'from_column', 'to_table', 'to_column', 'cardinality', 'mandatory'],
      rows: foreignKeys,
    },
    {
      name: 'views',
      columns: ['node', 'name', 'schema', 'comment', 'sql'],
      rows: views.map((v) => [nodeIdOf('view', v.id), v.name, v.schema, text(v.comment), text(v.sql)]),
    },
    {
      name: 'view_columns',
      columns: ['node', 'view_name', 'name', 'type'],
      rows: views.flatMap((v) => v.columns.map((c) => [nodeIdOf('view', v.id), v.name, c.name, text(c.type)])),
    },
    {
      name: 'areas',
      columns: ['node', 'name', 'comment'],
      rows: (def.areas ?? []).map((a) => [nodeIdOf('area', a.id), a.name, text(a.comment)]),
    },
    {
      name: 'notes',
      columns: ['node', 'text'],
      rows: (def.notes ?? []).map((n) => [nodeIdOf('note', n.id), n.text]),
    },
    {
      name: 'cubes',
      columns: ['node', 'name', 'title', 'source_kind', 'source_table', 'sql'],
      rows: cubes.map((c) => [
        nodeIdOf('cube', c.id), c.name, text(c.title), c.source.kind, text(c.source.table), text(c.source.sql),
      ]),
    },
    {
      name: 'dimensions',
      columns: ['node', 'cube_name', 'name', 'type', 'column_name', 'sql', 'primary_key'],
      rows: cubes.flatMap((c) => c.dimensions.map((d) => [
        nodeIdOf('cube', c.id), c.name, d.name, d.type, text(d.column), text(d.sql), d.primaryKey ? 1 : 0,
      ])),
    },
    {
      name: 'measures',
      columns: ['node', 'cube_name', 'name', 'type', 'column_name', 'sql', 'format'],
      rows: cubes.flatMap((c) => c.measures.map((m) => [
        nodeIdOf('cube', c.id), c.name, m.name, m.type, text(m.column), text(m.sql), m.format ?? null,
      ])),
    },
  ];
}
