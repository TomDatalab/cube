import { createGlobalStyle } from 'styled-components';

import { DARK_CSS, LIGHT_CSS } from './dark-tokens';

// the colors of both themes the antd sheets do not cover; the dark ones apply
// under html[data-theme="dark"], set by src/shared/theme.ts
const ThemeStyles = createGlobalStyle`
  ${LIGHT_CSS}
  ${DARK_CSS}
`;

export default ThemeStyles;
