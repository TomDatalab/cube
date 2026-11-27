// @vitest-environment jsdom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import {
  afterEach, beforeEach, describe, expect, it, vi,
} from 'vitest';

import type { CubeModelView } from '../pages/Schema/api';
import { ModelEditor, ModelEditorProps } from '../pages/Schema/models/ModelEditor';
import { explorerTables } from '../pages/Schema/models/explorer/explorerTables';
import { runQuery } from '../pages/Schema/models/explorer/runQuery';
import { toCsv } from '../pages/Schema/models/explorer/csv';
import type { ModelDefinition } from '../pages/Schema/models/modelGraph';
import { fixtureColumnsOf, fixtureTables, modelFixture } from './fixtures/model-fixture';

// The canvas as plain lists, as in model-editor.test.ts: jsdom cannot measure
// React Flow's nodes.
const flow = vi.hoisted(() => ({ props: null as any, setCenter: null as any }));

vi.mock('@xyflow/react', async () => {
  const React = await import('react');
  const h = React.createElement;
  return {
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
    Handle: ({ type, id }: any) => h('span', { 'data-handle': type, 'data-id': id }),
    NodeResizer: () => null,
    NodeResizeControl: () => null,
    useUpdateNodeInternals: () => () => undefined,
    MiniMap: () => null,
    BaseEdge: ({ id }: any) => h('path', { 'data-edge-path': id }),
    EdgeLabelRenderer: ({ children }: any) => h('div', null, children),
    useInternalNode: () => ({ internals: { positionAbsolute: { x: 0, y: 0 } }, measured: { width: 160 } }),
    ConnectionMode: { Strict: 'strict', Loose: 'loose' },
    Position: { Left: 'left', Right: 'right', Top: 'top', Bottom: 'bottom' },
    Background: () => null,
    Controls: () => null,
    useReactFlow: () => ({
      screenToFlowPosition: (p: { x: number; y: number }) => p,
      zoomIn: () => undefined,
      zoomOut: () => undefined,
      zoomTo: () => undefined,
      setCenter: flow.setCenter,
      getZoom: () => 1,
      getInternalNode: () => undefined,
      fitView: () => undefined,
    }),
    applyNodeChanges: (_changes: unknown, nodes: unknown) => nodes,
  };
});

(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;
(globalThis as any).ResizeObserver ??= class {
  observe() {}

  unobserve() {}

  disconnect() {}
};

const NAMES = [
  'tables', 'columns', 'primary_keys', 'unique_keys', 'indexes', 'checks', 'foreign_keys',
  'views', 'view_columns', 'areas', 'notes', 'cubes', 'dimensions', 'measures',
];

describe('model explorer', () => {
  it('explorerTables lists every object kind', () => {
    const tables = explorerTables(modelFixture(), fixtureColumnsOf);
    expect(tables.map((t) => t.name)).toEqual(NAMES);
    const of = (name: string) => tables.find((t) => t.name === name)!;
    expect(of('tables').columns).toEqual(['node', 'alias', 'schema', 'name', 'comment', 'is_draft']);
    expect(of('tables').rows).toEqual([
      ['orders', 'orders', 'public', 'orders', 'One row per order', 1],
      ['customers', 'customers', 'public', 'customers', null, 0],
    ]);
    expect(of('columns').columns)
      .toEqual(['node', 'table_alias', 'name', 'type', 'nullable', 'primary_key', 'comment', 'default_value']);
    expect(of('columns').rows[0]).toEqual(['orders', 'orders', 'id', 'integer', 0, 1, null, null]);
    expect(of('columns').rows[2]).toEqual(['orders', 'orders', 'email', 'text', 1, 0, 'Who to write to', "''"]);
    const existing = of('columns').rows.filter((r) => r[1] === 'customers');
    expect(existing.map((r) => [r[2], r[3]])).toEqual([['id', 'integer'], ['name', 'text']]);
    expect(existing.every((r) => r[4] === null && r[5] === null && r[6] === null && r[7] === null)).toBe(true);
    expect(of('primary_keys').rows).toEqual([['orders', 'orders', 'orders_pk', 'id']]);
    expect(of('unique_keys').rows).toEqual([['orders', 'orders', 'orders_email_uq', 'email, customer_id']]);
    expect(of('indexes').columns).toEqual(['node', 'table_alias', 'name', 'columns', 'is_unique']);
    expect(of('indexes').rows).toEqual([['orders', 'orders', 'orders_customer_idx', 'customer_id', 0]]);
    expect(of('checks').rows).toEqual([['orders', 'orders', 'orders_amount_ck', 'amount >= 0']]);
    expect(of('foreign_keys').columns)
      .toEqual(['node', 'name', 'from_table', 'from_column', 'to_table', 'to_column', 'cardinality', 'mandatory']);
    expect(of('foreign_keys').rows)
      .toEqual([['orders', 'orders_customer_fk', 'orders', 'customer_id', 'customers', 'id', '0..*', 0]]);
    expect(of('views').rows).toEqual([[
      'view:v1', 'big_orders', 'public', 'Orders over 100', 'SELECT * FROM public.orders WHERE amount > 100',
    ]]);
    expect(of('view_columns').rows).toEqual([
      ['view:v1', 'big_orders', 'id', 'integer'],
      ['view:v1', 'big_orders', 'amount', 'numeric'],
    ]);
    expect(of('areas').rows).toEqual([['area:a1', 'Sales area', 'Where money is']]);
    expect(of('notes').rows).toEqual([['note:n1', 'Check the amounts']]);
    expect(of('cubes').columns).toEqual(['node', 'name', 'title', 'source_kind', 'source_table', 'sql']);
    expect(of('cubes').rows[0][0]).toBe('cube:c1');
    expect(of('cubes').rows).toEqual([['cube:c1', 'sales', 'Sales', 'table', 'orders', null]]);
    expect(of('dimensions').columns).toEqual(['node', 'cube_name', 'name', 'type', 'column_name', 'sql', 'primary_key']);
    expect(of('dimensions').rows).toEqual([['cube:c1', 'sales', 'id', 'number', 'id', null, 1]]);
    expect(of('measures').columns).toEqual(['node', 'cube_name', 'name', 'type', 'column_name', 'sql', 'format']);
    expect(of('measures').rows).toEqual([['cube:c1', 'sales', 'total', 'sum', 'amount', null, 'currency']]);
  });

  it('explorerTables gives an empty model its 14 empty tables', () => {
    const tables = explorerTables({ dataSource: '', tables: [], joins: [] }, () => []);
    expect(tables.map((t) => t.name)).toEqual(NAMES);
    expect(tables.every((t) => t.rows.length === 0 && t.columns[0] === 'node')).toBe(true);
  });

  it('runQuery runs SQL over the model', async () => {
    const tables = explorerTables(modelFixture(), fixtureColumnsOf);
    const names = await runQuery(tables, "SELECT name FROM columns WHERE table_alias = 'orders' ORDER BY name");
    expect(names).toEqual({ columns: ['name'], rows: [['amount'], ['customer_id'], ['email'], ['id']] });
    const joined = await runQuery(
      tables,
      'SELECT c.name AS cube, c.title, d.name FROM dimensions d JOIN cubes c ON c.node = d.node'
    );
    expect(joined).toEqual({ columns: ['cube', 'title', 'name'], rows: [['sales', 'Sales', 'id']] });
    // The last statement answers, with its columns even when it has no row.
    const last = await runQuery(tables, "SELECT 1; SELECT node FROM notes WHERE text = 'none'");
    expect(last).toEqual({ columns: ['node'], rows: [] });
  });

  it('runQuery rejects with the SQLite message', async () => {
    await expect(runQuery(explorerTables(modelFixture(), fixtureColumnsOf), 'SELECT * FROM nope'))
      .rejects.toThrow(/no such table: nope/);
  });

  it('toCsv quotes per RFC 4180', () => {
    expect(toCsv(['a', 'b'], [['x,y', 'say "hi"'], [null, 3]])).toBe('a,b\r\n"x,y","say ""hi"""\r\n,3\r\n');
    expect(toCsv(['a'], [['two\nlines'], ['cr\rhere']])).toBe('a\r\n"two\nlines"\r\n"cr\rhere"\r\n');
  });
});

const view = (definition: ModelDefinition): CubeModelView => ({
  id: 'm1',
  organizationId: 'o1',
  name: 'Sales',
  definition,
  cubeNames: [],
  version: 3,
  updatedAt: '2026-11-25T00:00:00Z',
  updatedBy: null,
  dialects: { warehouse: 'postgres' },
});

describe('explorer dialog', () => {
  let root: ReturnType<typeof createRoot>;
  let api: Record<string, ReturnType<typeof vi.fn>>;

  beforeEach(() => {
    window.localStorage.clear();
    Element.prototype.hasPointerCapture = vi.fn(() => false) as any;
    Element.prototype.scrollIntoView = vi.fn() as any;
    flow.setCenter = vi.fn();
    api = {
      model: vi.fn(async () => view(modelFixture())),
      saveModel: vi.fn(),
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
  const button = (text: string) => [...document.querySelectorAll('button')]
    .find((b) => b.textContent!.trim() === text) as HTMLButtonElement;
  const dialog = () => document.querySelector('[role="dialog"]') as HTMLElement | null;
  const textarea = () => dialog()!.querySelector('textarea') as HTMLTextAreaElement;
  const gridRows = () => [...(dialog()?.querySelectorAll('[data-slot="model-explorer-grid"] tbody tr') ?? [])];
  const flowNode = (id: string) => flow.props.nodes.find((n: any) => n.id === id);
  // sql.js loads and answers asynchronously.
  const until = async (ok: () => boolean) => {
    for (let i = 0; i < 200 && !ok(); i++) {
      // eslint-disable-next-line no-await-in-loop
      await act(async () => new Promise((r) => { setTimeout(r, 10); }));
    }
    expect(ok()).toBe(true);
  };
  const type = async (value: string) => {
    const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')!.set!;
    await act(async () => {
      setter.call(textarea(), value);
      textarea().dispatchEvent(new Event('input', { bubbles: true }));
    });
  };
  const select = async (id: string) => {
    await act(async () => flow.props.onNodesChange(flow.props.nodes.map((n: any) => ({
      type: 'select', id: n.id, selected: n.id === id,
    }))));
  };

  it('explorer dialog: run, error, find, export', async () => {
    await render();
    await select('orders');
    await act(async () => tool('Model explorer').click());
    expect(dialog()!.textContent).toContain('Model explorer');
    expect(textarea().value).toBe('SELECT * FROM tables');
    // The default query runs on open.
    await until(() => gridRows().length === 2);
    expect(gridRows()[0].textContent).toContain('orders');

    // Typing does not reach the canvas.
    await act(async () => {
      textarea().dispatchEvent(new KeyboardEvent('keydown', { key: 'Delete', bubbles: true }));
    });
    expect(flowNode('orders')).toBeDefined();
    expect(flowNode('orders').selected).toBe(true);

    // The chips write a query over each table.
    const chips = [...dialog()!.querySelectorAll('[data-slot="model-explorer-tables"] button')]
      .map((b) => b.textContent);
    expect(chips).toEqual(NAMES);

    await type('SELECT * FROM nope');
    await act(async () => button('Run').click());
    await until(() => dialog()!.textContent!.includes('no such table: nope'));
    expect(gridRows()).toHaveLength(0);

    await type('SELECT name FROM cubes');
    await act(async () => {
      textarea().dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', ctrlKey: true, bubbles: true }));
    });
    await until(() => gridRows().length === 1);
    expect(dialog()!.textContent).not.toContain('no such table');
    await act(async () => (gridRows()[0] as HTMLElement).click());
    expect(button('Find in diagram').disabled).toBe(true);

    await act(async () => (dialog()!.querySelector('[data-slot="model-explorer-tables"] button:nth-child(12)') as HTMLButtonElement).click());
    expect(textarea().value).toBe('SELECT * FROM cubes');
    await act(async () => button('Run').click());
    await until(() => gridRows().length === 1 && gridRows()[0].textContent!.includes('cube:c1'));
    expect(button('Find in diagram').disabled).toBe(true);
    await act(async () => (gridRows()[0] as HTMLElement).click());
    expect(gridRows()[0].getAttribute('aria-selected')).toBe('true');

    // Export: a Blob through an anchor named after the model.
    const blobs: Blob[] = [];
    (URL as any).createObjectURL = vi.fn((b: Blob) => {
      blobs.push(b);
      return 'blob:explorer';
    });
    (URL as any).revokeObjectURL = vi.fn();
    const anchors: HTMLAnchorElement[] = [];
    vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function click(this: HTMLAnchorElement) {
      anchors.push(this);
    });
    await act(async () => button('Export to CSV').click());
    expect(URL.createObjectURL).toHaveBeenCalledTimes(1);
    expect(anchors).toHaveLength(1);
    expect(anchors[0].download).toBe('Sales-explorer.csv');
    expect(anchors[0].href).toBe('blob:explorer');
    const csv = await new Promise<string>((resolve) => {
      const reader = new FileReader();
      reader.onload = () => resolve(reader.result as string);
      reader.readAsText(blobs[0]);
    });
    expect(csv).toBe('node,name,title,source_kind,source_table,sql\r\ncube:c1,sales,Sales,table,orders,\r\n');

    await act(async () => button('Find in diagram').click());
    expect(flowNode('cube:c1').selected).toBe(true);
    expect(flowNode('orders').selected).toBeFalsy();
    expect(flow.setCenter).toHaveBeenCalled();
    // The explorer changes nothing.
    expect(api.saveModel).not.toHaveBeenCalled();
  });

  it('explorer stays open to a read-only editor', async () => {
    api.lockModel.mockResolvedValue({ held: false, holder: 'bob' });
    await render();
    expect(tool('Undo').disabled).toBe(true);
    expect(tool('Model explorer').disabled).toBe(false);
    await act(async () => tool('Model explorer').click());
    await until(() => gridRows().length === 2);
  });
});
