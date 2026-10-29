// @vitest-environment jsdom
import { createElement, act } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { applyTheme, readPreference, resolveTheme, writePreference } from '../shared/theme';
import { useTheme } from '../shared/useTheme';

(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;

describe('theme', () => {
  it('resolves the effective theme', () => {
    expect(resolveTheme('system', true)).toBe('dark');
    expect(resolveTheme('system', false)).toBe('light');
    expect(resolveTheme('light', true)).toBe('light');
    expect(resolveTheme('dark', false)).toBe('dark');
  });

  it('reads the preference defensively', () => {
    expect(readPreference({ getItem: () => null })).toBe('system');
    expect(readPreference({ getItem: () => 'blue' })).toBe('system');
    expect(
      readPreference({
        getItem: () => {
          throw new Error('blocked');
        },
      })
    ).toBe('system');
    expect(readPreference({ getItem: () => 'dark' })).toBe('dark');
  });

  it('swallows storage errors on write', () => {
    const blocked = {
      setItem: () => {
        throw new Error('blocked');
      },
    };
    expect(() => writePreference('dark', blocked)).not.toThrow();
  });

  it('applies the theme to the document and the antd links', () => {
    document.head.innerHTML = '<link id="antd-light"><link id="antd-dark">';
    applyTheme('dark');
    expect(document.documentElement.dataset.theme).toBe('dark');
    expect((document.getElementById('antd-dark') as HTMLLinkElement).disabled).toBe(false);
    expect((document.getElementById('antd-light') as HTMLLinkElement).disabled).toBe(true);
  });

  it('tolerates missing links', () => {
    document.head.innerHTML = '';
    expect(() => applyTheme('light')).not.toThrow();
    expect(document.documentElement.dataset.theme).toBe('light');
  });
});

describe('useTheme', () => {
  let hook: ReturnType<typeof useTheme>;
  let root: ReturnType<typeof createRoot>;
  let systemDark: boolean;
  let listeners: Array<(e: { matches: boolean }) => void>;

  function setSystem(dark: boolean) {
    systemDark = dark;
    act(() => listeners.forEach((l) => l({ matches: dark })));
  }

  beforeEach(() => {
    window.localStorage.clear();
    systemDark = false;
    listeners = [];
    (window as any).matchMedia = () => ({
      get matches() {
        return systemDark;
      },
      addEventListener: (_: string, l: any) => listeners.push(l),
      removeEventListener: (_: string, l: any) => {
        listeners = listeners.filter((x) => x !== l);
      },
    });
    function Probe() {
      hook = useTheme();
      return null;
    }
    root = createRoot(document.createElement('div'));
    act(() => root.render(createElement(Probe)));
  });

  afterEach(() => {
    act(() => root.unmount());
    delete (window as any).matchMedia;
  });

  it('follows the system while the preference is system', () => {
    expect(hook.theme).toBe('light');
    setSystem(true);
    expect(hook.theme).toBe('dark');
    expect(document.documentElement.dataset.theme).toBe('dark');
  });

  it('ignores the system once the preference is explicit', () => {
    act(() => hook.setPreference('light'));
    setSystem(true);
    expect(hook.theme).toBe('light');
    expect(document.documentElement.dataset.theme).toBe('light');
  });
});
