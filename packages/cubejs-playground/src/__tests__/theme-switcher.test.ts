// @vitest-environment jsdom
import { createElement, act } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { ThemeSwitcher } from '../components/ThemeSwitcher';

(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;

describe('ThemeSwitcher', () => {
  let root: ReturnType<typeof createRoot>;
  let host: HTMLElement;

  beforeEach(() => {
    window.localStorage.clear();
    delete document.documentElement.dataset.theme;
    // a light system
    window.matchMedia = vi.fn().mockImplementation((query: string) => ({
      matches: false,
      media: query,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    })) as any;
    host = document.createElement('div');
    document.body.appendChild(host);
    root = createRoot(host);
    act(() => root.render(createElement(ThemeSwitcher)));
  });

  afterEach(() => {
    act(() => root.unmount());
    document.body.innerHTML = '';
  });

  async function choose(label: string) {
    const button = host.querySelector('[data-testid="theme-switcher"]') as HTMLElement;
    expect(button.getAttribute('aria-label')).toBe('Theme');
    await act(async () => {
      button.click();
    });
    const item = Array.from(document.body.querySelectorAll('.ant-dropdown-menu-item')).find(
      (el) => el.textContent?.includes(label)
    ) as HTMLElement;
    expect(item).toBeTruthy();
    await act(async () => {
      item.click();
    });
  }

  it('applies and persists Dark', async () => {
    await choose('Dark');
    expect(window.localStorage.getItem('cube-playground-theme')).toBe('dark');
    expect(document.documentElement.dataset.theme).toBe('dark');
  });

  it('System follows a light system', async () => {
    await choose('Dark');
    await choose('System');
    expect(window.localStorage.getItem('cube-playground-theme')).toBe('system');
    expect(document.documentElement.dataset.theme).toBe('light');
  });
});
