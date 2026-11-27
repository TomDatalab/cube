import { CircleAlert, Lock } from 'lucide-react';

import { Button } from '@/components/ui/button';

// Above the canvas while someone else edits the model.
export function ReadOnlyBanner({ holder, onTake }: { holder: string | null; onTake(): void }) {
  return (
    <div
      role="status"
      data-slot="model-read-only"
      className="tw:flex tw:items-center tw:gap-2 tw:rounded-md tw:bg-muted tw:px-3 tw:py-1.5 tw:text-sm tw:text-muted-foreground"
    >
      <Lock aria-hidden className="tw:size-4 tw:shrink-0" />
      <span className="tw:min-w-0 tw:flex-1 tw:truncate">
        {`Read-only mode · The document is being edited by ${holder ?? 'someone else'}`}
      </span>
      <Button type="button" size="sm" variant="outline" onClick={onTake}>Take control</Button>
    </div>
  );
}

// Above the canvas while the edit lock could not be claimed: nothing is
// edited until a claim reaches the server.
export function LockErrorBanner({ onRetry }: { onRetry(): void }) {
  return (
    <div
      role="alert"
      data-slot="model-lock-error"
      className="tw:flex tw:items-center tw:gap-2 tw:rounded-md tw:bg-muted tw:px-3 tw:py-1.5 tw:text-sm tw:text-muted-foreground"
    >
      <CircleAlert aria-hidden className="tw:size-4 tw:shrink-0 tw:text-destructive" />
      <span className="tw:min-w-0 tw:flex-1 tw:truncate">Could not get edit control.</span>
      <Button type="button" size="sm" variant="outline" onClick={onRetry}>Retry</Button>
    </div>
  );
}
