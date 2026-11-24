// @vitest-environment jsdom
import { createElement, act } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { adminApi } from '../pages/Admin/api';
import type { AdminApi, User } from '../pages/Admin/api';
import { UsersTab } from '../pages/Admin/UsersTab';
import { WorkspacePanel } from '../pages/Admin/WorkspacePanel';
import { getSession, login } from '../shared/auth';

(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;

const UUID = '3f2b8c1e-5d4a-4e7b-9a10-2c6d8e0f1a23';
const UUID2 = '9b1d7e22-0c3f-4a55-8e61-7a4b2d9c5f10';

function answer(status: number, body: unknown) {
  return Promise.resolve(new Response(body == null ? null : JSON.stringify(body), { status }));
}

const inAnHour = () => new Date(Date.now() + 3600_000).toISOString();

describe('the session', () => {
  beforeEach(() => sessionStorage.clear());
  afterEach(() => vi.unstubAllGlobals());

  it('parses the username of the login answer', async () => {
    vi.stubGlobal('fetch', vi.fn(() => answer(200, {
      token: 'jwt',
      expiresAt: inAnHour(),
      user: { id: UUID, username: 'ana', name: null, platformAdmin: false },
    })));

    const session = await login('/cube', 'ana', 'secret');

    expect(session.user.id).toBe(UUID);
    expect(session.user.username).toBe('ana');
    expect(getSession()?.user.username).toBe('ana');
  });

  it('falls back to the id for a session stored without a username', () => {
    sessionStorage.setItem('cubePlaygroundSession', JSON.stringify({
      token: 'jwt',
      expiresAt: inAnHour(),
      user: { id: 'ana', name: null, platformAdmin: false },
    }));

    expect(getSession()?.user.username).toBe('ana');
  });
});

describe('the Admin API client with ids', () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    sessionStorage.clear();
    sessionStorage.setItem('cubePlaygroundSession', JSON.stringify({
      token: 'jwt',
      expiresAt: inAnHour(),
      user: { id: UUID, username: 'admin', name: null, platformAdmin: true },
    }));
    fetchMock = vi.fn(() => answer(200, {}));
    vi.stubGlobal('fetch', fetchMock);
  });

  afterEach(() => vi.unstubAllGlobals());

  const last = () => {
    const [url, init] = fetchMock.mock.calls.at(-1)!;
    return { url, init };
  };

  it('creates a user by username', async () => {
    await adminApi('/cube').createUser({ username: 'ana', password: 'a-long-password' });
    expect(JSON.parse(last().init.body)).toEqual({ username: 'ana', password: 'a-long-password' });
  });

  it('renames a user with the username in the patch', async () => {
    await adminApi('/cube').updateUser(UUID, { username: 'ana2' });
    expect(last().url).toBe(`/cube/v1/users/${UUID}`);
    expect(JSON.parse(last().init.body)).toEqual({ username: 'ana2' });
  });

  it('lists rule members with their usernames', async () => {
    fetchMock.mockReturnValueOnce(answer(200, {
      members: [{ user: UUID, username: 'ana' }, { user: UUID2, username: null }],
    }));

    expect(await adminApi('/cube').ruleMembers('r1')).toEqual([
      { user: UUID, username: 'ana' },
      { user: UUID2, username: null },
    ]);
  });
});

function mount() {
  window.matchMedia = vi.fn().mockImplementation((query: string) => ({
    matches: false,
    media: query,
    addListener: vi.fn(),
    removeListener: vi.fn(),
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  })) as any;
  const host = document.createElement('div');
  document.body.appendChild(host);
  return { root: createRoot(host) };
}

const flush = () => act(async () => {
  await new Promise((r) => setTimeout(r, 0));
});

const ana: User = {
  id: UUID, username: 'ana', name: 'Ana', email: null, disabled: false, createdAt: '2026-01-01',
};

describe('the Users tab', () => {
  let root: ReturnType<typeof createRoot>;
  let updateUser: ReturnType<typeof vi.fn>;
  let api: AdminApi;

  beforeEach(() => {
    ({ root } = mount());
    updateUser = vi.fn(() => Promise.resolve({}));
    api = new Proxy({
      searchUsers: () => Promise.resolve({ items: [ana], total: 1 }),
      searchWorkspaces: () => Promise.resolve({ items: [], total: 0 }),
      searchRules: () => Promise.resolve({ items: [], total: 0 }),
      updateUser,
    } as any, {
      get: (target, key) => (target as any)[key] ?? (() => Promise.resolve({ items: [], total: 0 })),
    }) as AdminApi;
  });

  afterEach(() => {
    act(() => root.unmount());
    document.body.innerHTML = '';
  });

  it('shows the username and an Id column with the UUID', async () => {
    act(() => root.render(createElement(UsersTab, { api, currentUser: UUID })));
    await flush();

    const headers = [...document.body.querySelectorAll('th')].map((th) => th.textContent);
    expect(headers).toContain('Username');
    expect(headers).toContain('Id');
    const row = document.body.querySelector('tbody tr:not([aria-hidden])')!;
    expect(row.textContent).toContain('ana');
    expect(row.textContent).toContain(UUID);
    // "you" is found by id, not by name.
    expect(row.textContent).toContain('you');
  });

  it('sends the new username to rename, by id', async () => {
    act(() => root.render(createElement(UsersTab, { api, currentUser: null })));
    await flush();

    act(() => {
      (document.body.querySelector('[aria-label="Edit ana"]') as HTMLElement).click();
    });
    await flush();
    const input = document.body.querySelector('input#username') as HTMLInputElement;
    expect(input.value).toBe('ana');
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!;
    act(() => {
      setter.call(input, 'ana2');
      input.dispatchEvent(new Event('input', { bubbles: true }));
    });
    act(() => {
      (document.body.querySelector('.ant-modal-footer .ant-btn-primary') as HTMLElement).click();
    });
    await flush();

    expect(updateUser).toHaveBeenCalledWith(UUID, { username: 'ana2', name: 'Ana' });
  });
});

describe('the workspace members', () => {
  it('shows usernames and removes by id', async () => {
    const { root } = mount();
    const removeMember = vi.fn(() => Promise.resolve({}));
    const api = {
      members: () => Promise.resolve([
        { user: UUID, username: 'ana', role: 'viewer' },
        { user: UUID2, username: null, role: 'editor' },
      ]),
      searchUsers: () => Promise.resolve({ items: [], total: 0 }),
      removeMember,
    } as unknown as AdminApi;
    const workspace = {
      id: 'w1', organizationId: 'o1', name: 'W', createdAt: '', createdBy: null,
    };

    act(() => root.render(createElement(WorkspacePanel, {
      api, workspace, connections: [], reloadConnections: async () => {},
    })));
    await flush();

    const text = document.body.textContent!;
    expect(text).toContain('ana');
    expect(text).toContain(UUID2);
    expect(text).not.toContain(UUID);
    act(() => root.unmount());
    document.body.innerHTML = '';
  });
});
