import { useCallback, useState } from 'react';

export const clampWidth = (width: number, min: number, max: number) => Math.round(Math.min(max, Math.max(min, width)));

function read(key: string, fallback: number, min: number, max: number): number {
  try {
    const saved = Number(window.localStorage.getItem(key));
    return saved ? clampWidth(saved, min, max) : fallback;
  } catch {
    return fallback;
  }
}

// A side panel's width, kept between `min` and `max` and remembered in this
// browser. Storage may be unavailable: the panel then starts at `fallback`.
export function useResizable(
  key: string,
  fallback: number,
  min: number,
  max: number,
): [number, (width: number) => void] {
  const [width, setWidth] = useState(() => read(key, fallback, min, max));
  const resize = useCallback((next: number) => {
    const w = clampWidth(next, min, max);
    setWidth(w);
    try {
      window.localStorage.setItem(key, String(w));
    } catch {
      // Kept for this visit only.
    }
  }, [key, min, max]);
  return [width, resize];
}
