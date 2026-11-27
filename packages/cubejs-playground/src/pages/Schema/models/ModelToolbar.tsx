import { ReactElement, ReactNode, useState } from 'react';
import {
  AlignCenterVertical, AlignVerticalDistributeCenter, BookOpen, Box, ClipboardPaste, Copy, DatabaseZap,
  FileCode, FileText, Forward, Image, Layers, Maximize, Network, PanelLeft, PanelRight, PanelTop,
  Redo2, Save, Scaling, Scissors, Spline, Trash2, Undo2, ZoomIn, ZoomOut,
} from 'lucide-react';

import { Button } from '@/components/ui/button';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import { Separator } from '@/components/ui/separator';
import {
  Tooltip, TooltipContent, TooltipProvider, TooltipTrigger,
} from '@/components/ui/tooltip';

import type { AlignMode, DistributeMode } from './arrange';

export const ZOOM_CHOICES = [25, 50, 75, 100, 200, 400];
const SOON = 'Coming soon';

export type ModelToolbarProps = {
  history: { canUndo: boolean; canRedo: boolean; onUndo(): void; onRedo(): void };
  file: {
    dirty: boolean;
    busy: boolean;
    hasDrafts: boolean;
    canBuild: boolean;
    // A Postgres or MySQL connection: Generate SQL and Create in database.
    canSql: boolean;
    onSql(): void;
    onSave(): void;
    onCreate(): void;
    onBuild(): void;
  };
  edit: {
    canCopy: boolean;
    canPaste: boolean;
    canDelete: boolean;
    onCopy(): void;
    onPaste(): void;
    onPasteShortcut(): void;
    onCut(): void;
    onDelete(): void;
  };
  arrange: {
    // How many tables are selected.
    count: number;
    onSize(kind: 'fit' | 'width' | 'height'): void;
    onAlign(mode: AlignMode): void;
    onDistribute(mode: DistributeMode): void;
    onArrange(): void;
    // Order applies to notes and areas only.
    order: { canOrder: boolean; onFront(): void; onBack(): void };
  };
  references: {
    edgeSelected: boolean;
    anyHidden: boolean;
    onShowAll(): void;
    onAddPoint(): void;
    onReducePoint(): void;
    onSwap(): void;
  };
  zoom: {
    percent: number;
    onZoomIn(): void;
    onZoomOut(): void;
    onOneToOne(): void;
    onZoomTo(percent: number): void;
    onFit(): void;
  };
  panes: {
    left: boolean;
    right: boolean;
    top: boolean;
    onToggleLeft(): void;
    onToggleRight(): void;
    onToggleTop(): void;
  };
  search: ReactNode;
  // Someone else edits the model: what changes it is off; exports, SQL,
  // copy, zoom, panes and search stay.
  readOnly?: boolean;
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
  label, hint, disabled, pressed, onClick, children,
}: {
  label: string;
  hint?: string;
  disabled?: boolean;
  pressed?: boolean;
  onClick?(): void;
  children: ReactNode;
}) {
  return (
    <Hint text={hint ?? label}>
      <Button
        type="button"
        variant="ghost"
        size="icon-sm"
        aria-label={label}
        aria-pressed={pressed}
        disabled={disabled}
        onClick={onClick}
      >
        {children}
      </Button>
    </Hint>
  );
}

const Divider = () => <Separator orientation="vertical" className="tw:mx-1 tw:data-[orientation=vertical]:h-5" />;

type Item = { label: string; aria?: string; disabled?: boolean; onClick(): void } | 'separator';

// A toolbar button that opens a list of actions, as Redgate's arrange menus.
function Menu({
  label, disabled, items, children,
}: { label: string; disabled: boolean; items: Item[]; children: ReactNode }) {
  const [open, setOpen] = useState(false);
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <Hint text={label}>
        <PopoverTrigger asChild>
          <Button type="button" variant="ghost" size="icon-sm" aria-label={label} disabled={disabled}>
            {children}
          </Button>
        </PopoverTrigger>
      </Hint>
      <PopoverContent className="tw:w-auto tw:min-w-44 tw:p-1" align="start">
        <div role="menu" aria-label={label} className="tw:flex tw:flex-col">
          {items.map((item, i) => (item === 'separator' ? (
            // Separators have no identity but their place.
            // eslint-disable-next-line react/no-array-index-key
            <Separator key={`sep-${i}`} className="tw:my-1" />
          ) : (
            <button
              key={item.aria ?? item.label}
              type="button"
              role="menuitem"
              aria-label={item.aria ?? item.label}
              disabled={item.disabled}
              className="tw:cursor-pointer tw:rounded-sm tw:border-0 tw:bg-transparent tw:px-2 tw:py-1.5 tw:text-left tw:text-sm tw:text-popover-foreground tw:hover:bg-accent tw:disabled:cursor-default tw:disabled:opacity-50"
              onClick={() => {
                setOpen(false);
                item.onClick();
              }}
            >
              {item.label}
            </button>
          )))}
        </div>
      </PopoverContent>
    </Popover>
  );
}

function createHint(file: ModelToolbarProps['file']): string {
  if (!file.canSql) {
    return 'Only for Postgres and MySQL connections';
  }
  return file.hasDrafts ? 'Create the new tables in the database' : 'No new table to create';
}

// Redgate's toolbar: history | file | clipboard | arrange | zoom | panes,
// then the search on the right.
export function ModelToolbar({
  history, file, edit, arrange, references, zoom, panes, search, readOnly = false,
}: ModelToolbarProps) {
  const zoomOptions = ZOOM_CHOICES.includes(zoom.percent)
    ? ZOOM_CHOICES
    : [...ZOOM_CHOICES, zoom.percent].sort((a, b) => a - b);
  return (
    <TooltipProvider>
      <div
        role="toolbar"
        aria-label="Model tools"
        data-slot="model-toolbar"
        className="tw:flex tw:flex-wrap tw:items-center tw:gap-0.5 tw:border-b tw:bg-card tw:px-2 tw:py-1"
      >
        <Tool label="Undo" hint="Undo (Ctrl + Z)" disabled={readOnly || !history.canUndo} onClick={history.onUndo}><Undo2 /></Tool>
        <Tool label="Redo" hint="Redo (Ctrl + Y)" disabled={readOnly || !history.canRedo} onClick={history.onRedo}><Redo2 /></Tool>
        <Divider />

        <Tool label="Save changes" hint="Save changes (Ctrl + S)" disabled={readOnly || !file.dirty || file.busy} onClick={file.onSave}>
          <Save />
        </Tool>
        <Tool label="Export model to image" hint={SOON} disabled><Image /></Tool>
        <Tool label="Export model to PDF file" hint={SOON} disabled><FileText /></Tool>
        <Tool
          label="Generate SQL script"
          hint={file.canSql ? 'Generate SQL script (Ctrl + G)' : 'Only for Postgres and MySQL connections'}
          disabled={!file.canSql}
          onClick={file.onSql}
        >
          <FileCode />
        </Tool>
        <Tool label="Generate model documentation" hint={SOON} disabled><BookOpen /></Tool>
        <Tool
          label="Create in database"
          hint={createHint(file)}
          disabled={readOnly || !file.canSql || !file.hasDrafts || file.busy}
          onClick={file.onCreate}
        >
          <DatabaseZap />
        </Tool>
        <Tool label="Build cube" disabled={readOnly || !file.canBuild || file.busy} onClick={file.onBuild}><Box /></Tool>
        <Divider />

        <Tool label="Copy" hint="Copy (Ctrl + C)" disabled={!edit.canCopy} onClick={edit.onCopy}><Copy /></Tool>
        <Tool label="Paste" hint="Paste (Ctrl + V)" disabled={readOnly || !edit.canPaste} onClick={edit.onPaste}>
          <ClipboardPaste />
        </Tool>
        <Tool label="Paste as shortcut" disabled={readOnly || !edit.canPaste} onClick={edit.onPasteShortcut}><Forward /></Tool>
        <Tool label="Cut" hint="Cut (Ctrl + X)" disabled={readOnly || !edit.canCopy} onClick={edit.onCut}><Scissors /></Tool>
        <Tool label="Delete" hint="Delete (Del)" disabled={readOnly || !edit.canDelete} onClick={edit.onDelete}><Trash2 /></Tool>
        <Divider />

        <Menu
          label="Size"
          disabled={readOnly || arrange.count < 1}
          items={[
            { label: 'Adjust to text', onClick: () => arrange.onSize('fit') },
            { label: 'Same width', disabled: arrange.count < 2, onClick: () => arrange.onSize('width') },
            { label: 'Same height', disabled: arrange.count < 2, onClick: () => arrange.onSize('height') },
          ]}
        >
          <Scaling />
        </Menu>
        <Menu
          label="Align"
          disabled={readOnly || arrange.count < 2}
          items={[
            { label: 'Top', aria: 'Align top', onClick: () => arrange.onAlign('top') },
            { label: 'Center', aria: 'Align middle', onClick: () => arrange.onAlign('middle') },
            { label: 'Bottom', aria: 'Align bottom', onClick: () => arrange.onAlign('bottom') },
            'separator',
            { label: 'Left', aria: 'Align left', onClick: () => arrange.onAlign('left') },
            { label: 'Center', aria: 'Align center', onClick: () => arrange.onAlign('center') },
            { label: 'Right', aria: 'Align right', onClick: () => arrange.onAlign('right') },
          ]}
        >
          <AlignCenterVertical />
        </Menu>
        <Menu
          label="Distribution"
          disabled={readOnly || arrange.count < 3}
          items={[
            { label: 'Justify', aria: 'Distribute vertical justify', onClick: () => arrange.onDistribute('v-justify') },
            { label: 'Top', aria: 'Distribute top', onClick: () => arrange.onDistribute('top') },
            { label: 'Center', aria: 'Distribute middle', onClick: () => arrange.onDistribute('middle') },
            { label: 'Bottom', aria: 'Distribute bottom', onClick: () => arrange.onDistribute('bottom') },
            'separator',
            { label: 'Justify', aria: 'Distribute horizontal justify', onClick: () => arrange.onDistribute('h-justify') },
            { label: 'Left', aria: 'Distribute left', onClick: () => arrange.onDistribute('left') },
            { label: 'Center', aria: 'Distribute center', onClick: () => arrange.onDistribute('center') },
            { label: 'Right', aria: 'Distribute right', onClick: () => arrange.onDistribute('right') },
          ]}
        >
          <AlignVerticalDistributeCenter />
        </Menu>
        <Menu
          label="Order"
          disabled={readOnly || !arrange.order.canOrder}
          items={[
            { label: 'Bring to front', aria: 'Bring to front', onClick: arrange.order.onFront },
            { label: 'Send to back', aria: 'Send to back', onClick: arrange.order.onBack },
          ]}
        >
          <Layers />
        </Menu>
        <Menu
          label="References"
          disabled={readOnly || (!references.edgeSelected && !references.anyHidden)}
          items={[
            { label: 'Show all references', disabled: !references.anyHidden, onClick: references.onShowAll },
            { label: 'Add reference point', disabled: !references.edgeSelected, onClick: references.onAddPoint },
            { label: 'Reduce reference point', disabled: !references.edgeSelected, onClick: references.onReducePoint },
            { label: 'Swap reference orientation', disabled: !references.edgeSelected, onClick: references.onSwap },
          ]}
        >
          <Spline />
        </Menu>
        <Tool
          label="Arrange"
          hint="Arrange the tables in layers, starting from the first cube's source table"
          disabled={readOnly}
          onClick={arrange.onArrange}
        >
          <Network />
        </Tool>
        <Divider />

        <Tool label="Zoom out" onClick={zoom.onZoomOut}><ZoomOut /></Tool>
        <Tool label="1:1" hint="Zoom to 100%" onClick={zoom.onOneToOne}>
          <span className="tw:text-[10px] tw:font-semibold">1:1</span>
        </Tool>
        <Tool label="Zoom in" onClick={zoom.onZoomIn}><ZoomIn /></Tool>
        <select
          aria-label="Zoom"
          value={String(zoom.percent)}
          className="tw:h-7 tw:rounded-md tw:border tw:bg-background tw:px-1 tw:text-xs tw:text-foreground"
          onChange={(e) => zoom.onZoomTo(Number(e.target.value))}
        >
          {zoomOptions.map((p) => <option key={p} value={String(p)}>{`${p}%`}</option>)}
        </select>
        <Tool label="Fit view" onClick={zoom.onFit}><Maximize /></Tool>
        <Divider />

        <Tool
          label="Toggle navigation pane"
          hint="Toggle navigation pane (Ctrl + Alt + N)"
          pressed={panes.left}
          onClick={panes.onToggleLeft}
        >
          <PanelLeft />
        </Tool>
        <Tool
          label="Toggle properties pane"
          hint="Toggle properties pane (Ctrl + Alt + P)"
          pressed={panes.right}
          onClick={panes.onToggleRight}
        >
          <PanelRight />
        </Tool>
        <Tool label="Toggle top bar" hint="Toggle top bar (Ctrl + Alt + B)" pressed={panes.top} onClick={panes.onToggleTop}>
          <PanelTop />
        </Tool>

        <div className="tw:ml-auto">{search}</div>
      </div>
    </TooltipProvider>
  );
}
