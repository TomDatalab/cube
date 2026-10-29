import { authHeaders, onUnauthorized, workspaceHeaders } from './auth';

export function notEmpty<T>(value: T | null | undefined): value is T {
  return value != null;
}

export function ucfirst(s: string): string {
  return s[0].toUpperCase() + s.slice(1);
}

// `playground/files`, `/cube/v1/meta`: a path on this server. Not `https://…`,
// `mailto:…` or `//host/…` (nor `/\\host`, which browsers read the same way).
export function isRelativeUrl(url: string): boolean {
  // The URL parser drops leading blanks and control characters.
  let start = 0;
  while (start < url.length && url.charCodeAt(start) <= 0x20) {
    start++;
  }
  const trimmed = url.slice(start);
  return !/^[a-z][a-z0-9+.-]*:/i.test(trimmed) && !/^[\\/][\\/]/.test(trimmed);
}

// A URL on this page's origin, relative or not (the API URLs the Playground
// builds from `window.location`).
export function isSameOrigin(url: string): boolean {
  try {
    return new URL(url, window.location.href).origin === window.location.origin;
  } catch {
    return false;
  }
}

// The key `playground/context` hands the page; the server wants it on the
// routes that change the data model or the data sources.
let playgroundKey = '';

export function setPlaygroundKey(key?: string | null) {
  playgroundKey = key || '';
}

// With RBAC on, every Playground route also wants the login token.
export function playgroundKeyHeaders(): Record<string, string> {
  return {
    ...authHeaders(),
    ...(playgroundKey ? { 'X-Playground-Key': playgroundKey } : {}),
  };
}

// Headers are only added for this server: a token or key must never reach
// another host.
export function playgroundFetch(url: string, options: any = {}): Promise<Response> {
  const { retries = 0, ...restOptions } = options;
  const own = isRelativeUrl(url) && isSameOrigin(url);

  return fetch(url, {
    ...restOptions,
    headers: {
      ...(restOptions.headers || {}),
      ...(own ? { ...workspaceHeaders(), ...playgroundKeyHeaders() } : {}),
    },
  })
    .then(async (r) => {
      if (own) {
        onUnauthorized(r);
      }
      if (r.status === 500) {
        let errorText = await r.text();
        try {
          const json = JSON.parse(errorText);
          errorText = json.error;
        } catch {
          // Nothing
        }
        throw errorText;
      }
      return r;
    })
    .catch((e) => {
      if (e?.message === 'Network request failed' && retries > 0) {
        return playgroundFetch(url, { ...options, retries: retries - 1 });
      }
      throw e;
    });
}

// A call to the Cube API (REST, GraphQL, pre-aggregations) on this server:
// scoped to the chosen workspace, and a 401 ends the session. The caller
// brings its own Authorization (the Playground's token or one typed in).
export function apiFetch(url: RequestInfo | URL, options: RequestInit = {}): Promise<Response> {
  let href: string;
  if (typeof url === 'string') {
    href = url;
  } else if (url instanceof URL) {
    href = url.href;
  } else {
    href = url.url;
  }
  if (!isSameOrigin(href)) {
    return fetch(url, options);
  }

  const headers = new Headers(options.headers || {});
  Object.entries(workspaceHeaders()).forEach(([name, value]) => headers.set(name, value));

  return fetch(url, { ...options, headers }).then((r) => {
    onUnauthorized(r);
    return r;
  });
}

// A GET on this server with the login token (the RBAC API: organizations,
// workspaces). A 401 ends the session; the body comes back for any other
// status, with the server's `error` thrown for a failure (403 included).
export async function sessionFetchJson(url: string): Promise<any> {
  if (!isRelativeUrl(url) || !isSameOrigin(url)) {
    throw new Error(`Not a URL on this server: ${url}`);
  }
  const response = await fetch(url, { headers: authHeaders() });
  onUnauthorized(response);
  let body: any = null;
  try {
    body = await response.json();
  } catch {
    // An empty or non-JSON body; the status says enough.
  }
  if (!response.ok) {
    throw new Error(body?.error || `${response.status} ${response.statusText}`);
  }
  return body;
}

type OpenWindowOptions = {
  url: string;
  width?: number;
  height?: number;
  title?: string;
};

export function openWindow({
  url,
  title = '',
  width = 640,
  height = 720,
}: OpenWindowOptions) {
  const dualScreenLeft = window.screenLeft !== undefined ? window.screenLeft : window.screenX;
  const dualScreenTop = window.screenTop !== undefined ? window.screenTop : window.screenY;

  const w = window.innerWidth || document.documentElement.clientWidth || screen.width;
  const h = window.innerHeight || document.documentElement.clientHeight || screen.height;

  const systemZoom = w / window.screen.availWidth;
  const left = (w - width) / 2 / systemZoom + dualScreenLeft;
  const top = (h - height) / 2 / systemZoom + dualScreenTop;

  const newWindow = window.open(
    url,
    title,
    `
      scrollbars=yes,
      width=${width / systemZoom},
      height=${height / systemZoom},
      top=${top},
      left=${left}
    `
  );

  newWindow?.focus?.();

  return newWindow;
}
