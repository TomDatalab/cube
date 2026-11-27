// @vitest-environment jsdom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import {
  afterEach, beforeEach, describe, expect, it, vi,
} from 'vitest';

import type { CubeModelView } from '../pages/Schema/api';
import { ModelEditor, ModelEditorProps } from '../pages/Schema/models/ModelEditor';
import {
  diagramBounds, mmToPx, pageGrid, pageSizeMm,
} from '../pages/Schema/models/export/pages';
import { renderDiagram } from '../pages/Schema/models/export/renderDiagram';
import { exportPdf } from '../pages/Schema/models/export/exportPdf';
import type { ModelDefinition } from '../pages/Schema/models/modelGraph';
import { fixtureTables, modelFixture } from './fixtures/model-fixture';

// The canvas as plain lists, as in model-editor.test.ts: jsdom cannot measure
// React Flow's nodes.
const flow = vi.hoisted(() => ({ props: null as any }));

vi.mock('@xyflow/react', async () => {
  const actual = await vi.importActual<typeof import('@xyflow/react')>('@xyflow/react');
  const React = await import('react');
  const h = React.createElement;
  return {
    getNodesBounds: actual.getNodesBounds,
    ReactFlow: (props: any) => {
      flow.props = props;
      return h(
        'div',
        { 'data-testid': 'flow' },
        h('ul', null, props.nodes.map((n: any) => h(
          'li',
          { key: n.id, 'data-node': n.id, className: 'react-flow__node', 'data-id': n.id },
          h(props.nodeTypes[n.type], { id: n.id, data: n.data, selected: n.selected })
        ))),
        props.children
      );
    },
    ReactFlowProvider: ({ children }: any) => children,
    ViewportPortal: ({ children }: any) => h('div', { 'data-testid': 'viewport-portal' }, children),
    Handle: ({ type, id }: any) => h('span', { 'data-handle': type, 'data-id': id }),
    NodeResizer: () => null,
    NodeResizeControl: () => null,
    useUpdateNodeInternals: () => () => undefined,
    MiniMap: () => null,
    BaseEdge: ({ id }: any) => h('path', { 'data-edge-path': id }),
    EdgeLabelRenderer: ({ children }: any) => h('div', null, children),
    useInternalNode: () => ({ internals: { positionAbsolute: { x: 0, y: 0 } }, measured: { width: 160 } }),
    ConnectionMode: { Strict: 'strict', Loose: 'loose' },
    Position: {
      Left: 'left', Right: 'right', Top: 'top', Bottom: 'bottom',
    },
    Background: () => null,
    Controls: () => null,
    useReactFlow: () => ({
      screenToFlowPosition: (p: { x: number; y: number }) => p,
      zoomIn: () => undefined,
      zoomOut: () => undefined,
      zoomTo: () => undefined,
      setCenter: () => undefined,
      getZoom: () => 1,
      getInternalNode: () => undefined,
      fitView: () => undefined,
    }),
    applyNodeChanges: (_changes: unknown, nodes: unknown) => nodes,
  };
});

// jsdom has no canvas: the renderers are stand-ins.
vi.mock('../pages/Schema/models/export/renderDiagram', () => ({
  renderDiagram: vi.fn(async () => 'data:image/png;base64,AAAA'),
}));
vi.mock('../pages/Schema/models/export/exportPdf', () => ({
  exportPdf: vi.fn(async () => undefined),
}));

(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;
(globalThis as any).ResizeObserver ??= class {
  observe() {}

  unobserve() {}

  disconnect() {}
};

const node = (id: string, x: number, y: number, width: number, height: number, selected = false): any => ({
  id, position: { x, y }, width, height, selected, data: {},
});

describe('page geometry', () => {
  it('pageSizeMm defaults to A4 landscape and swaps for portrait', () => {
    expect(pageSizeMm(undefined)).toEqual({ width: 297, height: 210 });
    expect(pageSizeMm({ orientation: 'portrait' })).toEqual({ width: 210, height: 297 });
    expect(pageSizeMm({ pageSize: 'Letter', orientation: 'portrait' })).toEqual({ width: 215.9, height: 279.4 });
    expect(pageSizeMm({ pageSize: 'A3' })).toEqual({ width: 420, height: 297 });
    expect(pageSizeMm({ pageSize: 'Legal' })).toEqual({ width: 355.6, height: 215.9 });
    expect(mmToPx(25.4)).toBeCloseTo(96, 10);
  });

  it('pageGrid splits the bounds into printable pages', () => {
    const w = mmToPx(277);
    const h = mmToPx(190);
    expect(w).toBeCloseTo(1046.9, 1);
    expect(h).toBeCloseTo(718.1, 1);
    const pages = pageGrid({
      x: 0, y: 0, width: 1500, height: 800,
    }, undefined);
    expect(pages).toHaveLength(4);
    expect(pages[0]).toEqual({
      x: 0, y: 0, width: w, height: h,
    });
    expect(pages[1].x).toBeCloseTo(1046.9, 1);
    expect(pages[1].y).toBe(0);
    expect(pages[2].x).toBe(0);
    expect(pages[2].y).toBeCloseTo(718.1, 1);
    // The grid starts at the bounds' corner.
    const shifted = pageGrid({
      x: -50, y: 30, width: 100, height: 100,
    }, { margin: 0, orientation: 'portrait' });
    expect(shifted).toEqual([{
      x: -50, y: 30, width: mmToPx(210), height: mmToPx(297),
    }]);
  });

  it('diagramBounds is null for an empty selection', () => {
    const nodes = [node('a', 0, 0, 100, 50), node('b', 200, 100, 100, 50, true)];
    expect(diagramBounds(nodes.map((n) => ({ ...n, selected: false })), 'selection')).toBeNull();
    expect(diagramBounds(nodes, 'all')).toEqual({
      x: -20, y: -20, width: 340, height: 190,
    });
    expect(diagramBounds(nodes, 'selection')).toEqual({
      x: 180, y: 80, width: 140, height: 90,
    });
    const blank = diagramBounds([], 'all');
    expect(blank).toEqual({
      x: 0, y: 0, width: 40, height: 40,
    });
    expect(pageGrid(blank!, undefined)).toHaveLength(1);
  });
});

const view = (definition: ModelDefinition, version = 3): CubeModelView => ({
  id: 'm1',
  organizationId: 'o1',
  name: 'Sales',
  definition,
  cubeNames: [],
  version,
  updatedAt: '2026-11-25T00:00:00Z',
  updatedBy: null,
  dialects: { warehouse: 'postgres' },
});

describe('model export', () => {
  let root: ReturnType<typeof createRoot>;
  let api: Record<string, ReturnType<typeof vi.fn>>;

  beforeEach(() => {
    window.localStorage.clear();
    Element.prototype.hasPointerCapture = vi.fn(() => false) as any;
    Element.prototype.scrollIntoView = vi.fn() as any;
    vi.mocked(renderDiagram).mockClear();
    vi.mocked(exportPdf).mockClear();
    api = {
      model: vi.fn(async () => view(modelFixture())),
      saveModel: vi.fn(async (_id: string, change: any) => view(change.definition, change.version + 1)),
      buildModel: vi.fn(),
      deleteModel: vi.fn(),
      createTables: vi.fn(),
      modelSql: vi.fn(async () => ({ script: '' })),
      lockModel: vi.fn(async () => ({ held: true, holder: 'me' })),
      takeModelLock: vi.fn(async () => ({ held: true, holder: 'me' })),
      releaseModelLock: vi.fn(async () => undefined),
    };
    const host = document.createElement('div');
    document.body.appendChild(host);
    root = createRoot(host);
  });

  afterEach(() => {
    act(() => root.unmount());
    document.body.innerHTML = '';
    flow.props = null;
    vi.restoreAllMocks();
  });

  const render = async () => {
    const props: ModelEditorProps = {
      api: api as any,
      org: { id: 'o1', slug: 'acme', name: 'Acme' } as any,
      modelId: 'm1',
      tables: fixtureTables,
      onBuilt: vi.fn(),
      onDeleted: vi.fn(),
      onTablesCreated: vi.fn(),
    };
    await act(async () => root.render(createElement(ModelEditor as any, props)));
  };
  const tool = (label: string) => document.querySelector(
    `[data-slot="model-toolbar"] button[aria-label="${label}"]`
  ) as HTMLButtonElement;
  const dialog = () => document.querySelector('[role="dialog"]') as HTMLElement | null;
  const inDialog = (text: string) => [...dialog()!.querySelectorAll('button')]
    .find((b) => b.textContent!.trim() === text) as HTMLButtonElement;
  const radio = (label: string) => dialog()!.querySelector(`[role="radio"][aria-label="${label}"]`) as HTMLButtonElement;
  const select = async (id: string) => {
    await act(async () => flow.props.onNodesChange(flow.props.nodes.map((n: any) => ({
      type: 'select', id: n.id, selected: n.id === id,
    }))));
  };
  const anchors = () => {
    const clicked: HTMLAnchorElement[] = [];
    vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function click(this: HTMLAnchorElement) {
      clicked.push(this);
    });
    return clicked;
  };
  const pick = async (label: string, value: string) => {
    const el = document.querySelector(`select[aria-label="${label}"]`) as HTMLSelectElement;
    await act(async () => {
      el.value = value;
      el.dispatchEvent(new Event('change', { bubbles: true }));
    });
  };
  const section = (title: string) => [...document.querySelectorAll('[data-slot="model-properties"] section > button')]
    .find((b) => b.textContent!.trim() === title) as HTMLButtonElement;

  it('the toolbar button downloads {modelName}-documentation.html', async () => {
    await render();
    const clicked = anchors();
    const blobs: Blob[] = [];
    URL.createObjectURL = ((b: Blob) => { blobs.push(b); return 'blob:docs'; }) as any;
    URL.revokeObjectURL = (() => undefined) as any;
    await act(async () => tool('Generate model documentation').click());
    expect(clicked.map((a) => a.download)).toEqual(['Sales-documentation.html']);
    expect(blobs[0].type).toBe('text/html');
    expect(vi.mocked(renderDiagram).mock.calls[0][2]).toBe('svg');
    expect(tool('Undo').disabled).toBe(true);
    expect(api.saveModel).not.toHaveBeenCalled();
  });

  it('export dialog: format, scope and download', async () => {
    await render();
    await act(async () => tool('Export model to image').click());
    expect(dialog()!.textContent).toContain('Export model to image');
    expect(radio('PNG')).not.toBeNull();
    expect(radio('SVG')).not.toBeNull();
    expect(radio('Whole diagram')).not.toBeNull();
    expect(radio('Selection')).not.toBeNull();
    // Nothing is selected: only the whole diagram.
    expect(radio('Selection').disabled).toBe(true);

    const clicked = anchors();
    await act(async () => inDialog('Export').click());
    expect(renderDiagram).toHaveBeenCalledTimes(1);
    expect(vi.mocked(renderDiagram).mock.calls[0][2]).toBe('png');
    expect(clicked.map((a) => a.download)).toEqual(['Sales.png']);
    expect(clicked[0].href).toBe('data:image/png;base64,AAAA');
    // The dialog closes once the file is on its way.
    expect(dialog()).toBeNull();

    await select('orders');
    await act(async () => tool('Export model to image').click());
    expect(radio('Selection').disabled).toBe(false);
    await act(async () => radio('SVG').click());
    await act(async () => radio('Selection').click());
    await act(async () => inDialog('Export').click());
    expect(vi.mocked(renderDiagram).mock.calls[1][2]).toBe('svg');
    expect(clicked.map((a) => a.download)).toEqual(['Sales.png', 'Sales.svg']);
    // Exports change nothing.
    expect(tool('Undo').disabled).toBe(true);
    expect(api.saveModel).not.toHaveBeenCalled();
  });

  it('exports stay on read-only; PDF uses the print settings', async () => {
    api.lockModel.mockResolvedValue({ held: false, holder: 'bob' });
    api.model.mockResolvedValue(view({ ...modelFixture(), settings: { print: { pageSize: 'Legal' } } }));
    await render();
    expect(tool('Undo').disabled).toBe(true);
    expect(tool('Export model to image').disabled).toBe(false);
    expect(tool('Export model to PDF file').disabled).toBe(false);
    await act(async () => tool('Export model to PDF file').click());
    expect(exportPdf).toHaveBeenCalledTimes(1);
    const [, bounds, print, fileName] = vi.mocked(exportPdf).mock.calls[0];
    expect(bounds).toEqual(expect.objectContaining({ width: expect.any(Number) }));
    expect(print).toEqual({ pageSize: 'Legal' });
    expect(fileName).toBe('Sales.pdf');
  });

  it('print settings edit the definition', async () => {
    await render();
    await act(async () => section('Print settings').click());
    const sizeField = document.querySelector('select[aria-label="Page size"]') as HTMLSelectElement;
    expect(sizeField.value).toBe('A4');
    expect((document.querySelector('select[aria-label="Orientation"]') as HTMLSelectElement).value).toBe('landscape');
    expect((document.querySelector('input[aria-label="Margins (mm)"]') as HTMLInputElement).value).toBe('10');
    await pick('Page size', 'A3');
    await pick('Orientation', 'portrait');
    expect(document.querySelectorAll('[data-slot="model-page-break"]')).toHaveLength(0);
    const breaks = document.querySelector('input[aria-label="Show page breaks"]') as HTMLInputElement;
    await act(async () => breaks.click());
    expect(document.querySelectorAll('[data-slot="model-page-break"]').length).toBeGreaterThan(0);

    await act(async () => tool('Save changes').click());
    expect(api.saveModel.mock.calls[0][1].definition.settings.print)
      .toEqual({ pageSize: 'A3', orientation: 'portrait', showPageBreaks: true });

    // Each field is one step of undo.
    await act(async () => tool('Undo').click());
    expect(document.querySelectorAll('[data-slot="model-page-break"]')).toHaveLength(0);
    await act(async () => tool('Undo').click());
    expect((document.querySelector('select[aria-label="Orientation"]') as HTMLSelectElement).value).toBe('landscape');
    expect((document.querySelector('select[aria-label="Page size"]') as HTMLSelectElement).value).toBe('A3');

    // The margin stays within 0 to 50 mm.
    const margin = document.querySelector('input[aria-label="Margins (mm)"]') as HTMLInputElement;
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!;
    await act(async () => {
      setter.call(margin, '60');
      margin.dispatchEvent(new Event('input', { bubbles: true }));
    });
    await act(async () => {
      margin.dispatchEvent(new FocusEvent('focusout', { bubbles: true }));
    });
    expect(margin.value).toBe('50');
    await act(async () => tool('Save changes').click());
    expect(api.saveModel.mock.calls[1][1].definition.settings.print).toEqual({ pageSize: 'A3', margin: 50 });
  });
});
