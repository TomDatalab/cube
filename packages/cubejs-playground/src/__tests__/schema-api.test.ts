import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { buildRbac } from '../hooks/rbac';
import { modelApi, openBlock } from '../pages/Schema/api';

function answer(status: number, body: unknown) {
  return Promise.resolve(new Response(body == null ? null : JSON.stringify(body), { status }));
}

const owned = [
  { id: 'o1', slug: 'digital', name: 'Digital' },
  { id: 'o2', slug: 'sales', name: 'Vendas' },
];

function session(platformAdmin: boolean, ownedOrganizations = owned) {
  return {
    token: 'jwt',
    expiresAt: new Date(Date.now() + 3600_000).toISOString(),
    user: { id: 'u', username: 'u', name: null, platformAdmin, ownedOrganizations },
  };
}

describe('model API client', () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    sessionStorage.clear();
    sessionStorage.setItem('cubePlaygroundSession', JSON.stringify(session(false)));
    fetchMock = vi.fn(() => answer(200, {}));
    vi.stubGlobal('fetch', fetchMock);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  const ownerRbac = () => buildRbac({ authRequired: true }, session(false));
  const adminRbac = () => buildRbac({ authRequired: true }, session(true, []));
  const lastCall = () => {
    const [url, init] = fetchMock.mock.calls.at(-1)!;
    return { url, init, headers: new Headers(init?.headers) };
  };

  it('owner list comes from ownedOrganizations and filters by q', async () => {
    const api = modelApi(ownerRbac());

    const all = await api.list('', 0);
    expect(all.organizations.map((o) => o.slug)).toEqual(['digital', 'sales']);
    expect(all.organizations[0].id).toBe('o1');
    expect(all.total).toBe(2);

    const found = await api.list('vend', 0);
    expect(found.organizations.map((o) => o.slug)).toEqual(['sales']);
    expect(found.total).toBe(1);
    expect((await api.list('', 5)).organizations).toEqual([]);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('admin list comes from /playground/tree', async () => {
    fetchMock.mockReturnValueOnce(answer(200, { organizations: [{ slug: '_platform', name: 'Platform' }], total: 1 }));

    const result = await modelApi(adminRbac()).list('a b', 50);

    expect(result.total).toBe(1);
    expect(lastCall().url).toBe('playground/tree?limit=50&offset=50&q=a+b');
  });

  it('detail url for an org and for _platform', async () => {
    const api = modelApi(ownerRbac());
    const summary = { id: 'o/1', slug: 'digital', name: 'Digital' };

    await api.detail(summary);
    expect(lastCall().url).toBe('/cube/v1/organizations/o%2F1/model');
    expect(lastCall().headers.get('Authorization')).toBe('jwt');
    expect(lastCall().headers.get('X-Cube-Workspace')).toBeNull();

    await api.detail({ slug: '_platform', name: 'Platform' });
    expect(lastCall().url).toBe('/cube/v1/platform/model');
  });

  it('file, save, remove, generate and move use the model routes', async () => {
    const api = modelApi(adminRbac(), '/x');
    const org = { id: 'o1', slug: 'digital', name: 'Digital' };
    const platform = { slug: '_platform', name: 'Platform' };

    await api.file(org, 'f1');
    expect(lastCall().url).toBe('/x/v1/organizations/o1/model/files/f1');

    await api.save(platform, 'f1', 'cubes: []', 3);
    expect(lastCall().url).toBe('/x/v1/platform/model/files/f1');
    expect(lastCall().init.method).toBe('PUT');
    expect(JSON.parse(lastCall().init.body)).toEqual({ content: 'cubes: []', version: 3 });

    await api.remove(org, 'f1', 3);
    expect(lastCall().url).toBe('/x/v1/organizations/o1/model/files/f1?version=3');
    expect(lastCall().init.method).toBe('DELETE');

    await api.generate(org, [['default.public', 't']]);
    expect(lastCall().url).toBe('/x/v1/organizations/o1/model/generate');
    expect(JSON.parse(lastCall().init.body)).toEqual({ tables: [['default.public', 't']] });

    await api.move('f1', null);
    expect(lastCall().url).toBe('/x/v1/model/files/f1/organization');
    expect(JSON.parse(lastCall().init.body)).toEqual({ organizationId: null });
  });

  it('errors carry the status and the server message', async () => {
    fetchMock.mockReturnValueOnce(answer(404, { error: 'gone' }));
    await expect(modelApi(ownerRbac()).detail(owned[0])).rejects.toMatchObject({ message: 'gone', status: 404 });
  });
});

describe('canModel', () => {
  it('canModel for admin, owner and member', () => {
    const context = { authRequired: true };
    expect(buildRbac(context, session(true, [])).canModel).toBe(true);
    expect(buildRbac(context, session(false)).canModel).toBe(true);
    expect(buildRbac(context, session(false, [])).canModel).toBe(false);
    expect(buildRbac({}, null).canModel).toBe(true);
  });
});

describe('openBlock', () => {
  const cube = { name: 'c', dataSource: 'sales_db', measures: [{ name: 'c.count' }], dimensions: [] };
  const workspaces = [{ id: 'w1', name: 'Main' }];

  it('needs a member', () => {
    const ctx = { owner: false, workspaces: [], chosen: null, released: null };
    expect(openBlock({ ...cube, measures: [] }, ctx).disabled).toBe(true);
  });

  it('asks an owner for a workspace', () => {
    const block = openBlock(cube, { owner: true, workspaces: [], chosen: null, released: null });
    expect(block.disabled).toBe(true);
    expect(block.hint).toBe('Create a workspace in this organization to query it');
    expect(openBlock(cube, { owner: false, workspaces: [], chosen: null, released: null }).disabled).toBe(false);
  });

  it('warns when the connection is not released to the chosen workspace', () => {
    const block = openBlock(cube, { owner: true, workspaces, chosen: 'w1', released: ['other'] });
    expect(block.disabled).toBe(false);
    expect(block.warning).toBe('Release the connection sales_db to the workspace Main');
    expect(openBlock(cube, { owner: true, workspaces, chosen: 'w1', released: ['sales_db'] }).warning).toBeUndefined();
  });
});

describe('model edit lock client', () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    sessionStorage.clear();
    sessionStorage.setItem('cubePlaygroundSession', JSON.stringify(session(false)));
    fetchMock = vi.fn(() => answer(200, { held: true, holder: 'me' }));
    vi.stubGlobal('fetch', fetchMock);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  const call = (i: number) => {
    const [url, init] = fetchMock.mock.calls[i];
    return { url, init, headers: new Headers(init?.headers) };
  };

  it('claims, takes and releases the lock, and sends the token on writes', async () => {
    const api = modelApi(buildRbac({ authRequired: true }, session(false)));
    expect(await api.lockModel('m/1', 't1')).toEqual({ held: true, holder: 'me' });
    expect(call(0).url).toBe('/cube/v1/cube-models/m%2F1/lock');
    expect(call(0).init.method).toBe('POST');
    expect(JSON.parse(call(0).init.body)).toEqual({ token: 't1' });
    await api.takeModelLock('m1', 't1');
    expect(call(1).url).toBe('/cube/v1/cube-models/m1/lock/take');
    fetchMock.mockReturnValueOnce(answer(204, null));
    await api.releaseModelLock('m1', 't 1');
    expect(call(2).url).toBe('/cube/v1/cube-models/m1/lock?token=t%201');
    expect(call(2).init.method).toBe('DELETE');
    expect(call(2).init.keepalive).toBe(true);
    expect(call(2).headers.get('Authorization')).toBe('jwt');

    await api.saveModel('m1', { version: 1 }, 't1');
    await api.deleteModel('m1', 1, 't1');
    await api.buildModel('m1', false, 't1');
    await api.createTables('m1', 1, true, 't1');
    [3, 4, 5, 6].forEach((i) => expect(call(i).headers.get('X-Model-Lock')).toBe('t1'));
    await api.saveModel('m1', { version: 1 });
    expect(call(7).headers.get('X-Model-Lock')).toBeNull();
  });
});
