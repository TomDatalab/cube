import { describe, expect, it } from 'vitest';

import { noteStyle } from '../pages/Schema/models/NoteNode';
import { noteFormat } from '../pages/Schema/models/modelGraph';

describe('note style', () => {
  it('defaults: pale yellow, 12px sans, top left, a thin solid border', () => {
    const s = noteStyle(undefined);
    expect(s).toEqual(expect.objectContaining({
      background: 'var(--model-note)',
      color: 'var(--foreground)',
      fontSize: 12,
      textAlign: 'left',
      justifyContent: 'flex-start',
    }));
    expect(String(s.border)).toContain('1px solid');
    expect(s.fontWeight).toBeUndefined();
  });

  it('a palette box is solid with dark text unless the text has a color', () => {
    expect(noteStyle('amber')).toEqual(expect.objectContaining({ background: 'var(--model-amber)', color: 'var(--model-ink)' }));
    expect(noteStyle('amber', { textColor: 'red' }).color).toBe('var(--model-red)');
  });

  it('applies the font, styles, alignment and border', () => {
    const s = noteStyle(undefined, {
      fontFamily: 'mono',
      fontSize: 20,
      bold: true,
      italic: true,
      underline: true,
      strike: true,
      align: 'right',
      valign: 'bottom',
      borderStyle: 'dotted',
      borderColor: 'blue',
    });
    expect(s).toEqual(expect.objectContaining({
      fontSize: 20,
      fontWeight: 700,
      fontStyle: 'italic',
      textDecoration: 'underline line-through',
      textAlign: 'right',
      justifyContent: 'flex-end',
      border: '1px dotted var(--model-blue)',
    }));
    expect(String(s.fontFamily)).toContain('monospace');
    expect(noteStyle(undefined, { borderStyle: 'none' }).border).toBe('none');
  });

  it('noteFormat keeps only the format', () => {
    const note = {
      id: 'n1',
      x: 1,
      y: 2,
      width: 3,
      height: 4,
      z: 1,
      text: 't',
      color: 'red' as const,
      bold: true,
      fontSize: 14,
    };
    expect(noteFormat(note)).toEqual({ bold: true, fontSize: 14 });
  });
});
