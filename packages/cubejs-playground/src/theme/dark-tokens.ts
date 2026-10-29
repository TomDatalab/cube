// Dark values for every color the antd sheet does not paint: the ui-kit
// tokens, the playground's own body variables (src/variables.ts), the query
// builder member colors and the `--pg-*` roles components use. Light values
// stay where they are; only `--pg-*` and `--playground-muted` get a light
// block here, with the colors the Playground already uses for those roles.

// ui-kit palette (`@cube-dev/ui-kit/es/tokens.js`, `colors`): text colors turn
// light, background colors turn into palette C surfaces, danger and warning are
// lightened just enough for AA contrast on the dark background
const UI_KIT_COLORS: Record<string, string> = {
  pink: '255, 125, 160',
  'pink-02': '255, 150, 185',
  purple: '143, 139, 255',
  'purple-text': '169, 165, 255',
  'purple-01': '143, 139, 255',
  'purple-02': '128, 125, 230',
  'purple-03': '110, 107, 200',
  'purple-04': '80, 78, 150',
  text: '200, 200, 214',
  dark: '231, 231, 239',
  'dark-01': '231, 231, 239',
  'dark-02': '200, 200, 214',
  'dark-03': '154, 154, 175',
  'dark-04': '110, 110, 130',
  'dark-05': '54, 54, 68',
  'grey-light': '35, 35, 45',
  light: '43, 43, 55',
  white: '35, 35, 45',
  black: '0, 0, 0',
  'danger-text': '235, 77, 106',
  danger: '255, 55, 73',
  'light-grey': '35, 35, 45',
  success: '63, 191, 127',
  'success-text': '63, 191, 127',
  note: '251, 188, 5',
  warning: '226, 94, 54',
};

// colors only src/variables.ts defines, on body
const PLAYGROUND_COLORS: Record<string, string> = {
  green: '63, 191, 127',
  yellow: '255, 200, 60',
  gray: '35, 35, 45',
};

function rgba(triple: string, opacity: number | string = 1) {
  return `rgba(${triple}, ${opacity})`;
}

const c = (name: string, opacity?: number) => rgba(UI_KIT_COLORS[name], opacity);

// ui-kit tokens that are colors but not part of the palette
const UI_KIT_DERIVED: Record<string, string> = {
  'border-color': 'rgba(54, 54, 68, 1)',
  'border-opaque-color': 'rgba(54, 54, 68, 1)',
  'shadow-color': c('black', 0.4),
  'draft-color': c('dark', 0.2),
  'minor-color': c('dark', 0.65),
  'success-bg-color': c('success', 0.12),
  'note-bg-color': c('note', 0.12),
  'note-text-color': c('note'),
  'danger-bg-color': c('danger', 0.1),
  'danger-bg-hover-color': c('danger', 0.16),
  'dark-75-color': c('dark', 0.75),
  'primary-color': c('purple'),
  'pink-8-color': c('pink', 0.2),
  'pink-9-color': c('pink', 0.1),
  'disabled-color': c('dark-01', 0.25),
  'disabled-text-color': c('dark-01', 0.25),
  'disabled-bg': c('dark-05', 0.4),
  'disabled-bg-color': c('dark-05', 0.4),
  ...Object.fromEntries(
    [0.9, 0.8, 0.7, 0.6, 0.5, 0.4, 0.3, 0.2, 0.1, 0].map((opacity, i) => [
      `primary-${i + 1}`,
      c('purple', opacity),
    ])
  ),
};

// the variables src/variables.ts puts on body that are colors
const PLAYGROUND_DERIVED: Record<string, string> = {
  'active-bg': c('purple', 0.1),
  'primary-bg': c('purple', 0.1),
  'light-5': c('light', 0.5),
  'remove-btn-bg': c('purple', 0.1),
  'remove-btn-hover-bg': c('purple', 0.2),
  'success-bg-color': rgba(PLAYGROUND_COLORS.green, 0.1),
  'success-color': rgba(PLAYGROUND_COLORS.green, 0.9),
  'warning-bg-color': rgba(PLAYGROUND_COLORS.yellow, 0.1),
  'warning-color': rgba(PLAYGROUND_COLORS.yellow, 0.9),
  'pink-8': c('pink', 0.2),
  'pink-9': c('pink', 0.1),
  'heading-color': c('dark'),
  'link-color': c('purple'),
  'info-color': c('purple'),
  'layout-body-background': '#1c1c24',
  'layout-header-background': '#17171f',
  'menu-highlight-color': c('dark-01'),
  'item-hover-bg': c('light'),
  'menu-item-active-bg': c('light'),
};

function rgbOf(value: string) {
  return value.match(/^rgba\((\d+, \d+, \d+), [\d.]+\)$/)![1];
}

// tasty emits `--<name>-color` and `--<name>-color-rgb` for every `-color` token
function withRgb(tokens: Record<string, string>) {
  return Object.entries(tokens).reduce<Record<string, string>>((map, [name, value]) => {
    map[`--${name}`] = value;
    if (name.endsWith('-color')) {
      map[`--${name}-rgb`] = rgbOf(value);
    }
    return map;
  }, {});
}

function palette(colors: Record<string, string>) {
  return Object.entries(colors).reduce<Record<string, string>>((map, [name, triple]) => {
    map[`--${name}-color`] = rgba(triple);
    map[`--${name}-color-rgb`] = triple;
    return map;
  }, {});
}

export const UI_KIT_DARK: Record<string, string> = {
  ...palette(UI_KIT_COLORS),
  ...withRgb(UI_KIT_DERIVED),
};

// the variables.ts names; where they clash with the ui-kit ones (success,
// warning) body keeps the playground value and Root the ui-kit one, as in light
export const PLAYGROUND_DARK: Record<string, string> = {
  ...palette({
    ...Object.fromEntries(
      ['pink', 'purple', 'purple-03', 'purple-04', 'text', 'dark-01', 'dark-02', 'dark-03', 'dark-04', 'dark-05', 'light']
        .map((name) => [name, UI_KIT_COLORS[name]])
    ),
    ...PLAYGROUND_COLORS,
  }),
  ...Object.fromEntries(
    Object.entries(PLAYGROUND_DERIVED).map(([name, value]) => [`--${name}`, value])
  ),
};

// query builder member colors (QueryBuilderV2/color-tokens.ts) keep their hue:
// strong and text turn light, the active and hover chip backgrounds turn dim
// and low in saturation
const QUERY_BUILDER_COLORS: Record<string, Record<string, string>> = {
  'time-dimension': {
    strong: '211, 231, 208',
    text: '173, 211, 167',
    active: '54, 90, 48',
    hover: '42, 64, 38',
  },
  measure: {
    strong: '241, 229, 198',
    text: '222, 204, 156',
    active: '90, 78, 48',
    hover: '64, 57, 38',
  },
  dimension: {
    strong: '207, 213, 232',
    text: '165, 178, 212',
    active: '48, 59, 90',
    hover: '38, 45, 64',
  },
  segment: {
    strong: '219, 210, 229',
    text: '189, 171, 207',
    active: '69, 50, 87',
    hover: '51, 38, 64',
  },
  filter: {
    strong: '231, 208, 219',
    text: '210, 167, 188',
    active: '90, 48, 68',
    hover: '64, 38, 50',
  },
  missing: {
    strong: '219, 219, 219',
    text: '189, 189, 189',
    active: '69, 69, 69',
    hover: '51, 51, 51',
  },
};

export const QUERY_BUILDER_DARK: Record<string, string> = Object.entries(
  QUERY_BUILDER_COLORS
).reduce<Record<string, string>>((map, [type, shades]) => {
  for (const [shade, triple] of Object.entries(shades)) {
    map[`--${type}-${shade}-color`] = `rgb(${triple})`;
    map[`--${type}-${shade}-color-rgb`] = triple;
  }
  return map;
}, {});

// playground roles; light = what the Playground paints today
export const PG_LIGHT: Record<string, string> = {
  '--pg-bar': '#43436b', // header, var(--dark-02-color)
  '--pg-bg': '#f6f6f8', // @layout-body-background
  '--pg-surface': '#ffffff',
  '--pg-raised': '#ffffff', // antd popovers and menus
  '--pg-line': '#eeeeee', // sidebar and panel borders
  '--pg-text': '#141446', // dark-01
  '--pg-muted': '#a1a1b5', // theme.ts grey, dark-04
  '--pg-accent': '#7a77ff', // purple
  '--pg-accent-soft': 'rgba(122, 119, 255, 0.1)', // @primary-bg
  '--pg-success': '#41b56f', // variables.ts green
  '--playground-muted': '#a9a9b6', // index.html menu links
  // surfaces and lines the components used to hardcode
  '--pg-hover': '#f3f3fb', // active dropdown item
  '--pg-code-bg': '#f2f4f5', // inline code
  '--pg-menu-text': '#5b5c7d',
  '--pg-subtle': '#f8f8f9', // database cards
  '--pg-hairline': 'rgba(0, 0, 0, 0.06)',
  '--pg-line-soft': '#d7d7f488', // card headers
  '--pg-tab-line': 'rgb(234, 234, 238)',
  '--pg-tab-bg': 'rgb(243, 243, 255)',
  '--pg-orange': '#D26E0B', // view tag
  '--pg-positive': '#52c41a',
  '--pg-negative': '#c2371b',
  // charts
  '--pg-grid': '#f5f5f5',
  '--pg-axis': '#666666', // recharts default tick color
  '--pg-chart-border': '#cccccc', // recharts default tooltip border
  // access graph
  '--pg-graph-accent-bg': '#e8e7ff',
  '--pg-graph-line': '#d9d8e8',
  '--pg-graph-muted': '#6b6a85',
  '--pg-graph-fg': '#1f1d3a',
  '--pg-graph-derived': '#9c9ab8',
  '--pg-graph-panel': '#f6f6fb',
  '--pg-graph-node': '#ffffff',
  '--pg-graph-user': '#1f1d3a',
  '--pg-graph-organization': '#2f7d6d',
  '--pg-graph-workspace': '#6a66f0',
  '--pg-graph-connection': '#b8641e',
  // atoms/Alert
  '--pg-alert-error-border': '#FFCCC7',
  '--pg-alert-error-bg': 'rgb(255, 100, 109, 0.05)',
  '--pg-alert-error-text': '#EF404A',
  '--pg-alert-warning-border': '#FFE58F',
  '--pg-alert-warning-bg': '#FFFBE6',
  '--pg-alert-info-border': '#91D5FF',
  '--pg-alert-info-bg': '#E6F7FF',
  '--pg-alert-success-border': '#B7EB8F',
  '--pg-alert-success-bg': '#F6FFED',
};

// palette C
export const PG_DARK: Record<string, string> = {
  '--pg-bar': '#17171f',
  '--pg-bg': '#1c1c24',
  '--pg-surface': '#23232d',
  '--pg-raised': '#2b2b37',
  '--pg-line': '#363644',
  '--pg-text': '#e7e7ef',
  '--pg-muted': '#9a9aaf',
  '--pg-accent': '#8f8bff',
  '--pg-accent-soft': 'rgba(143, 139, 255, 0.17)',
  '--pg-success': '#3fbf7f',
  '--playground-muted': '#9a9aaf',
  '--pg-hover': '#2b2b37',
  '--pg-code-bg': '#2b2b37',
  '--pg-menu-text': '#c8c8d6',
  '--pg-subtle': '#272731',
  '--pg-hairline': 'rgba(255, 255, 255, 0.08)',
  '--pg-line-soft': '#36364488',
  '--pg-tab-line': '#363644',
  '--pg-tab-bg': '#2b2b40',
  '--pg-orange': '#f0993f',
  '--pg-positive': '#3fbf7f',
  '--pg-negative': '#ff7a65',
  '--pg-grid': '#2c2c38',
  '--pg-axis': '#9a9aaf',
  '--pg-chart-border': '#363644',
  '--pg-graph-accent-bg': '#34336b',
  '--pg-graph-line': '#3d3d52',
  '--pg-graph-muted': '#a8a8c0',
  '--pg-graph-fg': '#e7e7ef',
  '--pg-graph-derived': '#6e6e88',
  '--pg-graph-panel': '#262631',
  '--pg-graph-node': '#2b2b37',
  '--pg-graph-user': '#e7e7ef',
  '--pg-graph-organization': '#4fc3a8',
  '--pg-graph-workspace': '#8f8bff',
  '--pg-graph-connection': '#f0993f',
  '--pg-alert-error-border': 'rgba(255, 100, 109, 0.45)',
  '--pg-alert-error-bg': 'rgba(255, 100, 109, 0.12)',
  '--pg-alert-error-text': '#ff8a93',
  '--pg-alert-warning-border': 'rgba(251, 188, 5, 0.45)',
  '--pg-alert-warning-bg': 'rgba(251, 188, 5, 0.12)',
  '--pg-alert-info-border': 'rgba(64, 169, 255, 0.45)',
  '--pg-alert-info-bg': 'rgba(64, 169, 255, 0.12)',
  '--pg-alert-success-border': 'rgba(63, 191, 127, 0.45)',
  '--pg-alert-success-bg': 'rgba(63, 191, 127, 0.12)',
};

function declarations(map: Record<string, string>) {
  return Object.entries(map)
    .map(([name, value]) => `  ${name}: ${value};`)
    .join('\n');
}

export const LIGHT_CSS = `:root {
${declarations(PG_LIGHT)}
}
`;

const DARK = ':root[data-theme="dark"]';

// @ant-design/compatible/assets/index.css paints these fields white; same
// selectors under the dark theme, so they always win on specificity
const COMPAT_LIGHT_FIELDS = ['has-warning', 'has-error'].flatMap((status) => [
  ':not(.ant-input-disabled):not(.ant-input-borderless).ant-input',
  ':not(.ant-input-affix-wrapper-disabled):not(.ant-input-affix-wrapper-borderless).ant-input-affix-wrapper',
  ':not(.ant-input-disabled):not(.ant-input-borderless).ant-input:hover',
  ':not(.ant-input-affix-wrapper-disabled):not(.ant-input-affix-wrapper-borderless).ant-input-affix-wrapper:hover',
  '.ant-input',
  '.ant-input:hover',
  '.ant-input-affix-wrapper .ant-input',
  '.ant-input-affix-wrapper .ant-input:hover',
  '.ant-input-group-addon',
].map((field) => `.${status} ${field}`));

// The ui-kit Root re-declares its tokens (and the query builder ones) on its
// own element with a doubled class, (0,2,0), and variables.ts declares its own
// on body, (0,0,1): the dark values go on both, with a higher specificity.
// The query builder panel (data-qa="QueryBuilder") re-declares the member
// colors on its own element, (0,2,0), so it gets them again at (0,3,0)
export const DARK_CSS = `${DARK} {
  color-scheme: dark;
${declarations(PG_DARK)}
}

${DARK},
${DARK} body,
${DARK} #cube-ui-kit-root {
${declarations(UI_KIT_DARK)}
${declarations(QUERY_BUILDER_DARK)}
}

${DARK} body {
${declarations(PLAYGROUND_DARK)}
}

${DARK} [data-qa="QueryBuilder"] {
${declarations(QUERY_BUILDER_DARK)}
}

${DARK} body {
  background-color: var(--pg-bg) !important;
  color: rgba(231, 231, 239, 0.75);
}

${DARK} kbd {
  background-color: var(--pg-raised);
}

${COMPAT_LIGHT_FIELDS.map((selector) => `${DARK} ${selector}`).join(',\n')} {
  background-color: var(--pg-surface);
}

${DARK} .token.comment,
${DARK} .token.prolog,
${DARK} .token.doctype,
${DARK} .token.cdata,
${DARK} .token.punctuation {
  color: var(--pg-muted);
}

${DARK} .token.property,
${DARK} .token.tag,
${DARK} .token.boolean,
${DARK} .token.number,
${DARK} .token.constant,
${DARK} .token.symbol,
${DARK} .token.deleted,
${DARK} .token.function,
${DARK} .token.class-name,
${DARK} .token.regex,
${DARK} .token.important,
${DARK} .token.variable {
  color: rgb(var(--pink-color-rgb));
}

${DARK} .token.selector,
${DARK} .token.attr-name,
${DARK} .token.string,
${DARK} .token.char,
${DARK} .token.builtin,
${DARK} .token.inserted {
  color: var(--pg-accent);
}

${DARK} .token.operator,
${DARK} .token.entity,
${DARK} .token.url,
${DARK} .token.atrule,
${DARK} .token.attr-value,
${DARK} .token.keyword {
  color: var(--pg-text);
}
`;
