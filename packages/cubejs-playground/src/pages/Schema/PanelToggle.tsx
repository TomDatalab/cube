import { PanelLeftClose, PanelLeftOpen } from 'lucide-react';

import { cn } from '@/lib/utils';

// The button that collapses a side panel to a strip, or opens it again.
export function PanelToggle({
  collapsed, onToggle, label, className,
}: {
  collapsed: boolean;
  onToggle: () => void;
  label: string;
  className?: string;
}) {
  const text = `${collapsed ? 'Show' : 'Hide'} ${label}`;
  const Icon = collapsed ? PanelLeftOpen : PanelLeftClose;
  return (
    <button
      type="button"
      data-slot="panel-toggle"
      aria-label={text}
      aria-expanded={!collapsed}
      title={text}
      className={cn(
        'tw:flex tw:size-8 tw:shrink-0 tw:cursor-pointer tw:items-center tw:justify-center tw:rounded-md tw:border-0 tw:bg-transparent tw:p-0 tw:text-muted-foreground tw:hover:bg-accent tw:hover:text-foreground',
        className,
      )}
      onClick={onToggle}
    >
      <Icon className="tw:size-4" />
    </button>
  );
}
