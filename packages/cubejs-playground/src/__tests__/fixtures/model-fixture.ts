import type { ModelDefinition, ModelTable } from '../../pages/Schema/models/modelGraph';

// One of every object kind a model holds: a draft table with its keys, an
// index and a check; a table that exists in the database; a reference; a
// view; an area; a note; and a cube with a dimension and a measure.

// The database of the fixture's connection.
export const fixtureTables = {
  warehouse: {
    public: {
      customers: [
        { name: 'id', type: 'integer' },
        { name: 'name', type: 'text' },
      ],
    },
  },
};

export const modelFixture = (): ModelDefinition => ({
  dataSource: 'warehouse',
  tables: [
    {
      alias: 'orders',
      schema: 'public',
      table: 'orders',
      comment: 'One row per order',
      x: 0,
      y: 0,
      draft: {
        columns: [
          { name: 'id', type: 'integer', nullable: false, primaryKey: true },
          { name: 'customer_id', type: 'integer', nullable: true, primaryKey: false },
          {
            name: 'email', type: 'text', nullable: true, primaryKey: false, comment: 'Who to write to', default: "''",
          },
          {
            name: 'amount', type: 'numeric', nullable: false, primaryKey: false, default: '0',
          },
        ],
        primaryKeyName: 'orders_pk',
        uniqueKeys: [{ name: 'orders_email_uq', columns: ['email', 'customer_id'] }],
        indexes: [{ name: 'orders_customer_idx', columns: [{ name: 'customer_id' }], unique: false }],
        checks: [{ name: 'orders_amount_ck', expression: 'amount >= 0' }],
      },
    },
    {
      alias: 'customers', schema: 'public', table: 'customers', x: 400, y: 0,
    },
  ],
  joins: [{
    id: 'r1',
    name: 'orders_customer_fk',
    from: { alias: 'orders', column: 'customer_id' },
    to: { alias: 'customers', column: 'id' },
    relationship: 'many_to_one',
    join: 'left',
    cardinality: '0..*',
    mandatory: false,
  }],
  views: [{
    id: 'v1',
    name: 'big_orders',
    schema: 'public',
    comment: 'Orders over 100',
    sql: 'SELECT * FROM public.orders WHERE amount > 100',
    columns: [{ name: 'id', type: 'integer' }, { name: 'amount', type: 'numeric' }],
    dependsOnTables: ['orders'],
    dependsOnViews: [],
    x: 0,
    y: 400,
  }],
  areas: [{
    id: 'a1', name: 'Sales area', comment: 'Where money is', x: -50, y: -50, width: 900, height: 300,
  }],
  notes: [{
    id: 'n1', x: 800, y: 400, text: 'Check the amounts',
  }],
  cubes: [{
    id: 'c1',
    name: 'sales',
    title: 'Sales',
    source: { kind: 'table', table: 'orders', primaryKey: 'id' },
    dimensions: [{
      name: 'id', type: 'number', column: 'id', primaryKey: true,
    }],
    measures: [{
      name: 'total', type: 'sum', column: 'amount', format: 'currency',
    }],
    segments: [],
    x: 800,
    y: 0,
  }],
});

// The columns of a fixture table, as the editor's `columnsOfTable` gives them.
export const fixtureColumnsOf = (t: ModelTable): { name: string; type: string }[] => (t.draft
  ? t.draft.columns.map((c) => ({ name: c.name, type: c.type }))
  : (fixtureTables as Record<string, Record<string, Record<string, { name: string; type: string }[]>>>)
    .warehouse[t.schema]?.[t.table] ?? []);
