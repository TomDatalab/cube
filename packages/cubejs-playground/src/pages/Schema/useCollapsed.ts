import { useCallback, useState } from 'react';

function read(key: string): boolean {
  try {
    return window.localStorage.getItem(key) === '1';
  } catch {
    return false;
  }
}

// Whether a side panel is collapsed, remembered in this browser. Storage may
// be unavailable (private window): the panel then just starts open.
export function useCollapsed(key: string): [boolean, () => void] {
  const [collapsed, setCollapsed] = useState(() => read(key));
  const toggle = useCallback(() => {
    setCollapsed((was) => {
      try {
        window.localStorage.setItem(key, was ? '0' : '1');
      } catch {
        // Kept for this visit only.
      }
      return !was;
    });
  }, [key]);
  return [collapsed, toggle];
}
