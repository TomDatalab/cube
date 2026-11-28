import { describe, expect, it } from 'vitest';

import { ApiError } from '../pages/Schema/api';
import { CubeEditError } from '../pages/Schema/cubeYaml';
import {
  cubeEditBlock, cubeFileOf, cubeUsers, editFailure, shortName,
} from '../pages/Schema/cubeEdit';

const cube = { name: 'cables', type: 'cube' as const, dataSource: 'db', file: 'bossa/cables.yml', members: { measures: 1, dimensions: 1 } };
const files = [
  { id: 'f1', fileName: 'bossa/cables.yml', version: 3, editable: true },
  { id: 'f2', fileName: 'bossa/sync.yml', version: 1, editable: false },
];

describe('cubeFileOf', () => {
  it('finds the file the cube is written in', () => {
    expect(cubeFileOf(files, cube)).toEqual(files[0]);
    expect(cubeFileOf(files, { ...cube, file: 'elsewhere.yml' })).toBeUndefined();
  });
});

describe('cubeEditBlock', () => {
  const ok = { cube, file: files[0], routed: true };

  it('lets an editable YAML cube be edited', () => {
    expect(cubeEditBlock(ok)).toBeNull();
    expect(cubeEditBlock({ ...ok, file: { ...files[0], fileName: 'bossa/cables.yaml' } })).toBeNull();
  });

  it('says why a cube cannot be edited here', () => {
    expect(cubeEditBlock({ ...ok, cube: { ...cube, type: 'view' } })).toMatch(/view/i);
    expect(cubeEditBlock({ ...ok, file: undefined })).toMatch(/file/);
    expect(cubeEditBlock({ ...ok, file: files[1] })).toMatch(/read-only/);
    expect(cubeEditBlock({ ...ok, file: { ...files[0], fileName: 'bossa/cables.js' } })).toMatch(/YAML/);
    expect(cubeEditBlock({ ...ok, routed: false })).toMatch(/file/);
  });
});

describe('shortName', () => {
  it('drops the cube prefix', () => {
    expect(shortName('cables.fiber_count')).toBe('fiber_count');
    expect(shortName('count')).toBe('count');
  });
});

describe('editFailure', () => {
  it('turns an edit refusal, a compile refusal and a conflict into text', () => {
    expect(editFailure(new CubeEditError('invalid name'))).toEqual({ message: 'invalid name', problems: [] });
    expect(editFailure(new ApiError('A mudança não compila', 400, [{ file: 'bossa/v.yml', cube: 'v', message: 'fiber_count is not defined' }])))
      .toEqual({ message: 'A mudança não compila', problems: ['bossa/v.yml: fiber_count is not defined'] });
    expect(editFailure(new ApiError('stale', 409)).message).toMatch(/changed/);
  });
});

describe('cubeUsers', () => {
  const base = { dataSource: 'default', file: 'f.yml', members: { measures: 0, dimensions: 0 } };
  const cubes = [
    { ...base, name: 'cables', type: 'cube' as const },
    { ...base, name: 'segments', type: 'cube' as const, joins: [{ name: 'cables', relationship: 'many_to_one' }] },
    { ...base, name: 'other', type: 'cube' as const, joins: [{ name: 'segments' }] },
    { ...base, name: 'network', type: 'view' as const, includes: ['segments', 'cables'] },
    { ...base, name: 'lonely', type: 'view' as const, includes: ['other'] },
  ];

  it('names the cubes that join it and the views that include it', () => {
    expect(cubeUsers('cables', cubes)).toEqual(['segments', 'network']);
  });

  it('is empty for a cube nobody uses', () => {
    expect(cubeUsers('lonely_cube', cubes)).toEqual([]);
  });
});
