import { RefObject, useRef, useState } from 'react';
import { Search } from 'lucide-react';

import type { Hit } from './search';

export type SearchBoxProps = {
  query: string;
  hits: Hit[];
  onQuery(q: string): void;
  onPick(hit: Hit): void;
  // Set by the editor so Ctrl+F can focus the box.
  inputRef?: RefObject<HTMLInputElement>;
};

// Redgate's search on the right of the toolbar: the hits drop under it.
export function SearchBox({
  query, hits, onQuery, onPick, inputRef,
}: SearchBoxProps) {
  const [open, setOpen] = useState(false);
  const own = useRef<HTMLInputElement>(null);
  const ref = inputRef ?? own;
  const pick = (hit: Hit) => {
    onPick(hit);
    setOpen(false);
  };
  return (
    <div className="tw:relative" data-slot="model-search">
      <div className="tw:flex tw:h-7 tw:items-center tw:gap-1 tw:rounded-md tw:border tw:bg-background tw:px-2">
        <Search aria-hidden className="tw:size-3.5 tw:text-muted-foreground" />
        <input
          ref={ref}
          aria-label="Search the model"
          placeholder="Search"
          value={query}
          className="tw:w-40 tw:border-0 tw:bg-transparent tw:text-xs tw:text-foreground tw:outline-none"
          onFocus={() => setOpen(true)}
          onBlur={() => window.setTimeout(() => setOpen(false), 150)}
          onChange={(e) => {
            onQuery(e.target.value);
            setOpen(true);
          }}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && hits[0]) {
              pick(hits[0]);
            } else if (e.key === 'Escape') {
              setOpen(false);
            }
          }}
        />
        <span className="tw:text-[10px] tw:whitespace-nowrap tw:text-muted-foreground">Ctrl + F</span>
      </div>
      {open && query.trim() ? (
        <ul
          role="listbox"
          aria-label="Search results"
          className="tw:absolute tw:right-0 tw:z-20 tw:mt-1 tw:max-h-72 tw:w-72 tw:list-none tw:overflow-y-auto tw:rounded-md tw:border tw:bg-popover tw:p-1 tw:text-popover-foreground tw:shadow-md"
        >
          {hits.length === 0 ? <li className="tw:px-2 tw:py-1 tw:text-xs tw:text-muted-foreground">Nothing found</li> : null}
          {hits.map((hit) => (
            <li key={`${hit.kind}:${hit.label}`}>
              <button
                type="button"
                data-slot="model-search-hit"
                className="tw:flex tw:w-full tw:cursor-pointer tw:items-center tw:gap-2 tw:rounded-sm tw:border-0 tw:bg-transparent tw:px-2 tw:py-1 tw:text-left tw:text-xs tw:text-inherit tw:hover:bg-accent"
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => pick(hit)}
              >
                <span className="tw:w-16 tw:shrink-0 tw:text-muted-foreground">{hit.kind}</span>
                <span className="tw:truncate">{hit.label}</span>
              </button>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}
