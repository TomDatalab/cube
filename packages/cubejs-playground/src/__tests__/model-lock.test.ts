// @vitest-environment jsdom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import {
  afterEach, beforeEach, describe, expect, it, vi,
} from 'vitest';

import type { LockState } from '../pages/Schema/api';
import { newToken, useEditLock } from '../pages/Schema/models/useEditLock';

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
  let onFreed: ReturnType<typeof vi.fn<() => void>>;
  let lock: Lock;

  function Probe({ enabled = true }: { enabled?: boolean }) {
    lock = useEditLock(api, 'm1', onLost, { enabled, onFreed });
    return null;
  }

  const mount = async (enabled = true) => {
    await act(async () => root.render(createElement(Probe, { enabled })));
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
    onFreed = vi.fn<() => void>();
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

  it('lost() after a renewal already reported the loss calls onLost once', async () => {
    await mount();
    api.lockModel.mockResolvedValue({ held: false, holder: 'bob' });
    await tick(30_000);
    expect(onLost).toHaveBeenCalledTimes(1);
    // A save on its way meanwhile answers 423.
    await act(async () => lock.lost('bob'));
    expect(onLost).toHaveBeenCalledTimes(1);
    expect(lock.status).toBe('readOnly');
  });

  it('a held answer to a renewal sent before lost() does not undo it', async () => {
    await mount();
    let answer: (s: LockState) => void = () => {};
    api.lockModel.mockImplementationOnce(() => new Promise<LockState>((resolve) => { answer = resolve; }));
    await tick(30_000);
    await act(async () => lock.lost('bob'));
    await act(async () => answer({ held: true, holder: 'me' }));
    expect(lock.status).toBe('readOnly');
    expect(lock.holder).toBe('bob');
    expect(onLost).toHaveBeenCalledTimes(1);
  });

  it('a renewal answered after take() does not report a loss', async () => {
    api.lockModel.mockResolvedValue({ held: false, holder: 'bob' });
    await mount();
    let answer: (s: LockState) => void = () => {};
    api.lockModel.mockImplementationOnce(() => new Promise<LockState>((resolve) => { answer = resolve; }));
    await tick(30_000);
    await act(async () => lock.take());
    await act(async () => answer({ held: false, holder: 'bob' }));
    expect(lock.status).toBe('editing');
    expect(onLost).not.toHaveBeenCalled();
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
  it('a first claim that fails is an error, retried after 5 s and then 10 s', async () => {
    api.lockModel.mockRejectedValue(new Error('offline'));
    await mount();
    expect(lock.status).toBe('error');
    await tick(4_999);
    expect(api.lockModel).toHaveBeenCalledTimes(1);
    await tick(1);
    expect(api.lockModel).toHaveBeenCalledTimes(2);
    expect(lock.status).toBe('error');
    await tick(9_999);
    expect(api.lockModel).toHaveBeenCalledTimes(2);
    api.lockModel.mockResolvedValue({ held: true, holder: 'me' });
    await tick(1);
    expect(api.lockModel).toHaveBeenCalledTimes(3);
    expect(lock.status).toBe('editing');
    expect(onFreed).not.toHaveBeenCalled();
  });

  it('retry() claims at once after a failed first claim', async () => {
    api.lockModel.mockRejectedValueOnce(new Error('offline'));
    await mount();
    expect(lock.status).toBe('error');
    await act(async () => lock.retry());
    expect(api.lockModel).toHaveBeenCalledTimes(2);
    expect(lock.status).toBe('editing');
  });

  it('a failed renewal after an answer is not an error', async () => {
    api.lockModel.mockResolvedValueOnce({ held: false, holder: 'bob' });
    await mount();
    api.lockModel.mockRejectedValue(new Error('offline'));
    await tick(30_000);
    expect(lock.status).toBe('readOnly');
  });

  it('calls onFreed when a renewal claims a lock that was freed, not on take()', async () => {
    api.lockModel.mockResolvedValue({ held: false, holder: 'bob' });
    await mount();
    api.lockModel.mockResolvedValue({ held: true, holder: 'me' });
    await tick(30_000);
    expect(lock.status).toBe('editing');
    expect(onFreed).toHaveBeenCalledTimes(1);
    await tick(30_000);
    expect(onFreed).toHaveBeenCalledTimes(1);

    api.lockModel.mockResolvedValue({ held: false, holder: 'bob' });
    await tick(30_000);
    await act(async () => lock.take());
    expect(onFreed).toHaveBeenCalledTimes(1);
  });

  it('claims nothing while disabled, and releases when disabled', async () => {
    await mount(false);
    await tick(60_000);
    expect(api.lockModel).not.toHaveBeenCalled();
    expect(lock.status).toBe('pending');
    await mount(true);
    expect(api.lockModel).toHaveBeenCalledTimes(1);
    expect(lock.status).toBe('editing');
    await mount(false);
    expect(api.releaseModelLock).toHaveBeenCalledWith('m1', lock.token);
    await tick(60_000);
    expect(api.lockModel).toHaveBeenCalledTimes(1);
  });
});

describe('newToken', () => {
  it('makes a v4 UUID without crypto.randomUUID (plain HTTP)', () => {
    const real = crypto.randomUUID;
    Object.defineProperty(crypto, 'randomUUID', { value: undefined, configurable: true });
    try {
      const a = newToken();
      expect(a).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
      expect(newToken()).not.toBe(a);
    } finally {
      Object.defineProperty(crypto, 'randomUUID', { value: real, configurable: true });
    }
  });
});
