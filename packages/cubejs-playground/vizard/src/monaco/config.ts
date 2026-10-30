import { editor } from 'monaco-editor';

export const MONOSPACE_FONT_FAMILY =
  'Menlo, Monaco, Consolas, "Ubuntu Mono", "Liberation Mono", "DejaVu Sans Mono", "Courier New", monospace';

export const LANGUAGE_MAP = {
  js: 'javascript',
  yml: 'yaml',
  yaml: 'yaml',
  ts: 'typescript',
  css: 'css',
  json: 'json',
  html: 'html',
  dockerfile: 'dockerfile',
  sql: 'sql',
  rust: 'rust',
  scss: 'scss',
  less: 'less',
  xml: 'xml',
  md: 'markdown',
  lua: 'lua',
  sh: 'shell',
  java: 'java',
  toml: 'toml',
  py: 'python',
  jinja: 'jinja',
};

export type FileExtension = keyof typeof LANGUAGE_MAP;

const PURPLE = '#665DE8';
const PINK = '#D3005A';
const GREEN = '#30A666';
const VIOLET = '#993388';
const DARK = '#141446';
const GREY = '#828282';
const BROWN = '#986801';

editor.defineTheme('cube', {
  base: 'vs',
  inherit: true,
  colors: {},
  rules: [
    { token: 'keyword.sql', foreground: PURPLE },
    { token: 'number.sql', foreground: BROWN },
    { token: 'constant.other.table-name.sql', foreground: PURPLE },
    { token: 'string.sql', foreground: GREEN },
    { token: 'comment.js', foreground: GREY },
    { token: 'comment.yaml', foreground: GREY },
    { token: 'identifier', foreground: DARK },
    { token: 'operators', foreground: GREEN },
    { token: 'number', foreground: GREEN },
    { token: 'number.yaml', foreground: PINK },
    { token: 'entity', foreground: GREEN },
    { token: 'keyword', foreground: PURPLE },
    { token: 'keyword.json', foreground: PURPLE },
    { token: 'string', foreground: PINK },
    { token: 'string.key.json', foreground: PINK },
    { token: 'string.yaml', foreground: PURPLE },
    { token: 'string.value.json', foreground: PURPLE },
    { token: 'regexp', foreground: PINK },
    { token: 'annotation', foreground: PINK },
    { token: 'type', foreground: DARK },
    { token: 'constant', foreground: PURPLE },
    { token: 'variable', foreground: PURPLE },
    { token: 'operator.sql', foreground: PURPLE },
    { token: 'delimiter', foreground: VIOLET },
    { token: 'operators.yaml', foreground: VIOLET },
    { token: 'tag', foreground: VIOLET },
    { token: 'key', foreground: VIOLET },
  ],
});

editor.defineTheme('cube-dark', {
  base: 'vs-dark',
  inherit: true,
  colors: {
    'editor.background': '#23232d',
    'editor.foreground': '#e7e7ef',
    'editor.lineHighlightBackground': '#363644',
    'editor.lineHighlightBorder': '#363644',
    'editor.selectionBackground': '#8f8bff44',
    'editor.inactiveSelectionBackground': '#8f8bff26',
    'editorCursor.foreground': '#8f8bff',
    'editorLineNumber.foreground': '#6e6e82',
    'editorLineNumber.activeForeground': '#e7e7ef',
    'editorWidget.background': '#2b2b37',
    'editorWidget.border': '#363644',
    'editorGutter.background': '#23232d',
    'scrollbarSlider.background': '#8f8bff33',
    'scrollbarSlider.hoverBackground': '#8f8bff55',
  },
  rules: [
    { token: 'keyword.sql', foreground: '#a9a5ff' },
    { token: 'number.sql', foreground: '#e0b25c' },
    { token: 'constant.other.table-name.sql', foreground: '#a9a5ff' },
    { token: 'string.sql', foreground: '#5fd39a' },
    { token: 'comment.js', foreground: '#7d7d94' },
    { token: 'comment.yaml', foreground: '#7d7d94' },
    { token: 'identifier', foreground: '#e7e7ef' },
    { token: 'operators', foreground: '#5fd39a' },
    { token: 'number', foreground: '#5fd39a' },
    { token: 'number.yaml', foreground: '#ff7da0' },
    { token: 'entity', foreground: '#5fd39a' },
    { token: 'keyword', foreground: '#a9a5ff' },
    { token: 'keyword.json', foreground: '#a9a5ff' },
    { token: 'string', foreground: '#ff7da0' },
    { token: 'string.key.json', foreground: '#ff7da0' },
    { token: 'string.yaml', foreground: '#a9a5ff' },
    { token: 'string.value.json', foreground: '#a9a5ff' },
    { token: 'regexp', foreground: '#ff7da0' },
    { token: 'annotation', foreground: '#ff7da0' },
    { token: 'type', foreground: '#e7e7ef' },
    { token: 'constant', foreground: '#a9a5ff' },
    { token: 'variable', foreground: '#a9a5ff' },
    { token: 'operator.sql', foreground: '#a9a5ff' },
    { token: 'delimiter', foreground: '#d58bd0' },
    { token: 'operators.yaml', foreground: '#d58bd0' },
    { token: 'tag', foreground: '#d58bd0' },
    { token: 'key', foreground: '#d58bd0' },
  ],
});
