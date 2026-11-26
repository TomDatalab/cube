// @vitest-environment jsdom
import { act, createElement, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { CodeEditor, CodeView, lineCount } from '../pages/Schema/NumberedCode';

vi.mock('prismjs', () => ({ default: { highlightAll: () => undefined } }));

(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;

describe('files with line numbers', () => {
  let host: HTMLDivElement;
  let root: ReturnType<typeof createRoot>;

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
    root = createRoot(host);
  });

  afterEach(() => {
    act(() => root.unmount());
    document.body.innerHTML = '';
  });

  const numbers = () => host.querySelector('[data-slot="line-numbers"]')!.textContent!.split('\n');

  it('counts lines, an empty text being one', () => {
    expect(lineCount('')).toBe(1);
    expect(lineCount('a\nb')).toBe(2);
    expect(lineCount('a\nb\n')).toBe(3);
  });

  it('numbers every line of a file shown', async () => {
    await act(async () => root.render(createElement(CodeView, { code: 'cubes:\n  - name: a\n    sql: x' })));
    expect(numbers()).toEqual(['1', '2', '3']);
    expect(host.querySelector('[data-testid="prism-code"]')!.textContent).toBe('cubes:\n  - name: a\n    sql: x');
  });

  it('renumbers while editing and keeps the gutter with the text when it scrolls', async () => {
    const seen: string[] = [];
    function Edit() {
      const [text, setText] = useState('a\nb');
      return createElement(CodeEditor, {
        value: text,
        onChange: (t: string) => {
          seen.push(t);
          setText(t);
        },
      });
    }
    await act(async () => root.render(createElement(Edit)));
    expect(numbers()).toEqual(['1', '2']);

    const area = host.querySelector('textarea')!;
    expect(area.getAttribute('wrap')).toBe('off');
    const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')!.set!;
    await act(async () => {
      setter.call(area, 'a\nb\nc\nd');
      area.dispatchEvent(new Event('input', { bubbles: true }));
    });
    expect(seen).toEqual(['a\nb\nc\nd']);
    expect(numbers()).toEqual(['1', '2', '3', '4']);

    area.scrollTop = 40;
    await act(async () => area.dispatchEvent(new Event('scroll')));
    expect((host.querySelector('.numbered-code-gutter-scroll') as HTMLElement).scrollTop).toBe(40);
  });
});
