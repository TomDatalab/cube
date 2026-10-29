export type ThemePreference = 'system' | 'light' | 'dark';
export type Theme = 'light' | 'dark';

export const THEME_STORAGE_KEY = 'cube-playground-theme';

export function resolveTheme(pref: ThemePreference, systemDark: boolean): Theme {
  if (pref === 'system') {
    return systemDark ? 'dark' : 'light';
  }

  return pref;
}

export function readPreference(storage?: Pick<Storage, 'getItem'>): ThemePreference {
  try {
    const value = (storage ?? window.localStorage).getItem(THEME_STORAGE_KEY);

    return value === 'light' || value === 'dark' ? value : 'system';
  } catch {
    return 'system';
  }
}

export function writePreference(pref: ThemePreference, storage?: Pick<Storage, 'setItem'>): void {
  try {
    (storage ?? window.localStorage).setItem(THEME_STORAGE_KEY, pref);
  } catch {
    // storage may be blocked, the preference just won't persist
  }
}

export function applyTheme(theme: Theme, doc: Document = document): void {
  doc.documentElement.dataset.theme = theme;

  // the links are absent in tests and in vizard
  const light = doc.getElementById('antd-light') as HTMLLinkElement | null;
  const dark = doc.getElementById('antd-dark') as HTMLLinkElement | null;

  const next = theme === 'dark' ? dark : light;
  const prev = theme === 'dark' ? light : dark;

  if (next) {
    next.disabled = false;
  }

  // a sheet not fetched yet (the other one never is until the first switch)
  // loads after it is enabled: keep the old one on until then, so antd is
  // never unstyled; a later switch back makes the pending load a no-op
  if (prev) {
    if (!next || next.sheet) {
      prev.disabled = true;
    } else {
      next.addEventListener('load', () => {
        if (doc.documentElement.dataset.theme === theme) {
          prev.disabled = true;
        }
      }, { once: true });
    }
  }

  window.dispatchEvent(new CustomEvent('cube-theme', { detail: theme }));
}
