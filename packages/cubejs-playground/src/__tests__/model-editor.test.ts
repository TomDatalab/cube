// @vitest-environment jsdom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import {
  afterEach, beforeEach, describe, expect, it, Mock, vi,
} from 'vitest';

import { notify } from '@/components/kit';
import { ApiError, CubeModelView } from '../pages/Schema/api';
import { ModelEditor, ModelEditorProps } from '../pages/Schema/models/ModelEditor';
import { TableNode } from '../pages/Schema/models/TableNode';
import type { ModelDefinition } from '../pages/Schema/models/modelGraph';

// React Flow measures its nodes before it draws edges, which jsdom cannot do:
// the canvas renders its nodes (with the editor's own node component) and its
// edges as plain lists, and keeps the props so a test can connect two handles.
const flow = vi.hoisted(() => ({
  props: null as any,
  zoomIn: null as any,
  zoomOut: null as any,
  zoomTo: null as any,
  setCenter: null as any,
  updated: [] as string[],
  fitView: null as any,
}));

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
        h('ul', null, props.edges.map((e: any) => h(
          'li',
          { key: e.id },
          h('button', {
            type: 'button', 'data-edge': e.id, onClick: (ev: any) => props.onEdgeClick?.(ev, e),
          }, e.label)
        ))),
        props.children
      );
    },
    ReactFlowProvider: ({ children }: any) => children,
    Handle: ({ type, id }: any) => h('span', { 'data-handle': type, 'data-id': id }),
    NodeResizer: () => null,
    useUpdateNodeInternals: () => (id: string) => flow.updated.push(id),
    MiniMap: () => null,
    BaseEdge: () => null,
    EdgeLabelRenderer: () => null,
    ConnectionMode: { Strict: 'strict', Loose: 'loose' },
    Position: { Left: 'left', Right: 'right', Top: 'top', Bottom: 'bottom' },
    Background: () => null,
    Controls: () => null,
    useReactFlow: () => ({
      screenToFlowPosition: (p: { x: number; y: number }) => p,
      zoomIn: flow.zoomIn,
      zoomOut: flow.zoomOut,
      zoomTo: flow.zoomTo,
      setCenter: flow.setCenter,
      getZoom: () => 1,
      getInternalNode: () => undefined,
      fitView: flow.fitView,
    }),
    applyNodeChanges: (_changes: unknown, nodes: unknown) => nodes,
  };
});

(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;
// The color popover measures its trigger.
(globalThis as any).ResizeObserver ??= class {
  observe() {}

  unobserve() {}

  disconnect() {}
};

const tables = {
  warehouse: {
    public: {
      orders: [
        { name: 'id', type: 'integer' },
        { name: 'customer_id', type: 'integer' },
        { name: 'amount', type: 'numeric' },
      ],
      customers: [
        { name: 'id', type: 'integer' },
        { name: 'name', type: 'text' },
      ],
      products: [{ name: 'id', type: 'integer' }],
    },
  },
  other: { public: { events: [{ name: 'id', type: 'integer' }] } },
  lake: { main: { logs: [{ name: 'id', type: 'integer' }] } },
};

const saved = (): ModelDefinition => ({
  dataSource: 'warehouse',
  tables: [
    {
      alias: 'orders', schema: 'public', table: 'orders', x: 0, y: 0,
    },
    {
      alias: 'customers', schema: 'public', table: 'customers', x: 300, y: 0,
    },
  ],
  primary: 'orders',
  primaryKey: 'id',
  joins: [{
    from: { alias: 'orders', column: 'customer_id' },
    to: { alias: 'customers', column: 'id' },
    relationship: 'many_to_one',
    join: 'left',
  }],
});

// Three tables in a row: orders at 0, customers at 300, products at 900.
const threeTables = (): ModelDefinition => ({
  ...saved(),
  tables: [...saved().tables, {
    alias: 'products', schema: 'public', table: 'products', x: 900, y: 0,
  }],
});

const view = (definition: ModelDefinition, version = 3): CubeModelView => ({
  id: 'm1',
  organizationId: 'o1',
  name: 'Sales',
  definition,
  cubeName: null,
  version,
  updatedAt: '2026-11-25T00:00:00Z',
  updatedBy: null,
  // The fixture's connection designs Postgres tables.
  dialects: { warehouse: 'postgres' },
});

const built = { fileName: 'model/sales.yml', content: '', cubeName: 'sales', detail: {} as any };

describe('ModelEditor', () => {
  let root: ReturnType<typeof createRoot>;
  let api: Record<string, ReturnType<typeof vi.fn>>;
  let onBuilt: Mock<(cubeName: string, detail: any) => void>;
  let onDeleted: Mock<() => void>;
  let onTablesCreated: Mock<(detail: any) => void>;

  beforeEach(() => {
    window.localStorage.clear();
    Element.prototype.hasPointerCapture = vi.fn(() => false) as any;
    Element.prototype.scrollIntoView = vi.fn() as any;
    flow.zoomIn = vi.fn();
    flow.zoomOut = vi.fn();
    flow.zoomTo = vi.fn();
    flow.updated = [];
    flow.setCenter = vi.fn();
    flow.fitView = vi.fn();
    api = {
      model: vi.fn(async () => view(saved())),
      saveModel: vi.fn(async (_id: string, change: any) => view(change.definition, change.version + 1)),
      buildModel: vi.fn(async () => built),
      deleteModel: vi.fn(async () => undefined),
      createTables: vi.fn(async () => ({})),
      modelSql: vi.fn(async () => ({ script: 'CREATE TABLE "public"."orders" ();\n' })),
    };
    onBuilt = vi.fn<(cubeName: string, detail: any) => void>();
    onDeleted = vi.fn<() => void>();
    onTablesCreated = vi.fn<(detail: any) => void>();
    const host = document.createElement('div');
    document.body.appendChild(host);
    root = createRoot(host);
  });

  afterEach(() => {
    act(() => root.unmount());
    document.body.innerHTML = '';
    document.documentElement.removeAttribute('data-theme');
    flow.props = null;
  });

  const render = async () => {
    const props: ModelEditorProps = {
      api: api as any,
      org: { id: 'o1', slug: 'acme', name: 'Acme' } as any,
      modelId: 'm1',
      tables,
      onBuilt,
      onDeleted,
      onTablesCreated,
    };
    // tsc's createElement inference gives up on this large file now and then;
    // the props are typed above.
    await act(async () => root.render(createElement(ModelEditor as any, props)));
  };
  const button = (text: string | RegExp) => [...document.querySelectorAll('button')]
    .find((b) => (typeof text === 'string' ? b.textContent!.trim() === text : text.test(b.textContent!))) as HTMLButtonElement;
  const rename = async (value: string) => {
    const input = document.querySelector('input[aria-label="Model name"]') as HTMLInputElement;
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!;
    await act(async () => {
      setter.call(input, value);
      input.dispatchEvent(new Event('input', { bubbles: true }));
    });
  };
  const tool = (label: string) => document.querySelector([
    `[data-slot="model-toolbar"] button[aria-label="${label}"]`,
    `[data-slot="model-tool-palette"] button[aria-label="${label}"]`,
  ].join(', ')) as HTMLButtonElement;
  // A menu of the toolbar: opens it and returns its item.
  const menuItem = async (menu: string, item: string) => {
    const trigger = tool(menu);
    await act(async () => trigger.click());
    return document.querySelector(`[role="menuitem"][aria-label="${item}"]`) as HTMLElement;
  };
  const choose = async (menu: string, item: string) => {
    const found = await menuItem(menu, item);
    await act(async () => found.click());
  };
  const selectMany = async (ids: string[]) => {
    // As React Flow reports a user's selection: one change per node.
    await act(async () => flow.props.onNodesChange(flow.props.nodes.map((n: any) => ({
      type: 'select', id: n.id, selected: ids.includes(n.id),
    }))));
  };
  const fill = async (selector: string, value: string) => {
    const input = document.querySelector(selector) as HTMLInputElement;
    const proto = input instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    const setter = Object.getOwnPropertyDescriptor(proto, 'value')!.set!;
    await act(async () => {
      setter.call(input, value);
      input.dispatchEvent(new Event('input', { bubbles: true }));
    });
  };
  const select = async (alias: string | null) => {
    await act(async () => flow.props.onNodesChange(flow.props.nodes.map((n: any) => ({
      type: 'select', id: n.id, selected: n.id === alias,
    }))));
  };
  const node = (alias: string) => document.querySelector(`[data-node="${alias}"]`) as HTMLElement;
  const panelItem = (name: string) => [...document.querySelectorAll('[data-slot="model-table-item"]')]
    .find((li) => li.textContent!.trim() === name) as HTMLElement;

  it('shows the saved tables, the principal and each link label', async () => {
    await render();
    expect(api.model).toHaveBeenCalledWith('m1');
    const nodes = [...document.querySelectorAll('[data-node]')];
    expect(nodes.map((n) => n.getAttribute('data-node'))).toEqual(['orders', 'customers']);
    expect(nodes[0].textContent).toContain('principal');
    expect(nodes[1].textContent).not.toContain('principal');
    // A handle on each side of every column, named after it.
    expect(nodes[0].querySelectorAll('[data-handle="source"][data-id="customer_id"]').length).toBe(1);
    expect(nodes[0].querySelectorAll('[data-handle="target"][data-id="customer_id"]').length).toBe(1);
    expect(nodes[0].textContent).toContain('numeric');
    expect([...document.querySelectorAll('[data-edge]')].map((e) => e.textContent)).toEqual(['N:1 · LEFT']);
  });

  it('builds a saved model and hands back the cube name', async () => {
    await render();
    await act(async () => tool('Build cube').click());
    expect(api.saveModel).not.toHaveBeenCalled();
    expect(api.buildModel).toHaveBeenCalledTimes(1);
    expect(api.buildModel.mock.calls[0][0]).toBe('m1');
    expect(api.buildModel.mock.calls[0][1]).toBeFalsy();
    expect(onBuilt).toHaveBeenCalledWith('sales', built.detail);
  });

  it('asks before overwriting a cube edited by hand, then builds with force', async () => {
    api.buildModel.mockRejectedValueOnce(new ApiError('O cubo foi editado desde o último build', 409));
    await render();
    await act(async () => tool('Build cube').click());
    expect(onBuilt).not.toHaveBeenCalled();
    expect(document.body.textContent).toContain('The cube was edited by hand since the last build. Overwrite?');
    await act(async () => button('Overwrite').click());
    expect(api.buildModel).toHaveBeenCalledTimes(2);
    expect(api.buildModel.mock.calls[1]).toEqual(['m1', true]);
    expect(onBuilt).toHaveBeenCalledWith('sales', built.detail);
  });

  it('shows the server reason for a duplicate name, not the stale hint', async () => {
    api.saveModel.mockRejectedValueOnce(new ApiError('Já existe um model com este nome nesta organização', 409));
    const error = vi.spyOn(notify, 'error');
    await render();
    await rename('Outro');
    await act(async () => tool('Save changes').click());
    expect(error).toHaveBeenCalledWith('Já existe um model com este nome nesta organização');
    api.saveModel.mockRejectedValueOnce(new ApiError('O model mudou desde que você abriu; abra de novo', 409));
    await act(async () => tool('Save changes').click());
    expect(error).toHaveBeenLastCalledWith('Changed elsewhere: reopen');
    error.mockRestore();
  });

  it('lists the problems of a build the ModelWriter refused', async () => {
    api.buildModel.mockRejectedValueOnce(new ApiError('A mudança não compila', 400, [
      { file: 'model/sales.yml', cube: 'sales', message: 'coluna inexistente' },
    ]));
    await render();
    await act(async () => tool('Build cube').click());
    const box = document.querySelector('[data-slot="model-problems"]')!;
    expect(box.textContent).toContain('model/sales.yml: coluna inexistente');
    expect(onBuilt).not.toHaveBeenCalled();
    await act(async () => tool('Build cube').click());
    expect(document.querySelector('[data-slot="model-problems"]')).toBeNull();
  });

  it('does not build while a table is not linked, and says which', async () => {
    api.model.mockResolvedValueOnce(view({
      ...saved(),
      tables: [...saved().tables, {
        alias: 'products', schema: 'public', table: 'products', x: 0, y: 300,
      }],
    }));
    const error = vi.spyOn(notify, 'error');
    await render();
    await act(async () => tool('Build cube').click());
    expect(error).toHaveBeenCalledWith('Link every table to the principal before building: products');
    expect(api.buildModel).not.toHaveBeenCalled();
    expect(document.body.textContent).not.toContain('Build anyway');
    error.mockRestore();
  });

  it('does not retry a build whose path belongs to another file', async () => {
    api.buildModel.mockRejectedValueOnce(
      new ApiError('O caminho model/sales.yml já é de outro arquivo; renomeie ou mova esse arquivo', 409)
    );
    await render();
    await act(async () => tool('Build cube').click());
    expect(document.body.textContent).not.toContain('Overwrite?');
    expect(api.buildModel).toHaveBeenCalledTimes(1);
    expect(onBuilt).not.toHaveBeenCalled();
  });

  it('disables the tables of another connection in the panel', async () => {
    await render();
    const events = panelItem('events');
    expect(events.getAttribute('aria-disabled')).toBe('true');
    expect(events.getAttribute('draggable')).toBe('false');
    expect(events.getAttribute('title')).toContain('warehouse');
    const products = panelItem('products');
    expect(products.getAttribute('aria-disabled')).toBe(null);
    expect(products.getAttribute('draggable')).toBe('true');
  });

  it('shows a table\'s columns in the panel on demand', async () => {
    await render();
    const expand = document.querySelector('[aria-label="Show the columns of customers"]') as HTMLButtonElement;
    expect(expand.getAttribute('aria-expanded')).toBe('false');
    await act(async () => expand.click());
    const list = expand.closest('li')!.querySelector('[data-slot="model-table-columns"]')!;
    expect([...list.querySelectorAll('li')].map((li) => li.textContent)).toEqual(
      tables.warehouse.public.customers.map((c) => `${c.name}${c.type}`),
    );
    await act(async () => expand.click());
    expect(expand.closest('li')!.querySelector('[data-slot="model-table-columns"]')).toBeNull();
  });

  it('drops a table from the panel onto the canvas', async () => {
    await render();
    const data: Record<string, string> = {};
    const dataTransfer = {
      setData: (type: string, value: string) => { data[type] = value; },
      getData: (type: string) => data[type] ?? '',
      types: [] as string[],
      effectAllowed: '',
      dropEffect: '',
    };
    const drag = new Event('dragstart', { bubbles: true }) as any;
    drag.dataTransfer = dataTransfer;
    await act(async () => panelItem('products').dispatchEvent(drag));
    expect(JSON.parse(data['application/x-cube-table'])).toEqual({ ds: 'warehouse', schema: 'public', table: 'products' });
    const drop = new Event('drop', { bubbles: true, cancelable: true }) as any;
    drop.dataTransfer = dataTransfer;
    drop.clientX = 40;
    drop.clientY = 60;
    await act(async () => document.querySelector('[data-testid="flow"]')!.dispatchEvent(drop));
    expect([...document.querySelectorAll('[data-node]')].map((n) => n.getAttribute('data-node')))
      .toEqual(['orders', 'customers', 'products']);
    expect(document.body.textContent).toContain('Unsaved changes');
  });

  it('links two columns through the join dialog, then saves before building', async () => {
    api.model.mockResolvedValueOnce(view({
      ...saved(),
      tables: [...saved().tables, {
        alias: 'products', schema: 'public', table: 'products', x: 0, y: 300,
      }],
    }));
    await render();
    await act(async () => flow.props.onConnect({
      source: 'orders', sourceHandle: 'id', target: 'products', targetHandle: 'id',
    }));
    expect(document.body.textContent).toContain('INNER keeps only rows with a match on both sides');
    expect(document.body.textContent).toContain('LEFT keeps every row of the principal table');
    await act(async () => button('Link').click());
    expect([...document.querySelectorAll('[data-edge]')].map((e) => e.textContent))
      .toEqual(['N:1 · LEFT', 'N:1 · LEFT']);

    await act(async () => tool('Build cube').click());
    expect(api.saveModel).toHaveBeenCalledTimes(1);
    const [id, change] = api.saveModel.mock.calls[0];
    expect(id).toBe('m1');
    expect(change.version).toBe(3);
    expect(change.definition.joins).toHaveLength(2);
    expect(api.buildModel).toHaveBeenCalledTimes(1);
    expect(api.saveModel.mock.invocationCallOrder[0]).toBeLessThan(api.buildModel.mock.invocationCallOrder[0]);
    expect(onBuilt).toHaveBeenCalledWith('sales', built.detail);
  });

  it('links a column dropped on the row of another table, away from its handle', async () => {
    api.model.mockResolvedValueOnce(view({
      ...saved(),
      tables: [...saved().tables, {
        alias: 'products', schema: 'public', table: 'products', x: 0, y: 300,
      }],
    }));
    await render();
    expect(flow.props.connectionMode).toBe('loose');
    const row = document.querySelector('[data-node="products"] [data-column="id"]')!;
    // jsdom has no layout, so it has no elementFromPoint either.
    document.elementFromPoint = () => row.querySelector('span:not([data-handle])');
    try {
      await act(async () => flow.props.onConnectEnd(
        { clientX: 10, clientY: 20 },
        { isValid: false, fromNode: { id: 'orders' }, fromHandle: { id: 'id' } }
      ));
    } finally {
      delete (document as any).elementFromPoint;
    }
    await act(async () => button('Link').click());
    expect([...document.querySelectorAll('[data-edge]')].map((e) => e.textContent))
      .toEqual(['N:1 · LEFT', 'N:1 · LEFT']);
  });

  it('ignores a link dropped on its own table or that React Flow already made', async () => {
    await render();
    const row = document.querySelector('[data-node="orders"] [data-column="amount"]')!;
    document.elementFromPoint = () => row;
    try {
      await act(async () => flow.props.onConnectEnd(
        { clientX: 1, clientY: 1 },
        { isValid: false, fromNode: { id: 'orders' }, fromHandle: { id: 'id' } }
      ));
      await act(async () => flow.props.onConnectEnd(
        { clientX: 1, clientY: 1 },
        { isValid: true, fromNode: { id: 'customers' }, fromHandle: { id: 'id' } }
      ));
    } finally {
      delete (document as any).elementFromPoint;
    }
    expect(document.body.textContent).not.toContain('INNER keeps only rows');
  });

  it('links two columns picked by a click on their names', async () => {
    api.model.mockResolvedValueOnce(view({
      ...saved(),
      tables: [...saved().tables, {
        alias: 'products', schema: 'public', table: 'products', x: 0, y: 300,
      }],
    }));
    await render();
    const name = (table: string, column: string) => document.querySelector(
      `[data-node="${table}"] [data-column="${column}"] [data-slot="model-column-name"]`
    ) as HTMLButtonElement;
    await act(async () => name('orders', 'id').click());
    expect(document.querySelector('[data-node="orders"] [data-column="id"]')!.hasAttribute('data-picked')).toBe(true);
    expect(document.querySelector('[data-slot="model-link-hint"]')!.textContent).toContain('Linking orders.id');
    await act(async () => name('products', 'id').click());
    expect(document.querySelector('[data-slot="model-link-hint"]')).toBeNull();
    await act(async () => button('Link').click());
    expect([...document.querySelectorAll('[data-edge]')].map((e) => e.textContent))
      .toEqual(['N:1 · LEFT', 'N:1 · LEFT']);
  });

  it('drops a picked column on Escape, and on a second click on it', async () => {
    await render();
    const name = document.querySelector(
      '[data-node="orders"] [data-column="id"] [data-slot="model-column-name"]'
    ) as HTMLButtonElement;
    await act(async () => name.click());
    await act(async () => window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' })));
    expect(document.querySelector('[data-picked]')).toBeNull();
    await act(async () => name.click());
    await act(async () => name.click());
    expect(document.querySelector('[data-picked]')).toBeNull();
  });

  it('collapses a table to its header, keeping its links, and remembers it', async () => {
    await render();
    expect(flow.props.edges.every((e: any) => e.type === 'reference')).toBe(true);
    await act(async () => (document.querySelector('[aria-label="Collapse orders"]') as HTMLButtonElement).click());
    const orders = document.querySelector('[data-node="orders"]')!;
    expect(orders.querySelector('[data-column]')).toBeNull();
    expect(orders.querySelectorAll('[data-handle]').length).toBeGreaterThan(0);
    expect(document.querySelectorAll('[data-edge]').length).toBe(1);
    expect(JSON.parse(window.localStorage.getItem('cubePlayground.modelCollapsed.m1')!)).toEqual(['orders']);
    await act(async () => (document.querySelector('[aria-label="Expand orders"]') as HTMLButtonElement).click());
    expect(orders.querySelector('[data-column]')).not.toBeNull();
  });

  it('refuses a second link between the same two tables', async () => {
    await render();
    await act(async () => flow.props.onConnect({
      source: 'customers', sourceHandle: 'name', target: 'orders', targetHandle: 'amount',
    }));
    expect(document.body.textContent).not.toContain('INNER keeps only rows');
  });

  it('shows a reference\'s properties from its edge and deletes it', async () => {
    await render();
    await act(async () => (document.querySelector('[data-edge]') as HTMLButtonElement).click());
    const pane = document.querySelector('[data-slot="model-properties"]')!;
    expect(pane.textContent).toContain('Reference properties');
    expect(pane.textContent).toContain('N:1 · LEFT');
    await act(async () => button('Delete reference').click());
    expect(document.querySelectorAll('[data-edge]').length).toBe(0);
    expect(document.querySelector('[data-slot="model-properties"]')!.textContent).toContain('Model properties');
  });

  it('opens a model whose tables are gone, naming them', async () => {
    api.model.mockResolvedValueOnce(view({
      ...saved(),
      tables: [...saved().tables, {
        alias: 'gone', schema: 'public', table: 'gone', x: 0, y: 300,
      }],
    }));
    await render();
    const alert = document.querySelector('[data-slot="alert"]')!;
    expect(alert.textContent).toContain('public.gone');
    expect(document.querySelector('[data-node="gone"]')).not.toBe(null);
  });

  it('saves with the version it read', async () => {
    await render();
    const name = document.querySelector('input[aria-label="Model name"]') as HTMLInputElement;
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!;
    await act(async () => {
      setter.call(name, 'Sales 2');
      name.dispatchEvent(new Event('input', { bubbles: true }));
    });
    await act(async () => tool('Save changes').click());
    expect(api.saveModel).toHaveBeenCalledWith('m1', expect.objectContaining({ name: 'Sales 2', version: 3 }));
    expect(document.body.textContent).not.toContain('Unsaved changes');
  });

  it('keeps an edit made while a save is on its way unsaved', async () => {
    let finish: (v: CubeModelView) => void = () => {};
    api.saveModel.mockImplementationOnce(() => new Promise<CubeModelView>((resolve) => { finish = resolve; }));
    await render();
    await rename('Sales 2');
    await act(async () => tool('Save changes').click());
    await rename('Sales 3');
    await act(async () => finish(view(saved(), 4)));
    expect(document.body.textContent).toContain('Unsaved changes');
    await act(async () => tool('Save changes').click());
    expect(api.saveModel.mock.calls[1][1]).toEqual(expect.objectContaining({ name: 'Sales 3', version: 4 }));
    expect(document.body.textContent).not.toContain('Unsaved changes');
  });

  it('sends the version each save returned', async () => {
    await render();
    await rename('Sales 2');
    await act(async () => tool('Save changes').click());
    await rename('Sales 3');
    await act(async () => tool('Save changes').click());
    expect(api.saveModel.mock.calls.map(([, c]) => c.version)).toEqual([3, 4]);
  });

  it('writes the dragged position into the definition', async () => {
    await render();
    await act(async () => flow.props.onNodeDragStop(
      {},
      { id: 'customers' },
      [{ id: 'customers', position: { x: 410.4, y: 19.6 } }]
    ));
    expect(document.body.textContent).toContain('Unsaved changes');
    await act(async () => tool('Save changes').click());
    const { tables: placed } = api.saveModel.mock.calls[0][1].definition;
    expect(placed.find((t: any) => t.alias === 'customers')).toEqual(expect.objectContaining({ x: 410, y: 20 }));
    expect(placed.find((t: any) => t.alias === 'orders')).toEqual(expect.objectContaining({ x: 0, y: 0 }));
  });

  it('deletes the model after asking, with the version it read', async () => {
    await render();
    await act(async () => button('Delete model').click());
    expect(document.body.textContent).toContain('Delete model Sales? The cube it built stays in Cubes.');
    expect(api.deleteModel).not.toHaveBeenCalled();
    await act(async () => button('Delete').click());
    expect(api.deleteModel).toHaveBeenCalledWith('m1', 3);
    expect(onDeleted).toHaveBeenCalledTimes(1);
  });

  it('keeps a model whose delete found a newer version', async () => {
    api.deleteModel.mockRejectedValueOnce(new ApiError('O model mudou desde que você abriu; abra de novo', 409));
    await render();
    await act(async () => button('Delete model').click());
    await act(async () => button('Delete').click());
    expect(onDeleted).not.toHaveBeenCalled();
    expect(document.querySelectorAll('[data-node]').length).toBe(2);
  });

  it('follows the page theme on the canvas', async () => {
    document.documentElement.setAttribute('data-theme', 'dark');
    await render();
    expect(flow.props.colorMode).toBe('dark');
    await act(async () => {
      document.documentElement.setAttribute('data-theme', 'light');
      await new Promise((r) => setTimeout(r, 0));
    });
    expect(flow.props.colorMode).toBe('light');
  });
  const draftColumns = () => [
    {
      name: 'id', type: 'integer' as const, nullable: false, primaryKey: true,
    },
    {
      name: 'label', type: 'text' as const, nullable: true, primaryKey: false,
    },
  ];
  const withDraft = (): ModelDefinition => ({
    ...saved(),
    tables: [...saved().tables, {
      alias: 'items', schema: 'shop', table: 'items', x: 0, y: 300, draft: { columns: draftColumns() },
    }],
  });
  const created = (): ModelDefinition => ({
    ...saved(),
    tables: [...saved().tables, {
      alias: 'items', schema: 'shop', table: 'items', x: 0, y: 300,
    }],
  });

  describe('editing', () => {
    const key = async (k: string, opts: KeyboardEventInit = {}, target: EventTarget = document.body) => {
      await act(async () => {
        target.dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true, ...opts }));
      });
    };
    const positionOf = (id: string) => flow.props.nodes.find((n: any) => n.id === id)?.position;
    const ids = () => [...document.querySelectorAll('[data-node]')].map((n) => n.getAttribute('data-node'));

    it('undoes and redoes a table move with Ctrl+Z and Ctrl+Y', async () => {
      await render();
      await act(async () => flow.props.onNodeDragStop({}, { id: 'customers' }, [{ id: 'customers', position: { x: 410, y: 20 } }]));
      expect(positionOf('customers')).toEqual({ x: 410, y: 20 });
      await key('z', { ctrlKey: true });
      expect(positionOf('customers')).toEqual({ x: 300, y: 0 });
      await key('y', { ctrlKey: true });
      expect(positionOf('customers')).toEqual({ x: 410, y: 20 });
      await key('z', { ctrlKey: true });
      await key('Z', { ctrlKey: true, shiftKey: true });
      expect(positionOf('customers')).toEqual({ x: 410, y: 20 });
    });

    it('ignores canvas shortcuts while typing in a field', async () => {
      await render();
      await act(async () => flow.props.onNodeDragStop({}, { id: 'customers' }, [{ id: 'customers', position: { x: 410, y: 20 } }]));
      await select('customers');
      const input = document.querySelector('input[aria-label="Search the model"]') as HTMLInputElement;
      await key('z', { ctrlKey: true }, input);
      await key('Delete', {}, input);
      await key('a', { ctrlKey: true }, input);
      expect(positionOf('customers')).toEqual({ x: 410, y: 20 });
      expect(ids()).toEqual(['orders', 'customers']);
      expect(flow.props.nodes.filter((n: any) => n.selected).map((n: any) => n.id)).toEqual(['customers']);
    });

    it('Ctrl+A selects every table, Delete removes the selection', async () => {
      await render();
      await key('a', { ctrlKey: true });
      expect(flow.props.nodes.every((n: any) => n.selected)).toBe(true);
      await key('Delete');
      expect(ids()).toEqual([]);
    });

    it('copies and pastes the selection with Ctrl+C / Ctrl+V', async () => {
      await render();
      await select('orders');
      await key('c', { ctrlKey: true });
      await key('v', { ctrlKey: true });
      expect(ids()).toEqual(['orders', 'customers', 'orders_1']);
      expect(positionOf('orders_1')).toEqual({ x: 20, y: 20 });
      // The pasted table is the new selection.
      expect(flow.props.nodes.filter((n: any) => n.selected).map((n: any) => n.id)).toEqual(['orders_1']);
      await key('x', { ctrlKey: true });
      expect(ids()).toEqual(['orders', 'customers']);
      await key('v', { ctrlKey: true });
      expect(ids()).toEqual(['orders', 'customers', 'orders_1']);
      expect(positionOf('orders_1')).toEqual({ x: 40, y: 40 });
    });

    it('pastes as a shortcut from the toolbar and removes only the shortcut', async () => {
      await render();
      await select('orders');
      await act(async () => tool('Copy').click());
      await act(async () => tool('Paste as shortcut').click());
      expect(ids()).toEqual(['orders', 'orders~s1', 'customers']);
      expect(positionOf('orders~s1')).toEqual({ x: 20, y: 20 });
      expect(node('orders~s1').querySelector('[aria-label="shortcut"]')).not.toBeNull();
      await act(async () => (node('orders~s1').querySelector('[data-slot="model-table-remove"]') as HTMLButtonElement).click());
      expect(ids()).toEqual(['orders', 'customers']);
    });

    it('clears history after create in database', async () => {
      api.model.mockResolvedValueOnce(view(withDraft()));
      api.createTables
        .mockResolvedValueOnce({ statements: ['CREATE TABLE x'] })
        .mockResolvedValueOnce({ model: view(created(), 5), created: ['shop.items'], detail: { tables: {} } });
      await render();
      expect(tool('Undo').disabled).toBe(true);
      await act(async () => flow.props.onNodeDragStop({}, { id: 'customers' }, [{ id: 'customers', position: { x: 410, y: 20 } }]));
      expect(tool('Undo').disabled).toBe(false);
      await act(async () => tool('Create in database').click());
      await act(async () => button('Create').click());
      expect(tool('Undo').disabled).toBe(true);
    });

    it('saves with Ctrl+S and writes formatVersion 2', async () => {
      await render();
      await rename('Sales 2');
      await key('s', { ctrlKey: true });
      await act(async () => {
        await new Promise((r) => { setTimeout(r, 20); });
      });
      expect(api.saveModel).toHaveBeenCalledTimes(1);
      expect(api.saveModel.mock.calls[0][1].definition.formatVersion).toBe(2);
    });
  });

  describe('panes', () => {
    const pane = () => document.querySelector('[data-slot="model-properties"]') as HTMLElement;
    const commit = async (selector: string, value: string) => {
      await fill(selector, value);
      await act(async () => {
        document.querySelector(selector)!.dispatchEvent(new FocusEvent('blur', { bubbles: false }));
        document.querySelector(selector)!.dispatchEvent(new FocusEvent('focusout', { bubbles: true }));
      });
    };

    it('the properties pane edits the model\'s name, principal and primary key', async () => {
      await render();
      expect(pane().textContent).toContain('Model properties');
      expect(pane().querySelector('#model-principal')).not.toBeNull();
      expect(pane().querySelector('#model-primary-key')).not.toBeNull();
      expect(pane().textContent).toContain('warehouse');
      expect(pane().textContent).toContain('2 tables');
      await rename('Sales 2');
      expect(document.querySelector('[data-slot="model-top-bar"]')!.textContent).toContain('Sales 2');
      await act(async () => tool('Save changes').click());
      expect(api.saveModel.mock.calls[0][1].name).toBe('Sales 2');
    });

    it('describes the model', async () => {
      await render();
      await fill('textarea[aria-label="Model description"]', 'Orders and their customers');
      await act(async () => {
        document.querySelector('textarea[aria-label="Model description"]')!.dispatchEvent(new FocusEvent('focusout', { bubbles: true }));
      });
      await act(async () => tool('Save changes').click());
      expect(api.saveModel.mock.calls[0][1].definition.description).toBe('Orders and their customers');
    });

    it('renames a table from its properties and keeps its links', async () => {
      await render();
      await select('orders');
      expect(pane().textContent).toContain('Table properties');
      await commit('input[aria-label="Selected table name"]', 'sales');
      expect(node('sales')).not.toBeNull();
      expect(flow.props.edges[0].source).toBe('sales');
      await commit('input[aria-label="Selected table name"]', 'customers');
      expect(pane().textContent).toContain('customers is already in this model');
      expect(node('sales')).not.toBeNull();
    });

    it('comments a table and lists its columns', async () => {
      await render();
      await select('orders');
      expect([...pane().querySelectorAll('[data-slot="model-property-column"]')].map((r) => r.textContent))
        .toEqual(['idinteger', 'customer_idinteger', 'amountnumeric']);
      await fill('textarea[aria-label="Table comment"]', 'every order');
      await act(async () => {
        document.querySelector('textarea[aria-label="Table comment"]')!.dispatchEvent(new FocusEvent('focusout', { bubbles: true }));
      });
      await act(async () => tool('Save changes').click());
      const def = api.saveModel.mock.calls[0][1].definition;
      expect(def.tables.find((t: any) => t.alias === 'orders').comment).toBe('every order');
    });

    it('the structure tree lists tables and their columns and selects on click', async () => {
      await render();
      const tree = document.querySelector('[data-slot="model-structure"]') as HTMLElement;
      expect(tree.textContent).toContain('Tables');
      expect(tree.textContent).toContain('References');
      expect(tree.textContent).toContain('Text notes');
      expect(tree.textContent).toContain('Views');
      expect(tree.textContent).toContain('Subject areas');
      expect(tree.textContent).toContain('Cubes');
      const item = tree.querySelector('[data-slot="model-structure-item"][data-id="orders"]') as HTMLButtonElement;
      await act(async () => item.click());
      expect(flow.props.nodes.find((n: any) => n.id === 'orders').selected).toBe(true);
      expect(flow.setCenter).toHaveBeenCalledTimes(1);
      await act(async () => (tree.querySelector('[aria-label="Show columns of orders"]') as HTMLButtonElement).click());
      expect(tree.textContent).toContain('customer_id');
      const ref = tree.querySelector('[data-slot="model-structure-item"][data-id="join-0"]') as HTMLButtonElement;
      await act(async () => ref.click());
      expect(pane().textContent).toContain('Reference properties');
    });

    it('lists the explorer as coming soon and the issues as open', async () => {
      await render();
      const left = document.querySelector('[data-slot="model-left-pane"]') as HTMLElement;
      expect(left.textContent).toContain('Model structure');
      expect(left.textContent).toContain('Database tables');
      expect((left.querySelector('button[aria-label="Model explorer"]') as HTMLButtonElement).disabled).toBe(true);
      expect((left.querySelector('button[aria-label="Issues"]') as HTMLButtonElement).disabled).toBe(false);
    });

    it('hides the properties pane with its toggle', async () => {
      await render();
      expect(pane()).not.toBeNull();
      await act(async () => tool('Toggle properties pane').click());
      expect(pane()).toBeNull();
    });
  });

  describe('review fixes', () => {
    const pane = () => document.querySelector('[data-slot="model-properties"]') as HTMLElement;
    const key = async (k: string, opts: KeyboardEventInit = {}, target: EventTarget = document.body) => {
      await act(async () => {
        target.dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true, ...opts }));
      });
    };
    const ids = () => [...document.querySelectorAll('[data-node]')].map((n) => n.getAttribute('data-node'));

    it('takes selection only from the user\'s changes, never from React Flow echoing it', async () => {
      await render();
      expect(flow.props.onSelectionChange).toBeUndefined();
    });

    it('a table picked after a reference replaces it, even while React Flow still reports the edge', async () => {
      await render();
      await act(async () => (document.querySelector('[data-edge]') as HTMLButtonElement).click());
      expect(pane().textContent).toContain('Reference properties');
      await act(async () => flow.props.onNodesChange([{ type: 'select', id: 'orders', selected: true }]));
      expect(pane().textContent).toContain('Table properties');
      expect(flow.props.edges[0].selected).toBe(false);
    });

    it('a click on the empty canvas drops the selected reference', async () => {
      await render();
      await act(async () => (document.querySelector('[data-edge]') as HTMLButtonElement).click());
      await act(async () => flow.props.onPaneClick({}));
      expect(pane().textContent).toContain('Model properties');
    });

    it('ignores canvas shortcuts while a dialog is open', async () => {
      await render();
      await select('customers');
      await act(async () => tool('New table').click());
      await key('Delete');
      await key('a', { ctrlKey: true });
      expect(ids()).toEqual(['orders', 'customers']);
      expect(flow.props.nodes.filter((n: any) => n.selected).map((n: any) => n.id)).toEqual(['customers']);
    });

    it('leaves Ctrl+C to the page when text is selected', async () => {
      await render();
      await select('orders');
      const text = document.querySelector('[data-slot="model-top-bar"] h2')!;
      const range = document.createRange();
      range.selectNodeContents(text);
      window.getSelection()!.removeAllRanges();
      window.getSelection()!.addRange(range);
      await key('c', { ctrlKey: true });
      window.getSelection()!.removeAllRanges();
      expect(tool('Paste').disabled).toBe(true);
    });

    it('Ctrl+S in the table name field saves the new name', async () => {
      await render();
      await select('orders');
      await fill('input[aria-label="Selected table name"]', 'purchases');
      const input = document.querySelector('input[aria-label="Selected table name"]') as HTMLInputElement;
      input.focus();
      await act(async () => {
        input.dispatchEvent(new KeyboardEvent('keydown', { key: 's', ctrlKey: true, bubbles: true }));
      });
      await act(async () => {
        await new Promise((r) => { setTimeout(r, 20); });
      });
      expect(api.saveModel).toHaveBeenCalledTimes(1);
      expect(api.saveModel.mock.calls[0][1].definition.tables.map((t: any) => t.alias)).toContain('purchases');
    });

    it('typing a comment is one undo step', async () => {
      await render();
      await select('orders');
      await fill('textarea[aria-label="Table comment"]', 'e');
      await fill('textarea[aria-label="Table comment"]', 'every order');
      await act(async () => {
        document.querySelector('textarea[aria-label="Table comment"]')!.dispatchEvent(new FocusEvent('focusout', { bubbles: true }));
      });
      expect(tool('Undo').disabled).toBe(false);
      await act(async () => tool('Undo').click());
      expect(tool('Undo').disabled).toBe(true);
      expect((document.querySelector('textarea[aria-label="Table comment"]') as HTMLTextAreaElement).value).toBe('');
    });

    it('undo that takes the selected reference away selects nothing', async () => {
      api.model.mockResolvedValueOnce(view(threeTables()));
      await render();
      await act(async () => tool('New reference').click());
      await act(async () => flow.props.onNodeClick({}, flow.props.nodes.find((n: any) => n.id === 'orders')));
      await act(async () => flow.props.onNodeClick({}, flow.props.nodes.find((n: any) => n.id === 'products')));
      expect(pane().textContent).toContain('Reference properties');
      await act(async () => tool('Undo').click());
      expect(pane().textContent).toContain('Model properties');
    });

    it('zooms from 25% to 400%', async () => {
      await render();
      expect(flow.props.minZoom).toBe(0.25);
      expect(flow.props.maxZoom).toBe(4);
    });

    it('links from a shortcut\'s column to its table', async () => {
      api.model.mockResolvedValueOnce(view(threeTables()));
      await render();
      await select('orders');
      await act(async () => tool('Copy').click());
      await act(async () => tool('Paste as shortcut').click());
      const error = vi.spyOn(notify, 'error');
      await act(async () => flow.props.onConnect({
        source: 'products', sourceHandle: 'id', target: 'orders~s1', targetHandle: 'customer_id',
      }));
      expect(error).not.toHaveBeenCalled();
      expect(document.body.textContent).toContain('products.id → orders.customer_id');
      error.mockRestore();
    });
  });

  describe('toolbar', () => {
    it('has Redgate\'s groups and enables them by the selection', async () => {
      api.model.mockResolvedValueOnce(view(threeTables()));
      await render();
      const labels = ['Undo', 'Redo', 'Save changes', 'Export model to image', 'Export model to PDF file',
        'Generate SQL script', 'Generate model documentation', 'Copy', 'Paste', 'Paste as shortcut', 'Cut', 'Delete',
        'Size', 'Align', 'Distribution', 'Order', 'References', 'Arrange', 'Zoom out', '1:1', 'Zoom in', 'Fit view',
        'Toggle navigation pane', 'Toggle properties pane', 'Toggle top bar', 'Create in database', 'Build cube'];
      labels.forEach((l) => expect(tool(l), l).toBeTruthy());
      ['Export model to image', 'Export model to PDF file', 'Generate model documentation', 'Order']
        .forEach((l) => expect(tool(l).disabled, l).toBe(true));
      expect(tool('Size').disabled).toBe(true);
      expect(tool('Delete').disabled).toBe(true);
      await select('orders');
      expect(tool('Size').disabled).toBe(false);
      expect(tool('Align').disabled).toBe(true);
      expect(tool('Delete').disabled).toBe(false);
      await selectMany(['orders', 'customers']);
      expect(tool('Align').disabled).toBe(false);
      expect(tool('Distribution').disabled).toBe(true);
      await selectMany(['orders', 'customers', 'products']);
      expect(tool('Distribution').disabled).toBe(false);
      expect(tool('Order').disabled).toBe(true);
      expect(tool('References').disabled).toBe(true);
    });

    it('aligns the selected tables to the left', async () => {
      await render();
      await selectMany(['orders', 'customers']);
      await choose('Align', 'Align left');
      await act(async () => tool('Save changes').click());
      const placed = api.saveModel.mock.calls[0][1].definition.tables;
      expect(placed.map((t: any) => t.x)).toEqual([0, 0]);
    });

    it('distributes three tables with equal gaps', async () => {
      api.model.mockResolvedValueOnce(view(threeTables()));
      await render();
      await selectMany(['orders', 'customers', 'products']);
      await choose('Distribution', 'Distribute left');
      await act(async () => tool('Save changes').click());
      const placed = api.saveModel.mock.calls[0][1].definition.tables;
      expect(placed.find((t: any) => t.alias === 'customers').x).toBe(450);
    });

    it('Same width copies the first selected table\'s width, Adjust to text clears it', async () => {
      api.model.mockResolvedValueOnce(view({
        ...saved(),
        tables: saved().tables.map((t) => (t.alias === 'orders' ? { ...t, width: 220 } : t)),
      }));
      await render();
      await selectMany(['orders', 'customers']);
      await choose('Size', 'Same width');
      expect(flow.props.nodes.find((n: any) => n.id === 'customers').width).toBe(220);
      await choose('Size', 'Adjust to text');
      expect(flow.props.nodes.find((n: any) => n.id === 'customers').width).toBeUndefined();
      expect(flow.props.nodes.find((n: any) => n.id === 'orders').width).toBeUndefined();
    });

    it('adds and removes a reference point and swaps the orientation', async () => {
      await render();
      await act(async () => flow.props.onEdgesChange([{ type: 'select', id: 'join-0', selected: true }]));
      await choose('References', 'Add reference point');
      const join = () => flow.props.edges[0].data.join;
      expect(join().points).toHaveLength(1);
      await choose('References', 'Reduce reference point');
      expect(join().points).toBeUndefined();
      await choose('References', 'Swap reference orientation');
      expect(join().swapped).toBe(true);
    });

    it('zooms to a chosen percentage and back to 1:1', async () => {
      await render();
      const zoom = document.querySelector('select[aria-label="Zoom"]') as HTMLSelectElement;
      await act(async () => {
        zoom.value = '50';
        zoom.dispatchEvent(new Event('change', { bubbles: true }));
      });
      expect(flow.zoomTo).toHaveBeenLastCalledWith(0.5);
      await act(async () => tool('1:1').click());
      expect(flow.zoomTo).toHaveBeenLastCalledWith(1);
      await act(async () => flow.props.onMove?.({}, { x: 0, y: 0, zoom: 2 }));
      expect(zoom.value).toBe('200');
    });

    it('search lists hits and centers the picked table', async () => {
      await render();
      await fill('input[aria-label="Search the model"]', 'amount');
      const hit = document.querySelector('[data-slot="model-search-hit"]') as HTMLButtonElement;
      expect(hit.textContent).toContain('orders.amount');
      await act(async () => hit.click());
      expect(flow.setCenter).toHaveBeenCalledTimes(1);
      expect(flow.props.nodes.find((n: any) => n.id === 'orders').selected).toBe(true);
    });

    it('the palette\'s table tool opens the new-table dialog; tools 5-8 are disabled', async () => {
      await render();
      const palette = document.querySelector('[data-slot="model-tool-palette"]')!;
      const labels = [...palette.querySelectorAll('button')].map((b) => b.getAttribute('aria-label'));
      expect(labels).toEqual(['Select', 'Select area', 'New table', 'New reference', 'New view', 'New note', 'New area', 'New cube']);
      ['New view', 'New note', 'New area', 'New cube'].forEach((l) => expect(tool(l).disabled, l).toBe(true));
      await act(async () => tool('New table').click());
      expect(document.body.textContent).toContain('New table');
      expect(document.querySelector('input[aria-label="Table name"]')).not.toBeNull();
    });

    it('the area tool selects by dragging instead of panning', async () => {
      await render();
      expect(flow.props.panOnDrag).toBe(true);
      await act(async () => tool('Select area').click());
      expect(flow.props.selectionOnDrag).toBe(true);
      expect(flow.props.panOnDrag).toBe(false);
      await act(async () => {
        window.dispatchEvent(new KeyboardEvent('keydown', { key: '1', code: 'Digit1', bubbles: true }));
      });
      expect(flow.props.panOnDrag).toBe(true);
    });

    it('toggles the panes with Ctrl+Alt+N / P / B', async () => {
      await render();
      const press = async (k: string) => act(async () => {
        window.dispatchEvent(new KeyboardEvent('keydown', {
          key: k, code: `Key${k.toUpperCase()}`, ctrlKey: true, altKey: true, bubbles: true,
        }));
      });
      expect(document.querySelector('[data-slot="model-left-pane"]')).not.toBeNull();
      await press('n');
      expect(document.querySelector('[data-slot="model-left-pane"]')).toBeNull();
      expect(tool('Toggle navigation pane').getAttribute('aria-pressed')).toBe('false');
      await press('b');
      expect(document.querySelector('[data-slot="model-top-bar"]')).toBeNull();
      await press('b');
      expect(document.querySelector('[data-slot="model-top-bar"]')).not.toBeNull();
      await press('p');
      expect(tool('Toggle properties pane').getAttribute('aria-pressed')).toBe('false');
    });

    it('creates only with drafts and edits a draft in its properties', async () => {
      api.model.mockResolvedValueOnce(view(withDraft()));
      await render();
      expect(tool('Create in database').disabled).toBe(false);
      await select('items');
      const pane = document.querySelector('[data-slot="model-properties"]')!;
      expect((pane.querySelector('input[aria-label="Column 2 name"]') as HTMLInputElement).value).toBe('label');
    });

    it('deletes the selected table', async () => {
      await render();
      await select('customers');
      await act(async () => tool('Delete').click());
      expect([...document.querySelectorAll('[data-node]')].map((n) => n.getAttribute('data-node'))).toEqual(['orders']);
      expect(document.querySelectorAll('[data-edge]').length).toBe(0);
      expect(tool('Delete').disabled).toBe(true);
    });

    it('colors the selected table from the palette', async () => {
      await render();
      await select('orders');
      const pane = document.querySelector('[data-slot="model-properties"]')!;
      await act(async () => (pane.querySelector('button[aria-label="blue"]') as HTMLButtonElement).click());
      const band = node('orders').querySelector('[data-slot="model-table-band"]')!;
      expect(band.getAttribute('style')).toContain('var(--model-blue)');
      await act(async () => tool('Save changes').click());
      const def = api.saveModel.mock.calls[0][1].definition;
      expect(def.tables.find((t: any) => t.alias === 'orders').color).toBe('blue');
    });

    it('zooms and fits through React Flow', async () => {
      await render();
      await act(async () => tool('Zoom in').click());
      await act(async () => tool('Zoom out').click());
      await act(async () => tool('Fit view').click());
      expect(flow.zoomIn).toHaveBeenCalledTimes(1);
      expect(flow.zoomOut).toHaveBeenCalledTimes(1);
      expect(flow.fitView).toHaveBeenCalledWith({ maxZoom: 1 });
    });

    it('arranges the tables in layers from the principal, then fits', async () => {
      api.model.mockResolvedValueOnce(view({
        ...saved(),
        tables: saved().tables.map((t) => ({ ...t, x: 999, y: 999 })),
      }));
      await render();
      // The fit must see the canvas with the new positions, not the old ones.
      let seen: number[][] = [];
      flow.fitView.mockImplementation(() => {
        seen = flow.props.nodes.map((n: any) => [n.position.x, n.position.y]);
      });
      await act(async () => {
        tool('Arrange').click();
      });
      // The fit waits for the nodes to render, then for a frame.
      const frame = () => new Promise((r) => window.requestAnimationFrame(() => r(null)));
      await act(async () => {
        await frame().then(frame).then(frame);
      });
      expect(flow.fitView).toHaveBeenCalled();
      expect(seen).toEqual([[0, 0], [320, 0]]);
      await act(async () => tool('Save changes').click());
      const placed = api.saveModel.mock.calls[0][1].definition.tables;
      expect(placed.map((t: any) => [t.alias, t.x, t.y])).toEqual([['orders', 0, 0], ['customers', 320, 0]]);
    });
  });

  describe('new table', () => {
    it('adds a draft table that the save sends', async () => {
      await render();
      await act(async () => tool('New table').click());
      await fill('input[aria-label="Table name"]', 'items');
      await fill('input[aria-label="Column 1 name"]', 'id');
      await act(async () => button('Add table').click());
      expect(document.body.textContent).not.toContain('Add table');
      expect(node('items').textContent).toContain('new');
      await act(async () => tool('Save changes').click());
      const def = api.saveModel.mock.calls[0][1].definition;
      expect(def.tables.find((t: any) => t.alias === 'items')).toEqual(expect.objectContaining({
        schema: 'public',
        table: 'items',
        x: 40,
        y: 40,
        draft: {
          columns: [{
            name: 'id', type: 'integer', nullable: false, primaryKey: true,
          }],
        },
      }));
    });

    it('resets the schema when the connection changes', async () => {
      api.model.mockResolvedValueOnce(view({
        dataSource: '', tables: [], primary: null, primaryKey: null, joins: [],
      } as any));
      await render();
      await act(async () => tool('New table').click());
      const schemaOf = () => document.querySelector('#table-schema')!.textContent;
      expect(schemaOf()).toContain('public');
      const trigger = document.querySelector('#table-connection') as HTMLElement;
      await act(async () => {
        trigger.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true, button: 0 }));
        trigger.click();
      });
      const lake = [...document.querySelectorAll('[role="option"]')]
        .find((o) => o.textContent === 'lake') as HTMLElement;
      await act(async () => lake.click());
      expect(schemaOf()).toContain('main');
      expect(schemaOf()).not.toContain('public');
    });

    it('shows why a bad name is refused and adds nothing', async () => {
      await render();
      await act(async () => tool('New table').click());
      await fill('input[aria-label="Table name"]', '1items');
      await act(async () => button('Add table').click());
      expect(document.querySelector('[role="alert"]')!.textContent).toBe('Invalid table name: 1items');
      expect(node('1items')).toBeNull();
      expect(document.body.textContent).not.toContain('Unsaved changes');
    });

    it('a double click on a draft selects it and focuses its first column', async () => {
      api.model.mockResolvedValueOnce(view(withDraft()));
      await render();
      await act(async () => flow.props.onNodeDoubleClick({}, { id: 'items' }));
      const first = document.querySelector('[data-slot="model-properties"] input[aria-label="Column 1 name"]');
      expect(document.activeElement).toBe(first);
      expect(document.body.textContent).not.toContain('Edit table items');
    });
  });

  describe('table properties', () => {
    const pane = () => document.querySelector('[data-slot="model-properties"]') as HTMLElement;
    const blur = async (selector: string) => act(async () => {
      pane().querySelector(selector)!.dispatchEvent(new FocusEvent('focusout', { bubbles: true }));
    });
    const commit = async (label: string, value: string) => {
      await fill(`[data-slot="model-properties"] [aria-label="${label}"]`, value);
      await blur(`[aria-label="${label}"]`);
    };
    const click = async (label: string) => act(async () => (pane().querySelector(`[aria-label="${label}"]`) as HTMLElement).click());
    const savedDraft = async () => {
      await act(async () => tool('Save changes').click());
      const def = api.saveModel.mock.calls[api.saveModel.mock.calls.length - 1][1].definition;
      return def.tables.find((t: any) => t.alias === 'items').draft;
    };

    it('edits a draft column name, type, N and PK and saves them', async () => {
      api.model.mockResolvedValueOnce(view(withDraft()));
      await render();
      await select('items');
      await commit('Column 2 name', 'title');
      await commit('Column 2 type', 'varchar(80)');
      await click('Column 2 allows nulls');
      await click('Column 2 primary key');
      await click('Add column');
      const draft = await savedDraft();
      expect(draft.columns[1]).toEqual({
        name: 'title', type: 'varchar(80)', nullable: false, primaryKey: true,
      });
      expect(draft.columns[2].name).toBe('column_1');
      expect(node('items').textContent).toContain('title');
    });

    it('remeasures a table whose columns change, so links to a new column draw', async () => {
      api.model.mockResolvedValueOnce(view(withDraft()));
      await render();
      await select('items');
      flow.updated = [];
      await act(async () => (document.querySelector('[data-slot="model-properties"] [aria-label="Add column"]') as HTMLElement).click());
      expect(flow.updated).toContain('items');
    });

    it('opens a column\'s details and sets default and comment', async () => {
      api.model.mockResolvedValueOnce(view(withDraft()));
      await render();
      await select('items');
      await click('Column 2 details');
      await commit('Column 2 default value', "'none'");
      await commit('Column 2 comment', 'what it is');
      await click('Column 2 auto increment');
      const draft = await savedDraft();
      expect(draft.columns[1]).toMatchObject({ default: "'none'", comment: 'what it is', autoIncrement: true });
    });

    it('adds a unique key, an index with DESC and a check', async () => {
      api.model.mockResolvedValueOnce(view(withDraft()));
      await render();
      await select('items');
      await click('Add alternate key');
      await click('Alternate key 1 column label');
      await click('Add index');
      await click('Index 1 column label');
      await click('Index 1 label descending');
      await click('Add check');
      await commit('Check 1 expression', 'id > 0');
      const draft = await savedDraft();
      expect(draft.uniqueKeys).toEqual([{ name: 'items_ak_1', columns: ['label'] }]);
      expect(draft.indexes).toEqual([{ name: 'items_idx_1', columns: [{ name: 'label', desc: true }], unique: false }]);
      expect(draft.checks).toEqual([{ name: 'items_ck_1', expression: 'id > 0' }]);
    });

    it('reorders columns with Alt+ArrowDown', async () => {
      api.model.mockResolvedValueOnce(view(withDraft()));
      await render();
      await select('items');
      const handle = pane().querySelector('[aria-label="Move id"]') as HTMLElement;
      await act(async () => {
        handle.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', altKey: true, bubbles: true }));
      });
      const draft = await savedDraft();
      expect(draft.columns.map((c: any) => c.name)).toEqual(['label', 'id']);
    });

    it('a database table\'s columns are read-only', async () => {
      await render();
      await select('orders');
      expect(pane().querySelector('input[aria-label="Column 1 name"]')).toBeNull();
      expect(pane().textContent).toContain('The structure comes from the database');
      expect(pane().querySelector('[aria-label="Add column"]')).toBeNull();
    });

    it('shows the table\'s issues', async () => {
      const noKey = withDraft();
      noKey.tables[2].draft!.columns[0].primaryKey = false;
      api.model.mockResolvedValueOnce(view(noKey));
      await render();
      await select('items');
      const section = pane().querySelector('[data-slot="model-table-issues"]')!;
      expect(section.textContent).toContain('Warnings (1)');
      expect(section.textContent).toContain('Table items has no primary key');
    });

    it('ignores canvas shortcuts while typing in a column', async () => {
      api.model.mockResolvedValueOnce(view(withDraft()));
      await render();
      await select('items');
      const input = pane().querySelector('[aria-label="Column 1 name"]')!;
      await act(async () => {
        input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Delete', bubbles: true }));
      });
      expect(node('items')).not.toBeNull();
    });
  });

  describe('reference properties', () => {
    const pane = () => document.querySelector('[data-slot="model-properties"]') as HTMLElement;
    const pick = async () => act(async () => (document.querySelector('[data-edge]') as HTMLButtonElement).click());
    const pickOption = async (label: string, value: string) => {
      const el = pane().querySelector(`select[aria-label="${label}"]`) as HTMLSelectElement;
      await act(async () => {
        el.value = value;
        el.dispatchEvent(new Event('change', { bubbles: true }));
      });
    };
    const click = async (label: string) => act(async () => (pane().querySelector(`[aria-label="${label}"]`) as HTMLElement).click());
    const savedJoin = async () => {
      await act(async () => tool('Save changes').click());
      return api.saveModel.mock.calls[api.saveModel.mock.calls.length - 1][1].definition.joins[0];
    };

    it('sets cardinality 1..1 and the cube join becomes 1:1', async () => {
      await render();
      await pick();
      await pickOption('Cardinality', '1..1');
      expect(document.querySelector('[data-edge]')!.textContent).toBe('1:1 · LEFT');
      expect(pane().textContent).toContain('1:1 · LEFT');
      expect((await savedJoin()).cardinality).toBe('1..1');
    });

    it('checking and unchecking mandatory sets the join INNER and LEFT', async () => {
      await render();
      await pick();
      const mandatory = () => pane().querySelector('input[aria-label="Mandatory"]') as HTMLInputElement;
      expect(mandatory().checked).toBe(false);
      await click('Mandatory');
      expect(document.querySelector('[data-edge]')!.textContent).toBe('N:1 · INNER');
      await click('Mandatory');
      expect(document.querySelector('[data-edge]')!.textContent).toBe('N:1 · LEFT');
    });

    it('sets the constraint actions and the name', async () => {
      await render();
      await pick();
      await pickOption('Delete constraint action', 'cascade');
      await pickOption('Update constraint action', 'restrict');
      await fill('[data-slot="model-properties"] input[aria-label="Reference name"]', 'fk_orders_customers');
      await act(async () => {
        pane().querySelector('input[aria-label="Reference name"]')!.dispatchEvent(new FocusEvent('focusout', { bubbles: true }));
      });
      expect(await savedJoin()).toMatchObject({ onDelete: 'cascade', onUpdate: 'restrict', name: 'fk_orders_customers' });
    });

    it('swap reference makes the other table the parent', async () => {
      await render();
      await pick();
      const primary = () => pane().querySelector('[data-slot="model-reference-primary"]')!.textContent;
      expect(primary()).toBe('customers');
      await act(async () => button('Swap reference').click());
      expect(primary()).toBe('orders');
      expect(document.querySelector('[data-edge]')!.textContent).toBe('1:N · LEFT');
    });

    it('adds a column pair and picks its columns', async () => {
      await render();
      await pick();
      await act(async () => button('Add pair').click());
      await pickOption('Pair 2 primary column', 'name');
      await pickOption('Pair 2 foreign column', 'amount');
      expect((await savedJoin()).pairs).toEqual([{ from: 'amount', to: 'name' }]);
    });

    it('the reference tool leaves selection to the editor while it is on', async () => {
      await render();
      expect(flow.props.elementsSelectable).not.toBe(false);
      await act(async () => tool('New reference').click());
      expect(flow.props.elementsSelectable).toBe(false);
    });

    it('the reference tool adds the FK column to a new child table and selects the reference', async () => {
      api.model.mockResolvedValueOnce(view(withDraft()));
      await render();
      await act(async () => tool('New reference').click());
      await act(async () => flow.props.onNodeClick({}, flow.props.nodes.find((n: any) => n.id === 'customers')));
      await act(async () => flow.props.onNodeClick({}, flow.props.nodes.find((n: any) => n.id === 'items')));
      expect(node('items').textContent).toContain('customers_id');
      expect(pane().textContent).toContain('Reference properties');
      expect(document.body.textContent).not.toContain('New link');
    });

    it('undo keeps the same reference selected by id', async () => {
      await render();
      await pick();
      await click('Mandatory');
      await act(async () => tool('Undo').click());
      expect(pane().textContent).toContain('Reference properties');
      expect((pane().querySelector('input[aria-label="Mandatory"]') as HTMLInputElement).checked).toBe(false);
    });
  });

  describe('issues and model settings', () => {
    const pane = () => document.querySelector('[data-slot="model-properties"]') as HTMLElement;
    const noKey = () => {
      const def = withDraft();
      def.tables[2].draft!.columns[0].primaryKey = false;
      return def;
    };
    const commitText = async (label: string, value: string) => {
      await fill(`[data-slot="model-properties"] [aria-label="${label}"]`, value);
      await act(async () => {
        pane().querySelector(`[aria-label="${label}"]`)!.dispatchEvent(new FocusEvent('focusout', { bubbles: true }));
      });
    };
    const openSection = async (title: string) => act(async () => {
      const header = [...pane().querySelectorAll('section > button')].find((x) => x.textContent!.trim() === title) as HTMLButtonElement;
      header.click();
    });
    const lastSaved = async () => {
      await act(async () => tool('Save changes').click());
      return api.saveModel.mock.calls[api.saveModel.mock.calls.length - 1][1].definition;
    };

    it('lists the issues and selects the table of one', async () => {
      api.model.mockResolvedValueOnce(view(noKey()));
      await render();
      const left = document.querySelector('[data-slot="model-left-pane"]') as HTMLElement;
      const header = left.querySelector('button[aria-label="Issues"]') as HTMLButtonElement;
      expect(header.disabled).toBe(false);
      expect(header.textContent).toContain('1');
      await act(async () => header.click());
      const list = left.querySelector('[data-slot="model-issues"]') as HTMLElement;
      expect(list.textContent).toContain('Warnings (1)');
      const item = [...list.querySelectorAll('button')].find((b) => b.textContent!.includes('Table items has no primary key'))!;
      await act(async () => item.click());
      expect(flow.props.nodes.find((n: any) => n.id === 'items').selected).toBe(true);
      expect(pane().textContent).toContain('Table properties');
    });

    it('switching a rule off hides its issues', async () => {
      api.model.mockResolvedValueOnce(view(noKey()));
      await render();
      await openSection('Model validation settings');
      await act(async () => (pane().querySelector('[aria-label="If has PK column(s) (warnings)"]') as HTMLInputElement).click());
      const left = document.querySelector('[data-slot="model-left-pane"]') as HTMLElement;
      expect((left.querySelector('button[aria-label="Issues"]') as HTMLButtonElement).textContent).not.toContain('1');
      expect((await lastSaved()).settings.validation).toEqual({ 'table.hasPk': false });
      await act(async () => button('Select all').click());
      expect((await lastSaved()).settings).toBeUndefined();
    });

    it('edits the model\'s scripts, format and SQL generation settings', async () => {
      await render();
      await openSection('Additional SQL scripts');
      await openSection('Format');
      await openSection('SQL generation settings');
      await commitText('Beginning of the create script', '-- begin');
      await commitText('End of the drop script', '-- dropped');
      await act(async () => (pane().querySelector('[aria-label="Generate DROP statements"]') as HTMLInputElement).click());
      await act(async () => (pane().querySelector('[aria-label="Show N (nullable)"]') as HTMLInputElement).click());
      expect((await lastSaved()).settings).toEqual({
        scripts: { createBefore: '-- begin', dropAfter: '-- dropped' },
        sqlGeneration: { drop: true },
        format: { showNullable: true },
      });
    });
  });

  describe('generate sql', () => {
    const withDialects = (def: ModelDefinition, dialect: 'postgres' | 'mysql' | null) => ({
      ...view(def), dialects: { warehouse: dialect },
    });
    const dialogText = () => document.querySelector('[data-slot="model-sql-script"]')?.textContent ?? '';

    it('Ctrl+G opens the script and the options re-ask the server', async () => {
      api.model.mockResolvedValueOnce(withDialects(saved(), 'postgres'));
      await render();
      await act(async () => {
        window.dispatchEvent(new KeyboardEvent('keydown', { key: 'g', ctrlKey: true, bubbles: true }));
      });
      expect(api.modelSql).toHaveBeenCalledWith('m1', expect.objectContaining({ dataSource: 'warehouse' }), {
        drop: false, onlyNew: false,
      });
      expect(dialogText()).toContain('CREATE TABLE "public"."orders"');
      await act(async () => (document.querySelector('[aria-label="Include DROP statements"]') as HTMLInputElement).click());
      expect(api.modelSql).toHaveBeenLastCalledWith('m1', expect.anything(), { drop: true, onlyNew: false });
    });

    it('starts with the model\'s DROP setting, and a table preview ignores Only new tables', async () => {
      api.model.mockResolvedValueOnce(withDialects({ ...saved(), settings: { sqlGeneration: { drop: true } } }, 'postgres'));
      await render();
      await act(async () => tool('Generate SQL script').click());
      expect(api.modelSql).toHaveBeenLastCalledWith('m1', expect.anything(), { drop: true, onlyNew: false });
      await act(async () => (document.querySelector('[aria-label="Only new tables"]') as HTMLInputElement).click());
      await act(async () => (document.querySelector('[role="dialog"] button[aria-label="Close"], [data-slot="dialog-close"]') as HTMLElement)?.click());
      await act(async () => {
        window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
      });
      await select('orders');
      await act(async () => button('SQL preview').click());
      expect(api.modelSql).toHaveBeenLastCalledWith('m1', expect.anything(), { drop: true, onlyNew: false, only: ['orders'] });
    });

    it('downloads the script as <name>.sql', async () => {
      api.model.mockResolvedValueOnce(withDialects(saved(), 'postgres'));
      (URL as any).createObjectURL = vi.fn(() => 'blob:x');
      (URL as any).revokeObjectURL = vi.fn();
      const clicked: string[] = [];
      const spy = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function recordClick(this: HTMLAnchorElement) {
        clicked.push(this.download);
      });
      await render();
      await act(async () => tool('Generate SQL script').click());
      await act(async () => button('Download').click());
      expect(clicked).toEqual(['Sales.sql']);
      spy.mockRestore();
    });

    it('opens the SQL preview of one table from its properties', async () => {
      api.model.mockResolvedValueOnce(withDialects(withDraft(), 'postgres'));
      await render();
      await select('items');
      await act(async () => button('SQL preview').click());
      expect(api.modelSql).toHaveBeenCalledWith('m1', expect.anything(), { drop: false, onlyNew: false, only: ['items'] });
    });

    it('Generate SQL and Create are disabled for a non-Postgres/MySQL connection', async () => {
      api.model.mockResolvedValueOnce(withDialects(withDraft(), null));
      await render();
      expect(tool('Generate SQL script').disabled).toBe(true);
      expect(tool('Create in database').disabled).toBe(true);
    });

    it('converts portable draft types for a mysql connection on open', async () => {
      api.model.mockResolvedValueOnce(withDialects(withDraft(), 'mysql'));
      await render();
      expect(node('items').querySelector('[data-column="id"]')!.textContent).toContain('int');
      expect(node('items').querySelector('[data-column="id"]')!.textContent).not.toContain('integer');
      expect(document.body.textContent).not.toContain('Unsaved changes');
    });

    it('ignores canvas shortcuts while typing in the SQL dialog', async () => {
      api.model.mockResolvedValueOnce(withDialects(saved(), 'postgres'));
      await render();
      await select('orders');
      await act(async () => tool('Generate SQL script').click());
      const drop = document.querySelector('[aria-label="Include DROP statements"]')!;
      await act(async () => {
        drop.dispatchEvent(new KeyboardEvent('keydown', { key: 'Delete', bubbles: true }));
      });
      expect(node('orders')).not.toBeNull();
    });
  });

  describe('create in database', () => {
    const statements = ['CREATE TABLE shop.items (id integer NOT NULL, label text, PRIMARY KEY (id))'];
    const detail = { tables: {} } as any;

    it('shows the statements, then creates and drops the new badge', async () => {
      api.model.mockResolvedValueOnce(view(withDraft()));
      api.createTables
        .mockResolvedValueOnce({ statements })
        .mockResolvedValueOnce({ model: view(created(), 4), created: ['shop.items'], detail });
      const success = vi.spyOn(notify, 'success');
      await render();
      expect(node('items').textContent).toContain('new');
      await act(async () => tool('Create in database').click());
      expect(api.createTables).toHaveBeenCalledWith('m1', 3, true);
      expect(document.querySelector('[data-slot="model-create-statements"]')!.textContent).toContain(statements[0]);
      expect(document.body.textContent).toContain('This changes the database of warehouse');
      await act(async () => button('Create').click());
      expect(api.createTables).toHaveBeenLastCalledWith('m1', 3, false);
      expect(node('items').textContent).not.toContain('new');
      expect(success).toHaveBeenCalledWith('Created: shop.items');
      expect(onTablesCreated).toHaveBeenCalledWith(detail);
      expect(document.querySelector('[data-slot="model-create-statements"]')).toBeNull();
      success.mockRestore();
    });

    it('keeps the dialog open with the error of a run', async () => {
      api.model.mockResolvedValueOnce(view(withDraft()));
      api.createTables
        .mockResolvedValueOnce({ statements })
        .mockResolvedValueOnce({
          model: view(withDraft(), 4), created: [], error: 'shop.items: permission denied', detail,
        });
      await render();
      await act(async () => tool('Create in database').click());
      await act(async () => button('Create').click());
      expect(document.querySelector('[data-slot="model-create-error"]')!.textContent).toContain('shop.items: permission denied');
      expect(document.querySelector('[data-slot="model-create-statements"]')).not.toBeNull();
      // A retry sends the version the run saved.
      await act(async () => button('Create').click());
      expect(api.createTables).toHaveBeenLastCalledWith('m1', 4, false);
    });

    it('asks again after a partial failure and shows what is still to create', async () => {
      api.model.mockResolvedValueOnce(view(withDraft()));
      const left = ['CREATE TABLE shop.rest (id integer)'];
      api.createTables
        .mockResolvedValueOnce({ statements: [...statements, ...left] })
        .mockResolvedValueOnce({
          model: view(withDraft(), 4), created: ['shop.items'], error: 'shop.rest: boom', detail,
        })
        .mockResolvedValueOnce({ statements: left });
      await render();
      await act(async () => tool('Create in database').click());
      await act(async () => button('Create').click());
      expect(api.createTables).toHaveBeenLastCalledWith('m1', 4, true);
      const shown = document.querySelector('[data-slot="model-create-statements"]')!.textContent!;
      expect(shown).toContain(left[0]);
      expect(shown).not.toContain(statements[0]);
      expect(document.querySelector('[data-slot="model-create-error"]')!.textContent).toContain('shop.rest: boom');
      expect(document.querySelector('[data-slot="model-create-created"]')!.textContent).toContain('shop.items');
    });

    it('closes the dialog when a failed run left no drafts', async () => {
      api.model.mockResolvedValueOnce(view(withDraft()));
      api.createTables
        .mockResolvedValueOnce({ statements })
        .mockResolvedValueOnce({
          model: view(created(), 4), created: ['shop.items'], error: 'late failure', detail,
        });
      const error = vi.spyOn(notify, 'error');
      await render();
      await act(async () => tool('Create in database').click());
      await act(async () => button('Create').click());
      expect(api.createTables).toHaveBeenCalledTimes(2);
      expect(document.querySelector('[data-slot="model-create-statements"]')).toBeNull();
      expect(error).toHaveBeenCalledWith('late failure');
      error.mockRestore();
    });

    it('saves a dirty model first, then asks with the saved version', async () => {
      api.model.mockResolvedValueOnce(view(withDraft()));
      api.createTables.mockResolvedValueOnce({ statements });
      await render();
      await rename('Sales 2');
      await act(async () => tool('Create in database').click());
      expect(api.saveModel).toHaveBeenCalledTimes(1);
      expect(api.saveModel.mock.invocationCallOrder[0]).toBeLessThan(api.createTables.mock.invocationCallOrder[0]);
      expect(api.createTables).toHaveBeenCalledWith('m1', 4, true);
    });
  });

  describe('table node', () => {
    it('shows a draft with its badge and its columns', async () => {
      api.model.mockResolvedValueOnce(view(withDraft()));
      await render();
      const items = node('items');
      expect(items.textContent).toContain('new');
      expect(items.textContent).toContain('label');
      expect(items.textContent).toContain('text');
      expect(items.querySelectorAll('[data-handle="source"][data-id="label"]').length).toBe(1);
      expect(items.querySelector('[aria-label="primary key"]')).not.toBeNull();
      expect(document.querySelector('[data-slot="alert"]')).toBeNull();
    });

    it('draws a table like Redgate: name, then name | type | marks per column', async () => {
      await render();
      const marks = (alias: string, column: string) => node(alias)
        .querySelector(`[data-column="${column}"] [data-slot="model-column-marks"]`)!.textContent!.trim();
      expect(marks('orders', 'id')).toBe('PK');
      expect(marks('orders', 'customer_id')).toBe('FK');
      expect(marks('orders', 'amount')).toBe('');
      expect(marks('customers', 'id')).toBe('');
      const row = node('orders').querySelector('[data-column="amount"]')!;
      expect(row.textContent).toContain('amount');
      expect(row.textContent).toContain('numeric');
      expect(node('orders').querySelector('[data-slot="model-table-name"]')!.textContent).toBe('orders');
    });

    it('hides types and shows N by the model\'s format', async () => {
      const format = { showTypes: false, showNullable: true };
      api.model.mockResolvedValueOnce(view({ ...withDraft(), settings: { format } }));
      await render();
      const row = (column: string) => node('items').querySelector(`[data-column="${column}"]`)!;
      expect(row('label').textContent).not.toContain('text');
      expect(row('label').querySelector('[data-slot="model-column-marks"]')!.textContent).toBe('N');
      expect(row('id').querySelector('[data-slot="model-column-marks"]')!.textContent).toBe('PK');
    });

    it('shows a shortcut icon on a shortcut node', () => {
      const host = document.createElement('div');
      const own = createRoot(host);
      const noop = () => {};
      const data = {
        alias: 'orders',
        schema: 'public',
        table: 'orders',
        columns: [{ name: 'id', type: 'integer' }],
        principal: false,
        missing: false,
        draft: false,
        keys: [],
        fks: [],
        linked: [],
        fixedSize: false,
        onRemove: noop,
        collapsed: false,
        onToggle: noop,
        picked: null,
        onPick: noop,
        onResize: noop,
      };
      act(() => own.render(createElement(TableNode as any, { id: 'orders~s1', data: { ...data, shortcut: 's1' } })));
      expect(host.querySelector('[aria-label="shortcut"]')).not.toBeNull();
      act(() => own.render(createElement(TableNode as any, { id: 'orders', data })));
      expect(host.querySelector('[aria-label="shortcut"]')).toBeNull();
      act(() => own.unmount());
    });

    it('bands the header with the table color', async () => {
      api.model.mockResolvedValueOnce(view({
        ...saved(),
        tables: saved().tables.map((t) => (t.alias === 'orders' ? { ...t, color: 'teal' as const } : t)),
      }));
      await render();
      const band = (alias: string) => node(alias).querySelector('[data-slot="model-table-band"]')!.getAttribute('style');
      expect(band('orders')).toContain('var(--model-teal)');
      expect(band('customers')).toContain('var(--muted)');
    });
  });
});
