import { useCallback, useRef } from 'react';

// `begin()` starts a call and returns whether it is still the latest one:
// an answer to an earlier call arrives too late to be used.
export function useLatestCall(): () => () => boolean {
  const latest = useRef(0);
  return useCallback(() => {
    latest.current += 1;
    const call = latest.current;
    return () => latest.current === call;
  }, []);
}
