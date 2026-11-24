// @vitest-environment jsdom
import { createElement, act } from 'react';
import { createRoot } from 'react-dom/client';
import { MemoryRouter, Route } from 'react-router-dom';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { buildRbac } from '../hooks/rbac';

(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;

let rbac: ReturnType<typeof buildRbac>;
vi.mock('../hooks', () => ({ useRbac: () => rbac }));

// eslint-disable-next-line import/first
import { OwnerOrAdmin } from '../components/AdminOnly/OwnerOrAdmin';

const session = (platformAdmin: boolean, owned: { id: string; slug: string; name: string }[]) => ({
  token: 't',
  expiresAt: '',
  user: { id: 'u', username: 'u', name: null, platformAdmin, ownedOrganizations: owned },
});
const ORG = [{ id: 'o1', slug: 'a', name: 'A' }];
const ON = { authRequired: true };

describe('OwnerOrAdmin', () => {
  let root: ReturnType<typeof createRoot>;
  let host: HTMLElement;

  function render() {
    act(() => root.render(createElement(MemoryRouter, { initialEntries: ['/schema'] }, [
      createElement(Route, { key: 's', path: '/schema', render: () => createElement(OwnerOrAdmin, null, 'model page') }),
      createElement(Route, { key: 'b', path: '/build', render: () => 'builder' }),
    ])));
    return host.textContent;
  }

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
    root = createRoot(host);
  });

  afterEach(() => {
    act(() => root.unmount());
    document.body.innerHTML = '';
  });

  it('renders for an owner', () => {
    rbac = buildRbac(ON, session(false, ORG));
    expect(render()).toBe('model page');
  });

  it('redirects a member with no organization', () => {
    rbac = buildRbac(ON, session(false, []));
    expect(render()).toBe('builder');
  });

  it('renders for an admin', () => {
    rbac = buildRbac(ON, session(true, []));
    expect(render()).toBe('model page');
  });

  it('renders with RBAC off', () => {
    rbac = buildRbac({}, null);
    expect(render()).toBe('model page');
  });
});
