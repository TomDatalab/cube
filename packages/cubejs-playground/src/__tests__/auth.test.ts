import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { authHeaders, getSession, login, logout } from '../shared/auth';

function answer(status: number, body: unknown) {
  return Promise.resolve(new Response(JSON.stringify(body), { status }));
}

const inAnHour = () => new Date(Date.now() + 3600_000).toISOString();

describe('Playground login', () => {
  beforeEach(() => {
    sessionStorage.clear();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('starts logged out', () => {
    expect(getSession()).toBeNull();
    expect(authHeaders()).toEqual({});
  });

  it('logs a platform admin in and sends the token afterwards', async () => {
    const fetchMock = vi.fn(() => answer(200, {
      token: 'jwt',
      expiresAt: inAnHour(),
      user: { id: 'admin', name: 'Admin', platformAdmin: true },
    }));
    vi.stubGlobal('fetch', fetchMock);

    const session = await login('/cube', 'admin', 'secret');

    expect(fetchMock).toHaveBeenCalledWith('/cube/v1/auth/login', expect.objectContaining({
      method: 'POST',
      body: JSON.stringify({ username: 'admin', password: 'secret' }),
    }));
    expect(session.user.id).toBe('admin');
    expect(getSession()?.token).toBe('jwt');
    expect(authHeaders()).toEqual({ Authorization: 'jwt' });
  });

  it('shows the server message for a wrong password', async () => {
    vi.stubGlobal('fetch', vi.fn(() => answer(401, { error: 'Invalid username or password' })));

    await expect(login('/cube', 'admin', 'wrong')).rejects.toThrow('Invalid username or password');
    expect(getSession()).toBeNull();
  });

  it('refuses a user who is not a platform admin', async () => {
    vi.stubGlobal('fetch', vi.fn(() => answer(200, {
      token: 'jwt',
      expiresAt: inAnHour(),
      user: { id: 'ana', name: 'Ana', platformAdmin: false },
    })));

    await expect(login('/cube', 'ana', 'secret')).rejects.toThrow(/platform admins/);
    expect(getSession()).toBeNull();
  });

  it('forgets an expired session', () => {
    sessionStorage.setItem('cubePlaygroundSession', JSON.stringify({
      token: 'old',
      expiresAt: new Date(Date.now() - 1000).toISOString(),
      user: { id: 'admin', name: 'Admin', platformAdmin: true },
    }));

    expect(getSession()).toBeNull();
    expect(sessionStorage.getItem('cubePlaygroundSession')).toBeNull();
  });

  it('logs out', async () => {
    vi.stubGlobal('fetch', vi.fn(() => answer(200, {
      token: 'jwt',
      expiresAt: inAnHour(),
      user: { id: 'admin', name: 'Admin', platformAdmin: true },
    })));
    await login('/cube', 'admin', 'secret');

    logout();

    expect(getSession()).toBeNull();
    expect(authHeaders()).toEqual({});
  });
});
