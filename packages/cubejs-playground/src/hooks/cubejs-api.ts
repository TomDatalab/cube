import { useMemo } from 'react';

import { createCubeApi } from '../shared/cube-api';

export function useCubejsApi(apiUrl: string | null, token: string | null) {
  return useMemo(() => {
    if (!token || !apiUrl || token === 'undefined') {
      return null;
    }

    return createCubeApi(token, apiUrl);
  }, [apiUrl, token]);
}
