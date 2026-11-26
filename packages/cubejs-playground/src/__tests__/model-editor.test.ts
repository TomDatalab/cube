// @vitest-environment jsdom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import {
  afterEach, beforeEach, describe, expect, it, Mock, vi,
} from 'vitest';

import { notify } from '@/components/kit';
import { ApiError, CubeModelView } from '../pages/Schema/api';
import { ModelEditor } from '../pages/Schema/models/ModelEditor';
import type { ModelDefinition } from '../pages/Schema/models/modelGraph';

// React Flow measures its nodes before it draws edges, which jsdom cannot do:
// the canvas renders its nodes (with the editor's own node component) and its
// edges as plain lists, and keeps the props so a test can connect two handles.
const flow = vi.hoisted(() => ({ props: null as any }));

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
          { key: n.id, 'data-node': n.id },
          h(props.nodeTypes[n.type], { id: n.id, data: n.data })
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
    Position: { Left: 'left', Right: 'right', Top: 'top', Bottom: 'bottom' },
    Background: () => null,
    Controls: () => null,
    useReactFlow: () => ({ screenToFlowPosition: (p: { x: number; y: number }) => p }),
    applyNodeChanges: (_changes: unknown, nodes: unknown) => nodes,
  };
});

(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;

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

const view = (definition: ModelDefinition, version = 3): CubeModelView => ({
  id: 'm1',
  organizationId: 'o1',
  name: 'Sales',
  definition,
  cubeName: null,
  version,
  updatedAt: '2026-11-25T00:00:00Z',
  updatedBy: null,
});

const built = { fileName: 'model/sales.yml', content: '', cubeName: 'sales', detail: {} as any };

describe('ModelEditor', () => {
  let root: ReturnType<typeof createRoot>;
  let api: Record<string, ReturnType<typeof vi.fn>>;
  let onBuilt: Mock<(cubeName: string, detail: any) => void>;
  let onDeleted: Mock<() => void>;

  beforeEach(() => {
    Element.prototype.hasPointerCapture = vi.fn(() => false) as any;
    Element.prototype.scrollIntoView = vi.fn() as any;
    api = {
      model: vi.fn(async () => view(saved())),
      saveModel: vi.fn(async (_id: string, change: any) => view(change.definition, change.version + 1)),
      buildModel: vi.fn(async () => built),
      deleteModel: vi.fn(async () => undefined),
    };
    onBuilt = vi.fn<(cubeName: string, detail: any) => void>();
    onDeleted = vi.fn<() => void>();
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
    await act(async () => root.render(createElement(ModelEditor, {
      api: api as any,
      org: { id: 'o1', slug: 'acme', name: 'Acme' },
      modelId: 'm1',
      tables,
      onBuilt,
      onDeleted,
    })));
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
    await act(async () => button('Build cube').click());
    expect(api.saveModel).not.toHaveBeenCalled();
    expect(api.buildModel).toHaveBeenCalledTimes(1);
    expect(api.buildModel.mock.calls[0][0]).toBe('m1');
    expect(api.buildModel.mock.calls[0][1]).toBeFalsy();
    expect(onBuilt).toHaveBeenCalledWith('sales', built.detail);
  });

  it('asks before overwriting a cube edited by hand, then builds with force', async () => {
    api.buildModel.mockRejectedValueOnce(new ApiError('O cubo foi editado desde o último build', 409));
    await render();
    await act(async () => button('Build cube').click());
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
    await act(async () => button('Save').click());
    expect(error).toHaveBeenCalledWith('Já existe um model com este nome nesta organização');
    api.saveModel.mockRejectedValueOnce(new ApiError('O model mudou desde que você abriu; abra de novo', 409));
    await act(async () => button('Save').click());
    expect(error).toHaveBeenLastCalledWith('Changed elsewhere: reopen');
    error.mockRestore();
  });

  it('lists the problems of a build the ModelWriter refused', async () => {
    api.buildModel.mockRejectedValueOnce(new ApiError('A mudança não compila', 400, [
      { file: 'model/sales.yml', cube: 'sales', message: 'coluna inexistente' },
    ]));
    await render();
    await act(async () => button('Build cube').click());
    const box = document.querySelector('[data-slot="model-problems"]')!;
    expect(box.textContent).toContain('model/sales.yml: coluna inexistente');
    expect(onBuilt).not.toHaveBeenCalled();
    await act(async () => button('Build cube').click());
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
    await act(async () => button('Build cube').click());
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
    await act(async () => button('Build cube').click());
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

    await act(async () => button('Build cube').click());
    expect(api.saveModel).toHaveBeenCalledTimes(1);
    const [id, change] = api.saveModel.mock.calls[0];
    expect(id).toBe('m1');
    expect(change.version).toBe(3);
    expect(change.definition.joins).toHaveLength(2);
    expect(api.buildModel).toHaveBeenCalledTimes(1);
    expect(api.saveModel.mock.invocationCallOrder[0]).toBeLessThan(api.buildModel.mock.invocationCallOrder[0]);
    expect(onBuilt).toHaveBeenCalledWith('sales', built.detail);
  });

  it('refuses a second link between the same two tables', async () => {
    await render();
    await act(async () => flow.props.onConnect({
      source: 'customers', sourceHandle: 'name', target: 'orders', targetHandle: 'amount',
    }));
    expect(document.body.textContent).not.toContain('INNER keeps only rows');
  });

  it('changes and deletes a link from its edge', async () => {
    await render();
    await act(async () => (document.querySelector('[data-edge]') as HTMLButtonElement).click());
    const inner = document.querySelector('[role="radio"][value="inner"]') as HTMLButtonElement;
    await act(async () => inner.click());
    await act(async () => button('Save link').click());
    expect(document.querySelector('[data-edge]')!.textContent).toBe('N:1 · INNER');
    await act(async () => (document.querySelector('[data-edge]') as HTMLButtonElement).click());
    await act(async () => button('Delete').click());
    expect(document.querySelectorAll('[data-edge]').length).toBe(0);
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
    await act(async () => button('Save').click());
    expect(api.saveModel).toHaveBeenCalledWith('m1', expect.objectContaining({ name: 'Sales 2', version: 3 }));
    expect(document.body.textContent).not.toContain('Unsaved changes');
  });

  it('keeps an edit made while a save is on its way unsaved', async () => {
    let finish: (v: CubeModelView) => void = () => {};
    api.saveModel.mockImplementationOnce(() => new Promise<CubeModelView>((resolve) => { finish = resolve; }));
    await render();
    await rename('Sales 2');
    await act(async () => button('Save').click());
    await rename('Sales 3');
    await act(async () => finish(view(saved(), 4)));
    expect(document.body.textContent).toContain('Unsaved changes');
    await act(async () => button('Save').click());
    expect(api.saveModel.mock.calls[1][1]).toEqual(expect.objectContaining({ name: 'Sales 3', version: 4 }));
    expect(document.body.textContent).not.toContain('Unsaved changes');
  });

  it('sends the version each save returned', async () => {
    await render();
    await rename('Sales 2');
    await act(async () => button('Save').click());
    await rename('Sales 3');
    await act(async () => button('Save').click());
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
    await act(async () => button('Save').click());
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
});
