// @vitest-environment jsdom
import { createElement, act } from 'react';
import { createRoot } from 'react-dom/client';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { Rbac } from '../hooks/rbac';

(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;

let rbac: Rbac;
vi.mock('../hooks', () => ({ useRbac: () => rbac }));

// eslint-disable-next-line import/first
import { pageFor, pageTrail, Sidebar } from '../components/Sidebar/Sidebar';

const ADMIN = { enabled: true, isAdmin: true, canModel: true, session: { token: 't', user: { id: 'a', platformAdmin: true } } } as any;
const USER = { enabled: true, isAdmin: false, session: { token: 't', user: { id: 'u' } } } as any;
const NO_RBAC = { enabled: false, isAdmin: true, canModel: true, session: null } as Rbac;

describe('Sidebar', () => {
  let root: ReturnType<typeof createRoot>;
  let host: HTMLElement;

  function render(pathname = '/build') {
    act(() => root.render(
      createElement(MemoryRouter, null, createElement(Sidebar, { pathname }))
    ));
  }

  const labels = () => [...host.querySelectorAll('nav a')].map((a) => a.getAttribute('title'));
  const toggle = () => host.querySelector('[data-testid="sidebar-toggle"]') as HTMLButtonElement;

  beforeEach(() => {
    window.localStorage.clear();
    // a laptop
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

  const group = () => host.querySelector('[data-testid="nav-group-admin"]') as HTMLButtonElement | null;

  it('shows Data Model, and nothing of the admin, to an organization owner', () => {
    rbac = { enabled: true, isAdmin: false, canModel: true, session: { token: 't', user: { id: 'u' } } } as any;
    render();
    expect(labels()).toContain('Data Model');
    expect(labels()).not.toContain('Data Source');
    expect(group()).toBeNull();
  });

  it('lists every page, the Admin group under its divider, for a platform admin', () => {
    rbac = ADMIN;
    render();

    expect(labels()).toEqual(['Playground', 'Data Model', 'Data Source', 'Frontend Integrations']);
    expect(host.textContent).toContain('Administração');
    expect(group()?.getAttribute('aria-expanded')).toBe('false');
  });

  it('opens the Admin group onto its six pages', () => {
    rbac = ADMIN;
    render();

    act(() => group()!.click());

    expect(group()?.getAttribute('aria-expanded')).toBe('true');
    expect(labels().slice(4)).toEqual(['Organizations', 'Workspaces', 'Roles', 'Rules', 'Groups', 'Users']);
    expect([...host.querySelectorAll('[role="group"] a')].map((a) => a.getAttribute('href')))
      .toEqual(['/admin/organizations', '/admin/workspaces', '/admin/roles', '/admin/rules', '/admin/groups', '/admin/users']);

    act(() => group()!.click());
    expect(labels()).toHaveLength(4);
  });

  it('opens the group of the current page and marks the page', () => {
    rbac = ADMIN;
    render('/admin/users');

    expect(group()?.getAttribute('aria-expanded')).toBe('true');
    expect(host.querySelector('[aria-current="page"]')?.getAttribute('title')).toBe('Users');
  });

  it('hides the admin pages from other users', () => {
    rbac = USER;
    render();

    expect(labels()).toEqual(['Playground', 'Frontend Integrations']);
    expect(host.textContent).not.toContain('Administração');
    expect(group()).toBeNull();
  });

  it('has no Admin page without RBAC', () => {
    rbac = NO_RBAC;
    render();

    expect(labels()).toEqual(['Playground', 'Data Model', 'Data Source', 'Frontend Integrations']);
    expect(group()).toBeNull();
  });

  it('marks the page of the current path', () => {
    rbac = ADMIN;
    render('/schema/orders');

    expect(host.querySelector('[aria-current="page"]')?.getAttribute('title')).toBe('Data Model');
  });

  it('collapses to icons and remembers it', () => {
    rbac = ADMIN;
    render();
    expect(host.querySelector('nav a .nav-label')?.textContent).toBe('Playground');

    act(() => toggle().click());

    expect(host.querySelector('nav a .nav-label')).toBeNull();
    expect(toggle().getAttribute('aria-expanded')).toBe('false');
    expect(window.localStorage.getItem('playground:sidebar-collapsed')).toBe('true');

    act(() => root.unmount());
    root = createRoot(host);
    render();
    expect(host.querySelector('nav a .nav-label')).toBeNull();
  });
});

describe('pageFor', () => {
  it('matches a page and its subpaths, nothing else', () => {
    expect(pageFor('/build')?.label).toBe('Playground');
    expect(pageFor('/schema/x')?.label).toBe('Data Model');
    expect(pageFor('/buildings')).toBeUndefined();
    expect(pageFor('/')).toBeUndefined();
    expect(pageFor('/admin/workspaces')?.label).toBe('Workspaces');
  });

  it('gives the group and the page inside it', () => {
    expect(pageTrail('/admin/users').map((p) => p.label)).toEqual(['Admin', 'Users']);
    expect(pageTrail('/admin').map((p) => p.label)).toEqual(['Admin']);
    expect(pageTrail('/build').map((p) => p.label)).toEqual(['Playground']);
  });
});
