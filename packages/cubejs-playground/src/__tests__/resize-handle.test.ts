// @vitest-environment jsdom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { ResizeHandle } from '../pages/Schema/ResizeHandle';
import { clampWidth, useResizable } from '../pages/Schema/useResizable';

(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;

function Panel() {
  const [width, resize] = useResizable('test.width', 300, 200, 500);
  return createElement('aside', { 'data-width': String(width) },
    createElement(ResizeHandle, {
      width, min: 200, max: 500, label: 'tables', onResize: resize,
    }));
}

describe('a resizable side panel', () => {
  let host: HTMLDivElement;
  let root: ReturnType<typeof createRoot>;

  beforeEach(() => {
    localStorage.clear();
    Element.prototype.setPointerCapture = () => undefined;
    host = document.createElement('div');
    document.body.appendChild(host);
    root = createRoot(host);
  });

  afterEach(() => {
    act(() => root.unmount());
    document.body.innerHTML = '';
  });

  const handle = () => host.querySelector('[data-slot="resize-handle"]') as HTMLDivElement;
  const width = () => host.querySelector('aside')!.getAttribute('data-width');
  const pointer = (type: string, clientX: number) => {
    const e = new MouseEvent(type, { bubbles: true, clientX });
    Object.defineProperty(e, 'pointerId', { value: 1 });
    handle().dispatchEvent(e);
  };

  it('keeps the width between its limits', () => {
    expect(clampWidth(100, 200, 500)).toBe(200);
    expect(clampWidth(900, 200, 500)).toBe(500);
    expect(clampWidth(321.6, 200, 500)).toBe(322);
  });

  it('follows a drag of the edge and remembers the width', async () => {
    await act(async () => root.render(createElement(Panel)));
    expect(width()).toBe('300');
    await act(async () => {
      pointer('pointerdown', 100);
      pointer('pointermove', 180);
      pointer('pointerup', 180);
    });
    expect(width()).toBe('380');
    // After the pointer is up, moving it changes nothing.
    await act(async () => pointer('pointermove', 400));
    expect(width()).toBe('380');

    act(() => root.unmount());
    root = createRoot(host);
    await act(async () => root.render(createElement(Panel)));
    expect(width()).toBe('380');
  });

  it('resizes with the arrow keys, Home and End', async () => {
    await act(async () => root.render(createElement(Panel)));
    const key = (k: string) => handle().dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true }));
    await act(async () => key('ArrowRight'));
    expect(width()).toBe('316');
    await act(async () => key('ArrowLeft'));
    expect(width()).toBe('300');
    await act(async () => key('End'));
    expect(width()).toBe('500');
    await act(async () => key('Home'));
    expect(width()).toBe('200');
    expect(handle().getAttribute('aria-valuenow')).toBe('200');
  });
});
