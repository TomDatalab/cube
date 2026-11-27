import {
  useCallback, useEffect, useRef, useState,
} from 'react';

import type { LockState, modelApi } from '../api';

type LockApi = Pick<ReturnType<typeof modelApi>, 'lockModel' | 'takeModelLock' | 'releaseModelLock'>;

// The server forgets a lock 120 s after its last renewal.
export const RENEW_MS = 30_000;
// A first claim that does not reach the server is tried again sooner, then
// with the renewals.
export const RETRY_MS = [5_000, 10_000];

export type EditLock = {
  // This editor's token, sent with its writes.
  token: string;
  // `pending` until the first claim answers; nothing is edited meanwhile.
  // `error` while no claim has reached the server.
  status: 'pending' | 'editing' | 'readOnly' | 'error';
  holder: string | null;
  take(): Promise<void>;
  // Claims again at once, after an error.
  retry(): Promise<void>;
  // A write the server refused with 423: someone else holds the lock.
  lost(holder: string): void;
};

export type EditLockOptions = {
  // Nothing is claimed while false (the model did not load).
  enabled?: boolean;
  // A renewal claimed the lock someone else had left.
  onFreed?(): void;
};

// A random v4 UUID. crypto.randomUUID exists only in secure contexts (HTTPS
// or localhost), and the Playground is also opened over plain HTTP on the
// LAN; getRandomValues works in both.
export function newToken(): string {
  if (typeof crypto.randomUUID === 'function') return crypto.randomUUID();
  const b = crypto.getRandomValues(new Uint8Array(16));
  b[6] = 0x40 + (b[6] % 16); // version 4
  b[8] = 0x80 + (b[8] % 64); // RFC 4122 variant
  const h = Array.from(b, (x) => x.toString(16).padStart(2, '0')).join('');
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}

// One editor at a time: claims the model's lock on mount, renews it, and
// releases it when the editor goes away or the page is left.
export function useEditLock(
  api: LockApi,
  modelId: string,
  onLost: (holder: string) => void,
  { enabled = true, onFreed }: EditLockOptions = {}
): EditLock {
  const [token] = useState(newToken);
  const [state, setState] = useState<Pick<EditLock, 'status' | 'holder'>>({ status: 'pending', holder: null });
  // Whether the last answer gave us the lock: losing it is news only then.
  const held = useRef(false);
  // Whether any claim has answered since the lock was first claimed.
  const answered = useRef(false);
  // Bumped by lost() and take(): an answer to a claim sent before them is
  // stale and is dropped.
  const epoch = useRef(0);
  const latest = useRef({ api, onLost, onFreed });
  latest.current = { api, onLost, onFreed };
  const claimRef = useRef<() => Promise<unknown>>(() => Promise.resolve());

  const apply = useCallback((answer: LockState, renewal = false) => {
    const wasReading = answered.current && !held.current;
    answered.current = true;
    if (answer.held) {
      held.current = true;
      setState({ status: 'editing', holder: answer.holder });
      if (renewal && wasReading) {
        latest.current.onFreed?.();
      }
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
    held.current = false;
    answered.current = false;
    epoch.current += 1;
    setState({ status: 'pending', holder: null });
    if (!enabled) {
      claimRef.current = () => Promise.resolve();
      return undefined;
    }
    let alive = true;
    let failures = 0;
    let retryTimer: number | undefined;
    // A renewal that does not reach the server keeps what we knew; a first
    // claim that does not is an error, tried again soon.
    const claim = () => {
      const sent = epoch.current;
      return latest.current.api.lockModel(modelId, token).then(
        (answer) => {
          if (alive && sent === epoch.current) {
            window.clearTimeout(retryTimer);
            apply(answer, true);
          }
        },
        () => {
          if (!alive || answered.current) {
            return;
          }
          setState({ status: 'error', holder: null });
          const wait = RETRY_MS[failures];
          failures += 1;
          window.clearTimeout(retryTimer);
          if (wait !== undefined) {
            retryTimer = window.setTimeout(claim, wait);
          }
        }
      );
    };
    claimRef.current = claim;
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
      claimRef.current = () => Promise.resolve();
      window.clearInterval(timer);
      window.clearTimeout(retryTimer);
      window.removeEventListener('pagehide', release);
      release();
    };
  }, [modelId, token, apply, enabled]);

  const take = useCallback(async () => {
    epoch.current += 1;
    const sent = epoch.current;
    const answer = await latest.current.api.takeModelLock(modelId, token);
    if (sent === epoch.current) {
      apply(answer);
    }
  }, [modelId, token, apply]);

  const retry = useCallback(async () => {
    await claimRef.current();
  }, []);

  // Reported once: a renewal may have told us already.
  const lost = useCallback((holder: string) => {
    epoch.current += 1;
    answered.current = true;
    const had = held.current;
    held.current = false;
    setState({ status: 'readOnly', holder });
    if (had) {
      latest.current.onLost(holder);
    }
  }, []);

  return {
    token, status: state.status, holder: state.holder, take, retry, lost,
  };
}
