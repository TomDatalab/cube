import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import {
  authHeaders, getSession, getWorkspace, login, logout, setWorkspace, workspaceHeaders,
} from '../shared/auth';

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

  it('logs in a user who is not a platform admin', async () => {
    vi.stubGlobal('fetch', vi.fn(() => answer(200, {
      token: 'jwt',
      expiresAt: inAnHour(),
      user: { id: 'ana', name: 'Ana', platformAdmin: false },
    })));

    const session = await login('/cube', 'ana', 'secret');

    expect(session.user.platformAdmin).toBe(false);
    expect(getSession()?.user.id).toBe('ana');
    expect(authHeaders()).toEqual({ Authorization: 'jwt' });
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

  it('keeps the chosen workspace for the tab', () => {
    expect(getWorkspace()).toBeNull();
    expect(workspaceHeaders()).toEqual({});

    setWorkspace('ws-1');
    expect(getWorkspace()).toBe('ws-1');
    expect(sessionStorage.getItem('cubePlaygroundWorkspace')).toBe('ws-1');
    expect(workspaceHeaders()).toEqual({ 'X-Cube-Workspace': 'ws-1' });

    // "Todos (admin)"
    setWorkspace(null);
    expect(getWorkspace()).toBeNull();
    expect(workspaceHeaders()).toEqual({});
  });

  it('forgets the workspace on log out and on a new login', async () => {
    vi.stubGlobal('fetch', vi.fn(() => answer(200, {
      token: 'jwt',
      expiresAt: inAnHour(),
      user: { id: 'ana', name: 'Ana', platformAdmin: false },
    })));
    setWorkspace('someone-elses');
    await login('/cube', 'ana', 'secret');
    expect(getWorkspace()).toBeNull();

    setWorkspace('ws-1');
    logout();
    expect(getWorkspace()).toBeNull();
  });

  it('applies a workspace without reloading when storage cannot keep it', async () => {
    vi.resetModules();
    const auth = await import('../shared/auth');
    const throwing = {
      getItem: () => { throw new Error('blocked'); },
      setItem: () => { throw new Error('blocked'); },
      removeItem: () => { throw new Error('blocked'); },
    };
    vi.spyOn(window, 'sessionStorage', 'get').mockReturnValue(throwing as unknown as Storage);
    const reload = vi.fn();
    const changed = vi.fn();
    auth.onWorkspaceChange(changed);
    try {
      auth.chooseWorkspace('ws-1', reload);
      auth.chooseWorkspace('ws-1', reload);

      expect(reload).not.toHaveBeenCalled();
      expect(auth.getWorkspace()).toBe('ws-1');
      expect(auth.workspaceHeaders()).toEqual({ 'X-Cube-Workspace': 'ws-1' });
      expect(changed).toHaveBeenCalledTimes(2);
    } finally {
      vi.restoreAllMocks();
    }
  });

  it('reloads once to apply a workspace storage kept', async () => {
    const auth = await import('../shared/auth');
    const reload = vi.fn();

    auth.chooseWorkspace('ws-2', reload);

    expect(reload).toHaveBeenCalledTimes(1);
    expect(sessionStorage.getItem('cubePlaygroundWorkspace')).toBe('ws-2');
  });

  it('survives storage that throws', () => {
    const throwing = {
      getItem: () => { throw new Error('blocked'); },
      setItem: () => { throw new Error('blocked'); },
      removeItem: () => { throw new Error('blocked'); },
    };
    vi.spyOn(window, 'sessionStorage', 'get').mockReturnValue(throwing as unknown as Storage);
    try {
      expect(() => setWorkspace('ws-1')).not.toThrow();
      // Kept in memory for this page.
      expect(getWorkspace()).toBe('ws-1');
      expect(getSession()).toBeNull();
    } finally {
      vi.restoreAllMocks();
    }
  });
});

describe('a 401 from the server', () => {
  beforeEach(() => {
    sessionStorage.clear();
    vi.resetModules();
  });

  function saveSession() {
    sessionStorage.setItem('cubePlaygroundSession', JSON.stringify({
      token: 'jwt',
      expiresAt: inAnHour(),
      user: { id: 'ana', name: 'Ana', platformAdmin: false },
    }));
  }

  it('ends the session once, however many requests fail together', async () => {
    const auth = await import('../shared/auth');
    saveSession();

    auth.onUnauthorized({ status: 401 });
    expect(auth.getSession()).toBeNull();

    // The reload is already on its way: a second 401 changes nothing.
    saveSession();
    auth.onUnauthorized({ status: 401 });
    expect(auth.getSession()?.token).toBe('jwt');
  });

  it('leaves a 403 alone: a refusal, not a lost session', async () => {
    const auth = await import('../shared/auth');
    saveSession();

    auth.onUnauthorized({ status: 403 });

    expect(auth.getSession()?.token).toBe('jwt');
  });

  it('means nothing without a session (a wrong password)', async () => {
    const auth = await import('../shared/auth');
    auth.setWorkspace('ws-1');

    auth.onUnauthorized({ status: 401 });

    expect(auth.getWorkspace()).toBe('ws-1');
  });
});
