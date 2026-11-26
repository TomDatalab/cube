import { describe, expect, it } from 'vitest';

import {
  addDraftTable, addTable, canConnect, draftProblem, drafts, layout, setColor, updateDraftTable,
  connect, emptyDefinition, joinLabel, removeJoin, removeTable, setPrimary, unlinked,
  updateJoin, renameTable, setTableComment, placeTables, setJoin, showAllJoins, withFormat,
} from '../pages/Schema/models/modelGraph';

function build(tables: string[]) {
  let def = emptyDefinition();
  for (const t of tables) {
    def = addTable(def, 'ds', 'public', t, 0, 0).def;
  }
  return def;
}

describe('modelGraph', () => {
  it('emptyDefinition', () => {
    expect(emptyDefinition()).toEqual({ dataSource: '', tables: [], joins: [] });
  });

  it('addTable sets dataSource and primary, aliases duplicates, refuses other ds', () => {
    const a = addTable(emptyDefinition(), 'ds', 'public', 'users', 1, 2);
    expect(a.error).toBeUndefined();
    expect(a.def.dataSource).toBe('ds');
    expect(a.def.primary).toBe('users');
    const b = addTable(a.def, 'ds', 'public', 'users', 0, 0);
    const c = addTable(b.def, 'ds', 'public', 'users', 0, 0);
    expect(c.def.tables.map((t) => t.alias)).toEqual(['users', 'users_2', 'users_3']);
    const d = addTable(c.def, 'other', 'public', 'x', 0, 0);
    expect(d.error).toBe('Tables of one connection only: this model reads ds');
    expect(d.def).toEqual(c.def);
  });

  it('does not mutate inputs in addTable and connect', () => {
    const def = build(['a', 'b']);
    const snap = JSON.stringify(def);
    addTable(def, 'ds', 'public', 'c', 0, 0);
    connect(def, { alias: 'a', column: 'id' }, { alias: 'b', column: 'a_id' }, 'one_to_many', 'left');
    expect(JSON.stringify(def)).toBe(snap);
  });

  it('removeTable drops joins, moves primary, resets dataSource', () => {
    let def = build(['a', 'b']);
    def = connect(def, { alias: 'a', column: 'id' }, { alias: 'b', column: 'x' }, 'one_to_many', 'left');
    const r = removeTable(def, 'a');
    expect(r.joins).toEqual([]);
    expect(r.primary).toBe('b');
    const e = removeTable(r, 'b');
    expect(e.dataSource).toBe('');
    expect(e.tables).toEqual([]);
    expect(e.primary ?? null).toBeNull();
  });

  it('canConnect refuses same alias, duplicate pair and cycles', () => {
    let def = build(['a', 'b', 'c']);
    const e = (alias: string) => ({ alias, column: 'id' });
    expect(canConnect(def, e('a'), e('a'))).toMatch(/itself|same/i);
    expect(canConnect(def, e('a'), e('b'))).toBeNull();
    def = connect(def, e('a'), e('b'), 'many_to_one', 'left');
    expect(canConnect(def, e('b'), e('a'))).toMatch(/already/i);
    def = connect(def, e('b'), e('c'), 'many_to_one', 'left');
    expect(canConnect(def, e('c'), e('a'))).toMatch(/cycle/i);
  });

  it('connect, updateJoin, removeJoin', () => {
    let def = build(['a', 'b']);
    def = connect(def, { alias: 'a', column: 'id' }, { alias: 'b', column: 'x' }, 'one_to_many', 'left');
    expect(def.joins).toHaveLength(1);
    const u = updateJoin(def, 0, { join: 'inner' });
    expect(u.joins[0].join).toBe('inner');
    expect(def.joins[0].join).toBe('left');
    expect(removeJoin(u, 0).joins).toEqual([]);
  });

  it('joinLabel', () => {
    const base = { from: { alias: 'a', column: 'x' }, to: { alias: 'b', column: 'y' } };
    expect(joinLabel({ ...base, relationship: 'many_to_one', join: 'left' })).toBe('N:1 · LEFT');
    expect(joinLabel({ ...base, relationship: 'one_to_many', join: 'inner' })).toBe('1:N · INNER');
    expect(joinLabel({ ...base, relationship: 'one_to_one', join: 'left' })).toBe('1:1 · LEFT');
  });

  it('unlinked lists aliases not reachable from primary', () => {
    let def = build(['a', 'b', 'c']);
    expect(unlinked(def)).toEqual(['b', 'c']);
    def = connect(def, { alias: 'b', column: 'x' }, { alias: 'a', column: 'y' }, 'many_to_one', 'left');
    expect(unlinked(def)).toEqual(['c']);
    expect(unlinked(emptyDefinition())).toEqual([]);
  });

  it('reuses a freed alias after removal', () => {
    let def = build(['customers', 'customers']);
    expect(def.tables.map((t) => t.alias)).toEqual(['customers', 'customers_2']);
    def = removeTable(def, 'customers');
    def = addTable(def, 'ds', 'public', 'customers', 0, 0).def;
    expect(def.tables.map((t) => t.alias).sort()).toEqual(['customers', 'customers_2']);
  });

  it('clears primaryKey when the primary changes', () => {
    const def = { ...build(['a', 'b']), primaryKey: 'id' };
    expect(removeTable(def, 'b').primaryKey).toBe('id');
    const r = removeTable(def, 'a');
    expect(r.primary).toBe('b');
    expect(r.primaryKey).toBeNull();
    expect(removeTable(r, 'b').primaryKey).toBeNull();
    expect(setPrimary(def, 'a')).toBe(def);
    expect(setPrimary(def, 'zzz')).toBe(def);
    const s = setPrimary(def, 'b');
    expect(s.primary).toBe('b');
    expect(s.primaryKey).toBeNull();
  });

  it('canConnect refuses unknown aliases', () => {
    const def = build(['a']);
    expect(canConnect(def, { alias: 'a', column: 'x' }, { alias: 'ghost', column: 'y' })).toBe('Unknown table: ghost');
    expect(canConnect(def, { alias: 'ghost', column: 'y' }, { alias: 'a', column: 'x' })).toBe('Unknown table: ghost');
  });
});

describe('draft tables', () => {
  const cols = [
    { name: 'id', type: 'integer' as const, nullable: false, primaryKey: true },
    { name: 'name', type: 'text' as const, nullable: true, primaryKey: false },
  ];

  it('a draft table may not take a view schema.name', () => {
    const view = { id: 'v1', name: 'sales', schema: 'public', sql: '', columns: [], dependsOnTables: [], dependsOnViews: [], x: 0, y: 0 };
    const def = { ...emptyDefinition(), views: [view] };
    expect(addDraftTable(def, 'ds', 'public', 'sales', cols, 0, 0).error).toBe('public.sales is already in this model');
    const ok = addDraftTable(def, 'ds', 'public', 'other', cols, 0, 0);
    expect(ok.error).toBeUndefined();
    expect(updateDraftTable(ok.def, 'other', 'public', 'sales', cols).error).toBe('public.sales is already in this model');
  });

  it('adds a draft table and makes the first one the primary', () => {
    const a = addDraftTable(emptyDefinition(), 'ds', 'public', 'users', cols, 0, 0);
    expect(a.error).toBeUndefined();
    expect(a.def.primary).toBe('users');
    expect(a.def.dataSource).toBe('ds');
    expect(drafts(a.def)).toHaveLength(1);
    const b = addDraftTable(a.def, 'ds', 'other', 'users', cols, 0, 0);
    expect(b.def.tables.map((t) => t.alias)).toEqual(['users', 'users_2']);
    expect(b.def.primary).toBe('users');
    expect(addDraftTable(a.def, 'x', 'public', 't', cols, 0, 0).error).toMatch(/one connection/);
  });

  it('refuses a draft with a bad name or duplicate columns', () => {
    const { def } = addDraftTable(emptyDefinition(), 'ds', 'public', 'users', cols, 0, 0);
    expect(draftProblem('public', '1bad', cols, def)).not.toBeNull();
    expect(draftProblem('public', 'a'.repeat(64), cols, def)).not.toBeNull();
    expect(draftProblem('public', 'ok', [], def)).not.toBeNull();
    expect(draftProblem('public', 'ok', [cols[0], cols[0]], def)).toMatch(/Duplicate/);
    expect(draftProblem('public', 'ok', [cols[0], { ...cols[0], name: 'ID' }], def)).toMatch(/Duplicate/);
    expect(draftProblem('public', 'ok', [{ ...cols[0], type: 'nope' as never }], def)).not.toBeNull();
    expect(draftProblem('public', 'users', cols, def)).toMatch(/already/);
    expect(draftProblem('public', 'users', cols, def, 'users')).toBeNull();
    expect(addDraftTable(def, 'ds', 'public', 'bad name', cols, 0, 0).error).toBeDefined();
  });

  it('editing a draft drops links to removed columns', () => {
    let { def } = addDraftTable(emptyDefinition(), 'ds', 'public', 'users', cols, 0, 0);
    def = addTable(def, 'ds', 'public', 'orders', 0, 0).def;
    def = connect(def, { alias: 'orders', column: 'user_id' }, { alias: 'users', column: 'name' }, 'many_to_one', 'left');
    def = { ...def, primaryKey: 'name' };
    const r = updateDraftTable(def, 'users', 'public', 'users', [cols[0]]);
    expect(r.error).toBeUndefined();
    expect(r.def.joins).toEqual([]);
    expect(r.def.primaryKey).toBeNull();
    expect(r.def.tables[0].alias).toBe('users');
    expect(r.def.tables[0].draft?.columns).toHaveLength(1);
  });

  it('refuses to edit a table that is not a draft', () => {
    let { def } = addDraftTable(emptyDefinition(), 'ds', 'public', 'users', cols, 0, 0);
    def = addTable(def, 'ds', 'public', 'orders', 0, 0).def;
    const r = updateDraftTable(def, 'orders', 'public', 'orders', cols);
    expect(r.error).toMatch(/Not a draft/);
    expect(r.def).toBe(def);
    expect(updateDraftTable(def, 'nope', 'public', 'x', cols).error).toBeDefined();
  });

  it('colors a table and clears its color', () => {
    let def = build(['a']);
    def = setColor(def, 'a', 'teal');
    expect(def.tables[0].color).toBe('teal');
    def = setColor(def, 'a', undefined);
    expect('color' in def.tables[0]).toBe(false);
  });

  it('lays tables out in layers from the principal', () => {
    let def = build(['a', 'b', 'c', 'd']);
    def = connect(def, { alias: 'a', column: 'i' }, { alias: 'b', column: 'i' }, 'one_to_many', 'left');
    def = connect(def, { alias: 'c', column: 'i' }, { alias: 'a', column: 'i' }, 'many_to_one', 'left');
    const l = layout(def);
    const pos = (a: string) => {
      const t = l.tables.find((x) => x.alias === a)!;

      return [t.x, t.y];
    };
    expect(pos('a')).toEqual([0, 0]);
    expect(pos('b')).toEqual([320, 0]);
    expect(pos('c')).toEqual([320, 44 + 24 * 6 + 40]);
    expect(pos('d')).toEqual([640, 0]);
  });

  describe('v2 helpers', () => {
    const linked = () => connect(build(['orders', 'customers']), { alias: 'orders', column: 'customer_id' }, { alias: 'customers', column: 'id' }, 'many_to_one', 'left');

    it('renameTable moves primary and join ends, refuses bad and taken names', () => {
      const def = linked();
      const { def: next, error } = renameTable(def, 'orders', 'sales');
      expect(error).toBeUndefined();
      expect(next.primary).toBe('sales');
      expect(next.tables.map((t) => t.alias)).toEqual(['sales', 'customers']);
      expect(next.joins[0].from).toEqual({ alias: 'sales', column: 'customer_id' });
      expect(renameTable(def, 'orders', '9x').error).toBe('Invalid name: 9x');
      expect(renameTable(def, 'orders', 'customers').error).toBe('customers is already in this model');
      expect(renameTable(def, 'orders', 'orders').def).toBe(def);
    });

    it('setTableComment sets and clears the comment', () => {
      const def = setTableComment(linked(), 'orders', 'every order');
      expect(def.tables[0].comment).toBe('every order');
      expect('comment' in setTableComment(def, 'orders', '').tables[0]).toBe(false);
    });

    it('placeTables moves tables and shortcuts, sets and clears sizes', () => {
      let def = linked();
      def = { ...def, tables: def.tables.map((t) => (t.alias === 'orders' ? { ...t, shortcuts: [{ id: 's1', x: 0, y: 0 }] } : t)) };
      def = placeTables(def, new Map([
        ['orders', { x: 10.4, y: 20.6, width: 200, height: 120 }],
        ['orders~s1', { x: 300, y: 5 }],
      ]));
      expect(def.tables[0]).toMatchObject({ x: 10, y: 21, width: 200, height: 120, shortcuts: [{ id: 's1', x: 300, y: 5 }] });
      def = placeTables(def, new Map([['orders', { width: null, height: null }]]));
      expect('width' in def.tables[0]).toBe(false);
      expect('height' in def.tables[0]).toBe(false);
      expect(def.tables[0].x).toBe(10);
    });

    it('setJoin patches one join; showAllJoins clears hidden', () => {
      let def = linked();
      def = setJoin(def, 0, { points: [{ x: 1, y: 2 }], swapped: true, hidden: true, join: 'inner' });
      expect(def.joins[0]).toMatchObject({ points: [{ x: 1, y: 2 }], swapped: true, hidden: true, join: 'inner', relationship: 'many_to_one' });
      def = showAllJoins(def);
      expect('hidden' in def.joins[0]).toBe(false);
    });

    it('withFormat marks the definition as version 2', () => {
      expect(withFormat(linked()).formatVersion).toBe(2);
    });
  });
});
