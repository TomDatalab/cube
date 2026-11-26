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
const flow = vi.hoisted(() => ({
  props: null as any,
  zoomIn: null as any,
  zoomOut: null as any,
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
          { key: n.id, 'data-node': n.id },
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
    Position: { Left: 'left', Right: 'right', Top: 'top', Bottom: 'bottom' },
    Background: () => null,
    Controls: () => null,
    useReactFlow: () => ({
      screenToFlowPosition: (p: { x: number; y: number }) => p,
      zoomIn: flow.zoomIn,
      zoomOut: flow.zoomOut,
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
  let onTablesCreated: Mock<(detail: any) => void>;

  beforeEach(() => {
    Element.prototype.hasPointerCapture = vi.fn(() => false) as any;
    Element.prototype.scrollIntoView = vi.fn() as any;
    flow.zoomIn = vi.fn();
    flow.zoomOut = vi.fn();
    flow.fitView = vi.fn();
    api = {
      model: vi.fn(async () => view(saved())),
      saveModel: vi.fn(async (_id: string, change: any) => view(change.definition, change.version + 1)),
      buildModel: vi.fn(async () => built),
      deleteModel: vi.fn(async () => undefined),
      createTables: vi.fn(async () => ({})),
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
    await act(async () => root.render(createElement(ModelEditor, {
      api: api as any,
      org: { id: 'o1', slug: 'acme', name: 'Acme' },
      modelId: 'm1',
      tables,
      onBuilt,
      onDeleted,
      onTablesCreated,
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
  const tool = (label: string) => document.querySelector(`[data-slot="model-toolbar"] button[aria-label="${label}"]`) as HTMLButtonElement;
  const fill = async (selector: string, value: string) => {
    const input = document.querySelector(selector) as HTMLInputElement;
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!;
    await act(async () => {
      setter.call(input, value);
      input.dispatchEvent(new Event('input', { bubbles: true }));
    });
  };
  const select = async (alias: string | null) => {
    await act(async () => flow.props.onSelectionChange({ nodes: alias ? [{ id: alias }] : [], edges: [] }));
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

  describe('toolbar', () => {
    it('has every tool, and enables them by the selection and the drafts', async () => {
      await render();
      const labels = ['New table', 'Table color', 'Edit table', 'Remove table', 'Arrange',
        'Zoom in', 'Zoom out', 'Fit view', 'Create in database'];
      labels.forEach((l) => expect(tool(l), l).toBeTruthy());
      expect(tool('Table color').disabled).toBe(true);
      expect(tool('Edit table').disabled).toBe(true);
      expect(tool('Remove table').disabled).toBe(true);
      expect(tool('Create in database').disabled).toBe(true);
      expect(tool('New table').disabled).toBe(false);

      await select('orders');
      expect(tool('Table color').disabled).toBe(false);
      expect(tool('Remove table').disabled).toBe(false);
      // A table of the database is not edited here.
      expect(tool('Edit table').disabled).toBe(true);
    });

    it('edits only a draft and creates only with drafts', async () => {
      api.model.mockResolvedValueOnce(view(withDraft()));
      await render();
      expect(tool('Create in database').disabled).toBe(false);
      await select('items');
      expect(tool('Edit table').disabled).toBe(false);
      await act(async () => tool('Edit table').click());
      expect(document.body.textContent).toContain('Edit table items');
      expect((document.querySelector('input[aria-label="Column 2 name"]') as HTMLInputElement).value).toBe('label');
    });

    it('removes the selected table', async () => {
      await render();
      await select('customers');
      await act(async () => tool('Remove table').click());
      expect([...document.querySelectorAll('[data-node]')].map((n) => n.getAttribute('data-node'))).toEqual(['orders']);
      expect(document.querySelectorAll('[data-edge]').length).toBe(0);
      expect(tool('Remove table').disabled).toBe(true);
    });

    it('colors the selected table from the palette', async () => {
      await render();
      await select('orders');
      await act(async () => tool('Table color').click());
      await act(async () => (document.querySelector('button[aria-label="blue"]') as HTMLButtonElement).click());
      const band = node('orders').querySelector('[data-slot="model-table-band"]')!;
      expect(band.getAttribute('style')).toContain('var(--model-blue)');
      await act(async () => button('Save').click());
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
      await act(async () => {
        tool('Arrange').click();
        await new Promise((r) => window.requestAnimationFrame(() => r(null)));
      });
      expect(flow.fitView).toHaveBeenCalled();
      await act(async () => button('Save').click());
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
      await act(async () => button('Save').click());
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

    it('opens the edit of a draft on a double click', async () => {
      api.model.mockResolvedValueOnce(view(withDraft()));
      await render();
      await act(async () => flow.props.onNodeDoubleClick({}, { id: 'orders' }));
      expect(document.body.textContent).not.toContain('Edit table');
      await act(async () => flow.props.onNodeDoubleClick({}, { id: 'items' }));
      expect(document.body.textContent).toContain('Edit table items');
      await fill('input[aria-label="Column 2 name"]', 'title');
      await act(async () => button('Save table').click());
      expect(node('items').textContent).toContain('title');
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

    it('marks the primary key and the linked columns', async () => {
      await render();
      const row = (alias: string, column: string) => [...node(alias).querySelectorAll('li')]
        .find((li) => li.querySelector(`[data-id="${column}"]`)) as HTMLElement;
      expect(row('orders', 'id').querySelector('[aria-label="primary key"]')).not.toBeNull();
      expect(row('orders', 'amount').querySelector('[aria-label="primary key"]')).toBeNull();
      expect(row('customers', 'id').querySelector('[aria-label="primary key"]')).toBeNull();
      expect(row('orders', 'customer_id').querySelector('[aria-label="linked"]')).not.toBeNull();
      expect(row('customers', 'id').querySelector('[aria-label="linked"]')).not.toBeNull();
      expect(row('customers', 'name').querySelector('[aria-label="linked"]')).toBeNull();
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
