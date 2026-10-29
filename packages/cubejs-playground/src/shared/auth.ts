// With RBAC on, `playground/context` hands out no token (`authRequired`):
// a user logs in, and the token goes on every request after. Platform admins
// see the whole Playground; anyone else gets the query builder and the
// Frontend Integrations page, scoped to a workspace they pick.
// Both live in sessionStorage, so they end with the tab.

const STORAGE_KEY = 'cubePlaygroundSession';
const WORKSPACE_KEY = 'cubePlaygroundWorkspace';

export const WORKSPACE_HEADER = 'X-Cube-Workspace';

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

function read(key: string): string | null {
  try {
    return storage()?.getItem(key) ?? null;
  } catch {
    return null;
  }
}

function write(key: string, value: string | null) {
  try {
    if (value == null) {
      storage()?.removeItem(key);
    } else {
      storage()?.setItem(key, value);
    }
  } catch {
    // Storage full or blocked: the value lasts until the next reload.
  }
}

// Whether this page has seen a session: a 401 after it expired still has
// to go back to the login.
let hadSession = false;
// The last session token this page saw, to tell it from one typed in.
let lastToken: string | null = null;
// The workspace when sessionStorage cannot keep it (blocked storage): it
// lasts as long as the page.
let memoryWorkspace: string | null = null;
// One reload is enough, however many requests answer 401 at once.
let reloading = false;

export function getSession(): Session | null {
  const raw = read(STORAGE_KEY);
  if (!raw) {
    return null;
  }
  try {
    const session: Session = JSON.parse(raw);
    if (session.token && Date.parse(session.expiresAt) > Date.now()) {
      hadSession = true;
      lastToken = session.token;
      return session;
    }
  } catch {
    // A value this page did not write; drop it.
  }
  logout();
  return null;
}

export function logout() {
  write(STORAGE_KEY, null);
  write(WORKSPACE_KEY, null);
  memoryWorkspace = null;
}

// Whether a 401 for this token means the session is gone; a token typed into
// the API console failing says nothing about the session.
export function isSessionToken(token: string | null | undefined): boolean {
  getSession();
  return Boolean(token) && token === lastToken;
}

export function authHeaders(): Record<string, string> {
  const session = getSession();
  return session ? { Authorization: session.token } : {};
}

// The workspace data requests are scoped to; `null` sends no header, which
// only platform admins may do ("Todos (admin)").
export function getWorkspace(): string | null {
  if (storage() == null) {
    return memoryWorkspace;
  }
  try {
    return storage()?.getItem(WORKSPACE_KEY) || null;
  } catch {
    return memoryWorkspace;
  }
}

const workspaceListeners = new Set<() => void>();

export function onWorkspaceChange(listener: () => void): () => void {
  workspaceListeners.add(listener);
  return () => {
    workspaceListeners.delete(listener);
  };
}

// Returns whether the choice outlives a reload (sessionStorage took it).
export function setWorkspace(id: string | null): boolean {
  memoryWorkspace = id || null;
  write(WORKSPACE_KEY, memoryWorkspace);
  workspaceListeners.forEach((listener) => listener());
  let stored: string | null;
  try {
    stored = storage()?.getItem(WORKSPACE_KEY) ?? null;
  } catch {
    return false;
  }
  return storage() != null && stored === memoryWorkspace;
}

// Switch workspace. With the choice in sessionStorage the page reloads, so
// everything starts over in the new workspace; without storage a reload
// would forget it (and pick again, forever), so the page applies it live:
// the Cube clients follow `onWorkspaceChange`, and other requests read the
// workspace when they are sent.
export function chooseWorkspace(id: string | null, reload = () => window.location.reload()) {
  if (setWorkspace(id)) {
    reload();
  }
}

export function workspaceHeaders(): Record<string, string> {
  const workspace = getWorkspace();
  return workspace ? { [WORKSPACE_HEADER]: workspace } : {};
}

// A request answered 401 while logged in: the token expired or was revoked.
// A 403 is a refusal for this user, not a lost session: callers show it.
export function onUnauthorized(response: { status: number }) {
  if (response.status !== 401 || reloading) {
    return;
  }
  if (getSession() || hadSession) {
    reloading = true;
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

  const session: Session = {
    token: body.token,
    expiresAt: body.expiresAt,
    user: { ...body.user, platformAdmin: Boolean(body.user?.platformAdmin) },
  };
  write(STORAGE_KEY, JSON.stringify(session));
  lastToken = session.token;
  // A workspace picked by whoever used this tab before.
  write(WORKSPACE_KEY, null);
  memoryWorkspace = null;
  return session;
}
