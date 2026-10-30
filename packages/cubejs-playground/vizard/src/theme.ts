// A minimal copy of src/shared/theme.ts (the source of truth) without the antd
// links, the vizard is a separate app and cannot import from the Playground.
// Keep the storage key and the resolution rules in sync with it.
export type ThemePreference = 'system' | 'light' | 'dark';
export type Theme = 'light' | 'dark';

export const THEME_STORAGE_KEY = 'cube-playground-theme';

const DARK_QUERY = '(prefers-color-scheme: dark)';

export function resolveTheme(pref: ThemePreference, systemDark: boolean): Theme {
  if (pref === 'system') {
    return systemDark ? 'dark' : 'light';
  }

  return pref;
}

export function readPreference(): ThemePreference {
  try {
    const value = window.localStorage.getItem(THEME_STORAGE_KEY);

    return value === 'light' || value === 'dark' ? value : 'system';
  } catch {
    return 'system';
  }
}

function isSystemDark(): boolean {
  return !!window.matchMedia && window.matchMedia(DARK_QUERY).matches;
}

export function getTheme(): Theme {
  return resolveTheme(readPreference(), isSystemDark());
}

export function applyTheme(theme: Theme, doc: Document = document): void {
  doc.documentElement.dataset.theme = theme;
}

// The Playground (same origin) writes the key, which fires `storage` here;
// the system preference only matters while the preference is 'system'.
export function watchTheme(onChange: (theme: Theme) => void): () => void {
  const update = () => onChange(getTheme());
  const onStorage = (event: StorageEvent) => {
    if (event.key === null || event.key === THEME_STORAGE_KEY) {
      update();
    }
  };
  const media = window.matchMedia ? window.matchMedia(DARK_QUERY) : null;

  window.addEventListener('storage', onStorage);
  media?.addEventListener('change', update);

  return () => {
    window.removeEventListener('storage', onStorage);
    media?.removeEventListener('change', update);
  };
}
