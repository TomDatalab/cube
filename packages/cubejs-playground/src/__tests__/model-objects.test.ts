import { describe, expect, it } from 'vitest';

import {
  addArea, addNote, addView, bringToFront, insideArea, nodeIdOf, nodeIdsOf, objectOf, placeObjects, previewIdOf,
  removeObjects, renameView, sendToBack, setNote, setView, setViewColumnType, setViewSchema,
} from '../pages/Schema/models/objects';
import { addShortcut } from '../pages/Schema/models/clipboard';
import {
  ModelDefinition, addDraftTable, addTable, emptyDefinition, removeTable, renameTable,
} from '../pages/Schema/models/modelGraph';

function base(): ModelDefinition {
  let def = emptyDefinition();
  def = addTable(def, 'ds', 'public', 'orders', 10, 10).def;
  def = addTable(def, 'ds', 'public', 'customers', 300, 10).def;
  return def;
}

describe('model objects', () => {
  it('objectOf parses every kind of node id', () => {
    expect(objectOf('note:n1')).toEqual({ kind: 'note', id: 'n1' });
    expect(objectOf('area:a2')).toEqual({ kind: 'area', id: 'a2' });
    expect(objectOf('view:v3')).toEqual({ kind: 'view', id: 'v3' });
    expect(objectOf('orders')).toEqual({ kind: 'table', id: 'orders' });
    expect(objectOf('orders~s1')).toEqual({ kind: 'table', id: 'orders', shortcut: 's1' });
    expect(nodeIdOf('note', 'n1')).toBe('note:n1');
  });

  it('adds notes, areas and views with the smallest free id', () => {
    let def = base();
    def = addNote(def, 1, 2).def;
    def = addNote(def, 1, 2).def;
    def = { ...def, notes: def.notes!.filter((n) => n.id !== 'n1') };
    const note = addNote(def, 5, 6);
    expect(note.id).toBe('n1');
    expect(note.def.notes!.find((n) => n.id === 'n1')).toMatchObject({ width: 200, height: 100, text: '' });
    const area = addArea(base(), 0, 0);
    expect(area.def.areas![0]).toMatchObject({ id: 'a1', name: 'Area_1', width: 400, height: 300 });
    const v1 = addView(base(), 0, 0, 'public');
    const v2 = addView(v1.def, 0, 0, 'public');
    expect(v2.def.views!.map((v) => [v.id, v.name])).toEqual([['v1', 'view_1'], ['v2', 'view_2']]);
    expect(v2.def.views![0].width).toBeUndefined();
  });

  it('setNote with undefined removes the key; renameView validates', () => {
    let { def } = addNote(base(), 0, 0);
    def = setNote(def, 'n1', { color: 'blue', width: undefined });
    expect(def.notes![0].color).toBe('blue');
    expect('width' in def.notes![0]).toBe(false);
    def = addView(addView(def, 0, 0, 'public').def, 0, 0, 'public').def;
    expect(renameView(def, 'v1', '1x').error).toBe('Invalid name: 1x');
    expect(renameView(def, 'v1', 'view_2').error).toBe('view_2 is already in this model');
    expect(renameView(def, 'v1', 'sales').def.views![0].name).toBe('sales');
  });

  it('renaming a table renames it in view dependencies', () => {
    let { def } = addView(base(), 0, 0, 'public');
    def = setView(def, 'v1', { dependsOnTables: ['orders', 'customers'] });
    const next = renameTable(def, 'orders', 'o').def;
    expect(next.views![0].dependsOnTables).toEqual(['o', 'customers']);
  });

  it('removing a table or view drops it from view dependencies', () => {
    let { def } = addView(addView(base(), 0, 0, 'public').def, 0, 0, 'public');
    def = setView(def, 'v1', { dependsOnTables: ['orders'] });
    def = setView(def, 'v2', { dependsOnTables: ['orders', 'customers'], dependsOnViews: ['v1'] });
    expect(removeTable(def, 'orders').views!.map((v) => v.dependsOnTables)).toEqual([[], ['customers']]);
    const next = removeObjects(def, ['view:v1', 'view:zz', 'note:n9', 'ghost']);
    expect(next.views!.map((v) => v.id)).toEqual(['v2']);
    expect(next.views![0].dependsOnViews).toEqual([]);
    expect(next.tables).toHaveLength(2);
  });

  it('placeObjects moves tables, shortcuts, notes, areas and views in one call', () => {
    let { def } = addShortcut(base(), 'orders', 0, 0);
    def = addNote(def, 0, 0).def;
    def = addArea(def, 0, 0).def;
    def = addView(def, 0, 0, 'public').def;
    const next = placeObjects(def, new Map([
      ['customers', { x: 1.4, y: 2 }],
      ['orders~s1', { x: 7, y: 8 }],
      ['note:n1', { x: 3, y: 4 }],
      ['area:a1', { x: 5, y: 6, width: 500 }],
      ['view:v1', { x: 9, y: 10 }],
    ]));
    expect([next.tables[1].x, next.tables[1].y]).toEqual([1, 2]);
    expect(next.tables[0].shortcuts![0]).toMatchObject({ x: 7, y: 8 });
    expect(next.notes![0]).toMatchObject({ x: 3, y: 4 });
    expect(next.areas![0]).toMatchObject({ x: 5, y: 6, width: 500, height: 300 });
    expect(next.views![0]).toMatchObject({ x: 9, y: 10 });
  });

  it('a null size frees a note but keeps an area\'s size', () => {
    let { def } = addArea(addNote(base(), 0, 0).def, 0, 0);
    def = placeObjects(def, new Map([
      ['note:n1', { width: null, height: null }],
      ['area:a1', { width: null, height: null }],
    ]));
    expect('width' in def.notes![0]).toBe(false);
    expect('height' in def.notes![0]).toBe(false);
    expect(def.areas![0]).toMatchObject({ width: 400, height: 300 });
  });

  it('bring to front and send to back reorder only notes and areas', () => {
    let def = base();
    for (let i = 0; i < 3; i++) def = addNote(def, 0, 0).def;
    def = addArea(def, 0, 0).def;
    def = addView(def, 0, 0, 'public').def;
    const front = bringToFront(def, ['note:n1', 'note:n2', 'view:v1', 'orders']);
    expect(front.notes!.map((n) => n.z ?? 0)).toEqual([1, 2, 0]);
    expect(front.views).toEqual(def.views);
    expect(front.tables).toEqual(def.tables);
    const back = sendToBack(front, ['note:n3', 'area:a1']);
    expect(back.notes!.map((n) => n.z)).toEqual([1, 2, -1]);
    expect(back.areas![0].z).toBe(-1);
    const b2 = sendToBack(front, ['note:n1', 'note:n2']);
    expect(b2.notes!.map((n) => n.z ?? 0)).toEqual([-2, -1, 0]);
    expect(b2.notes![2].z).toBeUndefined();
  });

  it('insideArea takes only boxes entirely inside', () => {
    const area = { id: 'area:a1', x: 0, y: 0, width: 100, height: 100 };
    const ids = insideArea(area, [
      area,
      { id: 'in', x: 10, y: 10, width: 20, height: 20 },
      { id: 'edge', x: 80, y: 80, width: 20, height: 20 },
      { id: 'partly', x: 90, y: 10, width: 20, height: 20 },
      { id: 'out', x: 200, y: 0, width: 5, height: 5 },
    ]);
    expect(ids).toEqual(['in', 'edge']);
  });

  it('nodeIdsOf lists every canvas node', () => {
    let { def } = addShortcut(base(), 'orders', 0, 0);
    def = addView(addArea(addNote(def, 0, 0).def, 0, 0).def, 0, 0, 'public').def;
    expect(nodeIdsOf(def)).toEqual(['orders', 'orders~s1', 'customers', 'note:n1', 'area:a1', 'view:v1']);
  });

  it('view names avoid draft tables of the same schema', () => {
    const cols = [{ name: 'id', type: 'integer' as const, nullable: false, primaryKey: true }];
    let { def } = addDraftTable(base(), 'ds', 'public', 'view_1', cols, 0, 0);
    expect(addView(def, 0, 0, 'public').def.views![0].name).toBe('view_2');
    expect(addView(def, 0, 0, 'other').def.views![0].name).toBe('view_1');
    def = addView(def, 0, 0, 'public').def;
    expect(renameView(def, 'v1', 'view_1').error).toBe('view_1 is already in this model');
  });

  it('previewIdOf sends views as node ids and tables as aliases', () => {
    expect(previewIdOf('view:v1')).toBe('view:v1');
    expect(previewIdOf('v1')).toBe('v1');
    expect(previewIdOf('orders')).toBe('orders');
  });

  it('setViewSchema validates like renameView', () => {
    let { def } = addView(addView(base(), 0, 0, 'public').def, 0, 0, 'public');
    expect(setViewSchema(def, 'v1', '1x').error).toBe('Invalid name: 1x');
    expect(setViewSchema(def, 'v1', 'a'.repeat(64)).error).toMatch(/^Invalid name/);
    def = addDraftTable(def, 'ds', 'sales', 'view_2', [{ name: 'id', type: 'integer', nullable: false, primaryKey: true }], 0, 0).def;
    expect(setViewSchema(def, 'v2', 'sales').error).toBe('view_2 is already in this model');
    def = setView(def, 'v2', { schema: 'other' });
    expect(setViewSchema(def, 'v2', 'fine').def.views![1].schema).toBe('fine');
    expect(setViewSchema(def, 'v2', 'other').def).toBe(def);
  });

  it('setViewColumnType refuses a type the dialect does not know', () => {
    let { def } = addView(base(), 0, 0, 'public');
    def = setView(def, 'v1', { columns: [{ name: 'a', type: 'text' }] });
    expect(setViewColumnType(def, 'v1', 0, 'jsonb', null).error).toBe('Invalid type: jsonb');
    expect(setViewColumnType(def, 'v1', 0, 'jsonb', 'postgres').def.views![0].columns[0].type).toBe('jsonb');
    expect(setViewColumnType(def, 'v1', 0, 'jsonb', 'mysql').error).toBe('Invalid type: jsonb');
    expect(setViewColumnType(def, 'v1', 0, 'bigint', null).def.views![0].columns[0].type).toBe('bigint');
  });
});
