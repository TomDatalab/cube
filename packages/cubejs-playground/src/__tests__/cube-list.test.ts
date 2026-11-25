import { describe, expect, it } from 'vitest';

import { cubeListRows } from '../pages/Schema/CubeList';

const base = { dataSource: 'bossa_sfdb', file: 'bossa/a.yml', members: { measures: 2, dimensions: 8 } };

describe('cubeListRows', () => {
  it('lists cubes before views, each by name, with their counts', () => {
    const rows = cubeListRows([
      { ...base, name: 'zeta', type: 'cube' },
      { ...base, name: 'network', type: 'view', members: { measures: 1, dimensions: 3 } },
      { ...base, name: 'alpha', type: 'cube', title: 'Alpha cables' },
    ], []);
    expect(rows.map((r) => r.name)).toEqual(['alpha', 'zeta', 'network']);
    expect(rows[0]).toMatchObject({
      key: 'alpha', title: 'Alpha cables', type: 'cube', dataSource: 'bossa_sfdb', measures: 2, dimensions: 8, file: 'bossa/a.yml', errors: 0,
    });
    expect(rows[2]).toMatchObject({ type: 'view', measures: 1, dimensions: 3 });
  });

  it('counts the problems of each cube, by its name or its file', () => {
    const rows = cubeListRows([
      { ...base, name: 'a', type: 'cube' },
      { ...base, name: 'b', type: 'cube', file: 'bossa/b.yml' },
    ], [
      { file: 'bossa/x.yml', cube: 'a', message: 'bad' },
      { file: 'bossa/b.yml', cube: '', message: 'broken file' },
      { file: 'bossa/b.yml', cube: 'b', message: 'also bad' },
    ]);
    expect(rows.map((r) => [r.name, r.errors])).toEqual([['a', 1], ['b', 2]]);
  });
});
