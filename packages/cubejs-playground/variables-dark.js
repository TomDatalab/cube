const colors = {
  pink: '255, 125, 160',
  purple: '143, 139, 255',
  'purple-03': '110, 107, 200',
  'purple-04': '80, 78, 150',
  text: '154, 154, 175',
  'dark-01': '231, 231, 239',
  'dark-02': '200, 200, 214',
  'dark-03': '154, 154, 175',
  'dark-04': '110, 110, 130',
  'dark-05': '54, 54, 68',
  light: '43, 43, 55',
  green: '63, 191, 127',
  yellow: '255, 200, 60',
  gray: '35, 35, 45',
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
  'primary-color': '#8f8bff',

  'success-bg-color': color('green', 0.1),
  'success-color': color('green', 0.9),
  'warning-bg-color': color('yellow', 0.1),
  'warning-color': color('yellow', 0.9),

  'pink-8': color('pink', 0.2),
  'pink-9': color('pink', 0.1),

  'heading-color': '#e7e7ef',
  'link-color': color('purple'),
  'info-color': color('purple'),
  'layout-body-background': '#1c1c24',
  'layout-header-background': '#17171f',
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
  'body-background': '#1c1c24',
  'component-background': '#23232d',
  'popover-background': '#2b2b37',
  'border-color-base': '#363644',
  'border-color-split': '#363644',
  'text-color': '#e7e7ef',
  'text-color-secondary': '#9a9aaf',
  'heading-color': '#e7e7ef',
  'primary-color': '#8f8bff',
  'layout-header-background': '#17171f',

  // dark.less maps the info alert to @primary-1, a saturated purple here
  'alert-info-bg-color': 'rgba(143, 139, 255, 0.12)',
  'alert-info-border-color': 'rgba(143, 139, 255, 0.35)',

  // greys dark.less hardcodes, moved onto palette C
  'table-header-bg': '#272731',
  'table-row-hover-bg': '#2b2b37',
  'table-header-sort-bg': '#2b2b37',
  'table-header-filter-active-bg': '#363644',
  'table-header-sort-active-bg': '#363644',
  'table-fixed-header-sort-active-bg': '#363644',
  'table-expanded-row-bg': '#23232d',
  'popover-customize-border-color': '#363644',
  'tooltip-bg': '#2b2b37',
  'transfer-item-hover-bg': '#2b2b37',
  'slider-rail-background-color': '#363644',
  'card-skeleton-bg': '#2b2b37',
  'picker-basic-cell-disabled-bg': '#2b2b37',
  'layout-trigger-background': '#23232d',
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
