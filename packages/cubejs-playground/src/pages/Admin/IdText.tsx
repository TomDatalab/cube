import { useState } from 'react';
import { Check, Copy } from 'lucide-react';

import {
  Tooltip, TooltipContent, TooltipProvider, TooltipTrigger,
} from '@/components/ui/tooltip';

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** How an id is shown: a UUID as its first 8 characters, anything else whole. */
export function shortId(id: string): string {
  return UUID.test(id) ? `${id.slice(0, 8)}…` : id;
}

/**
 * An id in a table: muted and short, so a UUID does not widen the column; the
 * whole id is in the tooltip and is what the copy button copies.
 */
export function IdText({ id }: { id: string }) {
  const short = shortId(id);
  const [copied, setCopied] = useState(false);
  const text = <span>{short}</span>;
  return (
    <span className="tw:inline-flex tw:items-center tw:gap-1 tw:whitespace-nowrap tw:text-muted-foreground">
      {short === id ? text : (
        <TooltipProvider delayDuration={300}>
          <Tooltip>
            <TooltipTrigger asChild>{text}</TooltipTrigger>
            <TooltipContent>{id}</TooltipContent>
          </Tooltip>
        </TooltipProvider>
      )}
      <button
        type="button"
        data-slot="copy-id"
        aria-label="Copy id"
        className="tw:cursor-pointer tw:text-muted-foreground tw:hover:text-foreground"
        onClick={(e) => {
          e.stopPropagation();
          navigator.clipboard?.writeText(id).then(() => {
            setCopied(true);
            setTimeout(() => setCopied(false), 1500);
          }, () => {});
        }}
      >
        {copied ? <Check className="tw:size-3.5" /> : <Copy className="tw:size-3.5" />}
      </button>
    </span>
  );
}
