import { Node, getNodesBounds } from '@xyflow/react';

import type { ModelSettings } from '../modelGraph';

export type PrintSettings = NonNullable<ModelSettings['print']>;
export type PageSize = NonNullable<PrintSettings['pageSize']>;
export type Orientation = NonNullable<PrintSettings['orientation']>;
export type Rect = { x: number; y: number; width: number; height: number };

// Portrait sizes, in millimetres.
export const PAGE_SIZES: Record<PageSize, { width: number; height: number }> = {
  A4: { width: 210, height: 297 },
  A3: { width: 297, height: 420 },
  Letter: { width: 215.9, height: 279.4 },
  Legal: { width: 215.9, height: 355.6 },
};

export const PRINT_DEFAULTS = {
  pageSize: 'A4' as PageSize,
  orientation: 'landscape' as Orientation,
  margin: 10,
  showPageBreaks: false,
};

// Space around what an export shows, in canvas units.
const PAD = 20;

// One canvas unit prints as one CSS pixel: 96 to the inch.
export function mmToPx(mm: number): number {
  return (mm * 96) / 25.4;
}

// The page, in millimetres, with its orientation applied.
export function pageSizeMm(print: PrintSettings | undefined): { width: number; height: number } {
  const { width, height } = PAGE_SIZES[print?.pageSize ?? PRINT_DEFAULTS.pageSize];
  return (print?.orientation ?? PRINT_DEFAULTS.orientation) === 'landscape'
    ? { width: height, height: width }
    : { width, height };
}

export function marginMm(print: PrintSettings | undefined): number {
  return print?.margin ?? PRINT_DEFAULTS.margin;
}

// The printable area of each page over `bounds`, in canvas units: row by
// row, left to right, from the bounds' top-left corner; at least one page.
export function pageGrid(bounds: Rect, print: PrintSettings | undefined): Rect[] {
  const page = pageSizeMm(print);
  const margin = marginMm(print);
  const width = mmToPx(page.width - 2 * margin);
  const height = mmToPx(page.height - 2 * margin);
  const columns = Math.max(1, Math.ceil(bounds.width / width));
  const rows = Math.max(1, Math.ceil(bounds.height / height));
  const pages: Rect[] = [];
  for (let row = 0; row < rows; row++) {
    for (let column = 0; column < columns; column++) {
      pages.push({
        x: bounds.x + column * width, y: bounds.y + row * height, width, height,
      });
    }
  }
  return pages;
}

// What an export shows, padded: every node, or the selected ones (null when
// none is). A model with no nodes is a small blank.
export function diagramBounds(nodes: Node[], scope: 'all' | 'selection'): Rect | null {
  const shown = scope === 'selection' ? nodes.filter((n) => n.selected) : nodes;
  if (!shown.length) {
    return scope === 'selection' ? null : {
      x: 0, y: 0, width: 2 * PAD, height: 2 * PAD,
    };
  }
  const box = getNodesBounds(shown);
  return {
    x: box.x - PAD, y: box.y - PAD, width: box.width + 2 * PAD, height: box.height + 2 * PAD,
  };
}
