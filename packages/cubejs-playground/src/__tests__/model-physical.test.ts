import { describe, expect, it } from 'vitest';

import {
  addColumn, addReference, ensureIds, moveColumn, referenceEnds, removeColumn, renameColumn, setColumn,
  setIndexes, setReference, setUniqueKeys, swapReference,
} from '../pages/Schema/models/physical';
import type { ModelDefinition, ModelTable } from '../pages/Schema/models/modelGraph';

const COLUMNS: Record<string, { name: string; type: string }[]> = {
  customers: [{ name: 'id', type: 'integer' }, { name: 'name', type: 'text' }],
  orders: [{ name: 'id', type: 'integer' }, { name: 'customer_id', type: 'integer' }],
};
const columnsOf = (t: ModelTable) => (t.draft
  ? t.draft.columns.map((c) => ({ name: c.name, type: c.type }))
  : COLUMNS[t.table] ?? []);

function model(): ModelDefinition {
  return {
    dataSource: 'pg',
    primary: 'customers',
    primaryKey: 'id',
    tables: [
      {
        alias: 'customers', schema: 's', table: 'customers', x: 0, y: 0,
      },
      {
        alias: 'orders', schema: 's', table: 'orders', x: 300, y: 0,
      },
      {
        alias: 'items',
        schema: 's',
        table: 'items',
        x: 0,
        y: 300,
        draft: {
          columns: [
            { name: 'id', type: 'integer', nullable: false, primaryKey: true },
            { name: 'sku', type: 'varchar(32)', nullable: false, primaryKey: false },
            { name: 'price', type: 'numeric(10,2)', nullable: true, primaryKey: false },
          ],
          uniqueKeys: [{ name: 'uk_sku', columns: ['sku'] }],
          indexes: [{ name: 'ix_sku_price', columns: [{ name: 'sku' }, { name: 'price', desc: true }], unique: false }],
        },
      },
    ],
    joins: [{
      id: 'r1',
      from: { alias: 'orders', column: 'customer_id' },
      to: { alias: 'customers', column: 'id' },
      relationship: 'many_to_one',
      join: 'left',
    }, {
      from: { alias: 'items', column: 'sku' },
      to: { alias: 'orders', column: 'id' },
      relationship: 'many_to_one',
      join: 'left',
    }],
  };
}
const items = (def: ModelDefinition) => def.tables.find((t) => t.alias === 'items')!.draft!;

describe('physical', () => {
  it('setColumn, addColumn and moveColumn edit a draft', () => {
    let def = setColumn(model(), 'items', 2, { nullable: false, comment: 'eur', default: '0' });
    expect(items(def).columns[2]).toMatchObject({ nullable: false, comment: 'eur', default: '0' });
    def = addColumn(def, 'items', 'postgres');
    def = addColumn(def, 'items', 'mysql');
    expect(items(def).columns.slice(3).map((c) => [c.name, c.type])).toEqual([
      ['column_1', 'text'], ['column_2', 'varchar(255)'],
    ]);
    def = moveColumn(def, 'items', 0, 2);
    expect(items(def).columns.map((c) => c.name).slice(0, 3)).toEqual(['sku', 'price', 'id']);
  });

  it('renameColumn follows keys, indexes and references', () => {
    const def = renameColumn(model(), 'items', 'sku', 'code');
    expect(items(def).columns[1].name).toBe('code');
    expect(items(def).uniqueKeys![0].columns).toEqual(['code']);
    expect(items(def).indexes![0].columns[0].name).toBe('code');
    expect(def.joins[1].from.column).toBe('code');
  });

  it('removeColumn drops empty keys and the references using it', () => {
    const def = removeColumn(model(), 'items', 'sku');
    expect(items(def).columns.map((c) => c.name)).toEqual(['id', 'price']);
    expect(items(def).uniqueKeys).toBeUndefined();
    expect(items(def).indexes![0].columns).toEqual([{ name: 'price', desc: true }]);
    expect(def.joins).toHaveLength(1);
  });

  it('sets keys and indexes, dropping the empty lists', () => {
    let def = setUniqueKeys(model(), 'items', []);
    expect('uniqueKeys' in items(def)).toBe(false);
    def = setIndexes(def, 'items', [{ name: 'ix', columns: [{ name: 'id' }], unique: true }]);
    expect(items(def).indexes).toEqual([{ name: 'ix', columns: [{ name: 'id' }], unique: true }]);
  });

  it('referenceEnds reads the parent as the one side', () => {
    const j = model().joins[0];
    expect(referenceEnds(j)).toEqual({ parent: j.to, child: j.from });
    expect(referenceEnds({ ...j, relationship: 'one_to_many' })).toEqual({ parent: j.from, child: j.to });
  });

  it('setReference keeps relationship and join in step', () => {
    const at = (def: ModelDefinition) => def.joins.find((j) => j.id === 'r1')!;
    let def = setReference(model(), 'r1', { cardinality: '1..1' });
    expect(at(def)).toMatchObject({ cardinality: '1..1', relationship: 'one_to_one' });
    def = setReference(def, 'r1', { cardinality: '0..*' });
    expect(at(def).relationship).toBe('many_to_one');
    def = setReference(def, 'r1', { mandatory: true });
    expect(at(def)).toMatchObject({ mandatory: true, join: 'inner' });
    def = setReference(def, 'r1', { mandatory: false, onDelete: 'cascade', name: 'fk_x' });
    expect(at(def)).toMatchObject({
      mandatory: false, join: 'left', onDelete: 'cascade', name: 'fk_x',
    });
    // The child on the `to` side: many reads one_to_many.
    const flipped = { ...model(), joins: [{ ...model().joins[0], relationship: 'one_to_many' as const }] };
    expect(setReference(flipped, 'r1', { cardinality: '1..*' }).joins[0].relationship).toBe('one_to_many');
  });

  it('one_to_one keeps the parent: a child on the `to` side moves to `from`', () => {
    const def = swapReference(model(), 'r1');
    expect(referenceEnds(def.joins[0]).parent.alias).toBe('orders');
    const one = setReference(def, 'r1', { cardinality: '1..1', fromRole: 'buyer' });
    const j = one.joins[0];
    expect(j.relationship).toBe('one_to_one');
    expect(referenceEnds(j).parent.alias).toBe('orders');
    expect(referenceEnds(j).child.alias).toBe('customers');
    expect(j.from).toEqual({ alias: 'customers', column: 'id' });
    expect(j.toRole).toBe('buyer');
    // Back to many: still the same parent.
    expect(referenceEnds(setReference(one, 'r1', { cardinality: '0..*' }).joins[0]).parent.alias).toBe('orders');
  });

  it('swapReference swaps parent and child', () => {
    let def = swapReference(model(), 'r1');
    expect(def.joins[0].relationship).toBe('one_to_many');
    expect(referenceEnds(def.joins[0]).parent.alias).toBe('orders');
    def = setReference(def, 'r1', { cardinality: '0..1' });
    expect(referenceEnds(def.joins[0]).parent.alias).toBe('orders');
    def = swapReference(def, 'r1');
    expect(referenceEnds(def.joins[0]).parent.alias).toBe('customers');
    expect(def.joins[0].from.alias).toBe('orders');
  });

  it('addReference adds the FK column to a draft child with the parent key\'s type', () => {
    const { def, id } = addReference(model(), 'customers', 'items', columnsOf);
    expect(id).toBe('r2');
    const fk = items(def).columns.find((c) => c.name === 'customers_id')!;
    expect(fk).toMatchObject({ type: 'integer', nullable: false, primaryKey: false });
    const j = def.joins.find((x) => x.id === id)!;
    expect(j).toMatchObject({
      from: { alias: 'items', column: 'customers_id' },
      to: { alias: 'customers', column: 'id' },
      relationship: 'many_to_one',
      join: 'inner',
      cardinality: '0..*',
      mandatory: true,
    });
  });

  it('addReference picks the foreign key column of an existing child, not its own key', () => {
    const { def, id } = addReference(model(), 'customers', 'orders', columnsOf);
    const j = def.joins.find((x) => x.id === id)!;
    expect(j.from).toEqual({ alias: 'orders', column: 'customer_id' });
    expect(j.to).toEqual({ alias: 'customers', column: 'id' });
    expect(def.tables.find((t) => t.alias === 'orders')!.draft).toBeUndefined();
    // No column named after the parent: the first that is not the key.
    const other = addReference(model(), 'items', 'orders', columnsOf);
    expect(other.def.joins.find((x) => x.id === other.id)!.from).toEqual({ alias: 'orders', column: 'customer_id' });
  });

  it('addReference writes the parent key\'s type in the dialect', () => {
    const typed = (type: string) => (t: ModelTable) => (t.alias === 'customers' ? [{ name: 'id', type }] : columnsOf(t));
    const fk = (type: string, dialect: 'postgres' | 'mysql') => {
      const { def } = addReference(model(), 'customers', 'items', typed(type), dialect);
      return def.tables.find((t) => t.alias === 'items')!.draft!.columns.find((c) => c.name === 'customers_id')!.type;
    };
    expect(fk('character varying', 'postgres')).toBe('varchar(255)');
    expect(fk('timestamp without time zone', 'postgres')).toBe('timestamp');
    expect(fk('serial', 'postgres')).toBe('integer');
    expect(fk('bigserial', 'postgres')).toBe('bigint');
    expect(fk('varchar', 'mysql')).toBe('varchar(255)');
    expect(fk('int', 'mysql')).toBe('int');
  });

  it('addReference carries the parent key\'s unsigned', () => {
    let def = model();
    def = addColumn(def, 'items', 'mysql');
    def = setColumn(def, 'items', 0, { unsigned: true, type: 'int' });
    const { def: next } = addReference(def, 'items', 'customers', columnsOf, 'mysql');
    expect(next.tables[0].draft).toBeUndefined();
    const draftChild = { ...model(), tables: [...model().tables, { ...model().tables[2], alias: 'lines', table: 'lines' }] };
    const parentSigned = setColumn(draftChild, 'items', 0, { unsigned: true, type: 'int' });
    const made = addReference(parentSigned, 'items', 'lines', columnsOf, 'mysql');
    const col = made.def.tables.find((t) => t.alias === 'lines')!.draft!.columns.find((c) => c.name === 'items_id')!;
    expect(col).toMatchObject({ type: 'int', unsigned: true });
  });

  it('ensureIds numbers the joins that have none', () => {
    const def = ensureIds(model());
    expect(def.joins.map((j) => j.id)).toEqual(['r1', 'r2']);
    expect(ensureIds(def)).toBe(def);
  });
});
