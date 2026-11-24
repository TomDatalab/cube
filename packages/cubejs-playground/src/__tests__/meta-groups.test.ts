import { describe, expect, it, vi } from 'vitest';

import {
  cubeCountByDataSource, fetchMetaGroups, MetaGroup, mergeMetaGroups, otherScopeSections,
  rememberCubeToOpen, sectionsOf, takeCubeToOpen, worthGrouping,
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

describe('mergeMetaGroups', () => {
  it('merges the answers of every scope, keeping the Platform group once', () => {
    const [platform, digital, on] = groups;
    const merged = mergeMetaGroups([
      [platform],
      [platform, digital],
      [{ ...platform, errors: 9 }, on],
    ]);
    expect(merged.map((g) => g.slug)).toEqual(['_platform', 'digital', 'on']);
    expect(merged[0]).toEqual(platform);
    expect(cubeCountByDataSource(merged)).toEqual({ default: 2, example: 1, sofiadb: 2 });
  });
});

describe('otherScopeSections', () => {
  const [platform, digital, on] = groups;
  const bossa: MetaGroup = {
    slug: 'bossa',
    id: 'o3',
    name: 'bossa',
    errors: 0,
    dataSources: [{ name: 'bossa_sfdb', cubes: ['eventos', 'resumo'] }],
    views: ['resumo'],
  };
  const all = [platform, digital, on, bossa];

  it('lists the organizations the current model does not serve', () => {
    const sections = otherScopeSections(all, [platform, digital], 'cubes');
    expect(sections.map((s) => s.slug)).toEqual(['on', 'bossa']);
    expect(sections[1]).toEqual({
      slug: 'bossa',
      name: 'bossa',
      errors: 0,
      count: 1,
      dataSources: [{ name: 'bossa_sfdb', names: ['eventos'] }],
    });
  });

  it('never lists the Platform, which every model serves', () => {
    expect(otherScopeSections(all, [], 'cubes').map((s) => s.slug)).toEqual(['digital', 'on', 'bossa']);
  });

  it('keeps cubes and views apart', () => {
    const sections = otherScopeSections(all, [platform], 'views');
    // Digital and ON have no views but errors, which the admin still sees.
    expect(sections.map((s) => [s.slug, s.count])).toEqual([['digital', 0], ['on', 0], ['bossa', 1]]);
  });

  it('filters by name and then drops what has nothing to show', () => {
    const sections = otherScopeSections(all, [platform], 'cubes', 'even');
    expect(sections.map((s) => [s.slug, s.dataSources])).toEqual([
      ['digital', [{ name: 'sofiadb', names: ['eventos'] }]],
      ['bossa', [{ name: 'bossa_sfdb', names: ['eventos'] }]],
    ]);
  });
});

describe('cube to open', () => {
  it('is taken once', () => {
    rememberCubeToOpen('eventos');
    expect(takeCubeToOpen()).toBe('eventos');
    expect(takeCubeToOpen()).toBeNull();
  });
});
