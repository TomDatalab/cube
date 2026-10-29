import { useEffect, useState } from 'react';

import { applyTheme, getTheme, Theme, watchTheme } from '../theme';

export function useTheme(): Theme {
  const [theme, setTheme] = useState<Theme>(getTheme);

  useEffect(() => {
    applyTheme(theme);
  }, [theme]);

  useEffect(() => watchTheme(setTheme), []);

  return theme;
}
