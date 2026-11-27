// @vitest-environment jsdom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import {
  afterEach, beforeEach, describe, expect, it, vi,
} from 'vitest';

import type { LockState } from '../pages/Schema/api';
import { useEditLock } from '../pages/Schema/models/useEditLock';

(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;

type Lock = ReturnType<typeof useEditLock>;

describe('useEditLock', () => {
  let root: ReturnType<typeof createRoot>;
  let api: {
    lockModel: ReturnType<typeof vi.fn<(id: string, token: string) => Promise<LockState>>>;
    takeModelLock: ReturnType<typeof vi.fn<(id: string, token: string) => Promise<LockState>>>;
    releaseModelLock: ReturnType<typeof vi.fn<(id: string, token: string) => Promise<void>>>;
  };
  let onLost: ReturnType<typeof vi.fn<(holder: string) => void>>;
  let lock: Lock;

  function Probe() {
    lock = useEditLock(api, 'm1', onLost);
    return null;
  }

  const mount = async () => {
    await act(async () => root.render(createElement(Probe)));
  };
  // Lets the renewal's answer land.
  const tick = async (ms: number) => {
    await act(async () => {
      vi.advanceTimersByTime(ms);
    });
  };

  beforeEach(() => {
    vi.useFakeTimers();
    api = {
      lockModel: vi.fn(async () => ({ held: true, holder: 'me' })),
      takeModelLock: vi.fn(async () => ({ held: true, holder: 'me' })),
      releaseModelLock: vi.fn(async () => undefined),
    };
    onLost = vi.fn<(holder: string) => void>();
    root = createRoot(document.createElement('div'));
  });

  afterEach(() => {
    act(() => root.unmount());
    vi.useRealTimers();
  });

  it('claims on mount and renews every 30 s', async () => {
    await mount();
    expect(api.lockModel).toHaveBeenCalledTimes(1);
    expect(api.lockModel).toHaveBeenCalledWith('m1', lock.token);
    expect(lock.status).toBe('editing');
    await tick(29_999);
    expect(api.lockModel).toHaveBeenCalledTimes(1);
    await tick(1);
    expect(api.lockModel).toHaveBeenCalledTimes(2);
    expect(api.lockModel.mock.calls[1]).toEqual(['m1', lock.token]);
  });

  it('goes read-only when another tab holds the lock', async () => {
    api.lockModel.mockResolvedValue({ held: false, holder: 'bob' });
    await mount();
    expect(lock.status).toBe('readOnly');
    expect(lock.holder).toBe('bob');
    await tick(30_000);
    expect(onLost).not.toHaveBeenCalled();
  });

  it('calls onLost once when control is taken', async () => {
    await mount();
    expect(lock.status).toBe('editing');
    api.lockModel.mockResolvedValue({ held: false, holder: 'bob' });
    await tick(30_000);
    expect(lock.status).toBe('readOnly');
    expect(lock.holder).toBe('bob');
    expect(onLost).toHaveBeenCalledTimes(1);
    expect(onLost).toHaveBeenCalledWith('bob');
    await tick(30_000);
    expect(onLost).toHaveBeenCalledTimes(1);
  });

  it('keeps the status when a renewal fails', async () => {
    await mount();
    api.lockModel.mockRejectedValue(new Error('offline'));
    await tick(30_000);
    expect(lock.status).toBe('editing');
    expect(onLost).not.toHaveBeenCalled();
  });

  it('lost() turns read-only and calls onLost', async () => {
    await mount();
    await act(async () => lock.lost('bob'));
    expect(lock.status).toBe('readOnly');
    expect(lock.holder).toBe('bob');
    expect(onLost).toHaveBeenCalledWith('bob');
  });

  it('take() claims through takeModelLock', async () => {
    api.lockModel.mockResolvedValue({ held: false, holder: 'bob' });
    await mount();
    expect(lock.status).toBe('readOnly');
    await act(async () => lock.take());
    expect(api.takeModelLock).toHaveBeenCalledWith('m1', lock.token);
    expect(lock.status).toBe('editing');
  });

  it('releases on unmount', async () => {
    await mount();
    const { token } = lock;
    act(() => root.unmount());
    expect(api.releaseModelLock).toHaveBeenCalledWith('m1', token);
    // afterEach unmounts again.
    root = createRoot(document.createElement('div'));
  });

  it('releases on pagehide, and a failed release does not throw', async () => {
    api.releaseModelLock.mockRejectedValue(new Error('offline'));
    await mount();
    await act(async () => window.dispatchEvent(new Event('pagehide')));
    expect(api.releaseModelLock).toHaveBeenCalledWith('m1', lock.token);
  });
});
