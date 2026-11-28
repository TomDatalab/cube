import { Check, Copy } from 'lucide-react';
import { useState } from 'react';

import { cn } from '@/lib/utils';

import type { Method } from './catalog';

const METHOD_CLASS: Record<Method, string> = {
  GET: 'tw:bg-sky-500/15 tw:text-sky-700 tw:dark:text-sky-300',
  POST: 'tw:bg-emerald-500/15 tw:text-emerald-700 tw:dark:text-emerald-300',
  PUT: 'tw:bg-amber-500/15 tw:text-amber-700 tw:dark:text-amber-300',
  PATCH: 'tw:bg-violet-500/15 tw:text-violet-700 tw:dark:text-violet-300',
  DELETE: 'tw:bg-rose-500/15 tw:text-rose-700 tw:dark:text-rose-300',
};

export function MethodBadge({ method, className }: { method: Method; className?: string }) {
  return (
    <span
      className={cn(
        'tw:inline-block tw:w-14 tw:shrink-0 tw:rounded tw:py-0.5 tw:text-center tw:font-mono tw:text-[11px] tw:font-semibold',
        METHOD_CLASS[method],
        className
      )}
    >
      {method}
    </span>
  );
}

/** Code in a box, with a copy button. */
export function CodeBlock({ code, maxHeight }: { code: string; maxHeight?: number }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="tw:relative">
      <pre
        className="tw:m-0 tw:overflow-auto tw:rounded-md tw:border tw:bg-muted tw:p-3 tw:pr-10 tw:font-mono tw:text-xs tw:whitespace-pre-wrap tw:break-all"
        style={maxHeight ? { maxHeight } : undefined}
      >
        {code}
      </pre>
      <button
        type="button"
        data-slot="copy-code"
        aria-label="Copy"
        className="tw:absolute tw:top-2 tw:right-2 tw:flex tw:size-6 tw:cursor-pointer tw:items-center tw:justify-center tw:rounded tw:text-muted-foreground tw:hover:bg-accent tw:hover:text-foreground"
        onClick={() => navigator.clipboard?.writeText(code).then(() => {
          setCopied(true);
          setTimeout(() => setCopied(false), 1500);
        }, () => {})}
      >
        {copied ? <Check className="tw:size-3.5" /> : <Copy className="tw:size-3.5" />}
      </button>
    </div>
  );
}
