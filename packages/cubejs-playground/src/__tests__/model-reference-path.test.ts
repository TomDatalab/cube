import { describe, expect, it } from 'vitest';

import {
  addPoint, endMarks, markPath, orthogonalPath, reducePoint,
} from '../pages/Schema/models/referencePath';
import type { ModelJoin } from '../pages/Schema/models/modelGraph';

const join = (relationship: ModelJoin['relationship'], kind: ModelJoin['join']): ModelJoin => ({
  from: { alias: 'orders', column: 'customer_id' },
  to: { alias: 'customers', column: 'id' },
  relationship,
  join: kind,
});

const straight = (segments: [{ x: number; y: number }, { x: number; y: number }][]) => segments
  .every(([p, q]) => p.x === q.x || p.y === q.y);

describe('referencePath', () => {
  it('endMarks for many_to_one left', () => {
    expect(endMarks(join('many_to_one', 'left'))).toEqual({
      from: { many: true, optional: true },
      to: { many: false, optional: true },
    });
  });

  it('endMarks for inner makes the one side mandatory', () => {
    expect(endMarks(join('many_to_one', 'inner')).to).toEqual({ many: false, optional: false });
    expect(endMarks(join('one_to_many', 'inner'))).toEqual({
      from: { many: false, optional: false },
      to: { many: true, optional: true },
    });
    expect(endMarks(join('one_to_one', 'left'))).toEqual({
      from: { many: false, optional: true },
      to: { many: false, optional: true },
    });
  });

  it('orthogonalPath without points has only horizontal and vertical segments', () => {
    const path = orthogonalPath({ x: 0, y: 0 }, 'right', { x: 300, y: 120 }, 'left');
    expect(straight(path.segments)).toBe(true);
    expect(path.d.startsWith('M 0 0')).toBe(true);
    expect(path.d.endsWith('300 120')).toBe(true);
  });

  it('orthogonalPath passes through each reference point', () => {
    const points = [{ x: 150, y: -40 }, { x: 200, y: 200 }];
    const path = orthogonalPath({ x: 0, y: 0 }, 'right', { x: 300, y: 120 }, 'left', points);
    expect(straight(path.segments)).toBe(true);
    const corners = path.segments.flat();
    for (const p of points) {
      expect(corners).toContainEqual(p);
    }
  });

  it('mid is the middle of the longest segment', () => {
    const path = orthogonalPath({ x: 0, y: 0 }, 'right', { x: 300, y: 0 }, 'left');
    expect(path.mid).toEqual({ x: 150, y: 0 });
  });

  it('addPoint splits the longest segment and reducePoint drops the last point', () => {
    const source = { x: 0, y: 0 };
    const target = { x: 300, y: 0 };
    const path = orthogonalPath(source, 'right', target, 'left');
    const one = addPoint([], path);
    expect(one).toEqual([{ x: 150, y: 0 }]);
    const withOne = orthogonalPath(source, 'right', target, 'left', one);
    const two = addPoint(one, withOne);
    expect(two).toHaveLength(2);
    expect(two[0].x).toBeLessThan(two[1].x);
    expect(reducePoint(two)).toEqual([two[0]]);
    expect(reducePoint([])).toEqual([]);
  });

  it('swapped sides start the path on the left', () => {
    const path = orthogonalPath({ x: 100, y: 0 }, 'left', { x: 400, y: 50 }, 'right');
    expect(path.segments[0]).toEqual([{ x: 100, y: 0 }, { x: 80, y: 0 }]);
    expect(path.segments[path.segments.length - 1][0]).toEqual({ x: 420, y: 50 });
  });

  it('markPath draws a crow\'s foot for many and a circle for optional', () => {
    const many = markPath({ x: 0, y: 0 }, 'right', { many: true, optional: true });
    expect(many.match(/M/g)!.length).toBeGreaterThanOrEqual(4);
    expect(many).toContain('a ');
    const one = markPath({ x: 0, y: 0 }, 'left', { many: false, optional: false });
    expect(one).not.toContain('a ');
    expect(one).toContain('M -4 -6');
  });
});
