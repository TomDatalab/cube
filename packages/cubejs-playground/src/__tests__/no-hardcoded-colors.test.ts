import { readdirSync, readFileSync, statSync } from 'fs';
import { join, relative } from 'path';
import { describe, expect, it } from 'vitest';

const SRC = join(__dirname, '..');

// files that define the theme itself
const SKIPPED = ['__tests__', 'theme', 'QueryBuilderV2/color-tokens.ts'];

// justified literals: [path, text found on the line]; an empty text exempts the file
const EXCEPTIONS: [string, string][] = [
  // the light values the antd less variables are defined with
  ['variables.ts', ''],
  // brand logo, not used in the UI chrome
  ['shared/icons/CubejsIcon.tsx', ''],
  // chart series palettes, they read well on both backgrounds
  ['QueryBuilderV2/utils/chart-colors.ts', ''],
  // terminal (ANSI) palette of the error output
  ['components/Error/utils.ts', ''],
  // recharts default pie fill, replaced by each Cell
  ['QueryBuilderV2/components/ChartRenderer.tsx', 'fill="#8884d8"'],
  // the header bar is dark in both themes
  ['components/Header/Header.tsx', ''],
  ['components/Header/Menu.tsx', ''],
  ['components/Header/WorkspaceSelector.tsx', "color: '#ff7875'"],
  // text on the accent color (selected items, primary buttons)
  ['components/Tree.tsx', 'color: white;'],
  ['components/GlobalStyles.tsx', 'color: white;'],
  ['atoms/Button.tsx', 'color: white;'],
];

function files(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    const rel = relative(SRC, path);
    if (SKIPPED.includes(rel)) {
      return [];
    }
    if (statSync(path).isDirectory()) {
      return files(path);
    }
    return /\.(tsx?|less|css)$/.test(name) ? [path] : [];
  });
}

// the fallback of var(--x, <literal>) is allowed
const FALLBACK = /var\(\s*--[\w-]+\s*,\s*(?:rgba?\([^)]*\)|[^()]*)\)/g;
// rgba(var(--x-rgb), .5) takes its color from a token
const NAMED = 'white|black|gray|grey|silver|whitesmoke|lightgray|lightgrey|darkgray|darkgrey';
// named colors, not part of an identifier, a tasty token (#white), a property
// name (gray:) or a hyphenated word (white-space)
const COLOR = new RegExp(
  `#[0-9a-fA-F]{3,8}\\b|\\brgba?\\((?!\\s*var\\()|\\bhsla?\\(|(?<![\\w#.$-])(?:${NAMED})(?![\\w-]|\\s*:)`,
  'g'
);

function literals(line: string) {
  if (/^\s*(\/\/|\*|\/\*)/.test(line)) {
    return [];
  }
  return line.replace(FALLBACK, '').match(COLOR) || [];
}

describe('hardcoded colors', () => {
  it('only theme files hold color literals', () => {
    const found: string[] = [];

    for (const file of files(SRC)) {
      readFileSync(file, 'utf8')
        .split('\n')
        .forEach((line, i) => {
          for (const literal of literals(line)) {
            const where = `${relative(SRC, file)}:${i + 1}`;
            if (!EXCEPTIONS.some(([path, text]) => relative(SRC, file) === path && line.includes(text))) {
              found.push(`${where} ${literal}`);
            }
          }
        });
    }

    expect(found).toEqual([]);
  });
});
