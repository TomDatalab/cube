import { useMemo } from 'react';

import { createCubeApi } from '../shared/cube-api';
import { useWorkspace } from './workspace';

export function useCubejsApi(apiUrl: string | null, token: string | null) {
  // A new client when the workspace changes without a reload.
  const workspace = useWorkspace();

  return useMemo(() => {
    if (!token || !apiUrl || token === 'undefined') {
      return null;
    }

    return createCubeApi(token, apiUrl);
  }, [apiUrl, token, workspace]);
}
