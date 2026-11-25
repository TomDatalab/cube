// @vitest-environment jsdom
import { createElement, act } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { AdminApi, Attributes, User } from '../pages/Admin/api';
import { AttributesModal } from '../pages/Admin/AttributesModal';
import { useLatestCall } from '../pages/Admin/useLatestCall';

(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;

type Deferred<T> = { promise: Promise<T>; resolve: (value: T) => void };

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

const user = (id: string) => ({ id }) as User;

describe('AttributesModal', () => {
  let root: ReturnType<typeof createRoot>;
  let host: HTMLElement;

  beforeEach(() => {
    window.matchMedia = vi.fn().mockImplementation((query: string) => ({
      matches: false,
      media: query,
      addListener: vi.fn(),
      removeListener: vi.fn(),
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    })) as any;
    host = document.createElement('div');
    document.body.appendChild(host);
    root = createRoot(host);
  });

  afterEach(() => {
    act(() => root.unmount());
    document.body.innerHTML = '';
  });

  // The values shown as tags in the form.
  const tags = () => [...document.body.querySelectorAll('[role="dialog"] [data-slot="badge"]')]
    .map((tag) => tag.textContent);

  it('ignores the attributes of a user it no longer shows', async () => {
    const loads: Record<string, Deferred<Attributes>> = { ana: deferred(), bia: deferred() };
    const setUserAttributes = vi.fn(async (_: string, attributes: Attributes) => attributes);
    const api = {
      userAttributes: (id: string) => loads[id].promise,
      setUserAttributes,
    } as unknown as AdminApi;
    const render = (shown: User | null) => act(() => root.render(
      createElement(AttributesModal, { api, user: shown, onClose: () => {} })
    ));

    render(user('ana'));
    render(user('bia'));
    await act(async () => loads.bia.resolve({ region: ['Norte'] }));
    // ana's answer arrives last: it must not replace bia's.
    await act(async () => loads.ana.resolve({ region: ['Sul'] }));
    expect(tags()).toEqual(['Norte']);

    const save = [...document.body.querySelectorAll('[role="dialog"] [data-slot="dialog-footer"] button')]
      .find((button) => button.textContent === 'Save') as HTMLButtonElement;
    expect(save.disabled).toBe(false);
    await act(async () => save.click());
    await act(async () => {});
    expect(setUserAttributes).toHaveBeenCalledTimes(1);
    expect(setUserAttributes).toHaveBeenCalledWith('bia', { region: ['Norte'] });
  });

  it('stays unsaveable while the latest load has not answered', async () => {
    const loads = [deferred<Attributes>(), deferred<Attributes>()];
    let calls = 0;
    const api = {
      userAttributes: () => {
        calls += 1;
        return loads[calls - 1].promise;
      },
    } as unknown as AdminApi;
    const render = (shown: User | null) => act(() => root.render(
      createElement(AttributesModal, { api, user: shown, onClose: () => {} })
    ));

    render(user('ana'));
    render(null);
    render(user('bia'));
    expect(calls).toBe(2);
    // The first load answers late, for a user no longer shown.
    await act(async () => loads[0].resolve({ region: ['Sul'] }));
    const save = [...document.body.querySelectorAll('[role="dialog"] [data-slot="dialog-footer"] button')]
      .find((button) => button.textContent === 'Save') as HTMLButtonElement;
    expect(save.disabled).toBe(true);
    expect(tags()).toEqual([]);
  });
});

describe('useLatestCall', () => {
  it('tells only the latest call that it is current', () => {
    let begin!: () => () => boolean;
    function Probe() {
      begin = useLatestCall();
      return null;
    }
    const root = createRoot(document.createElement('div'));
    act(() => root.render(createElement(Probe)));

    const first = begin();
    expect(first()).toBe(true);
    const second = begin();
    expect(first()).toBe(false);
    expect(second()).toBe(true);
    act(() => root.unmount());
  });
});
