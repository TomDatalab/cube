import {
  KeyboardEvent, ReactNode, useEffect, useState,
} from 'react';
import { Minus, Plus } from 'lucide-react';

import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { cn } from '@/lib/utils';

// A section of the properties pane that opens and closes with + and −, as
// Redgate's.
export function Section({
  title, slot, defaultOpen = true, children,
}: { title: string; slot?: string; defaultOpen?: boolean; children: ReactNode }) {
  const [open, setOpen] = useState(defaultOpen);
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
      {open ? <div className="tw:flex tw:flex-col tw:gap-3 tw:px-3 tw:py-3">{children}</div> : null}
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
