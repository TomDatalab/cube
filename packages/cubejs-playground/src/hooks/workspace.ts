import { useSyncExternalStore } from 'react';

import { getOrganizationScope, getWorkspace, onWorkspaceChange } from '../shared/auth';

// The chosen workspace, re-rendering when it changes without a reload.
export function useWorkspace(): string | null {
  return useSyncExternalStore(onWorkspaceChange, getWorkspace);
}

// The chosen organization scope of an admin, the same way.
export function useOrganizationScope(): string | null {
  return useSyncExternalStore(onWorkspaceChange, getOrganizationScope);
}
