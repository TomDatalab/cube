import { describe, expect, it } from 'vitest';

import {
  addPoint, endMarks, endSides, markPath, moveCorner, moveSegment, orthogonalPath, reducePoint,
  removeCorner, routeOf,
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

  it('endSides leave towards the other table, swapped the other way', () => {
    const left = { x: 0, width: 100 };
    const right = { x: 300, width: 100 };
    expect(endSides(left, right, false)).toEqual(['right', 'left']);
    expect(endSides(right, left, false)).toEqual(['left', 'right']);
    expect(endSides(left, right, true)).toEqual(['left', 'right']);
  });

  it('endMarks uses cardinality and mandatory', () => {
    const j = { ...join('many_to_one', 'left'), cardinality: '1..*' as const, mandatory: true };
    expect(endMarks(j)).toEqual({
      from: { many: true, optional: false },
      to: { many: false, optional: false },
    });
    const optional = { ...j, cardinality: '0..1' as const, mandatory: false, relationship: 'one_to_one' as const };
    expect(endMarks(optional)).toEqual({
      from: { many: false, optional: true },
      to: { many: false, optional: true },
    });
    // The child on the `to` side.
    const flipped = { ...join('one_to_many', 'inner'), cardinality: '1..*' as const, mandatory: true };
    expect(endMarks(flipped)).toEqual({
      from: { many: false, optional: false },
      to: { many: true, optional: false },
    });
  });

  describe('editing the line', () => {
    const source = { x: 0, y: 0 };
    const target = { x: 300, y: 120 };
    const route = (points: { x: number; y: number }[] = []) => routeOf(source, 'right', target, 'left', points);

    it('routeOf lists the corners an unedited line turns at', () => {
      // Out 20 to the right, across to the target's stub, down, in.
      expect(route()).toEqual({
        start: { x: 20, y: 0 },
        corners: [{ x: 280, y: 0 }],
        end: { x: 280, y: 120 },
      });
    });

    it('moveCorner moves a corner the line made by itself', () => {
      const { start, corners, end } = route();
      const points = moveCorner(start, corners, end, 0, { x: 150, y: 0 });
      // Turns down at x 150, then across at y 120.
      expect(points).toEqual([{ x: 150, y: 0 }, { x: 150, y: 120 }]);
      const path = orthogonalPath(source, 'right', target, 'left', points);
      expect(straight(path.segments)).toBe(true);
      expect(path.segments.some(([p, q]) => p.x === 150 && q.x === 150)).toBe(true);
    });

    it('moveSegment drags a vertical segment sideways and keeps square corners', () => {
      const { start, corners, end } = route();
      // Segments: start→(280,0), (280,0)→end. The second is vertical.
      const points = moveSegment(start, corners, end, 1, { x: 100, y: 60 });
      expect(points).toEqual([{ x: 100, y: 0 }, { x: 100, y: 120 }]);
      expect(straight(orthogonalPath(source, 'right', target, 'left', points).segments)).toBe(true);
    });

    it('moveSegment drags the first horizontal segment down with a new corner at the stub', () => {
      const { start, corners, end } = route();
      const points = moveSegment(start, corners, end, 0, { x: 150, y: 40 });
      expect(points).toEqual([{ x: 20, y: 40 }, { x: 280, y: 40 }]);
      const path = orthogonalPath(source, 'right', target, 'left', points);
      expect(straight(path.segments)).toBe(true);
      expect(path.segments.some(([p, q]) => p.y === 40 && q.y === 40)).toBe(true);
    });

    it('a corner dragged into line with its neighbours goes away', () => {
      const { start, corners, end } = route([{ x: 150, y: 0 }, { x: 150, y: 120 }]);
      expect(corners).toEqual([{ x: 150, y: 0 }, { x: 150, y: 120 }]);
      // Move the second corner back up onto y 0: the dent is gone and the
      // line turns where it would by itself.
      expect(moveCorner(start, corners, end, 1, { x: 150, y: 0 })).toEqual([{ x: 280, y: 0 }]);
    });

    it('removeCorner drops a corner and straightens the rest', () => {
      const { start, corners, end } = route([{ x: 150, y: 0 }, { x: 150, y: 120 }]);
      const points = removeCorner(start, corners, end, 1);
      expect(points).toEqual([{ x: 150, y: 0 }]);
      expect(straight(orthogonalPath(source, 'right', target, 'left', points).segments)).toBe(true);
    });
  });
});
