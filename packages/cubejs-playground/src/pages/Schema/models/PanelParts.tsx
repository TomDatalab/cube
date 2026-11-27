import {
  createContext, KeyboardEvent, ReactNode, useContext, useEffect, useState,
} from 'react';
import { Ban, Minus, Plus } from 'lucide-react';

import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { cn } from '@/lib/utils';

import { TABLE_COLORS, TableColor } from './modelGraph';
import { MAX_TRANSPARENCY } from './paint';

// While someone else edits the model, the pane only shows: every field of a
// section is off, but its header still opens and closes it.
export const ReadOnlyContext = createContext(false);

// A section of the properties pane that opens and closes with + and −, as
// Redgate's. `action` sits above the fields and stays on when read-only
// (it only shows something, as SQL preview).
export function Section({
  title, slot, defaultOpen = true, action, children,
}: { title: string; slot?: string; defaultOpen?: boolean; action?: ReactNode; children: ReactNode }) {
  const [open, setOpen] = useState(defaultOpen);
  const readOnly = useContext(ReadOnlyContext);
  return (
    <section className="tw:border-b" data-slot={slot}>
      <button
        type="button"
        aria-expanded={open}
        className="tw:flex tw:w-full tw:cursor-pointer tw:items-center tw:justify-between tw:border-0 tw:bg-muted/50 tw:px-3 tw:py-2 tw:text-left tw:text-sm tw:font-semibold tw:text-foreground"
        onClick={() => setOpen((was) => !was)}
      >
        {title}
        {open ? <Minus className="tw:size-4" /> : <Plus className="tw:size-4" />}
      </button>
      {open ? (
        <div className="tw:flex tw:flex-col tw:gap-3 tw:px-3 tw:py-3">
          {action}
          <fieldset disabled={readOnly} className="tw:contents">
            {children}
          </fieldset>
        </div>
      ) : null}
    </section>
  );
}

export function Value({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="tw:flex tw:flex-col tw:gap-0.5 tw:text-sm">
      <span className="tw:text-xs tw:text-muted-foreground">{label}</span>
      <span className="tw:font-medium">{children}</span>
    </div>
  );
}

// A text kept as typed and written to the model when the field is left (or
// on Enter, for one line), so typing it is one step of undo.
export function DraftText({
  id, label, value, onCommit, multiline = true, className, list, placeholder,
}: {
  id?: string;
  label: string;
  value: string;
  onCommit(next: string): void;
  multiline?: boolean;
  className?: string;
  list?: string;
  placeholder?: string;
}) {
  const [text, setText] = useState(value);
  useEffect(() => setText(value), [value]);
  const commit = () => {
    if (text !== value) {
      onCommit(text);
    }
  };
  if (multiline) {
    return (
      <Textarea
        id={id}
        aria-label={label}
        rows={2}
        value={text}
        className={className}
        placeholder={placeholder}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
      />
    );
  }
  return (
    <Input
      id={id}
      aria-label={label}
      value={text}
      list={list}
      placeholder={placeholder}
      className={cn('tw:h-7 tw:px-2 tw:text-xs', className)}
      onChange={(e) => setText(e.target.value)}
      onBlur={commit}
      onKeyDown={(e: KeyboardEvent<HTMLInputElement>) => {
        if (e.key === 'Enter') {
          commit();
        }
      }}
    />
  );
}

// A small check box with its label, for the pane's grids.
export function Check({
  label, checked, onChange, children,
}: { label: string; checked: boolean; onChange(next: boolean): void; children?: ReactNode }) {
  return (
    <span className="tw:flex tw:items-center tw:gap-1.5 tw:text-xs">
      <input type="checkbox" aria-label={label} checked={checked} onChange={(e) => onChange(e.target.checked)} />
      {children}
    </span>
  );
}

// How see-through a color is: a slider saved when let go (one undo step),
// 0% saved as nothing.
function Transparency({
  slot, value, disabled, onCommit,
}: { slot: string; value: number | undefined; disabled: boolean; onCommit(next: number | undefined): void }) {
  const [draft, setDraft] = useState(value ?? 0);
  useEffect(() => setDraft(value ?? 0), [value]);
  const commit = () => {
    if (draft !== (value ?? 0)) {
      onCommit(draft === 0 ? undefined : draft);
    }
  };
  return (
    <div
      data-slot={`${slot}-transparency`}
      className={cn('tw:flex tw:w-full tw:items-center tw:gap-2 tw:text-xs tw:text-muted-foreground', disabled && 'tw:opacity-50')}
    >
      <span aria-hidden>Transparency</span>
      <input
        type="range"
        aria-label="Transparency"
        min={0}
        max={MAX_TRANSPARENCY}
        step={5}
        value={draft}
        disabled={disabled}
        className="tw:min-w-0 tw:flex-1"
        onChange={(e) => setDraft(Number(e.target.value))}
        onPointerUp={commit}
        onKeyUp={commit}
        onBlur={commit}
      />
      <span className="tw:w-8 tw:text-right tw:tabular-nums">{`${draft}%`}</span>
    </div>
  );
}

// The palette swatches of the table's Format, for one color of an object;
// `none` adds a button that clears it, `onTransparency` a slider for how
// see-through the color is.
export function Palette({
  slot, value, onPick, none, transparency, onTransparency,
}: {
  slot: string;
  value: TableColor | undefined;
  onPick(color: TableColor | undefined): void;
  none?: boolean;
  transparency?: number;
  onTransparency?(next: number | undefined): void;
}) {
  return (
    <div className="tw:flex tw:flex-wrap tw:items-center tw:gap-1" data-slot={slot}>
      {TABLE_COLORS.map((color) => (
        <button
          key={color}
          type="button"
          aria-label={color}
          aria-pressed={value === color}
          title={color}
          className="tw:size-6 tw:cursor-pointer tw:rounded-full tw:border-2 tw:border-transparent tw:p-0 tw:aria-pressed:border-foreground"
          style={{ background: `var(--model-${color})` }}
          onClick={() => onPick(color)}
        />
      ))}
      {none ? (
        <button
          type="button"
          aria-label="none"
          aria-pressed={value === undefined}
          title="none"
          className="tw:flex tw:size-6 tw:cursor-pointer tw:items-center tw:justify-center tw:rounded-full tw:border tw:bg-transparent tw:p-0 tw:text-muted-foreground tw:hover:bg-accent"
          onClick={() => onPick(undefined)}
        >
          <Ban className="tw:size-3.5" />
        </button>
      ) : null}
      {onTransparency ? (
        <Transparency slot={slot} value={transparency} disabled={value === undefined} onCommit={onTransparency} />
      ) : null}
    </div>
  );
}
