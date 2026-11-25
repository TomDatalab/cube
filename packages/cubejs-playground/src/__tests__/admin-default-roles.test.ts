// @vitest-environment jsdom
import { createElement, act } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { AdminApi, OrgRole, Rule } from '../pages/Admin/api';
import { RolesTab } from '../pages/Admin/RolesTab';
import { RulesTab } from '../pages/Admin/RulesTab';

vi.mock('../pages/Admin/RemoteSelect', () => ({
  organizationOptions: () => async () => [],
  workspaceOptions: () => async () => [],
  // A button that picks organization / workspace "x1".
  RemoteSelect: ({ onChange, placeholder }: any) => (
    createElement('button', { 'data-pick': placeholder, onClick: () => onChange('x1') }, placeholder)
  ),
}));

(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;

const VIEWER_ID = '33333333-aaaa-4bbb-8ccc-000000000003';
const role = (over: Partial<OrgRole>): OrgRole => ({
  id: 'r',
  organizationId: 'o1',
  name: 'n',
  permissions: ['view'],
  restrictions: { rowFilters: [], blockedMembers: [] },
  rules: 0,
  createdAt: '',
  createdBy: null,
  builtin: null,
  ...over,
} as OrgRole);

const roles: OrgRole[] = [
  role({ id: '11111111-aaaa-4bbb-8ccc-000000000001', name: 'owner', builtin: 'owner', rules: 2 }),
  role({ id: '22222222-aaaa-4bbb-8ccc-000000000002', name: 'Editors', builtin: 'editor' }),
  role({ id: VIEWER_ID, name: 'viewer', builtin: 'viewer' }),
  role({ id: '44444444-aaaa-4bbb-8ccc-000000000004', name: 'finance' }),
];

const rule = {
  id: 'rule1',
  description: 'Finance readers',
  type: 'finance',
  role: 'viewer',
  roleId: VIEWER_ID,
  roleName: 'viewer',
  permissions: ['view'],
  workspaceId: 'w1',
  workspaceName: 'W',
  organizationId: 'o1',
  organizationName: 'Acme',
  createdAt: '',
  createdBy: null,
} as Rule;

const flush = () => act(async () => {
  await new Promise((r) => setTimeout(r, 0));
});

describe('default roles in the Playground', () => {
  let root: ReturnType<typeof createRoot>;
  let host: HTMLElement;

  beforeEach(() => {
    window.matchMedia = vi.fn().mockImplementation((query: string) => ({
      matches: false,
      media: query,
      addListener: vi.fn(),
      removeListener: vi.fn(),
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    })) as any;
    host = document.createElement('div');
    document.body.appendChild(host);
    root = createRoot(host);
  });

  afterEach(() => {
    act(() => root.unmount());
    document.body.innerHTML = '';
  });

  const click = (el: Element) => act(async () => {
    el.dispatchEvent(new MouseEvent('click', { bubbles: true }));
  });
  const pick = (placeholder: string) => click(document.querySelector(`[data-pick="${placeholder}"]`)!);

  describe('RolesTab', () => {
    const api = { roles: vi.fn(async () => roles) } as unknown as AdminApi;

    async function render() {
      await act(async () => root.render(createElement(RolesTab, { api })));
      await pick('Pick an organization');
      await flush();
    }

    it('tags default roles "default" and shows their UUID', async () => {
      await render();
      const rows = [...document.querySelectorAll('tbody tr')];
      expect(rows).toHaveLength(4);
      const tagged = rows.filter((r) => [...r.querySelectorAll('.ant-tag')].some((t) => t.textContent === 'default'));
      expect(tagged.map((r) => r.querySelector('td')!.textContent)).toEqual(['ownerdefault', 'Editorsdefault', 'viewerdefault']);
      expect(rows[1].textContent).toContain('22222222…');
    });

    it('has no built-in roles table and lets default roles be edited and deleted', async () => {
      await render();
      expect(document.body.textContent).not.toContain('Built-in roles');
      expect(document.querySelectorAll('table')).toHaveLength(1);
      expect(document.querySelector('[aria-label="Edit Editors"]')).toBeTruthy();
      expect((document.querySelector('[aria-label="Delete Editors"]') as HTMLButtonElement).disabled).toBe(false);
      // In use: delete stays disabled.
      expect((document.querySelector('[aria-label="Delete owner"]') as HTMLButtonElement).disabled).toBe(true);
    });
  });

  describe('RulesTab', () => {
    function makeApi() {
      return {
        searchRules: vi.fn(async () => ({ items: [rule], total: 1 })),
        searchWorkspaces: vi.fn(async () => ({ items: [], total: 0 })),
        roles: vi.fn(async () => roles),
        workspace: vi.fn(async () => ({ organizationId: 'o1' })),
        createRule: vi.fn(async () => rule),
        updateRule: vi.fn(async () => rule),
      };
    }

    async function render(api: ReturnType<typeof makeApi>) {
      await act(async () => root.render(createElement(RulesTab, { api: api as unknown as AdminApi })));
      await flush();
    }

    const selected = () => document.querySelector('.ant-modal .ant-select-selection-item')?.textContent;

    it('offers only the organization roles and sends roleId', async () => {
      const api = makeApi();
      await render(api);
      await click(document.querySelector('[aria-label="Edit Finance readers"]')!);
      await flush();
      expect(selected()).toContain('viewer');
      // Open the role dropdown.
      await act(async () => {
        document.querySelector('.ant-modal .ant-select-selector')!
          .dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
      });
      const text = document.body.textContent!;
      expect(text).not.toContain('Built-in');
      expect(text).not.toContain('views and queries');
      expect(text).toContain('finance');
      await act(async () => {
        document.querySelector('.ant-modal .ant-btn-primary')!.dispatchEvent(new MouseEvent('click', { bubbles: true }));
      });
      await flush();
      expect(api.updateRule).toHaveBeenCalledTimes(1);
      const patch = (api.updateRule.mock.calls[0] as any[])[1];
      expect(patch).toMatchObject({ roleId: VIEWER_ID });
      expect(patch).not.toHaveProperty('role');
    });

    it('preselects the default viewer role of the new rule\'s organization', async () => {
      const api = makeApi();
      await render(api);
      await click([...document.querySelectorAll('button')].find((b) => b.textContent === 'New rule')!);
      expect(selected()).toBeUndefined();
      await pick('Organization / workspace');
      await flush();
      expect(api.roles).toHaveBeenCalledWith('o1');
      expect(selected()).toContain('viewer');
    });

    it('clears an edited rule\'s role the organization lacks, instead of picking viewer', async () => {
      const api = makeApi();
      // The rule's role is gone from the organization, which has a default viewer.
      api.searchRules.mockResolvedValue({ items: [{ ...rule, roleId: 'gone' }], total: 1 });
      await render(api);
      await click(document.querySelector('[aria-label="Edit Finance readers"]')!);
      await flush();
      expect(selected()).toBeUndefined();
      await act(async () => {
        document.querySelector('.ant-modal .ant-btn-primary')!.dispatchEvent(new MouseEvent('click', { bubbles: true }));
      });
      await flush();
      await flush();
      await act(async () => {
        await new Promise((r) => setTimeout(r, 100));
      });
      expect(document.body.textContent).toContain('Pick a role');
      expect(api.updateRule).not.toHaveBeenCalled();
    });

    it('stops loading and shows an empty list when the roles fail to load', async () => {
      const api = makeApi();
      api.roles.mockRejectedValue(new Error('boom'));
      await render(api);
      await click(document.querySelector('[aria-label="Edit Finance readers"]')!);
      await flush();
      expect(document.querySelector('.ant-modal .ant-select-arrow-loading')).toBeNull();
      expect(document.querySelector('.ant-modal .ant-select-loading')).toBeNull();
      // The list is empty: no organization role is offered.
      await act(async () => {
        document.querySelector('.ant-modal .ant-select-selector')!
          .dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
      });
      expect(document.body.textContent).not.toContain('Editors');
    });

    it('shows the rule\'s role name, not its id, while the roles load', async () => {
      const api = makeApi();
      api.roles.mockReturnValue(new Promise(() => {}));
      await render(api);
      await click(document.querySelector('[aria-label="Edit Finance readers"]')!);
      expect(selected()).toBe('viewer');
      expect(document.body.textContent).not.toContain(VIEWER_ID);
    });

    it('preselects nothing for a new rule in an organization without a default viewer', async () => {
      const api = makeApi();
      api.roles.mockResolvedValue(roles.filter((r) => r.builtin !== 'viewer'));
      await render(api);
      await click([...document.querySelectorAll('button')].find((b) => b.textContent === 'New rule')!);
      await pick('Organization / workspace');
      await flush();
      expect(selected()).toBeUndefined();
      await act(async () => {
        document.querySelector('.ant-modal .ant-btn-primary')!.dispatchEvent(new MouseEvent('click', { bubbles: true }));
      });
      await flush();
      await flush();
      await act(async () => {
        await new Promise((r) => setTimeout(r, 100));
      });
      expect(document.body.textContent).toContain('Pick a role');
      expect(api.createRule).not.toHaveBeenCalled();
    });
  });
});
