// @vitest-environment jsdom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { adminApi } from '../pages/Admin/api';
import type { AdminApi, Group } from '../pages/Admin/api';
import { countLabel, GroupsTab, ruleOptions } from '../pages/Admin/GroupsTab';

vi.mock('../pages/Admin/RemoteSelect', () => ({
  organizationOptions: () => async () => [],
  userOptions: () => async () => [],
  // A button that picks "u1" for users and "rule1" for rules.
  RemoteSelect: ({ onChange, placeholder }: any) => (
    createElement('button', {
      'data-pick': placeholder,
      onClick: () => onChange(placeholder === 'Rule' ? 'rule1' : 'u1', placeholder === 'Rule' ? 'W / Analysts' : 'ana'),
    }, placeholder)
  ),
}));

(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;

function answer(status: number, body: unknown) {
  return Promise.resolve(new Response(body == null ? null : JSON.stringify(body), { status }));
}

describe('the groups API client', () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    sessionStorage.clear();
    sessionStorage.setItem('cubePlaygroundSession', JSON.stringify({
      token: 'jwt',
      expiresAt: new Date(Date.now() + 3600_000).toISOString(),
      user: { id: 'admin', username: 'admin', name: null, platformAdmin: true },
    }));
    fetchMock = vi.fn(() => answer(200, {}));
    vi.stubGlobal('fetch', fetchMock);
  });

  afterEach(() => vi.unstubAllGlobals());

  const last = () => {
    const [url, init] = fetchMock.mock.calls.at(-1)!;
    return { url, method: init.method, body: init.body && JSON.parse(init.body) };
  };

  it('searches groups by organization', async () => {
    fetchMock.mockReturnValueOnce(answer(200, { groups: [{ id: 'g1' }], total: 1 }));
    const page = await adminApi('/cube').searchGroups({ q: 'fin', organizationId: 'o1', page: 2 });
    expect(page).toEqual({ items: [{ id: 'g1' }], total: 1 });
    expect(last().url).toBe('/cube/v1/groups?q=fin&limit=50&offset=50&organizationId=o1');
  });

  it('creates, renames and deletes a group', async () => {
    const api = adminApi('/cube');
    await api.createGroup('o1', 'Finance');
    expect(last()).toEqual({ url: '/cube/v1/organizations/o1/groups', method: 'POST', body: { name: 'Finance' } });
    await api.renameGroup('g1', 'Fin');
    expect(last()).toEqual({ url: '/cube/v1/groups/g1', method: 'PATCH', body: { name: 'Fin' } });
    await api.deleteGroup('g1');
    expect(last()).toMatchObject({ url: '/cube/v1/groups/g1', method: 'DELETE' });
  });

  it('adds and removes members and rules', async () => {
    const api = adminApi('/cube');
    await api.addGroupMember('g1', 'u/1');
    expect(last()).toMatchObject({ url: '/cube/v1/groups/g1/members/u%2F1', method: 'PUT' });
    await api.removeGroupMember('g1', 'u1');
    expect(last()).toMatchObject({ url: '/cube/v1/groups/g1/members/u1', method: 'DELETE' });
    await api.addGroupRule('g1', 'r1');
    expect(last()).toMatchObject({ url: '/cube/v1/groups/g1/rules/r1', method: 'PUT' });
    await api.removeGroupRule('g1', 'r1');
    expect(last()).toMatchObject({ url: '/cube/v1/groups/g1/rules/r1', method: 'DELETE' });
  });

  it('reads members with null usernames', async () => {
    fetchMock.mockReturnValueOnce(answer(200, { members: [{ user: 'u1', username: 'ana' }, { user: 'u2' }] }));
    expect(await adminApi('/cube').groupMembers('g1')).toEqual([
      { user: 'u1', username: 'ana' },
      { user: 'u2', username: null },
    ]);
  });
});

describe('group helpers', () => {
  it('counts in the singular and plural', () => {
    expect(countLabel(1, 'member', 'members')).toBe('1 member');
    expect(countLabel(0, 'rule', 'rules')).toBe('0 rules');
  });

  it('offers the rules of the group\'s organization only', async () => {
    const searchRules = vi.fn(async () => ({
      items: [{ id: 'r1', workspaceName: 'W', description: 'Analysts' }],
      total: 1,
    }));
    const options = await ruleOptions({ searchRules } as unknown as AdminApi, 'o1')('an');
    expect(searchRules).toHaveBeenCalledWith({ q: 'an', organizationId: 'o1', page: 1 });
    expect(options).toEqual([{ value: 'r1', label: 'W / Analysts' }]);
  });
});

const group: Group = {
  id: '11111111-aaaa-4bbb-8ccc-000000000001',
  organizationId: 'o1',
  organizationName: 'Acme',
  name: 'Finance team',
  members: 1,
  rules: 0,
  createdAt: '',
  createdBy: null,
};

describe('the Groups tab', () => {
  let root: ReturnType<typeof createRoot>;

  beforeEach(() => {
    const host = document.createElement('div');
    document.body.appendChild(host);
    root = createRoot(host);
  });

  afterEach(() => {
    act(() => root.unmount());
    document.body.innerHTML = '';
  });

  const flush = () => act(async () => {
    await new Promise((r) => setTimeout(r, 0));
  });
  const click = (el: Element) => act(async () => {
    el.dispatchEvent(new MouseEvent('click', { bubbles: true }));
  });
  const button = (text: string) => [...document.querySelectorAll('button')].find((b) => b.textContent === text)!;

  function makeApi(rules: unknown[] = []) {
    return {
      searchGroups: vi.fn(async () => ({ items: [group], total: 1 })),
      groupMembers: vi.fn(async () => [{ user: 'u0', username: 'bia' }]),
      groupRules: vi.fn(async () => rules),
      addGroupMember: vi.fn(async () => ({})),
      addGroupRule: vi.fn(async () => undefined),
    };
  }

  async function render(api: ReturnType<typeof makeApi>) {
    await act(async () => root.render(createElement(GroupsTab, { api: api as unknown as AdminApi })));
    await flush();
  }

  it('lists groups with their counts and opens one with its members and rules', async () => {
    const api = makeApi();
    await render(api);
    const row = document.querySelector('tbody tr')!;
    expect(row.textContent).toContain('Finance team');
    expect(row.textContent).toContain('Acme');
    expect(row.textContent).toContain('1 member');
    expect(row.textContent).toContain('0 rules');

    await click(row);
    await flush();
    expect(api.groupMembers).toHaveBeenCalledWith(group.id);
    expect(document.body.textContent).toContain('bia');
    expect(document.body.textContent).toContain('In no rule yet.');
    // Not in a rule: it can be deleted.
    expect(button('Delete').disabled).toBe(false);
  });

  it('adds the picked user and puts the group in the picked rule', async () => {
    const api = makeApi();
    await render(api);
    await click(document.querySelector('tbody tr')!);
    await flush();

    await click(document.querySelector('[data-pick="User"]')!);
    await click(button('Add member'));
    await flush();
    expect(api.addGroupMember).toHaveBeenCalledWith(group.id, 'u1');

    await click(document.querySelector('[data-pick="Rule"]')!);
    await click(button('Add to rule'));
    await flush();
    expect(api.addGroupRule).toHaveBeenCalledWith(group.id, 'rule1');
  });

  it('does not offer to delete a group that is in rules', async () => {
    const api = makeApi([{
      id: 'r1', description: 'Analysts', workspaceName: 'W', roleName: 'viewer',
    }]);
    await render(api);
    await click(document.querySelector('tbody tr')!);
    await flush();
    expect(document.body.textContent).toContain('W / Analysts');
    expect(button('Delete').disabled).toBe(true);
  });
});
