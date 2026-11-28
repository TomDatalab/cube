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

describe('cube issues', () => {
  const cube = (over = {}) => ({
    id: 'c2',
    name: 'orders_cube',
    source: { kind: 'table' as const, table: 'orders', primaryKey: 'id' },
    dimensions: [],
    measures: [],
    segments: [],
    x: 0,
    y: 0,
    ...over,
  });
  const withCubes = (...cubes: ReturnType<typeof cube>[]) => {
    const base = model();
    return { ...base, cubes: [...(base.cubes ?? []), ...cubes] } as ModelDefinition;
  };
  const rulesOf = (def: ModelDefinition, kind = 'cube') => issues(def, columnsOf, 'postgres')
    .filter((i) => i.target.kind === kind)
    .map((i) => i.rule)
    .sort();

  it('a clean auto cube has no issue', () => {
    expect(rulesOf(withCubes(cube()))).toEqual([]);
  });

  it('flags names: empty, invalid, repeated, not long', () => {
    expect(rulesOf(withCubes(cube({ name: ' ' })))).toEqual(['cube.emptyName']);
    expect(rulesOf(withCubes(cube({ name: '1a' })))).toEqual(['cube.invalidName']);
    // A converted model's cube may have a long name: Cube has no cap.
    expect(rulesOf(withCubes(cube({ name: 'a'.repeat(64) })))).toEqual([]);
    expect(rulesOf(withCubes(cube({ name: 'customers' })))).toEqual(['cube.uniqueName']);
  });

  it('flags a cube with no table, a table outside the model and a draft table', () => {
    expect(rulesOf(withCubes(cube({ source: { kind: 'table' } })))).toEqual(['cube.noTable']);
    expect(rulesOf(withCubes(cube({ source: { kind: 'table', table: 'gone' } })))).toEqual(['cube.unknownTable']);
    expect(rulesOf(withCubes(cube({ source: { kind: 'table', table: 'items', primaryKey: 'id' } })))).toEqual(['cube.draftTable']);
  });

  it('flags a sql cube with no query and no members, but not one with members', () => {
    const sql = (over = {}) => cube({ source: { kind: 'sql', sql: 'select 1', tables: ['orders'] }, ...over });
    expect(rulesOf(withCubes(sql()))).toEqual(['cube.noMembers']);
    expect(rulesOf(withCubes(sql({ source: { kind: 'sql', sql: ' ' }, measures: [{ name: 'n', type: 'count' }] })))).toEqual(['cube.noSql']);
    expect(rulesOf(withCubes(sql({ measures: [{ name: 'n', type: 'count' }] })))).toEqual([]);
    expect(rulesOf(withCubes(sql({ source: { kind: 'sql', sql: 'x', tables: ['gone'] }, measures: [{ name: 'n', type: 'count' }] })))).toEqual(['cube.unknownTable']);
  });

  it('flags an auto cube with no primary key, and a key that is not a column', () => {
    expect(rulesOf(withCubes(cube({ source: { kind: 'table', table: 'orders' } })))).toEqual(['cube.primaryKey']);
    expect(rulesOf(withCubes(cube({ source: { kind: 'joined', table: 'orders' } })))).toEqual(['cube.primaryKey']);
    expect(rulesOf(withCubes(cube({ source: { kind: 'table', table: 'orders', primaryKey: 'nope' } })))).toEqual(['cube.primaryKey']);
    expect(rulesOf(withCubes(cube({
      source: { kind: 'table', table: 'orders' }, measures: [{ name: 'n', type: 'count' }],
    })))).toEqual([]);
  });

  it('flags member names, types, sql-or-column and formats', () => {
    const def = withCubes(cube({
      dimensions: [
        { name: 'a', type: 'string', column: 'id' },
        { name: 'a', type: 'bogus', column: 'id' },
        { name: '', type: 'string', sql: 'x' },
        { name: 'b c', type: 'string' },
        { name: 'd', type: 'string', sql: 'x', column: 'id' },
      ],
      measures: [
        { name: 'm', type: 'sum' },
        { name: 'n', type: 'count', format: 'weird' },
        { name: 'k', type: 'nope', sql: 'x' },
      ],
      segments: [{ name: 'a', sql: '1=1' }],
    }));
    const found = rulesOf(def);
    expect(found.filter((r) => r === 'cubeMember.uniqueName')).toHaveLength(2);
    expect(found).toContain('cubeMember.emptyName');
    expect(found).toContain('cubeMember.invalidName');
    expect(found.filter((r) => r === 'cubeMember.type')).toHaveLength(2);
    expect(found.filter((r) => r === 'cubeMember.sqlOrColumn')).toHaveLength(3);
    expect(found).toContain('cubeMember.format');
  });

  it('a count measure needs neither sql nor column', () => {
    expect(rulesOf(withCubes(cube({ measures: [{ name: 'n', type: 'count' }] })))).toEqual([]);
  });

  it('flags a column that is not in the table the cube reads', () => {
    const def = withCubes(cube({ dimensions: [{ name: 'a', type: 'string', column: 'nope' }] }));
    expect(rulesOf(def)).toEqual(['cubeMember.unknownColumn']);
    expect(rulesOf(withCubes(cube({ dimensions: [{ name: 'a', type: 'string', column: 'id' }] })))).toEqual([]);
  });

  it('a joined cube takes alias.column over the tables it reaches', () => {
    const source = { kind: 'joined' as const, table: 'orders', primaryKey: 'id' };
    const ok = cube({ source, dimensions: [{ name: 'a', type: 'string', column: 'customers.email' }] });
    expect(rulesOf(withCubes(ok))).toEqual([]);
    const bad = cube({
      source,
      dimensions: [{ name: 'a', type: 'string', column: 'email' }, { name: 'b', type: 'string', column: 'customers.zip' }],
    });
    expect(rulesOf(withCubes(bad))).toEqual(['cubeMember.unknownColumn', 'cubeMember.unknownColumn']);
  });

  it('a joined cube flags a column whose type Cube cannot hold', () => {
    const odd = { ...COLUMNS, orders: [...COLUMNS.orders, { name: 'tags', type: 'text[]' }] };
    const cols = (t: ModelTable) => (odd[t.table] ?? columnsOf(t));
    const source = { kind: 'joined' as const, table: 'orders', primaryKey: 'id' };
    const def = withCubes(cube({ source, dimensions: [{ name: 'a', type: 'string', column: 'orders.tags' }] }));
    const found = issues(def, cols, 'postgres').filter((i) => i.target.kind === 'cube').map((i) => i.rule);
    expect(found).toEqual(['cubeMember.unknownColumn']);
  });

  it('a sql cube does not check columns', () => {
    const def = withCubes(cube({
      source: { kind: 'sql', sql: 'select 1', tables: ['orders'] },
      dimensions: [{ name: 'a', type: 'string', column: 'whatever' }],
    }));
    expect(rulesOf(def)).toEqual([]);
  });

  it('a disabled cube rule gives no issue', () => {
    const def = { ...withCubes(cube({ source: { kind: 'sql', sql: '' } })), settings: { validation: { 'cube.noSql': false } } };
    expect(rulesOf(def as ModelDefinition)).toEqual(['cube.noMembers']);
  });

  describe('cube joins', () => {
    const base = () => withCubes(cube({ dimensions: [{ name: 'customer_id', type: 'number', column: 'customer_id' }] }));
    const join = (over = {}) => ({
      id: 'cj1', from: 'c2', to: 'c1', relationship: 'many_to_one' as const, on: [{ from: 'customer_id', to: 'customers_id' }], points: [], ...over,
    });
    const withJoins = (...joins: ReturnType<typeof join>[]) => ({ ...base(), cubeJoins: joins }) as ModelDefinition;

    it('a good join has no issue', () => {
      expect(rulesOf(withJoins(join()), 'cubeJoin')).toEqual([]);
    });

    it('flags ends that are not cubes, a self join, a bad relationship and no pairs', () => {
      expect(rulesOf(withJoins(join({ to: 'c9' })), 'cubeJoin')).toEqual(['cubeJoin.cubes']);
      expect(rulesOf(withJoins(join({ to: 'c2' })), 'cubeJoin')).toContain('cubeJoin.cubes');
      expect(rulesOf(withJoins(join({ relationship: 'x' as never })), 'cubeJoin')).toEqual(['cubeJoin.relationship']);
      expect(rulesOf(withJoins(join({ on: [] })), 'cubeJoin')).toEqual(['cubeJoin.noPairs']);
    });

    it('flags a pair that is not a dimension of its cube', () => {
      expect(rulesOf(withJoins(join({ on: [{ from: 'nope', to: 'customers_id' }] })), 'cubeJoin')).toEqual(['cubeJoin.dimension']);
      expect(rulesOf(withJoins(join({ on: [{ from: 'customer_id', to: 'nope' }] })), 'cubeJoin')).toEqual(['cubeJoin.dimension']);
    });

    it('flags the second join between the same two cubes, in either direction', () => {
      const found = issues(withJoins(join(), join({ id: 'cj2', from: 'c1', to: 'c2', on: [{ from: 'customers_id', to: 'customer_id' }] })), columnsOf, 'postgres')
        .filter((i) => i.rule === 'cubeJoin.duplicate');
      expect(found.map((i) => (i.target as { id: string }).id)).toEqual(['cj2']);
    });

    it('a dimension of an auto cube counts, under the name auto mode gives it', () => {
      const def = { ...model(), cubeJoins: [join()] };
      def.cubes = [...def.cubes!, cube()];
      expect(rulesOf(def as ModelDefinition, 'cubeJoin')).toEqual([]);
    });
  });

  it('every cube rule is listed', () => {
    const ids = RULES.map((r) => r.id);
    for (const id of ['cube.noSql', 'cube.primaryKey', 'cubeMember.unknownColumn', 'cubeJoin.duplicate']) {
      expect(ids).toContain(id);
    }
  });
});
