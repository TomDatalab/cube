// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import {
  enhanceGrid, enhanceTable, HANDLE_CLASS, installColumnResize, MIN_WIDTH, RESIZED_ATTR,
  resetTable, resizeGridColumn, resizeTableColumn, scan,
} from '../shared/column-resize';

// jsdom lays nothing out: each cell is as wide as its data-w.
function stubWidths() {
  HTMLElement.prototype.getBoundingClientRect = function rect(this: HTMLElement) {
    const w = Number(this.dataset.w || 100);
    return {
      width: w, height: 20, top: 0, left: 0, right: w, bottom: 20, x: 0, y: 0, toJSON: () => ({}),
    } as DOMRect;
  };
}

function table(html: string): HTMLTableElement {
  document.body.innerHTML = html;
  return document.querySelector('table')!;
}

const SIMPLE = `<table>
  <thead><tr><th data-w="120">Name</th><th data-w="80">Id</th><th data-w="200">Notes</th></tr></thead>
  <tbody><tr><td>a</td><td>b</td><td>c</td></tr></tbody>
</table>`;

const handles = () => [...document.querySelectorAll(`.${HANDLE_CLASS}`)] as HTMLElement[];
const ths = () => [...document.querySelectorAll('th')] as HTMLElement[];

function drag(handle: HTMLElement, from: number, to: number) {
  handle.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true, clientX: from }));
  window.dispatchEvent(new MouseEvent('pointermove', { clientX: to }));
  window.dispatchEvent(new MouseEvent('pointerup', { clientX: to }));
}

describe('column resizing', () => {
  const original = HTMLElement.prototype.getBoundingClientRect;
  beforeEach(stubWidths);
  afterEach(() => {
    HTMLElement.prototype.getBoundingClientRect = original;
    document.body.innerHTML = '';
  });

  it('puts one handle on each header cell, once', () => {
    const t = table(SIMPLE);
    enhanceTable(t);
    enhanceTable(t);
    expect(handles()).toHaveLength(3);
    expect(ths().every((th) => th.lastElementChild?.classList.contains(HANDLE_CLASS))).toBe(true);
    // Relative, so the handle sits on the cell's edge.
    expect(ths()[0].style.position).toBe('relative');
  });

  it('leaves grouped header cells and opted-out tables alone', () => {
    table(`<table><thead><tr><th colspan="2">Both</th><th>One</th></tr></thead></table>
      <div data-no-resize><table><thead><tr><th>X</th></tr></thead></table></div>`);
    scan(document.body);
    expect(handles()).toHaveLength(1);
    expect(handles()[0].parentElement!.textContent).toContain('One');
  });

  it('freezes every column on the first resize and sets the one dragged', () => {
    const t = table(SIMPLE);
    enhanceTable(t);
    drag(handles()[1], 500, 560);
    expect(ths().map((th) => th.style.width)).toEqual(['120px', '140px', '200px']);
    expect(t.style.tableLayout).toBe('fixed');
    expect(t.style.width).toBe('460px');
    expect(t.hasAttribute(RESIZED_ATTR)).toBe(true);
  });

  it('never makes a column narrower than the minimum', () => {
    const t = table(SIMPLE);
    resizeTableColumn(t, 0, 3);
    expect(ths()[0].style.width).toBe(`${MIN_WIDTH}px`);
  });

  it('gives the automatic widths back on a double click', () => {
    const t = table(SIMPLE);
    t.style.width = '100%';
    enhanceTable(t);
    drag(handles()[0], 0, 50);
    handles()[0].dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
    expect(ths().map((th) => th.style.width)).toEqual(['', '', '']);
    expect(t.style.tableLayout).toBe('');
    expect(t.style.width).toBe('100%');
    expect(t.hasAttribute(RESIZED_ATTR)).toBe(false);
  });

  it('keeps a drag on the handle from reaching the header', () => {
    const t = table(SIMPLE);
    enhanceTable(t);
    let clicks = 0;
    ths()[0].addEventListener('click', () => {
      clicks += 1;
    });
    handles()[0].dispatchEvent(new MouseEvent('click', { bubbles: true }));
    expect(clicks).toBe(0);
  });

  it('resizes both halves of an antd table through their cols', () => {
    document.body.innerHTML = `<div class="ant-table">
      <table class="head"><colgroup><col><col></colgroup>
        <thead><tr><th data-w="100">A</th><th data-w="100">B</th></tr></thead></table>
      <table class="body"><colgroup><col><col></colgroup><tbody><tr><td>1</td><td>2</td></tr></tbody></table>
    </div>`;
    const head = document.querySelector('table.head') as HTMLTableElement;
    const body = document.querySelector('table.body') as HTMLTableElement;
    resizeTableColumn(head, 1, 250);
    const widths = (t: HTMLTableElement) => [...t.querySelectorAll('col')].map((c) => (c as HTMLElement).style.width);
    expect(widths(head)).toEqual(['100px', '250px']);
    expect(widths(body)).toEqual(['100px', '250px']);
    expect(body.style.tableLayout).toBe('fixed');
    expect(body.style.width).toBe('350px');
    resetTable(head);
    expect(widths(body)).toEqual(['', '']);
    expect(body.style.tableLayout).toBe('');
  });

  it('puts the widths back when a re-render resets them, and starts over with other columns', () => {
    const t = table(SIMPLE);
    resizeTableColumn(t, 2, 300);
    ths()[2].style.width = '';
    enhanceTable(t);
    expect(ths()[2].style.width).toBe('300px');
    t.tHead!.rows[0].appendChild(document.createElement('th'));
    enhanceTable(t);
    expect(t.style.tableLayout).toBe('');
    expect(ths()[0].style.width).toBe('');
  });

  it('resizes the results grid through its template', () => {
    document.body.innerHTML = `<div data-qa="ResultsGrid">
      <div data-qa="ResultsColumnHeader" data-w="150">a</div>
      <div data-qa="ResultsColumnHeader" data-w="180">b</div>
      <div>cell</div><div>cell</div>
    </div>`;
    const grid = document.querySelector('[data-qa="ResultsGrid"]') as HTMLElement;
    enhanceGrid(grid);
    expect(handles()).toHaveLength(2);
    drag(handles()[0], 10, 110);
    expect(grid.style.gridTemplateColumns).toBe('250px 180px');
    resizeGridColumn(grid, 1, 20);
    expect(grid.style.gridTemplateColumns).toBe(`250px ${MIN_WIDTH}px`);
    // A column more: back to the automatic widths.
    const extra = document.createElement('div');
    extra.dataset.qa = 'ResultsColumnHeader';
    grid.prepend(extra);
    enhanceGrid(grid);
    expect(grid.style.gridTemplateColumns).toBe('');
    expect(handles()).toHaveLength(3);
  });

  it('finds tables that appear later', async () => {
    document.body.innerHTML = '<div id="app"></div>';
    const stop = installColumnResize(document.body);
    document.getElementById('app')!.innerHTML = SIMPLE;
    await new Promise((r) => setTimeout(r, 50));
    expect(handles()).toHaveLength(3);
    stop();
  });
});
