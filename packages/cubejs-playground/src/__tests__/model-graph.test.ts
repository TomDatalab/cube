import { describe, expect, it } from 'vitest';

import {
  addTable, canConnect, connect, emptyDefinition, joinLabel, removeJoin, removeTable, setPrimary, unlinked,
  updateJoin,
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
