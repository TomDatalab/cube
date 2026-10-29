// @vitest-environment jsdom
import { createElement, act } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { useSearch } from '../pages/Admin/useSearch';

(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;

describe('useSearch', () => {
  let hook: ReturnType<typeof useSearch<string>>;
  let root: ReturnType<typeof createRoot>;

  function mount(fetch: (q: string, page: number) => Promise<{ items: string[]; total: number }>) {
    function Probe() {
      hook = useSearch(fetch, []);
      return null;
    }
    root = createRoot(document.createElement('div'));
    act(() => root.render(createElement(Probe)));
  }

  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    act(() => root.unmount());
    vi.useRealTimers();
  });

  it('debounces q by 300 ms and goes back to page 1', async () => {
    const fetch = vi.fn(async (q: string, page: number) => ({ items: [`${q}:${page}`], total: 1 }));
    mount(fetch);
    await act(async () => {});
    expect(fetch).toHaveBeenCalledTimes(1);

    act(() => hook.setPage(3));
    await act(async () => {});
    expect(fetch).toHaveBeenLastCalledWith('', 3);

    act(() => hook.setQ('ana'));
    await act(async () => {
      vi.advanceTimersByTime(299);
    });
    expect(fetch).not.toHaveBeenCalledWith('ana', expect.anything());

    await act(async () => {
      vi.advanceTimersByTime(1);
    });
    expect(fetch).toHaveBeenLastCalledWith('ana', 1);
    expect(hook.page).toBe(1);
  });

  it('drops a response that arrives after a newer one', async () => {
    const resolvers: Array<(v: any) => void> = [];
    const fetch = vi.fn(() => new Promise<any>((resolve) => resolvers.push(resolve)));
    mount(fetch);
    act(() => hook.setQ('b'));
    await act(async () => {
      vi.advanceTimersByTime(300);
    });
    expect(resolvers).toHaveLength(2);

    await act(async () => resolvers[1]({ items: ['new'], total: 1 }));
    await act(async () => resolvers[0]({ items: ['old'], total: 1 }));
    expect(hook.result).toEqual({ items: ['new'], total: 1 });
  });

  it('steps back to the last page when its page empties', async () => {
    // 51 rows until the 51st is deleted: page 2 then comes back empty.
    let total = 51;
    const fetch = vi.fn(async (_q: string, page: number) => ({
      items: page === 1 ? ['a'] : Array(Math.max(0, total - 50)).fill('x'),
      total,
    }));
    mount(fetch);
    await act(async () => {});
    act(() => hook.setPage(2));
    await act(async () => {});
    expect(hook.result?.items).toHaveLength(1);

    total = 50;
    act(() => hook.reload());
    await act(async () => {});
    await act(async () => {});
    expect(hook.page).toBe(1);
    expect(fetch).toHaveBeenLastCalledWith('', 1);
  });

  it('exposes a failed search as error', async () => {
    mount(vi.fn(() => Promise.reject(new Error('forbidden'))));
    await act(async () => {});
    expect(hook.error).toBe('forbidden');
    expect(hook.loading).toBe(false);
  });
});
