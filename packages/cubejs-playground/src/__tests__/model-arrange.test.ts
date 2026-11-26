import { describe, expect, it } from 'vitest';

import {
  Box, align, distribute, sameSize,
} from '../pages/Schema/models/arrange';

const a: Box = {
  id: 'a', x: 0, y: 0, width: 100, height: 50,
};
const b: Box = {
  id: 'b', x: 200, y: 100, width: 50, height: 30,
};
const at = (m: Map<string, { x: number; y: number }>, id: string) => [m.get(id)!.x, m.get(id)!.y];

describe('arrange', () => {
  it('align left/center/right/top/middle/bottom', () => {
    let m = align([a, b], 'left');
    expect(at(m, 'a')).toEqual([0, 0]);
    expect(at(m, 'b')).toEqual([0, 100]);
    m = align([a, b], 'right');
    expect(at(m, 'a')).toEqual([150, 0]);
    expect(at(m, 'b')).toEqual([200, 100]);
    m = align([a, b], 'center');
    expect(at(m, 'a')).toEqual([75, 0]);
    expect(at(m, 'b')).toEqual([100, 100]);
    m = align([a, b], 'top');
    expect(at(m, 'b')).toEqual([200, 0]);
    m = align([a, b], 'bottom');
    expect(at(m, 'a')).toEqual([0, 80]);
    expect(at(m, 'b')).toEqual([200, 100]);
    m = align([a, b], 'middle');
    expect(at(m, 'a')).toEqual([0, 40]);
    expect(at(m, 'b')).toEqual([200, 50]);
  });

  it('distribute h-justify leaves equal gaps', () => {
    const boxes: Box[] = [
      { id: 'c', x: 400, y: 0, width: 100, height: 10 },
      { id: 'a', x: 0, y: 5, width: 100, height: 10 },
      { id: 'b', x: 130, y: 9, width: 50, height: 10 },
    ];
    const m = distribute(boxes, 'h-justify');
    expect(at(m, 'a')).toEqual([0, 5]);
    expect(at(m, 'b')).toEqual([225, 9]);
    expect(at(m, 'c')).toEqual([400, 0]);
  });

  it('distribute left steps the left edges evenly', () => {
    const boxes: Box[] = [
      { id: 'a', x: 0, y: 0, width: 10, height: 10 },
      { id: 'b', x: 50, y: 0, width: 80, height: 10 },
      { id: 'c', x: 400, y: 0, width: 30, height: 10 },
    ];
    expect(at(distribute(boxes, 'left'), 'b')).toEqual([200, 0]);
    // Centers 5, 90, 415 step to 5, 210, 415: b's left is 210 - 40.
    expect(at(distribute(boxes, 'center'), 'b')).toEqual([170, 0]);
    // Right edges 10, 130, 430 step to 10, 220, 430.
    expect(at(distribute(boxes, 'right'), 'b')).toEqual([140, 0]);
  });

  it('distribute vertically by top and justify', () => {
    const boxes: Box[] = [
      { id: 'a', x: 0, y: 0, width: 10, height: 100 },
      { id: 'b', x: 0, y: 130, width: 10, height: 50 },
      { id: 'c', x: 0, y: 400, width: 10, height: 100 },
    ];
    expect(at(distribute(boxes, 'v-justify'), 'b')).toEqual([0, 225]);
    expect(at(distribute(boxes, 'top'), 'b')).toEqual([0, 200]);
  });

  it('align needs 2 and distribute needs 3', () => {
    expect(align([a], 'left').size).toBe(0);
    expect(distribute([a, b], 'left').size).toBe(0);
  });

  it('sameSize copies the first box\'s width or height', () => {
    expect(sameSize([b, a], 'width').get('a')).toEqual({ width: 50 });
    expect(sameSize([b, a], 'height').get('a')).toEqual({ height: 30 });
    expect(sameSize([a], 'width').size).toBe(0);
  });
});
