import { describe, expect, it } from 'vitest';

import { fileListRows, tableListRows } from '../pages/Schema/OrgLists';

const col = { name: 'id', type: 'text' };

describe('tableListRows', () => {
  it('lists every table by data source, schema and name, keyed like the tree', () => {
    const rows = tableListRows('bossa', {
      sfdb: { dfos: { fibercable: [col, col], segment: [col] } },
      analytics: { public: { orders: [] } },
    });
    expect(rows.map((r) => r.key)).toEqual([
      'table:bossa:analytics.public.orders',
      'table:bossa:sfdb.dfos.fibercable',
      'table:bossa:sfdb.dfos.segment',
    ]);
    expect(rows[1]).toMatchObject({ dataSource: 'sfdb', schema: 'dfos', table: 'fibercable', columns: 2 });
  });

  it('is empty without tables', () => {
    expect(tableListRows('bossa', undefined as any)).toEqual([]);
  });
});

describe('fileListRows', () => {
  const base = { dataSource: 'd', members: { measures: 0, dimensions: 0 } };

  it('names the cubes of each file, its problems and whether it is read-only', () => {
    const rows = fileListRows(
      'bossa',
      [
        { fileName: 'bossa/b.yml', version: 3, editable: false },
        { fileName: 'bossa/a.yml', version: 1 },
      ],
      [
        { ...base, name: 'x', type: 'cube', file: 'bossa/a.yml' },
        { ...base, name: 'v', type: 'view', file: 'bossa/a.yml' },
        { ...base, name: 'y', type: 'cube', file: 'bossa/b.yml' },
      ],
      [{ file: 'bossa/b.yml', cube: '', message: 'broken' }]
    );
    expect(rows).toEqual([
      { key: 'file:bossa:bossa/a.yml', fileName: 'bossa/a.yml', shown: 'a.yml', cubes: ['x', 'v'], version: 1, readOnly: false, errors: 0 },
      { key: 'file:bossa:bossa/b.yml', fileName: 'bossa/b.yml', shown: 'b.yml', cubes: ['y'], version: 3, readOnly: true, errors: 1 },
    ]);
  });
});
