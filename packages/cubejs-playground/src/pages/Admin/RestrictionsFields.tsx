import { useId, useRef, useState } from 'react';
import {
  Check, ChevronsUpDown, MinusCircle, Plus, X,
} from 'lucide-react';

import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import {
  Command, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList,
} from '@/components/ui/command';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import { RadioGroup, RadioGroupItem } from '@/components/ui/radio-group';
import { SimpleSelect, TagsInput } from '@/components/kit';
import { cn } from '@/lib/utils';

import {
  ATTRIBUTE_NAME, ModelCube, Restrictions, RowFilter,
} from './api';

// A row filter as the form edits it: `source` picks which of `attribute` and
// `values` is kept on save.
export type FilterField = {
  member?: string;
  operator?: RowFilter['operator'];
  source?: 'attribute' | 'values';
  attribute?: string;
  values?: string[];
};

export type RestrictionsFormValues = { rowFilters?: FilterField[]; blockedMembers?: string[] };

export function toFormRestrictions(r: Restrictions): RestrictionsFormValues {
  return {
    rowFilters: r.rowFilters.map((f) => ({
      member: f.member,
      operator: f.operator,
      source: f.attribute != null ? 'attribute' : 'values',
      attribute: f.attribute,
      values: f.values ?? [],
    })),
    blockedMembers: r.blockedMembers,
  };
}

// Sends exactly `attribute` OR `values` per filter, as its source says.
export function toRestrictions(v: RestrictionsFormValues | undefined): Restrictions {
  return {
    rowFilters: (v?.rowFilters || []).map((f): RowFilter => (f.source === 'attribute'
      ? { member: f.member!, operator: f.operator!, attribute: (f.attribute || '').trim() }
      : { member: f.member!, operator: f.operator!, values: f.values || [] })),
    blockedMembers: v?.blockedMembers || [],
  };
}

// What is wrong with one row filter, per field.
export type FilterErrors = { member?: string; attribute?: string; values?: string };

// The errors of each row filter; only the field its source shows is checked.
export function restrictionErrors(v: RestrictionsFormValues | undefined): FilterErrors[] {
  return (v?.rowFilters || []).map((f) => {
    const errors: FilterErrors = {};
    if (!f.member) {
      errors.member = 'Pick a dimension';
    }
    if (f.source === 'attribute') {
      if (!f.attribute) {
        errors.attribute = 'Name the attribute';
      } else if (!ATTRIBUTE_NAME.test(f.attribute)) {
        errors.attribute = 'a-z, 0-9 and _, up to 64';
      }
    } else if (!f.values?.length) {
      errors.values = 'Add at least one value';
    }
    return errors;
  });
}

export const hasRestrictionErrors = (errors: FilterErrors[]) => errors.some((e) => e.member || e.attribute || e.values);

type Props = {
  cubes: ModelCube[];
  value: RestrictionsFormValues;
  onChange: (value: RestrictionsFormValues) => void;
  // Shown after a submit; a changed field clears its own.
  errors?: FilterErrors[];
  onErrorsChange?: (errors: FilterErrors[]) => void;
};

const OPERATORS = [
  { value: 'equals', label: 'equals' },
  { value: 'notEquals', label: 'not equals' },
];

const SOURCES = [['attribute', 'User attribute'], ['values', 'Fixed values']] as const;

type MemberGroup = { key: string; label: string; members: string[] };

function ErrorText({ children }: { children?: string }) {
  return children ? <p className="tw:mt-1 tw:text-sm tw:text-destructive">{children}</p> : null;
}

// A searchable pick among the members of the cubes, grouped by cube: one
// member, or (`multiple`) several, shown as chips.
function MemberPicker({
  groups, value, onChange, multiple, placeholder, ariaLabel, invalid,
}: {
  groups: MemberGroup[];
  value: string[];
  onChange: (value: string[]) => void;
  multiple?: boolean;
  placeholder: string;
  ariaLabel: string;
  invalid?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const listId = useId();

  function choose(member: string) {
    if (multiple) {
      onChange(value.includes(member) ? value.filter((m) => m !== member) : [...value, member]);
    } else {
      onChange([member]);
      setOpen(false);
    }
  }

  const empty = multiple || !value.length;
  const trigger = (
    <PopoverTrigger asChild>
      <Button
        type="button"
        variant={multiple ? 'ghost' : 'outline'}
        size={multiple ? 'sm' : 'default'}
        role="combobox"
        aria-expanded={open}
        aria-controls={listId}
        aria-label={ariaLabel}
        aria-invalid={invalid || undefined}
        className={cn('tw:justify-between tw:font-normal', multiple ? 'tw:min-w-[180px] tw:flex-1' : 'tw:w-full')}
      >
        <span className={cn('tw:truncate', empty && 'tw:text-muted-foreground')}>
          {empty ? placeholder : value[0]}
        </span>
        <ChevronsUpDown className="tw:size-4 tw:opacity-50" />
      </Button>
    </PopoverTrigger>
  );

  return (
    <Popover open={open} onOpenChange={setOpen}>
      {multiple ? (
        <div className="tw:flex tw:min-h-9 tw:w-full tw:flex-wrap tw:items-center tw:gap-1 tw:rounded-md tw:border tw:border-input tw:p-1">
          {value.map((m) => (
            <Badge key={m} variant="secondary" className="tw:gap-1 tw:font-normal">
              {m}
              <button
                type="button"
                data-slot="tag-remove"
                aria-label={`Remove ${m}`}
                className="tw:cursor-pointer tw:text-muted-foreground tw:hover:text-foreground"
                onClick={() => onChange(value.filter((x) => x !== m))}
              >
                <X className="tw:size-3" />
              </button>
            </Badge>
          ))}
          {trigger}
        </div>
      ) : trigger}
      <PopoverContent
        id={listId}
        className="tw:w-(--radix-popover-trigger-width) tw:min-w-[240px] tw:p-0"
        align="start"
      >
        <Command>
          <CommandInput placeholder="Search…" />
          <CommandList>
            <CommandEmpty>No matches</CommandEmpty>
            {groups.map((g) => (
              <CommandGroup key={g.key} heading={g.label}>
                {g.members.map((m) => (
                  <CommandItem key={m} value={m} onSelect={() => choose(m)}>
                    <Check className={cn('tw:size-4', value.includes(m) ? 'tw:opacity-100' : 'tw:opacity-0')} />
                    {m}
                  </CommandItem>
                ))}
              </CommandGroup>
            ))}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}

// The "Row filters" and "Blocked columns" sections of the role form, editing
// `rowFilters` and `blockedMembers` of `value`.
export function RestrictionsFields({
  cubes, value, onChange, errors = [], onErrorsChange,
}: Props) {
  const filters = value.rowFilters || [];
  // Stable keys for the filters, so removing one keeps the others' state.
  const keys = useRef<number[]>([]);
  const nextKey = useRef(0);
  while (keys.current.length < filters.length) {
    keys.current.push(nextKey.current++);
  }
  keys.current.length = filters.length;

  const dimensionGroups = cubes.map((cube) => ({
    key: cube.name,
    label: cube.title || cube.name,
    members: cube.dimensions.map((d) => d.name),
  }));
  const memberGroups = cubes.map((cube) => ({
    key: cube.name,
    label: cube.title || cube.name,
    members: [...cube.measures, ...cube.dimensions, ...cube.segments].map((m) => m.name),
  }));

  function setFilters(rowFilters: FilterField[]) {
    onChange({ ...value, rowFilters });
  }

  function update(index: number, patch: FilterField) {
    setFilters(filters.map((f, i) => (i === index ? { ...f, ...patch } : f)));
    if (errors[index]) {
      const cleared = { ...errors[index] };
      Object.keys(patch).forEach((k) => {
        delete cleared[k as keyof FilterErrors];
      });
      onErrorsChange?.(errors.map((e, i) => (i === index ? cleared : e)));
    }
  }

  function remove(index: number) {
    keys.current.splice(index, 1);
    setFilters(filters.filter((_, i) => i !== index));
    onErrorsChange?.(errors.filter((_, i) => i !== index));
  }

  return (
    <>
      <h5 className="tw:mb-2 tw:text-sm tw:font-semibold">Row filters</h5>
      {filters.map((filter, index) => {
        const key = keys.current[index];
        const err = errors[index] || {};
        return (
          <div key={key} className="tw:mb-2 tw:rounded-md tw:border tw:p-2">
            <div className="tw:flex tw:flex-wrap tw:items-start tw:gap-2">
              <div className="tw:w-[200px]">
                <MemberPicker
                  groups={dimensionGroups}
                  value={filter.member ? [filter.member] : []}
                  onChange={(v) => update(index, { member: v[0] })}
                  placeholder="Member"
                  ariaLabel="Row filter member"
                  invalid={Boolean(err.member)}
                />
                <ErrorText>{err.member}</ErrorText>
              </div>
              <div className="tw:w-[140px]">
                <Label htmlFor={`filter-${key}-operator`} className="tw:sr-only">Row filter operator</Label>
                <SimpleSelect
                  id={`filter-${key}-operator`}
                  value={filter.operator}
                  onChange={(v) => update(index, { operator: v as RowFilter['operator'] })}
                  options={OPERATORS}
                />
              </div>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                aria-label="Remove filter"
                onClick={() => remove(index)}
              >
                <MinusCircle />
              </Button>
            </div>
            <RadioGroup
              className="tw:mt-2 tw:mb-1 tw:flex tw:gap-4"
              value={filter.source}
              onValueChange={(v) => update(index, { source: v as FilterField['source'] })}
            >
              {SOURCES.map(([v, label]) => (
                <div key={v} className="tw:flex tw:items-center tw:gap-2">
                  <RadioGroupItem value={v} id={`filter-${key}-${v}`} />
                  <Label htmlFor={`filter-${key}-${v}`} className="tw:font-normal">{label}</Label>
                </div>
              ))}
            </RadioGroup>
            {filter.source === 'attribute' ? (
              <div>
                <Input
                  placeholder="Attribute name, e.g. region"
                  maxLength={64}
                  autoComplete="off"
                  value={filter.attribute ?? ''}
                  aria-invalid={Boolean(err.attribute)}
                  onChange={(e) => update(index, { attribute: e.target.value })}
                />
                <ErrorText>{err.attribute}</ErrorText>
              </div>
            ) : (
              <div>
                <TagsInput
                  placeholder="Values"
                  value={filter.values}
                  onChange={(v) => update(index, { values: v })}
                />
                <ErrorText>{err.values}</ErrorText>
              </div>
            )}
          </div>
        );
      })}
      <Button
        type="button"
        variant="outline"
        className="tw:border-dashed"
        onClick={() => setFilters([...filters, { operator: 'equals', source: 'attribute' }])}
      >
        <Plus />
        Add filter
      </Button>

      <h5 className="tw:mt-4 tw:mb-2 tw:text-sm tw:font-semibold">Blocked columns</h5>
      <MemberPicker
        multiple
        groups={memberGroups}
        value={value.blockedMembers || []}
        onChange={(blockedMembers) => onChange({ ...value, blockedMembers })}
        placeholder="Pick members to block"
        ariaLabel="Blocked members"
      />
    </>
  );
}
