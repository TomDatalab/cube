import { Field } from '@/components/kit';

import type { ModelDefinition } from './modelGraph';
import { setArea } from './objects';
import { DraftText, Palette, Section } from './PanelParts';

export type AreaPropertiesProps = {
  def: ModelDefinition;
  id: string;
  onChange(next: ModelDefinition): void;
  focusKey?: number;
};

// Redgate's subject area properties: name and comment, fill and border.
export function AreaProperties({ def, id, onChange }: AreaPropertiesProps) {
  const area = def.areas?.find((a) => a.id === id);
  if (!area) {
    return null;
  }
  return (
    <>
      <Section title="General">
        <Field label="Name" htmlFor="model-area-name">
          <DraftText
            key={id}
            id="model-area-name"
            label="Area name"
            multiline={false}
            value={area.name}
            onCommit={(name) => onChange(setArea(def, id, { name }))}
          />
        </Field>
        <Field label="Comment" htmlFor="model-area-comment">
          <DraftText
            key={id}
            id="model-area-comment"
            label="Area comment"
            value={area.comment ?? ''}
            onCommit={(comment) => onChange(setArea(def, id, { comment: comment || undefined }))}
          />
        </Field>
      </Section>
      <Section title="Format">
        <Field label="Fill color">
          <Palette
            slot="model-area-fill"
            value={area.fill ?? 'slate'}
            onPick={(fill) => onChange(setArea(def, id, { fill }))}
            transparency={area.fillTransparency}
            onTransparency={(fillTransparency) => onChange(setArea(def, id, { fillTransparency }))}
          />
        </Field>
        <Field label="Border color">
          <Palette
            slot="model-area-border"
            value={area.border ?? 'slate'}
            onPick={(border) => onChange(setArea(def, id, { border }))}
            transparency={area.borderTransparency}
            onTransparency={(borderTransparency) => onChange(setArea(def, id, { borderTransparency }))}
          />
        </Field>
      </Section>
    </>
  );
}
