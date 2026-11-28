// @vitest-environment jsdom
import {
  afterEach, beforeEach, describe, expect, it, vi,
} from 'vitest';

import { renderDiagram } from '../pages/Schema/models/export/renderDiagram';

const fake = vi.hoisted(() => ({ toSvg: null as any, toPng: null as any }));
vi.mock('html-to-image', () => ({
  toSvg: (...args: unknown[]) => fake.toSvg(...args),
  toPng: (...args: unknown[]) => fake.toPng(...args),
}));

// The app's faces: each font in woff2 and woff, as the Playground serves them.
const FACES = `
  @font-face { font-family: 'Inter'; font-style: normal; font-weight: 400;
    src: url(/fonts/Inter-Regular.woff2) format('woff2'), url(/fonts/Inter-Regular.woff) format('woff'); }
  @font-face { font-family: 'Inter'; font-style: normal; font-weight: 700;
    src: url(/fonts/Inter-Bold.woff2) format('woff2'), url(/fonts/Inter-Bold.woff) format('woff'); }
  @font-face { font-family: 'Inter'; font-style: normal; font-weight: 600;
    src: url(/fonts/Inter-SemiBold.woff2) format('woff2'), url(/fonts/Inter-SemiBold.woff) format('woff'); }
  @font-face { font-family: 'JetBrains Mono'; font-style: normal; font-weight: 400;
    src: url(/fonts/JetBrainsMono-Regular.woff2) format('woff2'), url(/fonts/JetBrainsMono-Regular.woff) format('woff'); }
`;

describe('renderDiagram fonts', () => {
  let style: HTMLStyleElement;
  let fetched: string[];

  beforeEach(() => {
    fake.toSvg = vi.fn(async () => 'data:image/svg+xml,x');
    fake.toPng = vi.fn(async () => 'data:image/png;base64,x');
    style = document.createElement('style');
    style.textContent = FACES;
    document.head.appendChild(style);
    fetched = [];
    vi.stubGlobal('fetch', vi.fn(async (url: string) => {
      fetched.push(new URL(url, document.baseURI).pathname);
      return new Response(new Blob([new Uint8Array([1, 2, 3])], { type: 'font/woff2' }));
    }));
  });

  afterEach(() => {
    style.remove();
    document.body.innerHTML = '';
    vi.unstubAllGlobals();
  });

  // A viewport whose text is Inter, regular and bold.
  const viewport = () => {
    const el = document.createElement('div');
    el.className = 'react-flow__viewport';
    el.innerHTML = `
      <div style="font-family: Inter, system-ui, sans-serif; font-weight: 400">orders</div>
      <span style="font-family: 'Inter', sans-serif; font-weight: 700">id</span>
      <div style="font-family: 'JetBrains Mono', monospace"></div>
    `;
    document.body.appendChild(el);
    return el;
  };
  const bounds = {
    x: 0, y: 0, width: 100, height: 50,
  };

  it('embeds only the faces the diagram uses, in woff2', async () => {
    await renderDiagram(viewport(), bounds, 'svg');
    const options = fake.toSvg.mock.calls[0][1];
    expect(options.fontEmbedCSS).toEqual(expect.any(String));
    expect(fetched.sort()).toEqual(['/fonts/Inter-Bold.woff2', '/fonts/Inter-Regular.woff2']);
    const css: string = options.fontEmbedCSS;
    expect(css.match(/@font-face/g)).toHaveLength(2);
    expect(css).toContain('font-weight: 700');
    expect(css).toContain('font-weight: 400');
    expect(css).toContain("format('woff2')");
    expect(css).toContain('url(data:font/woff2;base64,');
    expect(css).not.toContain('/fonts/');
    expect(css).not.toContain('JetBrains');
  });

  it('a png gets the same, and a face that fails to load is left out', async () => {
    vi.stubGlobal('fetch', vi.fn(async (url: string) => {
      if (url.includes('Bold')) {
        throw new Error('offline');
      }
      return new Response(new Blob([new Uint8Array([1])], { type: 'font/woff2' }));
    }));
    await renderDiagram(viewport(), bounds, 'png', 2);
    const css: string = fake.toPng.mock.calls[0][1].fontEmbedCSS;
    expect(css.match(/@font-face/g)).toHaveLength(1);
    expect(css).toContain('font-weight: 400');
  });

  it('embeds nothing when the diagram uses no web font', async () => {
    const el = document.createElement('div');
    el.className = 'react-flow__viewport';
    el.innerHTML = '<div style="font-family: Arial">x</div>';
    document.body.appendChild(el);
    await renderDiagram(el, bounds, 'svg');
    expect(fake.toSvg.mock.calls[0][1].fontEmbedCSS).toBe('');
    expect(fetch).not.toHaveBeenCalled();
  });
});

describe('renderDiagram svg', () => {
  beforeEach(() => {
    const markup = '<svg xmlns="http://www.w3.org/2000/svg"><foreignObject>'
      + '<div xmlns="http://www.w3.org/1999/xhtml" style="--tw-x: 1; color: red">a</div></foreignObject></svg>';
    fake.toSvg = vi.fn(async () => `data:image/svg+xml;charset=utf-8,${encodeURIComponent(markup)}`);
    fake.toPng = vi.fn(async () => 'data:image/png;base64,x');
  });

  it('compacts the SVG html-to-image made, not the PNG', async () => {
    const el = document.createElement('div');
    el.className = 'react-flow__viewport';
    document.body.appendChild(el);
    const bounds = {
      x: 0, y: 0, width: 10, height: 10,
    };
    const out = decodeURIComponent(await renderDiagram(el, bounds, 'svg'));
    expect(out).toContain('color: red');
    expect(out).not.toContain('--tw-x');
    expect(await renderDiagram(el, bounds, 'png')).toBe('data:image/png;base64,x');
    el.remove();
  });
});
