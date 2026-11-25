import { PanelLeftClose, PanelLeftOpen } from 'lucide-react';

// The button that collapses a side panel to a strip, or opens it again.
export function PanelToggle({
  collapsed, onToggle, label,
}: {
  collapsed: boolean;
  onToggle: () => void;
  label: string;
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
      className="tw:flex tw:size-8 tw:shrink-0 tw:cursor-pointer tw:items-center tw:justify-center tw:rounded-md tw:border-0 tw:bg-transparent tw:p-0 tw:text-muted-foreground tw:hover:bg-accent tw:hover:text-foreground"
      onClick={onToggle}
    >
      <Icon className="tw:size-4" />
    </button>
  );
}
