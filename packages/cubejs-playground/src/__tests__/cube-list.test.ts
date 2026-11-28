import { describe, expect, it } from 'vitest';

import { cubeListRows } from '../pages/Schema/CubeList';

const base = { dataSource: 'bossa_sfdb', file: 'bossa/a.yml', members: { measures: 2, dimensions: 8 } };

describe('cubeListRows', () => {
  it('lists the cubes or the views, each by name, with their counts', () => {
    const rows = cubeListRows([
      { ...base, name: 'zeta', type: 'cube' },
      { ...base, name: 'network', type: 'view', members: { measures: 1, dimensions: 3 } },
      { ...base, name: 'alpha', type: 'cube', title: 'Alpha cables' },
    ], [], 'cube');
    expect(rows.map((r) => r.name)).toEqual(['alpha', 'zeta']);
    expect(rows[0]).toMatchObject({
      key: 'alpha', title: 'Alpha cables', type: 'cube', dataSource: 'bossa_sfdb', measures: 2, dimensions: 8, file: 'bossa/a.yml', errors: 0,
    });
    const views = cubeListRows([
      { ...base, name: 'zeta', type: 'cube' },
      { ...base, name: 'network', type: 'view', members: { measures: 1, dimensions: 3 }, includes: ['zeta'] },
    ], [], 'view');
    expect(views).toEqual([expect.objectContaining({ name: 'network', measures: 1, dimensions: 3, includes: ['zeta'] })]);
  });

  it('counts the problems of each cube, by its name or its file', () => {
    const rows = cubeListRows([
      { ...base, name: 'a', type: 'cube' },
      { ...base, name: 'b', type: 'cube', file: 'bossa/b.yml' },
    ], [
      { file: 'bossa/x.yml', cube: 'a', message: 'bad' },
      { file: 'bossa/b.yml', cube: '', message: 'broken file' },
      { file: 'bossa/b.yml', cube: 'b', message: 'also bad' },
    ], 'cube');
    expect(rows.map((r) => [r.name, r.errors])).toEqual([['a', 1], ['b', 2]]);
  });
});
