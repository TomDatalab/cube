import { useEffect, useRef } from 'react';

// Keys that work while typing too: saving should not wait for a blur.
const ANYWHERE = new Set(['mod+s']);

function typing(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el || typeof el.closest !== 'function') {
    return false;
  }
  return Boolean(el.closest('input, textarea, select, [contenteditable=""], [contenteditable="true"]'));
}

// `mod+shift+z`, `mod+alt+n`, `delete`, `1`: Ctrl or Cmd is `mod`; letters
// and digits come from the physical key, so Alt combos read the same on a Mac.
export function comboOf(e: KeyboardEvent): string {
  let key = e.key.toLowerCase();
  if (e.code?.startsWith('Key')) {
    key = e.code.slice(3).toLowerCase();
  } else if (e.code?.startsWith('Digit')) {
    key = e.code.slice(5);
  }
  return [
    e.ctrlKey || e.metaKey ? 'mod' : null,
    e.shiftKey ? 'shift' : null,
    e.altKey ? 'alt' : null,
    key,
  ].filter(Boolean).join('+');
}

// Runs the handler of a pressed combo, unless the focus is in a text field.
export function useEditorShortcuts(map: Record<string, (e: KeyboardEvent) => void>, enabled: boolean) {
  const current = useRef(map);
  current.current = map;
  useEffect(() => {
    if (!enabled) {
      return undefined;
    }
    const onKey = (e: KeyboardEvent) => {
      const combo = comboOf(e);
      const run = current.current[combo];
      if (!run || (typing(e.target) && !ANYWHERE.has(combo))) {
        return;
      }
      e.preventDefault();
      run(e);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [enabled]);
}
