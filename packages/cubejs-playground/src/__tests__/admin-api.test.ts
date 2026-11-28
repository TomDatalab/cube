import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import {
  adminApi, assignableDataSources, attributesFromPairs, moveTargets, ownerLabel,
  restrictionSummary, withImplied,
} from '../pages/Admin/api';

function answer(status: number, body: unknown) {
  return Promise.resolve(new Response(body == null ? null : JSON.stringify(body), { status }));
}

function saveSession() {
  sessionStorage.setItem('cubePlaygroundSession', JSON.stringify({
    token: 'jwt',
    expiresAt: new Date(Date.now() + 3600_000).toISOString(),
    user: { id: 'admin', name: null, platformAdmin: true },
  }));
}

describe('the Admin page API client', () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    sessionStorage.clear();
    saveSession();
    fetchMock = vi.fn(() => answer(200, {}));
    vi.stubGlobal('fetch', fetchMock);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  function lastCall() {
    const [url, init] = fetchMock.mock.calls.at(-1)!;
    return { url, init, headers: new Headers(init.headers) };
  }

  it('sends the login token and no workspace', async () => {
    sessionStorage.setItem('cubePlaygroundWorkspace', 'ws-1');
    fetchMock.mockReturnValueOnce(answer(200, { users: [{ id: 'ana' }] }));

    const users = await adminApi('/cube').users();

    expect(users).toEqual([{ id: 'ana' }]);
    const { url, headers } = lastCall();
    expect(url).toBe('/cube/v1/users');
    expect(headers.get('Authorization')).toBe('jwt');
    expect(headers.get('X-Cube-Workspace')).toBeNull();
  });

  it('posts JSON bodies', async () => {
    await adminApi('/cube').createUser({ username: 'ana', name: 'Ana', password: 'a-long-password' });

    const { url, init, headers } = lastCall();
    expect(url).toBe('/cube/v1/users');
    expect(init.method).toBe('POST');
    expect(headers.get('Content-Type')).toBe('application/json');
    expect(JSON.parse(init.body)).toEqual({ username: 'ana', name: 'Ana', password: 'a-long-password' });
  });

  it('encodes ids in paths', async () => {
    await adminApi('/cube/').setMember('ws/1', 'a b', 'editor');

    const { url, init } = lastCall();
    expect(url).toBe('/cube/v1/workspaces/ws%2F1/members/a%20b');
    expect(init.method).toBe('PUT');
    expect(JSON.parse(init.body)).toEqual({ role: 'editor' });
  });

  it('accepts an empty answer', async () => {
    fetchMock.mockReturnValueOnce(answer(204, null));

    await expect(adminApi('/cube').removeOwner('org-1', 'ana')).resolves.toBeNull();
    expect(lastCall().init.method).toBe('DELETE');
  });

  it('throws the server message', async () => {
    fetchMock.mockReturnValueOnce(answer(400, { error: 'the last owner cannot be removed' }));

    await expect(adminApi('/cube').removeOwner('org-1', 'ana'))
      .rejects.toThrow('the last owner cannot be removed');
  });

  it('reads the data sources a connection can be made from', async () => {
    fetchMock.mockReturnValueOnce(answer(200, {
      dataSources: [{ name: 'default', type: 'postgres' }],
    }));

    await expect(adminApi('/cube').dataSources()).resolves.toEqual([
      { name: 'default', type: 'postgres' },
    ]);
  });

  it('reports removing a user who is not an owner', async () => {
    fetchMock.mockReturnValueOnce(answer(404, { error: 'ana is not an owner of this organization' }));

    await expect(adminApi('/cube').removeOwner('org-1', 'ana'))
      .rejects.toThrow('ana is not an owner of this organization');
  });

  it('offers only data sources no organization holds, never default', async () => {
    fetchMock.mockImplementation((url: string) => {
      if (url === '/cube/v1/data-sources') {
        return answer(200, {
          dataSources: [
            { name: 'default', type: 'postgres' },
            { name: 'sales', type: 'postgres' },
            { name: 'mine', type: 'sqlite' },
            { name: 'free', type: 'duckdb' },
          ],
        });
      }
      if (url === '/cube/v1/connections') {
        return answer(200, {
          connections: [
            { name: 'mine', organizationId: 'org-1', origin: 'created' },
            { name: 'sales', organizationId: 'org-2', origin: 'assigned' },
          ],
        });
      }
      return answer(404, { error: 'unexpected' });
    });

    await expect(adminApi('/cube').assignableDataSources()).resolves.toEqual([
      { name: 'free', type: 'duckdb' },
    ]);
  });

  it('searches users a page at a time', async () => {
    fetchMock.mockReturnValueOnce(answer(200, { users: [{ id: 'ana' }], total: 61 }));

    const page = await adminApi('/cube').searchUsers('ana', 2);

    expect(page).toEqual({ items: [{ id: 'ana' }], total: 61 });
    expect(lastCall().url).toBe('/cube/v1/users?q=ana&limit=50&offset=50');
  });

  it('searches organizations', async () => {
    fetchMock.mockReturnValueOnce(answer(200, { organizations: [{ id: 'o1' }], total: 1 }));

    const page = await adminApi('/cube').searchOrganizations('a b', 1);

    expect(page).toEqual({ items: [{ id: 'o1' }], total: 1 });
    expect(lastCall().url).toBe('/cube/v1/organizations?q=a+b&limit=50&offset=0');
  });

  it('searches workspaces of an organization without sending an empty q', async () => {
    fetchMock.mockReturnValueOnce(answer(200, { workspaces: [{ id: 'w1' }], total: 1 }));

    const page = await adminApi('/cube').searchWorkspaces({ q: '', organizationId: 'o1', page: 1 });

    expect(page).toEqual({ items: [{ id: 'w1' }], total: 1 });
    expect(lastCall().url).toBe('/cube/v1/workspaces?limit=50&offset=0&organizationId=o1');
  });

  it('lists the tracked connections', async () => {
    fetchMock.mockReturnValueOnce(answer(200, {
      connections: [{ name: 'mine', organizationId: 'o1', origin: 'created' }],
    }));

    await expect(adminApi('/cube').trackedConnections()).resolves.toEqual([
      { name: 'mine', organizationId: 'o1', origin: 'created' },
    ]);
    expect(lastCall().url).toBe('/cube/v1/connections');
  });

  it('filters assignable data sources', () => {
    expect(assignableDataSources(
      [{ name: 'default', type: null }, { name: 'a', type: null }, { name: 'b', type: null }],
      ['b']
    )).toEqual([{ name: 'a', type: null }]);
  });

  it('moves a connection to another organization', async () => {
    fetchMock.mockReturnValueOnce(answer(204, null));

    await adminApi('/cube').moveConnection('sales/eu', 'o2');

    const { url, init } = lastCall();
    expect(url).toBe('/cube/v1/connections/sales%2Feu/organization');
    expect(init.method).toBe('PUT');
    expect(JSON.parse(init.body)).toEqual({ organizationId: 'o2' });
  });

  it('labels a data source by its owner', () => {
    expect(ownerLabel({ organization: { id: 'o1', name: 'ON' } })).toBe('ON');
    expect(ownerLabel({ organization: null })).toBe('Platform');
    expect(ownerLabel({})).toBe('Platform');
  });

  it('offers every other organization to move to, by name', () => {
    const org = (id: string, name: string) => ({
      id, name, slug: id, createdAt: '', createdBy: null,
    });
    expect(moveTargets([org('o1', 'ON'), org('o2', 'Digital'), org('o3', 'Agro')], 'o1'))
      .toEqual([org('o3', 'Agro'), org('o2', 'Digital')]);
  });

  it('searches rules by workspace', async () => {
    fetchMock.mockReturnValueOnce(answer(200, { rules: [{ id: 'r1' }], total: 1 }));

    const page = await adminApi('/cube').searchRules({ q: 'ops', workspaceId: 'w1', page: 2 });

    expect(page).toEqual({ items: [{ id: 'r1' }], total: 1 });
    const url = new URL(lastCall().url, 'http://x');
    expect(url.pathname).toBe('/cube/v1/rules');
    expect(url.searchParams.get('q')).toBe('ops');
    expect(url.searchParams.get('workspaceId')).toBe('w1');
    expect(url.searchParams.get('offset')).toBe('50');
  });

  it('moves a rule to another workspace', async () => {
    await adminApi('/cube').updateRule('r/1', { workspaceId: 'w2' });

    const { url, init } = lastCall();
    expect(url).toBe('/cube/v1/rules/r%2F1');
    expect(init.method).toBe('PATCH');
    expect(JSON.parse(init.body)).toEqual({ workspaceId: 'w2' });
  });

  it('lists and adds rule members', async () => {
    fetchMock.mockReturnValueOnce(answer(200, { members: [{ user: 'ana' }, { user: 'bia' }] }));

    expect(await adminApi('/cube').ruleMembers('r1')).toEqual([
      { user: 'ana', username: null },
      { user: 'bia', username: null },
    ]);

    await adminApi('/cube').addRuleMember('r1', 'a b');
    const { url, init } = lastCall();
    expect(url).toBe('/cube/v1/rules/r1/members/a%20b');
    expect(init.method).toBe('PUT');
  });

  it('adds what a permission implies, in order', () => {
    expect(withImplied(['query'])).toEqual(['view', 'query']);
    expect(withImplied(['write_objects'])).toEqual(['view', 'read_objects', 'write_objects']);
    expect(withImplied(['manage'])).toEqual(
      ['view', 'query', 'read_objects', 'write_objects', 'execute', 'manage']
    );
    expect(withImplied([])).toEqual([]);
  });

  it('creates an organization role and a rule that uses it', async () => {
    await adminApi('/cube').createRole('o/1', { name: 'Analyst', permissions: ['view', 'query'] });
    let { url, init } = lastCall();
    expect(url).toBe('/cube/v1/organizations/o%2F1/roles');
    expect(JSON.parse(init.body)).toEqual({ name: 'Analyst', permissions: ['view', 'query'] });

    await adminApi('/cube').createRule('w1', { description: 'd', type: 't', roleId: 'r1' });
    ({ url, init } = lastCall());
    expect(url).toBe('/cube/v1/workspaces/w1/rules');
    expect(JSON.parse(init.body)).toEqual({ description: 'd', type: 't', roleId: 'r1' });
  });

  it('userAttributes and setUserAttributes call the attributes route', async () => {
    fetchMock.mockReturnValueOnce(answer(200, { attributes: { region: ['eu', 'us'] } }));
    expect(await adminApi('/cube').userAttributes('bia')).toEqual({ region: ['eu', 'us'] });
    let { url, init } = lastCall();
    expect(url).toBe('/cube/v1/users/bia/attributes');
    expect(init.method).toBe('GET');

    fetchMock.mockReturnValueOnce(answer(200, { attributes: { region: ['eu'] } }));
    expect(await adminApi('/cube').setUserAttributes('bia', { region: ['eu'] })).toEqual({ region: ['eu'] });
    ({ url, init } = lastCall());
    expect(url).toBe('/cube/v1/users/bia/attributes');
    expect(init.method).toBe('PUT');
    expect(JSON.parse(init.body)).toEqual({ attributes: { region: ['eu'] } });
  });

  it('modelMembers calls the organization route', async () => {
    const cubes = [{ name: 'orders', measures: [], dimensions: [{ name: 'orders.region' }], segments: [] }];
    fetchMock.mockReturnValueOnce(answer(200, { cubes }));
    expect(await adminApi('/cube').modelMembers('o1')).toEqual(cubes);
    expect(lastCall().url).toBe('/cube/v1/organizations/o1/model-members');
  });

  it('createRole sends restrictions', async () => {
    const restrictions = {
      rowFilters: [{ member: 'orders.region', operator: 'equals' as const, attribute: 'region' }],
      blockedMembers: ['orders.margin'],
    };
    await adminApi('/cube').createRole('o1', { name: 'Analyst', permissions: ['view'], restrictions });
    const { url, init } = lastCall();
    expect(url).toBe('/cube/v1/organizations/o1/roles');
    expect(JSON.parse(init.body)).toEqual({ name: 'Analyst', permissions: ['view'], restrictions });
  });

  it('summarizes restrictions', () => {
    const f = { member: 'a.b', operator: 'equals' as const, values: ['x'] };
    expect(restrictionSummary({ rowFilters: [f, f], blockedMembers: ['a', 'b', 'c'] })).toBe('2 filters · 3 blocked');
    expect(restrictionSummary({ rowFilters: [f], blockedMembers: ['a'] })).toBe('1 filter · 1 blocked');
    expect(restrictionSummary({ rowFilters: [], blockedMembers: ['a'] })).toBe('1 blocked');
    expect(restrictionSummary({ rowFilters: [], blockedMembers: [] })).toBe('');
  });

  it('builds attributes from pairs, with own-property duplicate checks', () => {
    const a = attributesFromPairs([
      { name: 'constructor', values: ['x'] },
      { name: ' toString ', values: ['y'] },
      { name: '__proto__', values: ['z'] },
    ]);
    expect(JSON.parse(JSON.stringify(a))).toEqual({ constructor: ['x'], toString: ['y'], ['__proto__']: ['z'] });
    expect(Object.keys(a)).toEqual(['constructor', 'toString', '__proto__']);
    expect(() => attributesFromPairs([{ name: 'r', values: [] }, { name: 'r ', values: [] }])).toThrow('twice');
  });
});
