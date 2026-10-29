// With RBAC on, `playground/context` hands out no token (`authRequired`):
// a platform admin logs in, and the token goes on every request after.
// It lives in sessionStorage, so it ends with the tab.

const STORAGE_KEY = 'cubePlaygroundSession';

export type Session = {
  token: string;
  expiresAt: string;
  user: { id: string; name: string | null; platformAdmin: boolean };
};

function storage(): Storage | null {
  try {
    return window.sessionStorage;
  } catch {
    return null;
  }
}

export function getSession(): Session | null {
  const raw = storage()?.getItem(STORAGE_KEY);
  if (!raw) {
    return null;
  }
  try {
    const session: Session = JSON.parse(raw);
    if (session.token && Date.parse(session.expiresAt) > Date.now()) {
      return session;
    }
  } catch {
    // A value this page did not write; drop it.
  }
  logout();
  return null;
}

export function logout() {
  storage()?.removeItem(STORAGE_KEY);
}

export function authHeaders(): Record<string, string> {
  const session = getSession();
  return session ? { Authorization: session.token } : {};
}

// A request answered 401 while logged in: the token expired or was revoked.
export function onUnauthorized(response: Response) {
  if (response.status === 401 && getSession()) {
    logout();
    window.location.reload();
  }
}

export async function login(
  basePath: string,
  username: string,
  password: string
): Promise<Session> {
  const response = await fetch(`${basePath}/v1/auth/login`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ username, password }),
  });
  let body: any = null;
  try {
    body = await response.json();
  } catch {
    // An empty or non-JSON body; the status says enough.
  }
  if (!response.ok) {
    throw new Error(body?.error || `${response.status} ${response.statusText}`);
  }
  if (!body?.user?.platformAdmin) {
    throw new Error('The Playground is for platform admins');
  }

  const session: Session = {
    token: body.token,
    expiresAt: body.expiresAt,
    user: body.user,
  };
  storage()?.setItem(STORAGE_KEY, JSON.stringify(session));
  return session;
}
