// @vitest-environment jsdom
import { act, createElement, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import {
  CodeEditor, CodeView, highlighted, lineCount,
} from '../pages/Schema/NumberedCode';

vi.mock('prismjs', () => ({
  default: {
    highlightAll: () => undefined,
    languages: { javascript: {} },
    // Marks every "name" so the test sees Prism's output, escaped like Prism.
    highlight: (text: string) => text.replace(/&/g, '&amp;').replace(/</g, '&lt;')
      .replace(/name/g, '<span class="token property">name</span>'),
  },
}));

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

  it('colors the text being edited like the view, escaped', async () => {
    expect(highlighted('a <b> & name')).toBe('a &lt;b> &amp; <span class="token property">name</span>\n');
    await act(async () => root.render(createElement(CodeEditor, { value: '- name: x', onChange: () => undefined })));
    const colors = host.querySelector('[data-slot="code-colors"]')!;
    expect(colors.getAttribute('aria-hidden')).toBe('true');
    expect(colors.querySelector('code')!.className).toBe('language-javascript');
    expect(colors.querySelector('.token.property')!.textContent).toBe('name');
    expect(colors.textContent).toBe('- name: x\n');
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
    expect((host.querySelector('[data-slot="code-colors"]') as HTMLElement).scrollTop).toBe(40);
  });
});
