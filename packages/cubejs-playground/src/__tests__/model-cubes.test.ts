import { describe, expect, it } from 'vitest';

import {
  addCube, addCubeJoin, addMember, autoMembers, cubeKeyOf, cubeReads, cubeType, dimensionNames, generateMembers,
  moveCubeJoinEnd, removeCubeJoin, removeMember, renameCube, renameMember, setCube, setCubeJoin, setCubeSource,
  friendlyNameProblem, memberTitles, setCubeSourceLine, setMemberTitle, swapCubeJoin,
  setMember, snake,
} from '../pages/Schema/models/cubes';
import {
  CubeNode, ModelDefinition, addTable, connect, emptyDefinition, removeTable, renameTable,
} from '../pages/Schema/models/modelGraph';
import { nodeIdOf, removeObjects } from '../pages/Schema/models/objects';

const COLUMNS: Record<string, { name: string; type: string }[]> = {
  orders: [
    { name: 'id', type: 'int4' },
    { name: 'customerId', type: 'int4' },
    { name: 'amount', type: 'numeric' },
    { name: 'extra', type: 'jsonb' },
  ],
  customers: [
    { name: 'id', type: 'int4' },
    { name: 'region', type: 'text' },
  ],
  items: [{ name: 'id', type: 'int4' }],
};
const columnsOf = (_schema: string, table: string) => COLUMNS[table];

function model(): ModelDefinition {
  let def = emptyDefinition();
  def = addTable(def, 'ds', 'public', 'orders', 0, 0).def;
  def = addTable(def, 'ds', 'public', 'customers', 300, 0).def;
  def = addTable(def, 'ds', 'public', 'items', 600, 0).def;
  def = connect(def, { alias: 'orders', column: 'customerId' }, { alias: 'customers', column: 'id' }, 'many_to_one', 'left');
  return def;
}

function joined(): { def: ModelDefinition; cube: CubeNode } {
  const added = addCube(model(), 900, 0);
  const def = setCubeSource(added.def, added.id, { kind: 'joined', table: 'orders', primaryKey: 'id' });
  return { def, cube: def.cubes![0] };
}

describe('cubes', () => {
  it('snake and cubeType match the server', () => {
    expect(snake('fiberCableId')).toBe('fiber_cable_id');
    expect(snake('Order ID')).toBe('order_id');
    expect(snake('1a')).toBe('c_1a');
    expect(snake('')).toBe('col');
    expect(cubeType('int4')).toBe('number');
    expect(cubeType('timestamptz')).toBe('time');
    expect(cubeType('jsonb')).toBeUndefined();
    expect(cubeType('Nullable(String)')).toBe('string');
    expect(cubeType('Nullable(LowCardinality(UInt8))')).toBe('number');
    expect(cubeType('integer[]')).toBeUndefined();
    expect(cubeType('boolean')).toBe('boolean');
    expect(cubeType('int4range')).toBe('string');
  });

  it('autoMembers of a joined cube', () => {
    const { def, cube } = joined();
    const auto = autoMembers(def, cube, columnsOf);
    expect(auto.dimensions.map((d) => d.name)).toEqual([
      'orders_id', 'orders_customer_id', 'orders_amount', 'customers_id', 'customers_region',
    ]);
    expect(auto.dimensions[0]).toEqual({
      name: 'orders_id', type: 'number', column: 'orders.id', primaryKey: true,
    });
    expect(auto.dimensions[4]).toEqual({ name: 'customers_region', type: 'string', column: 'customers.region' });
    expect(auto.measures).toEqual([{ name: 'count', type: 'count' }]);
  });

  it('autoMembers of a table cube, a one_to_many and a sql cube', () => {
    let { def } = addCube(model(), 0, 0);
    const table = def.cubes![0];
    expect(autoMembers(def, table, columnsOf).dimensions.map((d) => [d.name, d.column])).toEqual([
      ['id', 'id'], ['customer_id', 'customerId'], ['amount', 'amount'],
    ]);
    def = setCubeSource(def, 'c1', { kind: 'joined', table: 'customers', primaryKey: 'id' });
    expect(autoMembers(def, def.cubes![0], columnsOf).measures).toEqual([
      { name: 'count', type: 'count_distinct', column: 'customers.id' },
    ]);
    def = setCubeSource(def, 'c1', { kind: 'sql', sql: 'select 1', tables: ['orders'] });
    expect(autoMembers(def, def.cubes![0], columnsOf)).toEqual({ dimensions: [], measures: [] });
  });

  it('auto names are deduplicated and count is reserved', () => {
    const cols = { t: [{ name: 'count', type: 'int' }, { name: 'a_b', type: 'text' }, { name: 'aB', type: 'text' }] };
    let { def } = addTable(emptyDefinition(), 'ds', 'public', 't', 0, 0);
    def = addCube(def, 0, 0).def;
    const auto = autoMembers(def, def.cubes![0], (_s, t) => cols[t as 't']);
    expect(auto.dimensions.map((d) => d.name)).toEqual(['count_2', 'a_b', 'a_b_2']);
  });

  it('generateMembers makes them explicit', () => {
    const { def, cube } = joined();
    const before = dimensionNames(def, cube, columnsOf);
    const next = generateMembers(def, cube.id, columnsOf);
    const made = next.cubes![0];
    expect(made.dimensions.map((d) => d.name)).toEqual(before);
    expect(made.measures.map((m) => m.name)).toEqual(['count']);
    expect(dimensionNames(next, made, columnsOf)).toEqual(before);
  });

  it('friendly names: kept by name for an auto cube, carried by Generate members', () => {
    const { def, cube } = joined();
    const name = dimensionNames(def, cube, columnsOf)[0];
    let next = setMemberTitle(def, cube.id, 'dimensions', name, '  Cliente ');
    next = setMemberTitle(next, cube.id, 'measures', 'count', 'Quantidade');
    expect(next.cubes![0].memberTitles).toEqual({ [name]: 'Cliente', count: 'Quantidade' });
    expect(memberTitles(next.cubes![0])).toEqual({ [name]: 'Cliente', count: 'Quantidade' });
    // A blank title takes it away, and the empty map goes too.
    const cleared = setMemberTitle(setMemberTitle(next, cube.id, 'measures', 'count', ''), cube.id, 'dimensions', name, ' ');
    expect(cleared.cubes![0]).not.toHaveProperty('memberTitles');
    // Generated, the members carry their titles and the map is gone.
    const made = generateMembers(next, cube.id, columnsOf).cubes![0];
    expect(made).not.toHaveProperty('memberTitles');
    expect(made.dimensions.find((d) => d.name === name)!.title).toBe('Cliente');
    expect(made.measures[0].title).toBe('Quantidade');
    // A cube with members of its own sets the member's title.
    const own = setMemberTitle({ ...def, cubes: [made] }, cube.id, 'dimensions', name, 'Client');
    expect(own.cubes![0].dimensions.find((d) => d.name === name)!.title).toBe('Client');
    expect(memberTitles(own.cubes![0])[name]).toBe('Client');
  });

  it('friendlyNameProblem follows the server rules', () => {
    const names = ['id', 'count', 'status'];
    expect(friendlyNameProblem(names, { count: 'Quantidade Total' }, 'count')).toBeUndefined();
    expect(friendlyNameProblem(names, { status: 'Situação do Pedido' }, 'status')).toBeUndefined();
    for (const bad of ['Total!', 'a.b', 'a  b', '1 a']) {
      expect(friendlyNameProblem(names, { count: bad }, 'count')).toBe('Use letters, digits, _ and single spaces');
    }
    expect(friendlyNameProblem(names, { count: 'id' }, 'count')).toBe('Already the name of id');
    expect(friendlyNameProblem(names, { count: 'Total', status: 'Total' }, 'status')).toBe('Already the name of count');
  });

  it('cubeReads walks references from the primary', () => {
    let { def, cube } = joined();
    expect(cubeReads(def, cube)).toEqual(['orders', 'customers']);
    def = connect(def, { alias: 'items', column: 'order_id' }, { alias: 'orders', column: 'id' }, 'many_to_one', 'left');
    expect(cubeReads(def, def.cubes![0])).toEqual(['orders', 'customers', 'items']);
    def = setCubeSource(def, cube.id, { kind: 'joined', table: 'customers' });
    expect(cubeReads(def, def.cubes![0])).toEqual(['customers', 'orders', 'items']);
    def = setCubeSource(def, cube.id, { kind: 'table', table: 'items' });
    expect(cubeReads(def, def.cubes![0])).toEqual(['items']);
    def = setCubeSource(def, cube.id, { kind: 'sql', sql: '', tables: ['items', 'orders'] });
    expect(cubeReads(def, def.cubes![0])).toEqual(['items', 'orders']);
    def = setCubeSource(def, cube.id, { kind: 'table' });
    expect(cubeReads(def, def.cubes![0])).toEqual([]);
  });

  it('adds cubes and cube joins with the smallest free id', () => {
    const a = addCube(model(), 10.4, 20.6);
    expect(a.id).toBe('c1');
    expect(a.def.cubes![0]).toEqual({
      id: 'c1',
      name: 'cube_1',
      color: 'violet',
      source: { kind: 'table', table: 'orders' },
      dimensions: [],
      measures: [],
      segments: [],
      x: 10,
      y: 21,
    });
    const b = addCube(a.def, 0, 0);
    expect([b.id, b.def.cubes![1].name]).toEqual(['c2', 'cube_2']);
    let def: ModelDefinition = { ...b.def, cubes: b.def.cubes!.filter((c) => c.id !== 'c1') };
    const c = addCube(def, 0, 0);
    expect([c.id, c.def.cubes![1].name]).toEqual(['c1', 'cube_1']);
    expect(addCube(emptyDefinition(), 0, 0).def.cubes![0].source).toEqual({ kind: 'table' });
    def = c.def;
    const j1 = addCubeJoin(def, 'c2', 'c1', 'many_to_one', [{ from: 'a', to: 'b' }]);
    expect(j1.id).toBe('cj1');
    expect(j1.def.cubeJoins).toEqual([{
      id: 'cj1', from: 'c2', to: 'c1', relationship: 'many_to_one', on: [{ from: 'a', to: 'b' }], points: [],
    }]);
    const j2 = addCubeJoin(j1.def, 'c1', 'c2', 'one_to_one', []);
    expect(j2.id).toBe('cj2');
    const j3 = addCubeJoin(removeCubeJoin(j2.def, 'cj1'), 'c1', 'c2', 'one_to_one', []);
    expect(j3.id).toBe('cj1');
    const set = setCubeJoin(j2.def, 'cj2', { hidden: true, relationship: 'one_to_many' });
    expect(set.cubeJoins![1]).toMatchObject({ hidden: true, relationship: 'one_to_many' });
    expect('hidden' in setCubeJoin(set, 'cj2', { hidden: undefined }).cubeJoins![1]).toBe(false);
  });

  it('setCubeSourceLine adds a cube\'s line to a table, then reshapes it', () => {
    const { def, id } = addCube(model(), 0, 0);
    const one = setCubeSourceLine(def, id, 'orders', { points: [{ x: 1, y: 2 }] });
    expect(one.cubes![0].sourceLines).toEqual([{ table: 'orders', points: [{ x: 1, y: 2 }] }]);
    const two = setCubeSourceLine(one, id, 'orders', { toSide: 'left' });
    expect(two.cubes![0].sourceLines).toEqual([{ table: 'orders', points: [{ x: 1, y: 2 }], toSide: 'left' }]);
    // Another cube is left as it was.
    expect(setCubeSourceLine(two, 'c9', 'orders', { points: [] })).toEqual(two);
  });

  it('setCube, renameCube and members', () => {
    let { def, id } = addCube(model(), 0, 0);
    def = setCube(def, id, { title: 'Orders', color: 'teal' });
    expect(def.cubes![0]).toMatchObject({ title: 'Orders', color: 'teal' });
    def = setCube(def, id, { title: undefined });
    expect('title' in def.cubes![0]).toBe(false);
    def = addCube(def, 0, 0).def;
    expect(renameCube(def, id, '1x').error).toBe('Invalid name: 1x');
    expect(renameCube(def, id, 'cube_2').error).toBe('cube_2 is already in this model');
    expect(renameCube(def, id, 'cube_1').def).toBe(def);
    def = renameCube(def, id, 'orders').def;
    expect(def.cubes![0].name).toBe('orders');

    const d = addMember(def, id, 'dimensions');
    expect(d.name).toBe('dimension_1');
    const m = addMember(d.def, id, 'measures');
    const s = addMember(m.def, id, 'segments');
    def = s.def;
    expect(def.cubes![0].dimensions).toEqual([{ name: 'dimension_1', type: 'string', sql: '' }]);
    expect(def.cubes![0].measures).toEqual([{ name: 'measure_1', type: 'count' }]);
    expect(def.cubes![0].segments).toEqual([{ name: 'segment_1', sql: '1 = 1' }]);
    expect(addMember(def, id, 'dimensions').name).toBe('dimension_2');
    def = setMember(def, id, 'dimensions', 0, { column: 'id', sql: undefined, type: 'number' });
    expect(def.cubes![0].dimensions[0]).toEqual({ name: 'dimension_1', type: 'number', column: 'id' });
    def = removeMember(def, id, 'measures', 0);
    expect(def.cubes![0].measures).toEqual([]);
  });

  it('renameCube caps a new name at 63 characters', () => {
    const { def, id } = addCube(model(), 0, 0);
    expect(renameCube(def, id, 'a'.repeat(64)).error).toBe(`Invalid name: ${'a'.repeat(64)}`);
    expect(renameCube(def, id, 'a'.repeat(63)).def.cubes![0].name).toBe('a'.repeat(63));
  });

  it('renameMember checks the name against every member of the cube', () => {
    let { def, id } = addCube(model(), 0, 0);
    def = addMember(def, id, 'dimensions').def;
    def = addMember(def, id, 'measures').def;
    def = addMember(def, id, 'segments').def;
    expect(renameMember(def, id, 'dimensions', 0, 'dimension_1').def).toBe(def);
    expect(renameMember(def, id, 'dimensions', 0, '1x')).toEqual({ def, error: 'Invalid name: 1x' });
    expect(renameMember(def, id, 'dimensions', 0, 'a-b').error).toBe('Invalid name: a-b');
    expect(renameMember(def, id, 'dimensions', 0, 'a'.repeat(64)).error).toBe(`Invalid name: ${'a'.repeat(64)}`);
    expect(renameMember(def, id, 'dimensions', 0, 'measure_1').error).toBe('measure_1 is already in this cube');
    expect(renameMember(def, id, 'measures', 0, 'segment_1').error).toBe('segment_1 is already in this cube');
    expect(renameMember(def, id, 'segments', 0, 'dimension_1').error).toBe('dimension_1 is already in this cube');
    const renamed = renameMember(def, id, 'measures', 0, 'total');
    expect(renamed.error).toBeUndefined();
    expect(renamed.def.cubes![0].measures[0].name).toBe('total');
  });

  it('setMember keeps what the server needs on a type change and a segment', () => {
    let { def, id } = addCube(model(), 0, 0);
    def = addMember(def, id, 'measures').def;
    // A count with neither SQL nor column becomes a sum: it gets an empty SQL.
    def = setMember(def, id, 'measures', 0, { type: 'sum' });
    expect(def.cubes![0].measures[0]).toEqual({ name: 'measure_1', type: 'sum', sql: '' });
    // Back to a count, the blank SQL goes.
    def = setMember(def, id, 'measures', 0, { type: 'count' });
    expect(def.cubes![0].measures[0]).toEqual({ name: 'measure_1', type: 'count' });
    def = setMember(def, id, 'measures', 0, { type: 'sum', column: 'amount' });
    expect(def.cubes![0].measures[0]).toEqual({ name: 'measure_1', type: 'sum', column: 'amount' });
    def = setMember(def, id, 'measures', 0, { type: 'count' });
    expect(def.cubes![0].measures[0]).toEqual({ name: 'measure_1', type: 'count', column: 'amount' });
    def = setMember(def, id, 'measures', 0, { type: 'sum', column: undefined, sql: 'x' });
    def = setMember(def, id, 'measures', 0, { type: 'count' });
    expect(def.cubes![0].measures[0]).toEqual({ name: 'measure_1', type: 'count', sql: 'x' });
    // A segment always keeps its SQL.
    def = addMember(def, id, 'segments').def;
    def = setMember(def, id, 'segments', 0, { sql: undefined });
    expect(def.cubes![0].segments[0]).toEqual({ name: 'segment_1', sql: '' });
  });

  it('changing the source keeps members that no longer resolve', () => {
    let { def, cube } = joined();
    def = generateMembers(def, cube.id, columnsOf);
    const members = def.cubes![0].dimensions;
    def = setCubeSource(def, cube.id, { kind: 'table', table: 'items' });
    expect(def.cubes![0].dimensions).toEqual(members);
    expect(def.cubes![0].source).toEqual({ kind: 'table', table: 'items' });
  });

  it('cubeKeyOf finds the key of a table or joined cube on the alias', () => {
    const { def } = joined();
    expect(cubeKeyOf(def, 'orders')).toBe('id');
    expect(cubeKeyOf(def, 'customers')).toBeUndefined();
    expect(cubeKeyOf(setCubeSource(def, 'c1', { kind: 'sql', table: 'orders', primaryKey: 'id' }), 'orders'))
      .toBeUndefined();
  });

  it('renaming a table renames cube sources and joined columns', () => {
    let { def, cube } = joined();
    def = generateMembers(def, cube.id, columnsOf);
    def = addCube(def, 0, 0).def;
    def = setCubeSource(def, 'c2', { kind: 'sql', sql: 'x', tables: ['customers', 'orders'] });
    def = setMember(addMember(def, 'c2', 'dimensions').def, 'c2', 'dimensions', 0, { column: 'orders.id', sql: undefined });
    const next = renameTable(def, 'orders', 'o').def;
    const [c1, c2] = next.cubes!;
    expect(c1.source.table).toBe('o');
    expect(c1.dimensions.map((d) => d.column)).toEqual(['o.id', 'o.customerId', 'o.amount', 'customers.id', 'customers.region']);
    expect(c2.source.tables).toEqual(['customers', 'o']);
    // A sql cube's columns are its query's own: not renamed.
    expect(c2.dimensions[0].column).toBe('orders.id');
  });

  it('removing a table clears the cube source', () => {
    let { def } = joined();
    def = addCube(def, 0, 0).def;
    def = setCubeSource(def, 'c2', { kind: 'sql', sql: 'x', tables: ['customers', 'orders'] });
    const next = removeTable(def, 'orders');
    expect(next.cubes![0].source).toEqual({ kind: 'joined' });
    expect(next.cubes![1].source.tables).toEqual(['customers']);
    expect('tables' in removeTable(next, 'customers').cubes![1].source).toBe(false);
  });

  it('removing a cube removes its cube joins', () => {
    let def = model();
    for (let i = 0; i < 3; i++) {
      def = addCube(def, 0, 0).def;
    }
    def = addCubeJoin(def, 'c1', 'c2', 'many_to_one', [{ from: 'a', to: 'b' }]).def;
    def = addCubeJoin(def, 'c3', 'c1', 'many_to_one', [{ from: 'a', to: 'b' }]).def;
    def = addCubeJoin(def, 'c2', 'c3', 'many_to_one', [{ from: 'a', to: 'b' }]).def;
    const next = removeObjects(def, [nodeIdOf('cube', 'c1')]);
    expect(next.cubes!.map((c) => c.id)).toEqual(['c2', 'c3']);
    expect(next.cubeJoins!.map((j) => j.id)).toEqual(['cj3']);
    expect(next.tables).toEqual(def.tables);
  });
});

describe('changing a cube join', () => {
  const def = {
    dataSource: 'ds',
    tables: [],
    joins: [],
    cubeJoins: [{
      id: 'cj1',
      from: 'c1',
      to: 'c2',
      relationship: 'many_to_one' as const,
      on: [{ from: 'a', to: 'b' }],
      points: [{ x: 1, y: 1 }, { x: 2, y: 2 }],
      fromSide: 'left' as const,
    }],
  };

  it('swapCubeJoin reads it the other way', () => {
    expect(swapCubeJoin(def, 'cj1').cubeJoins![0]).toEqual({
      id: 'cj1',
      from: 'c2',
      to: 'c1',
      relationship: 'one_to_many',
      on: [{ from: 'b', to: 'a' }],
      points: [{ x: 2, y: 2 }, { x: 1, y: 1 }],
      toSide: 'left',
    });
  });

  it('moveCubeJoinEnd keeps the names the new cube has and drops the shape', () => {
    const kept = moveCubeJoinEnd(def, 'cj1', 'to', 'c3', ['x', 'b']).cubeJoins![0];
    expect(kept).toEqual(expect.objectContaining({ to: 'c3', on: [{ from: 'a', to: 'b' }], points: [] }));
    expect(kept).not.toHaveProperty('fromSide', 'right');
    const moved = moveCubeJoinEnd(def, 'cj1', 'from', 'c3', ['x']).cubeJoins![0];
    expect(moved).toEqual(expect.objectContaining({ from: 'c3', on: [{ from: 'x', to: 'b' }] }));
    expect(moved).not.toHaveProperty('fromSide');
  });
});
