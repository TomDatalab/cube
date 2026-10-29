// @vitest-environment jsdom
import { readFileSync } from 'fs';
import { createRequire } from 'module';
import { dirname, resolve } from 'path';
import { createElement, act } from 'react';
import { createRoot } from 'react-dom/client';
import { Root } from '@cube-dev/ui-kit';
import { describe, expect, it } from 'vitest';

import { QUERY_BUILDER_COLOR_TOKENS } from '../QueryBuilderV2/color-tokens';
import {
  DARK_CSS,
  LIGHT_CSS,
  PG_DARK,
  PG_LIGHT,
  QUERY_BUILDER_DARK,
  UI_KIT_DARK,
} from '../theme/dark-tokens';

(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;

// the ui-kit does not export its tokens, read the palette from its source
function uiKitColorNames(): string[] {
  const source = readFileSync(
    resolve(dirname(createRequire(import.meta.url).resolve('@cube-dev/ui-kit')), '../es/tokens.js'),
    'utf8'
  );
  const colors = source.match(/const colors = \{([\s\S]*?)\};/);

  expect(colors).not.toBeNull();

  return [...colors![1].matchAll(/^\s*'?([a-z0-9-]+)'?:/gm)].map((m) => m[1]);
}

// WCAG 2.x relative luminance and contrast ratio
function luminance([r, g, b]: number[]): number {
  const channel = (c: number) => {
    const v = c / 255;

    return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
  };

  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

function contrast(a: number[], b: number[]): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);

  return (hi + 0.05) / (lo + 0.05);
}

function parseColor(value: string): number[] {
  const hex = value.match(/^#([0-9a-f]{6})$/i);

  if (hex) {
    return [0, 2, 4].map((i) => parseInt(hex[1].slice(i, i + 2), 16));
  }

  const triple = value.match(/^(\d+),\s*(\d+),\s*(\d+)$/);

  if (triple) {
    return triple.slice(1, 4).map(Number);
  }

  throw new Error(`unparsed color ${value}`);
}

function block(css: string, selector: string): string {
  const start = css.indexOf(`${selector} {`);

  expect(start).toBeGreaterThanOrEqual(0);

  return css.slice(start, css.indexOf('}', start));
}

const BG = parseColor('#1c1c24');

describe('dark tokens', () => {
  it('covers every ui-kit color with a dark value and rgb triple', () => {
    const names = uiKitColorNames();

    expect(names.length).toBeGreaterThan(20);

    for (const name of names) {
      expect(UI_KIT_DARK[`--${name}-color`], name).toMatch(/^rgba\(\d+, \d+, \d+, 1\)$/);
      expect(UI_KIT_DARK[`--${name}-color-rgb`], name).toMatch(/^\d+, \d+, \d+$/);
      expect(DARK_CSS).toContain(`--${name}-color: ${UI_KIT_DARK[`--${name}-color`]};`);
      expect(DARK_CSS).toContain(`--${name}-color-rgb: ${UI_KIT_DARK[`--${name}-color-rgb`]};`);
    }
  });

  it('defines the palette C playground variables', () => {
    expect(DARK_CSS).toContain('--pg-bg: #1c1c24;');
    expect(DARK_CSS).toContain('--pg-accent: #8f8bff;');
    expect(DARK_CSS).toContain('--pg-text: #e7e7ef;');
  });

  it('defines every playground variable in both themes', () => {
    const names = [
      'bar', 'bg', 'surface', 'raised', 'line', 'text', 'muted', 'accent', 'accent-soft', 'success',
    ].map((n) => `--pg-${n}`).concat('--playground-muted');
    const light = block(LIGHT_CSS, ':root');
    const dark = block(DARK_CSS, ':root[data-theme="dark"]');

    expect(Object.keys(PG_LIGHT).sort()).toEqual([...names].sort());
    expect(Object.keys(PG_DARK).sort()).toEqual([...names].sort());

    for (const name of names) {
      expect(light).toContain(`${name}: ${PG_LIGHT[name]};`);
      expect(dark).toContain(`${name}: ${PG_DARK[name]};`);
    }

    // the light page keeps the muted link color index.html had inline
    expect(PG_LIGHT['--playground-muted']).toBe('#a9a9b6');
  });

  it('gives every query builder token a dark value', () => {
    const keys = Object.keys(QUERY_BUILDER_COLOR_TOKENS);

    expect(keys).toHaveLength(24);

    for (const key of keys) {
      const property = key.replace(/^@/, '--');

      expect(QUERY_BUILDER_DARK[property], key).toMatch(/^rgb\(\d+, \d+, \d+\)$/);
      expect(QUERY_BUILDER_DARK[`${property}-rgb`], key).toMatch(/^\d+, \d+, \d+$/);
      expect(DARK_CSS).toContain(`${property}: ${QUERY_BUILDER_DARK[property]};`);
    }
  });

  it('keeps the query builder hues and inverts their lightness', () => {
    const rgb = (key: string) => parseColor(QUERY_BUILDER_DARK[`--${key}-color-rgb`]);

    for (const type of ['time-dimension', 'measure', 'dimension', 'segment', 'filter', 'missing']) {
      // light text on a dim chip, both readable over the page
      expect(luminance(rgb(`${type}-text`))).toBeGreaterThan(luminance(rgb(`${type}-active`)));
      expect(contrast(rgb(`${type}-text`), rgb(`${type}-active`)), type).toBeGreaterThanOrEqual(4.5);
      expect(contrast(rgb(`${type}-strong`), rgb(`${type}-hover`)), type).toBeGreaterThanOrEqual(4.5);
      expect(contrast(rgb(`${type}-text`), BG), type).toBeGreaterThanOrEqual(4.5);
    }
  });

  it('reaches WCAG AA contrast on the dark background', () => {
    const samples = {
      '--pg-text': PG_DARK['--pg-text'],
      '--pg-muted': PG_DARK['--pg-muted'],
      '--playground-muted': PG_DARK['--playground-muted'],
      danger: UI_KIT_DARK['--danger-color-rgb'],
      'danger-text': UI_KIT_DARK['--danger-text-color-rgb'],
      warning: UI_KIT_DARK['--warning-color-rgb'],
      note: UI_KIT_DARK['--note-color-rgb'],
    };

    for (const [name, value] of Object.entries(samples)) {
      expect(contrast(parseColor(value), BG), name).toBeGreaterThanOrEqual(4.5);
    }
  });

  it('scopes the dark rules to the dark theme and the ui-kit root', () => {
    expect(DARK_CSS).toContain(':root[data-theme="dark"] #cube-ui-kit-root');
    expect(DARK_CSS).not.toMatch(/(^|[^:\w])\[data-theme="dark"\]/);
  });

  it('beats the specificity of the ui-kit root tokens', () => {
    const container = document.createElement('div');
    document.body.appendChild(container);
    const root = createRoot(container);

    act(() => root.render(createElement(Root, { styles: QUERY_BUILDER_COLOR_TOKENS })));

    const element = document.getElementById('cube-ui-kit-root');
    expect(element).not.toBeNull();

    const rules = [...document.querySelectorAll('style')]
      .flatMap((style) => [...((style as HTMLStyleElement).sheet?.cssRules ?? [])])
      .filter((rule): rule is CSSStyleRule => 'selectorText' in rule)
      .filter((rule) => rule.style.getPropertyValue('--purple-color') !== ''
        || rule.style.getPropertyValue('--measure-text-color') !== '');

    expect(rules.length).toBeGreaterThan(0);

    for (const rule of rules) {
      // styled-components doubles the class, (0,2,0): no id, so it loses to
      // :root[data-theme] #cube-ui-kit-root, (1,2,0)
      expect(rule.selectorText).toMatch(/^(\.[\w-]+){1,2}$/);
      expect(element!.matches(rule.selectorText)).toBe(true);
    }

    act(() => root.unmount());
    container.remove();
  });
});
