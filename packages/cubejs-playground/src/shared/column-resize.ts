// Columns the user resizes by hand, in every table of the app: a handle on
// the right edge of each header cell; dragging it sets the column's width,
// a double click gives the table back its automatic widths.
//
// One observer over the page finds the tables as they appear, whatever
// renders them (antd, shadcn/ui, plain HTML), so no table has to opt in:
// - an HTML table with a header row. The first drag freezes every column at
//   its current width with `table-layout: fixed`, so a column can also get
//   narrower than its content (cut with an ellipsis). antd's tables split in
//   a header and a body table (sticky headers, scrolling) share the widths
//   through their `<col>`s.
// - the Playground's results grid (CSS grid, `data-qa="ResultsGrid"`, its
//   headers `data-qa="ResultsColumnHeader"`): the drag sets
//   `grid-template-columns`.
// A table opts out with `data-no-resize`.

export const HANDLE_CLASS = 'pg-col-resize';
export const RESIZED_ATTR = 'data-col-resized';
export const MIN_WIDTH = 40;

const GRID = '[data-qa="ResultsGrid"]';
const GRID_HEADER = '[data-qa="ResultsColumnHeader"]';

type TableState = {
  // The widths the user set, per header cell index.
  widths: number[];
  // The table's inline styles before the first drag, for the reset.
  saved: Map<HTMLTableElement, { tableLayout: string; width: string; minWidth: string }>;
};

const tableStates = new WeakMap<HTMLTableElement, TableState>();
const gridStates = new WeakMap<HTMLElement, { widths: number[]; columns: number }>();

/** The cells of a table's last header row: one per column. */
export function headerCells(table: HTMLTableElement): HTMLTableCellElement[] {
  const rows = table.tHead?.rows;
  if (!rows || !rows.length) {
    return [];
  }
  return [...rows[rows.length - 1].cells];
}

/**
 * The tables that share `table`'s columns: antd draws a scrolling or sticky
 * table as a header table and a body table, each with its `<col>`s.
 */
export function linkedTables(table: HTMLTableElement): HTMLTableElement[] {
  const container = table.closest('.ant-table');
  if (!container) {
    return [table];
  }
  return [...container.querySelectorAll('table')].filter((t) => t.closest('.ant-table') === container);
}

function cols(table: HTMLTableElement): HTMLTableColElement[] {
  return [...table.querySelectorAll(':scope > colgroup > col')] as HTMLTableColElement[];
}

/** Sets column `i`'s width on its header cell and on every linked `<col>`. */
function setColumnWidth(table: HTMLTableElement, cells: HTMLTableCellElement[], i: number, width: number) {
  cells[i].style.width = `${width}px`;
  linkedTables(table).forEach((t) => {
    const c = cols(t);
    if (c.length === cells.length) {
      c[i].style.width = `${width}px`;
      c[i].style.minWidth = `${width}px`;
    }
  });
}

function setTableWidth(table: HTMLTableElement, widths: number[]) {
  const total = widths.reduce((sum, w) => sum + w, 0);
  linkedTables(table).forEach((t) => {
    t.style.tableLayout = 'fixed';
    t.style.width = `${total}px`;
    t.style.minWidth = '0';
    t.setAttribute(RESIZED_ATTR, '');
  });
}

/** Freezes every column at its current width; the table's state. */
function freeze(table: HTMLTableElement, cells: HTMLTableCellElement[]): TableState {
  const existing = tableStates.get(table);
  if (existing) {
    return existing;
  }
  const saved = new Map<HTMLTableElement, { tableLayout: string; width: string; minWidth: string }>();
  linkedTables(table).forEach((t) => saved.set(t, {
    tableLayout: t.style.tableLayout,
    width: t.style.width,
    minWidth: t.style.minWidth,
  }));
  // As they are: only the column dragged keeps to MIN_WIDTH.
  const widths = cells.map((c) => Math.max(1, Math.round(c.getBoundingClientRect().width)));
  const state = { widths, saved };
  tableStates.set(table, state);
  widths.forEach((w, i) => setColumnWidth(table, cells, i, w));
  setTableWidth(table, widths);
  return state;
}

/** Back to the widths the table had before any drag. */
export function resetTable(table: HTMLTableElement) {
  const state = tableStates.get(table);
  if (!state) {
    return;
  }
  tableStates.delete(table);
  const cells = headerCells(table);
  cells.forEach((c) => {
    c.style.width = '';
  });
  state.saved.forEach((style, t) => {
    t.style.tableLayout = style.tableLayout;
    t.style.width = style.width;
    t.style.minWidth = style.minWidth;
    t.removeAttribute(RESIZED_ATTR);
    cols(t).forEach((c) => {
      c.style.width = '';
      c.style.minWidth = '';
    });
  });
}

/** Column `i` of `table` at `width` (at least MIN_WIDTH). */
export function resizeTableColumn(table: HTMLTableElement, i: number, width: number) {
  const cells = headerCells(table);
  if (!cells[i]) {
    return;
  }
  const state = freeze(table, cells);
  state.widths[i] = Math.max(MIN_WIDTH, Math.round(width));
  setColumnWidth(table, cells, i, state.widths[i]);
  setTableWidth(table, state.widths);
}

/** Puts the widths back after a re-render replaced cells or `<col>`s. */
function reapply(table: HTMLTableElement) {
  const state = tableStates.get(table);
  const cells = headerCells(table);
  if (!state) {
    return;
  }
  if (cells.length !== state.widths.length) {
    // Other columns now: they start over.
    resetTable(table);
    return;
  }
  cells.forEach((c, i) => {
    if (c.style.width !== `${state.widths[i]}px`) {
      setColumnWidth(table, cells, i, state.widths[i]);
    }
  });
  if (!table.hasAttribute(RESIZED_ATTR)) {
    setTableWidth(table, state.widths);
  }
}

/** The grid's column widths, frozen on the first drag. */
function gridWidths(grid: HTMLElement, headers: HTMLElement[]) {
  let state = gridStates.get(grid);
  if (!state || state.columns !== headers.length) {
    state = {
      widths: headers.map((h) => Math.max(1, Math.round(h.getBoundingClientRect().width))),
      columns: headers.length,
    };
    gridStates.set(grid, state);
  }
  return state;
}

function gridHeaders(grid: HTMLElement): HTMLElement[] {
  return [...grid.querySelectorAll(GRID_HEADER)] as HTMLElement[];
}

/** Column `i` of the results grid at `width`. */
export function resizeGridColumn(grid: HTMLElement, i: number, width: number) {
  const headers = gridHeaders(grid);
  if (!headers[i]) {
    return;
  }
  const state = gridWidths(grid, headers);
  state.widths[i] = Math.max(MIN_WIDTH, Math.round(width));
  grid.style.gridTemplateColumns = state.widths.map((w) => `${w}px`).join(' ');
  grid.setAttribute(RESIZED_ATTR, '');
}

export function resetGrid(grid: HTMLElement) {
  gridStates.delete(grid);
  grid.style.gridTemplateColumns = '';
  grid.removeAttribute(RESIZED_ATTR);
}

type Target = { cell: HTMLElement; resize: (width: number) => void; reset: () => void };

/** Starts a drag from a handle: the width follows the pointer. */
function startDrag(event: PointerEvent, target: Target, handle: HTMLElement) {
  event.preventDefault();
  event.stopPropagation();
  const startX = event.clientX;
  const startWidth = target.cell.getBoundingClientRect().width;
  handle.classList.add('active');
  document.body.classList.add('pg-col-resizing');
  const move = (e: PointerEvent) => target.resize(startWidth + e.clientX - startX);
  const up = () => {
    handle.classList.remove('active');
    document.body.classList.remove('pg-col-resizing');
    window.removeEventListener('pointermove', move);
    window.removeEventListener('pointerup', up);
    window.removeEventListener('pointercancel', up);
  };
  window.addEventListener('pointermove', move);
  window.addEventListener('pointerup', up);
  window.addEventListener('pointercancel', up);
}

/** Adds the handle to `cell` unless it has one. */
function addHandle(cell: HTMLElement, target: () => Target | null) {
  if (cell.querySelector(`:scope > .${HANDLE_CLASS}`)) {
    return;
  }
  const handle = document.createElement('span');
  handle.className = HANDLE_CLASS;
  handle.setAttribute('role', 'separator');
  handle.setAttribute('aria-orientation', 'vertical');
  handle.setAttribute('aria-label', 'Resize column (double-click to reset)');
  handle.title = 'Drag to resize, double-click to reset';
  handle.addEventListener('pointerdown', (e) => {
    const t = target();
    if (t) {
      startDrag(e, t, handle);
    }
  });
  // Neither a click nor a double click reaches the header (sorting,
  // selecting, reordering).
  handle.addEventListener('click', (e) => e.stopPropagation());
  handle.addEventListener('mousedown', (e) => e.stopPropagation());
  handle.addEventListener('dblclick', (e) => {
    e.preventDefault();
    e.stopPropagation();
    target()?.reset();
  });
  const { position } = getComputedStyle(cell);
  if (!position || position === 'static') {
    cell.style.position = 'relative';
  }
  cell.appendChild(handle);
}

export function enhanceTable(table: HTMLTableElement) {
  if (table.hasAttribute('data-no-resize') || table.closest('[data-no-resize]')) {
    return;
  }
  const cells = headerCells(table);
  cells.forEach((cell) => {
    if (cell.colSpan > 1) {
      return;
    }
    addHandle(cell, () => {
      // The cell's table and index now: a re-render may have moved it.
      const own = cell.closest('table') as HTMLTableElement | null;
      if (!own) {
        return null;
      }
      const i = headerCells(own).indexOf(cell as HTMLTableCellElement);
      if (i < 0) {
        return null;
      }
      return {
        cell,
        resize: (w) => resizeTableColumn(own, i, w),
        reset: () => resetTable(own),
      };
    });
  });
  reapply(table);
}

export function enhanceGrid(grid: HTMLElement) {
  const headers = gridHeaders(grid);
  const state = gridStates.get(grid);
  if (state && state.columns !== headers.length) {
    // Other columns now: automatic widths again.
    resetGrid(grid);
  }
  headers.forEach((header) => addHandle(header, () => {
    const i = gridHeaders(grid).indexOf(header);
    if (i < 0) {
      return null;
    }
    return {
      cell: header,
      resize: (w) => resizeGridColumn(grid, i, w),
      reset: () => resetGrid(grid),
    };
  }));
}

/** Enhances every table and results grid under `root`. */
export function scan(root: ParentNode) {
  root.querySelectorAll('table').forEach((t) => {
    if (t.tHead) {
      enhanceTable(t as HTMLTableElement);
    }
  });
  root.querySelectorAll(GRID).forEach((g) => enhanceGrid(g as HTMLElement));
}

/**
 * Watches `root` and enhances its tables as they appear or re-render. Returns
 * the function that stops watching.
 */
export function installColumnResize(root: HTMLElement = document.body): () => void {
  let scheduled = false;
  const run = () => {
    scheduled = false;
    scan(root);
  };
  const observer = new MutationObserver((mutations) => {
    // Our own handles coming and going are not news.
    const relevant = mutations.some((m) => [...m.addedNodes, ...m.removedNodes]
      .some((n) => !(n instanceof HTMLElement && n.classList.contains(HANDLE_CLASS))));
    if (relevant && !scheduled) {
      scheduled = true;
      requestAnimationFrame(run);
    }
  });
  observer.observe(root, { childList: true, subtree: true });
  scan(root);
  return () => observer.disconnect();
}
