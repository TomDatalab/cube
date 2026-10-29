// @vitest-environment jsdom
import { readFileSync } from 'fs';
import path from 'path';

import { THEME_STORAGE_KEY } from '../shared/theme';

const html = readFileSync(path.resolve(__dirname, '../../index.html'), 'utf8');
const head = html.slice(html.indexOf('<head>'), html.indexOf('</head>'));

function inlineScripts(): string[] {
  return [...head.matchAll(/<script>([\s\S]*?)<\/script>/g)].map((m) => m[1]);
}

function runBootstrap({ stored, throws = false, systemDark = false }: {
  stored?: string;
  throws?: boolean;
  systemDark?: boolean;
}) {
  document.documentElement.innerHTML = '<head><link id="antd-light" rel="stylesheet" href="/a.css"><link id="antd-dark" rel="stylesheet" href="/b.css" disabled></head><body></body>';
  delete document.documentElement.dataset.theme;

  vi.stubGlobal('localStorage', {
    getItem: () => {
      if (throws) {
        throw new Error('blocked');
      }
      return stored ?? null;
    },
  });
  window.matchMedia = ((query: string) => ({
    matches: systemDark && query.includes('dark'),
  })) as any;

  // scripts that come before the links run first, the ones after run once the links exist
  inlineScripts().forEach((code) => {
    // jsdom does not run injected scripts, so evaluate the inline source directly
    // oxlint-disable-next-line no-eval
    window.eval(code);
  });

  return {
    theme: document.documentElement.dataset.theme,
    light: document.getElementById('antd-light') as HTMLLinkElement,
    dark: document.getElementById('antd-dark') as HTMLLinkElement,
  };
}

describe('index.html', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('runs the bootstrap script before any stylesheet', () => {
    expect(head.indexOf('<script')).toBeGreaterThan(-1);
    expect(head.indexOf('<script')).toBeLessThan(head.indexOf('rel="stylesheet"'));
  });

  it('uses the same storage key as the theme module', () => {
    expect(inlineScripts()[0]).toContain(`'${THEME_STORAGE_KEY}'`);
  });

  it('declares both antd sheets, the dark one starting disabled', () => {
    expect(head).toMatch(/<link id="antd-light"/);
    expect(head).toMatch(/<link id="antd-dark"[^>]*\sdisabled/);
  });

  it('applies the stored dark preference', () => {
    const { theme, light, dark } = runBootstrap({ stored: 'dark' });

    expect(theme).toBe('dark');
    expect(dark.disabled).toBe(false);
    expect(light.disabled).toBe(true);
  });

  it('falls back to the system theme when storage throws', () => {
    const { theme, light, dark } = runBootstrap({ throws: true, systemDark: true });

    expect(theme).toBe('dark');
    expect(dark.disabled).toBe(false);
    expect(light.disabled).toBe(true);
  });

  it('keeps the light sheet for a light preference over a dark system', () => {
    const { theme, light, dark } = runBootstrap({ stored: 'light', systemDark: true });

    expect(theme).toBe('light');
    expect(light.disabled).toBe(false);
    expect(dark.disabled).toBe(true);
  });
});
