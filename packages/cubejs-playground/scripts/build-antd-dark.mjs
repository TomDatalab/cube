// Generates public/antd.dark.min.css from antd's dark theme with the Playground's dark palette.
// Run with `yarn build:antd-dark`; the output is committed.
import { createRequire } from 'module';
import { readFileSync, writeFileSync } from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

const require = createRequire(import.meta.url);
const less = require('less');
const { VARIABLES } = require('../variables-dark.js');

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const antdDir = path.dirname(require.resolve('antd/package.json'));
const entry = path.join(antdDir, 'dist/antd.dark.less');

const modifyVars = { ...VARIABLES };

const { css } = await less.render(readFileSync(entry, 'utf8'), {
  filename: entry,
  paths: [path.join(antdDir, 'dist'), path.join(antdDir, 'lib/style')],
  javascriptEnabled: true,
  compress: true,
  modifyVars,
});

// the light sheet embeds the same font import
const fontImport = '@import url("https://fonts.googleapis.com/css2?family=Inter:wght@400;500;700&display=swap&css");';
const output = fontImport + css;

writeFileSync(path.join(root, 'public/antd.dark.min.css'), output);
console.log(`antd.dark.min.css written (${output.length} bytes)`);
