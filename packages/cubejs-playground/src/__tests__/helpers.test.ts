import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { setWorkspace } from '../shared/auth';
import {
  apiFetch, isRelativeUrl, isSameOrigin, playgroundFetch, setPlaygroundKey,
} from '../shared/helpers';

const inAnHour = () => new Date(Date.now() + 3600_000).toISOString();

function headersOf(fetchMock: ReturnType<typeof vi.fn>, call = 0): Record<string, string> {
  const init = fetchMock.mock.calls[call][1] as RequestInit;
  return Object.fromEntries(new Headers(init.headers).entries());
}

describe('which URLs are this server', () => {
  it('takes relative paths', () => {
    expect(isRelativeUrl('playground/files')).toBe(true);
    expect(isRelativeUrl('/cube/v1/meta')).toBe(true);
    expect(isRelativeUrl('./playground/context')).toBe(true);
  });

  it('refuses absolute and protocol-relative URLs', () => {
    expect(isRelativeUrl('https://evil.example/x')).toBe(false);
    expect(isRelativeUrl(`${window.location.origin}/playground/files`)).toBe(false);
    expect(isRelativeUrl('//evil.example/x')).toBe(false);
    expect(isRelativeUrl('/\\evil.example/x')).toBe(false);
    expect(isRelativeUrl(' \t//evil.example/x')).toBe(false);
    expect(isRelativeUrl('data:text/plain,x')).toBe(false);
    expect(isRelativeUrl('\u0000https://evil.example/x')).toBe(false);
  });

  it('compares origins', () => {
    expect(isSameOrigin('/cube/v1/load')).toBe(true);
    expect(isSameOrigin(`${window.location.origin}/cube/v1/load`)).toBe(true);
    expect(isSameOrigin('https://evil.example/cube/v1/load')).toBe(false);
  });
});

describe('playgroundFetch', () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    sessionStorage.clear();
    sessionStorage.setItem('cubePlaygroundSession', JSON.stringify({
      token: 'jwt',
      expiresAt: inAnHour(),
      user: { id: 'admin', name: 'Admin', platformAdmin: true },
    }));
    setPlaygroundKey('key');
    fetchMock = vi.fn(() => Promise.resolve(new Response('{}', { status: 200 })));
    vi.stubGlobal('fetch', fetchMock);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    setPlaygroundKey(null);
  });

  it('sends the token and the key to this server', async () => {
    await playgroundFetch('playground/files');

    expect(headersOf(fetchMock)).toMatchObject({ authorization: 'jwt', 'x-playground-key': 'key' });
  });

  it.each([
    'https://evil.example/playground/files',
    '//evil.example/playground/files',
    `${window.location.origin}/playground/files`,
  ])('sends neither to %s', async (url) => {
    await playgroundFetch(url, { headers: { Accept: 'application/json' } });

    expect(headersOf(fetchMock)).toEqual({ accept: 'application/json' });
  });

  it('retries with the original options', async () => {
    fetchMock
      .mockImplementationOnce(() => Promise.reject(new Error('Network request failed')))
      .mockImplementationOnce(() => Promise.resolve(new Response('{}', { status: 200 })));

    await playgroundFetch('playground/generate-schema', {
      method: 'POST',
      body: '{"tables":[]}',
      retries: 1,
    });

    expect(fetchMock).toHaveBeenCalledTimes(2);
    const retry = fetchMock.mock.calls[1][1] as RequestInit & { options?: unknown };
    expect(retry.method).toBe('POST');
    expect(retry.body).toBe('{"tables":[]}');
    expect(retry.options).toBeUndefined();
    expect(headersOf(fetchMock, 1)).toMatchObject({ authorization: 'jwt' });
  });
});

describe('apiFetch', () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    sessionStorage.clear();
    fetchMock = vi.fn(() => Promise.resolve(new Response('{}', { status: 200 })));
    vi.stubGlobal('fetch', fetchMock);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('scopes requests to this server to the chosen workspace', async () => {
    setWorkspace('ws-1');

    await apiFetch(`${window.location.origin}/cube/v1/load`, { headers: { Authorization: 't' } });

    expect(headersOf(fetchMock)).toEqual({ authorization: 't', 'x-cube-workspace': 'ws-1' });
  });

  it('sends no workspace without one, nor to another host', async () => {
    await apiFetch('/cube/v1/meta', { headers: { Authorization: 't' } });
    setWorkspace('ws-1');
    await apiFetch('https://evil.example/cube/v1/meta', { headers: { Authorization: 't' } });

    expect(headersOf(fetchMock, 0)).toEqual({ authorization: 't' });
    expect(headersOf(fetchMock, 1)).toEqual({ authorization: 't' });
  });

  it('ends the session for a 401 on its token, not on a token typed in', async () => {
    sessionStorage.setItem('cubePlaygroundSession', JSON.stringify({
      token: 'jwt',
      expiresAt: inAnHour(),
      user: { id: 'ana', name: 'Ana', platformAdmin: false },
    }));
    fetchMock.mockImplementation(() => Promise.resolve(new Response('{}', { status: 401 })));

    await apiFetch('/cube/v1/meta', { headers: { Authorization: 'typed-in' } });
    expect(sessionStorage.getItem('cubePlaygroundSession')).not.toBeNull();

    await apiFetch('/cube/v1/meta', { headers: { Authorization: 'jwt' } });
    expect(sessionStorage.getItem('cubePlaygroundSession')).toBeNull();
  });
});
