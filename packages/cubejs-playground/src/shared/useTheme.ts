import { useCallback, useEffect, useState } from 'react';

import {
  applyTheme,
  readPreference,
  resolveTheme,
  THEME_STORAGE_KEY,
  writePreference,
} from './theme';
import type { ThemePreference, Theme } from './theme';

const QUERY = '(prefers-color-scheme: dark)';

function getMedia(): MediaQueryList | null {
  return typeof window.matchMedia === 'function' ? window.matchMedia(QUERY) : null;
}

function initialPreference(): ThemePreference {
  try {
    return readPreference(window.localStorage);
  } catch {
    return 'system';
  }
}

export function useTheme(): {
  preference: ThemePreference;
  theme: Theme;
  setPreference(p: ThemePreference): void;
} {
  const [preference, setPreferenceState] = useState<ThemePreference>(initialPreference);
  const [systemDark, setSystemDark] = useState<boolean>(() => !!getMedia()?.matches);

  useEffect(() => {
    const media = getMedia();
    if (!media) {
      return;
    }

    const onChange = (e: { matches: boolean }) => setSystemDark(e.matches);
    setSystemDark(media.matches);
    media.addEventListener('change', onChange);

    return () => media.removeEventListener('change', onChange);
  }, []);

  useEffect(() => {
    // follows the preference changed in other tabs
    const onStorage = (e: StorageEvent) => {
      if (e.key === THEME_STORAGE_KEY || e.key === null) {
        setPreferenceState(initialPreference());
      }
    };
    window.addEventListener('storage', onStorage);

    return () => window.removeEventListener('storage', onStorage);
  }, []);

  const theme = resolveTheme(preference, systemDark);

  useEffect(() => {
    applyTheme(theme);
  }, [theme]);

  const setPreference = useCallback((pref: ThemePreference) => {
    writePreference(pref);
    setPreferenceState(pref);
  }, []);

  return { preference, theme, setPreference };
}
