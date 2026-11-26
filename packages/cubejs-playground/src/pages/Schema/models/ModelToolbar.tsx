import { ReactElement, ReactNode, useState } from 'react';
import {
  Ban, DatabaseZap, Maximize, Network, Palette, Pencil, Plus, Trash2, ZoomIn, ZoomOut,
} from 'lucide-react';

import { Button } from '@/components/ui/button';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import { Separator } from '@/components/ui/separator';
import {
  Tooltip, TooltipContent, TooltipProvider, TooltipTrigger,
} from '@/components/ui/tooltip';

import { TABLE_COLORS, TableColor } from './modelGraph';

export type ModelToolbarProps = {
  // The selected table: what Color, Edit and Remove act on.
  selected: { alias: string; draft: boolean; color?: TableColor } | null;
  hasDrafts: boolean;
  busy: boolean;
  onNewTable(): void;
  onColor(color: TableColor | undefined): void;
  onEdit(): void;
  onRemove(): void;
  onArrange(): void;
  onZoomIn(): void;
  onZoomOut(): void;
  onFitView(): void;
  onCreate(): void;
};

// The hint of a toolbar button. The trigger is a span so the hint also
// shows on a disabled button, which gets no pointer events.
function Hint({ text, children }: { text: string; children: ReactElement }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span className="tw:inline-flex">{children}</span>
      </TooltipTrigger>
      <TooltipContent>{text}</TooltipContent>
    </Tooltip>
  );
}

function Tool({
  label, hint, disabled, onClick, children,
}: {
  label: string;
  hint?: string;
  disabled?: boolean;
  onClick(): void;
  children: ReactNode;
}) {
  return (
    <Hint text={hint ?? label}>
      <Button type="button" variant="ghost" size="icon-sm" aria-label={label} disabled={disabled} onClick={onClick}>
        {children}
      </Button>
    </Hint>
  );
}

const Divider = () => <Separator orientation="vertical" className="tw:mx-1 tw:data-[orientation=vertical]:h-5" />;

// The 8 colors of the palette and "No color", for the selected table.
function ColorTool({ selected, onColor }: Pick<ModelToolbarProps, 'selected' | 'onColor'>) {
  const [open, setOpen] = useState(false);
  const pick = (color: TableColor | undefined) => {
    onColor(color);
    setOpen(false);
  };
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <Hint text="Table color">
        <PopoverTrigger asChild>
          <Button type="button" variant="ghost" size="icon-sm" aria-label="Table color" disabled={!selected}>
            <Palette />
          </Button>
        </PopoverTrigger>
      </Hint>
      <PopoverContent className="tw:w-auto tw:p-2" align="start">
        <div className="tw:flex tw:items-center tw:gap-1" data-slot="model-color-palette">
          {TABLE_COLORS.map((color) => (
            <button
              key={color}
              type="button"
              aria-label={color}
              aria-pressed={selected?.color === color}
              title={color}
              className="tw:size-6 tw:cursor-pointer tw:rounded-full tw:border-2 tw:border-transparent tw:p-0 tw:aria-pressed:border-foreground"
              style={{ background: `var(--model-${color})` }}
              onClick={() => pick(color)}
            />
          ))}
          <button
            type="button"
            aria-label="No color"
            title="No color"
            className="tw:flex tw:size-6 tw:cursor-pointer tw:items-center tw:justify-center tw:rounded-full tw:border tw:bg-transparent tw:p-0 tw:text-muted-foreground tw:hover:bg-accent"
            onClick={() => pick(undefined)}
          >
            <Ban className="tw:size-3.5" />
          </button>
        </div>
      </PopoverContent>
    </Popover>
  );
}

// The row of tools over the canvas.
export function ModelToolbar(props: ModelToolbarProps) {
  const { selected, hasDrafts, busy } = props;
  return (
    <TooltipProvider>
      <div
        role="toolbar"
        aria-label="Model tools"
        data-slot="model-toolbar"
        className="tw:flex tw:flex-wrap tw:items-center tw:gap-0.5 tw:rounded-md tw:border tw:bg-card tw:p-1"
      >
        <Tool label="New table" onClick={props.onNewTable}><Plus /></Tool>
        <ColorTool selected={selected} onColor={props.onColor} />
        <Tool
          label="Edit table"
          hint={selected && !selected.draft ? 'Only new tables are edited here' : 'Edit table'}
          disabled={!selected?.draft}
          onClick={props.onEdit}
        >
          <Pencil />
        </Tool>
        <Tool label="Remove table" disabled={!selected} onClick={props.onRemove}><Trash2 /></Tool>
        <Divider />
        <Tool label="Arrange" onClick={props.onArrange}><Network /></Tool>
        <Divider />
        <Tool label="Zoom in" onClick={props.onZoomIn}><ZoomIn /></Tool>
        <Tool label="Zoom out" onClick={props.onZoomOut}><ZoomOut /></Tool>
        <Tool label="Fit view" onClick={props.onFitView}><Maximize /></Tool>
        <Divider />
        <Tool
          label="Create in database"
          hint={hasDrafts ? 'Create the new tables in the database' : 'No new table to create'}
          disabled={!hasDrafts || busy}
          onClick={props.onCreate}
        >
          <DatabaseZap />
        </Tool>
      </div>
    </TooltipProvider>
  );
}
