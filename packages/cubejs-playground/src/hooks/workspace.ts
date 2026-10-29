import { useSyncExternalStore } from 'react';

import { getWorkspace, onWorkspaceChange } from '../shared/auth';

// The chosen workspace, re-rendering when it changes without a reload.
export function useWorkspace(): string | null {
  return useSyncExternalStore(onWorkspaceChange, getWorkspace);
}
