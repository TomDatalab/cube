import { describe, expect, it } from 'vitest';

import {
  addShortcut, copy, cut, parseNodeId, paste, removeShortcut, shortcutNodeId,
} from '../pages/Schema/models/clipboard';
import { addView, nodeIdOf, setView } from '../pages/Schema/models/objects';
import {
  ModelDefinition, addDraftTable, addTable, connect, emptyDefinition, removeTable,
} from '../pages/Schema/models/modelGraph';

function model(): ModelDefinition {
  let def = emptyDefinition();
  def = addTable(def, 'ds', 'public', 'orders', 10, 10).def;
  def = addTable(def, 'ds', 'public', 'customers', 300, 10).def;
  def = addTable(def, 'ds', 'public', 'items', 600, 10).def;
  def = connect(def, { alias: 'orders', column: 'customer_id' }, { alias: 'customers', column: 'id' }, 'many_to_one', 'left');
  def = connect(def, { alias: 'items', column: 'order_id' }, { alias: 'orders', column: 'id' }, 'many_to_one', 'left');
  return def;
}

describe('clipboard', () => {
  it('copies the selected tables and only the joins between them', () => {
    const clip = copy(model(), ['orders', 'customers']);
    expect(clip.tables.map((t) => t.alias)).toEqual(['orders', 'customers']);
    expect(clip.joins).toHaveLength(1);
    expect(clip.joins[0].to.alias).toBe('customers');
  });

  it('copies a shortcut node as its table', () => {
    let def = model();
    def = addShortcut(def, 'orders', 50, 50).def;
    expect(copy(def, [shortcutNodeId('orders', 's1')]).tables.map((t) => t.alias)).toEqual(['orders']);
  });

  it('pastes with _1 names, 20px offset and remapped joins', () => {
    const def = model();
    const clip = copy(def, ['orders', 'customers']);
    const first = paste(def, clip);
    expect(first.nodeIds).toEqual(['orders_1', 'customers_1']);
    const pasted = first.def.tables.find((t) => t.alias === 'orders_1')!;
    expect([pasted.x, pasted.y]).toEqual([30, 30]);
    expect(pasted.table).toBe('orders');
    const join = first.def.joins[first.def.joins.length - 1];
    expect([join.from.alias, join.to.alias]).toEqual(['orders_1', 'customers_1']);
    const second = paste(first.def, clip);
    expect(second.nodeIds).toEqual(['orders_2', 'customers_2']);
  });

  it('a cut table pastes back under its own name', () => {
    const { def, clip } = cut(model(), ['orders']);
    const { def: next, nodeIds: aliases } = paste(def, clip);
    expect(aliases).toEqual(['orders']);
    expect(next.tables.find((t) => t.alias === 'orders')!.x).toBe(30);
  });

  it('pastes a draft under a free table name', () => {
    let def = emptyDefinition();
    def = addDraftTable(def, 'ds', 'public', 'items', [
      { name: 'id', type: 'integer', nullable: false, primaryKey: true },
    ], 0, 0).def;
    const { def: next } = paste(def, copy(def, ['items']));
    const pasted = next.tables[1];
    expect(pasted.alias).toBe('items_1');
    expect(pasted.table).toBe('items_1');
    expect(pasted.draft).toEqual(def.tables[0].draft);
  });

  it('does not copy shortcuts with a pasted table', () => {
    let def = model();
    def = addShortcut(def, 'orders', 50, 50).def;
    const { def: next } = paste(def, copy(def, ['orders']));
    expect(next.tables.find((t) => t.alias === 'orders_1')!.shortcuts).toBeUndefined();
  });

  it('cut removes the tables and returns the clip', () => {
    const { def, clip } = cut(model(), ['orders']);
    expect(clip.tables.map((t) => t.alias)).toEqual(['orders']);
    expect(def.tables.map((t) => t.alias)).toEqual(['customers', 'items']);
    expect(def.joins).toHaveLength(0);
  });

  it('cut of a shortcut node removes only that shortcut', () => {
    let def = model();
    def = addShortcut(def, 'orders', 50, 50).def;
    const result = cut(def, [shortcutNodeId('orders', 's1')]);
    expect(result.def.tables.map((t) => t.alias)).toContain('orders');
    expect(result.def.tables[0].shortcuts).toBeUndefined();
  });

  it('a shortcut gets the first free id, and removing it keeps the table', () => {
    let def = model();
    const a = addShortcut(def, 'orders', 50, 60);
    expect(a.id).toBe('s1');
    def = addShortcut(a.def, 'customers', 70, 80).def;
    const c = addShortcut(def, 'orders', 90, 100);
    expect(c.id).toBe('s3');
    expect(c.def.tables[0].shortcuts).toEqual([{ id: 's1', x: 50, y: 60 }, { id: 's3', x: 90, y: 100 }]);
    def = removeShortcut(c.def, 'orders', 's1');
    expect(def.tables[0].shortcuts).toEqual([{ id: 's3', x: 90, y: 100 }]);
    expect(addShortcut(def, 'items', 0, 0).id).toBe('s1');
    def = removeShortcut(def, 'orders', 's3');
    expect('shortcuts' in def.tables[0]).toBe(false);
  });

  it('removing a table removes its shortcuts', () => {
    let def = model();
    def = addShortcut(def, 'orders', 50, 50).def;
    def = removeTable(def, 'orders');
    expect(def.tables.flatMap((t) => t.shortcuts ?? [])).toEqual([]);
  });

  it('parses node ids', () => {
    expect(parseNodeId('orders')).toEqual({ alias: 'orders' });
    expect(parseNodeId(shortcutNodeId('orders', 's2'))).toEqual({ alias: 'orders', shortcut: 's2' });
  });

  it('pasted references get fresh ids and no name', () => {
    let def = model();
    def = { ...def, joins: def.joins.map((j, i) => ({ ...j, id: `r${i + 1}`, name: `fk_${i}` })) };
    const { def: next } = paste(def, copy(def, ['orders', 'customers']));
    const ids = next.joins.map((j) => j.id);
    expect(new Set(ids).size).toBe(ids.length);
    expect(next.joins[next.joins.length - 1].name).toBeUndefined();
  });

  it('copies and pastes a view with the table it depends on', () => {
    let def = model();
    const { def: withView, id } = addView(def, 100, 200, 'public');
    def = setView(withView, id, { dependsOnTables: ['orders'] });
    const clip = copy(def, ['orders', nodeIdOf('view', id)]);
    const { def: next, nodeIds } = paste(def, clip);
    expect(nodeIds).toEqual(['orders_1', 'view:v2']);
    const pasted = next.views!.find((v) => v.id === 'v2')!;
    expect(pasted.name).toBe('view_1_1');
    expect(pasted.dependsOnTables).toEqual(['orders_1']);
    expect([pasted.x, pasted.y]).toEqual([120, 220]);
  });

  it('pasting a view whose table was deleted keeps no dependency on it', () => {
    let { def } = addView(model(), 0, 0, 'public');
    def = setView(def, 'v1', { dependsOnTables: ['orders'] });
    const clip = copy(def, [nodeIdOf('view', 'v1')]);
    const { def: next } = paste(removeTable(def, 'orders'), clip);
    expect(next.views!.find((v) => v.id === 'v2')!.dependsOnTables).toEqual([]);
  });

  it('pasting a view alone drops view dependencies on views not pasted', () => {
    let { def } = addView(model(), 0, 0, 'public');
    def = addView(def, 50, 50, 'public').def;
    def = setView(def, 'v2', { dependsOnViews: ['v1'] });
    const { def: next } = paste(def, copy(def, [nodeIdOf('view', 'v2')]));
    expect(next.views!.find((v) => v.id === 'v3')!.dependsOnViews).toEqual([]);
    const both = paste(def, copy(def, [nodeIdOf('view', 'v1'), nodeIdOf('view', 'v2')]));
    expect(both.def.views!.find((v) => v.id === 'v4')!.dependsOnViews).toEqual(['v3']);
  });

  it('pasting a view frees its name against draft tables', () => {
    const cols = [{ name: 'id', type: 'integer' as const, nullable: false, primaryKey: true }];
    let { def } = addView(model(), 0, 0, 'public');
    def = addDraftTable(def, 'ds', 'public', 'view_1_1', cols, 0, 0).def;
    const { def: next } = paste(def, copy(def, ['view:v1']));
    expect(next.views![1].name).toBe('view_1_2');
  });
});
