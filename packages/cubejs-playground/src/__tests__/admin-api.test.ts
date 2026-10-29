import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { adminApi, assignableDataSources } from '../pages/Admin/api';

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
    await adminApi('/cube').createUser({ id: 'ana', name: 'Ana', password: 'a-long-password' });

    const { url, init, headers } = lastCall();
    expect(url).toBe('/cube/v1/users');
    expect(init.method).toBe('POST');
    expect(headers.get('Content-Type')).toBe('application/json');
    expect(JSON.parse(init.body)).toEqual({ id: 'ana', name: 'Ana', password: 'a-long-password' });
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
            { name: 'theirs', type: 'sqlite' },
            { name: 'free', type: 'duckdb' },
          ],
        });
      }
      if (url === '/cube/v1/organizations/org-1/connections') {
        return answer(200, { connections: [{ name: 'mine', type: 'sqlite', origin: 'created', workspaces: [] }] });
      }
      if (url === '/cube/v1/organizations/org-2/connections') {
        return answer(200, {
          connections: [
            { name: 'theirs', type: 'sqlite', origin: 'created', workspaces: [] },
            { name: 'sales', type: 'postgres', origin: 'assigned', workspaces: ['ws-1'] },
          ],
        });
      }
      return answer(404, { error: 'unexpected' });
    });

    await expect(adminApi('/cube').assignableDataSources(['org-1', 'org-2'])).resolves.toEqual([
      { name: 'free', type: 'duckdb' },
    ]);
  });

  it('filters assignable data sources', () => {
    expect(assignableDataSources(
      [{ name: 'default', type: null }, { name: 'a', type: null }, { name: 'b', type: null }],
      ['b']
    )).toEqual([{ name: 'a', type: null }]);
  });

  it('unassigns a connection from its organization', async () => {
    fetchMock.mockReturnValueOnce(answer(204, null));

    await adminApi('/cube').unassignConnection('sales/eu');

    const { url, init } = lastCall();
    expect(url).toBe('/cube/v1/connections/sales%2Feu/organization');
    expect(init.method).toBe('DELETE');
    expect(init.body).toBeUndefined();
  });
});
