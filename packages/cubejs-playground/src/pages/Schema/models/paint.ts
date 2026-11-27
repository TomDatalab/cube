import type { TableColor } from './modelGraph';

// How transparent a palette color may be made, in percent (the server's
// TRANSPARENCY): never fully clear.
export const MAX_TRANSPARENCY = 90;

// A palette color made `transparency` percent see-through.
export function paint(color: TableColor, transparency?: number): string {
  return transparency
    ? `color-mix(in oklch, var(--model-${color}) ${100 - transparency}%, transparent)`
    : `var(--model-${color})`;
}

// Text on a palette color: dark on the color, the theme's own once the color
// is mostly see-through and the canvas shows under it.
export function inkOn(transparency?: number): string {
  return (transparency ?? 0) >= 50 ? 'var(--foreground)' : 'var(--model-ink)';
}
