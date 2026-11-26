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
});
