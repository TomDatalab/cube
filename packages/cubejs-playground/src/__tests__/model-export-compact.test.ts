// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';

import { compactSvg, compactSvgMarkup, declarations } from '../pages/Schema/models/export/compactSvg';

const INITIAL: Record<string, string> = {
  display: 'inline', margin: '0px', color: 'rgb(0, 0, 0)', border: '0px none rgb(0, 0, 0)',
};
const initialOf = (name: string) => INITIAL[name] ?? '';

const XHTML = 'http://www.w3.org/1999/xhtml';
const svg = (body: string) => `<svg xmlns="http://www.w3.org/2000/svg"><foreignObject>${body}</foreignObject></svg>`;
const styleOf = (markup: string, id: string) => {
  const doc = new DOMParser().parseFromString(markup, 'image/svg+xml');
  return declarations(doc.getElementById(id)!.getAttribute('style')!);
};

describe('compactSvg', () => {
  const markup = svg(`<div xmlns="${XHTML}" id="root" style="--tw-a: 1; color: red; display: block; margin: 0px; border: 0px none red">`
    + '<span id="a" style="--tw-a: 1; color: red; display: inline; margin: 0px; border: 0px none red; font-weight: 700">x</span>'
    + '<span id="b" style="color: blue; display: block; margin: 0px; border: 0px none red">y</span>'
    + '<button id="c" style="--tw-a: 1; color: red; margin: 0px; font-family: Inter">z</button>'
    + '</div>');

  it('drops custom properties everywhere, and what an element would get anyway', () => {
    const out = compactSvgMarkup(markup, initialOf);
    // The root keeps everything but custom properties.
    expect([...styleOf(out, 'root').entries()]).toEqual([
      ['color', 'red'], ['display', 'block'], ['margin', '0px'], ['border', '0px none red'],
    ]);
    // Inherited and equal to the parent: dropped; initial and equal to the
    // parent: dropped; borders: kept; the rest: kept.
    expect([...styleOf(out, 'a').entries()]).toEqual([
      ['display', 'inline'], ['border', '0px none red'], ['font-weight', '700'],
    ]);
    // A non-inherited value equal to the parent's but not initial stays.
    expect([...styleOf(out, 'b').entries()]).toEqual([
      ['color', 'blue'], ['display', 'block'], ['border', '0px none red'],
    ]);
    // The browser's stylesheet gives a button its own font and margins: only
    // its custom properties go.
    expect([...styleOf(out, 'c').entries()]).toEqual([['color', 'red'], ['margin', '0px'], ['font-family', 'Inter']]);
    expect(out).toContain('>x</span>');
  });

  it('splits declarations outside parentheses and quotes', () => {
    expect([...declarations('background-image: url(data:image/png;base64,AA==); content: "a;b"; color: red').entries()])
      .toEqual([['background-image', 'url(data:image/png;base64,AA==)'], ['content', '"a;b"'], ['color', 'red']]);
  });

  it('round-trips the data URL and leaves other URLs alone', () => {
    const url = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(markup)}`;
    const out = compactSvg(url, initialOf);
    expect(out.startsWith('data:image/svg+xml;charset=utf-8,')).toBe(true);
    const text = decodeURIComponent(out.slice('data:image/svg+xml;charset=utf-8,'.length));
    expect(text).not.toContain('--tw-a');
    expect(text.length).toBeLessThan(markup.length);
    expect(compactSvg('data:image/png;base64,AAAA', initialOf)).toBe('data:image/png;base64,AAAA');
  });
});
