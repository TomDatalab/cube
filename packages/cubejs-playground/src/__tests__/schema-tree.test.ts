import { describe, expect, it } from 'vitest';

import {
  buildQueryUrl, detailNodes, filterTree, orgNodes, parseKey, tablesFromSameOrg,
} from '../pages/Schema/tree';

const detail = {
  cubes: [
    {
      name: 'dfos_fibercable', type: 'cube' as const, dataSource: 'default', file: 'digital/a.yml',
      members: { measures: 1, dimensions: 2 },
    },
  ],
  tables: { default: { public: { t1: [{ name: 'id', type: 'int' }], t2: [] } } },
  files: [{ fileName: 'digital/sofiadb/a.yml' }, { fileName: 'b.yml' }],
  errors: [],
};

describe('organization tree helpers', () => {
  it('detailNodes returns Cubes, Tables and Files with counts', () => {
    const nodes = detailNodes('digital', detail);
    expect(nodes.map((n) => n.title)).toEqual(['Cubes (1)', 'Tables (2)', 'Files (2)']);
  });

  it('filterTree keeps matches and their ancestors', () => {
    const org = { ...orgNodes([{
      slug: 'digital', name: 'Digital', counts: { cubes: 1, files: 2, connections: 1 }, errors: 0,
    }])[0], children: detailNodes('digital', detail) };
    const out = filterTree([org], 'FIB');
    expect(out).toHaveLength(1);
    expect(out[0].key).toBe('org:digital');
    expect(out[0].children).toHaveLength(1);
    expect(out[0].children![0].key).toBe('org:digital:cubes');
    expect(out[0].children![0].children!.map((c) => c.key)).toEqual(['cube:digital:dfos_fibercable']);
    expect(filterTree([org], '  ')).toEqual([org]);
  });

  it('filterTree matches titles and names, never the kind or the slug of a key', () => {
    const org = { ...orgNodes([{
      slug: 'digital', name: 'Digital', counts: { cubes: 1, files: 2, connections: 1 }, errors: 0,
    }])[0], children: detailNodes('digital', detail) };
    // Kinds of keys: no title holds them.
    expect(filterTree([org], 'datasource')).toEqual([]);
    expect(filterTree([org], 'schema:')).toEqual([]);
    // The slug is in every key; only what names it matches (a file in its folder).
    const out = filterTree([org], 'digital');
    expect(out).toHaveLength(1);
    expect(out[0].children!.map((c) => c.key)).toEqual(['files:digital']);
    expect(out[0].children![0].children!.map((c) => c.key))
      .toEqual(['file:digital:digital/sofiadb/a.yml']);
    // The part of the key after the slug still matches: `ds.schema.table`.
    const tables = filterTree([org], 'public.t1');
    expect(tables[0].children!.map((c) => c.key)).toEqual(['tables:digital']);
  });

  it('parseKey splits on the first two colons only', () => {
    expect(parseKey('file:digital:digital/sofiadb/a.yml'))
      .toEqual({ kind: 'file', slug: 'digital', rest: 'digital/sofiadb/a.yml' });
    expect(parseKey('org:digital')).toEqual({ kind: 'org', slug: 'digital', rest: undefined });
    expect(parseKey('org:digital:cubes').kind).toBe('cubes');
    expect(parseKey('datasource:digital:sofiadb'))
      .toEqual({ kind: 'datasource', slug: 'digital', rest: 'sofiadb' });
    expect(parseKey('schema:digital:sofiadb.dfos'))
      .toEqual({ kind: 'schema', slug: 'digital', rest: 'sofiadb.dfos' });
    expect(() => parseKey('bogus:x:y')).toThrow();
  });

  it('buildQueryUrl uses all measures or the first non-primary dimension', () => {
    expect(buildQueryUrl({
      name: 'c', measures: [{ name: 'c.a' }, { name: 'c.b' }], dimensions: [],
    })).toBe(`/build?query=${encodeURIComponent(JSON.stringify({ measures: ['c.a', 'c.b'] }))}`);
    expect(buildQueryUrl({
      name: 'c', measures: [], dimensions: [{ name: 'c.id', primaryKey: true }, { name: 'c.x' }],
    })).toBe(`/build?query=${encodeURIComponent(JSON.stringify({ dimensions: ['c.x'] }))}`);
  });

  it('tablesFromSameOrg returns the common slug or null', () => {
    expect(tablesFromSameOrg(['table:on:x.s.t', 'table:digital:y.s.t'])).toBeNull();
    expect(tablesFromSameOrg(['table:on:x.s.t', 'table:on:y.s.t'])).toBe('on');
  });
});
