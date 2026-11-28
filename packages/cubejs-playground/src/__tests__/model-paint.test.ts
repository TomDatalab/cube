import { describe, expect, it } from 'vitest';

import { inkOn, paint } from '../pages/Schema/models/paint';
import { noteStyle } from '../pages/Schema/models/NoteNode';

describe('paint', () => {
  it('a color as it is, or mixed toward clear by its transparency', () => {
    expect(paint('blue')).toBe('var(--model-blue)');
    expect(paint('blue', 0)).toBe('var(--model-blue)');
    expect(paint('blue', 40)).toBe('color-mix(in oklch, var(--model-blue) 60%, transparent)');
  });

  it('text stays dark on a color until the canvas shows through', () => {
    expect(inkOn()).toBe('var(--model-ink)');
    expect(inkOn(45)).toBe('var(--model-ink)');
    expect(inkOn(50)).toBe('var(--foreground)');
  });

  it('a note applies each color\'s own transparency', () => {
    const s = noteStyle('amber', {
      colorTransparency: 30,
      textColor: 'red',
      textTransparency: 20,
      borderColor: 'blue',
      borderTransparency: 50,
    });
    expect(s.background).toBe('color-mix(in oklch, var(--model-amber) 70%, transparent)');
    expect(s.color).toBe('color-mix(in oklch, var(--model-red) 80%, transparent)');
    expect(String(s.border)).toContain('color-mix(in oklch, var(--model-blue) 50%, transparent)');
  });
});
