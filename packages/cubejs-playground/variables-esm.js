const colors = {
  pink: '255, 100, 146',
  purple: '47, 85, 151', // bossa-arq blue #2f5597
  'purple-03': '118, 156, 214',
  'purple-04': '159, 182, 221',
  text: '65, 69, 88',
  'dark-01': '33, 38, 63', // bossa-arq navy
  'dark-02': '65, 69, 88',
  'dark-03': '108, 115, 147',
  'dark-04': '169, 173, 194',
  'dark-05': '227, 228, 238',
  light: '241, 242, 248',
  green: '65, 181, 111',
  yellow: '251, 188, 5',
  gray: '251, 251, 254',
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
  'primary-color': color('purple'),

  'success-bg-color': color('green', 0.1),
  'success-color': color('green', 0.9),
  'warning-bg-color': color('yellow', 0.1),
  'warning-color': color('yellow', 0.9),

  'pink-8': color('pink', 0.2),
  'pink-9': color('pink', 0.1),

  'heading-color': color('dark', 0.65),
  'link-color': color('purple'),
  'info-color': color('purple'),
  'layout-body-background': '#fbfbfe',
  'layout-header-background': '#fbfbfe',
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

const LESS_VARIABLES = {};
const CSS_PROPERTIES = {};

Object.keys(VARIABLES).forEach((key) => {
  LESS_VARIABLES[`@${key}`] = VARIABLES[key];
});

Object.keys(VARIABLES).forEach((key) => {
  CSS_PROPERTIES[`--${key}`] = VARIABLES[key];
});

export { VARIABLES, LESS_VARIABLES, CSS_PROPERTIES };
