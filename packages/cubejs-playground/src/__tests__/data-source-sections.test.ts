import { describe, expect, it } from 'vitest';

import { dataSourceSections } from '../pages/DataSources/sections';
import type { MetaGroup } from '../shared/meta-groups';

const digital = { id: 'o1', name: 'Digital' };
const sources = [
  { name: 'default', organization: null },
  { name: 'sofiadb', organization: digital },
  { name: 'example', organization: digital },
  { name: 'stray', organization: { id: 'o9', name: 'Later' } },
];
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
    errors: 3,
    dataSources: [{ name: 'sofiadb', cubes: ['a', 'b', 'c'] }],
  },
];

describe('dataSourceSections', () => {
  it('puts Platform first, then every organization by name', () => {
    const sections = dataSourceSections(sources, [{ id: 'o2', name: 'ON' }, digital], groups);
    expect(sections.map((s) => s.name)).toEqual(['Platform', 'Digital', 'Later', 'ON']);
    expect(sections.map((s) => s.dataSources.map((d) => d.name))).toEqual([
      ['default'], ['sofiadb', 'example'], ['stray'], [],
    ]);
  });

  it('counts cubes and errors from the meta groups', () => {
    const sections = dataSourceSections(sources, [digital], groups);
    expect(sections.map((s) => [s.name, s.cubes, s.errors])).toEqual([
      ['Platform', 2, 0], ['Digital', 3, 3], ['Later', 0, 0],
    ]);
  });

  it('leaves errors unknown without groups', () => {
    const sections = dataSourceSections(sources, [], null);
    expect(sections.every((s) => s.errors === null && s.cubes === 0)).toBe(true);
  });
});
