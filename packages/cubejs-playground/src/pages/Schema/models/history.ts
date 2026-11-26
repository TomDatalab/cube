import { useCallback, useMemo, useState } from 'react';

// Undo and redo over whole values: what was, what is, what was undone.
export type History<T> = { past: T[]; present: T; future: T[] };

export const HISTORY_LIMIT = 100;

export function start<T>(value: T): History<T> {
  return { past: [], present: value, future: [] };
}

export function record<T>(h: History<T>, next: T, limit = HISTORY_LIMIT): History<T> {
  if (next === h.present) {
    return h;
  }
  const past = [...h.past, h.present];
  return { past: past.slice(Math.max(0, past.length - limit)), present: next, future: [] };
}

export function undo<T>(h: History<T>): History<T> {
  if (h.past.length === 0) {
    return h;
  }
  return {
    past: h.past.slice(0, -1),
    present: h.past[h.past.length - 1],
    future: [h.present, ...h.future],
  };
}

export function redo<T>(h: History<T>): History<T> {
  if (h.future.length === 0) {
    return h;
  }
  return {
    past: [...h.past, h.present],
    present: h.future[0],
    future: h.future.slice(1),
  };
}

export type HistoryApi<T> = {
  value: T;
  set(next: T): void;
  undo(): void;
  redo(): void;
  // A value that cannot be undone past, as after a load.
  reset(value: T): void;
  canUndo: boolean;
  canRedo: boolean;
};

export function useHistory<T>(initial: T | (() => T)): HistoryApi<T> {
  const [h, setH] = useState<History<T>>(() => start(
    typeof initial === 'function' ? (initial as () => T)() : initial
  ));
  const set = useCallback((next: T) => setH((was) => record(was, next)), []);
  const back = useCallback(() => setH(undo), []);
  const forward = useCallback(() => setH(redo), []);
  const reset = useCallback((value: T) => setH(start(value)), []);
  return useMemo(() => ({
    value: h.present,
    set,
    undo: back,
    redo: forward,
    reset,
    canUndo: h.past.length > 0,
    canRedo: h.future.length > 0,
  }), [h, set, back, forward, reset]);
}
