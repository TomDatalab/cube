// @vitest-environment jsdom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { curlFor, requestUrl } from '../pages/Api/ApiConsole';
import { ApiPage, sdkSnippets } from '../pages/Api/ApiPage';

vi.mock('../hooks', () => ({
  usePlaygroundContext: () => ({ basePath: '/cube' }),
  useRbac: () => ({ enabled: true }),
  useToken: () => null,
}));

(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;

describe('the API console helpers', () => {
  it('sends relative paths under the API prefix, absolute ones at the root', () => {
    expect(requestUrl('http://h:4000', '/cube', '/v1/meta', false)).toBe('http://h:4000/cube/v1/meta');
    expect(requestUrl('http://h:4000', '/cube', '/readyz', true)).toBe('http://h:4000/readyz');
  });

  it('writes the curl without the token itself', () => {
    const curl = curlFor({
      method: 'POST',
      url: 'http://h/cube/v1/load',
      hasToken: true,
      scopeHeader: ['X-Cube-Workspace', 'w1'],
      body: '{\n  "query": "it\'s"\n}',
    });
    expect(curl).toBe([
      'curl -X POST',
      '  -H "Authorization: <token>"',
      '  -H "X-Cube-Workspace: w1"',
      '  -H "Content-Type: application/json"',
      `  -d '{ "query": "it'\\''s" }'`,
      "  'http://h/cube/v1/load'",
    ].join(' \\\n'));
  });

  it('points the client libraries at this server', () => {
    const all = sdkSnippets('http://h/cube').flatMap((t) => t.blocks.map(([, code]) => code)).join('\n');
    expect(all).toContain("apiUrl: 'http://h/cube/v1'");
    expect(all).not.toContain('localhost');
  });
});

describe('the API page', () => {
  let root: ReturnType<typeof createRoot>;
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    sessionStorage.clear();
    fetchMock = vi.fn(async () => new Response('{"ok":true}', { status: 200, statusText: 'OK' }));
    vi.stubGlobal('fetch', fetchMock);
    const host = document.createElement('div');
    document.body.appendChild(host);
    root = createRoot(host);
  });

  afterEach(() => {
    act(() => root.unmount());
    document.body.innerHTML = '';
    vi.unstubAllGlobals();
  });

  const endpoint = (path: string, method: string) => [...document.querySelectorAll('[data-slot="api-endpoint"]')]
    .find((b) => b.querySelector('code')!.textContent === path && b.textContent!.startsWith(method)) as HTMLButtonElement;
  const sendButton = () => [...document.querySelectorAll('[data-testid="api-console"] button')]
    .find((b) => /^(Send|Fill in)/.test(b.textContent!)) as HTMLButtonElement;
  const type = (el: HTMLInputElement, value: string) => {
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!;
    setter.call(el, value);
    el.dispatchEvent(new Event('input', { bubbles: true }));
  };

  it('lists every section and loads an endpoint into the console, waiting for its parameters', async () => {
    await act(async () => root.render(createElement(ApiPage)));
    const titles = [...document.querySelectorAll('main h2')].map((h) => h.textContent);
    expect(titles).toContain('Groups');
    expect(titles).toContain('Data model files');
    expect(titles.at(-1)).toBe('Client libraries');

    await act(async () => endpoint('/v1/groups/:id/members/:user', 'PUT').click());
    expect(sendButton().textContent).toBe('Fill in :id, :user');
    expect(sendButton().disabled).toBe(true);

    await act(async () => type(document.getElementById('param-id') as HTMLInputElement, 'g1'));
    await act(async () => type(document.getElementById('param-user') as HTMLInputElement, 'ana'));
    expect(sendButton().disabled).toBe(false);
    await act(async () => sendButton().click());

    const [url, init] = fetchMock.mock.calls[0] as any[];
    expect(url).toBe(`${window.location.origin}/cube/v1/groups/g1/members/ana`);
    expect(init.method).toBe('PUT');
    expect(document.querySelector('[data-testid="api-console-status"]')!.textContent).toBe('200 OK');
  });

  it('filters the endpoints by the search', async () => {
    await act(async () => root.render(createElement(ApiPage)));
    const input = document.querySelector('input[type="search"]') as HTMLInputElement;
    await act(async () => type(input, 'readyz'));
    const titles = [...document.querySelectorAll('main h2')].map((h) => h.textContent);
    expect(titles).toEqual(['System', 'Client libraries']);
    expect(document.querySelectorAll('[data-slot="api-endpoint"]')).toHaveLength(1);
  });
});
