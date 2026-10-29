import { useEffect, useState } from 'react';

import { PG_LIGHT } from '../theme/dark-tokens';

function read(name: string) {
  try {
    return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || PG_LIGHT[name];
  } catch {
    return PG_LIGHT[name];
  }
}

/**
 * The current value of a `--pg-*` token (the light one without a theme), for props that end up as SVG attributes
 * (recharts), where `var()` is not reliable. Updates when the theme changes.
 */
export function useThemeColor(name: string) {
  const [value, setValue] = useState(() => read(name));

  useEffect(() => {
    const update = () => setValue(read(name));

    update();
    window.addEventListener('cube-theme', update);

    return () => window.removeEventListener('cube-theme', update);
  }, [name]);

  return value;
}
