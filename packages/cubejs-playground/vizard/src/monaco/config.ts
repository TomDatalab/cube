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

const PURPLE = '#2f5597';
const PINK = '#D3005A';
const GREEN = '#30A666';
const VIOLET = '#993388';
const DARK = '#21263f';
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
    'editor.background': '#1e2337',
    'editor.foreground': '#e8eaf3',
    'editor.lineHighlightBackground': '#333a57',
    'editor.lineHighlightBorder': '#333a57',
    'editor.selectionBackground': '#8fb4e644',
    'editor.inactiveSelectionBackground': '#8fb4e626',
    'editorCursor.foreground': '#8fb4e6',
    'editorLineNumber.foreground': '#6f7594',
    'editorLineNumber.activeForeground': '#e8eaf3',
    'editorWidget.background': '#262c43',
    'editorWidget.border': '#333a57',
    'editorGutter.background': '#1e2337',
    'scrollbarSlider.background': '#8fb4e633',
    'scrollbarSlider.hoverBackground': '#8fb4e655',
  },
  rules: [
    { token: 'keyword.sql', foreground: '#9fbeeb' },
    { token: 'number.sql', foreground: '#e0b25c' },
    { token: 'constant.other.table-name.sql', foreground: '#9fbeeb' },
    { token: 'string.sql', foreground: '#5fd39a' },
    { token: 'comment.js', foreground: '#7c82a3' },
    { token: 'comment.yaml', foreground: '#7c82a3' },
    { token: 'identifier', foreground: '#e8eaf3' },
    { token: 'operators', foreground: '#5fd39a' },
    { token: 'number', foreground: '#5fd39a' },
    { token: 'number.yaml', foreground: '#ff7da0' },
    { token: 'entity', foreground: '#5fd39a' },
    { token: 'keyword', foreground: '#9fbeeb' },
    { token: 'keyword.json', foreground: '#9fbeeb' },
    { token: 'string', foreground: '#ff7da0' },
    { token: 'string.key.json', foreground: '#ff7da0' },
    { token: 'string.yaml', foreground: '#9fbeeb' },
    { token: 'string.value.json', foreground: '#9fbeeb' },
    { token: 'regexp', foreground: '#ff7da0' },
    { token: 'annotation', foreground: '#ff7da0' },
    { token: 'type', foreground: '#e8eaf3' },
    { token: 'constant', foreground: '#9fbeeb' },
    { token: 'variable', foreground: '#9fbeeb' },
    { token: 'operator.sql', foreground: '#9fbeeb' },
    { token: 'delimiter', foreground: '#d58bd0' },
    { token: 'operators.yaml', foreground: '#d58bd0' },
    { token: 'tag', foreground: '#d58bd0' },
    { token: 'key', foreground: '#d58bd0' },
  ],
});
