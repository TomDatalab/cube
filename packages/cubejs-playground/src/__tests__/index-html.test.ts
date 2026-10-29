// @vitest-environment jsdom
import { readFileSync } from 'fs';
import path from 'path';

import { THEME_STORAGE_KEY } from '../shared/theme';

const html = readFileSync(path.resolve(__dirname, '../../index.html'), 'utf8');
const head = html.slice(html.indexOf('<head>'), html.indexOf('</head>'));

function inlineScripts(): string[] {
  return [...head.matchAll(/<script>([\s\S]*?)<\/script>/g)].map((m) => m[1]);
}

function runBootstrap({ stored, throws = false, systemDark = false, matchMediaThrows = false }: {
  stored?: string;
  throws?: boolean;
  systemDark?: boolean;
  matchMediaThrows?: boolean;
}) {
  delete document.documentElement.dataset.theme;

  vi.stubGlobal('localStorage', {
    getItem: () => {
      if (throws) {
        throw new Error('blocked');
      }
      return stored ?? null;
    },
  });
  window.matchMedia = ((query: string) => {
    if (matchMediaThrows) {
      throw new Error('boom');
    }
    return { matches: systemDark && query.includes('dark') };
  }) as any;

  let written = '';

  vi.spyOn(document, 'write').mockImplementation((...chunks: string[]) => {
    written += chunks.join('');
  });

  // jsdom does not run injected scripts, so evaluate the inline source directly
  // oxlint-disable-next-line no-eval
  window.eval(inlineScripts()[0]);

  const light = written.match(/<link id="antd-light"[^>]*>/)?.[0] ?? '';
  const dark = written.match(/<link id="antd-dark"[^>]*>/)?.[0] ?? '';

  return {
    theme: document.documentElement.dataset.theme,
    written,
    light,
    dark,
    // the render-blocking link is the first one written
    first: written.match(/<link id="(antd-[a-z]+)"/)?.[1],
  };
}

describe('index.html', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('has one inline script before any stylesheet markup', () => {
    expect(inlineScripts()).toHaveLength(1);
    expect(head.indexOf('<script')).toBeGreaterThan(-1);
    expect(head.indexOf('<script')).toBeLessThan(head.search(/rel="stylesheet"/));
    expect(head.indexOf('<script')).toBeLessThan(head.indexOf('<style'));
  });

  it('has no static antd links, the script writes them', () => {
    const outside = head.replace(/<script>[\s\S]*?<\/script>/g, '');

    expect(outside).not.toMatch(/antd(\.dark)?\.min\.css/);
    expect(outside).not.toMatch(/rel="stylesheet"/);
  });

  it('uses the same storage key as the theme module', () => {
    expect(inlineScripts()[0]).toContain(`'${THEME_STORAGE_KEY}'`);
  });

  it.each([
    ['stored dark', { stored: 'dark' }],
    ['storage throws with a dark system', { throws: true, systemDark: true }],
  ])('writes the dark sheet enabled when %s', (_name, opts) => {
    const r = runBootstrap(opts);

    expect(r.theme).toBe('dark');
    expect(r.first).toBe('antd-dark');
    expect(r.dark).toContain('href="/antd.dark.min.css"');
    expect(r.dark).not.toContain('disabled');
    expect(r.light).toContain('href="/antd.min.css"');
    expect(r.light).toContain('disabled');
    expect(r.written).toContain('html[data-theme="dark"]{background:#1c1c24;color-scheme:dark}');
  });

  it('keeps the light sheet for a light preference over a dark system', () => {
    const r = runBootstrap({ stored: 'light', systemDark: true });

    expect(r.theme).toBe('light');
    expect(r.first).toBe('antd-light');
    expect(r.light).not.toContain('disabled');
    expect(r.dark).toContain('disabled');
  });

  it('falls back to an enabled light sheet when resolution throws', () => {
    const r = runBootstrap({ throws: true, matchMediaThrows: true });

    expect(r.theme).toBe('light');
    expect(r.light).not.toContain('disabled');
    expect(r.dark).toContain('disabled');
  });
});
