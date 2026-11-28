// @vitest-environment jsdom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { PanelToggle } from '../pages/Schema/PanelToggle';
import { useCollapsed } from '../pages/Schema/useCollapsed';

(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;

function Panel() {
  const [collapsed, toggle] = useCollapsed('test.panel');
  return createElement('aside', { 'data-collapsed': String(collapsed) },
    createElement(PanelToggle, { collapsed, onToggle: toggle, label: 'tables' }));
}

describe('a collapsible side panel', () => {
  let host: HTMLDivElement;
  let root: ReturnType<typeof createRoot>;

  beforeEach(() => {
    localStorage.clear();
    host = document.createElement('div');
    document.body.appendChild(host);
    root = createRoot(host);
  });

  afterEach(() => {
    act(() => root.unmount());
    document.body.innerHTML = '';
  });

  const button = () => host.querySelector('[data-slot="panel-toggle"]') as HTMLButtonElement;
  const state = () => host.querySelector('aside')!.getAttribute('data-collapsed');

  it('collapses and opens again, and remembers it', async () => {
    await act(async () => root.render(createElement(Panel)));
    expect(state()).toBe('false');
    expect(button().getAttribute('aria-label')).toBe('Hide tables');

    await act(async () => button().click());
    expect(state()).toBe('true');
    expect(button().getAttribute('aria-label')).toBe('Show tables');
    expect(button().getAttribute('aria-expanded')).toBe('false');

    act(() => root.unmount());
    root = createRoot(host);
    await act(async () => root.render(createElement(Panel)));
    expect(state()).toBe('true');

    await act(async () => button().click());
    expect(state()).toBe('false');
  });
});
