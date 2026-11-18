const colors = {
  pink: '255, 125, 160',
  purple: '143, 180, 230', // bossa-arq light blue #8fb4e6
  'purple-03': '86, 120, 180',
  'purple-04': '58, 84, 130',
  text: '163, 168, 195',
  'dark-01': '232, 234, 243',
  'dark-02': '203, 206, 222',
  'dark-03': '163, 168, 195',
  'dark-04': '111, 117, 148',
  'dark-05': '51, 58, 87',
  light: '38, 44, 67',
  green: '63, 191, 127',
  yellow: '255, 200, 60',
  gray: '30, 35, 55',
};

function color(name, opacity = 1) {
  return `rgba(${colors[name]}, ${opacity})`;
}

const VARIABLES = {
  'active-bg': color('purple', 0.1),
  'primary-bg': color('purple', 0.1),
  'primary-1': color('purple', 0.9),
  'primary-2': color('purple', 0.8),
  'primary-3': color('purple', 0.7),
  'primary-4': color('purple', 0.6),
  'primary-5': color('purple', 0.5),
  'primary-6': color('purple', 0.4),
  'primary-7': color('purple', 0.3),
  'primary-8': color('purple', 0.2),
  'primary-9': color('purple', 0.1),
  'primary-10': color('purple', 0),
  'light-5': color('light', 0.5),
  'remove-btn-bg': color('purple', 0.1),
  'remove-btn-hover-bg': color('purple', 0.2),
  'primary-color': '#8fb4e6',

  'success-bg-color': color('green', 0.1),
  'success-color': color('green', 0.9),
  'warning-bg-color': color('yellow', 0.1),
  'warning-color': color('yellow', 0.9),

  'pink-8': color('pink', 0.2),
  'pink-9': color('pink', 0.1),

  'heading-color': '#e8eaf3',
  'link-color': color('purple'),
  'info-color': color('purple'),
  'layout-body-background': '#171b2c',
  'layout-header-background': '#141726',
  'menu-highlight-color': color('dark-01'),
  'item-hover-bg': color('light'),
  'layout-header-height': '48px',

  'font-family':
    'Inter, -apple-system, BlinkMacSystemFont, "Segoe UI", "Avenir Next", Roboto, Oxygen-Sans, Ubuntu, Cantarell, "Helvetica Neue", sans-serif',
  'menu-item-font-size': '15px',
  'btn-primary-shadow': 'none',
  'btn-text-shadow': 'none',
  'modal-body-padding': '32px',
  'form-item-margin-bottom': '23px',

  'disabled-color': color('dark-01', 0.25),
  'disabled-bg': color('dark-05', 0.2),

  // menu
  'menu-item-active-bg': color('light'),

  'font-size-base': '14px',
  'border-radius-base': '4px',
  'padding-lg': '16px',
};

Object.keys(colors).forEach((name) => {
  VARIABLES[`${name}-color`] = color(name);
});

// the loop above derives `text-color` and friends from the palette, pin the antd ones
Object.assign(VARIABLES, {
  'body-background': '#171b2c',
  'component-background': '#1e2337',
  'popover-background': '#262c43',
  'border-color-base': '#333a57',
  'border-color-split': '#333a57',
  'text-color': '#e8eaf3',
  'text-color-secondary': '#a3a8c3',
  'heading-color': '#e8eaf3',
  'primary-color': '#8fb4e6',
  'layout-header-background': '#141726',

  // dark.less maps the info alert to @primary-1, a saturated purple here
  'alert-info-bg-color': 'rgba(143, 180, 230, 0.12)',
  'alert-info-border-color': 'rgba(143, 180, 230, 0.35)',

  // greys dark.less hardcodes, moved onto palette C
  'table-header-bg': '#1b2032',
  'table-row-hover-bg': '#262c43',
  'table-header-sort-bg': '#262c43',
  'table-header-filter-active-bg': '#333a57',
  'table-header-sort-active-bg': '#333a57',
  'table-fixed-header-sort-active-bg': '#333a57',
  'table-expanded-row-bg': '#1e2337',
  'popover-customize-border-color': '#333a57',
  'tooltip-bg': '#262c43',
  'transfer-item-hover-bg': '#262c43',
  'slider-rail-background-color': '#333a57',
  'card-skeleton-bg': '#262c43',
  'picker-basic-cell-disabled-bg': '#262c43',
  'layout-trigger-background': '#1e2337',
});

const LESS_VARIABLES = {};
const CSS_PROPERTIES = {};

Object.keys(VARIABLES).forEach((key) => {
  LESS_VARIABLES[`@${key}`] = VARIABLES[key];
});

Object.keys(VARIABLES).forEach((key) => {
  CSS_PROPERTIES[`--${key}`] = VARIABLES[key];
});

module.exports = {
  VARIABLES,
  LESS_VARIABLES,
  CSS_PROPERTIES,
};
