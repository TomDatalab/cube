import { describe, expect, it, vi } from 'vitest';

import {
  cubeCountByDataSource, fetchMetaGroups, MetaGroup, sectionsOf, worthGrouping,
} from '../shared/meta-groups';

const groups: MetaGroup[] = [
  {
    slug: '_platform',
    id: null,
    name: 'Platform',
    errors: 0,
    dataSources: [{ name: 'default', cubes: ['users', 'orders'] }],
  },
  {
    slug: 'digital',
    id: 'o1',
    name: 'Digital',
    errors: 2,
    dataSources: [
      { name: 'example', cubes: ['fatos'] },
      { name: 'sofiadb', cubes: ['eventos', 'pos_falha'] },
    ],
  },
  { slug: 'on', id: 'o2', name: 'ON', errors: 1, dataSources: [] },
];

const items = (...names: string[]) => names.map((name) => ({ name }));

describe('sectionsOf', () => {
  it('splits by organization and data source, keeping the item order', () => {
    const sections = sectionsOf(items('pos_falha', 'orders', 'eventos', 'users', 'fatos'), groups);
    expect(sections.map((s) => s.slug)).toEqual(['_platform', 'digital']);
    expect(sections[0].items.map((i) => i.name)).toEqual(['orders', 'users']);
    expect(sections[1].dataSources).toEqual([
      { name: 'example', items: [{ name: 'fatos' }] },
      { name: 'sofiadb', items: [{ name: 'pos_falha' }, { name: 'eventos' }] },
    ]);
    expect(sections[1].errors).toBe(2);
  });

  it('drops groups and data sources with none of the items', () => {
    const sections = sectionsOf(items('eventos'), groups);
    expect(sections).toHaveLength(1);
    expect(sections[0].dataSources.map((d) => d.name)).toEqual(['sofiadb']);
  });

  it('keeps empty groups when asked, for their errors', () => {
    const sections = sectionsOf(items('users'), groups, { keepEmpty: true });
    expect(sections.map((s) => [s.slug, s.items.length])).toEqual([
      ['_platform', 1], ['digital', 0], ['on', 0],
    ]);
  });

  it('puts what no group names under Other, last', () => {
    const sections = sectionsOf(items('ghost', 'users'), groups);
    expect(sections.map((s) => s.name)).toEqual(['Platform', 'Other']);
    expect(sections[1].items).toEqual([{ name: 'ghost' }]);
  });
});

describe('worthGrouping', () => {
  it('needs two organizations or two data sources', () => {
    expect(worthGrouping(sectionsOf(items('users', 'orders'), groups))).toBe(false);
    expect(worthGrouping(sectionsOf(items('fatos', 'eventos'), groups))).toBe(true);
    expect(worthGrouping(sectionsOf(items('users', 'eventos'), groups))).toBe(true);
    expect(worthGrouping([])).toBe(false);
  });
});

describe('cubeCountByDataSource', () => {
  it('sums over the groups', () => {
    expect(cubeCountByDataSource(groups)).toEqual({ default: 2, example: 1, sofiadb: 2 });
  });
});

describe('fetchMetaGroups', () => {
  it('returns the groups', async () => {
    const doFetch = vi.fn(async () => new Response(JSON.stringify({ groups })));
    expect(await fetchMetaGroups('/cube/v1/meta/groups', {}, doFetch)).toEqual(groups);
  });

  it('is null when the server has no such route or fails', async () => {
    const notFound = vi.fn(async () => new Response('{"error":"Cannot GET"}', { status: 404 }));
    expect(await fetchMetaGroups('/x', {}, notFound)).toBeNull();
    const broken = vi.fn(async () => { throw new Error('network'); });
    expect(await fetchMetaGroups('/x', {}, broken)).toBeNull();
    const html = vi.fn(async () => new Response('<html>'));
    expect(await fetchMetaGroups('/x', {}, html)).toBeNull();
  });
});
