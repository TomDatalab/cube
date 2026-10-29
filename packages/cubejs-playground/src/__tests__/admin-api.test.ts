import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { adminApi } from '../pages/Admin/api';

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
});
