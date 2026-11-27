import { useEffect } from 'react';
import type { ReactNode } from 'react';
import {
  AlignCenter, AlignLeft, AlignRight, Bold, Italic, Strikethrough, Underline,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';

import { Field } from '@/components/kit';
import { cn } from '@/lib/utils';

import {
  ModelDefinition, NOTE_ALIGNS, NOTE_BORDERS, NOTE_FONTS, NOTE_FONT_SIZES, NOTE_VALIGNS, Note,
} from './modelGraph';
import { setNote } from './objects';
import { DraftText, Palette, Section } from './PanelParts';

export type NotePropertiesProps = {
  def: ModelDefinition;
  id: string;
  onChange(next: ModelDefinition): void;
  focusKey?: number;
};

const SELECT = 'tw:h-7 tw:min-w-0 tw:rounded-md tw:border tw:bg-background tw:px-1 tw:text-xs tw:text-foreground';
const FONT_LABEL = { sans: 'Sans serif', serif: 'Serif', mono: 'Monospace' };
const VALIGN_LABEL = { top: 'Top', middle: 'Middle', bottom: 'Bottom' };
const BORDER_LABEL = {
  solid: 'Solid', dashed: 'Dashed', dotted: 'Dotted', none: 'No border',
};
const ALIGN_ICON: Record<(typeof NOTE_ALIGNS)[number], LucideIcon> = {
  left: AlignLeft, center: AlignCenter, right: AlignRight,
};

// A pressed-or-not button of the format bar.
function Toggle({
  label, pressed, onToggle, children,
}: { label: string; pressed: boolean; onToggle(): void; children: ReactNode }) {
  return (
    <button
      type="button"
      aria-label={label}
      aria-pressed={pressed}
      title={label}
      className={cn(
        'tw:flex tw:size-7 tw:cursor-pointer tw:items-center tw:justify-center tw:rounded-md tw:border tw:bg-transparent tw:p-0 tw:text-foreground tw:hover:bg-accent',
        pressed && 'tw:bg-accent tw:ring-1 tw:ring-ring'
      )}
      onClick={onToggle}
    >
      {children}
    </button>
  );
}

// Redgate's text note properties: its text, and how it looks: the font, its
// size and styles, the alignment, and the colors of the text, the box and
// its border.
export function NoteProperties({
  def, id, onChange, focusKey,
}: NotePropertiesProps) {
  const note = def.notes?.find((n) => n.id === id);
  useEffect(() => {
    if (focusKey) {
      (document.getElementById('model-note-text') as HTMLTextAreaElement | null)?.focus();
    }
  }, [focusKey, id]);
  if (!note) {
    return null;
  }
  const patch = (next: Partial<Omit<Note, 'id'>>) => onChange(setNote(def, id, next));
  // A style is saved only while on.
  const flip = (key: 'bold' | 'italic' | 'underline' | 'strike') => patch({ [key]: note[key] ? undefined : true });
  return (
    <>
      <Section title="General">
        <Field label="Text" htmlFor="model-note-text">
          <DraftText
            key={id}
            id="model-note-text"
            label="Note text"
            value={note.text}
            onCommit={(text) => patch({ text })}
          />
        </Field>
      </Section>
      <Section title="Format">
        <Field label="Font">
          <div className="tw:flex tw:gap-1" data-slot="model-note-font">
            <select
              aria-label="Font family"
              className={cn(SELECT, 'tw:flex-1')}
              value={note.fontFamily ?? 'sans'}
              onChange={(e) => patch({ fontFamily: e.target.value === 'sans' ? undefined : e.target.value as Note['fontFamily'] })}
            >
              {NOTE_FONTS.map((f) => <option key={f} value={f}>{FONT_LABEL[f]}</option>)}
            </select>
            <select
              aria-label="Font size"
              className={cn(SELECT, 'tw:w-16')}
              value={note.fontSize ?? 12}
              onChange={(e) => {
                const size = Number(e.target.value);
                patch({ fontSize: size === 12 ? undefined : size });
              }}
            >
              {NOTE_FONT_SIZES.map((s) => <option key={s} value={s}>{s}</option>)}
            </select>
          </div>
        </Field>
        <Field label="Style">
          <div className="tw:flex tw:flex-wrap tw:gap-1" data-slot="model-note-style">
            <Toggle label="Bold" pressed={Boolean(note.bold)} onToggle={() => flip('bold')}><Bold className="tw:size-3.5" /></Toggle>
            <Toggle label="Italic" pressed={Boolean(note.italic)} onToggle={() => flip('italic')}><Italic className="tw:size-3.5" /></Toggle>
            <Toggle label="Underline" pressed={Boolean(note.underline)} onToggle={() => flip('underline')}><Underline className="tw:size-3.5" /></Toggle>
            <Toggle label="Strikethrough" pressed={Boolean(note.strike)} onToggle={() => flip('strike')}><Strikethrough className="tw:size-3.5" /></Toggle>
            <span className="tw:mx-1 tw:w-px tw:self-stretch tw:bg-border" />
            {NOTE_ALIGNS.map((a) => {
              const Icon = ALIGN_ICON[a];
              const on = (note.align ?? 'left') === a;
              return (
                <Toggle key={a} label={`Align ${a}`} pressed={on} onToggle={() => patch({ align: a === 'left' ? undefined : a })}>
                  <Icon className="tw:size-3.5" />
                </Toggle>
              );
            })}
          </div>
        </Field>
        <Field label="Vertical alignment">
          <select
            aria-label="Vertical alignment"
            className={cn(SELECT, 'tw:w-full')}
            value={note.valign ?? 'top'}
            onChange={(e) => patch({ valign: e.target.value === 'top' ? undefined : e.target.value as Note['valign'] })}
          >
            {NOTE_VALIGNS.map((v) => <option key={v} value={v}>{VALIGN_LABEL[v]}</option>)}
          </select>
        </Field>
        <Field label="Text color">
          <Palette slot="model-note-text-color" none value={note.textColor} onPick={(textColor) => patch({ textColor })} />
        </Field>
        <Field label="Box color">
          <Palette slot="model-color-palette" none value={note.color} onPick={(color) => patch({ color })} />
        </Field>
        <Field label="Border">
          <div className="tw:flex tw:flex-col tw:gap-1">
            <select
              aria-label="Border style"
              className={cn(SELECT, 'tw:w-full')}
              value={note.borderStyle ?? 'solid'}
              onChange={(e) => patch({ borderStyle: e.target.value === 'solid' ? undefined : e.target.value as Note['borderStyle'] })}
            >
              {NOTE_BORDERS.map((b) => <option key={b} value={b}>{BORDER_LABEL[b]}</option>)}
            </select>
            <Palette slot="model-note-border-color" none value={note.borderColor} onPick={(borderColor) => patch({ borderColor })} />
          </div>
        </Field>
      </Section>
    </>
  );
}
