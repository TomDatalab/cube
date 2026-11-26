// @vitest-environment jsdom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { describe, expect, it } from 'vitest';

import {
  History, record, redo, start, undo, useHistory,
} from '../pages/Schema/models/history';

describe('history', () => {
  it('record, undo and redo walk the states', () => {
    let h = start(1);
    h = record(h, 2);
    h = record(h, 3);
    expect(h.present).toBe(3);
    h = undo(h);
    expect(h.present).toBe(2);
    h = undo(h);
    expect(h.present).toBe(1);
    expect(undo(h)).toBe(h);
    h = redo(h);
    expect(h.present).toBe(2);
    h = redo(h);
    expect(h.present).toBe(3);
    expect(redo(h)).toBe(h);
  });

  it('a new record after undo drops the future', () => {
    let h = record(record(start(1), 2), 3);
    h = undo(h);
    h = record(h, 9);
    expect(h.future).toEqual([]);
    expect(h.past).toEqual([1, 2]);
    expect(h.present).toBe(9);
  });

  it('keeps at most 100 past states', () => {
    let h: History<number> = start(0);
    for (let i = 1; i <= 105; i++) {
      h = record(h, i);
    }
    expect(h.past.length).toBe(100);
    expect(h.past[0]).toBe(5);
  });

  it('recording the same value is a no-op', () => {
    const value = { a: 1 };
    const h = start(value);
    expect(record(h, value)).toBe(h);
  });

  it('useHistory exposes the walk and reset to a component', () => {
    let api: ReturnType<typeof useHistory<number>> | null = null;
    function Probe() {
      api = useHistory(1);
      return null;
    }
    const root = createRoot(document.createElement('div'));
    act(() => root.render(createElement(Probe)));
    act(() => api!.set(2));
    expect(api!.value).toBe(2);
    expect(api!.canUndo).toBe(true);
    act(() => api!.undo());
    expect(api!.value).toBe(1);
    expect(api!.canRedo).toBe(true);
    act(() => api!.redo());
    expect(api!.value).toBe(2);
    act(() => api!.reset(7));
    expect(api!.value).toBe(7);
    expect(api!.canUndo).toBe(false);
    expect(api!.canRedo).toBe(false);
    act(() => root.unmount());
  });
});
