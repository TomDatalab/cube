import { describe, expect, it } from 'vitest';

import { searchModel } from '../pages/Schema/models/search';
import {
  ModelDefinition, ModelTable, addTable, connect, emptyDefinition,
} from '../pages/Schema/models/modelGraph';

const COLUMNS: Record<string, { name: string; type: string }[]> = {
  orders: [{ name: 'id', type: 'int' }, { name: 'customer_id', type: 'int' }],
  customers: [{ name: 'id', type: 'int' }, { name: 'Name', type: 'text' }],
};
const columnsOf = (t: ModelTable) => COLUMNS[t.table] ?? [];

function model(): ModelDefinition {
  let def = emptyDefinition();
  def = addTable(def, 'ds', 'sales', 'orders', 0, 0).def;
  def = addTable(def, 'ds', 'sales', 'customers', 300, 0).def;
  return connect(def, { alias: 'orders', column: 'customer_id' }, { alias: 'customers', column: 'id' }, 'many_to_one', 'left');
}

describe('searchModel', () => {
  it('finds tables, columns and references, case-insensitive', () => {
    const hits = searchModel(model(), columnsOf, 'CUSTOMER');
    expect(hits).toEqual([
      { kind: 'table', label: 'customers', nodeId: 'customers' },
      { kind: 'column', label: 'orders.customer_id', nodeId: 'orders' },
      { kind: 'reference', label: 'orders.customer_id → customers.id', nodeId: 'orders', edgeIndex: 0 },
    ]);
    expect(searchModel(model(), columnsOf, 'sales.ord').map((h) => h.label)).toEqual(['orders']);
    expect(searchModel(model(), columnsOf, 'name')).toEqual([
      { kind: 'column', label: 'customers.Name', nodeId: 'customers' },
    ]);
  });

  it('empty query finds nothing', () => {
    expect(searchModel(model(), columnsOf, '  ')).toEqual([]);
  });

  it('caps the hits at the limit', () => {
    expect(searchModel(model(), columnsOf, 'i', 2)).toHaveLength(2);
  });

  it('search finds notes, areas, views and view columns', () => {
    const def: ModelDefinition = {
      ...model(),
      notes: [{ id: 'n1', x: 0, y: 0, text: `zeta line one\nzeta line two and a long tail beyond forty` }],
      areas: [{ id: 'a1', name: 'Zeta area', x: 0, y: 0, width: 1, height: 1 }],
      views: [{
        id: 'v1', name: 'zeta_view', schema: 's', sql: '', columns: [{ name: 'zeta_col', type: 'int' }], dependsOnTables: [], dependsOnViews: [], x: 0, y: 0,
      }],
    };
    expect(searchModel(def, columnsOf, 'zeta')).toEqual([
      { kind: 'note', label: 'zeta line one zeta line two and a long t', nodeId: 'note:n1' },
      { kind: 'area', label: 'Zeta area', nodeId: 'area:a1' },
      { kind: 'view', label: 'zeta_view', nodeId: 'view:v1' },
      { kind: 'view', label: 'zeta_view.zeta_col', nodeId: 'view:v1' },
    ]);
  });
});

describe('searchModel cubes', () => {
  const withCube = () => ({
    ...model(),
    cubes: [{
      id: 'c1',
      name: 'sales_cube',
      title: 'Sales overview',
      source: { kind: 'table' as const, table: 'orders' },
      dimensions: [{ name: 'region', type: 'string' as const, column: 'id' }],
      measures: [{ name: 'revenue', type: 'sum' as const, column: 'id', title: 'Total revenue' }],
      segments: [{ name: 'big', sql: '1 = 1' }],
      x: 0,
      y: 0,
    }],
  });

  it('finds a cube by name or title and selects it', () => {
    expect(searchModel(withCube(), columnsOf, 'sales_c')).toEqual([{ kind: 'cube', label: 'sales_cube', nodeId: 'cube:c1' }]);
    expect(searchModel(withCube(), columnsOf, 'overview')).toEqual([{ kind: 'cube', label: 'sales_cube', nodeId: 'cube:c1' }]);
  });

  it('finds members by name or title, labelled with their cube', () => {
    expect(searchModel(withCube(), columnsOf, 'region')).toEqual([{ kind: 'cube', label: 'sales_cube.region', nodeId: 'cube:c1' }]);
    expect(searchModel(withCube(), columnsOf, 'big').map((h) => h.label)).toEqual(['sales_cube.big']);
    expect(searchModel(withCube(), columnsOf, 'total rev').map((h) => h.label)).toEqual(['sales_cube.revenue']);
  });
});
