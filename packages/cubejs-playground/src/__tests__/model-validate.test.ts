import { describe, expect, it } from 'vitest';

import { RULES, issues } from '../pages/Schema/models/validate';
import type { ModelDefinition, ModelTable } from '../pages/Schema/models/modelGraph';

const COLUMNS: Record<string, { name: string; type: string }[]> = {
  customers: [{ name: 'id', type: 'integer' }, { name: 'email', type: 'text' }],
  orders: [{ name: 'id', type: 'integer' }, { name: 'customer_id', type: 'text' }],
};
const columnsOf = (t: ModelTable) => (t.draft
  ? t.draft.columns.map((c) => ({ name: c.name, type: c.type }))
  : COLUMNS[t.table] ?? []);

function model(): ModelDefinition {
  return {
    dataSource: 'pg',
    cubes: [{
      id: 'c1',
      name: 'customers',
      source: { kind: 'joined', table: 'customers', primaryKey: 'id' },
      dimensions: [],
      measures: [],
      segments: [],
      x: 600,
      y: 0,
    }],
    tables: [
      {
        alias: 'customers', schema: 's', table: 'customers', x: 0, y: 0,
      },
      {
        alias: 'orders', schema: 's', table: 'orders', x: 0, y: 0,
      },
      {
        alias: 'items',
        schema: 's',
        table: 'items',
        x: 0,
        y: 0,
        draft: {
          columns: [
            { name: 'id', type: 'integer', nullable: true, primaryKey: false },
            { name: 'price', type: 'money', nullable: true, primaryKey: false },
          ],
          indexes: [{ name: 'ix_a', columns: [], unique: false }],
        },
      },
    ],
    joins: [{
      id: 'r1',
      from: { alias: 'orders', column: 'customer_id' },
      to: { alias: 'customers', column: 'email' },
      relationship: 'many_to_one',
      join: 'left',
    }, {
      id: 'r2',
      from: { alias: 'items', column: 'id' },
      to: { alias: 'items', column: 'id' },
      relationship: 'many_to_one',
      join: 'inner',
      mandatory: true,
    }],
  };
}

const messages = (def: ModelDefinition, dialect: 'postgres' | null = 'postgres') => issues(def, columnsOf, dialect)
  .map((i) => i.message);

describe('validate', () => {
  it('flags a table without a primary key', () => {
    const found = issues(model(), columnsOf, 'postgres').find((i) => i.rule === 'table.hasPk')!;
    expect(found).toEqual({
      rule: 'table.hasPk',
      severity: 'warning',
      target: { kind: 'table', alias: 'items' },
      message: 'Table items has no primary key',
    });
  });

  it('flags a primary key column that allows nulls', () => {
    const def = model();
    def.tables[2].draft!.columns[0].primaryKey = true;
    expect(messages(def)).toContain('Column items.id is in the primary key and allows nulls');
    expect(messages(def)).not.toContain('Table items has no primary key');
  });

  it('flags an index without columns', () => {
    const found = issues(model(), columnsOf, 'postgres').find((i) => i.rule === 'index.columns')!;
    expect(found.severity).toBe('error');
    expect(found.message).toBe('Index ix_a of items has no columns');
  });

  it('flags a reference whose columns are not a key of the parent', () => {
    const found = issues(model(), columnsOf, 'postgres').find((i) => i.rule === 'reference.keys')!;
    expect(found).toEqual({
      rule: 'reference.keys',
      severity: 'error',
      target: { kind: 'reference', id: 'r1' },
      message: 'Reference r1: orders.customer_id does not match a key of customers',
    });
    const fixed = model();
    fixed.joins[0].to.column = 'id';
    // The key now matches, but the types differ.
    expect(messages(fixed)).toContain('Reference r1: orders.customer_id does not match a key of customers');
    COLUMNS.orders[1].type = 'integer';
    expect(messages(fixed)).not.toContain('Reference r1: orders.customer_id does not match a key of customers');
    COLUMNS.orders[1].type = 'text';
  });

  it('flags a mandatory reference of a table to itself', () => {
    expect(messages(model())).toContain('Reference r2 links items to itself and is mandatory');
  });

  it('flags names, duplicates and types', () => {
    const def = model();
    const draft = def.tables[2].draft!;
    draft.columns.push({
      name: 'price', type: '', nullable: true, primaryKey: false,
    });
    draft.columns.push({
      name: 'column_3', type: 'varchar(%)', nullable: true, primaryKey: false,
    });
    draft.uniqueKeys = [{ name: 'uk', columns: [] }, { name: 'uk', columns: ['id'] }];
    draft.checks = [{ name: '9x', expression: ' ' }];
    const all = messages(def);
    [
      'Column items.price is repeated',
      'Column items.price has no type',
      'Column items.column_3 has a generated name',
      'Column items.column_3 has a % placeholder in its type',
      'Column items.price: money is not a postgres type',
      'Alternate key uk of items has no columns',
      'Alternate key uk of items is repeated',
      'Check 9x of items has an empty expression',
      'Check 9x of items has an invalid name',
    ].forEach((m) => expect(all, m).toContain(m));
  });

  it('a rule switched off in settings is skipped', () => {
    const def = { ...model(), settings: { validation: { 'table.hasPk': false } } };
    expect(messages(def)).not.toContain('Table items has no primary key');
  });

  it('dialect rules are off without a dialect', () => {
    expect(messages(model(), null).some((m) => m.includes('is not a'))).toBe(false);
    expect(RULES.filter((r) => r.dialect).map((r) => r.id)).toEqual(['column.supportedType', 'viewColumn.supportedType']);
  });

  it('every rule has a group, a label and a severity', () => {
    expect(RULES.length).toBeGreaterThanOrEqual(34);
    RULES.forEach((r) => {
      expect(r.label, r.id).toBeTruthy();
      expect(['error', 'warning']).toContain(r.severity);
    });
  });

  const view = (over = {}) => ({
    id: 'v1', name: 'v_a', schema: 's', sql: 'select 1', columns: [], dependsOnTables: [], dependsOnViews: [], x: 0, y: 0, ...over,
  });

  it('flags a view with no query and a repeated column', () => {
    const def = {
      ...model(),
      views: [view({ sql: ' ', columns: [{ name: 'a', type: 'integer' }, { name: 'A', type: 'integer' }] }), view({ id: 'v2' })],
    };
    const found = issues(def, columnsOf, 'postgres').filter((i) => i.target.kind === 'view');
    expect(found.map((i) => i.rule).sort()).toEqual(['view.emptyQuery', 'view.uniqueName', 'viewColumn.uniqueName']);
    expect(found.every((i) => (i.target as { id: string }).id.startsWith('v'))).toBe(true);
  });

  it('a dialect-less model gives no view column type issue', () => {
    const def = { ...model(), views: [view({ columns: [{ name: 'a', type: 'money' }] })] };
    expect(issues(def, columnsOf, 'postgres').map((i) => i.rule)).toContain('viewColumn.supportedType');
    expect(issues(def, columnsOf, null).map((i) => i.rule)).not.toContain('viewColumn.supportedType');
  });

  it('a disabled view rule gives no issue', () => {
    const def = { ...model(), views: [view({ sql: '' })], settings: { validation: { 'view.emptyQuery': false } } };
    expect(issues(def, columnsOf, 'postgres').map((i) => i.rule)).not.toContain('view.emptyQuery');
  });

  it('a view with no columns has no issue', () => {
    expect(issues({ ...model(), views: [view()] }, columnsOf, 'postgres').filter((i) => i.target.kind === 'view')).toEqual([]);
  });
});
