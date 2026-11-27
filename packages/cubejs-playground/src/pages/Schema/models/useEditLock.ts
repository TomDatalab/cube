import {
  useCallback, useEffect, useRef, useState,
} from 'react';

import type { LockState, modelApi } from '../api';

type LockApi = Pick<ReturnType<typeof modelApi>, 'lockModel' | 'takeModelLock' | 'releaseModelLock'>;

// The server forgets a lock 120 s after its last renewal.
export const RENEW_MS = 30_000;

export type EditLock = {
  // This editor's token, sent with its writes.
  token: string;
  // `pending` until the first claim answers; nothing is edited meanwhile.
  status: 'pending' | 'editing' | 'readOnly';
  holder: string | null;
  take(): Promise<void>;
  // A write the server refused with 423: someone else holds the lock.
  lost(holder: string): void;
};

// One editor at a time: claims the model's lock on mount, renews it, and
// releases it when the editor goes away or the page is left.
export function useEditLock(api: LockApi, modelId: string, onLost: (holder: string) => void): EditLock {
  const [token] = useState(() => crypto.randomUUID());
  const [state, setState] = useState<Pick<EditLock, 'status' | 'holder'>>({ status: 'pending', holder: null });
  // Whether the last answer gave us the lock: losing it is news only then.
  const held = useRef(false);
  const latest = useRef({ api, onLost });
  latest.current = { api, onLost };

  const apply = useCallback((answer: LockState) => {
    if (answer.held) {
      held.current = true;
      setState({ status: 'editing', holder: answer.holder });
      return;
    }
    const had = held.current;
    held.current = false;
    setState({ status: 'readOnly', holder: answer.holder });
    if (had) {
      latest.current.onLost(answer.holder ?? '');
    }
  }, []);

  useEffect(() => {
    let alive = true;
    held.current = false;
    setState({ status: 'pending', holder: null });
    // A renewal that does not reach the server keeps what we knew.
    const claim = () => latest.current.api.lockModel(modelId, token).then(
      (answer) => alive && apply(answer),
      () => {}
    );
    const release = () => {
      try {
        Promise.resolve(latest.current.api.releaseModelLock(modelId, token)).catch(() => {});
      } catch {
        // Leaving goes on; the lock expires by itself.
      }
    };
    claim();
    const timer = window.setInterval(claim, RENEW_MS);
    window.addEventListener('pagehide', release);
    return () => {
      alive = false;
      window.clearInterval(timer);
      window.removeEventListener('pagehide', release);
      release();
    };
  }, [modelId, token, apply]);

  const take = useCallback(async () => {
    apply(await latest.current.api.takeModelLock(modelId, token));
  }, [modelId, token, apply]);

  const lost = useCallback((holder: string) => {
    held.current = false;
    setState({ status: 'readOnly', holder });
    latest.current.onLost(holder);
  }, []);

  return {
    token, status: state.status, holder: state.holder, take, lost,
  };
}
