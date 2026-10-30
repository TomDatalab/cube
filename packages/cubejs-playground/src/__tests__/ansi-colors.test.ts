import { describe, expect, it } from 'vitest';

import { generateAnsiHTML } from '../components/Error/utils';
import { PG_DARK, PG_LIGHT } from '../theme/dark-tokens';

const ESC = '\u001b';

describe('error output ANSI colors', () => {
  // colors unreadable on the dark error Alert come from tokens whose light
  // value is the color the output always had
  it.each([
    ['90', '--pg-ansi-black', 'black'],
    ['31', '--pg-ansi-red', 'red'],
    ['32', '--pg-ansi-green', 'green'],
    ['37', '--pg-ansi-darkgrey', 'darkgrey'],
  ])('maps code %s to %s', (code, token, light) => {
    const html = generateAnsiHTML(`${ESC}[${code}mtext${ESC}[0m`);

    expect(html).toContain(`<span style="color: var(${token}, ${light});">text</span>`);
    expect(PG_LIGHT[token]).toBe(light);
    expect(PG_DARK[token]).toMatch(/^#[0-9a-f]{6}$/);
  });
});
