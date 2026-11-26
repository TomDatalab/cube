import { useEffect } from 'react';

import { Field } from '@/components/kit';

import type { ModelDefinition } from './modelGraph';
import { setNote } from './objects';
import { DraftText, Palette, Section } from './PanelParts';

export type NotePropertiesProps = {
  def: ModelDefinition;
  id: string;
  onChange(next: ModelDefinition): void;
  focusKey?: number;
};

// Redgate's text note properties: its text, and its color.
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
  return (
    <>
      <Section title="General">
        <Field label="Text" htmlFor="model-note-text">
          <DraftText
            key={id}
            id="model-note-text"
            label="Note text"
            value={note.text}
            onCommit={(text) => onChange(setNote(def, id, { text }))}
          />
        </Field>
      </Section>
      <Section title="Format">
        <Palette slot="model-color-palette" none value={note.color} onPick={(color) => onChange(setNote(def, id, { color }))} />
      </Section>
    </>
  );
}
