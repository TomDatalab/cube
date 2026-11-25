import { describe, expect, it } from 'vitest';

import {
  buildQueryUrl, detailNodes, filterTree, iconKind, loadedTreeKeys, orgNodes, parseKey,
  tablesFromSameOrg,
} from '../pages/Schema/tree';

const detail = {
  cubes: [
    {
      name: 'dfos_fibercable',
      type: 'cube' as const,
      dataSource: 'default',
      file: 'digital/a.yml',
      members: { measures: 1, dimensions: 2 },
    },
  ],
  tables: { default: { public: { t1: [{ name: 'id', type: 'int' }], t2: [] } } },
  files: [{ fileName: 'digital/sofiadb/a.yml' }, { fileName: 'b.yml' }],
  errors: [],
};

describe('organization tree helpers', () => {
  it('detailNodes returns Cubes, Views, Tables and Files with counts', () => {
    const nodes = detailNodes('digital', detail);
    expect(nodes.map((n) => n.title)).toEqual(['Cubes (1)', 'Views (0)', 'Tables (2)', 'Files (2)']);
  });

  it('detailNodes puts views under Views and cubes under Cubes', () => {
    const view = {
      name: 'network',
      type: 'view' as const,
      dataSource: 'default',
      file: 'digital/v.yml',
      members: { measures: 1, dimensions: 1 },
    };
    const nodes = detailNodes('digital', { ...detail, cubes: [...detail.cubes, view] });
    expect(nodes[0].children!.map((c) => c.key)).toEqual(['cube:digital:dfos_fibercable']);
    expect(nodes[1].key).toBe('org:digital:views');
    expect(nodes[1].children!.map((c) => c.key)).toEqual(['cube:digital:network']);
  });

  it('filterTree keeps matches and their ancestors', () => {
    const org = {
      ...orgNodes([{
        slug: 'digital', name: 'Digital', counts: { cubes: 1, files: 2, connections: 1 }, errors: 0,
      }])[0],
      children: detailNodes('digital', detail),
    };
    const out = filterTree([org], 'FIB');
    expect(out).toHaveLength(1);
    expect(out[0].key).toBe('org:digital');
    expect(out[0].children).toHaveLength(1);
    expect(out[0].children![0].key).toBe('org:digital:cubes');
    expect(out[0].children![0].children!.map((c) => c.key)).toEqual(['cube:digital:dfos_fibercable']);
    expect(filterTree([org], '  ')).toEqual([org]);
  });

  it('filterTree matches titles and names, never the kind or the slug of a key', () => {
    const org = {
      ...orgNodes([{
        slug: 'digital', name: 'Digital', counts: { cubes: 1, files: 2, connections: 1 }, errors: 0,
      }])[0],
      children: detailNodes('digital', detail),
    };
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
    expect(parseKey('org:digital:views')).toEqual({ kind: 'views', slug: 'digital', rest: undefined });
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

  it('buildQueryUrl skips hidden members', () => {
    const url = buildQueryUrl({
      name: 'c',
      measures: [{ name: 'c.secret', public: false }],
      dimensions: [{ name: 'c.id', primaryKey: true }, { name: 'c.name' }],
    });
    expect(JSON.parse(decodeURIComponent(url.split('query=')[1]))).toEqual({ dimensions: ['c.name'] });
  });

  it('tablesFromSameOrg returns the common slug or null', () => {
    expect(tablesFromSameOrg(['table:on:x.s.t', 'table:digital:y.s.t'])).toBeNull();
    expect(tablesFromSameOrg(['table:on:x.s.t', 'table:on:y.s.t'])).toBe('on');
  });
});

describe('iconKind', () => {
  it('gives the Cubes group and every cube the cube icon', () => {
    expect(iconKind('org:digital:cubes')).toBe('cube');
    expect(iconKind('org:digital:views')).toBe('cube');
    expect(iconKind('cube:digital:fatos')).toBe('cube');
  });

  it('gives the Tables group and every table the table icon', () => {
    expect(iconKind('tables:digital')).toBe('table');
    expect(iconKind('table:digital:sofiadb.dfos.fibercable')).toBe('table');
  });

  it('gives the Files group and every file the file icon', () => {
    expect(iconKind('files:digital')).toBe('file');
    expect(iconKind('file:digital:sofiadb/dfos_a.yml')).toBe('file');
  });

  it('gives the Models group and every model the model icon', () => {
    expect(iconKind('org:acme:models')).toBe('model');
    expect(iconKind('model:acme:m1')).toBe('model');
  });

  it('leaves organizations, connections, schemas and unknown keys bare', () => {
    expect(iconKind('org:digital')).toBeNull();
    expect(iconKind('datasource:digital:sofiadb')).toBeNull();
    expect(iconKind('schema:digital:sofiadb.dfos')).toBeNull();
    expect(iconKind('__more__')).toBeNull();
  });
});

describe('file titles', () => {
  it('drop the organization folder they all share, keeping the full path in the key', () => {
    const files = detailNodes('digital', detail)[3].children!;
    expect(files.map((n) => n.title)).toEqual(['sofiadb/a.yml', 'b.yml']);
    expect(files.map((n) => n.key)).toEqual(['file:digital:digital/sofiadb/a.yml', 'file:digital:b.yml']);
  });

  it('drop the Platform folder too', () => {
    const files = detailNodes('_platform', { ...detail, files: [{ fileName: '_platform/orders.yml' }] })[3].children!;
    expect(files.map((n) => n.title)).toEqual(['orders.yml']);
  });
});

describe('loadedTreeKeys', () => {
  // The Tree is given loadData for the organizations. A node it is not told is
  // loaded and that has no children (an empty Cubes or Files group) would be
  // "loaded" again on every render when expanded, freezing the page.
  it('marks every non-organization node as loaded, and only the loaded organizations', () => {
    const empty = { cubes: [], tables: {}, files: [], errors: [] };
    const summary = (slug: string) => ({
      slug, name: slug, counts: { cubes: 0, files: 0, connections: 0 }, errors: 0,
    });
    const [on, bossa] = orgNodes([summary('on'), summary('bossa')]);
    const tree = [{ ...on, children: detailNodes('on', empty) }, bossa];
    const keys = loadedTreeKeys(tree, ['on']);
    expect(keys).toEqual(expect.arrayContaining(['org:on', 'org:on:cubes', 'org:on:views', 'tables:on', 'files:on']));
    expect(keys).not.toContain('org:bossa');
  });

  it('includes nested nodes', () => {
    const [on] = orgNodes([{
      slug: 'on', name: 'on', counts: { cubes: 1, files: 2, connections: 1 }, errors: 0,
    }]);
    const keys = loadedTreeKeys([{ ...on, children: detailNodes('on', detail) }], ['on']);
    expect(keys.some((k) => k.startsWith('schema:'))).toBe(true);
    expect(keys.some((k) => k.startsWith('file:'))).toBe(true);
  });
});

describe('models in the tree', () => {
  it('detailNodes adds a Models group directly after Cubes with a leaf per model', () => {
    const nodes = detailNodes('acme', { ...detail, models: [{ id: 'm1', name: 'Sales' }] });
    expect(nodes[1].key).toBe('org:acme:models');
    expect(nodes[1].title).toBe('Models (1)');
    expect(nodes[1].children!.map((c) => [c.key, c.title])).toEqual([['model:acme:m1', 'Sales']]);
  });

  it('detailNodes has no Models group without `models`', () => {
    expect(detailNodes('acme', detail).map((n) => n.key)).not.toContain('org:acme:models');
  });

  it('detailNodes shows Models (0) for an organization with `models: []`', () => {
    expect(detailNodes('acme', { ...detail, models: [] })[1].title).toBe('Models (0)');
  });

  it('parseKey reads model and models keys', () => {
    expect(parseKey('model:acme:m1')).toEqual({ kind: 'model', slug: 'acme', rest: 'm1' });
    expect(parseKey('org:acme:models')).toEqual({ kind: 'models', slug: 'acme', rest: undefined });
  });
});
