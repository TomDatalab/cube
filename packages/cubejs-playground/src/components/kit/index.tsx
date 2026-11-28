// Small building blocks on shadcn/ui for the screens that leave antd: the
// shapes those screens used (a table with pages, a confirm popover, a form
// dialog, notifications), so moving a screen keeps its behavior.
//
// Classes carry Tailwind's `tw:` prefix (src/shadcn.css).

import { ReactElement, ReactNode, useState } from 'react';
import { ChevronLeft, ChevronRight, Loader2, Search, X } from 'lucide-react';
import { toast } from 'sonner';

import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import {
  Select, SelectContent, SelectGroup, SelectItem, SelectLabel, SelectTrigger, SelectValue,
} from '@/components/ui/select';
import {
  Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
} from '@/components/ui/table';
import { cn } from '@/lib/utils';

/** Notifications, in place of antd's `message` (the Toaster is in App). */
export const notify = {
  success: (text: ReactNode) => toast.success(text),
  error: (text: ReactNode) => toast.error(text),
  info: (text: ReactNode) => toast.info(text),
};

export function Spinner({ className }: { className?: string }) {
  return <Loader2 aria-label="Loading" className={cn('tw:size-4 tw:animate-spin tw:text-muted-foreground', className)} />;
}

export function EmptyState({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <div className={cn('tw:py-8 tw:text-center tw:text-sm tw:text-muted-foreground', className)}>
      {children}
    </div>
  );
}

/** A label, the control and its error under it. */
export function Field({
  label, htmlFor, error, children, className,
}: {
  label: ReactNode;
  htmlFor?: string;
  error?: string | null;
  children: ReactNode;
  className?: string;
}) {
  return (
    <div className={cn('tw:flex tw:flex-col tw:gap-2', className)}>
      <Label htmlFor={htmlFor}>{label}</Label>
      {children}
      {error ? <p className="tw:text-sm tw:text-destructive">{error}</p> : null}
    </div>
  );
}

/**
 * Asks before an action, in a popover by its trigger (antd's Popconfirm).
 * `children` is the trigger; a disabled confirm leaves it as it is. The
 * popover stays open while an async `onConfirm` runs.
 */
export function Confirm({
  title, onConfirm, disabled, okText = 'OK', children,
}: {
  title: ReactNode;
  onConfirm: () => unknown;
  disabled?: boolean;
  okText?: string;
  children: ReactElement;
}) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  if (disabled) {
    return children;
  }
  return (
    <Popover open={open} onOpenChange={(next) => !busy && setOpen(next)}>
      <PopoverTrigger asChild>{children}</PopoverTrigger>
      <PopoverContent className="tw:w-80 tw:text-sm" align="start">
        <p className="tw:mb-3">{title}</p>
        <div className="tw:flex tw:justify-end tw:gap-2">
          <Button size="sm" variant="outline" disabled={busy} onClick={() => setOpen(false)}>Cancel</Button>
          <Button
            size="sm"
            variant="destructive"
            disabled={busy}
            onClick={async () => {
              setBusy(true);
              try {
                await onConfirm();
              } finally {
                setBusy(false);
                setOpen(false);
              }
            }}
          >
            {busy ? <Spinner className="tw:text-current" /> : null}
            {okText}
          </Button>
        </div>
      </PopoverContent>
    </Popover>
  );
}

/** A dialog with Cancel and an OK button (antd's Modal with onOk). */
export function FormDialog({
  open, title, description, onClose, onOk, okText = 'Save', saving, okDisabled, children, className, footer,
}: {
  open: boolean;
  title: ReactNode;
  description?: ReactNode;
  onClose: () => void;
  onOk?: () => void;
  okText?: string;
  saving?: boolean;
  okDisabled?: boolean;
  children: ReactNode;
  className?: string;
  // Replaces Cancel and OK.
  footer?: ReactNode;
}) {
  return (
    <Dialog open={open} onOpenChange={(next) => !next && onClose()}>
      <DialogContent className={cn('tw:max-h-[90vh] tw:overflow-y-auto tw:sm:max-w-lg', className)}>
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          {description ? <DialogDescription>{description}</DialogDescription> : null}
        </DialogHeader>
        {children}
        <DialogFooter>
          {footer ?? (
            <>
              <Button variant="outline" onClick={onClose}>Cancel</Button>
              {onOk ? (
                <Button onClick={onOk} disabled={saving || okDisabled}>
                  {saving ? <Spinner className="tw:text-current" /> : null}
                  {okText}
                </Button>
              ) : null}
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

const TAG_VARIANTS = { blue: 'default', red: 'destructive', default: 'secondary' } as const;

/** A small label (antd's Tag). */
export function Tag({
  children, color, title, className,
}: {
  children: ReactNode;
  color?: 'blue' | 'red' | 'default';
  title?: string;
  className?: string;
}) {
  return (
    <Badge
      variant={TAG_VARIANTS[color ?? 'default']}
      title={title}
      className={cn('tw:font-normal', className)}
    >
      {children}
    </Badge>
  );
}

/** A search box with an icon and a clear button (antd's Input.Search). */
export function SearchInput({
  value, onChange, placeholder, className,
}: {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  className?: string;
}) {
  return (
    <div className={cn('tw:relative tw:w-full tw:max-w-[400px]', className)}>
      <Search className="tw:pointer-events-none tw:absolute tw:top-1/2 tw:left-2.5 tw:size-4 tw:-translate-y-1/2 tw:text-muted-foreground" />
      <Input
        type="search"
        className="tw:pr-8 tw:pl-8"
        placeholder={placeholder}
        value={value}
        onChange={(e) => onChange(e.target.value)}
      />
      {value ? (
        <button
          type="button"
          data-slot="search-clear"
          aria-label="Clear"
          className="tw:absolute tw:inset-y-0 tw:right-0 tw:flex tw:w-8 tw:cursor-pointer tw:items-center tw:justify-center tw:text-muted-foreground tw:hover:text-foreground"
          onClick={() => onChange('')}
        >
          <X className="tw:size-4" />
        </button>
      ) : null}
    </div>
  );
}

export type Option = { value: string; label: ReactNode; group?: string; disabled?: boolean };

// Radix's Select has no empty value; this one stands for "nothing chosen".
const NONE = '__none__';

/**
 * A select over fixed options (antd's Select without search). Options with
 * a `group` are listed under it. `allowClear` adds an entry that clears it.
 */
export function SimpleSelect({
  value, onChange, options, placeholder, disabled, loading, allowClear, className, size, id,
}: {
  value?: string;
  onChange: (value: string | undefined) => void;
  options: Option[];
  placeholder?: string;
  disabled?: boolean;
  loading?: boolean;
  allowClear?: boolean;
  className?: string;
  size?: 'sm' | 'default';
  id?: string;
}) {
  const groups: { name?: string; options: Option[] }[] = [];
  options.forEach((o) => {
    const last = groups[groups.length - 1];
    if (last && last.name === o.group) {
      last.options.push(o);
    } else {
      groups.push({ name: o.group, options: [o] });
    }
  });
  return (
    <Select
      value={value ?? ''}
      onValueChange={(v) => onChange(v === NONE ? undefined : v)}
      disabled={disabled}
    >
      <SelectTrigger id={id} size={size} className={cn('tw:w-full', className)}>
        <SelectValue placeholder={loading ? 'Loading…' : placeholder} />
      </SelectTrigger>
      <SelectContent>
        {allowClear && value ? <SelectItem value={NONE}><span className="tw:text-muted-foreground">Clear</span></SelectItem> : null}
        {groups.map((g, i) => (
          <SelectGroup key={g.name ?? `group-${i}`}>
            {g.name ? <SelectLabel>{g.name}</SelectLabel> : null}
            {g.options.map((o) => (
              <SelectItem key={o.value} value={o.value} disabled={o.disabled}>{o.label}</SelectItem>
            ))}
          </SelectGroup>
        ))}
      </SelectContent>
    </Select>
  );
}

/** How many pages `total` rows fill, at least one. */
export const pageCount = (total: number, pageSize: number) => Math.max(1, Math.ceil(total / pageSize));

export function Pager({
  current, pageSize, total, onChange,
}: {
  current: number;
  pageSize: number;
  total: number;
  onChange: (page: number) => void;
}) {
  const pages = pageCount(total, pageSize);
  if (pages <= 1) {
    return null;
  }
  return (
    <div className="tw:flex tw:items-center tw:justify-end tw:gap-2 tw:pt-3 tw:text-sm tw:text-muted-foreground">
      <span>{`Page ${current} of ${pages} · ${total} total`}</span>
      <Button
        size="icon-sm"
        variant="outline"
        aria-label="Previous page"
        disabled={current <= 1}
        onClick={() => onChange(current - 1)}
      >
        <ChevronLeft />
      </Button>
      <Button
        size="icon-sm"
        variant="outline"
        aria-label="Next page"
        disabled={current >= pages}
        onClick={() => onChange(current + 1)}
      >
        <ChevronRight />
      </Button>
    </div>
  );
}

export type Column<T> = {
  key: string;
  title: ReactNode;
  // The row's field the cell shows (and `render` receives first).
  dataIndex?: keyof T;
  render?: (value: any, row: T, index: number) => ReactNode;
  width?: number | string;
  align?: 'left' | 'right' | 'center';
  className?: string;
};

const ALIGN = { left: undefined, right: 'tw:text-right', center: 'tw:text-center' } as const;

/**
 * A table (antd's Table, small): columns, a loading state, an empty text,
 * pages from the server and an optional selected row that clicks choose.
 */
export function DataTable<T>({
  rows, rowKey, columns, loading, emptyText = 'No data', pagination, selectedKey, onRowClick, className,
}: {
  rows: T[];
  rowKey: keyof T | ((row: T) => string);
  columns: Column<T>[];
  loading?: boolean;
  emptyText?: ReactNode;
  pagination?: { current: number; pageSize: number; total: number; onChange: (page: number) => void };
  selectedKey?: string | null;
  onRowClick?: (row: T) => void;
  className?: string;
}) {
  const keyOf = (row: T) => String(typeof rowKey === 'function' ? rowKey(row) : row[rowKey]);
  const alignClass = (align?: Column<T>['align']) => (align ? ALIGN[align] : undefined);
  return (
    <div className={cn('tw:relative', className)}>
      <div className="tw:rounded-md tw:border">
        <Table>
          <TableHeader>
            <TableRow>
              {columns.map((c) => (
                <TableHead
                  key={c.key}
                  style={c.width != null ? { width: c.width } : undefined}
                  className={cn(alignClass(c.align), c.className)}
                >
                  {c.title}
                </TableHead>
              ))}
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.length === 0 ? (
              <TableRow>
                <TableCell colSpan={columns.length}>
                  <EmptyState className="tw:py-6">{loading ? <Spinner className="tw:mx-auto" /> : emptyText}</EmptyState>
                </TableCell>
              </TableRow>
            ) : rows.map((row, index) => {
              const key = keyOf(row);
              return (
                <TableRow
                  key={key}
                  data-state={selectedKey === key ? 'selected' : undefined}
                  className={onRowClick ? 'tw:cursor-pointer' : undefined}
                  onClick={onRowClick ? () => onRowClick(row) : undefined}
                >
                  {columns.map((c) => {
                    const value = c.dataIndex != null ? row[c.dataIndex] : undefined;
                    return (
                      <TableCell key={c.key} className={cn('tw:whitespace-normal', alignClass(c.align), c.className)}>
                        {c.render ? c.render(value, row, index) : (value as ReactNode)}
                      </TableCell>
                    );
                  })}
                </TableRow>
              );
            })}
          </TableBody>
        </Table>
      </div>
      {loading && rows.length > 0 ? (
        <div className="tw:absolute tw:inset-0 tw:flex tw:items-center tw:justify-center tw:bg-background/50">
          <Spinner className="tw:size-5" />
        </div>
      ) : null}
      {pagination ? <Pager {...pagination} /> : null}
    </div>
  );
}

/** Free values typed and kept as chips (antd's Select mode="tags"). */
export function TagsInput({
  value = [], onChange, placeholder, id,
}: {
  value?: string[];
  onChange: (value: string[]) => void;
  placeholder?: string;
  id?: string;
}) {
  const [draft, setDraft] = useState('');
  const add = (text: string) => {
    const fresh = text.split(',').map((t) => t.trim()).filter((t) => t && !value.includes(t));
    if (fresh.length) {
      onChange([...value, ...fresh]);
    }
    setDraft('');
  };
  return (
    <div className="tw:flex tw:min-h-9 tw:w-full tw:flex-wrap tw:items-center tw:gap-1 tw:rounded-md tw:border tw:border-input tw:px-2 tw:py-1">
      {value.map((v) => (
        <Badge key={v} variant="secondary" className="tw:gap-1 tw:font-normal">
          {v}
          <button
            type="button"
            data-slot="tag-remove"
            aria-label={`Remove ${v}`}
            className="tw:cursor-pointer tw:text-muted-foreground tw:hover:text-foreground"
            onClick={() => onChange(value.filter((x) => x !== v))}
          >
            <X className="tw:size-3" />
          </button>
        </Badge>
      ))}
      <input
        id={id}
        data-slot="tags-input"
        className="tw:min-w-[80px] tw:flex-1 tw:bg-transparent tw:text-sm tw:outline-none"
        placeholder={value.length ? undefined : placeholder}
        value={draft}
        onChange={(e) => {
          if (e.target.value.includes(',')) {
            add(e.target.value);
          } else {
            setDraft(e.target.value);
          }
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            e.preventDefault();
            add(draft);
          } else if (e.key === 'Backspace' && !draft && value.length) {
            onChange(value.slice(0, -1));
          }
        }}
        onBlur={() => add(draft)}
      />
    </div>
  );
}
