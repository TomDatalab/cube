import {
  Box, Cable, MousePointer2, Square, SquareDashedMousePointer, StickyNote, Table2, TableProperties,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';

import { Button } from '@/components/ui/button';
import {
  Tooltip, TooltipContent, TooltipProvider, TooltipTrigger,
} from '@/components/ui/tooltip';

export type Tool = 'select' | 'area-select' | 'table' | 'reference' | 'view' | 'note' | 'area' | 'cube';

// Redgate's tools, in its order, each on its number key.
export const TOOLS: {
  tool: Tool; label: string; hint: string; icon: LucideIcon; soon?: boolean;
}[] = [
  {
    tool: 'select', label: 'Select', hint: '(1) Select', icon: MousePointer2,
  },
  {
    tool: 'area-select', label: 'Select area', hint: '(2) Select area', icon: SquareDashedMousePointer,
  },
  {
    tool: 'table', label: 'New table', hint: '(3) Add new table', icon: Table2,
  },
  {
    tool: 'reference', label: 'New reference', hint: '(4) Add new reference', icon: Cable,
  },
  {
    tool: 'view', label: 'New view', hint: '(5) Add new view', icon: TableProperties,
  },
  {
    tool: 'note', label: 'New note', hint: '(6) Add new note', icon: StickyNote,
  },
  {
    tool: 'area', label: 'New area', hint: '(7) Add new area', icon: Square,
  },
  {
    tool: 'cube', label: 'New cube', hint: '(8) Add new cube', icon: Box, soon: true,
  },
];

// The row of tools floating on the canvas' top-left corner.
export function ToolPalette({ tool, onTool }: { tool: Tool; onTool(t: Tool): void }) {
  return (
    <TooltipProvider>
      <div
        role="toolbar"
        aria-label="Canvas tools"
        data-slot="model-tool-palette"
        className="tw:absolute tw:top-2 tw:left-2 tw:z-10 tw:flex tw:items-center tw:gap-0.5 tw:rounded-md tw:border tw:bg-card tw:p-0.5 tw:shadow-sm"
      >
        {TOOLS.map(({
          tool: t, label, hint, icon: Icon, soon,
        }) => (
          <Tooltip key={t}>
            <TooltipTrigger asChild>
              <span className="tw:inline-flex">
                <Button
                  type="button"
                  size="icon-sm"
                  variant={tool === t ? 'default' : 'ghost'}
                  aria-label={label}
                  aria-pressed={tool === t}
                  disabled={soon}
                  onClick={() => onTool(t)}
                >
                  <Icon />
                </Button>
              </span>
            </TooltipTrigger>
            <TooltipContent>{soon ? `${hint}: coming soon` : hint}</TooltipContent>
          </Tooltip>
        ))}
      </div>
    </TooltipProvider>
  );
}
