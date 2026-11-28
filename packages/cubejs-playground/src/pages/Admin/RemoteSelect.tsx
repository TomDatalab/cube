import { CSSProperties, useEffect, useRef, useState } from 'react';
import { Check, ChevronsUpDown, X } from 'lucide-react';

import { Button } from '@/components/ui/button';
import {
  Command, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList,
} from '@/components/ui/command';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import { Spinner } from '@/components/kit';
import { cn } from '@/lib/utils';

import type { AdminApi } from './api';

type Option = { value: string; label: string };

type Props = {
  value?: string;
  // The label of the chosen option comes second, to name it in a message.
  onChange?: (v?: string, label?: string) => void;
  fetchOptions: (q: string) => Promise<Option[]>;
  placeholder?: string;
  allowClear?: boolean;
  style?: CSSProperties;
  className?: string;
  size?: 'small' | 'middle';
  // The label of `value` when it may not be among the first options.
  valueLabel?: string;
  id?: string;
  invalid?: boolean;
};

const DEBOUNCE_MS = 300;

// A search-as-you-type select: it asks `fetchOptions` for the first matches
// 300 ms after the last key, and remembers the label of the chosen value.
export function RemoteSelect({
  value, onChange, fetchOptions, placeholder, allowClear, style, className, size, valueLabel, id, invalid,
}: Props) {
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState('');
  const [options, setOptions] = useState<Option[]>([]);
  const [loading, setLoading] = useState(false);
  const [labels, setLabels] = useState<Record<string, string>>({});
  const fetchRef = useRef(fetchOptions);
  fetchRef.current = fetchOptions;
  const latest = useRef(0);
  const timer = useRef<ReturnType<typeof setTimeout>>();

  function run(term: string) {
    const request = ++latest.current;
    setLoading(true);
    fetchRef.current(term).then((loaded) => {
      if (request === latest.current) {
        setOptions(loaded);
        setLoading(false);
      }
    }, () => {
      if (request === latest.current) {
        setOptions([]);
        setLoading(false);
      }
    });
  }

  useEffect(() => {
    run('');
    return () => {
      latest.current++;
      clearTimeout(timer.current);
    };
  }, []);

  const label = value ? (labels[value] ?? valueLabel) : undefined;
  const shown = value && label && !options.some((o) => o.value === value)
    ? [{ value, label }, ...options]
    : options;
  const shownLabel = value ? (shown.find((o) => o.value === value)?.label ?? label ?? value) : undefined;

  function choose(v: string | undefined) {
    if (v) {
      const chosen = shown.find((o) => o.value === v);
      if (chosen) {
        setLabels((l) => ({ ...l, [v]: chosen.label }));
      }
    }
    onChange?.(v, v ? (shown.find((o) => o.value === v)?.label ?? labels[v]) : undefined);
    openChange(false);
  }

  function openChange(next: boolean) {
    setOpen(next);
    // Back to the first matches once the search box is closed.
    if (!next && q) {
      clearTimeout(timer.current);
      setQ('');
      run('');
    }
  }

  return (
    <Popover open={open} onOpenChange={openChange}>
      <div className={cn('tw:relative tw:w-full', className)} style={style}>
        <PopoverTrigger asChild>
          <Button
            id={id}
            type="button"
            variant="outline"
            aria-invalid={invalid || undefined}
            size={size === 'small' ? 'sm' : 'default'}
            className={cn('tw:w-full tw:justify-between tw:font-normal', allowClear && value && 'tw:pr-14')}
          >
            <span className={cn('tw:truncate', !shownLabel && 'tw:text-muted-foreground')}>
              {shownLabel ?? placeholder ?? 'Select'}
            </span>
            <ChevronsUpDown className="tw:size-4 tw:opacity-50" />
          </Button>
        </PopoverTrigger>
        {allowClear && value ? (
          <button
            type="button"
            data-slot="select-clear"
            aria-label="Clear"
            className="tw:absolute tw:inset-y-0 tw:right-7 tw:flex tw:w-6 tw:cursor-pointer tw:items-center tw:justify-center tw:text-muted-foreground tw:hover:text-foreground"
            onClick={() => choose(undefined)}
          >
            <X className="tw:size-4" />
          </button>
        ) : null}
      </div>
      <PopoverContent className="tw:w-(--radix-popover-trigger-width) tw:min-w-[220px] tw:p-0" align="start">
        <Command shouldFilter={false}>
          <CommandInput
            placeholder="Search…"
            value={q}
            onValueChange={(term) => {
              setQ(term);
              clearTimeout(timer.current);
              timer.current = setTimeout(() => run(term), DEBOUNCE_MS);
            }}
          />
          <CommandList>
            {loading ? (
              <div className="tw:flex tw:justify-center tw:py-4"><Spinner /></div>
            ) : (
              <>
                <CommandEmpty>No matches</CommandEmpty>
                <CommandGroup>
                  {shown.map((o) => (
                    <CommandItem key={o.value} value={o.value} onSelect={() => choose(o.value)}>
                      <Check className={cn('tw:size-4', o.value === value ? 'tw:opacity-100' : 'tw:opacity-0')} />
                      {o.label}
                    </CommandItem>
                  ))}
                </CommandGroup>
              </>
            )}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}

export const userOptions = (api: AdminApi) => async (q: string): Promise<Option[]> => (
  (await api.searchUsers(q, 1)).items
    .filter((u) => !u.disabled)
    .map((u) => ({ value: u.id, label: u.name ? `${u.username} (${u.name})` : u.username }))
);

export const organizationOptions = (api: AdminApi) => async (q: string): Promise<Option[]> => (
  (await api.searchOrganizations(q, 1)).items.map((o) => ({ value: o.id, label: o.name }))
);

export const workspaceOptions = (api: AdminApi) => async (q: string): Promise<Option[]> => (
  (await api.searchWorkspaces({ q, page: 1 })).items.map((w) => ({
    value: w.id,
    label: `${w.organizationName} / ${w.name}`,
  }))
);
