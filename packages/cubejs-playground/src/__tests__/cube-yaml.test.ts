import { describe, expect, it } from 'vitest';

import {
  addMeasure,
  CubeEditError,
  measureNeedsSql,
  renameDimension,
  setDimensionType,
  setMeasureType,
} from '../pages/Schema/cubeYaml';

const FILE = `# Fibre cables
cubes:
- name: cables
  sql_table: dfos.fibercable
  dimensions:
  - name: id
    sql: '{CUBE}.id'
    type: string
    primary_key: true
  - name: fiber_count
    sql: '{CUBE}.fiber_count' # how many fibres
    type: number
  - name: made_at
    sql: '{CUBE}.made_at'
    type: string
  measures:
  - name: count
    type: count
  - name: fibers
    sql: '{fiber_count}'
    type: sum
  segments:
  - name: big
    sql: '{CUBE.fiber_count} > 10'
- name: other
  sql_table: t
  dimensions:
  - name: fiber_count
    sql: fiber_count
    type: number
`;

describe('setMeasureType', () => {
  it('changes the type and keeps everything else, comments included', () => {
    const out = setMeasureType(FILE, 'cables', 'fibers', 'avg');
    expect(out).toContain('  - name: fibers\n    sql: \'{fiber_count}\'\n    type: avg');
    expect(out).toContain('# Fibre cables');
    expect(out).toContain('# how many fibres');
    expect(out.replace('type: avg', 'type: sum')).toBe(FILE);
  });

  it('adds the sql a type needs when the measure has none', () => {
    const out = setMeasureType(FILE, 'cables', 'count', 'count_distinct', '{CUBE}.id');
    expect(out).toContain('  - name: count\n    type: count_distinct\n    sql: \'{CUBE}.id\'');
  });

  it('refuses a type that needs sql when there is none', () => {
    expect(() => setMeasureType(FILE, 'cables', 'count', 'sum')).toThrow(CubeEditError);
    expect(() => setMeasureType(FILE, 'cables', 'count', 'sum')).toThrow(/SQL/);
  });

  it('refuses an unknown cube, measure or type', () => {
    expect(() => setMeasureType(FILE, 'nope', 'count', 'sum', 'x')).toThrow(/nope/);
    expect(() => setMeasureType(FILE, 'cables', 'nope', 'sum', 'x')).toThrow(/nope/);
    expect(() => setMeasureType(FILE, 'cables', 'count', 'median' as any, 'x')).toThrow(/median/);
  });
});

describe('measureNeedsSql', () => {
  it('is true for every type but count', () => {
    expect(measureNeedsSql('count')).toBe(false);
    expect(measureNeedsSql('sum')).toBe(true);
    expect(measureNeedsSql('count_distinct')).toBe(true);
  });
});

describe('addMeasure', () => {
  it('appends a measure to the cube', () => {
    const out = addMeasure(FILE, 'cables', {
      name: 'avg_fibers', type: 'avg', sql: '{fiber_count}', title: 'Average fibres',
    });
    expect(out).toContain(
      "  - name: avg_fibers\n    type: avg\n    sql: '{fiber_count}'\n    title: Average fibres\n  segments:"
    );
    expect(out).toContain('# how many fibres');
  });

  it('creates the measures list when the cube has none', () => {
    const out = addMeasure(FILE, 'other', { name: 'count', type: 'count' });
    expect(out).toContain('    type: number\n  measures:\n  - name: count\n    type: count\n');
  });

  it('refuses a taken or invalid name, and a missing sql', () => {
    expect(() => addMeasure(FILE, 'cables', { name: 'count', type: 'count' })).toThrow(/count/);
    expect(() => addMeasure(FILE, 'cables', { name: 'fiber_count', type: 'count' })).toThrow(/fiber_count/);
    expect(() => addMeasure(FILE, 'cables', { name: 'big', type: 'count' })).toThrow(/big/);
    expect(() => addMeasure(FILE, 'cables', { name: '9lives', type: 'count' })).toThrow(/9lives/);
    expect(() => addMeasure(FILE, 'cables', { name: 'total', type: 'sum' })).toThrow(/SQL/);
  });
});

describe('setDimensionType', () => {
  it('changes the type', () => {
    const out = setDimensionType(FILE, 'cables', 'made_at', 'time');
    expect(out).toContain("  - name: made_at\n    sql: '{CUBE}.made_at'\n    type: time");
    expect(out.replace("made_at'\n    type: time", "made_at'\n    type: string")).toBe(FILE);
  });

  it('refuses geo, which needs latitude and longitude', () => {
    expect(() => setDimensionType(FILE, 'cables', 'made_at', 'geo' as any)).toThrow(/geo/);
  });
});

describe('renameDimension', () => {
  it('renames it and the references to it in the same cube', () => {
    const out = renameDimension(FILE, 'cables', 'fiber_count', 'fibres');
    expect(out).toContain("  - name: fibres\n    sql: '{CUBE}.fiber_count' # how many fibres");
    // The column keeps its name; the members that used the dimension follow.
    expect(out).toContain("sql: '{fibres}'");
    expect(out).toContain("sql: '{CUBE.fibres} > 10'");
    // Another cube's dimension of the same name is untouched.
    expect(out).toContain('  - name: fiber_count\n    sql: fiber_count\n    type: number');
  });

  it('refuses a taken or invalid name', () => {
    expect(() => renameDimension(FILE, 'cables', 'fiber_count', 'id')).toThrow(/id/);
    expect(() => renameDimension(FILE, 'cables', 'fiber_count', 'count')).toThrow(/count/);
    expect(() => renameDimension(FILE, 'cables', 'fiber_count', 'has space')).toThrow(/has space/);
    expect(() => renameDimension(FILE, 'cables', 'nope', 'x')).toThrow(/nope/);
  });
});

describe('file layout', () => {
  it('keeps lists indented when the file indents them', () => {
    const indented = 'cubes:\n  - name: c\n    sql_table: t\n    measures:\n      - name: count\n        type: count\n';
    expect(setMeasureType(indented, 'c', 'count', 'count_distinct', 'id')).toBe(
      'cubes:\n  - name: c\n    sql_table: t\n    measures:\n      - name: count\n        type: count_distinct\n        sql: id\n'
    );
  });

  it('refuses a file that is not valid YAML', () => {
    expect(() => setMeasureType('cubes: [', 'c', 'count', 'sum', 'x')).toThrow(CubeEditError);
  });
});
